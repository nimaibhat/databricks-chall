//! Shopping Sessions pipeline: native, parallel, single binary.
//!
//! Phases
//!   0. manifest, products, campaigns (tiny)
//!   1. links  -> sharded ID interning -> union-find -> ID -> person-name map
//!   2. events + clicks parsed in parallel chunks of mmapped files;
//!      events radix-partitioned by event_id, clicks by person
//!   3. event dedup per event_id partition; survivors re-partitioned by person
//!   4. per person partition: group, sessionize, credit, assist, aggregate
//!   5. Parquet output, one file per worker thread
use mimalloc::MiMalloc;
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use std::cell::UnsafeCell;
use std::cmp::Ordering as Cmp;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

mod out;

pub type Key = u128;
const EMPTY: Key = u128::MAX;
const DAY_MS: i64 = 86_400_000;
const GAP_MS: i64 = 1_800_000;
const WEEK_MS: i64 = 7 * DAY_MS;

// ---------------------------------------------------------------------------
// Keys: strings of <= 15 bytes packed big-endian into a u128 with the length
// in the lowest byte, so u128 order == byte-wise string order. Longer strings
// are interned in a global table (key low byte 0xFF) and compared by bytes.
// ---------------------------------------------------------------------------
struct LongTab {
    map: HashMap<Vec<u8>, u32>,
    strs: Vec<Vec<u8>>,
}
static LONG: Mutex<Option<LongTab>> = Mutex::new(None);

#[cold]
#[inline(never)]
fn pack_long(s: &[u8]) -> Key {
    let mut g = LONG.lock().unwrap();
    let t = g.get_or_insert_with(|| LongTab { map: HashMap::new(), strs: Vec::new() });
    let idx = if let Some(&i) = t.map.get(s) {
        i
    } else {
        let i = t.strs.len() as u32;
        t.strs.push(s.to_vec());
        t.map.insert(s.to_vec(), i);
        i
    };
    ((idx as u128) << 8) | 0xFF
}

#[inline(always)]
pub fn pack(s: &[u8]) -> Key {
    let n = s.len();
    if n <= 15 {
        let mut b = [0u8; 16];
        b[..n].copy_from_slice(s);
        b[15] = n as u8;
        u128::from_be_bytes(b)
    } else {
        pack_long(s)
    }
}

#[inline(always)]
fn is_long(k: Key) -> bool {
    (k & 0xFF) == 0xFF
}

pub fn key_bytes(k: Key, out: &mut Vec<u8>) {
    if is_long(k) {
        let g = LONG.lock().unwrap();
        out.extend_from_slice(&g.as_ref().unwrap().strs[(k >> 8) as usize]);
    } else {
        let n = (k & 0xFF) as usize;
        out.extend_from_slice(&k.to_be_bytes()[..n]);
    }
}

#[inline(always)]
fn key_cmp(a: Key, b: Key) -> Cmp {
    if !is_long(a) && !is_long(b) {
        a.cmp(&b)
    } else {
        key_cmp_slow(a, b)
    }
}

#[cold]
fn key_cmp_slow(a: Key, b: Key) -> Cmp {
    let mut x = Vec::new();
    let mut y = Vec::new();
    key_bytes(a, &mut x);
    key_bytes(b, &mut y);
    x.cmp(&y)
}

#[inline(always)]
fn hk(k: Key) -> u64 {
    let lo = k as u64;
    let hi = (k >> 64) as u64;
    let mut x = hi ^ lo.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(23);
    x ^= x >> 32;
    x = x.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    x ^= x >> 29;
    x = x.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^ (x >> 32)
}

// Keys stored as two u64 so records align to 8 and pack tighter.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct K2(u64, u64);
impl K2 {
    #[inline(always)]
    fn from(k: Key) -> K2 {
        K2(k as u64, (k >> 64) as u64)
    }
    #[inline(always)]
    fn get(self) -> Key {
        (self.0 as u128) | ((self.1 as u128) << 64)
    }
}

