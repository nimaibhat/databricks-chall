#![allow(dead_code)]
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::Path;

#[derive(Clone, Copy)]
pub struct StrCol<'a> {
    pub offsets: &'a [u32],
    pub data: &'a [u8],
}
impl<'a> StrCol<'a> {
    pub fn len(&self) -> usize { self.offsets.len().saturating_sub(1) }
    #[inline]
    pub fn get(&self, i: usize) -> &'a [u8] {
        &self.data[self.offsets[i] as usize..self.offsets[i + 1] as usize]
    }
    fn bytes_in(&self, a: usize, b: usize) -> usize {
        if b <= a { 0 } else { (self.offsets[b] - self.offsets[a]) as usize }
    }
    fn slice(&self, a: usize, b: usize) -> StrCol<'a> {
        if self.offsets.is_empty() { return *self; }
        StrCol { offsets: &self.offsets[a..b + 1], data: self.data }
    }
}

#[derive(Clone, Copy)]
pub enum Keys<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
    U32(&'a [u32]),
    U32Off(&'a [u32], u32),
}
impl<'a> Keys<'a> {
    pub fn len(&self) -> usize {
        match self { Keys::U8(k) => k.len(), Keys::U16(k) => k.len(), Keys::U32(k) => k.len(), Keys::U32Off(k, _) => k.len() }
    }
    fn slice(&self, a: usize, b: usize) -> Keys<'a> {
        match self { Keys::U8(k) => Keys::U8(&k[a..b]), Keys::U16(k) => Keys::U16(&k[a..b]), Keys::U32(k) => Keys::U32(&k[a..b]), Keys::U32Off(k, o) => Keys::U32Off(&k[a..b], *o) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntEnc { Plain, Dense, Hash, Auto }

#[derive(Clone, Copy)]
pub enum Data<'a> {
    I32(&'a [i32], IntEnc),
    I64(&'a [i64], IntEnc),
    Date(&'a [i32], IntEnc),
    Str(StrCol<'a>),
    Dict { dict: StrCol<'a>, keys: Keys<'a>, nullable: bool, plain: bool },
}

pub struct Column<'a> {
    pub name: &'a str,
    pub data: Data<'a>,
}

#[derive(Clone, Copy)]
pub struct Opts {
    pub rows_per_group: usize,
}
impl Default for Opts {
    fn default() -> Self { Opts { rows_per_group: usize::MAX } }
}

#[derive(Clone, Copy, Debug)]
pub enum Method {
    Whole,
    Stream(usize),
    Mmap { populate: bool },
}


struct Out {
    buf: Vec<u8>,
    flushed: u64,
    file: Option<File>,
    limit: usize,
}
impl Out {
    fn mem(cap: usize) -> Out { Out { buf: Vec::with_capacity(cap), flushed: 0, file: None, limit: usize::MAX } }
    #[inline]
    fn pos(&self) -> u64 { self.flushed + self.buf.len() as u64 }
    #[inline]
    fn put(&mut self, b: &[u8]) { self.buf.extend_from_slice(b); }
    fn flush(&mut self) -> io::Result<()> {
        if let Some(f) = self.file.as_mut() {
            if !self.buf.is_empty() {
                #[cfg(feature = "tl")]
                let t0 = crate::sys::tl::now();
                #[cfg(feature = "tl")]
                let skip = crate::out::ev::NOIO.load(std::sync::atomic::Ordering::Relaxed);
                #[cfg(not(feature = "tl"))]
                let skip = false;
                if !skip { f.write_all(&self.buf)?; }
                #[cfg(feature = "tl")]
                { crate::out::ev::WN.fetch_add(1, std::sync::atomic::Ordering::Relaxed); crate::out::ev::WB.fetch_add(self.buf.len() as u64, std::sync::atomic::Ordering::Relaxed); }
                #[cfg(feature = "tl")]
                crate::sys::tl::WR_IO.fetch_add(crate::sys::tl::now() - t0, std::sync::atomic::Ordering::Relaxed);
                self.flushed += self.buf.len() as u64;
                self.buf.clear();
            }
        }
        Ok(())
    }
    #[inline]
    fn check(&mut self) -> io::Result<()> {
        if self.file.is_some() && self.buf.len() >= self.limit { self.flush() } else { Ok(()) }
    }
    fn raw(&mut self, b: &[u8]) -> io::Result<()> {
        if self.file.is_some() && b.len() >= self.limit {
            self.flush()?;
            #[cfg(feature = "tl")]
            let t0 = crate::sys::tl::now();
            #[cfg(feature = "tl")]
            let skip = crate::out::ev::NOIO.load(std::sync::atomic::Ordering::Relaxed);
            #[cfg(not(feature = "tl"))]
            let skip = false;
            if !skip { self.file.as_mut().unwrap().write_all(b)?; }
            #[cfg(feature = "tl")]
            crate::sys::tl::WR_IO.fetch_add(crate::sys::tl::now() - t0, std::sync::atomic::Ordering::Relaxed);
            #[cfg(feature = "tl")]
            { crate::out::ev::WN.fetch_add(1, std::sync::atomic::Ordering::Relaxed); crate::out::ev::WB.fetch_add(b.len() as u64, std::sync::atomic::Ordering::Relaxed); }
            self.flushed += b.len() as u64;
            return Ok(());
        }
        self.put(b);
        self.check()
    }
    #[inline]
    unsafe fn tail(&mut self, n: usize) -> *mut u8 {
        self.buf.reserve(n + 32);
        self.buf.as_mut_ptr().add(self.buf.len())
    }
    #[inline]
    unsafe fn commit(&mut self, n: usize) { let l = self.buf.len(); self.buf.set_len(l + n); }
}


struct Tw<'a> {
    b: &'a mut Vec<u8>,
    stack: Vec<i16>,
    last: i16,
}
impl<'a> Tw<'a> {
    fn new(b: &'a mut Vec<u8>) -> Self { Tw { b, stack: Vec::new(), last: 0 } }
    #[inline]
    fn varint(&mut self, mut v: u64) {
        while v >= 0x80 { self.b.push(v as u8 | 0x80); v >>= 7; }
        self.b.push(v as u8);
    }
    fn fh(&mut self, id: i16, t: u8) {
        let d = id - self.last;
        if d > 0 && d <= 15 { self.b.push(((d as u8) << 4) | t); } else { self.b.push(t); self.varint(zz32(id as i32)); }
        self.last = id;
    }
    fn i32(&mut self, id: i16, v: i32) { self.fh(id, 5); self.varint(zz32(v)); }
    fn i64(&mut self, id: i16, v: i64) { self.fh(id, 6); self.varint(zz64(v)); }
    fn i16(&mut self, id: i16, v: i16) { self.fh(id, 4); self.varint(zz32(v as i32)); }
    fn bin(&mut self, id: i16, s: &[u8]) { self.fh(id, 8); self.varint(s.len() as u64); self.b.extend_from_slice(s); }
    fn begin(&mut self, id: i16) { self.fh(id, 12); self.stack.push(self.last); self.last = 0; }
    fn elem(&mut self) { self.stack.push(self.last); self.last = 0; }
    fn end(&mut self) { self.b.push(0); self.last = self.stack.pop().unwrap_or(0); }
    fn list(&mut self, id: i16, et: u8, n: usize) {
        self.fh(id, 9);
        if n < 15 { self.b.push(((n as u8) << 4) | et); } else { self.b.push(0xF0 | et); self.varint(n as u64); }
    }
    fn stop(&mut self) { self.b.push(0); }
}
#[inline]
fn zz32(v: i32) -> u64 { ((v << 1) ^ (v >> 31)) as u32 as u64 }
#[inline]
fn zz64(v: i64) -> u64 { ((v << 1) ^ (v >> 63)) as u64 }
fn varint_len(mut v: u64) -> usize { let mut n = 1; while v >= 0x80 { v >>= 7; n += 1; } n }
fn put_varint(b: &mut Vec<u8>, mut v: u64) { while v >= 0x80 { b.push(v as u8 | 0x80); v >>= 7; } b.push(v as u8); }

const T_INT32: i32 = 1;
const T_INT64: i32 = 2;
const T_BYTE_ARRAY: i32 = 6;
const E_PLAIN: i32 = 0;
const E_RLE: i32 = 3;
const E_RLE_DICT: i32 = 8;
const P_DATA: i32 = 0;
const P_DICT: i32 = 2;

fn page_header(b: &mut Vec<u8>, ptype: i32, size: usize, nv: usize, enc: i32) {
    assert!(size <= i32::MAX as usize, "page too large; set rows_per_group");
    let mut t = Tw::new(b);
    t.i32(1, ptype);
    t.i32(2, size as i32);
    t.i32(3, size as i32);
    if ptype == P_DATA {
        t.begin(5);
        t.i32(1, nv as i32);
        t.i32(2, enc);
        t.i32(3, E_RLE);
        t.i32(4, E_RLE);
        t.end();
    } else {
        t.begin(7);
        t.i32(1, nv as i32);
        t.i32(2, E_PLAIN);
        t.end();
    }
    t.stop();
}


struct Packer { p: *mut u8, start: *mut u8, acc: u64, nb: u32, w: u32 }
impl Packer {
    #[inline(always)]
    unsafe fn new(p: *mut u8, w: u32) -> Self { Packer { p, start: p, acc: 0, nb: 0, w } }
    #[inline(always)]
    unsafe fn push(&mut self, v: u32) {
        self.acc |= (v as u64) << self.nb;
        self.nb += self.w;
        if self.nb >= 32 {
            (self.p as *mut u32).write_unaligned(self.acc as u32);
            self.p = self.p.add(4);
            self.acc >>= 32;
            self.nb -= 32;
        }
    }
    #[inline(always)]
    unsafe fn finish(self, total: usize) -> usize {
        let written = self.p.offset_from(self.start) as usize;
        let rem = total - written;
        std::ptr::write_bytes(self.p, 0, rem);
        let a = self.acc.to_le_bytes();
        std::ptr::copy_nonoverlapping(a.as_ptr(), self.p, rem.min(8));
        total
    }
}
#[inline]
fn bits_for(max: u64) -> u32 { (64 - max.leading_zeros()).max(1) }
fn hybrid_len(n: usize, w: u32) -> usize {
    if n == 0 { return 0; }
    let g = (n + 7) / 8;
    varint_len(((g as u64) << 1) | 1) + g * w as usize
}
const BLOCK: usize = 8192;

#[inline(always)]
unsafe fn pack_group<const W: u32>(p: *mut u8, v: [u32; 8]) {
    if W <= 8 {
        let mut x = 0u64;
        for k in 0..8 { x |= (v[k] as u64) << (k as u32 * W); }
        (p as *mut u64).write_unaligned(x);
    } else if W <= 16 {
        let mut x = 0u128;
        for k in 0..8 { x |= (v[k] as u128) << (k as u32 * W); }
        (p as *mut u128).write_unaligned(x);
    } else {
        let (mut lo, mut hi) = (0u128, 0u128);
        for k in 0..4 { lo |= (v[k] as u128) << (k as u32 * W); hi |= (v[k + 4] as u128) << (k as u32 * W); }
        if W == 32 {
            (p as *mut u128).write_unaligned(lo);
            (p.add(16) as *mut u128).write_unaligned(hi);
        } else {
            (p as *mut u128).write_unaligned(lo | (hi << (4 * W)));
            (p.add(16) as *mut u128).write_unaligned(hi >> (128 - 4 * W));
        }
    }
}
#[inline(always)]
unsafe fn pack_w<const W: u32, F: Fn(usize) -> u32>(dst: *mut u8, n: usize, get: &F) {
    let full = n / 8;
    let mut p = dst;
    for g in 0..full {
        let b = g * 8;
        let a = [get(b), get(b + 1), get(b + 2), get(b + 3), get(b + 4), get(b + 5), get(b + 6), get(b + 7)];
        put8::<W>(p, _mm256_loadu_si256(std::hint::black_box(a.as_ptr()) as *const __m256i));
        p = p.add(W as usize);
    }
    let rem = n - full * 8;
    if rem > 0 {
        let mut a = [0u32; 8];
        for k in 0..rem { a[k] = get(full * 8 + k); }
        put8::<W>(p, _mm256_loadu_si256(a.as_ptr() as *const __m256i));
    }
}
unsafe fn pack_any<F: Fn(usize) -> u32>(dst: *mut u8, n: usize, w: u32, get: &F) {
    macro_rules! d { ($($k:literal)*) => { match w { $($k => pack_w::<$k, F>(dst, n, get),)* _ => panic!("bit width") } } }
    d!(1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32)
}

unsafe fn emit_packed(o: &mut Out, n: usize, w: u32, get: impl Fn(usize) -> u32) -> io::Result<()> {
    if n == 0 { return Ok(()); }
    let g = (n + 7) / 8;
    let mut hv = Vec::with_capacity(10);
    put_varint(&mut hv, ((g as u64) << 1) | 1);
    o.put(&hv);
    let mut i = 0;
    while i < n {
        let e = (i + BLOCK).min(n);
        let cnt = e - i;
        let bytes = (cnt + 7) / 8 * w as usize;
        let p = o.tail(bytes);
        let base = i;
        pack_any(p, cnt, w, &|j| get(base + j));
        o.commit(bytes);
        o.check()?;
        i = e;
    }
    Ok(())
}


#[derive(Clone, Copy)]
enum Ints<'a> { A(&'a [i32]), B(&'a [i64]) }
impl<'a> Ints<'a> {
    #[inline(always)]
    fn get(&self, i: usize) -> i64 { match self { Ints::A(v) => v[i] as i64, Ints::B(v) => v[i] } }
    fn len(&self) -> usize { match self { Ints::A(v) => v.len(), Ints::B(v) => v.len() } }
    fn esize(&self) -> usize { match self { Ints::A(_) => 4, Ints::B(_) => 8 } }
    fn bytes(&self) -> &'a [u8] {
        unsafe {
            match self {
                Ints::A(v) => std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 4),
                Ints::B(v) => std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 8),
            }
        }
    }
    fn minmax(&self) -> (i64, i64) {
        let mut lo = i64::MAX; let mut hi = i64::MIN;
        match self {
            Ints::A(v) => { let (mut l, mut h) = (i32::MAX, i32::MIN); for &x in v.iter() { l = l.min(x); h = h.max(x); } if !v.is_empty() { lo = l as i64; hi = h as i64; } }
            Ints::B(v) => { for &x in v.iter() { lo = lo.min(x); hi = hi.max(x); } }
        }
        (lo, hi)
    }
}

enum Plan {
    Plain,
    Dense { min: i64, card: usize, w: u32 },
    Hash { dict: Vec<i64>, keys: Vec<u32>, w: u32 },
    Dict { m: usize, vbytes: usize, w: u32 },
}

const DENSE_MAX: u64 = 1 << 20;

fn plan_ints(v: Ints, enc: IntEnc) -> Plan {
    let n = v.len();
    if n == 0 || enc == IntEnc::Plain { return Plan::Plain; }
    if enc == IntEnc::Hash { return plan_hash(v); }
    let (lo, hi) = v.minmax();
    let r = (hi as i128 - lo as i128) as u128;
    if r >= DENSE_MAX as u128 { return Plan::Plain; }
    let card = r as usize + 1;
    let w = bits_for(r as u64);
    if enc == IntEnc::Auto {
        let es = v.esize();
        let dense = card * es + (n + 7) / 8 * w as usize;
        if dense * 10 > n * es * 8 { return Plan::Plain; }
    }
    Plan::Dense { min: lo, card, w }
}

fn plan_hash(v: Ints) -> Plan {
    let n = v.len();
    let limit = (n / 2).max(16);
    let mut cap = 1024usize;
    let mut slots: Vec<u32> = vec![u32::MAX; cap];
    let mut dict: Vec<i64> = Vec::new();
    let mut keys: Vec<u32> = Vec::with_capacity(n);
    let mut shift = 64 - cap.trailing_zeros();
    for i in 0..n {
        let x = v.get(i);
        let mut h = ((x as u64).wrapping_mul(0x9E3779B97F4A7C15) >> shift) as usize;
        loop {
            let s = slots[h];
            if s == u32::MAX {
                if dict.len() >= limit { return Plan::Plain; }
                slots[h] = dict.len() as u32;
                keys.push(dict.len() as u32);
                dict.push(x);
                if dict.len() * 2 > cap {
                    cap *= 2;
                    shift -= 1;
                    slots = vec![u32::MAX; cap];
                    for (j, &d) in dict.iter().enumerate() {
                        let mut h2 = ((d as u64).wrapping_mul(0x9E3779B97F4A7C15) >> shift) as usize;
                        while slots[h2] != u32::MAX { h2 = (h2 + 1) & (cap - 1); }
                        slots[h2] = j as u32;
                    }
                }
                break;
            }
            if dict[s as usize] == x { keys.push(s); break; }
            h = (h + 1) & (cap - 1);
        }
    }
    let w = bits_for(dict.len().saturating_sub(1) as u64);
    Plan::Hash { dict, keys, w }
}

trait Key: Copy { const NUL: Self; fn u(self) -> u32; }
impl Key for u8 { const NUL: u8 = u8::MAX; #[inline(always)] fn u(self) -> u32 { self as u32 } }
impl Key for u16 { const NUL: u16 = u16::MAX; #[inline(always)] fn u(self) -> u32 { self as u32 } }
impl Key for u32 { const NUL: u32 = u32::MAX; #[inline(always)] fn u(self) -> u32 { self } }

fn plan_dict_t<K: Key + PartialEq>(dict: &StrCol, keys: &[K], nullable: bool, plain: bool) -> Plan {
    let mut m = 0usize;
    let mut vbytes = 0usize;
    if plain {
        for &k in keys { if !(nullable && k == K::NUL) { m += 1; vbytes += 4 + dict.get(k.u() as usize).len(); } }
    } else if nullable {
        for &k in keys { m += (k != K::NUL) as usize; }
    } else { m = keys.len(); }
    let w = bits_for(dict.len().saturating_sub(1) as u64);
    Plan::Dict { m, vbytes, w }
}

fn plan(d: &Data) -> Plan {
    match *d {
        Data::I32(v, e) | Data::Date(v, e) => plan_ints(Ints::A(v), e),
        Data::I64(v, e) => plan_ints(Ints::B(v), e),
        Data::Str(_) => Plan::Plain,
        Data::Dict { dict, keys, nullable, plain } => match keys {
            Keys::U8(k) => plan_dict_t(&dict, k, nullable, plain),
            Keys::U16(k) => plan_dict_t(&dict, k, nullable, plain),
            Keys::U32(k) => plan_dict_t(&dict, k, nullable, plain),
            Keys::U32Off(k, _) => {
                assert!(!nullable && !plain, "U32Off keys are required dictionary keys");
                Plan::Dict { m: k.len(), vbytes: 0, w: bits_for(dict.len().saturating_sub(1) as u64) }
            }
        },
    }
}

fn rows_of(d: &Data) -> usize {
    match d { Data::I32(v, _) | Data::Date(v, _) => v.len(), Data::I64(v, _) => v.len(), Data::Str(s) => s.len(), Data::Dict { keys, .. } => keys.len() }
}
fn slice_data<'a>(d: &Data<'a>, a: usize, b: usize) -> Data<'a> {
    match *d {
        Data::I32(v, e) => Data::I32(&v[a..b], e),
        Data::Date(v, e) => Data::Date(&v[a..b], e),
        Data::I64(v, e) => Data::I64(&v[a..b], e),
        Data::Str(s) => Data::Str(s.slice(a, b)),
        Data::Dict { dict, keys, nullable, plain } => Data::Dict { dict, keys: keys.slice(a, b), nullable, plain },
    }
}
fn narrow<'a>(d: Data<'a>) -> Data<'a> {
    if let Data::Dict { dict, keys: Keys::U32Off(k, base), nullable, plain } = d {
        if k.is_empty() {
            return Data::Dict { dict: StrCol { offsets: &[], data: dict.data }, keys: Keys::U32Off(k, base), nullable, plain };
        }
        let (mut lo, mut hi) = (u32::MAX, 0u32);
        for &x in k {
            lo = lo.min(x);
            hi = hi.max(x);
        }
        let (l, h) = ((lo - base) as usize, (hi - base) as usize);
        return Data::Dict { dict: StrCol { offsets: &dict.offsets[l..h + 2], data: dict.data }, keys: Keys::U32Off(k, lo), nullable, plain };
    }
    d
}
fn ptype(d: &Data) -> i32 {
    match d { Data::I32(..) | Data::Date(..) => T_INT32, Data::I64(..) => T_INT64, _ => T_BYTE_ARRAY }
}
fn is_opt(d: &Data) -> bool { matches!(d, Data::Dict { nullable: true, .. }) }

fn chunk_bound(d: &Data, p: &Plan) -> usize {
    let n = rows_of(d);
    let hdr = 64;
    match (d, p) {
        (Data::Str(s), _) => hdr + 4 * n + s.bytes_in(0, n),
        (Data::Dict { dict, nullable, plain, .. }, Plan::Dict { m, vbytes, w }) => {
            let def = if *nullable { 4 + hybrid_len(n, 1) } else { 0 };
            if *plain { hdr + def + vbytes } else { 2 * hdr + 4 * dict.len() + dict.bytes_in(0, dict.len()) + def + 1 + hybrid_len(*m, *w) + 16 }
        }
        (Data::I64(..), Plan::Plain) => hdr + 8 * n,
        (_, Plan::Plain) => hdr + 4 * n,
        (_, Plan::Dense { card, w, .. }) => 2 * hdr + 8 * card + 1 + hybrid_len(n, *w) + 16,
        (_, Plan::Hash { dict, w, .. }) => 2 * hdr + 8 * dict.len() + 1 + hybrid_len(n, *w) + 16,
        _ => unreachable!(),
    }
}


struct Meta { ptype: i32, dict: bool, size: u64, data_off: u64, dict_off: u64, nv: usize }

#[inline(always)]
unsafe fn copy_short(base: *const u8, dl: usize, a: usize, l: usize, dst: *mut u8) {
    if l <= 16 && a + 16 <= dl {
        (dst as *mut u128).write_unaligned((base.add(a) as *const u128).read_unaligned());
    } else {
        std::ptr::copy_nonoverlapping(base.add(a), dst, l);
    }
}

unsafe fn emit_plain_strs(o: &mut Out, s: &StrCol) -> io::Result<()> {
    let n = s.len();
    let mut i = 0;
    while i < n {
        let e = (i + BLOCK).min(n);
        let bytes = 4 * (e - i) + s.bytes_in(i, e);
        let mut p = o.tail(bytes);
        let base = s.data.as_ptr();
        let dl = s.data.len();
        for j in i..e {
            let a = *s.offsets.get_unchecked(j) as usize;
            let b = *s.offsets.get_unchecked(j + 1) as usize;
            let l = b - a;
            (p as *mut u32).write_unaligned(l as u32);
            copy_short(base, dl, a, l, p.add(4));
            p = p.add(4 + l);
        }
        o.commit(bytes);
        o.check()?;
        i = e;
    }
    Ok(())
}

fn emit_dict_page_strs(o: &mut Out, dict: &StrCol) -> io::Result<()> {
    let size = 4 * dict.len() + dict.bytes_in(0, dict.len());
    let mut h = Vec::with_capacity(32);
    page_header(&mut h, P_DICT, size, dict.len(), E_PLAIN);
    o.put(&h);
    unsafe { emit_plain_strs(o, dict) }
}

#[inline(always)]
unsafe fn valid_mask64(k: *const u8, cnt: usize) -> u64 {
    if cnt >= 64 {
        let ff = _mm256_set1_epi8(-1);
        let a = _mm256_movemask_epi8(_mm256_cmpeq_epi8(_mm256_loadu_si256(k as *const __m256i), ff)) as u32 as u64;
        let b = _mm256_movemask_epi8(_mm256_cmpeq_epi8(_mm256_loadu_si256(k.add(32) as *const __m256i), ff)) as u32 as u64;
        return !(a | (b << 32));
    }
    #[allow(unreachable_code)]
    {
        let mut m = 0u64;
        for j in 0..cnt.min(64) { m |= ((*k.add(j) != 255) as u64) << j; }
        m
    }
}

unsafe fn emit_dict_u8_opt(o: &mut Out, dict: &StrCol, keys: &[u8], m: usize, w: u32) -> io::Result<u64> {
    let n = keys.len();
    emit_dict_page_strs(o, dict)?;
    let data_off = o.pos();
    let def = hybrid_len(n, 1);
    let vals = 1 + hybrid_len(m, w);
    let total = 4 + def + vals;
    let mut h = Vec::with_capacity(32);
    page_header(&mut h, P_DATA, total, n, E_RLE_DICT);
    o.put(&h);
    let p = o.tail(total + 8);
    (p as *mut u32).write_unaligned(def as u32);
    let mut hv = Vec::with_capacity(10);
    put_varint(&mut hv, ((((n + 7) / 8) as u64) << 1) | 1);
    std::ptr::copy_nonoverlapping(hv.as_ptr(), p.add(4), hv.len());
    let q = p.add(4 + hv.len());
    let r = p.add(4 + def);
    *r = w as u8;
    hv.clear();
    put_varint(&mut hv, ((((m + 7) / 8) as u64) << 1) | 1);
    std::ptr::copy_nonoverlapping(hv.as_ptr(), r.add(1), hv.len());
    let vstart = r.add(1 + hv.len());
    let mut pk = Packer::new(vstart, w);
    let kp = keys.as_ptr();
    let mut i = 0;
    while i < n {
        let cnt = (n - i).min(64);
        let mut mk = valid_mask64(kp.add(i), cnt);
        let mb = mk.to_le_bytes();
        let nb = (cnt + 7) / 8;
        std::ptr::copy_nonoverlapping(mb.as_ptr(), q.add(i / 8), nb);
        while mk != 0 {
            let j = mk.trailing_zeros() as usize;
            pk.push(*kp.add(i + j) as u32);
            mk &= mk - 1;
        }
        i += cnt;
    }
    pk.finish((m + 7) / 8 * w as usize);
    o.commit(total);
    o.check()?;
    Ok(data_off)
}

unsafe fn emit_dict_t<K: Key + PartialEq>(o: &mut Out, dict: &StrCol, keys: &[K], nullable: bool, plain: bool, m: usize, vbytes: usize, w: u32) -> io::Result<Option<u64>> {
    let n = keys.len();
    let use_dict = !plain && m > 0;
    if use_dict { emit_dict_page_strs(o, dict)?; }
    let data_off = o.pos();
    let def = if nullable { 4 + hybrid_len(n, 1) } else { 0 };
    let vals = if use_dict { 1 + hybrid_len(m, w) } else { vbytes };
    let mut h = Vec::with_capacity(32);
    page_header(&mut h, P_DATA, def + vals, n, if use_dict { E_RLE_DICT } else { E_PLAIN });
    o.put(&h);
    if nullable {
        o.put(&((def - 4) as u32).to_le_bytes());
        emit_packed(o, n, 1, |i| (*keys.get_unchecked(i) != K::NUL) as u32)?;
    }
    if use_dict {
        o.put(&[w as u8]);
        if !nullable {
            emit_packed(o, n, w, |i| keys.get_unchecked(i).u())?;
        } else {
            let g = (m + 7) / 8;
            let mut hv = Vec::with_capacity(10);
            put_varint(&mut hv, ((g as u64) << 1) | 1);
            o.put(&hv);
            let bytes = g * w as usize;
            let p = o.tail(bytes);
            let mut pk = Packer::new(p, w);
            for &k in keys { if k != K::NUL { pk.push(k.u()); } }
            pk.finish(bytes);
            o.commit(bytes);
            o.check()?;
        }
    } else if m > 0 {
        let mut p = o.tail(vbytes);
        let base = dict.data.as_ptr();
        let dl = dict.data.len();
        for &k in keys {
            if nullable && k == K::NUL { continue; }
            let a = *dict.offsets.get_unchecked(k.u() as usize) as usize;
            let l = *dict.offsets.get_unchecked(k.u() as usize + 1) as usize - a;
            (p as *mut u32).write_unaligned(l as u32);
            copy_short(base, dl, a, l, p.add(4));
            p = p.add(4 + l);
        }
        o.commit(vbytes);
        o.check()?;
    }
    Ok(if use_dict { Some(data_off) } else { None })
}

unsafe fn emit_dict_off(o: &mut Out, dict: &StrCol, keys: &[u32], base: u32, m: usize, w: u32) -> io::Result<Option<u64>> {
    let n = keys.len();
    if m == 0 {
        let data_off = o.pos();
        let mut h = Vec::with_capacity(32);
        page_header(&mut h, P_DATA, 0, n, E_PLAIN);
        o.put(&h);
        let _ = data_off;
        return Ok(None);
    }
    emit_dict_page_strs(o, dict)?;
    let data_off = o.pos();
    let vals = 1 + hybrid_len(m, w);
    let mut h = Vec::with_capacity(32);
    page_header(&mut h, P_DATA, vals, n, E_RLE_DICT);
    o.put(&h);
    o.put(&[w as u8]);
    emit_packed(o, n, w, |i| keys.get_unchecked(i).wrapping_sub(base))?;
    Ok(Some(data_off))
}

fn int_dict_page(o: &mut Out, es: usize, card: usize, val: impl Fn(usize) -> i64) {
    let mut h = Vec::with_capacity(32);
    page_header(&mut h, P_DICT, card * es, card, E_PLAIN);
    o.put(&h);
    if es == 4 { for k in 0..card { o.put(&(val(k) as i32).to_le_bytes()); } }
    else { for k in 0..card { o.put(&val(k).to_le_bytes()); } }
}

unsafe fn emit_int_keys(o: &mut Out, n: usize, w: u32, key: impl Fn(usize) -> u32) -> io::Result<()> {
    let size = 1 + hybrid_len(n, w);
    let mut h = Vec::with_capacity(32);
    page_header(&mut h, P_DATA, size, n, E_RLE_DICT);
    o.put(&h);
    o.put(&[w as u8]);
    emit_packed(o, n, w, key)
}

fn ints_of<'a>(d: &Data<'a>) -> Ints<'a> {
    match *d { Data::I32(v, _) | Data::Date(v, _) => Ints::A(v), Data::I64(v, _) => Ints::B(v), _ => unreachable!() }
}

