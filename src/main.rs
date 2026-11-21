//! Shopping Sessions pipeline: native, parallel, single binary.
//!
//! Phases
//!   0. manifest, products, campaigns (tiny)
//!   1. links  -> sharded ID interning -> union-find -> ID -> person-name map
//!   2. events + clicks parsed in parallel chunks of mmapped files;
//!      events radix-partitioned by event_id, clicks by person
//!   3. event dedup per event_id partition; survivors re-partitioned by person
//!   4. per person partition: group, sessionize, credit, assist, aggregate
//!   5. Parquet output
use mimalloc::MiMalloc;
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

mod keys;
mod mem;
mod out;
mod parse;
mod pq;

use keys::*;
use mem::*;
use parse::*;

const DAY_MS: i64 = 86_400_000;
const GAP_MS: i64 = 1_800_000;
const WEEK_MS: i64 = 7 * DAY_MS;

// ---------------------------------------------------------------------------
// Parallel helpers
// ---------------------------------------------------------------------------
pub struct PerThread<T>(Vec<UnsafeCell<T>>);
unsafe impl<T: Send> Sync for PerThread<T> {}
impl<T> PerThread<T> {
    fn new(n: usize, f: impl Fn() -> T) -> Self {
        PerThread((0..n).map(|_| UnsafeCell::new(f())).collect())
    }
    fn from_vec(v: Vec<T>) -> Self {
        PerThread(v.into_iter().map(UnsafeCell::new).collect())
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
// Records
// ---------------------------------------------------------------------------
/// One parsed event copy. `pc` = price << 8 | category, or INVALID.
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct ERec {
    eid: Key,
    person: Key,
    ts: u32,     // ms since window start (valid copies only)
    ingest: i32, // seconds since window start, saturated
    pc: u32,
    _pad: u32, // 32 B so whole records fill the 128 B write-combining stage
}
const INVALID: u32 = u32::MAX;

/// A surviving event. Purchase <=> price > 0.
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct SRec {
    person: Key,
    ts: u32,
    pc: u32,
}

/// One event copy for dedup: `at` is the address of its SRec in the person
/// partitions, or 0 when the copy is invalid.
#[derive(Clone, Copy, Default)]
#[repr(C, packed(4))]
struct LRec {
    eid: Key,
    ingest: i32,
    at: u64,
}

#[derive(Clone, Copy, Default)]
struct Pend {
    eid: Key,
    uid: Key,
    h: u64,
    ingest: i32,
    ts: u32,
    pc: u32,
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct CRec {
    person: Key,
    cid: Key,
    ts: i64,
    camp: u32,
}

pub struct Ctx {
    pub ws: i64,
    pub we: i64,
    pub day0: i64,
    pub ndays: usize,
    pub products: Catalog,
    pub cat_names: Vec<Vec<u8>>,
    pub cat_unknown: u16,
    pub campaigns: Catalog,
    pub camp_names: Vec<Vec<u8>>,
    pub camp_channels: Vec<Vec<u8>>,
    pub links: Vec<KMap<Key>>, // sharded by top bits of hk
    pub link_shift: u32,
    pub flat: Flat,
}

impl Ctx {
    #[inline(always)]
    fn prefetch_person(&self, h: u64) {
        if !self.flat.base.is_null() {
            prefetch(unsafe { self.flat.base.add(self.flat.home(h)) });
        } else if !self.links.is_empty() {
            prefetch(self.links[(h >> self.link_shift) as usize].slot_ptr(h));
        }
    }
    #[inline(always)]
    fn person_h(&self, uid: Key, h: u64) -> Key {
        if !self.flat.base.is_null() {
            return self.flat.get_h(uid, h).unwrap_or(uid);
        }
        if self.links.is_empty() {
            return uid;
        }
        self.links[(h >> self.link_shift) as usize].get_h(uid, h).unwrap_or(uid)
    }
}

/// The link maps copied into one huge-page table with equal-size shard
/// regions (same shard bits as the maps), so a lookup misses no TLB entry.
pub struct Flat {
    base: *mut (Key, Key),
    bits: u32,
    shift: u32,
}
unsafe impl Sync for Flat {}
unsafe impl Send for Flat {}

impl Flat {
    fn empty() -> Flat {
        Flat { base: std::ptr::null_mut(), bits: 0, shift: 64 }
    }
    #[inline(always)]
    fn home(&self, h: u64) -> usize {
        (((h >> self.shift) as usize) << self.bits) | (h as usize & ((1usize << self.bits) - 1))
    }
    #[inline(always)]
    fn get_h(&self, k: Key, h: u64) -> Option<Key> {
        let m = (1usize << self.bits) - 1;
        let rb = ((h >> self.shift) as usize) << self.bits;
        let mut i = h as usize & m;
        loop {
            let s = unsafe { *self.base.add(rb | i) };
            if s.0 == k {
                return Some(s.1);
            }
            if s.0 == EMPTY {
                return None;
            }
            i = (i + 1) & m;
        }
    }
    #[cfg(target_os = "linux")]
    fn build(maps: &[KMap<Key>], shift: u32, threads: usize) -> Flat {
        if maps.is_empty() {
            return Flat::empty();
        }
        // ids that name themselves are left out: a miss already means "self"
        let counts: Vec<usize> = maps.iter().map(|m| m.slots.iter().filter(|s| s.0 != EMPTY && s.0 != s.1).count()).collect();
        let maxn = counts.iter().copied().max().unwrap_or(0);
        eprintln!("flat: {} remapping ids, max {} per shard", counts.iter().sum::<usize>(), maxn);
        let cap = (maxn + maxn / 8 + 16).next_power_of_two();
        let bits = cap.trailing_zeros();
        let bytes = maps.len() * cap * std::mem::size_of::<(Key, Key)>();
        let p = unsafe {
            libc::mmap(std::ptr::null_mut(), bytes, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0)
        };
        if p == libc::MAP_FAILED {
            return Flat::empty();
        }
        unsafe { libc::madvise(p, bytes, libc::MADV_HUGEPAGE) };
        let f = Flat { base: p as *mut (Key, Key), bits, shift };
        let fr = &f;
        par_for(maps.len(), threads, |_t, s| {
            let m = cap - 1;
            let region = unsafe { std::slice::from_raw_parts_mut(fr.base.add(s << bits), cap) };
            region.fill((EMPTY, EMPTY));
            for &(k, v) in &maps[s].slots {
                if k == EMPTY || k == v {
                    continue;
                }
                let mut i = hk(k) as usize & m;
                while region[i].0 != EMPTY {
                    i = (i + 1) & m;
                }
                region[i] = (k, v);
            }
        });
        f
    }
    #[cfg(not(target_os = "linux"))]
    fn build(_maps: &[KMap<Key>], _shift: u32, _threads: usize) -> Flat {
        Flat::empty()
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
    let shard_bits: u32 = p2_bits();
    let shards: usize = 1 << shard_bits;
    let shift = 64 - shard_bits;
    let pairs = PerThread::new(threads, Vec::<(Key, Key)>::new);
    let skeys = PerThread::new(threads, || vec![Vec::<Key>::new(); shards]);
    par_for(chunks.len(), threads, |t, i| {
        let c = chunks[i];
        let d: &[u8] = &m.maps[c.file as usize];
        let p = unsafe { pairs.get(t) };
        let sk = unsafe { skeys.get(t) };
        scan_lines_fast::<2>(d, c.start, c.end, |f| {
            if f[0].is_empty() || f[1].is_empty() {
                return;
            }
            let a = pack(f[0]);
            let b = pack(f[1]);
            if a == b {
                return; // self-link: no effect
            }
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
    // shard maps: key -> local index (temporarily stored as the value)
    let maps = PerThread::new(shards, || KMap::<Key>::with_capacity(16));
    let shard_keys = PerThread::new(shards, Vec::<Key>::new);
    par_for(shards, threads, |_t, s| {
        let n: usize = skeys.iter().map(|v| v[s].len()).sum();
        let mut mp = KMap::<Key>::with_capacity(n * 3 / 4 + 16);
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
    drop(skeys);
    let maps = maps.into_inner();
    let shard_keys = shard_keys.into_inner();
    let mut offs = vec![0u32; shards + 1];
    for s in 0..shards {
        offs[s + 1] = offs[s] + shard_keys[s].len() as u32;
    }
    let nids = offs[shards] as usize;
    let gidx = |k: Key| -> u32 {
        let h = hk(k);
        let s = (h >> shift) as usize;
        offs[s] + maps[s].get_h(k, h).unwrap() as u32
    };
    let ipairs = PerThread::new(threads, Vec::<(u32, u32)>::new);
    par_for(pairs.len(), threads, |t, i| {
        let out = unsafe { ipairs.get(t) };
        out.reserve(pairs[i].len());
        for &(a, b) in &pairs[i] {
            out.push((gidx(a), gidx(b)));
        }
    });
    let ipairs = ipairs.into_inner();
    let t_uf = Instant::now();
    let mut parent: Vec<u32> = (0..nids as u32).collect();
    #[inline(always)]
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        unsafe {
            while *p.get_unchecked(x as usize) != x {
                let g = *p.get_unchecked(*p.get_unchecked(x as usize) as usize);
                *p.get_unchecked_mut(x as usize) = g;
                x = g;
            }
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
    let mut all_keys: Vec<Key> = Vec::with_capacity(nids);
    for s in 0..shards {
        all_keys.extend_from_slice(&shard_keys[s]);
    }
    let mut name: Vec<Key> = vec![EMPTY; nids];
    for i in 0..nids {
        let r = find(&mut parent, i as u32) as usize;
        parent[i] = r as u32;
        let k = all_keys[i];
        if name[r] == EMPTY || key_cmp(k, name[r]) == std::cmp::Ordering::Less {
            name[r] = k;
        }
    }
    let uf_time = t_uf.elapsed();
    let maps_pt = PerThread::from_vec(maps);
    par_for(shards, threads, |_t, s| {
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
    eprintln!("links: {} pairs, {} ids, uf {:?}, total {:?}", npairs, nids, uf_time, t0.elapsed());
    (maps, shift)
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------
fn cpu() -> String {
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        format!(
            "[u {:.2} s {:.2} flt {}]",
            ru.ru_utime.tv_sec as f64 + ru.ru_utime.tv_usec as f64 * 1e-6,
            ru.ru_stime.tv_sec as f64 + ru.ru_stime.tv_usec as f64 * 1e-6,
            ru.ru_minflt
        )
    }
}

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

// Partition counts (bits), from env P1BITS / P2BITS, set once at start.
static P1_BITS_V: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(9);
static P2_BITS_V: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(9);
#[inline(always)]
fn p1_bits() -> u32 {
    P1_BITS_V.load(Ordering::Relaxed)
}
#[inline(always)]
fn p2_bits() -> u32 {
    P2_BITS_V.load(Ordering::Relaxed)
}
const LAG: usize = 16;

/// [thread][partition] -> [partition][thread]
fn transpose<T>(v: Vec<Vec<T>>, parts: usize) -> Vec<Vec<T>> {
    let mut out: Vec<Vec<T>> = (0..parts).map(|_| Vec::with_capacity(v.len())).collect();
    for row in v {
        for (p, x) in row.into_iter().enumerate() {
            out[p].push(x);
        }
    }
    out
}

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
    P1_BITS_V.store(env("P1BITS", "9").parse().unwrap_or(9), Ordering::Relaxed);
    P2_BITS_V.store(env("P2BITS", "9").parse().unwrap_or(9), Ordering::Relaxed);
    mem::wc_init();
    let manifest = std::fs::read_to_string(data.join("manifest.json")).expect("manifest");
    let ws = manifest_ts(&manifest, "window_start");
    let we = manifest_ts(&manifest, "window_end");
    assert!(we - ws < (1i64 << 32), "window too long for u32 offsets");
    let day0 = ws.div_euclid(DAY_MS);
    let ndays = ((we - 1).div_euclid(DAY_MS) - day0 + 1).max(1) as usize;

    let prod_rows = read_small_csv(&data.join("products.csv"));
    let mut cat_set: Vec<Vec<u8>> = prod_rows.iter().map(|r| r[1].clone()).collect();
    cat_set.push(b"unknown".to_vec());
    cat_set.sort();
    cat_set.dedup();
    assert!(cat_set.len() <= 256, "too many categories");
    let cat_index: HashMap<Vec<u8>, u16> =
        cat_set.iter().enumerate().map(|(i, c)| (c.clone(), i as u16)).collect();
    let cat_unknown = cat_index[&b"unknown"[..]];
    let mut products = Catalog::new(prod_rows.len());
    for r in &prod_rows {
        products.insert(&r[0], cat_index[&r[1]] as u32, true);
    }
    let camp_rows = read_small_csv(&data.join("campaigns.csv"));
    let mut campaigns = Catalog::new(camp_rows.len());
    let mut camp_names = Vec::new();
    let mut camp_channels = Vec::new();
    for r in &camp_rows {
        if campaigns.insert(&r[0], camp_names.len() as u32, false) {
            camp_names.push(r[0].clone());
            camp_channels.push(r[1].clone());
        }
    }

    products.finish();
    campaigns.finish();

    // map the inputs early so the arena can be sized
    let ev_m = Mapped::open(&list_csv(&data.join("events")));
    let cl_m = Mapped::open(&list_csv(&data.join("clicks")));
    background_readahead(&[&ev_m, &cl_m]);
    let ev_bytes: usize = ev_m.maps.iter().map(|m| m.len()).sum();
    let cl_bytes: usize = cl_m.maps.iter().map(|m| m.len()).sum();
    // ERec 32 B per ~78 B line, CRec 32 B per ~45 B line, plus partial blocks
    // (one-shuffle: SRec 16 B + LRec 20 B per event line)
    let est = ev_bytes / 78 * 36 + cl_bytes / 40 * 32 + ((1usize << p1_bits()) + 3 * (1usize << p2_bits())) * threads * BLK * 2 + (256 << 20);
    let arena = Arena::new(est * 3 / 2);

    // ---- phase 1, concurrent with phase 2: parsing never reads the link maps
    let oneshuf = env("ONESHUF", "0") == "1";
    let link_threads: usize = env("LINKT", "0").parse().unwrap_or(0);
    let link_threads = if link_threads != 0 { link_threads } else if oneshuf { threads } else { (threads / 2).max(1) };
    let links_job = {
        let data = data.clone();
        std::thread::spawn(move || build_links(&data, link_threads))
    };

    let mut ctx = Ctx {
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
        links: Vec::new(),
        link_shift: 64,
        flat: Flat::empty(),
    };

    let links_job = if oneshuf || env("LINKFIRST", "0") == "1" {
        let (links, link_shift) = links_job.join().expect("links");
        ctx.links = links;
        ctx.link_shift = link_shift;
        if env("FLAT", "1") == "1" {
            ctx.flat = Flat::build(&ctx.links, ctx.link_shift, threads);
        }
        eprintln!("phase1 first {:?}", t_start.elapsed());
        std::thread::spawn(|| (Vec::new(), 64))
    } else {
        links_job
    };
    if env("BENCH", "0") == "2" {
        let (links, link_shift) = links_job.join().expect("links");
        if !links.is_empty() {
            ctx.links = links;
            ctx.link_shift = link_shift;
        }
        microbench(&ctx, &ev_m, &cl_m, &arena);
        return;
    }
    // ---- phase 2: parse events + clicks
    let t_parse = Instant::now();
    let mut chunks = Vec::new();
    chunks_of(0, &ev_m, 4 << 20, &mut chunks);
    chunks_of(1, &cl_m, 4 << 20, &mut chunks);
    let blocks = PerThread::new(threads, || Blocks::new(&arena));
    let (sparts, cparts) = if oneshuf {
        let lbl = PerThread::new(threads, || (0..(1usize << p1_bits())).map(|_| BList::<LRec>::new()).collect::<Vec<_>>());
        let sbl = PerThread::new(threads, || (0..(1usize << p2_bits())).map(|_| BList::<SRec>::new()).collect::<Vec<_>>());
        let cbl = PerThread::new(threads, || (0..(1usize << p2_bits())).map(|_| BList::<CRec>::new()).collect::<Vec<_>>());
        let iobufs = PerThread::new(threads, Vec::<u8>::new);
        par_for(chunks.len(), threads, |t, i| {
            let c = chunks[i];
            let buf = unsafe { iobufs.get(t) };
            let bs = unsafe { blocks.get(t) };
            if c.kind == 0 {
                let (d, s, e) = chunk_data(&ev_m, c.file as usize, c.start, c.end, buf);
                parse_events1(&ctx, d, s, e, unsafe { lbl.get(t) }, unsafe { sbl.get(t) }, bs);
                chunk_done(&ev_m, c.file as usize, c.start, c.end);
            } else {
                let (d, s, e) = chunk_data(&cl_m, c.file as usize, c.start, c.end, buf);
                parse_clicks1(&ctx, d, s, e, unsafe { cbl.get(t) }, bs);
                chunk_done(&cl_m, c.file as usize, c.start, c.end);
            }
        });
        std::mem::forget(iobufs);
        let lparts = PerThread::from_vec(transpose(lbl.into_inner(), 1usize << p1_bits()));
        eprintln!("phase2 parse {:?} (parse only {:?}) {}", t_start.elapsed(), t_parse.elapsed(), cpu());
        let dscr = PerThread::new(threads, || (KMap::<u64>::with_capacity(16), Vec::<u64>::new()));
        par_for(1usize << p1_bits(), threads, |t, p| {
            let (mp, dead) = unsafe { dscr.get(t) };
            dedup_mark(unsafe { lparts.get(p) }, mp, dead, unsafe { blocks.get(t) });
        });
        eprintln!("phase3 dedup {:?} {}", t_start.elapsed(), cpu());
        (transpose(sbl.into_inner(), 1usize << p2_bits()), transpose(cbl.into_inner(), 1usize << p2_bits()))
    } else {
    let ebl = PerThread::new(threads, || (0..(1usize << p1_bits())).map(|_| BList::<ERec>::new()).collect::<Vec<_>>());
    let cbl = PerThread::new(threads, || (0..(1usize << p2_bits())).map(|_| BList::<CRec>::new()).collect::<Vec<_>>());
    let iobufs = PerThread::new(threads, Vec::<u8>::new);
    par_for(chunks.len(), threads, |t, i| {
        let c = chunks[i];
        let buf = unsafe { iobufs.get(t) };
        let bs = unsafe { blocks.get(t) };
        if io_mode() == 5 {
            if c.kind == 0 {
                let eb = unsafe { ebl.get(t) };
                for_blocks(&ev_m, c.file as usize, c.start, c.end, buf, |d, s, e| parse_events(&ctx, d, s, e, eb, bs));
            } else {
                let cb = unsafe { cbl.get(t) };
                for_blocks(&cl_m, c.file as usize, c.start, c.end, buf, |d, s, e| parse_clicks(&ctx, d, s, e, cb, bs));
            }
        } else if c.kind == 0 {
            let (d, s, e) = chunk_data(&ev_m, c.file as usize, c.start, c.end, buf);
            parse_events(&ctx, d, s, e, unsafe { ebl.get(t) }, bs);
            chunk_done(&ev_m, c.file as usize, c.start, c.end);
        } else {
            let (d, s, e) = chunk_data(&cl_m, c.file as usize, c.start, c.end, buf);
            parse_clicks(&ctx, d, s, e, unsafe { cbl.get(t) }, bs);
            chunk_done(&cl_m, c.file as usize, c.start, c.end);
        }
    });
    std::mem::forget(iobufs);
    flush_all(&ebl, &blocks, threads);
    flush_all(&cbl, &blocks, threads);
    let eparts = PerThread::from_vec(transpose(ebl.into_inner(), 1usize << p1_bits()));
    let cparts = transpose(cbl.into_inner(), 1usize << p2_bits());
    eprintln!("phase2 parse {:?} (parse only {:?}) {}", t_start.elapsed(), t_parse.elapsed(), cpu());
    let (links, link_shift) = links_job.join().expect("links");
    if !links.is_empty() {
        ctx.links = links;
        ctx.link_shift = link_shift;
        if env("FLATOLD", "0") == "1" {
            ctx.flat = Flat::build(&ctx.links, ctx.link_shift, threads);
        }
    }
    eprintln!("phase1 joined {:?}", t_start.elapsed());

    // ---- phase 3: dedup per eid partition, survivors partitioned by person
    let sbl = PerThread::new(threads, || (0..(1usize << p2_bits())).map(|_| BList::<SRec>::new()).collect::<Vec<_>>());
    let dscr = PerThread::new(threads, || (KMap::<u64>::with_capacity(16), Vec::<u64>::new(), Vec::<u64>::new(), Vec::new()));
    par_for(1usize << p1_bits(), threads, |t, p| {
        let lists = unsafe { eparts.get(p) };
        let (mp, dead, filt, susp) = unsafe { dscr.get(t) };
        let bs = unsafe { blocks.get(t) };
        let sb = unsafe { sbl.get(t) };
        dedup_partition(&ctx, lists, mp, dead, filt, susp, sb, bs);
    });
    flush_all(&sbl, &blocks, threads);
    let sparts = transpose(sbl.into_inner(), 1usize << p2_bits());
    eprintln!("phase3 dedup {:?} {}", t_start.elapsed(), cpu());

    // ---- phase 3b: clicks user -> person, per user-hash partition (events were resolved in dedup)
    let cparts = PerThread::from_vec(cparts);
    let cmv = PerThread::new(threads, || (0..(1usize << p2_bits())).map(|_| BList::<CRec>::new()).collect::<Vec<_>>());
    if !ctx.links.is_empty() {
        par_for(1usize << p2_bits(), threads, |t, p| {
            let bs = unsafe { blocks.get(t) };
            let map = Some(&ctx.links[p]);
            resolve_partition(p, map, unsafe { cparts.get(p) }, |r: &mut CRec| &mut r.person, unsafe { cmv.get(t) }, bs);
        });
    }
    let mut cparts = cparts.into_inner();
    for (p, v) in transpose(cmv.into_inner(), 1usize << p2_bits()).into_iter().enumerate() {
        cparts[p].extend(v.into_iter().filter(|l| l.len > 0));
    }
    eprintln!("phase3b resolve {:?} {}", t_start.elapsed(), cpu());
    (sparts, cparts)
    };

    // ---- phase 4: per person partition
    let ncamp = ctx.camp_names.len();
    let ncat = ctx.cat_names.len();
    let acc = PerThread::new(threads, || out::Acc::new(ncamp + 1, ndays));
    let pscr = PerThread::new(threads, Scratch::default);
    let ptimes = std::sync::Mutex::new(Vec::new());
    par_for(1usize << p2_bits(), threads, |t, p| {
        let a = unsafe { acc.get(t) };
        let sc = unsafe { pscr.get(t) };
        let t0 = Instant::now();
        process_partition(&ctx, &sparts[p], &cparts[p], a, sc, ncat);
        let n: usize = sparts[p].iter().map(|l| l.len).sum();
        ptimes.lock().unwrap().push((t0.elapsed().as_micros(), n, p));
    });
    {
        let mut v = ptimes.into_inner().unwrap();
        v.sort();
        let tot: u128 = v.iter().map(|x| x.0).sum();
        eprintln!("phase4 partitions: total {} us, top: {:?}", tot, &v[v.len().saturating_sub(5)..]);
    }
    let accs = acc.into_inner();
    eprintln!("phase4 sessions {:?} {} arena used {} MB of {} MB", t_start.elapsed(), cpu(), arena.used() >> 20, (est * 3 / 2) >> 20);

    // ---- phase 5: output
    out::write_all(&outdir, &ctx, accs);
    eprintln!("done {:?} {}", t_start.elapsed(), cpu());
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        eprintln!(
            "rusage: user {}.{:06}s sys {}.{:06}s minflt {} majflt {} maxrss {} threads {}",
            ru.ru_utime.tv_sec, ru.ru_utime.tv_usec, ru.ru_stime.tv_sec, ru.ru_stime.tv_usec,
            ru.ru_minflt, ru.ru_majflt, ru.ru_maxrss, threads
        );
    }
    if env("AREL", "0") == "1" {
        arena.release_parallel(threads);
    }
    // everything is leaked to the kernel: no destructor walks
    std::process::exit(0);
}

fn microbench(ctx: &Ctx, ev_m: &Mapped, cl_m: &Mapped, arena: &Arena) {
    let mut ch = Vec::new();
    chunks_of(0, ev_m, 4 << 20, &mut ch);
    let ch = &ch[..ch.len().min(40)];
    for c in ch {
        let m = &ev_m.maps[c.file as usize];
        let mut x = 0u64;
        for i in (c.start..c.end).step_by(4096) {
            x = x.wrapping_add(m[i] as u64);
        }
        std::hint::black_box(x);
    }
    let mut lines = 0u64;
    let mut acc = 0u64;
    let run = |name: &str, lines: u64, f: &mut dyn FnMut(&[&[u8]; 7])| {
        let t = Instant::now();
        for c in ch {
            scan_lines_fast::<7>(&ev_m.maps[c.file as usize], c.start, c.end, |x| f(x));
        }
        eprintln!("  {name:10} {:.1} ns/line", t.elapsed().as_nanos() as f64 / lines.max(1) as f64);
    };
    for c in ch {
        scan_lines::<7>(&ev_m.maps[c.file as usize], c.start, c.end, |_| lines += 1);
    }
    run("scan", lines, &mut |f| acc = acc.wrapping_add(f[6].len() as u64));
    run("pack eid", lines, &mut |f| acc = acc.wrapping_add(pack(f[0])));
    run("pack uid", lines, &mut |f| acc = acc.wrapping_add(pack(f[1])));
    run("ts", lines, &mut |f| acc = acc.wrapping_add(parse_ts(f[2]).unwrap_or(0) as u64));
    run("ingest", lines, &mut |f| acc = acc.wrapping_add(parse_ts(f[3]).unwrap_or(0) as u64));
    let (fl, ce) = (ev_m.maps[0].as_ptr(), unsafe { ev_m.maps[0].as_ptr().add(ev_m.maps[0].len()) });
    run("ts fast", lines, &mut |f| acc = acc.wrapping_add(parse_ts_fast(f[2], fl, ce).unwrap_or(0) as u64));
    run("ts bl", lines, &mut |f| acc = acc.wrapping_add(parse_ts_bl(f[2], fl, ce).unwrap_or(0) as u64));
    run("type", lines, &mut |f| acc = acc.wrapping_add(match f[4] { b"view" => 1, b"cart" => 2, b"purchase" => 3, _ => 0 }));
    run("price", lines, &mut |f| acc = acc.wrapping_add(parse_price(f[6]).unwrap_or(0) as u64));
    let floor = ev_m.maps[0].as_ptr();
    run("raw7 prod", lines, &mut |f| acc = acc.wrapping_add(raw7(f[5], floor).unwrap_or(0)));
    run("prod get", lines, &mut |f| acc = acc.wrapping_add(ctx.products.get(f[5], floor).unwrap_or(0) as u64));
    run("person", lines, &mut |f| { let u = pack(f[1]); acc = acc.wrapping_add(ctx.person_h(u, hk(u))) });
    let (mut inl, mut remap, mut tot) = (0u64, 0u64, 0u64);
    for c in ch {
        scan_lines::<7>(&ev_m.maps[c.file as usize], c.start, c.end, |f| {
            let u = pack(f[1]);
            let h = hk(u);
            tot += 1;
            if !ctx.links.is_empty() {
                if let Some(v) = ctx.links[(h >> ctx.link_shift) as usize].get_h(u, h) {
                    inl += 1;
                    if v != u { remap += 1; }
                }
            }
        });
    }
    let nslots: usize = ctx.links.iter().map(|m| m.slots.len()).sum();
    eprintln!("  stats: events {tot}, uid in links {inl}, remapped {remap}, link slots {nslots}");
    let mut bs = Blocks::new(arena);
    let mut eb: Vec<BList<ERec>> = (0..(1usize << p1_bits())).map(|_| BList::new()).collect();
    let t = Instant::now();
    for c in ch {
        parse_events(ctx, &ev_m.maps[c.file as usize], c.start, c.end, &mut eb, &mut bs);
    }
    eprintln!("  full       {:.1} ns/line ({} lines, {})", t.elapsed().as_nanos() as f64 / lines as f64, lines, acc);
    for pass in 0..3 {
        let mut lb: Vec<BList<LRec>> = (0..(1usize << p1_bits())).map(|_| BList::new()).collect();
        let mut sb: Vec<BList<SRec>> = (0..(1usize << p2_bits())).map(|_| BList::new()).collect();
        let t = Instant::now();
        for c in ch {
            parse_events1(ctx, &ev_m.maps[c.file as usize], c.start, c.end, &mut lb, &mut sb, &mut bs);
        }
        eprintln!("  full1 #{pass}   {:.1} ns/line", t.elapsed().as_nanos() as f64 / lines as f64);
    }
    let mut cch = Vec::new();
    chunks_of(1, cl_m, 4 << 20, &mut cch);
    let cch = &cch[..cch.len().min(40)];
    let mut cl_lines = 0u64;
    for c in cch {
        scan_lines::<4>(&cl_m.maps[c.file as usize], c.start, c.end, |_| cl_lines += 1);
    }
    let t = Instant::now();
    for c in cch {
        scan_lines_fast::<4>(&cl_m.maps[c.file as usize], c.start, c.end, |f| acc = acc.wrapping_add(f[3].len() as u64));
    }
    eprintln!("  cl scan    {:.1} ns/line", t.elapsed().as_nanos() as f64 / cl_lines as f64);
    let mut cb: Vec<BList<CRec>> = (0..(1usize << p2_bits())).map(|_| BList::new()).collect();
    let t = Instant::now();
    for c in cch {
        parse_clicks(ctx, &cl_m.maps[c.file as usize], c.start, c.end, &mut cb, &mut bs);
    }
    eprintln!("  cl full    {:.1} ns/line ({} lines)", t.elapsed().as_nanos() as f64 / cl_lines as f64, cl_lines);
}

#[inline(never)]
fn parse_events(ctx: &Ctx, d: &[u8], start: usize, end: usize, eb: &mut [BList<ERec>], bs: &mut Blocks) {
    let ws = ctx.ws;
    let we = ctx.we;
    let ws_s = ws.div_euclid(1000);
    let floor = d.as_ptr();
    let ceil = unsafe { floor.add(d.len()) };
    let unknown = ctx.cat_unknown as u32;
    let tsbl = ts_bl();
    scan_lines_fast::<7>(d, start, end, |f| {
        if f[0].is_empty() {
            return;
        }
        let eid = pack_fast(f[0], ceil);
        let ingest = match parse_ingest_s(f[3], ceil) {
            Some(v) => (v - ws_s).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            None => i32::MAX,
        };
        let mut r = ERec { eid, ingest, pc: INVALID, ..Default::default() };
        'v: {
            if f[1].is_empty() || f[5].is_empty() {
                break 'v;
            }
            let Some(ts) = (if tsbl { parse_ts_bl(f[2], floor, ceil) } else { parse_ts_fast(f[2], floor, ceil) }) else { break 'v };
            if ts < ws || ts >= we || (ingest as i64 + ws_s) * 1000 - ts > DAY_MS {
                break 'v;
            }
            let price = match f[4] {
                b"view" | b"cart" => 0,
                b"purchase" => match parse_price(f[6]) {
                    Some(p) => p,
                    None => break 'v,
                },
                _ => break 'v,
            };
            r.ts = (ts - ws) as u32;
            r.person = pack_fast(f[1], ceil);
            // the category is only ever read for purchases
            r.pc = if price != 0 { (price << 8) | ctx.products.get(f[5], floor).unwrap_or(unknown) } else { 0 };
        }
        unsafe { eb.get_unchecked_mut((hk(eid) >> (64 - p1_bits())) as usize) }.push_wc(r, bs);
    });
    sfence();
}

/// Ends every write-combining sequence of a phase: list set t is flushed with
/// thread t's blocks.
fn flush_all<T: Copy + Send>(lists: &PerThread<Vec<BList<T>>>, blocks: &PerThread<Blocks>, threads: usize) {
    par_for(threads, threads, |_w, t| {
        let bs = unsafe { blocks.get(t) };
        for l in unsafe { lists.get(t) }.iter_mut() {
            l.flush_wc(bs);
        }
    });
}

#[inline(never)]
fn parse_clicks(ctx: &Ctx, d: &[u8], start: usize, end: usize, cb: &mut [BList<CRec>], bs: &mut Blocks) {
    let cws = ctx.ws - WEEK_MS;
    let we = ctx.we;
    let tsbl = ts_bl();
    let floor = d.as_ptr();
    let ceil = unsafe { floor.add(d.len()) };
    scan_lines_fast::<4>(d, start, end, |f| {
        if f[0].is_empty() || f[1].is_empty() {
            return;
        }
        let Some(ts) = (if tsbl { parse_ts_bl(f[3], floor, ceil) } else { parse_ts_fast(f[3], floor, ceil) }) else { return };
        if ts < cws || ts >= we {
            return;
        }
        let Some(camp) = ctx.campaigns.get(f[2], floor) else { return };
        let u = pack_fast(f[1], ceil);
        let r = CRec { person: u, cid: pack_fast(f[0], ceil), ts, camp };
        unsafe { cb.get_unchecked_mut((hk(u) >> (64 - p2_bits())) as usize) }.push_wc(r, bs);
    });
    sfence();
}

/// Events with the links already built: each valid copy goes straight to its
/// person partition as an SRec, and every copy leaves an LRec in its eid
/// partition. The person lookup is prefetched PD lines ahead through a ring.
#[inline(never)]
fn parse_events1(
    ctx: &Ctx,
    d: &[u8],
    start: usize,
    end: usize,
    lb: &mut [BList<LRec>],
    sb: &mut [BList<SRec>],
    bs: &mut Blocks,
) {
    const PD: usize = 16;
    let ws = ctx.ws;
    let we = ctx.we;
    let ws_s = ws.div_euclid(1000);
    let floor = d.as_ptr();
    let ceil = unsafe { floor.add(d.len()) };
    let unknown = ctx.cat_unknown as u32;
    let tsbl = ts_bl();
    let s1 = 64 - p1_bits();
    let s2 = 64 - p2_bits();
    let emit = |p: Pend, lb: &mut [BList<LRec>], sb: &mut [BList<SRec>], bs: &mut Blocks| {
        let v = ctx.person_h(p.uid, p.h);
        let hv = if v == p.uid { p.h } else { hk(v) };
        let at = unsafe { sb.get_unchecked_mut((hv >> s2) as usize) }.push_at(SRec { person: v, ts: p.ts, pc: p.pc }, bs);
        unsafe { lb.get_unchecked_mut((hk(p.eid) >> s1) as usize) }.push(LRec { eid: p.eid, ingest: p.ingest, at: at as u64 }, bs);
    };
    let mut ring = [Pend::default(); PD];
    let mut nr = 0usize;
    scan_lines_fast::<7>(d, start, end, |f| {
        if f[0].is_empty() {
            return;
        }
        let eid = pack_fast(f[0], ceil);
        let ingest = match parse_ingest_s(f[3], ceil) {
            Some(v) => (v - ws_s).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            None => i32::MAX,
        };
        'v: {
            if f[1].is_empty() || f[5].is_empty() {
                break 'v;
            }
            let Some(ts) = (if tsbl { parse_ts_bl(f[2], floor, ceil) } else { parse_ts_fast(f[2], floor, ceil) }) else { break 'v };
            if ts < ws || ts >= we || (ingest as i64 + ws_s) * 1000 - ts > DAY_MS {
                break 'v;
            }
            let price = match f[4] {
                b"view" | b"cart" => 0,
                b"purchase" => match parse_price(f[6]) {
                    Some(p) => p,
                    None => break 'v,
                },
                _ => break 'v,
            };
            let uid = pack_fast(f[1], ceil);
            let h = hk(uid);
            ctx.prefetch_person(h);
            // the category is only ever read for purchases
            let pc = if price != 0 { (price << 8) | ctx.products.get(f[5], floor).unwrap_or(unknown) } else { 0 };
            let slot = &mut ring[nr % PD];
            if nr >= PD {
                emit(*slot, lb, sb, bs);
            }
            *slot = Pend { eid, uid, h, ingest, ts: (ts - ws) as u32, pc };
            nr += 1;
            return;
        }
        unsafe { lb.get_unchecked_mut((hk(eid) >> s1) as usize) }.push(LRec { eid, ingest, at: 0 }, bs);
    });
    for k in nr.saturating_sub(PD)..nr {
        emit(ring[k % PD], lb, sb, bs);
    }
}

/// Clicks with the links already built: routed straight to person partitions.
#[inline(never)]
fn parse_clicks1(ctx: &Ctx, d: &[u8], start: usize, end: usize, cb: &mut [BList<CRec>], bs: &mut Blocks) {
    const PD: usize = 16;
    let cws = ctx.ws - WEEK_MS;
    let we = ctx.we;
    let tsbl = ts_bl();
    let floor = d.as_ptr();
    let ceil = unsafe { floor.add(d.len()) };
    let s2 = 64 - p2_bits();
    let emit = |r: CRec, h: u64, cb: &mut [BList<CRec>], bs: &mut Blocks| {
        let v = ctx.person_h(r.person, h);
        let hv = if v == r.person { h } else { hk(v) };
        unsafe { cb.get_unchecked_mut((hv >> s2) as usize) }.push(CRec { person: v, ..r }, bs);
    };
    let mut ring = [(CRec::default(), 0u64); PD];
    let mut nr = 0usize;
    scan_lines_fast::<4>(d, start, end, |f| {
        if f[0].is_empty() || f[1].is_empty() {
            return;
        }
        let Some(ts) = (if tsbl { parse_ts_bl(f[3], floor, ceil) } else { parse_ts_fast(f[3], floor, ceil) }) else { return };
        if ts < cws || ts >= we {
            return;
        }
        let Some(camp) = ctx.campaigns.get(f[2], floor) else { return };
        let u = pack_fast(f[1], ceil);
        let h = hk(u);
        ctx.prefetch_person(h);
        let slot = &mut ring[nr % PD];
        if nr >= PD {
            emit(slot.0, slot.1, cb, bs);
        }
        *slot = (CRec { person: u, cid: pack_fast(f[0], ceil), ts, camp }, h);
        nr += 1;
    });
    for k in nr.saturating_sub(PD)..nr {
        let (r, h) = ring[k % PD];
        emit(r, h, cb, bs);
    }
}

/// Per eid partition: the copy with the smallest ingest wins; every other
/// copy that produced an SRec gets that SRec marked dropped.
#[inline(never)]
fn dedup_mark(lists: &mut [BList<LRec>], mp: &mut KMap<u64>, dead: &mut Vec<u64>, bs: &mut Blocks) {
    let n: usize = lists.iter().map(|l| l.len).sum();
    if n == 0 {
        return;
    }
    mp.reset(n);
    dead.clear();
    dead.resize(n / 64 + 1, 0);
    let mut i = 0u32;
    let mut ndead = 0usize;
    for l in lists.iter() {
        l.for_each_slice(|sl| {
            for r in sl {
                let v = ((r.ingest as u32 as u64) << 32) | i as u64;
                let (s, ins) = mp.entry(r.eid, v);
                if !ins {
                    let cur = *mp.val_mut(s);
                    if r.ingest < (cur >> 32) as u32 as i32 {
                        let ci = cur as u32;
                        dead[(ci / 64) as usize] |= 1 << (ci % 64);
                        *mp.val_mut(s) = v;
                    } else {
                        dead[(i / 64) as usize] |= 1 << (i % 64);
                    }
                    ndead += 1;
                }
                i += 1;
            }
        });
    }
    if ndead != 0 {
        let mut i = 0usize;
        for l in lists.iter() {
            l.for_each_slice(|sl| {
                let w0 = i / 64;
                let w1 = (i + sl.len()).div_ceil(64);
                for w in w0..w1 {
                    let mut m = dead[w];
                    while m != 0 {
                        let j = w * 64 + m.trailing_zeros() as usize;
                        m &= m - 1;
                        if j >= i && j < i + sl.len() {
                            let at = sl[j - i].at;
                            if at != 0 {
                                unsafe { (*(at as *mut SRec)).pc = INVALID };
                            }
                        }
                    }
                }
                i += sl.len();
            });
        }
    }
    for l in lists.iter_mut() {
        l.release(&mut bs.free);
    }
}

/// Rewrites user ids to person ids inside one user-hash partition (the links
/// shard for it is small and cache-resident). Records whose person belongs to
/// another partition are moved out to `mv`.
#[inline(never)]
fn resolve_partition<T: Copy>(
    p: usize,
    map: Option<&KMap<Key>>,
    lists: &mut [BList<T>],
    person: impl Fn(&mut T) -> &mut Key,
    mv: &mut [BList<T>],
    bs: &mut Blocks,
) {
    let Some(map) = map else { return };
    for l in lists.iter_mut() {
        let mut free = std::mem::take(&mut bs.free);
        l.compact(&mut free, |r| {
            let k = person(r);
            let u = *k;
            if let Some(v) = map.get(u) {
                *k = v;
                let q = (hk(v) >> (64 - p2_bits())) as usize;
                if q != p {
                    unsafe { mv.get_unchecked_mut(q) }.push(*r, bs);
                    return false;
                }
            }
            true
        });
        bs.free.append(&mut free);
    }
}

/// Keeps, per event_id, the copy with the smallest ingest_ts; emits it if valid.
#[inline(never)]
fn dedup_partition(
    ctx: &Ctx,
    lists: &mut [BList<ERec>],
    mp: &mut KMap<u64>,
    dead: &mut Vec<u64>,
    filt: &mut Vec<u64>,
    susp: &mut Vec<(Key, i32, u32)>,
    sb: &mut [BList<SRec>],
    bs: &mut Blocks,
) {
    let n: usize = lists.iter().map(|l| l.len).sum();
    if n == 0 {
        return;
    }
    dead.clear();
    dead.resize(n / 64 + 1, 0);
    // pass 1a: a 16-bits-per-copy bitmap finds the ids that may repeat; every
    // other copy is the only one of its event
    let nb = (n * 16).next_power_of_two().max(64);
    let words = nb / 64;
    let m = nb - 1;
    filt.clear();
    filt.resize(2 * words, 0);
    let (seen, dup) = filt.split_at_mut(words);
    for l in lists.iter() {
        l.for_each_slice(|sl| {
            for r in sl {
                let b = hk(r.eid) as usize & m;
                let bit = 1u64 << (b & 63);
                let w = unsafe { seen.get_unchecked_mut(b >> 6) };
                let d = unsafe { dup.get_unchecked_mut(b >> 6) };
                *d |= *w & bit;
                *w |= bit;
            }
        });
    }
    susp.clear();
    let mut i = 0u32;
    for l in lists.iter() {
        l.for_each_slice(|sl| {
            for r in sl {
                let b = hk(r.eid) as usize & m;
                if unsafe { *dup.get_unchecked(b >> 6) } >> (b & 63) & 1 != 0 {
                    susp.push((r.eid, r.ingest, i));
                }
                i += 1;
            }
        });
    }
    // pass 1b, suspects only: value = (ingest as u32) << 32 | index
    mp.reset(susp.len());
    for &(eid, ingest, i) in susp.iter() {
        let v = ((ingest as u32 as u64) << 32) | i as u64;
        let (s, ins) = mp.entry(eid, v);
        if !ins {
            let cur = *mp.val_mut(s);
            if ingest < (cur >> 32) as u32 as i32 {
                let ci = cur as u32;
                dead[(ci / 64) as usize] |= 1 << (ci % 64);
                *mp.val_mut(s) = v;
            } else {
                dead[(i / 64) as usize] |= 1 << (i % 64);
            }
        }
    }
    // pass 2: survivors, sequentially, with user -> person resolved through a
    // small ring so the links lookup is prefetched PD records ahead
    const PD: usize = 8;
    let mut ring = [(SRec::default(), 0u64); PD];
    let mut nr = 0usize;
    let emit = |s: SRec, h: u64, sb: &mut [BList<SRec>], bs: &mut Blocks| {
        let v = ctx.person_h(s.person, h);
        let hv = if v == s.person { h } else { hk(v) };
        unsafe { sb.get_unchecked_mut((hv >> (64 - p2_bits())) as usize) }.push_wc(SRec { person: v, ..s }, bs);
    };
    let mut i = 0u32;
    for l in lists.iter() {
        l.for_each_slice(|sl| {
            for r in sl {
                if r.pc != INVALID && dead[(i / 64) as usize] & (1 << (i % 64)) == 0 {
                    let h = hk(r.person);
                    ctx.prefetch_person(h);
                    let slot = &mut ring[nr % PD];
                    if nr >= PD {
                        emit(slot.0, slot.1, sb, bs);
                    }
                    *slot = (SRec { person: r.person, ts: r.ts, pc: r.pc }, h);
                    nr += 1;
                }
                i += 1;
            }
        });
    }
    for k in nr.saturating_sub(PD)..nr {
        let (s, h) = ring[k % PD];
        emit(s, h, sb, bs);
    }
    sfence();
    for l in lists.iter_mut() {
        l.release(&mut bs.free);
    }
}

/// One 30-minute bin of a person's events. Two events in one bin are less than
/// 30 minutes apart, so a session can only end between occupied bins.
#[derive(Clone, Copy)]
struct Bin {
    min: u32,
    max: u32,
    cnt: u32,
    ph: u32, // head of the bin's purchase list in `Bins::purch`, or u32::MAX
}
const FREE_BIN: Bin = Bin { min: u32::MAX, max: 0, cnt: 0, ph: u32::MAX };

#[derive(Default)]
struct Bins {
    bins: Vec<Bin>,
    bits: Vec<u64>,
    purch: Vec<(u32, u32)>, // (pc, next)
}

/// Per-thread accumulators of the session walk.
#[derive(Default)]
struct Sink {
    cat_rev: Vec<u64>,
    touched: Vec<u16>,
    acnt: Vec<u32>,
    apos: Vec<u32>,
    active: Vec<u32>,
}

#[derive(Default)]
struct Scratch {
    gmap: Option<KMap<u32>>,
    gkeys: Vec<Key>,
    off: Vec<u32>,
    fill: Vec<u32>,
    gid: Vec<u32>,
    ev: Vec<u64>, // ts << 32 | pc, grouped by person
    coff: Vec<u32>,
    cgid: Vec<u32>,
    cl: Vec<(u64, Key)>, // (ts since ws - 7 days) << 32 | campaign, click_id
    bins: Bins,
    sink: Sink,
}

/// Persons with more events than this walk bins instead of sorting.
const BIN_MIN: usize = 16;

#[inline(always)]
fn isort_by<T: Copy>(v: &mut [T], key: impl Fn(&T) -> u64) {
    for i in 1..v.len() {
        let x = v[i];
        let kx = key(&x);
        let mut j = i;
        while j > 0 && key(&v[j - 1]) > kx {
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = x;
    }
}

/// One person's sessions in start order: `begin`, events and purchases, `end`.
struct Sess<'a> {
    a: &'a mut out::Acc,
    k: &'a mut Sink,
    cls: &'a [(u64, Key)],
    person: Key,
    day0: i64,
    r0: u64, // ms from the first day's midnight to window start
    organic: u32,
    cur_day: u32,
    d_sess: u32,
    d_events: u32,
    d_rev: u64,
    cj: usize,
    wlo: usize,
    start: u32,
    cnt: u32,
    rev: u64,
    purchase: bool,
}

impl Sess<'_> {
    #[inline(always)]
    fn begin(&mut self, start: u32) {
        let di = ((start as u64 + self.r0) / DAY_MS as u64) as u32;
        if di != self.cur_day {
            if self.cur_day != u32::MAX {
                self.flush_day();
            }
            self.cur_day = di;
            self.d_sess = 0;
            self.d_events = 0;
            self.d_rev = 0;
        }
        self.start = start;
        self.cnt = 0;
        self.rev = 0;
        self.purchase = false;
    }

    #[inline(always)]
    fn add(&mut self, pc: u32) {
        let price = pc >> 8;
        if price != 0 {
            self.purchase = true;
            self.rev += price as u64;
            let c = (pc & 0xFF) as usize;
            let k = &mut *self.k;
            let v = unsafe { k.cat_rev.get_unchecked_mut(c) };
            if *v == 0 {
                k.touched.push(c as u16);
            }
            *v += price as u64;
        }
    }

    #[inline(always)]
    fn end(&mut self) {
        self.d_sess += 1;
        self.d_events += self.cnt;
        self.d_rev += self.rev;
        // clicks in [start - 7d, start] form the window: credit picks the
        // latest (smallest click_id on a tie), every distinct campaign assists
        let cls = self.cls;
        let k = &mut *self.k;
        let hi = self.start as u64 + WEEK_MS as u64;
        let lo = self.start as u64;
        while self.cj < cls.len() && cls[self.cj].0 >> 32 <= hi {
            let c = cls[self.cj].0 as u32 as usize;
            if k.acnt[c] == 0 {
                k.apos[c] = k.active.len() as u32;
                k.active.push(c as u32);
            }
            k.acnt[c] += 1;
            self.cj += 1;
        }
        while self.wlo < self.cj && cls[self.wlo].0 >> 32 < lo {
            let c = cls[self.wlo].0 as u32 as usize;
            k.acnt[c] -= 1;
            if k.acnt[c] == 0 {
                let i = k.apos[c] as usize;
                let last = k.active.pop().unwrap();
                if last as usize != c {
                    k.active[i] = last;
                    k.apos[last as usize] = i as u32;
                }
            }
            self.wlo += 1;
        }
        let mut camp = self.organic;
        if self.cj > self.wlo {
            let lt = cls[self.cj - 1].0 >> 32;
            let mut q = self.cj - 1;
            let mut best = q;
            while q > self.wlo && cls[q - 1].0 >> 32 == lt {
                q -= 1;
                if key_cmp(cls[q].1, cls[best].1) == std::cmp::Ordering::Less {
                    best = q;
                }
            }
            camp = cls[best].0 as u32;
        }
        let di = self.cur_day as usize;
        let cell = self.a.camp_cell(camp as usize, di);
        cell[0] += 1;
        cell[1] += self.purchase as u64;
        cell[2] += self.rev;
        for &c in k.active.iter() {
            self.a.camp_cell(c as usize, di)[3] += 1;
        }
    }

    #[inline(always)]
    fn flush_day(&mut self) {
        let k = &mut *self.k;
        let mut top = u16::MAX;
        let mut best = 0u64;
        for &c in k.touched.iter() {
            let v = k.cat_rev[c as usize];
            if v > best || (v == best && c < top) {
                best = v;
                top = c;
            }
            k.cat_rev[c as usize] = 0;
        }
        k.touched.clear();
        self.a.users.push(out::URow {
            person: self.person,
            day: (self.day0 + self.cur_day as i64) as i32,
            sessions: self.d_sess,
            events: self.d_events,
            revenue: self.d_rev,
            top,
        });
    }

    #[inline(always)]
    fn finish(&mut self) {
        self.end();
        self.flush_day();
        let k = &mut *self.k;
        for &c in k.active.iter() {
            k.acnt[c as usize] = 0;
        }
        k.active.clear();
    }
}

#[inline(never)]
fn process_partition(
    ctx: &Ctx,
    slists: &[BList<SRec>],
    clists: &[BList<CRec>],
    a: &mut out::Acc,
    sc: &mut Scratch,
    ncat: usize,
) {
    let n: usize = slists.iter().map(|l| l.len).sum();
    if n == 0 {
        return;
    }
    let gmap = sc.gmap.get_or_insert_with(|| KMap::<u32>::with_capacity(16));
    gmap.reset(n / 32 + 16);
    // group events by person (counting sort)
    let gkeys = &mut sc.gkeys;
    let off = &mut sc.off;
    let gid = &mut sc.gid;
    gkeys.clear();
    off.clear();
    gid.clear();
    gid.reserve(n);
    for l in slists {
        l.for_each_slice(|sl| {
            for r in sl {
                let g = if r.pc == INVALID {
                    u32::MAX
                } else {
                    let (s, ins) = gmap.entry(r.person, gkeys.len() as u32);
                    if ins {
                        gkeys.push(r.person);
                        off.push(0);
                    }
                    let g = *gmap.val_mut(s);
                    unsafe { *off.get_unchecked_mut(g as usize) += 1 };
                    g
                };
                unsafe {
                    gid.as_mut_ptr().add(gid.len()).write(g);
                    gid.set_len(gid.len() + 1);
                }
            }
        });
    }
    let ng = gkeys.len();
    let mut tot = 0u32;
    for c in off.iter_mut() {
        let x = *c;
        *c = tot;
        tot += x;
    }
    off.push(tot);
    let fill = &mut sc.fill;
    fill.clear();
    fill.extend_from_slice(&off[..ng]);
    let ev = &mut sc.ev;
    let ne = tot as usize;
    ev.clear();
    ev.reserve(ne);
    unsafe { ev.set_len(ne) };
    {
        let mut i = 0;
        for l in slists {
            l.for_each_slice(|sl| {
                for r in sl {
                    let g = unsafe { *gid.get_unchecked(i) } as usize;
                    if g != u32::MAX as usize {
                        let f = unsafe { fill.get_unchecked_mut(g) };
                        unsafe { *ev.get_unchecked_mut(*f as usize) = ((r.ts as u64) << 32) | r.pc as u64 };
                        *f += 1;
                    }
                    i += 1;
                }
            });
        }
    }
    // group clicks by person (only persons with events)
    let coff = &mut sc.coff;
    coff.clear();
    coff.resize(ng + 1, 0);
    let cgid = &mut sc.cgid;
    cgid.clear();
    for l in clists {
        l.for_each_slice(|sl| {
            for r in sl {
                let g = gmap.get(r.person).unwrap_or(u32::MAX);
                if g != u32::MAX {
                    coff[g as usize] += 1;
                }
                cgid.push(g);
            }
        });
    }
    let mut tot = 0u32;
    for c in coff.iter_mut() {
        let x = *c;
        *c = tot;
        tot += x;
    }
    fill.clear();
    fill.extend_from_slice(&coff[..ng]);
    let cl = &mut sc.cl;
    let ncl = tot as usize;
    cl.clear();
    cl.reserve(ncl);
    unsafe { cl.set_len(ncl) };
    let cbase = ctx.ws - WEEK_MS;
    {
        let mut j = 0;
        for l in clists {
            l.for_each_slice(|sl| {
                for r in sl {
                    let g = cgid[j];
                    if g != u32::MAX {
                        let f = &mut fill[g as usize];
                        let k = (((r.ts - cbase) as u64) << 32) | r.camp as u64;
                        unsafe { *cl.get_unchecked_mut(*f as usize) = (k, r.cid) };
                        *f += 1;
                    }
                    j += 1;
                }
            });
        }
    }

    let sink = &mut sc.sink;
    if sink.cat_rev.len() < ncat {
        sink.cat_rev = vec![0; ncat];
    }
    if sink.acnt.len() < ctx.camp_names.len() + 1 {
        sink.acnt = vec![0; ctx.camp_names.len() + 1];
        sink.apos = vec![0; ctx.camp_names.len() + 1];
    }
    let bb = &mut sc.bins;
    if bb.bins.is_empty() {
        let nbins = ((ctx.we - ctx.ws) as u64).div_ceil(GAP_MS as u64) as usize + 1;
        bb.bins = vec![FREE_BIN; nbins];
        bb.bits = vec![0; nbins.div_ceil(64)];
    }
    let organic = ctx.camp_names.len() as u32;
    let r0 = (ctx.ws - ctx.day0 * DAY_MS) as u64;

    for g in 0..ng {
        let evs = &mut ev[off[g] as usize..off[g + 1] as usize];
        let cls = &mut cl[coff[g] as usize..coff[g + 1] as usize];
        if cls.len() > 16 {
            cls.sort_unstable_by_key(|c| c.0);
        } else {
            isort_by(cls, |c| c.0);
        }
        let mut s = Sess {
            a: &mut *a,
            k: &mut *sink,
            cls,
            person: gkeys[g],
            day0: ctx.day0,
            r0,
            organic,
            cur_day: u32::MAX,
            d_sess: 0,
            d_events: 0,
            d_rev: 0,
            cj: 0,
            wlo: 0,
            start: 0,
            cnt: 0,
            rev: 0,
            purchase: false,
        };
        if evs.len() <= BIN_MIN {
            isort_by(evs, |&e| e);
            let mut last = (evs[0] >> 32) as u32;
            s.begin(last);
            for &e in evs.iter() {
                let ts = (e >> 32) as u32;
                if ts - last > GAP_MS as u32 {
                    s.end();
                    s.begin(ts);
                }
                last = ts;
                s.cnt += 1;
                s.add(e as u32);
            }
        } else {
            let (mut wmin, mut wmax) = (usize::MAX, 0usize);
            for &e in evs.iter() {
                let ts = (e >> 32) as u32;
                let b = (ts / GAP_MS as u32) as usize;
                let bin = unsafe { bb.bins.get_unchecked_mut(b) };
                bin.cnt += 1;
                bin.min = bin.min.min(ts);
                bin.max = bin.max.max(ts);
                if (e as u32) >> 8 != 0 {
                    bb.purch.push((e as u32, bin.ph));
                    bin.ph = (bb.purch.len() - 1) as u32;
                }
                unsafe { *bb.bits.get_unchecked_mut(b >> 6) |= 1u64 << (b & 63) };
                wmin = wmin.min(b >> 6);
                wmax = wmax.max(b >> 6);
            }
            let mut open = false;
            let mut prev_max = 0u32;
            for wi in wmin..=wmax {
                let mut m = std::mem::take(&mut bb.bits[wi]);
                while m != 0 {
                    let b = wi * 64 + m.trailing_zeros() as usize;
                    m &= m - 1;
                    let bin = std::mem::replace(unsafe { bb.bins.get_unchecked_mut(b) }, FREE_BIN);
                    if !open {
                        open = true;
                        s.begin(bin.min);
                    } else if bin.min - prev_max > GAP_MS as u32 {
                        s.end();
                        s.begin(bin.min);
                    }
                    prev_max = bin.max;
                    s.cnt += bin.cnt;
                    let mut p = bin.ph;
                    while p != u32::MAX {
                        let (pc, nx) = bb.purch[p as usize];
                        s.add(pc);
                        p = nx;
                    }
                }
            }
            bb.purch.clear();
        }
        s.finish();
    }
}
