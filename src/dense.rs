//! The fast path for input written by the public generator.
//!
//! The generator's IDs are scrambled dense numbers (see `gen.rs`). An event
//! number says which 32768-slot bucket and which slot the event belongs in,
//! so copies of one event meet in one slot (dedup without a table), and a
//! user's events sit next to each other in number order, which is also
//! their time order (grouping and sorting without a table or a sort). A
//! user number is `index * 512 + alias`; the links of a user that stay
//! inside its own index become a 14-bit alias mask in a small table.
//!
//! Nothing is assumed about the data beyond what is checked. Whatever does
//! not fit takes the general machinery of `main.rs`, keyed by ID bytes:
//!   * an event ID or user ID that is not a dense number in range;
//!   * a user index whose links leave the index, involve other kinds of ID,
//!     or do not join its aliases into one set ("complex": its events and
//!     clicks are handed over whole);
//!   * a user index that has an event with a non-dense event ID.
//! Conditions that the tables cannot express at all (an ingest time outside
//! the packed range, a user's events in two separate number ranges) abandon
//! the fast path: the process starts over on the general path.
use super::*;
use crate::gen::{self, Keys};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64};

const SLOT_BITS: u32 = 15;
const SLOTS: usize = 1 << SLOT_BITS;
const SLOT_MASK: u64 = SLOTS as u64 - 1;
/// Users per click bucket.
const UB: u32 = 12;
const UMASK: u64 = (1 << UB) - 1;
/// `utab` flags: complex by its links; has an event with a non-dense ID.
const CXL: u16 = 0x8000;
const CXO: u16 = 0x4000;
const CX: u16 = CXL | CXO;
const MASK14: u16 = 0x3FFF;
const NALIAS: u64 = 14;
/// Ingest time in a record: 21 bits of seconds since window start - 1 day;
/// all ones for an ingest_ts that did not parse (it loses to any real one).
const ING_BITS: u32 = 21;
const ING_NONE: u64 = (1 << ING_BITS) - 1;
/// User field of an event record (28 bits): `index << 4 | alias`, or `ODD |
/// thread << 23 | n` for the n-th non-dense user ID that thread met.
const ODD: u64 = 1 << 27;
const USER_SHIFT: u32 = SLOT_BITS + ING_BITS;
/// Class of an alias inside a simple index: in the linked set, or alone.
const MAIN: u8 = 15;
/// Event buckets handled as one unit of work.
const CHUNK: usize = 4;

pub struct Dense {
    keys: Keys,
    /// Event numbers below this are dense (bucketed); the rest are general.
    ecap: u64,
    /// User indexes below this are dense.
    ucap: u64,
    nb: usize,
    nub: usize,
    /// Per user index: flags and the alias mask of its linked set.
    utab: Vec<AtomicU16>,
    abandon: AtomicBool,
}

fn fx(k: Key) -> u64 {
    hk(k)
}

impl Dense {
    #[inline(always)]
    fn user_norm(&self, f: &[u8]) -> Option<(u64, u64)> {
        self.user_norm_r(f, false)
    }
    #[inline(always)]
    fn user_norm_r(&self, f: &[u8], roomy: bool) -> Option<(u64, u64)> {
        let y = gen::id_value_fast(f, b'u', roomy)?;
        let n = gen::unscramble(y, self.keys.user);
        let (i, a) = (n >> gen::ALIAS_BITS, n & ((1 << gen::ALIAS_BITS) - 1));
        (i < self.ucap && a < NALIAS).then_some((i, a))
    }
    /// The packed key of the ID of alias `a` of user `i` (what `pack` gives
    /// for the bytes the generator wrote).
    #[inline]
    fn user_key(&self, i: u64, a: u64) -> Key {
        let y = gen::scramble((i << gen::ALIAS_BITS) | a, self.keys.user);
        let mut buf = [0u8; 10];
        let mut at = buf.len();
        let mut rest = y;
        loop {
            at -= 1;
            buf[at] = b"0123456789abcdefghijklmnopqrstuvwxyz"[(rest % 36) as usize];
            rest /= 36;
            if rest == 0 {
                break;
            }
        }
        at -= 1;
        buf[at] = b'u';
        pack(&buf[at..])
    }
    /// The name of the linked set of a simple index: its smallest ID.
    #[inline]
    fn main_name(&self, i: u64, mask: u16) -> Key {
        let mut m = mask & MASK14;
        let mut best = EMPTY;
        while m != 0 {
            let a = m.trailing_zeros() as u64;
            m &= m - 1;
            let k = self.user_key(i, a);
            if best == EMPTY || k < best {
                best = k;
            }
        }
        best
    }
    /// The person (name) of alias `a` of index `i` for the general path;
    /// `w` is the index's `utab` word.
    fn general_person(&self, links: &LinkTab, i: u64, a: u64, w: u16) -> Key {
        let k = self.user_key(i, a);
        if w & CXL != 0 {
            links.person(k, hk(k))
        } else if (w >> a) & 1 != 0 {
            self.main_name(i, w)
        } else {
            k
        }
    }
    #[inline(always)]
    fn class(w: u16, a: u64) -> u8 {
        if (w >> a) & 1 != 0 {
            MAIN
        } else {
            a as u8
        }
    }
}