fn emit_chunk(o: &mut Out, d: &Data, p: &Plan) -> io::Result<Meta> {
    let n = rows_of(d);
    let start = o.pos();
    let mut h = Vec::with_capacity(64);
    let mut meta = Meta { ptype: ptype(d), dict: false, size: 0, data_off: start, dict_off: 0, nv: n };
    unsafe {
        match (d, p) {
            (Data::Str(s), _) => {
                page_header(&mut h, P_DATA, 4 * n + s.bytes_in(0, n), n, E_PLAIN);
                o.put(&h);
                emit_plain_strs(o, s)?;
            }
            (Data::Dict { dict, keys, nullable, plain }, Plan::Dict { m, vbytes, w }) => {
                let r = match keys {
                    Keys::U8(k) if *nullable && !*plain && *m > 0 => Some(emit_dict_u8_opt(o, dict, k, *m, *w)?),
                    Keys::U8(k) => emit_dict_t(o, dict, k, *nullable, *plain, *m, *vbytes, *w)?,
                    Keys::U16(k) => emit_dict_t(o, dict, k, *nullable, *plain, *m, *vbytes, *w)?,
                    Keys::U32(k) => emit_dict_t(o, dict, k, *nullable, *plain, *m, *vbytes, *w)?,
                    Keys::U32Off(k, b) => emit_dict_off(o, dict, k, *b, *m, *w)?,
                };
                if let Some(off) = r { meta.dict = true; meta.dict_off = start; meta.data_off = off; }
            }
            (_, Plan::Plain) => {
                let v = ints_of(d);
                page_header(&mut h, P_DATA, v.bytes().len(), n, E_PLAIN);
                o.put(&h);
                o.raw(v.bytes())?;
            }
            (_, Plan::Dense { min, card, w }) => {
                let v = ints_of(d);
                let min = *min;
                int_dict_page(o, v.esize(), *card, |k| min + k as i64);
                meta.dict = true; meta.dict_off = start; meta.data_off = o.pos();
                match v {
                    Ints::A(a) => emit_int_keys(o, n, *w, |i| (*a.get_unchecked(i) as i64 - min) as u32)?,
                    Ints::B(b) => emit_int_keys(o, n, *w, |i| (*b.get_unchecked(i) - min) as u32)?,
                }
            }
            (_, Plan::Hash { dict, keys, w }) => {
                let v = ints_of(d);
                int_dict_page(o, v.esize(), dict.len(), |k| dict[k]);
                meta.dict = true; meta.dict_off = start; meta.data_off = o.pos();
                emit_int_keys(o, n, *w, |i| *keys.get_unchecked(i))?;
            }
            _ => unreachable!(),
        }
    }
    meta.size = o.pos() - start;
    Ok(meta)
}


