//! Minimal Parquet writer: one row group, one PLAIN uncompressed v1 data page
//! per column, Thrift compact metadata. Supports required BYTE_ARRAY (UTF8),
//! optional BYTE_ARRAY (UTF8), required INT32 (DATE) and required INT64.
use std::io::Write;

pub enum Data {
    /// strings as concatenated bytes + end offsets; `def` = Some(levels) for optional
    Str { bytes: Vec<u8>, ends: Vec<u32>, def: Option<Vec<bool>> },
    Date(Vec<i32>),
    I64(Vec<i64>),
}

pub struct Column {
    pub name: &'static str,
    pub data: Data,
}

// ---- thrift compact protocol
struct T {
    b: Vec<u8>,
    last: Vec<i16>,
}
const CT_I32: u8 = 5;
const CT_I64: u8 = 6;
const CT_BIN: u8 = 8;
const CT_LIST: u8 = 9;
const CT_STRUCT: u8 = 12;

impl T {
    fn new() -> T {
        T { b: Vec::new(), last: vec![0] }
    }
    fn varint(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.b.push((v as u8) | 0x80);
            v >>= 7;
        }
        self.b.push(v as u8);
    }
    fn zz(&mut self, v: i64) {
        self.varint(((v << 1) ^ (v >> 63)) as u64);
    }
    fn field(&mut self, id: i16, ty: u8) {
        let last = *self.last.last().unwrap();
        let d = id - last;
        if d > 0 && d <= 15 {
            self.b.push(((d as u8) << 4) | ty);
        } else {
            self.b.push(ty);
            self.zz(id as i64);
        }
        *self.last.last_mut().unwrap() = id;
    }
    fn i32(&mut self, id: i16, v: i32) {
        self.field(id, CT_I32);
        self.zz(v as i64);
    }
    fn i64(&mut self, id: i16, v: i64) {
        self.field(id, CT_I64);
        self.zz(v);
    }
    fn str(&mut self, id: i16, s: &str) {
        self.field(id, CT_BIN);
        self.varint(s.len() as u64);
        self.b.extend_from_slice(s.as_bytes());
    }
    fn begin(&mut self, id: i16) {
        self.field(id, CT_STRUCT);
        self.last.push(0);
    }
    /// a struct as a list element
    fn begin_elem(&mut self) {
        self.last.push(0);
    }
    fn end(&mut self) {
        self.b.push(0);
        self.last.pop();
    }
    fn list(&mut self, id: i16, ety: u8, n: usize) {
        self.field(id, CT_LIST);
        if n < 15 {
            self.b.push(((n as u8) << 4) | ety);
        } else {
            self.b.push(0xF0 | ety);
            self.varint(n as u64);
        }
    }
}

const TYPE_INT32: i32 = 1;
const TYPE_INT64: i32 = 2;
const TYPE_BYTE_ARRAY: i32 = 6;

fn rle_levels(def: &[bool], out: &mut Vec<u8>) {
    // RLE/bit-packed hybrid, bit width 1, as bit-packed groups of 8 values.
    let groups = def.len().div_ceil(8);
    let mut body = Vec::with_capacity(groups + 8);
    // header: (number of groups << 1) | 1, varint
    let mut h = ((groups as u64) << 1) | 1;
    while h >= 0x80 {
        body.push((h as u8) | 0x80);
        h >>= 7;
    }
    body.push(h as u8);
    for g in 0..groups {
        let mut byte = 0u8;
        for i in 0..8 {
            let j = g * 8 + i;
            if j < def.len() && def[j] {
                byte |= 1 << i;
            }
        }
        body.push(byte);
    }
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
}