// ---------------------------------------------------------------------------
// Open-addressing map Key -> V
// ---------------------------------------------------------------------------
pub struct KMap<V: Copy> {
    slots: Vec<(Key, V)>,
    mask: usize,
    pub len: usize,
}
impl<V: Copy + Default> KMap<V> {
    fn with_capacity(n: usize) -> Self {
        let cap = (n * 2).next_power_of_two().max(16);
        KMap { slots: vec![(EMPTY, V::default()); cap], mask: cap - 1, len: 0 }
    }
    #[inline(always)]
    fn get(&self, k: Key) -> Option<V> {
        let mut i = (hk(k) as usize) & self.mask;
        loop {
            let s = unsafe { self.slots.get_unchecked(i) };
            if s.0 == k {
                return Some(s.1);
            }
            if s.0 == EMPTY {
                return None;
            }
            i = (i + 1) & self.mask;
        }
    }
    /// Returns (slot index, inserted?)
    #[inline(always)]
    fn entry(&mut self, k: Key, v: V) -> (usize, bool) {
        if (self.len + 1) * 2 > self.slots.len() {
            self.grow();
        }
        let mut i = (hk(k) as usize) & self.mask;
        loop {
            let s = unsafe { self.slots.get_unchecked_mut(i) };
            if s.0 == k {
                return (i, false);
            }
            if s.0 == EMPTY {
                *s = (k, v);
                self.len += 1;
                return (i, true);
            }
            i = (i + 1) & self.mask;
        }
    }
    fn grow(&mut self) {
        let old = std::mem::take(&mut self.slots);
        let cap = old.len() * 2;
        self.slots = vec![(EMPTY, V::default()); cap];
        self.mask = cap - 1;
        for (k, v) in old {
            if k != EMPTY {
                let mut i = (hk(k) as usize) & self.mask;
                while self.slots[i].0 != EMPTY {
                    i = (i + 1) & self.mask;
                }
                self.slots[i] = (k, v);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parallel helpers
// ---------------------------------------------------------------------------
pub struct PerThread<T>(Vec<UnsafeCell<T>>);
unsafe impl<T: Send> Sync for PerThread<T> {}
impl<T> PerThread<T> {
    fn new(n: usize, f: impl Fn() -> T) -> Self {
        PerThread((0..n).map(|_| UnsafeCell::new(f())).collect())
    }
    #[allow(clippy::mut_from_ref)]
    #[inline(always)]
    unsafe fn get(&self, t: usize) -> &mut T {
        &mut *self.0[t].get()
    }
    fn into_inner(self) -> Vec<T> {
        self.0.into_iter().map(|c| c.into_inner()).collect()
    }
}

fn par_for(n: usize, threads: usize, f: impl Fn(usize, usize) + Sync) {
    let next = AtomicUsize::new(0);
    let threads = threads.min(n.max(1));
    std::thread::scope(|s| {
        for t in 0..threads {
            let f = &f;
            let next = &next;
            s.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                f(t, i);
            });
        }
    });
}

// ---------------------------------------------------------------------------
// CSV dialect
// ---------------------------------------------------------------------------
/// Splits a line into exactly N fields. Quotes must wrap a whole field.
#[inline(always)]
fn split<'a, const N: usize>(line: &'a [u8], out: &mut [&'a [u8]; N]) -> bool {
    let len = line.len();
    if len == 0 {
        return false;
    }
    let mut f = 0usize;
    let mut start = 0usize;
    loop {
        let mut end = start;
        let mut quotes = 0u32;
        while end < len {
            let c = unsafe { *line.get_unchecked(end) };
            if c == b',' {
                break;
            }
            quotes += (c == b'"') as u32;
            end += 1;
        }
        if f >= N {
            return false;
        }
        let field = unsafe { line.get_unchecked(start..end) };
        out[f] = if quotes == 0 {
            field
        } else if quotes == 2 && field.len() >= 2 && field[0] == b'"' && field[field.len() - 1] == b'"' {
            &field[1..field.len() - 1]
        } else {
            return false;
        };
        f += 1;
        if end == len {
            break;
        }
        start = end + 1;
    }
    f == N
}

fn for_each_line(data: &[u8], mut f: impl FnMut(&[u8])) {
    let mut start = 0;
    for nl in memchr::memchr_iter(b'\n', data) {
        if nl > start {
            f(&data[start..nl]);
        }
        start = nl + 1;
    }
    if start < data.len() {
        f(&data[start..]);
    }
}

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------
#[inline(always)]
fn d2(s: &[u8], i: usize) -> u32 {
    let a = s[i].wrapping_sub(b'0') as u32;
    let b = s[i + 1].wrapping_sub(b'0') as u32;
    if a < 10 && b < 10 {
        a * 10 + b
    } else {
        1000
    }
}

#[inline(always)]
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[inline(always)]
fn civil_secs(s: &[u8]) -> Option<i64> {
    // s has at least 19 bytes: YYYY-MM-DDTHH:MM:SS
    let y1 = d2(s, 0);
    let y2 = d2(s, 2);
    let mo = d2(s, 5);
    let da = d2(s, 8);
    let h = d2(s, 11);
    let mi = d2(s, 14);
    let se = d2(s, 17);
    if s[4] != b'-' || s[7] != b'-' || s[10] != b'T' || s[13] != b':' || s[16] != b':' {
        return None;
    }
    if y1 > 99 || y2 > 99 || mo == 0 || mo > 12 || da == 0 || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    let y = (y1 * 100 + y2) as i64;
    let dim = match mo {
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if da > dim {
        return None;
    }
    let days = days_from_civil(y, mo as i64, da as i64);
    Some(days * 86400 + (h * 3600 + mi * 60 + se) as i64)
}

/// Parses any valid `ts` form to epoch ms. None = bad (incl. F3 overflow,
/// which is necessarily outside every window).
#[inline(always)]
fn parse_ts(s: &[u8]) -> Option<i64> {
    let n = s.len();
    if n >= 19 && s[4] == b'-' {
        if n == 20 {
            if s[19] != b'Z' {
                return None;
            }
            return civil_secs(s).map(|v| v * 1000);
        }
        if n == 25 {
            let sign = s[19];
            if (sign != b'+' && sign != b'-') || s[22] != b':' {
                return None;
            }
            let oh = d2(s, 20);
            let om = d2(s, 23);
            if oh > 23 || om > 59 {
                return None;
            }
            let off = (oh * 60 + om) as i64 * 60;
            let base = civil_secs(s)?;
            return Some(if sign == b'+' { base - off } else { base + off } * 1000);
        }
        return None;
    }
    parse_f3(s)
}

#[inline(always)]
fn parse_f3(s: &[u8]) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    let mut v: u64 = 0;
    let mut sig = 0u32;
    for &c in s {
        let d = c.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        if sig > 0 || d != 0 {
            sig += 1;
            if sig > 19 {
                return None;
            }
            v = v * 10 + d as u64;
        }
    }
    if v > i64::MAX as u64 {
        None
    } else {
        Some(v as i64)
    }
}

#[inline(always)]
fn parse_price(s: &[u8]) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut v: u64 = 0;
    for &c in s {
        let d = c.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        v = v * 10 + d as u64;
        if v > 10_000_000 {
            return None;
        }
    }
    if v == 0 {
        None
    } else {
        Some(v as u32)
    }
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------
fn list_csv(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".csv") && !name.starts_with('.') {
                let p = e.path();
                if p.is_file() {
                    v.push(p);
                }
            }
        }
    }
    v.sort();
    v
}