fn footer(cols: &[Column], groups: &[(usize, Vec<Meta>)], nrows: usize) -> Vec<u8> {
    let mut b = Vec::with_capacity(256 + 128 * cols.len() * groups.len().max(1));
    let mut t = Tw::new(&mut b);
    t.i32(1, 1);
    t.list(2, 12, cols.len() + 1);
    t.elem();
    t.bin(4, b"schema");
    t.i32(5, cols.len() as i32);
    t.end();
    for c in cols {
        t.elem();
        t.i32(1, ptype(&c.data));
        t.i32(3, if is_opt(&c.data) { 1 } else { 0 });
        t.bin(4, c.name.as_bytes());
        match c.data {
            Data::Date(..) => { t.i32(6, 6); t.begin(10); t.begin(6); t.end(); t.end(); }
            Data::Str(_) | Data::Dict { .. } => { t.i32(6, 0); t.begin(10); t.begin(1); t.end(); t.end(); }
            _ => {}
        }
        t.end();
    }
    t.i64(3, nrows as i64);
    t.list(4, 12, groups.len());
    for (gi, (rows, metas)) in groups.iter().enumerate() {
        t.elem();
        t.list(1, 12, metas.len());
        let mut total = 0i64;
        for (c, m) in cols.iter().zip(metas) {
            t.elem();
            t.i64(2, 0);
            t.begin(3);
            t.i32(1, m.ptype);
            if m.dict { t.list(2, 5, 3); t.varint(zz32(E_PLAIN)); t.varint(zz32(E_RLE)); t.varint(zz32(E_RLE_DICT)); }
            else { t.list(2, 5, 2); t.varint(zz32(E_PLAIN)); t.varint(zz32(E_RLE)); }
            t.list(3, 8, 1);
            t.varint(c.name.len() as u64);
            t.b.extend_from_slice(c.name.as_bytes());
            t.i32(4, 0);
            t.i64(5, m.nv as i64);
            t.i64(6, m.size as i64);
            t.i64(7, m.size as i64);
            t.i64(9, m.data_off as i64);
            if m.dict { t.i64(11, m.dict_off as i64); }
            t.end();
            t.end();
            total += m.size as i64;
        }
        t.i64(2, total);
        t.i64(3, *rows as i64);
        let first = metas.first().map(|m| if m.dict { m.dict_off } else { m.data_off }).unwrap_or(4);
        t.i64(5, first as i64);
        t.i64(6, total);
        t.i16(7, gi as i16);
        t.end();
    }
    t.bin(6, b"pqw version 0.1");
    t.stop();
    b
}


