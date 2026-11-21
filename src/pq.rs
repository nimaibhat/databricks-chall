//! Minimal Parquet writer: row groups appended one at a time, uncompressed, v1 data pages, Thrift
//! compact metadata, no statistics. A column is either PLAIN, or dictionary
//! encoded (a PLAIN dictionary page followed by data pages of RLE /
//! bit-packed dictionary indices), which is what every Parquet writer emits
//! by default and so what every reader handles.
use std::io::Write;

/// PLAIN-encodable values.
pub enum Values {
    /// strings as concatenated bytes + end offsets
    Str { bytes: Vec<u8>, ends: Vec<u32> },
    I32(Vec<i32>),
    I64(Vec<i64>),
}

impl Values {
    fn len(&self) -> usize {
        match self {
            Values::Str { ends, .. } => ends.len(),
            Values::I32(v) => v.len(),
            Values::I64(v) => v.len(),
        }
    }
    fn plain_size(&self) -> usize {
        match self {
            Values::Str { bytes, ends } => bytes.len() + ends.len() * 4,
            Values::I32(v) => v.len() * 4,
            Values::I64(v) => v.len() * 8,
        }
    }
    fn write_plain(&self, out: &mut Vec<u8>) {
        match self {
            Values::Str { bytes, ends } => {
                let mut s = 0usize;
                for &e in ends {
                    let e = e as usize;
                    out.extend_from_slice(&((e - s) as u32).to_le_bytes());
                    out.extend_from_slice(&bytes[s..e]);
                    s = e;
                }
            }
            Values::I32(v) => {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            Values::I64(v) => {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
        }
    }
    fn ptype(&self) -> i32 {
        match self {
            Values::Str { .. } => TYPE_BYTE_ARRAY,
            Values::I32(_) => TYPE_INT32,
            Values::I64(_) => TYPE_INT64,
        }
    }
}

/// Dictionary indices of the non-null values, in row order.
pub enum Idx {
    /// (index, repeat count) runs; written as RLE runs in one page
    Runs(Vec<(u32, u32)>),
    /// one index per value; written bit-packed, in pages of PAGE_ROWS values
    /// (each with its own bit width) so that a few large indices do not
    /// widen every value
    Each(Vec<u32>),
}

pub enum Enc {
    Plain(Values),
    Dict { dict: Values, idx: Idx },
}

#[derive(Clone, Copy, PartialEq)]
pub enum Logical {
    None,
    Utf8,
    Date,
}

pub struct Column {
    pub name: &'static str,
    pub logical: Logical,
    /// Some(present flags) for an optional column; values cover present rows only
    pub def: Option<Vec<bool>>,
    pub enc: Enc,
}

const PAGE_ROWS: usize = 2048;

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
const ENC_PLAIN: i32 = 0;
const ENC_PLAIN_DICTIONARY: i32 = 2;
const ENC_RLE: i32 = 3;

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Bits needed for values up to `max` (at least 1).
fn width(max: u32) -> u32 {
    (32 - max.leading_zeros()).max(1)
}

/// RLE / bit-packed hybrid: `vals` as bit-packed groups of 8, width `w`.
fn bit_packed(vals: &[u32], w: u32, out: &mut Vec<u8>) {
    let groups = vals.len().div_ceil(8);
    varint(out, ((groups as u64) << 1) | 1);
    let mut acc: u64 = 0;
    let mut nbits: u32 = 0;
    for i in 0..groups * 8 {
        let v = if i < vals.len() { vals[i] as u64 } else { 0 };
        acc |= v << nbits;
        nbits += w;
        while nbits >= 8 {
            out.push(acc as u8);
            acc >>= 8;
            nbits -= 8;
        }
    }
    debug_assert!(nbits == 0); // 8 values of w bits are a whole number of bytes
}

/// Definition levels (bit width 1) with their 4-byte length prefix.
fn def_levels(def: &[bool], out: &mut Vec<u8>) {
    let at = out.len();
    out.extend_from_slice(&[0; 4]);
    let groups = def.len().div_ceil(8);
    varint(out, ((groups as u64) << 1) | 1);
    for g in 0..groups {
        let mut byte = 0u8;
        for i in 0..8 {
            let j = g * 8 + i;
            if j < def.len() && def[j] {
                byte |= 1 << i;
            }
        }
        out.push(byte);
    }
    let n = (out.len() - at - 4) as u32;
    out[at..at + 4].copy_from_slice(&n.to_le_bytes());
}

fn data_page_header(size: usize, rows: usize, encoding: i32) -> Vec<u8> {
    let mut h = T::new();
    h.i32(1, 0); // DATA_PAGE
    h.i32(2, size as i32);
    h.i32(3, size as i32);
    h.begin(5);
    h.i32(1, rows as i32);
    h.i32(2, encoding);
    h.i32(3, ENC_RLE);
    h.i32(4, ENC_RLE);
    h.end();
    h.b.push(0);
    h.b
}

struct ChunkMeta {
    offset: i64, // first page of the chunk
    data_offset: i64,
    dict_offset: Option<i64>,
    size: i64,
    ty: i32,
    has_dict: bool,
}

/// A Parquet file written one row group at a time, so that output can leave
/// the process (and the kernel can start writing it out) while later rows
/// are still being computed. Every row group is self-contained: its own
/// dictionaries, its own pages.
pub struct FileWriter {
    f: std::fs::File,
    off: u64,
    /// (name, physical type, optional, logical) of each column, from the first group
    schema: Vec<(&'static str, i32, bool, Logical)>,
    groups: Vec<(Vec<ChunkMeta>, usize)>,
    buf: Vec<u8>,
}

impl FileWriter {
    pub fn create(path: &std::path::Path) -> std::io::Result<FileWriter> {
        let mut f = std::fs::File::create(path)?;
        f.write_all(b"PAR1")?;
        Ok(FileWriter { f, off: 4, schema: Vec::new(), groups: Vec::new(), buf: Vec::new() })
    }
    #[cfg(unix)]
    pub fn raw_fd(&self) -> i32 {
        use std::os::unix::io::AsRawFd;
        self.f.as_raw_fd()
    }
    /// Appends one row group; returns (file offset, length) of its bytes.
    pub fn write_group(&mut self, cols: &[Column], rows: usize) -> std::io::Result<(u64, u64)> {
        let file = &mut self.buf;
        file.clear();
        let base = self.off as i64;
        let mut metas = Vec::with_capacity(cols.len());
        let mut page: Vec<u8> = Vec::new();
        for c in cols {
            let offset = base + file.len() as i64;
            let (ty, data_offset, dict_offset) = match &c.enc {
                Enc::Plain(v) => {
                    page.clear();
                    if let Some(d) = &c.def {
                        def_levels(d, &mut page);
                    }
                    v.write_plain(&mut page);
                    file.extend_from_slice(&data_page_header(page.len(), rows, ENC_PLAIN));
                    file.extend_from_slice(&page);
                    (v.ptype(), offset, None)
                }
                Enc::Dict { dict, idx } => {
                    // dictionary page
                    let mut h = T::new();
                    h.i32(1, 2); // DICTIONARY_PAGE
                    h.i32(2, dict.plain_size() as i32);
                    h.i32(3, dict.plain_size() as i32);
                    h.begin(7);
                    h.i32(1, dict.len() as i32);
                    h.i32(2, ENC_PLAIN_DICTIONARY);
                    h.end();
                    h.b.push(0);
                    file.extend_from_slice(&h.b);
                    dict.write_plain(file);
                    let data_offset = base + file.len() as i64;
                    match idx {
                        Idx::Runs(runs) => {
                            page.clear();
                            if let Some(d) = &c.def {
                                def_levels(d, &mut page);
                            }
                            let w = width(runs.iter().map(|r| r.0).max().unwrap_or(0));
                            let nb = w.div_ceil(8) as usize;
                            page.push(w as u8);
                            for &(v, n) in runs {
                                varint(&mut page, (n as u64) << 1);
                                page.extend_from_slice(&v.to_le_bytes()[..nb]);
                            }
                            file.extend_from_slice(&data_page_header(page.len(), rows, ENC_PLAIN_DICTIONARY));
                            file.extend_from_slice(&page);
                        }
                        Idx::Each(v) => {
                            if c.def.is_some() || v.is_empty() {
                                // one page: the levels cover all rows
                                page.clear();
                                if let Some(d) = &c.def {
                                    def_levels(d, &mut page);
                                }
                                let w = width(v.iter().copied().max().unwrap_or(0));
                                page.push(w as u8);
                                if !v.is_empty() {
                                    bit_packed(v, w, &mut page);
                                }
                                file.extend_from_slice(&data_page_header(page.len(), rows, ENC_PLAIN_DICTIONARY));
                                file.extend_from_slice(&page);
                            } else {
                                for part in v.chunks(PAGE_ROWS) {
                                    page.clear();
                                    let w = width(part.iter().copied().max().unwrap_or(0));
                                    page.push(w as u8);
                                    bit_packed(part, w, &mut page);
                                    file.extend_from_slice(&data_page_header(page.len(), part.len(), ENC_PLAIN_DICTIONARY));
                                    file.extend_from_slice(&page);
                                }
                            }
                        }
                    }
                    (dict.ptype(), data_offset, Some(offset))
                }
            };
            metas.push(ChunkMeta { offset, data_offset, dict_offset, size: base + file.len() as i64 - offset, ty, has_dict: dict_offset.is_some() });
        }
        if self.schema.is_empty() {
            self.schema = cols.iter().zip(metas.iter()).map(|(c, m)| (c.name, m.ty, c.def.is_some(), c.logical)).collect();
        } else {
            // every group must have the same columns with the same physical types
            assert!(self.schema.len() == cols.len() && self.schema.iter().zip(cols.iter().zip(metas.iter())).all(|(s, (c, m))| s.0 == c.name && s.1 == m.ty && s.2 == c.def.is_some()));
        }
        self.groups.push((metas, rows));
        let at = self.off;
        self.f.write_all(&self.buf)?;
        self.off += self.buf.len() as u64;
        Ok((at, self.buf.len() as u64))
    }
    /// Writes the footer. The file handle is returned still open.
    pub fn finish(mut self) -> std::io::Result<std::fs::File> {
        let mut t = T::new();
        t.i32(1, 1);
        t.list(2, CT_STRUCT, self.schema.len() + 1);
        t.begin_elem();
        t.str(4, "schema");
        t.i32(5, self.schema.len() as i32);
        t.end();
        for &(name, ty, optional, logical) in &self.schema {
            t.begin_elem();
            t.i32(1, ty);
            t.i32(3, if optional { 1 } else { 0 });
            t.str(4, name);
            match logical {
                Logical::Utf8 => {
                    t.i32(6, 0);
                    t.begin(10);
                    t.begin(1); // StringType: empty struct
                    t.end();
                    t.end();
                }
                Logical::Date => {
                    t.i32(6, 6);
                    t.begin(10);
                    t.begin(6); // DateType: empty struct
                    t.end();
                    t.end();
                }
                Logical::None => {}
            }
            t.end();
        }
        let total_rows: usize = self.groups.iter().map(|g| g.1).sum();
        t.i64(3, total_rows as i64);
        t.list(4, CT_STRUCT, self.groups.len());
        for (metas, rows) in &self.groups {
            t.begin_elem();
            t.list(1, CT_STRUCT, metas.len());
            let total: i64 = metas.iter().map(|m| m.size).sum();
            for (m, sc) in metas.iter().zip(self.schema.iter()) {
                t.begin_elem();
                t.i64(2, m.offset);
                t.begin(3);
                t.i32(1, m.ty);
                if m.has_dict {
                    t.list(2, CT_I32, 3);
                    t.zz(ENC_PLAIN as i64);
                    t.zz(ENC_RLE as i64);
                    t.zz(ENC_PLAIN_DICTIONARY as i64);
                } else {
                    t.list(2, CT_I32, 2);
                    t.zz(ENC_PLAIN as i64);
                    t.zz(ENC_RLE as i64);
                }
                t.list(3, CT_BIN, 1);
                t.varint(sc.0.len() as u64);
                t.b.extend_from_slice(sc.0.as_bytes());
                t.i32(4, 0); // UNCOMPRESSED
                t.i64(5, *rows as i64);
                t.i64(6, m.size);
                t.i64(7, m.size);
                t.i64(9, m.data_offset);
                if let Some(d) = m.dict_offset {
                    t.i64(11, d);
                }
                t.end();
                t.end();
            }
            t.i64(2, total);
            t.i64(3, *rows as i64);
            t.end();
        }
        t.str(6, "sessions-rs");
        t.b.push(0);
        let mut tail = t.b;
        let n = tail.len() as u32;
        tail.extend_from_slice(&n.to_le_bytes());
        tail.extend_from_slice(b"PAR1");
        self.f.write_all(&tail)?;
        Ok(self.f)
    }
}

/// One file with one row group.
pub fn write(path: &std::path::Path, cols: &[Column], rows: usize) -> std::io::Result<()> {
    let mut w = FileWriter::create(path)?;
    w.write_group(cols, rows)?;
    w.finish()?;
    Ok(())
}