pub fn write(path: &std::path::Path, cols: &[Column], rows: usize) -> std::io::Result<()> {
    let mut file: Vec<u8> = Vec::new();
    file.extend_from_slice(b"PAR1");
    struct Meta {
        offset: i64,
        size: i64,
        ty: i32,
    }
    let mut metas = Vec::new();
    for c in cols {
        let mut page: Vec<u8> = Vec::new();
        let ty = match &c.data {
            Data::Str { bytes, ends, def } => {
                if let Some(d) = def {
                    rle_levels(d, &mut page);
                }
                page.reserve(bytes.len() + ends.len() * 4);
                let mut s = 0usize;
                for &e in ends {
                    let e = e as usize;
                    page.extend_from_slice(&((e - s) as u32).to_le_bytes());
                    page.extend_from_slice(&bytes[s..e]);
                    s = e;
                }
                TYPE_BYTE_ARRAY
            }
            Data::Date(v) => {
                page.reserve(v.len() * 4);
                for x in v {
                    page.extend_from_slice(&x.to_le_bytes());
                }
                TYPE_INT32
            }
            Data::I64(v) => {
                page.reserve(v.len() * 8);
                for x in v {
                    page.extend_from_slice(&x.to_le_bytes());
                }
                TYPE_INT64
            }
        };
        // page header
        let mut h = T::new();
        h.i32(1, 0); // DATA_PAGE
        h.i32(2, page.len() as i32);
        h.i32(3, page.len() as i32);
        h.begin(5);
        h.i32(1, rows as i32);
        h.i32(2, 0); // PLAIN
        h.i32(3, 3); // RLE
        h.i32(4, 3); // RLE
        h.end();
        h.b.push(0);
        let offset = file.len() as i64;
        file.extend_from_slice(&h.b);
        file.extend_from_slice(&page);
        metas.push(Meta { offset, size: (h.b.len() + page.len()) as i64, ty });
    }
    // file metadata
    let mut t = T::new();
    t.i32(1, 1);
    t.list(2, CT_STRUCT, cols.len() + 1);
    t.begin_elem();
    t.str(4, "schema");
    t.i32(5, cols.len() as i32);
    t.end();
    for c in cols {
        t.begin_elem();
        let (ty, rep, conv, logical) = match &c.data {
            Data::Str { def, .. } => (TYPE_BYTE_ARRAY, if def.is_some() { 1 } else { 0 }, Some(0), Some(1)),
            Data::Date(_) => (TYPE_INT32, 0, Some(6), Some(6)),
            Data::I64(_) => (TYPE_INT64, 0, None, None),
        };
        t.i32(1, ty);
        t.i32(3, rep);
        t.str(4, c.name);
        if let Some(cv) = conv {
            t.i32(6, cv);
        }
        if let Some(lt) = logical {
            t.begin(10);
            t.begin(lt); // StringType (1) / DateType (6): empty structs
            t.end();
            t.end();
        }
        t.end();
    }
    t.i64(3, rows as i64);
    t.list(4, CT_STRUCT, 1);
    t.begin_elem();
    t.list(1, CT_STRUCT, cols.len());
    let total: i64 = metas.iter().map(|m| m.size).sum();
    for (c, m) in cols.iter().zip(metas.iter()) {
        t.begin_elem();
        t.i64(2, m.offset);
        t.begin(3);
        t.i32(1, m.ty);
        t.list(2, CT_I32, 2);
        t.zz(0); // PLAIN
        t.zz(3); // RLE
        t.list(3, CT_BIN, 1);
        t.varint(c.name.len() as u64);
        t.b.extend_from_slice(c.name.as_bytes());
        t.i32(4, 0); // UNCOMPRESSED
        t.i64(5, rows as i64);
        t.i64(6, m.size);
        t.i64(7, m.size);
        t.i64(9, m.offset);
        t.end();
        t.end();
    }
    t.i64(2, total);
    t.i64(3, rows as i64);
    t.end();
    t.str(6, "sessions-rs");
    t.b.push(0);
    file.extend_from_slice(&t.b);
    file.extend_from_slice(&(t.b.len() as u32).to_le_bytes());
    file.extend_from_slice(b"PAR1");
    let mut f = std::fs::File::create(path)?;
    f.write_all(&file)?;
    Ok(())
}