fn encode_into(o: &mut Out, cols: &[Column], opts: &Opts) -> io::Result<()> {
    assert!(!cols.is_empty());
    let n = rows_of(&cols[0].data);
    for c in cols { assert_eq!(rows_of(&c.data), n, "column {} has a different row count", c.name); }
    let rpg = opts.rows_per_group.max(1);
    o.put(b"PAR1");
    let mut groups = Vec::new();
    let mut a = 0;
    while a < n {
        let b = if n - a > rpg { a + rpg } else { n };
        let ds: Vec<Data> = cols.iter().map(|c| narrow(slice_data(&c.data, a, b))).collect();
        #[cfg(feature = "tl")]
        let tp = crate::sys::tl::now();
        let plans: Vec<Plan> = ds.iter().map(plan).collect();
        #[cfg(feature = "tl")]
        crate::sys::tl::WR_COL[7].fetch_add(crate::sys::tl::now() - tp, std::sync::atomic::Ordering::Relaxed);
        if o.file.is_none() {
            let bound: usize = ds.iter().zip(&plans).map(|(d, p)| chunk_bound(d, p)).sum();
            o.buf.reserve(bound + 512 + 160 * cols.len());
        }
        let mut metas = Vec::with_capacity(cols.len());
        for (_j, (d, p)) in ds.iter().zip(&plans).enumerate() {
            #[cfg(feature = "tl")]
            let tp = crate::sys::tl::now();
            #[cfg(feature = "tl")]
            let io0 = crate::sys::tl::WR_IO.load(std::sync::atomic::Ordering::Relaxed);
            metas.push(emit_chunk(o, d, p)?);
            #[cfg(feature = "tl")]
            { let io1 = crate::sys::tl::WR_IO.load(std::sync::atomic::Ordering::Relaxed);
              crate::sys::tl::WR_COL[_j.min(6)].fetch_add((crate::sys::tl::now() - tp).saturating_sub(io1.wrapping_sub(io0)), std::sync::atomic::Ordering::Relaxed); }
        }
        groups.push((b - a, metas));
        a = b;
    }
    #[cfg(feature = "tl")]
    let tf = crate::sys::tl::now();
    let f = footer(cols, &groups, n);
    o.put(&f);
    o.put(&(f.len() as u32).to_le_bytes());
    o.put(b"PAR1");
    let r = o.flush();
    #[cfg(feature = "tl")]
    crate::out::ev::FT.fetch_add(crate::sys::tl::now() - tf, std::sync::atomic::Ordering::Relaxed);
    r
}