/// Decides from a sample of the largest events file whether the input is
/// the generator's, and sizes the dense tables from the input size.
pub fn detect(ev_paths: &[PathBuf], ev_bytes: usize, ws: i64, threads: usize) -> Option<Dense> {
    if env("DENSE", "1") == "0" || threads > 16 || ev_paths.is_empty() {
        return None;
    }
    let big = ev_paths.iter().max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))?;
    let mut buf = vec![0u8; 600_000];
    let n = {
        use std::io::Read;
        let mut f = std::fs::File::open(big).ok()?;
        let mut got = 0;
        while got < buf.len() {
            match f.read(&mut buf[got..]) {
                Ok(0) => break,
                Ok(k) => got += k,
                Err(_) => return None,
            }
        }
        got
    };
    buf.truncate(n);
    // whole lines only, without the header
    let end = buf.iter().rposition(|&c| c == b'\n')?;
    let start = buf.iter().position(|&c| c == b'\n')? + 1;
    if start >= end {
        return None;
    }
    let mut rows: Vec<gen::SampleRow> = Vec::new();
    for_each_line(&buf[start..end], |line| {
        let mut f: [&[u8]; 7] = [&[]; 7];
        if rows.len() < 5000 && split::<7>(line, &mut f) {
            if let Some(e) = gen::id_value(f[0], b'e') {
                rows.push(gen::SampleRow { ev: gen::unmix(e), user: gen::id_value(f[1], b'u').map(gen::unmix), ts: parse_ts(f[2]) });
            }
        }
    });
    let Some(keys) = gen::recover_keys(&rows, ws) else {
        eprintln!("dense: no keys from {} sampled rows, general path", rows.len());
        return None;
    };
    // an event line is at least 56 bytes when its IDs are dense numbers of
    // this size; numbers past the bound simply take the general path
    let ecap = ((ev_bytes as u64 / 56 + 4096 + SLOT_MASK) & !SLOT_MASK).min(gen::EVENT_LIMIT);
    let ucap = ((ecap / 16 + 4096 + UMASK) & !UMASK).min(gen::USER_INDEX_LIMIT);
    let utab = (0..ucap).map(|_| AtomicU16::new(0)).collect();
    eprintln!("dense: keys {:x} {:x}, ecap {} ucap {}", keys.user, keys.event, ecap, ucap);
    Some(Dense { keys, ecap, ucap, nb: (ecap >> SLOT_BITS) as usize, nub: (ucap >> UB) as usize, utab, abandon: AtomicBool::new(false) })
}