struct Mapped {
    maps: Vec<memmap2::Mmap>,
}
impl Mapped {
    fn open(paths: &[PathBuf]) -> Mapped {
        let mut maps = Vec::new();
        for p in paths {
            let f = File::open(p).expect("open");
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if len == 0 {
                continue;
            }
            let m = unsafe { memmap2::MmapOptions::new().map(&f).expect("mmap") };
            #[cfg(unix)]
            unsafe {
                libc::madvise(m.as_ptr() as *mut _, m.len(), libc::MADV_WILLNEED);
            }
            maps.push(m);
        }
        Mapped { maps }
    }
}

#[derive(Clone, Copy)]
struct Chunk {
    kind: u8,
    file: u32,
    start: usize,
    end: usize,
}

fn chunks_of(kind: u8, m: &Mapped, target: usize, out: &mut Vec<Chunk>) {
    for (fi, map) in m.maps.iter().enumerate() {
        let d: &[u8] = map;
        let Some(h) = memchr::memchr(b'\n', d) else { continue };
        let mut s = h + 1;
        while s < d.len() {
            let mut e = s + target;
            if e >= d.len() {
                e = d.len();
            } else {
                match memchr::memchr(b'\n', &d[e..]) {
                    Some(p) => e = e + p + 1,
                    None => e = d.len(),
                }
            }
            out.push(Chunk { kind, file: fi as u32, start: s, end: e });
            s = e;
        }
    }
}