pub fn encode(cols: &[Column], opts: &Opts) -> Vec<u8> {
    let mut o = Out::mem(0);
    encode_into(&mut o, cols, opts).expect("in-memory encode cannot fail");
    o.buf
}

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn ftruncate(fd: i32, len: i64) -> i32;
}

pub fn write_file(path: &Path, cols: &[Column], opts: &Opts, method: Method) -> io::Result<u64> {
    match method {
        Method::Whole => {
            let v = encode(cols, opts);
            #[cfg(feature = "tl")]
            let t0 = crate::sys::tl::now();
            let mut f = File::create(path)?;
            #[cfg(feature = "tl")]
            let t1 = crate::sys::tl::now();
            f.write_all(&v)?;
            #[cfg(feature = "tl")]
            { crate::out::ev::WN.fetch_add(1, std::sync::atomic::Ordering::Relaxed); crate::out::ev::WB.fetch_add(v.len() as u64, std::sync::atomic::Ordering::Relaxed); crate::out::ev::FN.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
            #[cfg(feature = "tl")]
            {
                crate::sys::tl::WR_OPEN.fetch_add(t1 - t0, std::sync::atomic::Ordering::Relaxed);
                crate::sys::tl::WR_IO.fetch_add(crate::sys::tl::now() - t1, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(v.len() as u64)
        }
        Method::Stream(limit) => {
            #[cfg(feature = "tl")]
            let t0 = crate::sys::tl::now();
            let f = File::create(path)?;
            #[cfg(feature = "tl")]
            crate::out::ev::FN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            #[cfg(feature = "tl")]
            crate::sys::tl::WR_OPEN.fetch_add(crate::sys::tl::now() - t0, std::sync::atomic::Ordering::Relaxed);
            #[cfg(feature = "tl")]
            let ta = crate::sys::tl::now();
            let mut o = Out { buf: Vec::with_capacity(limit + BLOCK * 64), flushed: 0, file: Some(f), limit };
            #[cfg(feature = "tl")]
            let tb = crate::sys::tl::now();
            encode_into(&mut o, cols, opts)?;
            #[cfg(feature = "tl")]
            { crate::out::ev::FA.fetch_add(tb - ta, std::sync::atomic::Ordering::Relaxed); crate::out::ev::FE.fetch_add(crate::sys::tl::now() - tb, std::sync::atomic::Ordering::Relaxed); }
            #[cfg(feature = "tl")]
            {
                let t0 = crate::sys::tl::now();
                drop(o.file.take());
                crate::sys::tl::WR_CLOSE.fetch_add(crate::sys::tl::now() - t0, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(o.flushed)
        }
        Method::Mmap { populate } => {
            let v = encode(cols, opts);
            let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)?;
            let fd = f.as_raw_fd();
            unsafe {
                if ftruncate(fd, v.len() as i64) != 0 { return Err(io::Error::last_os_error()); }
                if !v.is_empty() {
                    let flags = 1 | if populate { 0x8000 } else { 0 };
                    let p = mmap(std::ptr::null_mut(), v.len(), 3, flags, fd, 0);
                    if p as isize == -1 { return Err(io::Error::last_os_error()); }
                    std::ptr::copy_nonoverlapping(v.as_ptr(), p, v.len());
                    munmap(p, v.len());
                }
            }
            Ok(v.len() as u64)
        }
    }
}


use std::arch::x86_64::*;

pub struct UserGroup<'a> {
    pub keys: &'a [u32],
    pub names: StrCol<'a>,
    pub day: &'a [i32],
    pub ses: &'a [i32],
    pub evs: &'a [i32],
    pub rev: &'a [i32],
    pub cat: &'a [u8],
    pub cats: StrCol<'a>,
    pub day0: i32,
    pub nd: usize,
}

pub struct CampGroup<'a> {
    pub id: StrCol<'a>,
    pub ch_dict: StrCol<'a>,
    pub ch_keys: &'a [u32],
    pub day: &'a [i32],
    pub day0: i32,
    pub nd: usize,
    pub v: [&'a [i64]; 4],
}

pub struct GroupFile {
    o: Out,
    groups: Vec<(usize, Vec<Meta>)>,
    nrows: usize,
}

#[inline(always)]
fn lowmask(w: u32) -> u64 { if w >= 64 { u64::MAX } else { (1u64 << w) - 1 } }

#[inline(always)]
unsafe fn put8<const W: u32>(p: *mut u8, d: __m256i) {
    let m2 = lowmask(W) | (lowmask(W) << 32);
    let lo = _mm256_castsi256_si128(d);
    let hi = _mm256_extracti128_si256::<1>(d);
    let a0 = _pext_u64(_mm_cvtsi128_si64(lo) as u64, m2);
    let a1 = _pext_u64(_mm_extract_epi64::<1>(lo) as u64, m2);
    let a2 = _pext_u64(_mm_cvtsi128_si64(hi) as u64, m2);
    let a3 = _pext_u64(_mm_extract_epi64::<1>(hi) as u64, m2);
    if W <= 16 {
        let q0 = a0 | (a1 << (2 * W));
        let q1 = a2 | (a3 << (2 * W));
        let v = q0 as u128 | ((q1 as u128) << (4 * W));
        (p as *mut u128).write_unaligned(v);
    } else {
        let q0 = a0 as u128 | ((a1 as u128) << (2 * W));
        let q1 = a2 as u128 | ((a3 as u128) << (2 * W));
        if W == 32 {
            (p as *mut u128).write_unaligned(q0);
            (p.add(16) as *mut u128).write_unaligned(q1);
        } else {
            (p as *mut u128).write_unaligned(q0 | (q1 << (4 * W)));
            (p.add(16) as *mut u128).write_unaligned(q1 >> (128 - 4 * W));
        }
    }
}

#[inline(always)]
unsafe fn pack32<const W: u32>(dst: *mut u8, src: *const u32, n: usize, base: u32) -> u32 {
    let b = _mm256_set1_epi32(base as i32);
    let mut mx = _mm256_setzero_si256();
    let full = n / 8;
    let mut p = dst;
    for g in 0..full {
        let v = _mm256_loadu_si256(src.add(g * 8) as *const __m256i);
        let d = _mm256_sub_epi32(v, b);
        mx = _mm256_max_epu32(mx, d);
        put8::<W>(p, d);
        p = p.add(W as usize);
    }
    let rem = n - full * 8;
    if rem > 0 {
        let mut t = [base; 8];
        std::ptr::copy_nonoverlapping(src.add(full * 8), t.as_mut_ptr(), rem);
        let v = _mm256_loadu_si256(t.as_ptr() as *const __m256i);
        let d = _mm256_sub_epi32(v, b);
        mx = _mm256_max_epu32(mx, d);
        put8::<W>(p, d);
    }
    let mut a = [0u32; 8];
    _mm256_storeu_si256(a.as_mut_ptr() as *mut __m256i, mx);
    let mut m = 0;
    for x in a { m = m.max(x); }
    m
}


#[inline(always)]
unsafe fn pack32_b<const W: u32>(dst: *mut u8, src: *const u32, n: usize, base: u32) -> u32 {
    let b = _mm256_set1_epi32(base as i32);
    let mut mx = _mm256_setzero_si256();
    let km = 0x0101010101010101u64 * lowmask(W);
    let mut p = dst;
    let mut i = 0;
    let mut t;
    while i < n {
        let s = if i + 32 <= n { src.add(i) } else {
            t = [base; 32];
            std::ptr::copy_nonoverlapping(src.add(i), t.as_mut_ptr(), n - i);
            t.as_ptr()
        };
        let d0 = _mm256_sub_epi32(_mm256_loadu_si256(s as *const __m256i), b);
        let d1 = _mm256_sub_epi32(_mm256_loadu_si256(s.add(8) as *const __m256i), b);
        let d2 = _mm256_sub_epi32(_mm256_loadu_si256(s.add(16) as *const __m256i), b);
        let d3 = _mm256_sub_epi32(_mm256_loadu_si256(s.add(24) as *const __m256i), b);
        mx = _mm256_max_epu32(mx, _mm256_max_epu32(_mm256_max_epu32(d0, d1), _mm256_max_epu32(d2, d3)));
        let w01 = _mm256_packus_epi32(d0, d1);
        let w23 = _mm256_packus_epi32(d2, d3);
        let by = _mm256_packus_epi16(w01, w23);
        let by = _mm256_permutevar8x32_epi32(by, _mm256_setr_epi32(0, 4, 1, 5, 2, 6, 3, 7));
        let lo = _mm256_castsi256_si128(by);
        let hi = _mm256_extracti128_si256::<1>(by);
        let q = [_mm_cvtsi128_si64(lo) as u64, _mm_extract_epi64::<1>(lo) as u64, _mm_cvtsi128_si64(hi) as u64, _mm_extract_epi64::<1>(hi) as u64];
        let cnt = ((n - i).min(32) + 7) / 8;
        for j in 0..cnt {
            (p as *mut u64).write_unaligned(_pext_u64(q[j], km));
            p = p.add(W as usize);
        }
        i += 32;
    }
    let mut a = [0u32; 8];
    _mm256_storeu_si256(a.as_mut_ptr() as *mut __m256i, mx);
    let mut m = 0;
    for x in a { m = m.max(x); }
    m
}

#[inline(always)]
unsafe fn pack32_h<const W: u32>(dst: *mut u8, src: *const u32, n: usize, base: u32) -> u32 {
    let b = _mm256_set1_epi32(base as i32);
    let mut mx = _mm256_setzero_si256();
    let km = 0x0001000100010001u64 * lowmask(W);
    let mut p = dst;
    let mut i = 0;
    let mut t;
    while i < n {
        let s = if i + 16 <= n { src.add(i) } else {
            t = [base; 16];
            std::ptr::copy_nonoverlapping(src.add(i), t.as_mut_ptr(), n - i);
            t.as_ptr()
        };
        let d0 = _mm256_sub_epi32(_mm256_loadu_si256(s as *const __m256i), b);
        let d1 = _mm256_sub_epi32(_mm256_loadu_si256(s.add(8) as *const __m256i), b);
        mx = _mm256_max_epu32(mx, _mm256_max_epu32(d0, d1));
        let w = _mm256_packus_epi32(d0, d1);
        let w = _mm256_permute4x64_epi64::<0b11_01_10_00>(w);
        let lo = _mm256_castsi256_si128(w);
        let hi = _mm256_extracti128_si256::<1>(w);
        let a0 = _pext_u64(_mm_cvtsi128_si64(lo) as u64, km);
        let a1 = _pext_u64(_mm_extract_epi64::<1>(lo) as u64, km);
        let a2 = _pext_u64(_mm_cvtsi128_si64(hi) as u64, km);
        let a3 = _pext_u64(_mm_extract_epi64::<1>(hi) as u64, km);
        (p as *mut u128).write_unaligned(a0 as u128 | ((a1 as u128) << (4 * W)));
        if i + 8 < n {
            (p.add(W as usize) as *mut u128).write_unaligned(a2 as u128 | ((a3 as u128) << (4 * W)));
            p = p.add(2 * W as usize);
        } else {
            p = p.add(W as usize);
        }
        i += 16;
    }
    let mut a = [0u32; 8];
    _mm256_storeu_si256(a.as_mut_ptr() as *mut __m256i, mx);
    let mut m = 0;
    for x in a { m = m.max(x); }
    m
}

const PACK_MODE: u32 = crate::env_u32(option_env!("XL_PACK_DEF"), 1);
unsafe fn pack32_any(dst: *mut u8, src: *const u32, n: usize, base: u32, w: u32) -> u32 {
    pack32_any_mode(dst, src, n, base, w, PACK_MODE)
}
unsafe fn pack32_any_mode(dst: *mut u8, src: *const u32, n: usize, base: u32, w: u32, mode: u32) -> u32 {
    if mode == 1 {
        macro_rules! b { ($($k:literal)*) => { match w { $($k => return pack32_b::<$k>(dst, src, n, base),)* _ => {} } } }
        b!(1 2 3 4 5 6 7 8);
        macro_rules! h { ($($k:literal)*) => { match w { $($k => return pack32_h::<$k>(dst, src, n, base),)* _ => {} } } }
        h!(9 10 11 12 13 14 15 16);
    }
    macro_rules! d { ($($k:literal)*) => { match w { $($k => pack32::<$k>(dst, src, n, base),)* _ => u32::MAX } } }
    d!(1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32)
}

unsafe fn stats2(a: &[i32], b: &[i32], c: &[u8]) -> (i32, i32, i32, i32, usize) {
    let n = a.len();
    let (mut amin, mut amax, mut bmin, mut bmax) = (_mm256_set1_epi32(i32::MAX), _mm256_set1_epi32(i32::MIN), _mm256_set1_epi32(i32::MAX), _mm256_set1_epi32(i32::MIN));
    let ff = _mm256_set1_epi8(-1);
    let mut nul = 0usize;
    let mut i = 0;
    while i + 32 <= n {
        for j in 0..4 {
            let x = _mm256_loadu_si256(a.as_ptr().add(i + 8 * j) as *const __m256i);
            amin = _mm256_min_epi32(amin, x);
            amax = _mm256_max_epi32(amax, x);
            let y = _mm256_loadu_si256(b.as_ptr().add(i + 8 * j) as *const __m256i);
            bmin = _mm256_min_epi32(bmin, y);
            bmax = _mm256_max_epi32(bmax, y);
        }
        let z = _mm256_loadu_si256(c.as_ptr().add(i) as *const __m256i);
        nul += (_mm256_movemask_epi8(_mm256_cmpeq_epi8(z, ff)) as u32).count_ones() as usize;
        i += 32;
    }
    let mut r = [[0i32; 8]; 4];
    _mm256_storeu_si256(r[0].as_mut_ptr() as *mut __m256i, amin);
    _mm256_storeu_si256(r[1].as_mut_ptr() as *mut __m256i, amax);
    _mm256_storeu_si256(r[2].as_mut_ptr() as *mut __m256i, bmin);
    _mm256_storeu_si256(r[3].as_mut_ptr() as *mut __m256i, bmax);
    let (mut a0, mut a1, mut b0, mut b1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for k in 0..8 { a0 = a0.min(r[0][k]); a1 = a1.max(r[1][k]); b0 = b0.min(r[2][k]); b1 = b1.max(r[3][k]); }
    while i < n {
        a0 = a0.min(a[i]); a1 = a1.max(a[i]); b0 = b0.min(b[i]); b1 = b1.max(b[i]);
        nul += (c[i] == 255) as usize;
        i += 1;
    }
    (a0, a1, b0, b1, n - nul)
}

unsafe fn pack_opt_u8(dst: *mut u8, vdst: *mut u8, c: *const u8, n: usize, w: u32) -> u32 {
    let ff = _mm256_set1_epi8(-1);
    let mut mx = _mm256_setzero_si256();
    let kmask = 0x0101010101010101u64 * (lowmask(w) as u64);
    let (mut acc, mut nb, mut p) = (0u64, 0u32, vdst);
    let mut i = 0;
    let mut tail = [255u8; 32];
    while i < n {
        let src = if i + 32 <= n { c.add(i) } else {
            std::ptr::copy_nonoverlapping(c.add(i), tail.as_mut_ptr(), n - i);
            tail.as_ptr()
        };
        let z = _mm256_loadu_si256(src as *const __m256i);
        let eq = _mm256_cmpeq_epi8(z, ff);
        mx = _mm256_max_epu8(mx, _mm256_andnot_si256(eq, z));
        let vm = !(_mm256_movemask_epi8(eq) as u32);
        if i + 32 <= n {
            (dst.add(i / 8) as *mut u32).write_unaligned(vm);
        } else {
            let nbytes = (n - i + 7) / 8;
            let b = vm.to_le_bytes();
            std::ptr::copy_nonoverlapping(b.as_ptr(), dst.add(i / 8), nbytes);
        }
        for j in 0..4 {
            let bm = (vm >> (8 * j)) & 0xFF;
            let x = (src.add(8 * j) as *const u64).read_unaligned();
            let sel = _pdep_u64(bm as u64, 0x0101010101010101) * 0xFF;
            let y = _pext_u64(x, sel & kmask);
            acc |= y << nb;
            nb += bm.count_ones() * w;
            (p as *mut u64).write_unaligned(acc);
            let adv = nb >> 3;
            p = p.add(adv as usize);
            acc = if adv == 0 { acc } else { acc >> (adv * 8) };
            nb &= 7;
        }
        i += 32;
    }
    if nb > 0 { (p as *mut u64).write_unaligned(acc); }
    let mut a = [0u8; 32];
    _mm256_storeu_si256(a.as_mut_ptr() as *mut __m256i, mx);
    a.iter().copied().max().unwrap_or(0) as u32
}

fn data_page_header(b: &mut Vec<u8>, size: usize, nv: usize, enc: i32) { page_header(b, P_DATA, size, nv, enc) }

impl GroupFile {
    pub fn mem(cap: usize) -> GroupFile {
        let mut o = Out { buf: Vec::with_capacity(cap), flushed: 0, file: None, limit: usize::MAX };
        o.put(b"PAR1");
        GroupFile { o, groups: Vec::new(), nrows: 0 }
    }
    pub fn o_len(&self) -> usize { self.o.buf.len() }
    pub fn size(&self) -> u64 { self.o.pos() }
    pub fn reset(&mut self) { self.o.buf.truncate(4); self.groups.clear(); self.nrows = 0; }
    pub fn create(path: &Path, limit: usize) -> io::Result<GroupFile> {
        Ok(GroupFile::from_file(File::create(path)?, limit))
    }
    pub fn from_file_buf(f: File, limit: usize, mut buf: Vec<u8>) -> GroupFile {
        buf.clear();
        let mut o = Out { buf, flushed: 0, file: Some(f), limit };
        o.put(b"PAR1");
        GroupFile { o, groups: Vec::new(), nrows: 0 }
    }
    pub fn take_buf(&mut self) -> io::Result<Vec<u8>> {
        self.o.flush()?;
        Ok(std::mem::take(&mut self.o.buf))
    }
    pub fn put_buf(&mut self, mut b: Vec<u8>) {
        b.clear();
        self.o.buf = b;
    }
    pub fn finish_buf(mut self, schema: &[Column]) -> io::Result<Vec<u8>> {
        let f = footer(schema, &self.groups, self.nrows);
        self.o.put(&f);
        self.o.put(&(f.len() as u32).to_le_bytes());
        self.o.put(b"PAR1");
        self.o.flush()?;
        Ok(std::mem::take(&mut self.o.buf))
    }
    pub fn from_file(f: File, limit: usize) -> GroupFile {
        let mut o = Out { buf: Vec::with_capacity(limit + (limit >> 2).max(1 << 16)), flushed: 0, file: Some(f), limit };
        o.put(b"PAR1");
        GroupFile { o, groups: Vec::new(), nrows: 0 }
    }

    fn room(&mut self, n: usize) -> io::Result<()> {
        if self.o.file.is_none() { self.o.buf.reserve(n + 64); return Ok(()); }
        if self.o.buf.len() + n + 64 > self.o.buf.capacity() {
            self.o.flush()?;
            if n + 64 > self.o.buf.capacity() { self.o.buf.reserve(n + 64); }
        }
        Ok(())
    }

    unsafe fn dense_chunk(&mut self, v: &[i32], base: i32, card: usize, w: u32) -> io::Result<Option<Meta>> {
        let n = v.len();
        let vals = 1 + hybrid_len(n, w);
        let bound = 64 + 4 * card + 64 + vals + 32;
        self.room(bound)?;
        let start = self.o.pos();
        let l0 = self.o.buf.len();
        int_dict_page(&mut self.o, 4, card, |k| base as i64 + k as i64);
        let data_off = self.o.pos();
        let mut h = Vec::with_capacity(32);
        data_page_header(&mut h, vals, n, E_RLE_DICT);
        self.o.put(&h);
        self.o.put(&[w as u8]);
        let mut hv = Vec::with_capacity(10);
        put_varint(&mut hv, ((((n + 7) / 8) as u64) << 1) | 1);
        self.o.put(&hv);
        let p = self.o.tail((n + 7) / 8 * w as usize + 32);
        let mx = pack32_any(p, v.as_ptr() as *const u32, n, base as u32, w);
        if mx as usize >= card {
            self.o.buf.truncate(l0);
            return Ok(None);
        }
        self.o.commit((n + 7) / 8 * w as usize);
        Ok(Some(Meta { ptype: T_INT32, dict: true, size: self.o.pos() - start, data_off, dict_off: start, nv: n }))
    }

    pub fn user_group(&mut self, g: &UserGroup) -> io::Result<()> {
        let n = g.keys.len();
        if n == 0 { return Ok(()); }
        let mut metas = Vec::with_capacity(6);
        #[cfg(feature = "tl")]
        let mut tt = (crate::sys::tl::now(), crate::sys::tl::WR_IO.load(std::sync::atomic::Ordering::Relaxed));
        #[cfg(feature = "tl")]
        let mut lap = |j: usize| { let t = crate::sys::tl::now(); let io = crate::sys::tl::WR_IO.load(std::sync::atomic::Ordering::Relaxed); crate::sys::tl::WR_COL[j].fetch_add((t - tt.0).saturating_sub(io.wrapping_sub(tt.1)), std::sync::atomic::Ordering::Relaxed); tt = (crate::sys::tl::now(), io); };
        #[cfg(not(feature = "tl"))]
        let lap = |_j: usize| {};
        unsafe {
            let (k0, k1) = (g.keys[0], g.keys[n - 1]);
            let mut done = false;
            if k1 >= k0 && (k1 as usize) < g.names.len() {
                let card = (k1 - k0) as usize + 1;
                let w = bits_for((card - 1) as u64);
                let d = g.names.slice(k0 as usize, k1 as usize + 1);
                let dbytes = 4 * card + d.bytes_in(0, card);
                let vals = 1 + hybrid_len(n, w);
                self.room(64 + dbytes + 64 + vals + 32)?;
                let l0 = self.o.buf.len();
                let start = self.o.pos();
                let mut h = Vec::with_capacity(32);
                page_header(&mut h, P_DICT, dbytes, card, E_PLAIN);
                self.o.put(&h);
                emit_plain_strs(&mut self.o, &d)?;
                let data_off = self.o.pos();
                h.clear();
                data_page_header(&mut h, vals, n, E_RLE_DICT);
                self.o.put(&h);
                self.o.put(&[w as u8]);
                h.clear();
                put_varint(&mut h, ((((n + 7) / 8) as u64) << 1) | 1);
                self.o.put(&h);
                let pb = (n + 7) / 8 * w as usize;
                let p = self.o.tail(pb + 32);
                let mx = pack32_any(p, g.keys.as_ptr(), n, k0, w);
                if (mx as usize) < card {
                    self.o.commit(pb);
                    metas.push(Meta { ptype: T_BYTE_ARRAY, dict: true, size: self.o.pos() - start, data_off, dict_off: start, nv: n });
                    done = true;
                } else {
                    self.o.buf.truncate(l0);
                }
            }
            if !done {
                let d = narrow(Data::Dict { dict: g.names, keys: Keys::U32Off(g.keys, 0), nullable: false, plain: false });
                self.room(chunk_bound(&d, &plan(&d)))?;
                metas.push(emit_chunk(&mut self.o, &d, &plan(&d))?);
            }
            lap(0);
            let wd = bits_for(g.nd.max(1) as u64 - 1);
            match self.dense_chunk(g.day, g.day0, g.nd.max(1), wd)? {
                Some(m) => metas.push(m),
                None => { let d = Data::Date(g.day, IntEnc::Dense); let p = plan(&d); self.room(chunk_bound(&d, &p))?; metas.push(emit_chunk(&mut self.o, &d, &p)?); }
            }
            lap(1);
            let (s0, s1, e0, e1, m) = stats2(g.ses, g.evs, g.cat);
            lap(7);
            for (v, lo, hi, auto) in [(g.ses, s0, s1, false), (g.evs, e0, e1, true)] {
                let r = hi as i64 - lo as i64;
                let card = r as usize + 1;
                let w = bits_for(r as u64);
                let dense_ok = r < DENSE_MAX as i64 && (!auto || (card * 4 + (n + 7) / 8 * w as usize) * 10 <= n * 4 * 8);
                let mut mm = None;
                if dense_ok { mm = self.dense_chunk(v, lo, card, w)?; }
                match mm {
                    Some(x) => metas.push(x),
                    None => {
                        let d = Data::I32(v, IntEnc::Plain);
                        let mut hh = Vec::with_capacity(32);
                        data_page_header(&mut hh, 4 * n, n, E_PLAIN);
                        let start = self.o.pos();
                        self.o.put(&hh);
                        self.o.raw(ints_of(&d).bytes())?;
                        metas.push(Meta { ptype: T_INT32, dict: false, size: self.o.pos() - start, data_off: start, dict_off: 0, nv: n });
                    }
                }
            }
            lap(2);
            {
                let mut hh = Vec::with_capacity(32);
                data_page_header(&mut hh, 4 * n, n, E_PLAIN);
                let start = self.o.pos();
                self.o.put(&hh);
                let b = std::slice::from_raw_parts(g.rev.as_ptr() as *const u8, 4 * n);
                self.o.raw(b)?;
                metas.push(Meta { ptype: T_INT32, dict: false, size: self.o.pos() - start, data_off: start, dict_off: 0, nv: n });
            }
            lap(4);
            let nc = g.cats.len();
            let w = bits_for(nc.saturating_sub(1) as u64);
            let mut done = false;
            if m > 0 && nc > 0 && nc < 255 && w <= 6 {
                let dbytes = 4 * nc + g.cats.bytes_in(0, nc);
                let def = hybrid_len(n, 1);
                let vals = 1 + hybrid_len(m, w);
                let total = 4 + def + vals;
                self.room(64 + dbytes + 64 + total + 64)?;
                let l0 = self.o.buf.len();
                let start = self.o.pos();
                emit_dict_page_strs(&mut self.o, &g.cats)?;
                let data_off = self.o.pos();
                let mut h = Vec::with_capacity(32);
                data_page_header(&mut h, total, n, E_RLE_DICT);
                self.o.put(&h);
                let p = self.o.tail(total + 64);
                (p as *mut u32).write_unaligned(def as u32);
                h.clear();
                put_varint(&mut h, ((((n + 7) / 8) as u64) << 1) | 1);
                std::ptr::copy_nonoverlapping(h.as_ptr(), p.add(4), h.len());
                let q = p.add(4 + h.len());
                let r = p.add(4 + def);
                h.clear();
                put_varint(&mut h, ((((m + 7) / 8) as u64) << 1) | 1);
                *r = w as u8;
                std::ptr::copy_nonoverlapping(h.as_ptr(), r.add(1), h.len());
                let vstart = r.add(1 + h.len());
                let vbytes = (m + 7) / 8 * w as usize;
                std::ptr::write_bytes(vstart, 0, vbytes + 8);
                let mx = pack_opt_u8(q, vstart, g.cat.as_ptr(), n, w);
                if (mx as usize) < nc {
                    self.o.commit(total);
                    metas.push(Meta { ptype: T_BYTE_ARRAY, dict: true, size: self.o.pos() - start, data_off, dict_off: start, nv: n });
                    done = true;
                } else {
                    self.o.buf.truncate(l0);
                }
            }
            if !done {
                let d = Data::Dict { dict: g.cats, keys: Keys::U8(g.cat), nullable: true, plain: false };
                let p = plan(&d);
                self.room(chunk_bound(&d, &p))?;
                metas.push(emit_chunk(&mut self.o, &d, &p)?);
            }
        }
        lap(5);
        self.o.check()?;
        lap(6);
        self.groups.push((n, metas));
        self.nrows += n;
        Ok(())
    }

    pub fn campaign_group(&mut self, c: &CampGroup) -> io::Result<()> {
        let n = c.day.len();
        if n == 0 { return Ok(()); }
        let mut metas = Vec::with_capacity(7);
        unsafe {
            let sb = 4 * n + c.id.bytes_in(0, n);
            self.room(64 + sb + 32)?;
            let start = self.o.pos();
            let mut h = Vec::with_capacity(32);
            data_page_header(&mut h, sb, n, E_PLAIN);
            self.o.put(&h);
            emit_plain_strs(&mut self.o, &c.id)?;
            metas.push(Meta { ptype: T_BYTE_ARRAY, dict: false, size: self.o.pos() - start, data_off: start, dict_off: 0, nv: n });
            let card = c.ch_dict.len();
            let w = bits_for(card.saturating_sub(1) as u64);
            let dbytes = 4 * card + c.ch_dict.bytes_in(0, card);
            let vals = 1 + hybrid_len(n, w);
            self.room(64 + dbytes + 64 + vals + 32)?;
            let l0 = self.o.buf.len();
            let start = self.o.pos();
            emit_dict_page_strs(&mut self.o, &c.ch_dict)?;
            let data_off = self.o.pos();
            h.clear();
            data_page_header(&mut h, vals, n, E_RLE_DICT);
            self.o.put(&h);
            self.o.put(&[w as u8]);
            h.clear();
            put_varint(&mut h, ((((n + 7) / 8) as u64) << 1) | 1);
            self.o.put(&h);
            let pb = (n + 7) / 8 * w as usize;
            let p = self.o.tail(pb + 32);
            let mx = pack32_any(p, c.ch_keys.as_ptr(), n, 0, w);
            if card > 0 && (mx as usize) < card {
                self.o.commit(pb);
                metas.push(Meta { ptype: T_BYTE_ARRAY, dict: true, size: self.o.pos() - start, data_off, dict_off: start, nv: n });
            } else {
                self.o.buf.truncate(l0);
                return Err(io::Error::new(io::ErrorKind::InvalidData, "channel key"));
            }
            let wd = bits_for(c.nd.max(1) as u64 - 1);
            match self.dense_chunk(c.day, c.day0, c.nd.max(1), wd)? {
                Some(m) => metas.push(m),
                None => return Err(io::Error::new(io::ErrorKind::InvalidData, "campaign day")),
            }
            for v in c.v {
                let mut hh = Vec::with_capacity(32);
                data_page_header(&mut hh, 8 * n, n, E_PLAIN);
                let start = self.o.pos();
                self.o.put(&hh);
                self.o.raw(std::slice::from_raw_parts(v.as_ptr() as *const u8, 8 * n))?;
                metas.push(Meta { ptype: T_INT64, dict: false, size: self.o.pos() - start, data_off: start, dict_off: 0, nv: n });
            }
        }
        self.o.check()?;
        self.groups.push((n, metas));
        self.nrows += n;
        Ok(())
    }

    pub fn finish(mut self, schema: &[Column]) -> io::Result<u64> {
        let f = footer(schema, &self.groups, self.nrows);
        self.o.put(&f);
        self.o.put(&(f.len() as u32).to_le_bytes());
        self.o.put(b"PAR1");
        self.o.flush()?;
        Ok(self.o.flushed)
    }
}

#[cfg(test)]
mod tests_w6 {
    use super::*;
    fn rng(s: &mut u64) -> u64 { *s ^= *s << 13; *s ^= *s >> 7; *s ^= *s << 17; *s }
    #[test]
    fn packers_match_reference() {
        let mut s = 0x1234_5678_9abc_def1u64;
        for w in 1..=32u32 {
            for &n in &[0usize, 1, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 100, 1000, 4097] {
                let base = (rng(&mut s) as u32) >> 1;
                let src: Vec<u32> = (0..n).map(|_| base.wrapping_add((rng(&mut s) & lowmask(w)) as u32)).collect();
                let bytes = (n + 7) / 8 * w as usize;
                let mut a = vec![0u8; bytes + 64];
                let mut b = vec![0xAAu8; bytes + 64];
                let mut c = vec![0x55u8; bytes + 64];
                unsafe {
                    pack_any(a.as_mut_ptr(), n, w, &|i| src[i] - base);
                    let mx = pack32_any(b.as_mut_ptr(), src.as_ptr(), n, base, w);
                    let m2 = pack32_any_mode(c.as_mut_ptr(), src.as_ptr(), n, base, w, 0);
                    let want = src.iter().map(|&x| x - base).max().unwrap_or(0);
                    assert_eq!(mx, want, "w {w} n {n}");
                    assert_eq!(m2, want, "w {w} n {n}");
                }
                assert_eq!(&a[..bytes], &b[..bytes], "w {w} n {n}");
                assert_eq!(&a[..bytes], &c[..bytes], "w {w} n {n}");
            }
            let n = 40;
            let base = 1000u32;
            let mut src: Vec<u32> = vec![base; n];
            src[37] = base - 1;
            let mut b = vec![0u8; 4 * n + 64];
            let mx = unsafe { pack32_any(b.as_mut_ptr(), src.as_ptr(), n, base, w) };
            assert_eq!(mx, u32::MAX);
        }
    }
    #[test]
    fn opt_u8_matches_reference() {
        let mut s = 99u64;
        for w in 1..=6u32 {
            for &n in &[1usize, 5, 8, 31, 32, 33, 64, 100, 1001] {
                let cat: Vec<u8> = (0..n).map(|_| if rng(&mut s) % 3 == 0 { 255 } else { (rng(&mut s) & lowmask(w)) as u8 }).collect();
                let m = cat.iter().filter(|&&x| x != 255).count();
                let mut d = vec![0u8; n / 8 + 16];
                let vb = (m + 7) / 8 * w as usize;
                let mut v = vec![0u8; vb + 16];
                let mx = unsafe { pack_opt_u8(d.as_mut_ptr(), v.as_mut_ptr(), cat.as_ptr(), n, w) };
                let want = cat.iter().filter(|&&x| x != 255).map(|&x| x as u32).max().unwrap_or(0);
                assert_eq!(mx, want);
                let mut rd = vec![0u8; (n + 7) / 8 + 16];
                let nn: Vec<u32> = cat.iter().filter(|&&x| x != 255).map(|&x| x as u32).collect();
                let mut rv = vec![0u8; vb + 64];
                unsafe {
                    pack_any(rd.as_mut_ptr(), n, 1, &|i| (cat[i] != 255) as u32);
                    pack_any(rv.as_mut_ptr(), m, w, &|i| nn[i]);
                }
                assert_eq!(&d[..(n + 7) / 8], &rd[..(n + 7) / 8], "def w {w} n {n}");
                assert_eq!(&d[(n + 7) / 8..], &vec![0u8; d.len() - (n + 7) / 8][..], "def overrun w {w} n {n}");
                assert_eq!(&v[..vb], &rv[..vb], "vals w {w} n {n}");
            }
        }
    }
}