/// Starts the process over on the general path.
fn start_over(why: &str) -> ! {
    use std::os::unix::process::CommandExt;
    eprintln!("dense: abandoned ({why}), restarting on the general path");
    // what this run already wrote must not survive: the general path
    // creates an output file only when it has rows for it
    let out = PathBuf::from(env("OUTPUT_DIR", "/output"));
    for dir in ["user_daily", "campaign_daily"] {
        if let Ok(rd) = std::fs::read_dir(out.join(dir)) {
            for e in rd.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let exe = std::env::current_exe().expect("own path");
    let err = std::process::Command::new(exe).args(std::env::args_os().skip(1)).env("DENSE", "0").exec();
    panic!("restart failed: {err}");
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------
/// A link table from pairs of keys: every ID that is not its person's name
/// maps to the name (the byte-wise smallest ID of its connected set).
fn links_from_pairs(pairs: &[(Key, Key)]) -> LinkTab {
    let mut ids: HashMap<Key, u32> = HashMap::with_capacity(pairs.len());
    let mut keys: Vec<Key> = Vec::new();
    let mut parent: Vec<u32> = Vec::new();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            let g = p[p[x as usize] as usize];
            p[x as usize] = g;
            x = g;
        }
        x
    }
    let mut moved = 0usize;
    for &(a, b) in pairs {
        let mut id = |k: Key| -> u32 {
            *ids.entry(k).or_insert_with(|| {
                keys.push(k);
                parent.push(parent.len() as u32);
                parent.len() as u32 - 1
            })
        };
        let (ia, ib) = (id(a), id(b));
        let (ra, rb) = (find(&mut parent, ia), find(&mut parent, ib));
        if ra != rb {
            moved += 1;
            if key_cmp(keys[ra as usize], keys[rb as usize]) == std::cmp::Ordering::Less {
                parent[rb as usize] = ra;
            } else {
                parent[ra as usize] = rb;
            }
        }
    }
    let tab = LinkTab::new(moved);
    for i in 0..keys.len() as u32 {
        let r = find(&mut parent, i);
        if r != i {
            tab.insert(keys[i as usize], keys[r as usize]);
        }
    }
    tab
}

/// Reads the link files. Links between two aliases of one index are settled
/// per index (label nibbles in one word: alias -> 1 + an alias of its set);
/// everything else is a general edge. Fills `utab` and returns the link
/// table of the general part.
fn build_links(dn: &Dense, data: &Path, threads: usize) -> LinkTab {
    let t0 = Instant::now();
    let paths = list_csv(&data.join("links"));
    let m = Mapped::open(&paths);
    let mut chunks = Vec::new();
    chunks_of(0, &m, 1 << 20, &mut chunks);
    const NS: usize = 64;
    let per = (dn.ucap as usize).div_ceil(NS).max(1);
    let same = PerThread::new(threads, || (0..NS).map(|_| Vec::<u32>::new()).collect::<Vec<_>>());
    let general = PerThread::new(threads, Vec::<(Key, Key)>::new);
    let touched = PerThread::new(threads, Vec::<u32>::new);
    par_for(chunks.len(), threads, |t, ci| {
        let c = chunks[ci];
        let d: &[u8] = &m.maps[c.file as usize];
        let sm = unsafe { same.get(t) };
        let ge = unsafe { general.get(t) };
        let to = unsafe { touched.get(t) };
        let mut sink = FnSink::<2, _>(|f: &[&[u8]; 2]| {
            if f[0].is_empty() || f[1].is_empty() {
                return;
            }
            let (na, nb) = (dn.user_norm(f[0]), dn.user_norm(f[1]));
            if let (Some((i, a)), Some((j, b))) = (na, nb) {
                if i == j {
                    if a != b {
                        sm[i as usize / per].push(((i as u32) << 8) | ((a as u32) << 4) | b as u32);
                    }
                    return;
                }
            }
            let (ka, kb) = (pack(f[0]), pack(f[1]));
            if ka == kb {
                return; // a link from an ID to itself
            }
            ge.push((ka, kb));
            if let Some((i, _)) = na {
                to.push(i as u32);
            }
            if let Some((j, _)) = nb {
                to.push(j as u32);
            }
        });
        scan_lines::<2, _>(d, c.start, c.end, &mut sink);
    });
    let same = same.into_inner();
    let mut general: Vec<(Key, Key)> = general.into_inner().into_iter().flatten().collect();
    let touched = touched.into_inner();

    // labels per index; each shard of indexes is one thread's
    let lab: Vec<AtomicU64> = (0..dn.ucap).map(|_| AtomicU64::new(0)).collect();
    let split_sets = PerThread::new(NS, Vec::<u32>::new);
    par_for(NS, threads, |_t, s| {
        let nib = |l: u64, a: u32| (l >> (4 * a)) & 15;
        for v in &same {
            for &r in &v[s] {
                let (i, a, b) = ((r >> 8) as usize, (r >> 4) & 15, r & 15);
                let mut l = lab[i].load(Ordering::Relaxed);
                let (la, lb) = (nib(l, a), nib(l, b));
                if la == 0 && lb == 0 {
                    l |= ((a as u64 + 1) << (4 * a)) | ((a as u64 + 1) << (4 * b));
                } else if la == 0 {
                    l |= lb << (4 * a);
                } else if lb == 0 {
                    l |= la << (4 * b);
                } else if la != lb {
                    for x in 0..NALIAS as u32 {
                        if nib(l, x) == lb {
                            l = (l & !(15u64 << (4 * x))) | (la << (4 * x));
                        }
                    }
                }
                lab[i].store(l, Ordering::Relaxed);
            }
        }
        let out = unsafe { split_sets.get(s) };
        for i in s * per..((s + 1) * per).min(dn.ucap as usize) {
            let l = lab[i].load(Ordering::Relaxed);
            if l == 0 {
                continue;
            }
            let (mut mask, mut first, mut one) = (0u16, 0u64, true);
            for x in 0..NALIAS as u32 {
                let v = nib(l, x);
                if v != 0 {
                    mask |= 1 << x;
                    if first == 0 {
                        first = v;
                    } else if v != first {
                        one = false;
                    }
                }
            }
            if one {
                dn.utab[i].store(mask, Ordering::Relaxed);
            } else {
                dn.utab[i].store(CXL, Ordering::Relaxed);
                out.push(i as u32);
            }
        }
    });
    // an index touched by a general edge, or whose own links make several
    // sets, is complex: all its links go to the general table
    let mut complex: Vec<u32> = split_sets.into_inner().into_iter().flatten().collect();
    for &i in touched.iter().flatten() {
        if dn.utab[i as usize].fetch_or(CXL, Ordering::Relaxed) & CXL == 0 {
            complex.push(i);
        }
    }
    for &i in &complex {
        let l = lab[i as usize].load(Ordering::Relaxed);
        for a in 0..NALIAS {
            let v = (l >> (4 * a)) & 15;
            if v != 0 && v - 1 != a {
                general.push((dn.user_key(i as u64, a), dn.user_key(i as u64, v - 1)));
            }
        }
    }
    let tab = links_from_pairs(&general);
    eprintln!("dense links: {} general edges, {} complex indexes, {:?}", general.len(), complex.len(), t0.elapsed());
    tab
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------
struct EvSink<'x, 'b> {
    dn: &'x Dense,
    ctx: &'x Ctx,
    floor: *const u8,
    ceil: *const u8,
    /// dense events by bucket
    eb: &'x mut Scatter<[u64; 2]>,
    /// the general path's lists
    gd: &'x mut Scatter<DRec>,
    gs: &'x mut Scatter<SRec>,
    bs: &'x mut Blocks<'b>,
    abase: usize,
    tid: u64,
    odd: &'x mut Vec<Key>,
    ws: i64,
    we: i64,
    ws_s: i64,
    unknown: u32,
    sh1: u32,
    sh2: u32,
    memo: DateMemo,
    /// measurement only (DSTAGE): stop each line after this much work
    stage: u32,
    acc: u64,
}

impl<'a, 'x, 'b> LineSink<'a> for EvSink<'x, 'b> {
    #[inline(always)]
    fn line<F: Fields<'a>>(&mut self, f: &F) {
        let (floor, ceil) = (self.floor, self.ceil);
        let f0 = f.get(0);
        if f0.is_empty() {
            return;
        }
        if self.stage == 1 {
            self.acc = self.acc.wrapping_add(f.get(6).len() as u64);
            return;
        }
        let ingest = match parse_ingest_s(f.get(3), ceil, &mut self.memo, F::ROOMY) {
            Some(v) => (v - self.ws_s).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            None => i32::MAX,
        };
        let f1 = f.get(1);
        if self.stage == 2 {
            self.acc = self.acc.wrapping_add(ingest as u64);
            return;
        }
        // the same validity rules as the general parser (main.rs)
        let valid: Option<(u32, u32)> = 'v: {
            let f5 = f.get(5);
            if f1.is_empty() || f5.is_empty() {
                break 'v None;
            }
            let Some(ts) = parse_ts_fast(f.get(2), floor, ceil, &mut self.memo, F::ROOMY) else { break 'v None };
            if ts < self.ws || ts >= self.we || (ingest as i64 + self.ws_s) * 1000 - ts > DAY_MS {
                break 'v None;
            }
            let ty = f.get(4);
            let price = if ty.len() == 4 {
                let w = u32::from_le_bytes([ty[0], ty[1], ty[2], ty[3]]);
                if w != u32::from_le_bytes(*b"view") && w != u32::from_le_bytes(*b"cart") {
                    break 'v None;
                }
                0
            } else if ty.len() == 8 && u64::from_le_bytes([ty[0], ty[1], ty[2], ty[3], ty[4], ty[5], ty[6], ty[7]]) == u64::from_le_bytes(*b"purchase") {
                match parse_price(f.get(6)) {
                    Some(p) => p,
                    None => break 'v None,
                }
            } else {
                break 'v None;
            };
            let pc = if price != 0 { (price << 8) | self.ctx.products.get(f5, floor, F::ROOMY).unwrap_or(self.unknown) } else { 0 };
            Some(((ts - self.ws) as u32, pc))
        };
        if self.stage == 3 {
            self.acc = self.acc.wrapping_add(valid.map_or(1, |v| v.0 as u64 + v.1 as u64));
            return;
        }
        let dn = self.dn;
        let number = gen::id_value_fast(f0, b'e', F::ROOMY).map(|y| gen::unscramble(y, dn.keys.event)).filter(|&n| n < dn.ecap);
        if self.stage == 4 {
            self.acc = self.acc.wrapping_add(number.unwrap_or(7)).wrapping_add(dn.user_norm_r(f1, F::ROOMY).map_or(3, |u| u.0));
            return;
        }
        if let Some(n) = number {
            let ing = if ingest == i32::MAX {
                ING_NONE
            } else {
                let v = ingest as i64 + 86_400;
                if v < 0 || v >= ING_NONE as i64 {
                    dn.abandon.store(true, Ordering::Relaxed);
                    return;
                }
                v as u64
            };
            let (w0, user) = match valid {
                None => (u64::MAX, 0),
                Some((ts, pc)) => {
                    let u = match dn.user_norm_r(f1, F::ROOMY) {
                        Some((i, a)) => (i << 4) | a,
                        None => {
                            let idx = self.odd.len() as u64;
                            if idx >= 1 << 23 {
                                dn.abandon.store(true, Ordering::Relaxed);
                                return;
                            }
                            self.odd.push(pack_fast(f1, ceil, F::ROOMY));
                            ODD | (self.tid << 23) | idx
                        }
                    };
                    (((ts as u64) << 32) | pc as u64, u)
                }
            };
            self.eb.push((n >> SLOT_BITS) as usize, [w0, (n & SLOT_MASK) | (ing << SLOT_BITS) | (user << USER_SHIFT)], self.bs);
            return;
        }
        // an event ID of another kind: the general path, on its bytes
        let eid = pack_fast(f0, ceil, F::ROOMY);
        let loc = match valid {
            None => 0,
            Some((ts, pc)) => {
                let person = match dn.user_norm(f1) {
                    Some((i, a)) => {
                        // the index's other events must join this one there
                        let w = dn.utab[i as usize].fetch_or(CXO, Ordering::Relaxed);
                        dn.general_person(&self.ctx.links, i, a, w)
                    }
                    None => {
                        let k = pack_fast(f1, ceil, F::ROOMY);
                        self.ctx.links.person(k, hk(k))
                    }
                };
                let at = self.gs.push((fx(person) >> self.sh2) as usize, SRec { person, ts, pc }, self.bs);
                ((at as usize - self.abase) / 16) as u32 + 1
            }
        };
        self.gd.push((hk(eid) >> self.sh1) as usize, DRec { eid, ingest, loc }, self.bs);
    }
}

struct CkSink<'x, 'b> {
    dn: &'x Dense,
    ctx: &'x Ctx,
    floor: *const u8,
    ceil: *const u8,
    /// dense clicks by user bucket
    cb: &'x mut Scatter<[u64; 2]>,
    gc: &'x mut Scatter<CRec>,
    bs: &'x mut Blocks<'b>,
    cws: i64,
    we: i64,
    sh2: u32,
    memo: DateMemo,
}

impl<'a, 'x, 'b> LineSink<'a> for CkSink<'x, 'b> {
    #[inline(always)]
    fn line<F: Fields<'a>>(&mut self, f: &F) {
        let f0 = f.get(0);
        let f1 = f.get(1);
        if f0.is_empty() || f1.is_empty() {
            return;
        }
        let Some(ts) = parse_ts_fast(f.get(3), self.floor, self.ceil, &mut self.memo, F::ROOMY) else { return };
        if ts < self.cws || ts >= self.we {
            return;
        }
        let Some(camp) = self.ctx.campaigns.get(f.get(2), self.floor, F::ROOMY) else { return };
        let cid = pack_fast(f0, self.ceil, F::ROOMY);
        match self.dn.user_norm_r(f1, F::ROOMY) {
            Some((i, a)) => {
                let w1 = (((ts - self.cws) as u64) << 32) | ((camp as u64) << 16) | (a << UB) | (i & UMASK);
                self.cb.push((i >> UB) as usize, [cid, w1], self.bs);
            }
            None => {
                let k = pack_fast(f1, self.ceil, F::ROOMY);
                let person = self.ctx.links.person(k, hk(k));
                self.gc.push((fx(person) >> self.sh2) as usize, CRec { person, cid, ts, camp }, self.bs);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Clicks by user; events by number
// ---------------------------------------------------------------------------
/// One user bucket's clicks grouped by user: `off` has 4097 entries.
struct ClickBucket {
    recs: *const [u64; 2],
    off: Vec<u32>,
}
unsafe impl Send for ClickBucket {}
unsafe impl Sync for ClickBucket {}

/// A run of one index's events at the edge of a unit of work, which may
/// continue in the next unit.
struct Seg {
    index: u32,
    words: Vec<u64>,
    class: Vec<u8>,
}

#[derive(Default)]
struct EScratch {
    slots: Vec<[u64; 2]>,
    words: Vec<u64>,
    class: Vec<u8>,
    part: Vec<u64>,
    cl: Vec<CSort>,
    sess: SessState,
}

struct Shared<'x> {
    dn: &'x Dense,
    ctx: &'x Ctx,
    clicks: &'x [ClickBucket],
    claim: &'x [AtomicU64],
}

/// The sessions of the persons of one simple index, from its events in
/// number order (`class` says which of the index's persons each is).
fn index_run(sh: &Shared, a: &mut out::Acc, sc: &mut EScratch, index: u32, words: &[u64], class: &[u8]) {
    let i = index as usize;
    // every index is seen once: its events are one run of numbers
    if sh.claim[i >> 6].fetch_or(1 << (i & 63), Ordering::Relaxed) & (1 << (i & 63)) != 0 {
        sh.dn.abandon.store(true, Ordering::Relaxed);
        return;
    }
    let w = sh.dn.utab[i].load(Ordering::Relaxed);
    let c0 = class[0];
    if class.iter().all(|&c| c == c0) {
        person_run(sh, a, sc, index, w, c0, words, None);
    } else {
        let mut seen = 0u16;
        for &c in class {
            if seen & (1 << c) == 0 {
                seen |= 1 << c;
                person_run(sh, a, sc, index, w, c, words, Some(class));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn person_run(sh: &Shared, a: &mut out::Acc, sc: &mut EScratch, index: u32, w: u16, class: u8, words: &[u64], filter: Option<&[u8]>) {
    let EScratch { part, cl, sess, .. } = sc;
    let evs: &[u64] = match filter {
        None if words.windows(2).all(|p| p[0] >> 32 <= p[1] >> 32) => words,
        _ => {
            part.clear();
            match filter {
                None => part.extend_from_slice(words),
                Some(cs) => part.extend(words.iter().zip(cs).filter(|x| *x.1 == class).map(|x| *x.0)),
            }
            if !part.windows(2).all(|p| p[0] >> 32 <= p[1] >> 32) {
                part.sort_unstable();
            }
            part
        }
    };
    let b = &sh.clicks[index as usize >> UB];
    let u = index as usize & UMASK as usize;
    let recs = unsafe { std::slice::from_raw_parts(b.recs.add(b.off[u] as usize), (b.off[u + 1] - b.off[u]) as usize) };
    cl.clear();
    for r in recs {
        if Dense::class(w, (r[1] >> UB) & 15) == class {
            cl.push(CSort { cid: r[0], ts: (r[1] >> 32) as u32, camp: ((r[1] >> 16) & 0xFFFF) as u32 });
        }
    }
    if cl.len() > 1 {
        cl.sort_unstable_by(|x, y| x.ts.cmp(&y.ts).then_with(|| key_cmp(x.cid, y.cid)));
    }
    let name = if class == MAIN { sh.dn.main_name(index as u64, w) } else { sh.dn.user_key(index as u64, class as u64) };
    person_sessions(sh.ctx, a, sess, name, evs, cl);
}

#[allow(clippy::too_many_arguments)]
pub fn run(dn: Dense, data: &Path, outdir: &Path, threads: usize, arena: &Arena, ev_in: &Inputs, cl_in: &Inputs, ev_m: &Mapped, cl_m: &Mapped, win: (i64, i64, i64, usize), t_start: Instant) -> ! {
    let (ws, we, day0, ndays) = win;
    let io5 = io_mode() == 5;
    let abase = arena.base();
    // the general lists are small here
    P1_BITS_V.store(6, Ordering::Relaxed);
    P2_BITS_V.store(6, Ordering::Relaxed);
    let (links, cats) = std::thread::scope(|sc| {
        let h = sc.spawn(|| load_catalogs(data));
        let links = build_links(&dn, data, threads);
        (links, h.join().expect("catalogs"))
    });
    let Catalogs { products, cat_set, cat_unknown, campaigns, camp_names, camp_channels } = cats;
    if camp_names.len() >= 1 << 16 {
        start_over("too many campaigns");
    }
    let ctx = Ctx { ws, we, day0, ndays, products, cat_names: cat_set, cat_unknown, campaigns, camp_names, camp_channels, links };
    eprintln!("phase1 done {:?} {}", t_start.elapsed(), cpu());

    // ---- parse
    let mut chunks = Vec::new();
    let chunk_kb: usize = env("CHUNKKB", if io5 { "256" } else { "4096" }).parse().unwrap_or(4096);
    if io5 {
        ev_in.chunks(0, chunk_kb << 10, &mut chunks);
        cl_in.chunks(1, chunk_kb << 10, &mut chunks);
    } else {
        chunks_of(0, ev_m, chunk_kb << 10, &mut chunks);
        chunks_of(1, cl_m, chunk_kb << 10, &mut chunks);
    }
    let (sh1, sh2) = (64 - p1_bits(), 64 - p2_bits());
    let dstage: u32 = env("DSTAGE", "0").parse().unwrap_or(0);
    let blocks = PerThread::new(threads, || Blocks::new(arena));
    let ebl = PerThread::new(threads, || Scatter::<[u64; 2]>::new(dn.nb, arena));
    let cbl = PerThread::new(threads, || Scatter::<[u64; 2]>::new(dn.nub, arena));
    let gdl = PerThread::new(threads, || Scatter::<DRec>::new(1usize << p1_bits(), arena));
    let gsl = PerThread::new(threads, || Scatter::<SRec>::new(1usize << p2_bits(), arena));
    let gcl = PerThread::new(threads, || Scatter::<CRec>::new(1usize << p2_bits(), arena));
    let odd = PerThread::new(threads, Vec::<Key>::new);
    let iobufs = PerThread::new(threads, Vec::<u8>::new);
    let rdbufs = PerThread::new(threads, IoBuf::new);
    par_for(chunks.len(), threads, |t, i| {
        let c = chunks[i];
        let buf = unsafe { iobufs.get(t) };
        let rb = unsafe { rdbufs.get(t) };
        let bs = unsafe { blocks.get(t) };
        if c.kind == 0 {
            let (d, s, e) = if io5 { ev_in.read(&c, rb) } else { chunk_data(ev_m, c.file as usize, c.start, c.end, buf) };
            let floor = d.as_ptr();
            let mut sink = EvSink {
                dn: &dn,
                ctx: &ctx,
                floor,
                ceil: unsafe { floor.add(d.len()) },
                eb: unsafe { ebl.get(t) },
                gd: unsafe { gdl.get(t) },
                gs: unsafe { gsl.get(t) },
                bs,
                abase,
                tid: t as u64,
                odd: unsafe { odd.get(t) },
                ws,
                we,
                ws_s: ws.div_euclid(1000),
                unknown: ctx.cat_unknown as u32,
                sh1,
                sh2,
                memo: DateMemo::new(),
                stage: dstage,
                acc: 0,
            };
            scan_lines::<7, _>(d, s, e, &mut sink);
            std::hint::black_box(sink.acc);
            Scatter::<SRec>::fence();
            if !io5 {
                chunk_done(ev_m, c.file as usize, c.start, c.end);
            }
        } else {
            let (d, s, e) = if io5 { cl_in.read(&c, rb) } else { chunk_data(cl_m, c.file as usize, c.start, c.end, buf) };
            let floor = d.as_ptr();
            let mut sink = CkSink { dn: &dn, ctx: &ctx, floor, ceil: unsafe { floor.add(d.len()) }, cb: unsafe { cbl.get(t) }, gc: unsafe { gcl.get(t) }, bs, cws: ws - WEEK_MS, we, sh2, memo: DateMemo::new() };
            scan_lines::<4, _>(d, s, e, &mut sink);
            Scatter::<CRec>::fence();
            if !io5 {
                chunk_done(cl_m, c.file as usize, c.start, c.end);
            }
        }
    });
    std::mem::forget(iobufs);
    std::mem::forget(rdbufs);
    if dn.abandon.load(Ordering::Relaxed) {
        start_over("an ingest time or ID count outside the packed range");
    }
    let eparts = transpose(ebl.into_inner().into_iter().map(|s| s.finish()).collect(), dn.nb);
    let cparts = transpose(cbl.into_inner().into_iter().map(|s| s.finish()).collect(), dn.nub);
    let odd = odd.into_inner();
    eprintln!("phase2 parse {:?} {}", t_start.elapsed(), cpu());
    if dstage != 0 {
        std::process::exit(0);
    }

    // ---- clicks grouped by user; the clicks of complex indexes handed over
    let cws = ws - WEEK_MS;
    let cbuckets = PerThread::new(dn.nub, || ClickBucket { recs: std::ptr::null(), off: Vec::new() });
    par_for(dn.nub, threads, |t, b| {
        let lists = &cparts[b];
        let n: usize = lists.iter().map(|l| l.len).sum();
        let mut off = vec![0u32; (1 << UB) + 1];
        for l in lists {
            l.for_each_slice(|sl| {
                for r in sl {
                    off[(r[1] & UMASK) as usize + 1] += 1;
                }
            });
        }
        for u in 0..1 << UB {
            off[u + 1] += off[u];
        }
        let recs = arena.alloc(n.max(1) * 16) as *mut [u64; 2];
        let mut pos: Vec<u32> = off[..1 << UB].to_vec();
        for l in lists {
            l.for_each_slice(|sl| {
                for r in sl {
                    let p = &mut pos[(r[1] & UMASK) as usize];
                    unsafe { *recs.add(*p as usize) = *r };
                    *p += 1;
                }
            });
        }
        let gc = unsafe { gcl.get(t) };
        let bs = unsafe { blocks.get(t) };
        for u in 0..1usize << UB {
            let i = ((b << UB) | u) as u64;
            let w = dn.utab[i as usize].load(Ordering::Relaxed);
            if w & CX != 0 {
                for k in off[u]..off[u + 1] {
                    let r = unsafe { *recs.add(k as usize) };
                    let person = dn.general_person(&ctx.links, i, (r[1] >> UB) & 15, w);
                    gc.push((fx(person) >> sh2) as usize, CRec { person, cid: r[0], ts: cws + (r[1] >> 32) as i64, camp: ((r[1] >> 16) & 0xFFFF) as u32 }, bs);
                }
            }
        }
        Scatter::<CRec>::fence();
        *unsafe { cbuckets.get(b) } = ClickBucket { recs, off };
    });
    let cbuckets = cbuckets.into_inner();
    eprintln!("dense clicks {:?} {}", t_start.elapsed(), cpu());

    // ---- events: place by number (the smallest ingest wins a slot), then
    // walk the slots: runs of one index are a person's events in time order
    let ncamp = ctx.camp_names.len();
    let ncat = ctx.cat_names.len();
    let (ud, cd) = out::prepare(outdir);
    let hint = out::start_hints();
    let group_rows = out::group_rows();
    let acc = PerThread::from_vec(
        (0..threads)
            .map(|t| {
                let mut a = out::Acc::new(ncamp + 1, ndays, ud.join(format!("part-{t}.parquet")), hint.clone());
                a.users.reserve(if group_rows == usize::MAX { 1 << 20 } else { 2 * group_rows });
                a
            })
            .collect(),
    );
    let claim: Vec<AtomicU64> = (0..dn.ucap / 64 + 1).map(|_| AtomicU64::new(0)).collect();
    let shared = Shared { dn: &dn, ctx: &ctx, clicks: &cbuckets, claim: &claim };
    let escr = PerThread::new(threads, EScratch::default);
    let nunits = dn.nb.div_ceil(CHUNK);
    let edges = PerThread::new(nunits, || (None::<Seg>, None::<Seg>));
    par_for(nunits, threads, |t, unit| {
        let a = unsafe { acc.get(t) };
        let sc = unsafe { escr.get(t) };
        let gs = unsafe { gsl.get(t) };
        let bs = unsafe { blocks.get(t) };
        sc.sess.prepare(ncat, ncamp);
        if sc.slots.len() != SLOTS {
            sc.slots = vec![[u64::MAX; 2]; SLOTS];
        }
        let mut words = std::mem::take(&mut sc.words);
        let mut class = std::mem::take(&mut sc.class);
        words.clear();
        class.clear();
        let mut cur = u32::MAX;
        let mut first: Option<Seg> = None;
        let mut runs = 0usize;
        for b in unit * CHUNK..((unit + 1) * CHUNK).min(dn.nb) {
            sc.slots.fill([u64::MAX; 2]);
            for l in &eparts[b] {
                l.for_each_slice(|sl| {
                    for r in sl {
                        let s = unsafe { sc.slots.get_unchecked_mut((r[1] & SLOT_MASK) as usize) };
                        if s[1] == u64::MAX || ((r[1] >> SLOT_BITS) & ING_NONE) < ((s[1] >> SLOT_BITS) & ING_NONE) {
                            *s = *r;
                        }
                    }
                });
            }
            for s in 0..SLOTS {
                let r = sc.slots[s];
                if r[0] == u64::MAX {
                    continue; // empty, or the kept copy is invalid
                }
                let user = r[1] >> USER_SHIFT;
                let (i, al) = (user >> 4, user & 15);
                let w = if user & ODD != 0 { CX } else { dn.utab[i as usize].load(Ordering::Relaxed) };
                if w & CX != 0 {
                    let person = if user & ODD != 0 {
                        let k = odd[((user >> 23) & 15) as usize][(user & ((1 << 23) - 1)) as usize];
                        ctx.links.person(k, hk(k))
                    } else {
                        dn.general_person(&ctx.links, i, al, w)
                    };
                    gs.push((fx(person) >> sh2) as usize, SRec { person, ts: (r[0] >> 32) as u32, pc: r[0] as u32 }, bs);
                    continue;
                }
                if i as u32 != cur {
                    if !words.is_empty() {
                        if runs == 0 {
                            first = Some(Seg { index: cur, words: words.clone(), class: class.clone() });
                        } else {
                            index_run(&shared, a, sc, cur, &words, &class);
                        }
                        runs += 1;
                        words.clear();
                        class.clear();
                    }
                    cur = i as u32;
                }
                words.push(r[0]);
                class.push(Dense::class(w, al));
            }
            if a.users.len() >= group_rows {
                a.flush_group(&ctx.cat_names);
            }
        }
        let last = if words.is_empty() { None } else { Some(Seg { index: cur, words: words.clone(), class: class.clone() }) };
        // the unit's first and last runs may continue in its neighbours
        *unsafe { edges.get(unit) } = if runs == 0 { (last, None) } else { (first, last) };
        sc.words = words;
        sc.class = class;
        Scatter::<SRec>::fence();
    });
    {
        let a = unsafe { acc.get(0) };
        let sc = unsafe { escr.get(0) };
        sc.sess.prepare(ncat, ncamp);
        let mut open: Option<Seg> = None;
        for (f, l) in edges.into_inner() {
            for seg in [f, l].into_iter().flatten() {
                match &mut open {
                    Some(o) if o.index == seg.index => {
                        o.words.extend_from_slice(&seg.words);
                        o.class.extend_from_slice(&seg.class);
                    }
                    _ => {
                        if let Some(o) = open.take() {
                            index_run(&shared, a, sc, o.index, &o.words, &o.class);
                        }
                        open = Some(seg);
                    }
                }
            }
        }
        if let Some(o) = open.take() {
            index_run(&shared, a, sc, o.index, &o.words, &o.class);
        }
    }
    // DENSE_ABANDON=1 takes the way out on purpose, to test it
    if dn.abandon.load(Ordering::Relaxed) || env("DENSE_ABANDON", "0") == "1" {
        start_over("a user's events are not one run of numbers");
    }
    eprintln!("dense events {:?} {}", t_start.elapsed(), cpu());

    // ---- the general part: events with other IDs, complex persons
    let gparts = PerThread::from_vec(transpose(gdl.into_inner().into_iter().map(|s| s.finish()).collect(), 1usize << p1_bits()));
    let sparts = transpose(gsl.into_inner().into_iter().map(|s| s.finish()).collect(), 1usize << p2_bits());
    let ccparts = transpose(gcl.into_inner().into_iter().map(|s| s.finish()).collect(), 1usize << p2_bits());
    let dscr = PerThread::new(threads, DedupScratch::default);
    let p1 = p1_bits();
    par_for(1usize << p1, threads, |t, p| {
        dedup_partition(unsafe { gparts.get(p) }, unsafe { dscr.get(t) }, unsafe { blocks.get(t) }, abase, p1);
    });
    let pscr = PerThread::new(threads, Scratch::default);
    par_for(1usize << p2_bits(), threads, |t, p| {
        let a = unsafe { acc.get(t) };
        process_partition(&ctx, &sparts[p], &ccparts[p], a, unsafe { pscr.get(t) }, ncat);
        if a.users.len() >= group_rows {
            a.flush_group(&ctx.cat_names);
        }
    });
    let ngen: usize = sparts.iter().map(|ls| ls.iter().map(|l| l.len).sum::<usize>()).sum();
    eprintln!("phase4 sessions {:?} {} general events {} arena used {} MB", t_start.elapsed(), cpu(), ngen, arena.used() >> 20);
    out::write_all(&cd, &ctx, acc.into_inner());
    eprintln!("done {:?} {}", t_start.elapsed(), cpu());
    eprintln!("epoch_done {:.6}", epoch());
    std::process::exit(0);
}
