//! Shopping Sessions pipeline: native, parallel, single binary.
//!
//! Phases
//!   0. manifest, products, campaigns (tiny)
//!   1. links  -> sharded ID interning -> union-find -> global ID -> person table
//!   2. events + clicks parsed in parallel chunks of mmapped files; the person
//!      is resolved at parse time (prefetched table lookup), so each valid
//!      event is written once, into its person partition; every event copy
//!      also leaves a 16-byte (event_id, ingest, record index) key in its
//!      event_id partition
//!   3. dedup per event_id partition: the losing copies are marked dead in place
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
/// Dedup key of one parsed event copy. `loc` = 1 + index of its SRec in the
/// arena (in 16-byte units), or 0 if the copy is invalid and has no SRec.
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct DRec {
    eid: Key,
    ingest: i32, // seconds since window start, saturated
    loc: u32,
}
/// `SRec.pc` of an event copy that lost dedup.
const DEAD: u32 = u32::MAX;

/// A valid event copy, in its person partition. Purchase <=> price > 0.
/// `pc` = price << 8 | category (0 for non-purchases), or DEAD.
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct SRec {
    person: Key,
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
    pub links: LinkTab,
}

// ---------------------------------------------------------------------------
// Phase 1: links
// ---------------------------------------------------------------------------
fn build_links(data: &Path, threads: usize) -> LinkTab {
    let t0 = Instant::now();
    let paths = list_csv(&data.join("links"));
    let m = Mapped::open(&paths);
    let mut chunks = Vec::new();
    chunks_of(0, &m, 1 << 20, &mut chunks);
    let shard_bits: u32 = p2_bits();
    let shards: usize = 1 << shard_bits;
    let shift = 64 - shard_bits;
    // sized up front (a link line is about 20 bytes) so the lists do not regrow
    let lines = m.maps.iter().map(|x| x.len()).sum::<usize>() / 18;
    let pairs = PerThread::new(threads, || Vec::<(Key, Key)>::with_capacity(lines / threads * 3 / 2 + 1024));
    let skeys = PerThread::new(threads, || (0..shards).map(|_| Vec::<Key>::with_capacity(2 * lines / shards / threads * 3 / 2 + 64)).collect::<Vec<_>>());
    par_for(chunks.len(), threads, |t, i| {
        let c = chunks[i];
        let d: &[u8] = &m.maps[c.file as usize];
        let p = unsafe { pairs.get(t) };
        let sk = unsafe { skeys.get(t) };
        scan_lines::<2>(d, c.start, c.end, |f| {
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
    let t_parse = t0.elapsed();
    let pairs = pairs.into_inner();
    let skeys = skeys.into_inner();
    let npairs: usize = pairs.iter().map(|p| p.len()).sum();
    if npairs == 0 {
        return LinkTab::new(0);
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
    let t_intern = t0.elapsed();
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
    let t_xlate = t0.elapsed();
    let t_uf = Instant::now();
    let mut all_keys: Vec<Key> = Vec::with_capacity(nids);
    for s in 0..shards {
        all_keys.extend_from_slice(&shard_keys[s]);
    }
    // Union-find with path halving. The root of a set is always its
    // byte-wise smallest ID (the person's name), so no naming pass is needed.
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
    let mut nmoved = 0usize; // IDs that are not a root = unions made
    for v in &ipairs {
        for &(a, b) in v {
            let ra = find(&mut parent, a);
            let rb = find(&mut parent, b);
            if ra != rb {
                nmoved += 1;
                if key_cmp(all_keys[ra as usize], all_keys[rb as usize]) == std::cmp::Ordering::Less {
                    parent[rb as usize] = ra;
                } else {
                    parent[ra as usize] = rb;
                }
            }
        }
    }
    let uf_time = t_uf.elapsed();
    let t_union = t0.elapsed();
    let t_name = t0.elapsed();
    let tab = LinkTab::new(nmoved);
    // the table holds only IDs that are not their own person's name
    par_for(shards, threads, |_t, s| {
        let off = offs[s] as usize;
        for (j, &k) in shard_keys[s].iter().enumerate() {
            let mut r = off + j;
            loop {
                let up = parent[r] as usize;
                if up == r {
                    break;
                }
                r = up;
            }
            if r != off + j {
                tab.insert(k, all_keys[r]);
            }
        }
    });
    eprintln!(
        "links: {} pairs, {} ids, {} mapped, uf {:?}, total {:?} (parse {:?} intern {:?} xlate {:?} union {:?} name {:?})",
        npairs, nids, nmoved, uf_time, t0.elapsed(), t_parse, t_intern, t_xlate, t_union, t_name
    );
    tab
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

struct Catalogs {
    products: Catalog,
    cat_set: Vec<Vec<u8>>,
    cat_unknown: u16,
    campaigns: Catalog,
    camp_names: Vec<Vec<u8>>,
    camp_channels: Vec<Vec<u8>>,
}

/// products.csv and campaigns.csv; runs beside the link build.
fn load_catalogs(data: &Path) -> Catalogs {
    let t_cat = Instant::now();
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
    eprintln!("catalogs {:?}", t_cat.elapsed());
    Catalogs { products, cat_set, cat_unknown, campaigns, camp_names, camp_channels }
}

fn epoch() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn main() {
    let t_start = Instant::now();
    eprintln!("epoch_start {:.6}", epoch());
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
    let manifest = std::fs::read_to_string(data.join("manifest.json")).expect("manifest");
    let ws = manifest_ts(&manifest, "window_start");
    let we = manifest_ts(&manifest, "window_end");
    assert!(we - ws + WEEK_MS < (1i64 << 32), "window too long for u32 offsets");
    let day0 = ws.div_euclid(DAY_MS);
    let ndays = ((we - 1).div_euclid(DAY_MS) - day0 + 1).max(1) as usize;

    // map the inputs early so the arena can be sized
    let io5 = io_mode() == 5; // pread only: events and clicks are never mapped
    let ev_paths = list_csv(&data.join("events"));
    let cl_paths = list_csv(&data.join("clicks"));
    let ev_in = Inputs::open(if io5 { &ev_paths } else { &[] });
    let cl_in = Inputs::open(if io5 { &cl_paths } else { &[] });
    // diagnostics for IOMODE=5: IO5MAP=1 also maps the files (unused), IO5RA=1 starts the read-ahead thread on them
    let io5map = io5 && env("IO5MAP", "0") == "1";
    let ev_m = Mapped::open(if io5 && !io5map { &[] } else { &ev_paths });
    let cl_m = Mapped::open(if io5 && !io5map { &[] } else { &cl_paths });
    if !io5map || env("IO5RA", "0") == "1" {
        background_readahead(&[&ev_m, &cl_m]);
    }
    let ev_bytes: usize = if io5 { ev_in.bytes() } else { ev_m.maps.iter().map(|m| m.len()).sum::<usize>() };
    let cl_bytes: usize = if io5 { cl_in.bytes() } else { cl_m.maps.iter().map(|m| m.len()).sum::<usize>() };
    // Records are never larger than twice the input (a valid event line of
    // >= 35 bytes makes 32 bytes, an invalid one of >= 8 bytes makes 16, a
    // valid click line of >= 20 bytes makes 32), plus one partial block per
    // list. The reservation is address space only.
    let partial = ((1usize << p1_bits()) + 2 * (1usize << p2_bits())) * threads * BLK;
    let est = ev_bytes / 78 * 32 + cl_bytes / 40 * 32 + partial * 2 + (256 << 20);
    let arena = Arena::new(2 * (ev_bytes + cl_bytes) + partial + (64 << 20), est * 3 / 2);
    assert!(arena.cap() / 16 < u32::MAX as usize - 1, "input too large for 32-bit record indices");
    let abase = arena.base();
    eprintln!("mapped + arena at {:?}", t_start.elapsed());

    // ---- phase 1 (the catalogs load on a side thread meanwhile)
    let (links, cats) = std::thread::scope(|sc| {
        let h = sc.spawn(|| load_catalogs(&data));
        let links = build_links(&data, threads);
        (links, h.join().expect("catalogs"))
    });
    let Catalogs { products, cat_set, cat_unknown, campaigns, camp_names, camp_channels } = cats;
    eprintln!("phase1 done {:?} {}", t_start.elapsed(), cpu());

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
    };

    // ---- phase 2: parse events + clicks
    let t_parse = Instant::now();
    let mut chunks = Vec::new();
    // chunk size: 4 MiB for mapped input; a pread buffer should stay in L2
    let chunk_kb: usize = env("CHUNKKB", match io_mode() {
        4 => "256",
        5 => "256",
        _ => "4096",
    })
    .parse()
    .unwrap_or(4096);
    if io5 {
        ev_in.chunks(0, chunk_kb << 10, &mut chunks);
        cl_in.chunks(1, chunk_kb << 10, &mut chunks);
    } else {
        chunks_of(0, &ev_m, chunk_kb << 10, &mut chunks);
        chunks_of(1, &cl_m, chunk_kb << 10, &mut chunks);
    }
    let blocks = PerThread::new(threads, || Blocks::new(&arena));
    let ebl = PerThread::new(threads, || Scatter::<DRec>::new(1usize << p1_bits()));
    let sbl = PerThread::new(threads, || Scatter::<SRec>::new(1usize << p2_bits()));
    let cbl = PerThread::new(threads, || Scatter::<CRec>::new(1usize << p2_bits()));
    let iobufs = PerThread::new(threads, Vec::<u8>::new);
    let rdbufs = PerThread::new(threads, IoBuf::new);
    par_for(chunks.len(), threads, |t, i| {
        let c = chunks[i];
        let buf = unsafe { iobufs.get(t) };
        let rb = unsafe { rdbufs.get(t) };
        let bs = unsafe { blocks.get(t) };
        if c.kind == 0 {
            let (d, s, e) = if io5 { ev_in.read(&c, rb) } else { chunk_data(&ev_m, c.file as usize, c.start, c.end, buf) };
            parse_events(&ctx, d, s, e, unsafe { ebl.get(t) }, unsafe { sbl.get(t) }, bs, abase);
            if !io5 {
                chunk_done(&ev_m, c.file as usize, c.start, c.end);
            }
        } else {
            if diag_stage() != 0 {
                return;
            }
            let (d, s, e) = if io5 { cl_in.read(&c, rb) } else { chunk_data(&cl_m, c.file as usize, c.start, c.end, buf) };
            parse_clicks(&ctx, d, s, e, unsafe { cbl.get(t) }, bs);
            if !io5 {
                chunk_done(&cl_m, c.file as usize, c.start, c.end);
            }
        }
    });
    std::mem::forget(iobufs);
    std::mem::forget(rdbufs);
    let eparts = PerThread::from_vec(transpose(ebl.into_inner().into_iter().map(|s| s.finish()).collect(), 1usize << p1_bits()));
    let cparts = transpose(cbl.into_inner().into_iter().map(|s| s.finish()).collect(), 1usize << p2_bits());
    eprintln!("phase2 parse {:?} (parse only {:?}) {}", t_start.elapsed(), t_parse.elapsed(), cpu());
    if diag_stage() != 0 {
        std::process::exit(0);
    }

    let sparts = transpose(sbl.into_inner().into_iter().map(|s| s.finish()).collect(), 1usize << p2_bits());

    // ---- phase 3: dedup per eid partition; losers are marked dead in place
    let dscr = PerThread::new(threads, DedupScratch::default);
    let p1 = p1_bits();
    par_for(1usize << p1, threads, |t, p| {
        let lists = unsafe { eparts.get(p) };
        let sc = unsafe { dscr.get(t) };
        let bs = unsafe { blocks.get(t) };
        dedup_partition(lists, sc, bs, abase, p1);
    });
    eprintln!("phase3 dedup {:?} {}", t_start.elapsed(), cpu());
    eprintln!("phase3b resolve {:?} {}", t_start.elapsed(), cpu());

    // ---- phase 4: per person partition
    let ncamp = ctx.camp_names.len();
    let ncat = ctx.cat_names.len();
    // about 0.1 user-day rows per event; threads take uneven shares
    let nev: usize = sparts.iter().map(|ls| ls.iter().map(|l| l.len).sum::<usize>()).sum();
    let acc = PerThread::new(threads, || {
        let mut a = out::Acc::new(ncamp + 1, ndays);
        a.users.reserve(nev / 4 / threads + 1024);
        a
    });
    let pscr = PerThread::new(threads, Scratch::default);
    let ptimes = std::sync::Mutex::new(Vec::new());
    // largest partitions first, so a whale's partition is never the last one running alone
    let mut order: Vec<usize> = (0..1usize << p2_bits()).collect();
    order.sort_by_key(|&p| std::cmp::Reverse(sparts[p].iter().map(|l| l.len).sum::<usize>()));
    par_for(order.len(), threads, |t, i| {
        let p = order[i];
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
    eprintln!("phase4 sessions {:?} {} arena used {} MB of {} MB", t_start.elapsed(), cpu(), arena.used() >> 20, arena.cap() >> 20);

    // ---- phase 5: output
    out::write_all(&outdir, &ctx, accs);
    if env("AREL", "0") == "1" {
        arena.release_parallel(threads);
    }
    eprintln!("done {:?} {}", t_start.elapsed(), cpu());
    eprintln!("epoch_done {:.6}", epoch());
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        eprintln!(
            "rusage: user {}.{:06}s sys {}.{:06}s minflt {} majflt {} maxrss {} threads {}",
            ru.ru_utime.tv_sec, ru.ru_utime.tv_usec, ru.ru_stime.tv_sec, ru.ru_stime.tv_usec,
            ru.ru_minflt, ru.ru_majflt, ru.ru_maxrss, threads
        );
    }
    // everything is leaked to the kernel: no destructor walks
    std::process::exit(0);
}

/// A parsed valid event waiting for its link-table line to arrive in cache.
#[derive(Clone, Copy, Default)]
struct PendE {
    user: Key,
    h: u64,
    eid: Key,
    ts: u32,
    pc: u32,
    ingest: i32,
}
#[derive(Clone, Copy, Default)]
struct PendC {
    user: Key,
    h: u64,
    cid: Key,
    ts: i64,
    camp: u32,
}
/// Lines between the prefetch of a link-table bucket and its lookup.
const RING: usize = 16;

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn emit_event(tab: &LinkTab, p: &PendE, eb: &mut Scatter<DRec>, sb: &mut Scatter<SRec>, bs: &mut Blocks, sh1: u32, sh2: u32, abase: usize) {
    let person = tab.person(p.user, p.h);
    let hp = hk(person);
    let at = sb.push((hp >> sh2) as usize, SRec { person, ts: p.ts, pc: p.pc }, bs);
    let loc = ((at as usize - abase) / 16) as u32 + 1;
    eb.push((hk(p.eid) >> sh1) as usize, DRec { eid: p.eid, ingest: p.ingest, loc }, bs);
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn parse_events(ctx: &Ctx, d: &[u8], start: usize, end: usize, eb: &mut Scatter<DRec>, sb: &mut Scatter<SRec>, bs: &mut Blocks, abase: usize) {
    let ws = ctx.ws;
    let we = ctx.we;
    let ws_s = ws.div_euclid(1000);
    let floor = d.as_ptr();
    let ceil = unsafe { floor.add(d.len()) };
    let unknown = ctx.cat_unknown as u32;
    let mut memo = DateMemo::new();
    let sh1 = 64 - p1_bits();
    let sh2 = 64 - p2_bits();
    let tab = &ctx.links;
    let mut ring = [PendE::default(); RING];
    let mut rn = 0usize;
    let stage = diag_stage();
    let mut sink = 0u64;
    scan_lines::<7>(d, start, end, |f| {
        if cfg!(feature = "diag") && stage == 1 {
            sink = sink.wrapping_add(f[6].len() as u64);
            return;
        }
        if f[0].is_empty() {
            return;
        }
        let eid = pack_fast(f[0], ceil);
        let ingest = match parse_ingest_s(f[3], ceil, &mut memo) {
            Some(v) => (v - ws_s).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            None => i32::MAX,
        };
        if cfg!(feature = "diag") && stage == 2 {
            sink = sink.wrapping_add(eid).wrapping_add(ingest as u64);
            return;
        }
        'v: {
            if f[1].is_empty() || f[5].is_empty() {
                break 'v;
            }
            let Some(ts) = parse_ts_fast(f[2], floor, ceil, &mut memo) else { break 'v };
            if cfg!(feature = "diag") && stage == 3 {
                sink = sink.wrapping_add(eid).wrapping_add(ingest as u64).wrapping_add(ts as u64);
                return;
            }
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
            // the category is read only for purchases (price > 0)
            let pc = if price != 0 { (price << 8) | ctx.products.get(f[5], floor).unwrap_or(unknown) } else { 0 };
            let user = pack_fast(f[1], ceil);
            let h = hk(user);
            if cfg!(feature = "diag") && stage == 4 {
                sink = sink.wrapping_add(eid).wrapping_add(ingest as u64).wrapping_add(ts as u64).wrapping_add(pc as u64).wrapping_add(h);
                return;
            }
            tab.prefetch(h);
            let slot = rn & (RING - 1);
            if rn >= RING {
                let old = ring[slot];
                if cfg!(feature = "diag") && stage >= 5 {
                    let pr = tab.person(old.user, old.h);
                    sink = sink.wrapping_add(pr);
                    if stage >= 6 {
                        let hp = hk(pr);
                        let at = sb.push((hp >> sh2) as usize, SRec { person: pr, ts: old.ts, pc: old.pc }, bs);
                        sink = sink.wrapping_add(at as u64);
                    }
                } else {
                    emit_event(tab, &old, eb, sb, bs, sh1, sh2, abase);
                }
            }
            ring[slot] = PendE { user, h, eid, ts: (ts - ws) as u32, pc, ingest };
            rn += 1;
            return;
        }
        // an invalid copy still takes part in dedup
        eb.push((hk(eid) >> sh1) as usize, DRec { eid, ingest, loc: 0 }, bs);
    });
    std::hint::black_box(sink);
    if cfg!(feature = "diag") && stage != 0 {
        return;
    }
    for p in &ring[..rn.min(RING)] {
        emit_event(tab, p, eb, sb, bs, sh1, sh2, abase);
    }
    Scatter::<SRec>::fence();
}

/// Diagnostic stage (feature `diag` only): 0 = full run.
#[inline(always)]
fn diag_stage() -> u32 {
    if cfg!(feature = "diag") {
        static S: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        *S.get_or_init(|| std::env::var("STAGE").ok().and_then(|v| v.parse().ok()).unwrap_or(0))
    } else {
        0
    }
}

#[inline(always)]
fn emit_click(tab: &LinkTab, p: &PendC, cb: &mut Scatter<CRec>, bs: &mut Blocks, sh2: u32) {
    let person = tab.person(p.user, p.h);
    let hp = hk(person);
    cb.push((hp >> sh2) as usize, CRec { person, cid: p.cid, ts: p.ts, camp: p.camp }, bs);
}

#[inline(never)]
fn parse_clicks(ctx: &Ctx, d: &[u8], start: usize, end: usize, cb: &mut Scatter<CRec>, bs: &mut Blocks) {
    let cws = ctx.ws - WEEK_MS;
    let we = ctx.we;
    let mut memo = DateMemo::new();
    let floor = d.as_ptr();
    let ceil = unsafe { floor.add(d.len()) };
    let sh2 = 64 - p2_bits();
    let tab = &ctx.links;
    let mut ring = [PendC::default(); RING];
    let mut rn = 0usize;
    scan_lines::<4>(d, start, end, |f| {
        if f[0].is_empty() || f[1].is_empty() {
            return;
        }
        let Some(ts) = parse_ts_fast(f[3], floor, ceil, &mut memo) else { return };
        if ts < cws || ts >= we {
            return;
        }
        let Some(camp) = ctx.campaigns.get(f[2], floor) else { return };
        let user = pack_fast(f[1], ceil);
        let h = hk(user);
        tab.prefetch(h);
        let slot = rn & (RING - 1);
        if rn >= RING {
            let old = ring[slot];
            emit_click(tab, &old, cb, bs, sh2);
        }
        ring[slot] = PendC { user, h, cid: pack_fast(f[0], ceil), ts, camp };
        rn += 1;
    });
    for p in &ring[..rn.min(RING)] {
        emit_click(tab, p, cb, bs, sh2);
    }
    Scatter::<CRec>::fence();
}

/// Scratch of one dedup worker.
#[derive(Default)]
struct DedupScratch {
    map: Option<KMap<u64>>,
    seen: Vec<u64>,
    twice: Vec<u64>,
}

/// Keeps, per event_id, the copy with the smallest ingest_ts: every other
/// copy that has an event record gets that record marked DEAD. An invalid
/// copy has no record (loc 0) but still wins or loses like any other, so a
/// later valid copy never rescues an event whose earliest copy is invalid.
///
/// Almost every id occurs once, so a first pass only finds the ids that may
/// repeat: each copy sets a bit chosen by its id's hash (16 bits per copy,
/// so the bitmap stays in L2), and a bit that is hit again is noted. All
/// copies of one id hit the same bit, so every repeated id is noted. The
/// second pass sends just the copies on noted bits through the exact table.
#[inline(never)]
fn dedup_partition(lists: &mut [BList<DRec>], sc: &mut DedupScratch, bs: &mut Blocks, abase: usize, pbits: u32) {
    let n: usize = lists.iter().map(|l| l.len).sum();
    if n == 0 {
        return;
    }
    let bits = (n * 16).next_power_of_two().max(1 << 12);
    let sh = 64 - bits.trailing_zeros();
    let seen = &mut sc.seen;
    let twice = &mut sc.twice;
    seen.clear();
    seen.resize(bits / 64, 0);
    twice.clear();
    twice.resize(bits / 64, 0);
    // the hash bits below the ones that chose this partition
    let slot = |eid: Key| -> (usize, u64) {
        let s = ((hk(eid) << pbits) >> sh) as usize;
        (s >> 6, 1u64 << (s & 63))
    };
    for l in lists.iter() {
        l.for_each_slice(|sl| {
            for r in sl {
                let (w, m) = slot(r.eid);
                unsafe {
                    let cur = *seen.get_unchecked(w);
                    *twice.get_unchecked_mut(w) |= cur & m;
                    *seen.get_unchecked_mut(w) = cur | m;
                }
            }
        });
    }
    let mp = sc.map.get_or_insert_with(|| KMap::<u64>::with_capacity(16));
    mp.reset(n / 8 + 16);
    for l in lists.iter() {
        l.for_each_slice(|sl| {
            for r in sl {
                let (w, m) = slot(r.eid);
                if unsafe { *twice.get_unchecked(w) } & m == 0 {
                    continue; // the only copy of its id
                }
                // value = (ingest as u32) << 32 | loc
                let v = ((r.ingest as u32 as u64) << 32) | r.loc as u64;
                let (s, ins) = mp.entry(r.eid, v);
                if !ins {
                    let cur = *mp.val_mut(s);
                    let loser = if r.ingest < (cur >> 32) as u32 as i32 {
                        *mp.val_mut(s) = v;
                        cur as u32
                    } else {
                        r.loc
                    };
                    if loser != 0 {
                        // SRec.pc of the losing copy; no other thread touches this record
                        unsafe { *((abase + (loser as usize - 1) * 16 + 12) as *mut u32) = DEAD };
                    }
                }
            }
        });
    }
    for l in lists.iter_mut() {
        l.release(&mut bs.free);
    }
}

#[cfg(test)]
mod dedup_tests {
    use super::*;
    /// `dedup_partition` against the rule stated directly: per id the copy
    /// with the smallest ingest wins (the first seen on a tie), every other
    /// copy that has a record is dead, and nothing else is touched.
    #[test]
    fn dedup_matches_reference() {
        let arena = Arena::new(256 << 20, 256 << 20);
        let mut x: u64 = 0x5DEECE66D;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for &(n, idspace, pbits) in &[(1usize, 1u64, 0u32), (2, 1, 9), (50, 10, 9), (5000, 4000, 9), (200_000, 150_000, 9), (100_000, 3, 4), (70_000, 1 << 40, 11)] {
            let mut recs: Vec<SRec> = (0..n).map(|i| SRec { person: i as u64, ts: i as u32, pc: 7 }).collect();
            let abase = recs.as_mut_ptr() as usize;
            let mut bs = Blocks::new(&arena);
            let mut sc = Scatter::<DRec>::new(3);
            let mut all = Vec::new();
            for i in 0..n {
                let eid = pack(format!("e{}", rnd() % idspace).as_bytes());
                let ingest = (rnd() % 5) as i32 - 2;
                let loc = if rnd() % 7 == 0 { 0 } else { i as u32 + 1 };
                let d = DRec { eid, ingest, loc };
                sc.push((rnd() % 3) as usize, d, &mut bs);
                all.push(d);
            }
            let mut lists = sc.finish();
            // reference, in the order the lists hold the copies
            let mut order = Vec::new();
            for l in &lists {
                l.for_each_slice(|sl| order.extend_from_slice(sl));
            }
            assert_eq!(order.len(), n);
            let mut best: HashMap<Key, (i32, u32)> = HashMap::new();
            let mut dead = vec![false; n];
            for d in &order {
                match best.get_mut(&d.eid) {
                    None => {
                        best.insert(d.eid, (d.ingest, d.loc));
                    }
                    Some(b) => {
                        let loser = if d.ingest < b.0 {
                            let old = b.1;
                            *b = (d.ingest, d.loc);
                            old
                        } else {
                            d.loc
                        };
                        if loser != 0 {
                            dead[loser as usize - 1] = true;
                        }
                    }
                }
            }
            dedup_partition(&mut lists, &mut DedupScratch::default(), &mut bs, abase, pbits);
            for i in 0..n {
                assert_eq!(recs[i].pc == DEAD, dead[i], "n {n} rec {i}");
                assert!(recs[i].pc == DEAD || recs[i].pc == 7);
                assert_eq!((recs[i].person, recs[i].ts), (i as u64, i as u32));
            }
        }
    }
}

/// A click inside a person group: time relative to the click window start.
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct CSort {
    cid: Key,
    ts: u32,
    camp: u32,
}

#[derive(Default)]
struct Scratch {
    gmap: Option<KMap<u32>>,
    gkeys: Vec<Key>,
    counts: Vec<u32>,
    off: Vec<u32>,
    pos: Vec<u32>,
    gid: Vec<u32>,
    ev: Vec<u64>,
    ccounts: Vec<u32>,
    coff: Vec<u32>,
    cgid: Vec<u32>,
    cl: Vec<CSort>,
    cat_rev: Vec<u64>,
    touched: Vec<u16>,
    acnt: Vec<u32>,
    apos: Vec<u32>,
    since: Vec<u32>,
    active: Vec<u32>,
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
    gmap.reset(n / 4 + 16);
    // group events by person (counting sort); copies that lost dedup are skipped
    let gkeys = &mut sc.gkeys;
    let counts = &mut sc.counts;
    let gid = &mut sc.gid;
    gkeys.clear();
    counts.clear();
    gid.clear();
    for l in slists {
        l.for_each_slice(|sl| {
            for r in sl {
                if r.pc == DEAD {
                    continue;
                }
                let (s, ins) = gmap.entry(r.person, gkeys.len() as u32);
                if ins {
                    gkeys.push(r.person);
                    counts.push(0);
                }
                let g = *gmap.val_mut(s);
                counts[g as usize] += 1;
                gid.push(g);
            }
        });
    }
    let ng = gkeys.len();
    let off = &mut sc.off;
    off.clear();
    off.push(0);
    for g in 0..ng {
        let o = off[g] + counts[g];
        off.push(o);
    }
    let pos = &mut sc.pos;
    pos.clear();
    pos.extend_from_slice(&off[..ng]);
    // an event inside its group is one word: ts << 32 | pc, so sorting the
    // words sorts by time (the order among equal times does not matter)
    let ev = &mut sc.ev;
    ev.clear();
    ev.reserve(n);
    unsafe { ev.set_len(n) };
    {
        let mut i = 0;
        for l in slists {
            l.for_each_slice(|sl| {
                for r in sl {
                    if r.pc == DEAD {
                        continue;
                    }
                    let g = gid[i] as usize;
                    unsafe { *ev.get_unchecked_mut(pos[g] as usize) = ((r.ts as u64) << 32) | r.pc as u64 };
                    pos[g] += 1;
                    i += 1;
                }
            });
        }
    }
    // group clicks by person (only persons with events)
    let ccounts = &mut sc.ccounts;
    ccounts.clear();
    ccounts.resize(ng, 0);
    let cgid = &mut sc.cgid;
    cgid.clear();
    for l in clists {
        l.for_each_slice(|sl| {
            for r in sl {
                let g = gmap.get(r.person).unwrap_or(u32::MAX);
                if g != u32::MAX {
                    ccounts[g as usize] += 1;
                }
                cgid.push(g);
            }
        });
    }
    let coff = &mut sc.coff;
    coff.clear();
    coff.push(0);
    for g in 0..ng {
        let o = coff[g] + ccounts[g];
        coff.push(o);
    }
    pos.clear();
    pos.extend_from_slice(&coff[..ng]);
    let cl = &mut sc.cl;
    let ncl = coff[ng] as usize;
    cl.clear();
    cl.reserve(ncl);
    unsafe { cl.set_len(ncl) };
    let cws = ctx.ws - WEEK_MS;
    {
        let mut j = 0;
        for l in clists {
            l.for_each_slice(|sl| {
                for r in sl {
                    let g = cgid[j];
                    if g != u32::MAX {
                        // a valid click is inside [window_start - 7 d, window_end)
                        let c = CSort { cid: r.cid, ts: (r.ts - cws) as u32, camp: r.camp };
                        unsafe { *cl.get_unchecked_mut(pos[g as usize] as usize) = c };
                        pos[g as usize] += 1;
                    }
                    j += 1;
                }
            });
        }
    }

    let cat_rev = &mut sc.cat_rev;
    cat_rev.clear();
    cat_rev.resize(ncat, 0);
    let touched = &mut sc.touched;
    if sc.acnt.len() < ctx.camp_names.len() + 1 {
        sc.acnt = vec![0; ctx.camp_names.len() + 1];
        sc.apos = vec![0; ctx.camp_names.len() + 1];
        sc.since = vec![0; ctx.camp_names.len() + 1];
    }
    let acnt = &mut sc.acnt;
    let apos = &mut sc.apos;
    let since = &mut sc.since;
    let active = &mut sc.active;
    let organic = ctx.camp_names.len() as u32;
    let ws = ctx.ws;
    const WEEK: u64 = WEEK_MS as u64;

    for g in 0..ng {
        let evs = &mut ev[off[g] as usize..off[g + 1] as usize];
        if evs.len() > 1 {
            evs.sort_unstable();
        }
        let cls = &mut cl[coff[g] as usize..coff[g + 1] as usize];
        if cls.len() > 1 {
            cls.sort_unstable_by(|x, y| x.ts.cmp(&y.ts).then_with(|| key_cmp(x.cid, y.cid)));
        }
        let person = gkeys[g];
        let mut cur_day: i32 = i32::MIN;
        // sessions, events, revenue of the current day so far
        let mut d_sess = 0u32;
        let mut d_events = 0u32;
        let mut d_rev = 0u64;
        let mut cj = 0usize;
        let mut wlo = 0usize;

        let mut k = 0usize;
        let ne = evs.len();
        while k < ne {
            let start_off = (evs[k] >> 32) as u32;
            let start = ws + start_off as i64;
            let mut last = start_off;
            let mut cnt = 0u32;
            let mut rev = 0u64;
            let mut purchase = false;
            let day = start.div_euclid(DAY_MS) as i32;
            if day != cur_day {
                if cur_day != i32::MIN {
                    // the day ends: every campaign still in the window assisted
                    // the day's sessions since it entered (or since the day began)
                    let pd = (cur_day as i64 - ctx.day0) as usize;
                    for &c in active.iter() {
                        a.camp_cell(c as usize, pd)[3] += (d_sess - since[c as usize]) as u64;
                        since[c as usize] = 0;
                    }
                    flush_user(a, person, cur_day, d_sess, d_events, d_rev, cat_rev, touched);
                }
                cur_day = day;
                d_sess = 0;
                d_events = 0;
                d_rev = 0;
            }
            while k < ne && ((evs[k] >> 32) as u32) - last <= GAP_MS as u32 {
                let r = evs[k];
                last = (r >> 32) as u32;
                cnt += 1;
                let price = (r as u32) >> 8;
                if price != 0 {
                    purchase = true;
                    rev += price as u64;
                    let c = (r & 0xFF) as usize;
                    if cat_rev[c] == 0 {
                        touched.push(c as u16);
                    }
                    cat_rev[c] += price as u64;
                }
                k += 1;
            }
            let di = (day as i64 - ctx.day0) as usize;
            // Clicks in [start - 7d, start] form the window: credit picks the
            // latest, every distinct campaign in it assists. Click times are
            // relative to window_start - 7d, so a click is at or before the
            // start iff ts <= start_off + 7d, and older than 7 days iff
            // ts < start_off. Assists are counted by intervals: a campaign
            // that enters the window notes how many sessions the day had
            // (`since`), and is paid for the sessions since then when it
            // leaves, when the day ends, or when the person ends.
            while cj < cls.len() && cls[cj].ts as u64 <= start_off as u64 + WEEK {
                let c = cls[cj].camp as usize;
                if acnt[c] == 0 {
                    apos[c] = active.len() as u32;
                    active.push(c as u32);
                    since[c] = d_sess;
                }
                acnt[c] += 1;
                cj += 1;
            }
            while wlo < cj && cls[wlo].ts < start_off {
                let c = cls[wlo].camp as usize;
                acnt[c] -= 1;
                if acnt[c] == 0 {
                    // it assisted the sessions before this one
                    a.camp_cell(c, di)[3] += (d_sess - since[c]) as u64;
                    let i = apos[c] as usize;
                    let last = active.pop().unwrap();
                    if last as usize != c {
                        active[i] = last;
                        apos[last as usize] = i as u32;
                    }
                }
                wlo += 1;
            }
            let mut camp = organic;
            if cj > 0 {
                let lt = cls[cj - 1].ts;
                if lt >= start_off {
                    let mut q = cj - 1;
                    while q > 0 && cls[q - 1].ts == lt {
                        q -= 1;
                    }
                    camp = cls[q].camp;
                }
            }
            let cell = a.camp_cell(camp as usize, di);
            cell[0] += 1;
            cell[1] += purchase as u64;
            cell[2] += rev;
            d_sess += 1;
            d_events += cnt;
            d_rev += rev;
        }
        let pd = (cur_day as i64 - ctx.day0) as usize;
        for &c in active.iter() {
            a.camp_cell(c as usize, pd)[3] += (d_sess - since[c as usize]) as u64;
            acnt[c as usize] = 0;
        }
        active.clear();
        flush_user(a, person, cur_day, d_sess, d_events, d_rev, cat_rev, touched);
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