fn read_small_csv(path: &Path) -> Vec<Vec<Vec<u8>>> {
    let data = std::fs::read(path).unwrap_or_default();
    let mut rows = Vec::new();
    let mut first = true;
    for_each_line(&data, |line| {
        if first {
            first = false;
            return;
        }
        let mut f: [&[u8]; 2] = [&[]; 2];
        if split(line, &mut f) {
            rows.push(vec![f[0].to_vec(), f[1].to_vec()]);
        }
    });
    // handle header-only/first-line logic for files whose first line is empty
    rows
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct ERec {
    eid: K2,
    person: K2,
    ts: i64,
    ingest: i64,
    price: u32,
    cat: u16,
    kind: u8,
    valid: u8,
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct SRec {
    person: K2,
    ts: i64,
    price: u32,
    cat: u16,
    kind: u8,
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct CRec {
    person: K2,
    cid: K2,
    ts: i64,
    camp: u32,
}

const KIND_PURCHASE: u8 = 2;

pub struct Ctx {
    pub ws: i64,
    pub we: i64,
    pub day0: i64,
    pub ndays: usize,
    pub products: KMap<u16>,
    pub cat_names: Vec<Vec<u8>>,
    pub cat_unknown: u16,
    pub campaigns: KMap<u32>,
    pub camp_names: Vec<Vec<u8>>,
    pub camp_channels: Vec<Vec<u8>>,
    pub links: Vec<KMap<Key>>, // sharded by top bits
    pub link_shift: u32,
}

impl Ctx {
    #[inline(always)]
    fn person(&self, uid: Key) -> Key {
        if self.links.is_empty() {
            return uid;
        }
        let h = hk(uid);
        let shard = (h >> self.link_shift) as usize;
        self.links[shard].get(uid).unwrap_or(uid)
    }
}

// ---------------------------------------------------------------------------
// Phase 1: links
// ---------------------------------------------------------------------------
fn build_links(data: &Path, threads: usize) -> (Vec<KMap<Key>>, u32) {
    let t0 = Instant::now();
    let paths = list_csv(&data.join("links"));
    let m = Mapped::open(&paths);
    let mut chunks = Vec::new();
    chunks_of(0, &m, 1 << 20, &mut chunks);
    const SHARD_BITS: u32 = 6;
    const SHARDS: usize = 1 << SHARD_BITS;
    let shift = 64 - SHARD_BITS;
    // per-thread pairs and per-thread per-shard keys
    let pairs = PerThread::new(threads, || Vec::<(Key, Key)>::new());
    let skeys = PerThread::new(threads, || vec![Vec::<Key>::new(); SHARDS]);
    par_for(chunks.len(), threads, |t, i| {
        let c = chunks[i];
        let d = &m.maps[c.file as usize][c.start..c.end];
        let p = unsafe { pairs.get(t) };
        let sk = unsafe { skeys.get(t) };
        for_each_line(d, |line| {
            let mut f: [&[u8]; 2] = [&[]; 2];
            if !split(line, &mut f) || f[0].is_empty() || f[1].is_empty() {
                return;
            }
            let a = pack(f[0]);
            let b = pack(f[1]);
            p.push((a, b));
            sk[(hk(a) >> shift) as usize].push(a);
            sk[(hk(b) >> shift) as usize].push(b);
        });
    });
    let pairs = pairs.into_inner();
    let skeys = skeys.into_inner();
    let npairs: usize = pairs.iter().map(|p| p.len()).sum();
    if npairs == 0 {
        return (Vec::new(), shift);
    }
    // shard maps: key -> local index (stored in value as Key for now)
    let maps = PerThread::new(SHARDS, || KMap::<Key>::with_capacity(16));
    let shard_keys = PerThread::new(SHARDS, || Vec::<Key>::new());
    par_for(SHARDS, threads, |_t, s| {
        let n: usize = skeys.iter().map(|v| v[s].len()).sum();
        let mut mp = KMap::<Key>::with_capacity(n / 2 + 16);
        let ks = unsafe { shard_keys.get(s) };
        for v in &skeys {
            for &k in &v[s] {
                let (_, ins) = mp.entry(k, ks.len() as Key);
                if ins {
                    ks.push(k);
                }
            }
        }
        unsafe { *maps.get(s) = mp };
    });
    let maps = maps.into_inner();
    let shard_keys = shard_keys.into_inner();
    let mut offs = vec![0u32; SHARDS + 1];
    for s in 0..SHARDS {
        offs[s + 1] = offs[s] + shard_keys[s].len() as u32;
    }
    let nids = offs[SHARDS] as usize;
    let gidx = |maps: &Vec<KMap<Key>>, k: Key| -> u32 {
        let s = (hk(k) >> shift) as usize;
        offs[s] + maps[s].get(k).unwrap() as u32
    };
    // pairs -> index pairs (parallel)
    let ipairs = PerThread::new(threads, || Vec::<(u32, u32)>::new());
    par_for(pairs.len(), threads, |t, i| {
        let out = unsafe { ipairs.get(t) };
        out.reserve(pairs[i].len());
        for &(a, b) in &pairs[i] {
            out.push((gidx(&maps, a), gidx(&maps, b)));
        }
    });
    let ipairs = ipairs.into_inner();
    // union-find
    let mut parent: Vec<u32> = (0..nids as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            let g = p[p[x as usize] as usize];
            p[x as usize] = g;
            x = g;
        }
        x
    }
    for v in &ipairs {
        for &(a, b) in v {
            let ra = find(&mut parent, a);
            let rb = find(&mut parent, b);
            if ra != rb {
                if ra < rb {
                    parent[rb as usize] = ra;
                } else {
                    parent[ra as usize] = rb;
                }
            }
        }
    }
    // flatten + min name per root
    let mut all_keys: Vec<Key> = Vec::with_capacity(nids);
    for s in 0..SHARDS {
        all_keys.extend_from_slice(&shard_keys[s]);
    }
    let mut name: Vec<Key> = vec![EMPTY; nids];
    for i in 0..nids {
        let r = find(&mut parent, i as u32) as usize;
        parent[i] = r as u32;
        let k = all_keys[i];
        if name[r] == EMPTY || key_cmp(k, name[r]) == Cmp::Less {
            name[r] = k;
        }
    }
    // write names into maps
    let maps_pt = PerThread(maps.into_iter().map(UnsafeCell::new).collect());
    par_for(SHARDS, threads, |_t, s| {
        let mp: &mut KMap<Key> = unsafe { maps_pt.get(s) };
        let off = offs[s] as usize;
        for slot in mp.slots.iter_mut() {
            if slot.0 != EMPTY {
                let g = off + slot.1 as usize;
                slot.1 = name[parent[g] as usize];
            }
        }
    });
    let maps = maps_pt.into_inner();
    eprintln!("links: {} pairs, {} ids, {:?}", npairs, nids, t0.elapsed());
    (maps, shift)
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------
fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

fn manifest_ts(text: &str, key: &str) -> i64 {
    let at = text.find(&format!("\"{key}\"")).expect("manifest key");
    let rest = &text[at + key.len() + 2..];
    let q1 = rest.find('"').unwrap();
    let rest = &rest[q1 + 1..];
    let q2 = rest.find('"').unwrap();
    parse_ts(rest[..q2].as_bytes()).expect("manifest ts")
}

const P1_BITS: u32 = 8;
const P1: usize = 1 << P1_BITS;
const P2_BITS: u32 = 8;
const P2: usize = 1 << P2_BITS;

fn main() {
    let t_start = Instant::now();
    let data = PathBuf::from(env("DATA_DIR", "/data"));
    let outdir = PathBuf::from(env("OUTPUT_DIR", "/output"));
    let threads: usize = env("NCPU", "0").parse().unwrap_or(0);
    let threads = if threads == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8)
    } else {
        threads
    };

    // ---- phase 0
    let manifest = std::fs::read_to_string(data.join("manifest.json")).expect("manifest");
    let ws = manifest_ts(&manifest, "window_start");
    let we = manifest_ts(&manifest, "window_end");
    let day0 = ws.div_euclid(DAY_MS);
    let ndays = ((we - 1).div_euclid(DAY_MS) - day0 + 1).max(1) as usize;

    let prod_rows = read_small_csv(&data.join("products.csv"));
    let mut cat_set: Vec<Vec<u8>> = prod_rows.iter().map(|r| r[1].clone()).collect();
    cat_set.push(b"unknown".to_vec());
    cat_set.sort();
    cat_set.dedup();
    let cat_index: HashMap<Vec<u8>, u16> =
        cat_set.iter().enumerate().map(|(i, c)| (c.clone(), i as u16)).collect();
    let cat_unknown = cat_index[&b"unknown"[..]];
    let mut products = KMap::<u16>::with_capacity(prod_rows.len() + 16);
    for r in &prod_rows {
        let (i, ins) = products.entry(pack(&r[0]), cat_index[&r[1]]);
        if !ins {
            products.slots[i].1 = cat_index[&r[1]];
        }
    }
    let camp_rows = read_small_csv(&data.join("campaigns.csv"));
    let mut campaigns = KMap::<u32>::with_capacity(camp_rows.len() + 16);
    let mut camp_names = Vec::new();
    let mut camp_channels = Vec::new();
    for r in &camp_rows {
        let (_, ins) = campaigns.entry(pack(&r[0]), camp_names.len() as u32);
        if ins {
            camp_names.push(r[0].clone());
            camp_channels.push(r[1].clone());
        }
    }
    eprintln!("phase0 {:?}", t_start.elapsed());

    // ---- phase 1
    let (links, link_shift) = build_links(&data, threads);
    eprintln!("phase1 done {:?}", t_start.elapsed());

    let ctx = Ctx {
        ws,
        we,
        day0,
        ndays,
        products,
        cat_names: cat_set,
        cat_unknown,
        campaigns,
        camp_names,
        camp_channels,
        links,
        link_shift,
    };

    // ---- phase 2: parse events + clicks
    let ev_m = Mapped::open(&list_csv(&data.join("events")));
    let cl_m = Mapped::open(&list_csv(&data.join("clicks")));
    let mut chunks = Vec::new();
    chunks_of(0, &ev_m, 4 << 20, &mut chunks);
    chunks_of(1, &cl_m, 4 << 20, &mut chunks);
    let ev_bytes: usize = ev_m.maps.iter().map(|m| m.len()).sum();
    let cl_bytes: usize = cl_m.maps.iter().map(|m| m.len()).sum();
    let ev_res = (ev_bytes / 75 / P1 / threads) * 5 / 4 + 64;
    let cl_res = (cl_bytes / 45 / P2 / threads) * 5 / 4 + 64;
    let ebufs = PerThread::new(threads, || (0..P1).map(|_| Vec::<ERec>::with_capacity(ev_res)).collect::<Vec<_>>());
    let cbufs = PerThread::new(threads, || (0..P2).map(|_| Vec::<CRec>::with_capacity(cl_res)).collect::<Vec<_>>());
    let p1_shift = 64 - P1_BITS;
    let p2_shift = 64 - P2_BITS;
    let cws = ws - WEEK_MS;
    let exp: u32 = env("EXP", "0").parse().unwrap();
    par_for(chunks.len(), threads, |t, i| {
        let c = chunks[i];
        if c.kind == 0 {
            let d = &ev_m.maps[c.file as usize][c.start..c.end];
            let eb = unsafe { ebufs.get(t) };
            for_each_line(d, |line| {
                let mut f: [&[u8]; 7] = [&[]; 7];
                if !split(line, &mut f) || f[0].is_empty() {
                    return;
                }
                let eid = pack(f[0]);
                let ingest = parse_ts(f[3]).unwrap_or(i64::MAX);
                let mut r = ERec { eid: K2::from(eid), ingest, ..Default::default() };
                let ok = 'v: {
                    if f[1].is_empty() || f[5].is_empty() {
                        break 'v false;
                    }
                    let Some(ts) = parse_ts(f[2]) else { break 'v false };
                    if ts < ws || ts >= we || ingest - ts > DAY_MS {
                        break 'v false;
                    }
                    let kind = match f[4] {
                        b"view" => 0u8,
                        b"cart" => 1,
                        b"purchase" => 2,
                        _ => break 'v false,
                    };
                    if kind == KIND_PURCHASE {
                        match parse_price(f[6]) {
                            Some(p) => r.price = p,
                            None => break 'v false,
                        }
                    }
                    r.ts = ts;
                    r.kind = kind;
                    r.cat = if exp & 2 != 0 { 0 } else { ctx.products.get(pack(f[5])).unwrap_or(ctx.cat_unknown) };
                    r.person = if exp & 1 != 0 { K2::from(pack(f[1])) } else { K2::from(ctx.person(pack(f[1]))) };
                    true
                };
                r.valid = ok as u8;
                if exp & 4 != 0 { std::hint::black_box(r); return; }
                eb[(hk(eid) >> p1_shift) as usize].push(r);
            });
        } else {
            let d = &cl_m.maps[c.file as usize][c.start..c.end];
            let cb = unsafe { cbufs.get(t) };
            for_each_line(d, |line| {
                let mut f: [&[u8]; 4] = [&[]; 4];
                if !split(line, &mut f) || f[0].is_empty() || f[1].is_empty() {
                    return;
                }
                let Some(ts) = parse_ts(f[3]) else { return };
                if ts < cws || ts >= we {
                    return;
                }
                let Some(camp) = ctx.campaigns.get(pack(f[2])) else { return };
                let person = ctx.person(pack(f[1]));
                cb[(hk(person) >> p2_shift) as usize].push(CRec {
                    person: K2::from(person),
                    cid: K2::from(pack(f[0])),
                    ts,
                    camp,
                });
            });
        }
    });
    let ebufs = ebufs.into_inner();
    let cbufs = cbufs.into_inner();
    eprintln!("phase2 parse {:?}", t_start.elapsed());

    // ---- phase 3: dedup per eid partition
    let sres = (ev_bytes / 80 / P2 / threads) * 5 / 4 + 64;
    let sbufs = PerThread::new(threads, || (0..P2).map(|_| Vec::<SRec>::with_capacity(sres)).collect::<Vec<_>>());
    par_for(P1, threads, |t, p| {
        let n: usize = ebufs.iter().map(|b| b[p].len()).sum();
        let mut mp = KMap::<u32>::with_capacity(n);
        let mut all: Vec<&ERec> = Vec::with_capacity(n);
        for b in &ebufs {
            all.extend(b[p].iter());
        }
        for (i, r) in all.iter().enumerate() {
            let (s, ins) = mp.entry(r.eid.get(), i as u32);
            if !ins {
                let cur = mp.slots[s].1 as usize;
                if r.ingest < all[cur].ingest {
                    mp.slots[s].1 = i as u32;
                }
            }
        }
        let sb = unsafe { sbufs.get(t) };
        for s in &mp.slots {
            if s.0 != EMPTY {
                let r = all[s.1 as usize];
                if r.valid != 0 {
                    sb[(hk(r.person.get()) >> p2_shift) as usize].push(SRec {
                        person: r.person,
                        ts: r.ts,
                        price: r.price,
                        cat: r.cat,
                        kind: r.kind,
                    });
                }
            }
        }
    });
    drop(ebufs);
    let sbufs = sbufs.into_inner();
    eprintln!("phase3 dedup {:?}", t_start.elapsed());

    // ---- phase 4: per person partition
    let ncamp = ctx.camp_names.len();
    let ncat = ctx.cat_names.len();
    let acc = PerThread::new(threads, || out::Acc::new(ncamp + 1, ndays));
    par_for(P2, threads, |t, p| {
        let a = unsafe { acc.get(t) };
        process_partition(&ctx, p, &sbufs, &cbufs, a, ncat);
    });
    let accs = acc.into_inner();
    eprintln!("phase4 sessions {:?}", t_start.elapsed());

    // ---- phase 5: output
    out::write_all(&outdir, &ctx, accs, threads);
    eprintln!("done {:?}", t_start.elapsed());
}

struct Sess {
    start: i64,
    day: i32,
}

fn process_partition(
    ctx: &Ctx,
    p: usize,
    sbufs: &[Vec<Vec<SRec>>],
    cbufs: &[Vec<Vec<CRec>>],
    a: &mut out::Acc,
    ncat: usize,
) {
    let n: usize = sbufs.iter().map(|b| b[p].len()).sum();
    if n == 0 {
        return;
    }
    // group events by person
    let mut gmap = KMap::<u32>::with_capacity(n / 8 + 16);
    let mut gkeys: Vec<Key> = Vec::new();
    let mut counts: Vec<u32> = Vec::new();
    let mut gid: Vec<u32> = Vec::with_capacity(n);
    for b in sbufs {
        for r in &b[p] {
            let k = r.person.get();
            let (s, ins) = gmap.entry(k, gkeys.len() as u32);
            if ins {
                gkeys.push(k);
                counts.push(0);
            }
            let g = gmap.slots[s].1;
            counts[g as usize] += 1;
            gid.push(g);
        }
    }
    let ng = gkeys.len();
    let mut off = vec![0u32; ng + 1];
    for g in 0..ng {
        off[g + 1] = off[g] + counts[g];
    }
    let mut pos: Vec<u32> = off[..ng].to_vec();
    let mut ev: Vec<SRec> = vec![SRec::default(); n];
    let mut i = 0;
    for b in sbufs {
        for r in &b[p] {
            let g = gid[i] as usize;
            ev[pos[g] as usize] = *r;
            pos[g] += 1;
            i += 1;
        }
    }
    drop(gid);
    // group clicks by person (only persons with events)
    let mut cgid: Vec<u32> = Vec::new();
    let mut ccounts = vec![0u32; ng];
    let mut cl_tmp: Vec<&CRec> = Vec::new();
    for b in cbufs {
        for r in &b[p] {
            if let Some(g) = gmap.get(r.person.get()) {
                ccounts[g as usize] += 1;
                cgid.push(g);
                cl_tmp.push(r);
            }
        }
    }
    let mut coff = vec![0u32; ng + 1];
    for g in 0..ng {
        coff[g + 1] = coff[g] + ccounts[g];
    }
    let mut cpos: Vec<u32> = coff[..ng].to_vec();
    let mut cl: Vec<CRec> = vec![CRec::default(); cl_tmp.len()];
    for (j, r) in cl_tmp.iter().enumerate() {
        let g = cgid[j] as usize;
        cl[cpos[g] as usize] = **r;
        cpos[g] += 1;
    }
    drop(cl_tmp);

    let mut cat_rev = vec![0u64; ncat];
    let mut touched: Vec<u16> = Vec::new();
    let mut sess: Vec<Sess> = Vec::new();
    let mut runs: Vec<(i32, u32, u32)> = Vec::new(); // (day, first sess idx, end idx)
    let mut ac: Vec<(u32, i64)> = Vec::new();
    let organic = ctx.camp_names.len() as u32;

    for g in 0..ng {
        let evs = &mut ev[off[g] as usize..off[g + 1] as usize];
        evs.sort_unstable_by_key(|r| r.ts);
        let cls = &mut cl[coff[g] as usize..coff[g + 1] as usize];
        if cls.len() > 1 {
            cls.sort_unstable_by(|x, y| x.ts.cmp(&y.ts).then_with(|| key_cmp(x.cid.get(), y.cid.get())));
        }
        let person = gkeys[g];
        sess.clear();
        runs.clear();
        // user-day accumulators
        let mut cur_day: i32 = i32::MIN;
        let mut d_sess = 0u32;
        let mut d_events = 0u32;
        let mut d_rev = 0u64;
        let mut cj = 0usize; // click pointer: clicks[..cj] have ts <= current session start

        let mut k = 0usize;
        let ne = evs.len();
        while k < ne {
            // one session starting at k
            let start = evs[k].ts;
            let mut last = start;
            let mut cnt = 0u32;
            let mut rev = 0u64;
            let mut purchase = false;
            let day = start.div_euclid(DAY_MS) as i32;
            if day != cur_day {
                if cur_day != i32::MIN {
                    flush_user(a, person, cur_day, d_sess, d_events, d_rev, &mut cat_rev, &mut touched);
                }
                cur_day = day;
                d_sess = 0;
                d_events = 0;
                d_rev = 0;
            }
            while k < ne && evs[k].ts - last <= GAP_MS {
                let r = &evs[k];
                last = r.ts;
                cnt += 1;
                if r.kind == KIND_PURCHASE {
                    purchase = true;
                    rev += r.price as u64;
                    let c = r.cat as usize;
                    if cat_rev[c] == 0 {
                        touched.push(r.cat);
                    }
                    cat_rev[c] += r.price as u64;
                }
                k += 1;
            }
            d_sess += 1;
            d_events += cnt;
            d_rev += rev;
            // credit
            while cj < cls.len() && cls[cj].ts <= start {
                cj += 1;
            }
            let mut camp = organic;
            if cj > 0 {
                let lt = cls[cj - 1].ts;
                if start - lt <= WEEK_MS {
                    let mut q = cj - 1;
                    while q > 0 && cls[q - 1].ts == lt {
                        q -= 1;
                    }
                    camp = cls[q].camp;
                }
            }
            let di = (day as i64 - ctx.day0) as usize;
            let cell = a.camp_cell(camp as usize, di);
            cell[0] += 1;
            cell[1] += purchase as u64;
            cell[2] += rev;
            let si = sess.len() as u32;
            match runs.last_mut() {
                Some(r) if r.0 == day => r.2 = si + 1,
                _ => runs.push((day, si, si + 1)),
            }
            sess.push(Sess { start, day });
        }
        flush_user(a, person, cur_day, d_sess, d_events, d_rev, &mut cat_rev, &mut touched);

        // assists
        if !cls.is_empty() {
            ac.clear();
            ac.extend(cls.iter().map(|c| (c.camp, c.ts)));
            ac.sort_unstable();
            let mut q = 0;
            while q < ac.len() {
                let camp = ac[q].0;
                let mut a_lo = ac[q].1;
                let mut a_hi = a_lo + WEEK_MS;
                q += 1;
                loop {
                    let more = q < ac.len() && ac[q].0 == camp;
                    if more && ac[q].1 <= a_hi {
                        a_hi = ac[q].1 + WEEK_MS;
                        q += 1;
                        continue;
                    }
                    // flush [a_lo, a_hi]
                    let lo = sess.partition_point(|s| s.start < a_lo);
                    let hi = sess.partition_point(|s| s.start <= a_hi);
                    if lo < hi {
                        let lo = lo as u32;
                        let hi = hi as u32;
                        for r in runs.iter() {
                            if r.2 <= lo {
                                continue;
                            }
                            if r.1 >= hi {
                                break;
                            }
                            let c = hi.min(r.2) - lo.max(r.1);
                            let di = (r.0 as i64 - ctx.day0) as usize;
                            a.camp_cell(camp as usize, di)[3] += c as u64;
                        }
                    }
                    if !more {
                        break;
                    }
                    a_lo = ac[q].1;
                    a_hi = a_lo + WEEK_MS;
                    q += 1;
                }
            }
        }
    }
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn flush_user(
    a: &mut out::Acc,
    person: Key,
    day: i32,
    sessions: u32,
    events: u32,
    rev: u64,
    cat_rev: &mut [u64],
    touched: &mut Vec<u16>,
) {
    let mut top = u16::MAX;
    let mut best = 0u64;
    for &c in touched.iter() {
        let v = cat_rev[c as usize];
        if v > best || (v == best && c < top) {
            best = v;
            top = c;
        }
        cat_rev[c as usize] = 0;
    }
    touched.clear();
    a.users.push(out::URow { person, day, sessions, events, revenue: rev, top });
}

#[allow(dead_code)]
fn _unused(_: Arc<()>) {}
