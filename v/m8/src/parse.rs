use crate::iface::*;
use crate::keyrec::unscramble;
use crate::sys::*;
use crate::*;
use std::sync::atomic::Ordering;

pub const CHUNK: usize = 4096;
pub const LINK_OFF: usize = 4032;
const BLOCK: usize = 2 << 20;
pub static mut NT: bool = true;
pub static mut RELEASE: bool = true;
pub static mut PREFETCH: bool = false;
static POPW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
fn popw() -> bool {
    *POPW.get_or_init(|| !std::env::var("XL_POPW").is_ok_and(|v| v == "0"))
}
const MADV_POPULATE_WRITE: i32 = 23;
#[cfg(feature = "knobs")]
pub static mut NOSINK: bool = false;
#[cfg(feature = "knobs")]
pub static mut SKIP: u32 = 0;
#[cfg(not(feature = "knobs"))]
pub const NOSINK: bool = false;
#[cfg(not(feature = "knobs"))]
pub const SKIP: u32 = 0;

#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct Line([u64; 8]);

pub struct Part {
    lines: Vec<Line>,
    fill: Vec<u8>,
    cur: Vec<*mut u64>,
    nch: Vec<u32>,
    n: Vec<u32>,
    head: Vec<*mut u64>,
    arena: *const Arena,
    blk: *mut u8,
    blk_end: *mut u8,
    popw: bool,
    pend: Vec<Line>,
}
const NOCHUNK: usize = LINK_OFF;
unsafe impl Send for Part {}
impl Part {
    pub fn empty() -> Part {
        Part { lines: Vec::new(), fill: Vec::new(), cur: Vec::new(), nch: Vec::new(), n: Vec::new(), head: Vec::new(), arena: std::ptr::null(), blk: std::ptr::null_mut(), blk_end: std::ptr::null_mut(), popw: false, pend: Vec::new() }
    }
    pub fn new(arena: &Arena, nb: usize) -> Part {
        Part {
            lines: vec![Line([0; 8]); nb],
            fill: vec![0; nb + 64],
            cur: vec![NOCHUNK as *mut u64; nb],
            nch: vec![0; nb],
            n: vec![0; nb],
            head: vec![std::ptr::null_mut(); nb],
            arena,
            blk: std::ptr::null_mut(),
            blk_end: std::ptr::null_mut(),
            popw: popw(),
            pend: vec![Line([0; 8]); 72],
        }
    }
    #[inline(always)]
    pub fn push(&mut self, b: usize, w0: u64, w1: u64) {
        unsafe {
            let fp = self.fill.as_mut_ptr().add(b);
            let f = *fp as usize;
            let l = self.lines.as_mut_ptr().add(b) as *mut u64;
            *l.byte_add(f) = w0;
            *l.byte_add(f + 8) = w1;
            if f == 48 {
                *fp = 0;
                self.flush(b, l);
            } else {
                *fp = (f + 16) as u8;
            }
        }
    }
    pub fn lines_ptr(&self) -> *const u8 {
        self.lines.as_ptr() as *const u8
    }
    #[inline(always)]
    pub unsafe fn push_many(&mut self, m: u64, n: usize, ob: *const u64, lp: *const u64, rec: *const [u64; 2]) {
        use std::arch::x86_64::*;
        let fill = self.fill.as_mut_ptr();
        let pend = self.pend.as_mut_ptr() as *mut __m512i;
        const PB: usize = 64 * 64;
        let mut k8 = 0usize;
        macro_rules! row {
            ($i:expr) => {{
                let i = $i;
                let l = *lp.add(i) as *mut u8;
                let b = *ob.add(i) as usize;
                let f = *fill.add(b) as usize;
                let r = _mm_load_si128(rec.add(i) as *const __m128i);
                let z = _mm512_load_si512(l as *const __m512i);
                _mm_store_si128(l.add(f) as *mut __m128i, r);
                let t = f + 16;
                *fill.add(b) = (t & 48) as u8;
                _mm512_store_si512(pend.byte_add(k8 * 8), _mm512_inserti32x4::<3>(z, r));
                *(pend.byte_add(PB + k8) as *mut u64) = b as u64;
                k8 += (t >> 6) * 8;
            }};
        }
        if m == (if n >= 64 { !0 } else { (1u64 << n) - 1 }) {
            for i in 0..n {
                row!(i);
            }
        } else {
            let mut m = m;
            while m != 0 {
                let i = m.trailing_zeros() as usize;
                m &= m - 1;
                row!(i);
            }
        }
        const _: () = assert!(LINK_OFF + 64 == CHUNK);
        let cur = self.cur.as_mut_ptr();
        macro_rules! flush {
            ($store:ident) => {{
                let mut j8 = 0;
                while j8 != k8 {
                    let b = *(pend.byte_add(PB + j8) as *const u64) as usize;
                    let cp = cur.add(b);
                    let mut c = *cp;
                    if (c as usize).wrapping_add(64) & (CHUNK - 1) == 0 {
                        c = self.new_chunk(b, c);
                    }
                    $store(c as *mut __m512i, _mm512_load_si512(pend.byte_add(j8 * 8)));
                    *cp = c.add(8);
                    j8 += 8;
                }
            }};
        }
        if NT {
            flush!(_mm512_stream_si512)
        } else {
            flush!(_mm512_storeu_si512)
        }
    }
    #[inline(always)]
    unsafe fn flush(&mut self, b: usize, l: *mut u64) {
        let cp = self.cur.as_mut_ptr().add(b);
        let mut c = *cp;
        if (c as usize & (CHUNK - 1)) == LINK_OFF {
            c = self.new_chunk(b, c);
        }
        if NT {
            use std::arch::x86_64::*;
            _mm512_stream_si512(c as *mut __m512i, _mm512_load_si512(l as *const __m512i));
        } else {
            std::ptr::copy_nonoverlapping(l, c, 8);
        }
        *cp = c.add(8);
    }
    #[inline(never)]
    unsafe fn new_chunk(&mut self, b: usize, old: *mut u64) -> *mut u64 {
        if self.blk == self.blk_end {
            self.blk = (*self.arena).alloc(BLOCK);
            self.blk_end = self.blk.add(BLOCK);
            if self.popw {
                madvise(self.blk, BLOCK, MADV_POPULATE_WRITE);
            }
        }
        let n = self.blk as *mut u64;
        self.blk = self.blk.add(CHUNK);
        if old as usize == NOCHUNK {
            self.head[b] = n;
        } else {
            *(old as *mut usize) = n as usize;
        }
        self.nch[b] += 1;
        n
    }
    pub fn finish(&mut self) {
        unsafe {
            for b in 0..self.head.len() {
                let f = self.fill[b] as usize / 8;
                if f > 0 {
                    let mut c = self.cur[b];
                    if (c as usize & (CHUNK - 1)) == LINK_OFF {
                        c = self.new_chunk(b, c);
                    }
                    std::ptr::copy_nonoverlapping(self.lines.as_ptr().add(b) as *const u64, c, f);
                    self.cur[b] = c.add(f);
                    self.fill[b] = 0;
                }
                self.n[b] = if self.head[b].is_null() {
                    0
                } else {
                    let cur = self.cur[b] as usize;
                    let last = (cur - 1) & !(CHUNK - 1);
                    (self.nch[b] - 1) * (LINK_OFF as u32 / 16) + ((cur - last) / 16) as u32
                };
            }
            if NT {
                std::arch::x86_64::_mm_sfence();
            }
        }
    }
    #[inline(always)]
    pub fn for_each(&self, b: usize, mut f: impl FnMut(&[Rec])) {
        unsafe {
            if b >= self.head.len() {
                return;
            }
            let mut c = self.head[b];
            if c.is_null() {
                return;
            }
            let cur = self.cur[b] as usize;
            let last = (cur - 1) & !(CHUNK - 1);
            loop {
                if c as usize == last {
                    f(std::slice::from_raw_parts(c as *const Rec, (cur - last) / 16));
                    break;
                }
                let next = *((c as usize + LINK_OFF) as *const usize) as *mut u64;
                if PREFETCH {
                    use std::arch::x86_64::*;
                    _mm_prefetch::<_MM_HINT_T0>(next as *const i8);
                    _mm_prefetch::<_MM_HINT_T0>((next as *const i8).add(64));
                    _mm_prefetch::<_MM_HINT_T0>((next as *const i8).add(LINK_OFF));
                }
                f(std::slice::from_raw_parts(c as *const Rec, LINK_OFF / 16));
                c = next;
            }
        }
    }
    pub fn count(&self, b: usize) -> usize {
        self.n[b] as usize
    }
}

#[inline(always)]
pub(crate) unsafe fn mul8(a: std::arch::x86_64::__m512i, b: std::arch::x86_64::__m512i) -> std::arch::x86_64::__m512i {
    use std::arch::x86_64::*;
    let m = |x: __m256i, y: __m256i| {
        let r: __m256i;
        std::arch::asm!("vpmullq {r}, {x}, {y}", r = lateout(ymm_reg) r, x = in(ymm_reg) x, y = in(ymm_reg) y, options(pure, nomem, nostack, preserves_flags));
        r
    };
    let lo = m(_mm512_castsi512_si256(a), _mm512_castsi512_si256(b));
    let hi = m(_mm512_extracti64x4_epi64::<1>(a), _mm512_extracti64x4_epi64::<1>(b));
    _mm512_inserti64x4::<1>(_mm512_castsi256_si512(lo), hi)
}
#[inline(always)]
unsafe fn mul8m<const D: usize, const D2: usize>(base: *const u8, j: usize, b: std::arch::x86_64::__m512i) -> std::arch::x86_64::__m512i {
    use std::arch::x86_64::*;
    let (lo, hi): (__m256i, __m256i);
    std::arch::asm!(
        "vpmullq {lo}, {y}, ymmword ptr [{a} + 8*{i} + {d}]",
        "vpmullq {hi}, {y}, ymmword ptr [{a} + 8*{i} + {d2}]",
        lo = out(ymm_reg) lo, hi = out(ymm_reg) hi, y = in(ymm_reg) _mm512_castsi512_si256(b), a = in(reg) base, i = in(reg) j, d = const D, d2 = const D2,
        options(pure, readonly, nostack, preserves_flags)
    );
    _mm512_inserti64x4::<1>(_mm512_castsi256_si512(lo), hi)
}
#[inline(always)]
unsafe fn mulu32_8(a: std::arch::x86_64::__m512i, b: std::arch::x86_64::__m512i) -> std::arch::x86_64::__m512i {
    use std::arch::x86_64::*;
    let m = |x: __m256i, y: __m256i| {
        let r: __m256i;
        std::arch::asm!("vpmuludq {r}, {x}, {y}", r = lateout(ymm_reg) r, x = in(ymm_reg) x, y = in(ymm_reg) y, options(pure, nomem, nostack, preserves_flags));
        r
    };
    let lo = m(_mm512_castsi512_si256(a), _mm512_castsi512_si256(b));
    let hi = m(_mm512_extracti64x4_epi64::<1>(a), _mm512_extracti64x4_epi64::<1>(b));
    _mm512_inserti64x4::<1>(_mm512_castsi256_si512(lo), hi)
}

#[repr(C, align(64))]
pub struct Out {
    rec: [[u64; 2]; NB],
    ob: [u64; NB],
    lp: [u64; NB],
    pd: crate::ckix::Pend,
}
impl Out {
    fn new() -> Box<Out> {
        unsafe { Box::new(std::mem::zeroed()) }
    }
}

struct Tabs {
    prod: std::mem::ManuallyDrop<Vec<u8>>,
    camp: std::mem::ManuallyDrop<Vec<u8>>,
    crange: Option<(u64, u64)>,
}
static TABS: std::sync::OnceLock<Tabs> = std::sync::OnceLock::new();
fn make_tabs(gprod: &[u8], gcamp: &[u8]) -> Tabs {
    let mut prod = vec![0u8; PBOUND + 1 + 64];
    prod[..PBOUND + 1].copy_from_slice(&gprod[..PBOUND + 1]);
    let mut camp = vec![255u8; CBOUND + 64];
    camp[..CBOUND].copy_from_slice(&gcamp[..CBOUND]);
    let crange = camp_range(&camp[..CBOUND]);
    Tabs { prod: std::mem::ManuallyDrop::new(prod), camp: std::mem::ManuallyDrop::new(camp), crange }
}
fn tabs(g: &G) -> &'static Tabs {
    TABS.get_or_init(|| make_tabs(&g.prod, &g.camp))
}
pub fn init_tabs(prod: &[u8], camp: &[u8]) {
    let _ = TABS.set(make_tabs(prod, camp));
}
pub unsafe fn init_tabs_shared(prod: *const u8, np: usize, camp: *const u8, nc: usize) {
    if np < PBOUND + 1 + 64 || nc < CBOUND + 64 {
        fallback("lookup table padding");
    }
    let v = |p: *const u8, n: usize| std::mem::ManuallyDrop::new(Vec::from_raw_parts(p as *mut u8, n, n));
    let crange = camp_range(std::slice::from_raw_parts(camp, CBOUND));
    let _ = TABS.set(Tabs { prod: v(prod, np), camp: v(camp, nc), crange });
}
pub static CORE_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn camp_range(t: &[u8]) -> Option<(u64, u64)> {
    let lo = t.iter().position(|&c| c != 255)?;
    let hi = t.iter().rposition(|&c| c != 255)?;
    if t[lo..=hi].iter().all(|&c| c != 255) { Some((lo as u64, (hi - lo) as u64)) } else { None }
}

struct Ctx<'a> {
    g: &'a G,
    t: &'a mut TState,
    eb: &'a mut EvBatch,
    cb: &'a mut CkBatch,
    out: &'a mut Out,
    tabs: &'static Tabs,
    tid: u64,
    base: u64,
    fend: u64,
}
impl<'a> Ctx<'a> {
    #[inline(always)]
    fn token(&mut self, id: u64) -> u64 {
        if id & ID_ODD == 0 {
            return id;
        }
        self.raw_token(self.base + odd_offset(id) as u64, odd_len(id))
    }
    #[cold]
    #[inline(never)]
    fn raw_token(&mut self, off: u64, mut len: usize) -> u64 {
        if len >= 255 {
            let p = self.g.region;
            unsafe {
                while off + (len as u64) < self.fend {
                    let c = *p.add(off as usize + len);
                    if c == b',' || c == b'"' || c == b'\n' {
                        break;
                    }
                    len += 1;
                }
            }
        }
        let idx = self.t.raws.len() as u64;
        if idx >= (1 << 34) {
            fallback("too many raw IDs");
        }
        let at = self.t.rawbuf.len() as u64;
        self.t.rawbuf.extend_from_slice(unsafe { std::slice::from_raw_parts(self.g.region.add(off as usize), len) });
        self.t.raws.push((at, len as u32));
        RAW | self.tid << 34 | idx
    }

    #[inline(never)]
    fn ev_one(&mut self, r: &EvRow) {
        let g = self.g;
        let ing = r.ingest.wrapping_sub(g.ing_base) as u64;
        if r.ingest == TS_BAD || ing >= ING_MAX * 1000 {
            fallback("ingest_ts bad or out of range");
        }
        let ing_s = ing / 1000;
        let rel = r.ts.wrapping_sub(g.ws) as u64;
        let valid = r.uid != ID_EMPTY
            && rel < g.wlen
            && r.kind <= 2
            && r.product != PROD_EMPTY
            && (r.kind != 2 || r.price != 0)
            && r.ingest.wrapping_sub(r.ts) <= DAY_MS;
        let mut w0 = u64::MAX;
        let mut user = 0u64;
        if valid {
            let mut cat = 0u64;
            if r.price != 0 {
                let p = if (r.product as usize) < PBOUND { r.product as usize } else { PBOUND };
                cat = unsafe { *g.prod.get_unchecked(p) } as u64;
            }
            let mut alias = 0u64;
            let mut normal = false;
            if r.uid < (1 << 40) {
                let w = unscramble(r.uid, g.ukey);
                if (w >> 9) < g.ucap && (w & 511) < NALIAS {
                    user = w >> 9;
                    alias = w & 511;
                    normal = true;
                }
            }
            if !normal {
                let tok = self.token(r.uid);
                let idx = self.t.odd_users.len() as u64;
                if idx >= (1 << 22) {
                    fallback("too many odd-user rows");
                }
                self.t.odd_users.push(tok);
                user = ODDU | self.tid << 22 | idx;
            }
            w0 = rel << 33 | (r.price as u64) << 9 | cat << 4 | alias;
        }
        if r.eid < (1 << 40) {
            let v = unscramble(r.eid, g.ekey);
            if v < g.ecap {
                let w1 = (v & (BSIZE as u64 - 1)) | ing_s << 16 | user << 37;
                self.t.ev.push((v >> BB) as usize, w0, w1);
                return;
            }
        }
        let eid = self.token(r.eid);
        self.t.odd_events.push(OddEv { eid, ing: r.ingest, w0, user });
    }

    #[inline(never)]
    fn ev_batch(&mut self) {
        let n = self.eb.n;
        if n == 0 {
            return;
        }
        self.eb.n = 0;
        self.t.n_ev_rows += n as u64;
        if unsafe { NOSINK } {
            for i in 0..n {
                self.t.prof[7] = self.t.prof[7].wrapping_add(self.eb.eid[i] ^ self.eb.uid[i] ^ self.eb.ts[i] as u64 ^ self.eb.ing[i] as u64 ^ self.eb.pp[i]);
            }
            return;
        }
        let g = self.g;
        let p = EvP { ing_base: g.ing_base, ws: g.ws, wlen: g.wlen, ukey: g.ukey, ekey: g.ekey, ucap: g.ucap, ecap: g.ecap, ptab: self.tabs.prod.as_ptr(), lines: self.t.ev.lines_ptr() };
        let (okb, spb, bad) = unsafe { ev_vec(&*self.eb, &mut *self.out, n, &p) };
        if bad {
            fallback("ingest_ts bad or out of range");
        }
        if unsafe { SKIP } & 2048 != 0 {
            self.t.prof[7] = self.t.prof[7].wrapping_add(okb ^ spb ^ self.out.rec[3][0] ^ self.out.rec[5][1] ^ self.out.ob[7]);
            return;
        }
        unsafe { self.t.ev.push_many(okb, n, self.out.ob.as_ptr(), self.out.lp.as_ptr(), self.out.rec.as_ptr()) };
        let mut m = spb;
        while m != 0 {
            let i = m.trailing_zeros() as usize;
            m &= m - 1;
            let r = self.eb.row(i);
            self.ev_one(&r);
        }
    }

    #[inline(never)]
    fn ck_one(&mut self, r: &ClRow) {
        let g = self.g;
        let rel = r.ts.wrapping_sub(g.cbase) as u64;
        if r.uid == ID_EMPTY || rel >= g.clen || r.campaign as usize >= CBOUND {
            return;
        }
        if unsafe { *g.camp.get_unchecked(r.campaign as usize) } == 255 {
            return;
        }
        let key = rel << 32 | (r.campaign as u64) << 4;
        let cid = self.token(r.cid);
        if r.uid < (1 << 40) {
            let w = unscramble(r.uid, g.ukey);
            if (w >> 9) < g.ucap && (w & 511) < NALIAS {
                let u = w >> 9;
                if g.ccap != 0 {
                    if r.cid < (1 << 40) {
                        let c = unscramble(r.cid, g.ckey);
                        if c < g.ccap {
                            if g.cpart {
                                self.t.us.push((u >> UB) as usize, c | (u & (UBSIZE as u64 - 1)) << 43, key | (w & 511));
                            } else {
                                unsafe { crate::ckix::store_one(&mut self.out.pd, g.csr, c, key | (w & 511), u) };
                            }
                            return;
                        }
                    }
                    self.t.odd_clicks.push([r.uid, key, cid]);
                    return;
                }
                self.t.us.push((u >> UB) as usize, cid | (u & (UBSIZE as u64 - 1)) << 43, key | (w & 511));
                return;
            }
        }
        let utok = self.token(r.uid);
        self.t.odd_clicks.push([utok, key, cid]);
    }

    #[inline(never)]
    fn ck_batch(&mut self) {
        let n = self.cb.n;
        if n == 0 {
            return;
        }
        self.cb.n = 0;
        self.t.n_ck_rows += n as u64;
        if unsafe { SKIP } & 64 != 0 {
            for i in 0..n {
                self.t.prof[7] = self.t.prof[7].wrapping_add(self.cb.cid[i] ^ self.cb.uid[i] ^ self.cb.ts[i] as u64 ^ self.cb.camp[i]);
            }
            return;
        }
        let g = self.g;
        if g.ccap != 0 && !g.cpart {
            let p = crate::ckix::CkxP { cbase: g.cbase, clen: g.clen, ukey: g.ukey, ucap: g.ucap, ckey: g.ckey, ccap: g.ccap, ctab: self.tabs.camp.as_ptr(), crange: self.tabs.crange, csr: g.csr };
            let mut m = unsafe { crate::ckix::ck_vec_ix(&*self.cb, &mut self.out.pd, n, &p) };
            while m != 0 {
                let i = m.trailing_zeros() as usize;
                m &= m - 1;
                let r = self.cb.row(i);
                self.ck_one(&r);
            }
            return;
        }
        let p = CkP { cbase: g.cbase, clen: g.clen, ukey: g.ukey, ucap: g.ucap, ckey: g.ckey, ccap: g.ccap, ctab: self.tabs.camp.as_ptr(), crange: self.tabs.crange, lines: self.t.us.lines_ptr() };
        let (okb, spb) = unsafe { ck_vec(&*self.cb, &mut *self.out, n, &p) };
        unsafe { self.t.us.push_many(okb, n, self.out.ob.as_ptr(), self.out.lp.as_ptr(), self.out.rec.as_ptr()) };
        let mut m = spb;
        while m != 0 {
            let i = m.trailing_zeros() as usize;
            m &= m - 1;
            let r = self.cb.row(i);
            self.ck_one(&r);
        }
    }
}

struct EvP {
    ing_base: i64,
    ws: i64,
    wlen: u64,
    ukey: u64,
    ekey: u64,
    ucap: u64,
    ecap: u64,
    ptab: *const u8,
    lines: *const u8,
}

#[inline(always)]
unsafe fn ev_vec(eb: &EvBatch, out: &mut Out, n: usize, p: &EvP) -> (u64, u64, bool) {
    use std::arch::x86_64::*;
    let mut okb = 0u64;
    let mut spb = 0u64;
    let mut pmb = 0u64;
    let mut badb = 0u64;
    let s = |v: u64| _mm512_set1_epi64(v as i64);
    let m40 = s(crate::keyrec::MASK);
    let binv = s(crate::keyrec::BINV);
    let ainv = s(crate::keyrec::AINV);
    let ukey = s(p.ukey & crate::keyrec::MASK);
    let ekey = s(p.ekey & crate::keyrec::MASK);
    let lim40 = s(1 << 40);
    let unscr = |yb: __m512i, key: __m512i| {
        let x = _mm512_and_si512(yb, m40);
        let x = _mm512_xor_si512(x, _mm512_xor_si512(_mm512_srli_epi64::<17>(x), _mm512_srli_epi64::<34>(x)));
        _mm512_and_si512(mul8(_mm512_xor_si512(x, key), ainv), m40)
    };
    let ebp = eb as *const EvBatch as *const u8;
    const UID: usize = std::mem::offset_of!(EvBatch, uid);
    const EID: usize = std::mem::offset_of!(EvBatch, eid);
    let ing_base = s(p.ing_base as u64);
    let ing_lim = s(ING_MAX * 1000);
    let ts_bad = s(TS_BAD as u64);
    let ws = s(p.ws as u64);
    let wlen = s(p.wlen);
    let empty = s(ID_EMPTY);
    let zero = _mm512_setzero_si512();
    let p24 = s(0xFF_FFFF);
    let day = s(DAY_MS as u64);
    let ucap = s(p.ucap);
    let nal = s(NALIAS);
    let ecap = s(p.ecap);
    let m511 = s(511);
    let slotm = s(BSIZE as u64 - 1);
    let magic = s(274_877_907);
    let ones = s(u64::MAX);
    let lines = s(p.lines as u64);
    let ix_lo = _mm512_setr_epi64(0, 8, 1, 9, 2, 10, 3, 11);
    let ix_hi = _mm512_setr_epi64(4, 12, 5, 13, 6, 14, 7, 15);
    let mut j = 0;
    while j < n {
        let ingr = _mm512_load_si512(eb.ing.as_ptr().add(j) as *const __m512i);
        let ing = _mm512_sub_epi64(ingr, ing_base);
        badb |= ((_mm512_cmpeq_epi64_mask(ingr, ts_bad) | _mm512_cmpge_epu64_mask(ing, ing_lim)) as u64) << j;
        let ing_s = _mm512_srli_epi64::<38>(mulu32_8(ing, magic));
        let ts = _mm512_load_si512(eb.ts.as_ptr().add(j) as *const __m512i);
        let rel = _mm512_sub_epi64(ts, ws);
        let eid = _mm512_load_si512(eb.eid.as_ptr().add(j) as *const __m512i);
        let uid = _mm512_load_si512(eb.uid.as_ptr().add(j) as *const __m512i);
        let pp = _mm512_load_si512(eb.pp.as_ptr().add(j) as *const __m512i);
        let price = _mm512_and_si512(_mm512_srli_epi64::<32>(pp), p24);
        let valid = _mm512_cmpneq_epi64_mask(uid, empty)
            & _mm512_cmplt_epu64_mask(rel, wlen)
            & !_mm512_movepi64_mask(pp)
            & _mm512_cmple_epi64_mask(_mm512_sub_epi64(ingr, ts), day);
        let w = unscr(mul8m::<UID, { UID + 32 }>(ebp, j, binv), ukey);
        let user = _mm512_srli_epi64::<9>(w);
        let alias = _mm512_and_si512(w, m511);
        let normal = _mm512_cmplt_epu64_mask(uid, lim40) & _mm512_cmplt_epu64_mask(user, ucap) & _mm512_cmplt_epu64_mask(alias, nal);
        pmb |= ((valid & _mm512_cmpneq_epi64_mask(price, zero)) as u64) << j;
        let w0v = _mm512_or_si512(_mm512_or_si512(_mm512_slli_epi64::<33>(rel), _mm512_slli_epi64::<9>(price)), alias);
        let w0v = _mm512_mask_blend_epi64(valid, ones, w0v);
        let userv = _mm512_maskz_mov_epi64(valid, user);
        let v = unscr(mul8m::<EID, { EID + 32 }>(ebp, j, binv), ekey);
        let evok = _mm512_cmplt_epu64_mask(eid, lim40) & _mm512_cmplt_epu64_mask(v, ecap);
        let w1v = _mm512_or_si512(_mm512_and_si512(v, slotm), _mm512_or_si512(_mm512_slli_epi64::<16>(ing_s), _mm512_slli_epi64::<37>(userv)));
        let ok = evok & !(valid & !normal);
        let bv = _mm512_srli_epi64::<BB>(v);
        _mm512_store_si512(out.rec.as_mut_ptr().add(j) as *mut __m512i, _mm512_permutex2var_epi64(w0v, ix_lo, w1v));
        _mm512_store_si512(out.rec.as_mut_ptr().add(j + 4) as *mut __m512i, _mm512_permutex2var_epi64(w0v, ix_hi, w1v));
        _mm512_store_si512(out.ob.as_mut_ptr().add(j) as *mut __m512i, bv);
        _mm512_store_si512(out.lp.as_mut_ptr().add(j) as *mut __m512i, _mm512_add_epi64(_mm512_slli_epi64::<6>(bv), lines));
        okb |= (ok as u64) << j;
        spb |= ((!ok) as u64) << j;
        j += 8;
    }
    let nm = if n >= 64 { !0 } else { (1u64 << n) - 1 };
    okb &= nm;
    spb &= nm;
    pmb &= nm;
    while pmb != 0 {
        let i = pmb.trailing_zeros() as usize;
        pmb &= pmb - 1;
        let prod = (*eb.pp.get_unchecked(i) as u32 as usize).min(PBOUND);
        out.rec.get_unchecked_mut(i)[0] |= (*p.ptab.add(prod) as u64) << 4;
    }
    (okb, spb, badb & nm != 0)
}

struct CkP {
    cbase: i64,
    clen: u64,
    ukey: u64,
    ucap: u64,
    ckey: u64,
    ccap: u64,
    ctab: *const u8,
    crange: Option<(u64, u64)>,
    lines: *const u8,
}

#[inline(always)]
unsafe fn ck_vec(cb: &CkBatch, out: &mut Out, n: usize, p: &CkP) -> (u64, u64) {
    use std::arch::x86_64::*;
    let mut okb = 0u64;
    let mut spb = 0u64;
    let s = |v: u64| _mm512_set1_epi64(v as i64);
    let m40 = s(crate::keyrec::MASK);
    let binv = s(crate::keyrec::BINV);
    let ainv = s(crate::keyrec::AINV);
    let ukey = s(p.ukey & crate::keyrec::MASK);
    let lim40 = s(1 << 40);
    let cbase = s(p.cbase as u64);
    let clen = s(p.clen);
    let empty = s(ID_EMPTY);
    let cbound = s(CBOUND as u64);
    let mff = s(255);
    let ucap = s(p.ucap);
    let nal = s(NALIAS);
    let m511 = s(511);
    let ulo = s(UBSIZE as u64 - 1);
    let lines = s(p.lines as u64);
    let ranged = p.crange.is_some();
    let (clo, cspan) = (s(p.crange.unwrap_or((0, 0)).0), s(p.crange.unwrap_or((0, 0)).1));
    let ix_lo = _mm512_setr_epi64(0, 8, 1, 9, 2, 10, 3, 11);
    let ix_hi = _mm512_setr_epi64(4, 12, 5, 13, 6, 14, 7, 15);
    let mut j = 0;
    while j < n {
        let ts = _mm512_load_si512(cb.ts.as_ptr().add(j) as *const __m512i);
        let rel = _mm512_sub_epi64(ts, cbase);
        let cid = _mm512_load_si512(cb.cid.as_ptr().add(j) as *const __m512i);
        let uid = _mm512_load_si512(cb.uid.as_ptr().add(j) as *const __m512i);
        let camp = _mm512_load_si512(cb.camp.as_ptr().add(j) as *const __m512i);
        let incsv = if ranged {
            _mm512_cmple_epu64_mask(_mm512_sub_epi64(camp, clo), cspan)
        } else {
            let inb = _mm512_cmplt_epu64_mask(camp, cbound);
            let ch = _mm512_and_si512(_mm512_mask_i64gather_epi64::<1>(mff, inb, camp, p.ctab as *const i64), mff);
            inb & _mm512_cmpneq_epi64_mask(ch, mff)
        };
        let keep = incsv & _mm512_cmpneq_epi64_mask(uid, empty) & _mm512_cmplt_epu64_mask(rel, clen);
        let x = _mm512_and_si512(mul8(uid, binv), m40);
        let x = _mm512_xor_si512(x, _mm512_xor_si512(_mm512_srli_epi64::<17>(x), _mm512_srli_epi64::<34>(x)));
        let w = _mm512_and_si512(mul8(_mm512_xor_si512(x, ukey), ainv), m40);
        let u = _mm512_srli_epi64::<9>(w);
        let alias = _mm512_and_si512(w, m511);
        let normal = _mm512_cmplt_epu64_mask(uid, lim40) & _mm512_cmplt_epu64_mask(u, ucap) & _mm512_cmplt_epu64_mask(alias, nal);
        let mut cidok = _mm512_cmplt_epu64_mask(cid, lim40);
        let mut idw = cid;
        if p.ccap != 0 {
            let y = _mm512_and_si512(mul8(cid, binv), m40);
            let y = _mm512_xor_si512(y, _mm512_xor_si512(_mm512_srli_epi64::<17>(y), _mm512_srli_epi64::<34>(y)));
            idw = _mm512_and_si512(mul8(_mm512_xor_si512(y, s(p.ckey & crate::keyrec::MASK)), ainv), m40);
            cidok &= _mm512_cmplt_epu64_mask(idw, s(p.ccap));
        }
        let key = _mm512_or_si512(_mm512_slli_epi64::<32>(rel), _mm512_slli_epi64::<4>(camp));
        let o0 = _mm512_or_si512(idw, _mm512_slli_epi64::<43>(_mm512_and_si512(u, ulo)));
        let o1 = _mm512_or_si512(key, alias);
        let ok = keep & normal & cidok;
        let bv = _mm512_srli_epi64::<UB>(u);
        _mm512_store_si512(out.rec.as_mut_ptr().add(j) as *mut __m512i, _mm512_permutex2var_epi64(o0, ix_lo, o1));
        _mm512_store_si512(out.rec.as_mut_ptr().add(j + 4) as *mut __m512i, _mm512_permutex2var_epi64(o0, ix_hi, o1));
        _mm512_store_si512(out.ob.as_mut_ptr().add(j) as *mut __m512i, bv);
        _mm512_store_si512(out.lp.as_mut_ptr().add(j) as *mut __m512i, _mm512_add_epi64(_mm512_slli_epi64::<6>(bv), lines));
        okb |= (ok as u64) << j;
        spb |= ((keep & !ok) as u64) << j;
        j += 8;
    }
    let nm = if n >= 64 { !0 } else { (1u64 << n) - 1 };
    (okb & nm, spb & nm)
}

impl<'a> EventSink for Ctx<'a> {
    #[inline(always)]
    fn ev(&mut self) -> *mut EvBatch {
        &mut *self.eb
    }
    #[inline(always)]
    fn ev_flush(&mut self) {
        self.ev_batch();
    }
}

impl<'a> ClickSink for Ctx<'a> {
    #[inline(always)]
    fn ck(&mut self) -> *mut CkBatch {
        &mut *self.cb
    }
    #[inline(always)]
    fn ck_flush(&mut self) {
        self.ck_batch();
    }
}

#[inline(always)]
unsafe fn link_pre(p: *const u8, len: usize) -> Option<(u64, u32, u64, u32)> {
    use std::arch::x86_64::*;
    let v = _mm512_loadu_si512(p as *const __m512i);
    let n = len as u32;
    let cm = _bzhi_u64(_mm512_cmpeq_epi8_mask(v, _mm512_set1_epi8(b',' as i8)), n);
    let qm = _bzhi_u64(_mm512_cmpeq_epi8_mask(v, _mm512_set1_epi8(b'"' as i8)), n);
    let c0 = _tzcnt_u64(cm) as u32;
    let qa = _bzhi_u64(qm, c0);
    let lb = n.wrapping_sub(c0 + 1);
    let qb = qm.wrapping_shr(c0 + 1);
    let fa = (qa != 0) as u32;
    let fb = (qb != 0) as u32;
    let bad_q = ((qa != 0) & (qa != (1 | 1u64.wrapping_shl(c0.wrapping_sub(1))))) | ((qb != 0) & (qb != (1 | 1u64.wrapping_shl(lb.wrapping_sub(1)))));
    let la = c0.wrapping_sub(2 * fa);
    let lv = lb.wrapping_sub(2 * fb);
    let sa = fa as usize;
    let sb = (c0 + 1 + fb) as usize;
    let bad = (n >= 64) | (_popcnt64(cm as i64) != 1) | bad_q | (la.wrapping_sub(2) > 7) | (lv.wrapping_sub(2) > 7);
    if bad {
        return None;
    }
    if (*p.add(sa) != b'u') | (*p.add(sb) != b'u') {
        return None;
    }
    Some(((p.add(sa + 1) as *const u64).read_unaligned(), la - 1, (p.add(sb + 1) as *const u64).read_unaligned(), lv - 1))
}

#[inline(always)]
unsafe fn link_fast(p: *const u8, len: usize) -> Option<(u64, u64)> {
    let (wa, na, wb, nb) = link_pre(p, len)?;
    let (ia, oka) = crate::keyrec::base36_word(wa, na);
    let (ib, okb) = crate::keyrec::base36_word(wb, nb);
    if !(oka & okb) {
        return None;
    }
    Some((ia, ib))
}

#[inline(always)]
unsafe fn b36_vec8(w: std::arch::x86_64::__m512i, n: std::arch::x86_64::__m512i) -> (std::arch::x86_64::__m512i, u8) {
    use std::arch::x86_64::*;
    let s = |v: u64| _mm512_set1_epi64(v as i64);
    let sh = _mm512_slli_epi64::<3>(_mm512_sub_epi64(s(8), n));
    let fill = _mm512_srlv_epi64(s(0x3030_3030_3030_3030 >> 1), _mm512_sub_epi64(s(63), sh));
    let w2 = _mm512_or_si512(_mm512_sllv_epi64(w, sh), fill);
    let d0 = _mm512_sub_epi8(w2, _mm512_set1_epi8(b'0' as i8));
    let isdig = _mm512_cmplt_epu8_mask(d0, _mm512_set1_epi8(10));
    let islow = _mm512_cmplt_epu8_mask(_mm512_sub_epi8(w2, _mm512_set1_epi8(b'a' as i8)), _mm512_set1_epi8(26));
    let badb = _mm512_movm_epi8(!(isdig | islow));
    let good = _mm512_testn_epi64_mask(badb, badb);
    let d = _mm512_mask_sub_epi8(d0, islow, d0, _mm512_set1_epi8(39));
    let (dl, dh) = (_mm512_castsi512_si256(d), _mm512_extracti64x4_epi64::<1>(d));
    let k1 = _mm256_set1_epi16(0x0124);
    let k2 = _mm256_set1_epi32(0x0001_0510);
    let k3 = _mm256_set1_epi64x(1_679_616);
    let (pl, ph): (__m256i, __m256i);
    let (ql, qh): (__m256i, __m256i);
    let (rl, rh): (__m256i, __m256i);
    std::arch::asm!(
        "vpmaddubsw {pl}, {dl}, {k1}",
        "vpmaddubsw {ph}, {dh}, {k1}",
        "vpmaddwd {ql}, {pl}, {k2}",
        "vpmaddwd {qh}, {ph}, {k2}",
        "vpmuludq {rl}, {ql}, {k3}",
        "vpmuludq {rh}, {qh}, {k3}",
        dl = in(ymm_reg) dl, dh = in(ymm_reg) dh, k1 = in(ymm_reg) k1, k2 = in(ymm_reg) k2, k3 = in(ymm_reg) k3,
        pl = out(ymm_reg) pl, ph = out(ymm_reg) ph, ql = out(ymm_reg) ql, qh = out(ymm_reg) qh, rl = out(ymm_reg) rl, rh = out(ymm_reg) rh,
        options(pure, nomem, nostack, preserves_flags)
    );
    let _ = (pl, ph);
    let q = _mm512_inserti64x4::<1>(_mm512_castsi256_si512(ql), qh);
    let r = _mm512_add_epi64(_mm512_inserti64x4::<1>(_mm512_castsi256_si512(rl), rh), _mm512_srli_epi64::<32>(q));
    let canon = _mm512_cmpeq_epi64_mask(n, s(1)) | _mm512_cmpneq_epi64_mask(_mm512_and_si512(w, s(0xFF)), s(0x30));
    (r, good & canon & _mm512_cmple_epu64_mask(r, s(crate::keyrec::MASK)))
}

#[inline(never)]
fn link_slow(c: &mut Ctx, buf: &[u8], s: usize, e: usize) {
    let g = c.g;
    let line = &buf[s..e];
    let Some((a, b)) = crate::setup::split2(line) else { return };
    if a.is_empty() || b.is_empty() {
        return;
    }
    c.t.n_lk_rows += 1;
    if a == b {
        return;
    }
    let oa = a.as_ptr() as usize - buf.as_ptr() as usize;
    let ob = b.as_ptr() as usize - buf.as_ptr() as usize;
    let ia = crate::k_ref::parse_id(buf, (oa, oa + a.len()), b'u');
    let ib = crate::k_ref::parse_id(buf, (ob, ob + b.len()), b'u');
    if ia & ID_ODD == 0 && ib & ID_ODD == 0 {
        if let (Some((ua, aa)), Some((ub, ab))) = (g.normal(ia), g.normal(ib)) {
            if ua == ub {
                let u = ua as u64;
                c.t.us.push((u >> UB) as usize, 1 << 63 | (u & (UBSIZE as u64 - 1)) << 43 | (aa as u64) << 4 | ab as u64, 0);
                return;
            }
        }
    }
    let ta = if ia & ID_ODD == 0 { ia } else { c.raw_token(c.base + oa as u64, a.len()) };
    let tb = if ib & ID_ODD == 0 { ib } else { c.raw_token(c.base + ob as u64, b.len()) };
    c.t.gen_edges.push((ta, tb));
}

fn links_chunk(c: &mut Ctx, buf: &[u8]) {
    use std::arch::x86_64::*;
    const LB: usize = 8192;
    let len = buf.len();
    let base = buf.as_ptr();
    let mut ends = [0u32; LB + 64];
    let mut lb: LkBatch = unsafe { std::mem::zeroed() };
    let mut nb = 0usize;
    let mut pos = 0usize;
    let mut off = 0usize;
    while off < len {
        let stop = (off + LB).min(len);
        let mut k = 0usize;
        let mut o = off;
        while o < stop {
            let mut m = unsafe { _mm512_cmpeq_epi8_mask(_mm512_loadu_si512(base.add(o) as *const __m512i), _mm512_set1_epi8(b'\n' as i8)) };
            if stop - o < 64 {
                m = unsafe { _bzhi_u64(m, (stop - o) as u32) };
            }
            while m != 0 {
                ends[k] = (o + m.trailing_zeros() as usize) as u32;
                k += 1;
                m &= m - 1;
            }
            o += 64;
        }
        for &e in &ends[..k] {
            let e = e as usize;
            if pos + 64 <= len {
                if let Some((wa, na, wb, nbb)) = unsafe { link_pre(base.add(pos), e - pos) } {
                    lb.a[nb] = wa;
                    lb.b[nb] = wb;
                    lb.na[nb] = na as u64;
                    lb.nb[nb] = nbb as u64;
                    lb.at[nb] = (pos as u32, e as u32);
                    pos = e + 1;
                    nb += 1;
                    if nb == NB {
                        lk_batch(c, buf, &mut lb, nb);
                        nb = 0;
                    }
                    continue;
                }
            }
            link_slow(c, buf, pos, e);
            pos = e + 1;
        }
        off = stop;
    }
    lk_batch(c, buf, &mut lb, nb);
    if pos < len {
        link_slow(c, buf, pos, len);
    }
}

#[repr(C, align(64))]
struct LkBatch {
    a: [u64; NB],
    b: [u64; NB],
    na: [u64; NB],
    nb: [u64; NB],
    at: [(u32, u32); NB],
}

#[inline(never)]
fn lk_batch(c: &mut Ctx, buf: &[u8], lb: &mut LkBatch, n: usize) {
    use std::arch::x86_64::*;
    if n == 0 {
        return;
    }
    let g = c.g;
    let out = &mut *c.out;
    let mut okb = 0u64;
    let mut genb = 0u64;
    let mut decb = 0u64;
    unsafe {
        let s = |v: u64| _mm512_set1_epi64(v as i64);
        let m40 = s(crate::keyrec::MASK);
        let binv = s(crate::keyrec::BINV);
        let ainv = s(crate::keyrec::AINV);
        let ukey = s(g.ukey & crate::keyrec::MASK);
        let ucap = s(g.ucap);
        let nal = s(NALIAS);
        let m511 = s(511);
        let ulo = s(UBSIZE as u64 - 1);
        let lines = s(c.t.us.lines_ptr() as u64);
        let link = s(1 << 63);
        let zero = _mm512_setzero_si512();
        let ix_lo = _mm512_setr_epi64(0, 8, 1, 9, 2, 10, 3, 11);
        let ix_hi = _mm512_setr_epi64(4, 12, 5, 13, 6, 14, 7, 15);
        let dec = |y: __m512i| {
            let x = _mm512_and_si512(mul8(y, binv), m40);
            let x = _mm512_xor_si512(x, _mm512_xor_si512(_mm512_srli_epi64::<17>(x), _mm512_srli_epi64::<34>(x)));
            let w = _mm512_and_si512(mul8(_mm512_xor_si512(x, ukey), ainv), m40);
            (_mm512_srli_epi64::<9>(w), _mm512_and_si512(w, m511))
        };
        let mut j = 0;
        while j < n {
            let (ia, oka) = b36_vec8(_mm512_load_si512(lb.a.as_ptr().add(j) as *const __m512i), _mm512_load_si512(lb.na.as_ptr().add(j) as *const __m512i));
            let (ib, okb2) = b36_vec8(_mm512_load_si512(lb.b.as_ptr().add(j) as *const __m512i), _mm512_load_si512(lb.nb.as_ptr().add(j) as *const __m512i));
            let dok = oka & okb2;
            _mm512_store_si512(lb.a.as_mut_ptr().add(j) as *mut __m512i, ia);
            _mm512_store_si512(lb.b.as_mut_ptr().add(j) as *mut __m512i, ib);
            let (ua, aa) = dec(ia);
            let (ub, ab) = dec(ib);
            let normal = _mm512_cmplt_epu64_mask(ua, ucap) & _mm512_cmplt_epu64_mask(aa, nal) & _mm512_cmplt_epu64_mask(ub, ucap) & _mm512_cmplt_epu64_mask(ab, nal);
            let ne = dok & _mm512_cmpneq_epi64_mask(ia, ib);
            let ok = ne & normal & _mm512_cmpeq_epi64_mask(ua, ub);
            decb |= (dok as u64) << j;
            let o0 = _mm512_or_si512(_mm512_or_si512(link, _mm512_slli_epi64::<43>(_mm512_and_si512(ua, ulo))), _mm512_or_si512(_mm512_slli_epi64::<4>(aa), ab));
            let bv = _mm512_srli_epi64::<UB>(ua);
            _mm512_store_si512(out.rec.as_mut_ptr().add(j) as *mut __m512i, _mm512_permutex2var_epi64(o0, ix_lo, zero));
            _mm512_store_si512(out.rec.as_mut_ptr().add(j + 4) as *mut __m512i, _mm512_permutex2var_epi64(o0, ix_hi, zero));
            _mm512_store_si512(out.ob.as_mut_ptr().add(j) as *mut __m512i, bv);
            _mm512_store_si512(out.lp.as_mut_ptr().add(j) as *mut __m512i, _mm512_add_epi64(_mm512_slli_epi64::<6>(bv), lines));
            okb |= (ok as u64) << j;
            genb |= ((ne & !ok) as u64) << j;
            j += 8;
        }
        let nm = if n >= 64 { !0 } else { (1u64 << n) - 1 };
        okb &= nm;
        genb &= nm;
        decb &= nm;
        c.t.us.push_many(okb, n, out.ob.as_ptr(), out.lp.as_ptr(), out.rec.as_ptr());
    }
    c.t.n_lk_rows += decb.count_ones() as u64;
    let mut slow = !decb & (if n >= 64 { !0 } else { (1u64 << n) - 1 });
    while slow != 0 {
        let i = slow.trailing_zeros() as usize;
        slow &= slow - 1;
        link_slow(c, buf, lb.at[i].0 as usize, lb.at[i].1 as usize);
    }
    while genb != 0 {
        let i = genb.trailing_zeros() as usize;
        genb &= genb - 1;
        c.t.gen_edges.push((lb.a[i], lb.b[i]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn b36_vec8_matches_scalar() {
        use std::arch::x86_64::*;
        let mut r = Rng(0x1234_5678_9ABC_DEF1);
        let alpha = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let odd = b"/:`{AZ @\x00\xff_-";
        for it in 0..400_000 {
            let mut w = [0u64; 8];
            let mut n = [0u64; 8];
            for i in 0..8 {
                let len = 1 + r.below(8);
                let mut b = [0u8; 8];
                for x in b.iter_mut() {
                    *x = if r.below(40) == 0 { odd[r.below(odd.len() as u64) as usize] } else { alpha[r.below(36) as usize] };
                }
                if it % 3 == 0 {
                    b[0] = alpha[(10 + r.below(8)) as usize];
                }
                if r.below(6) == 0 {
                    b[0] = b'0';
                }
                w[i] = u64::from_le_bytes(b);
                n[i] = len;
            }
            let (v, ok) = unsafe { b36_vec8(_mm512_loadu_si512(w.as_ptr() as *const __m512i), _mm512_loadu_si512(n.as_ptr() as *const __m512i)) };
            let mut vv = [0u64; 8];
            unsafe { _mm512_storeu_si512(vv.as_mut_ptr() as *mut __m512i, v) };
            for i in 0..8 {
                let (sv, sok) = crate::keyrec::base36_word(w[i], n[i] as u32);
                assert_eq!(sok, ok >> i & 1 == 1, "ok differs: word {:x} len {}", w[i], n[i]);
                if sok {
                    assert_eq!(sv, vv[i], "value differs: word {:x} len {}", w[i], n[i]);
                }
            }
        }
    }

    #[test]
    fn link_fast_matches_scalar() {
        let mut x = 0x1234_5678_9abc_def1u64;
        let mut rnd = |m: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % m
        };
        let alpha = b"0123456789abcdefghijklmnopqrstuvwxyzU\"\", u_\r0";
        let mut fast = 0;
        for _ in 0..3_000_000 {
            let mut line = Vec::new();
            for side in 0..2 {
                if side == 1 {
                    line.push(b',');
                }
                let q = rnd(4) == 0;
                if q {
                    line.push(b'"');
                }
                if rnd(20) != 0 {
                    line.push(b'u');
                }
                let k = rnd(11);
                for _ in 0..k {
                    let c = if rnd(10) == 0 { alpha[rnd(alpha.len() as u64) as usize] } else { alpha[rnd(36) as usize] };
                    line.push(c);
                }
                if q && rnd(10) != 0 {
                    line.push(b'"');
                }
            }
            if rnd(50) == 0 {
                line.push(b',');
            }
            let end = line.len();
            line.push(b'\n');
            line.resize(end + 80, b'x');
            let r = unsafe { link_fast(line.as_ptr(), end) }.map(|(a, b)| (end, a, b));
            let sc = crate::setup::split2(&line[..end]).and_then(|(a, b)| {
                if a.is_empty() || b.is_empty() {
                    return None;
                }
                let oa = a.as_ptr() as usize - line.as_ptr() as usize;
                let ob = b.as_ptr() as usize - line.as_ptr() as usize;
                Some((crate::k_ref::parse_id(&line, (oa, oa + a.len()), b'u'), crate::k_ref::parse_id(&line, (ob, ob + b.len()), b'u')))
            });
            if let Some((n, ia, ib)) = r {
                fast += 1;
                assert_eq!(n, end, "{:?}", String::from_utf8_lossy(&line[..end]));
                assert_eq!(sc, Some((ia, ib)), "{:?}", String::from_utf8_lossy(&line[..end]));
                assert!(ia < (1 << 40) && ib < (1 << 40));
            }
        }
        assert!(fast > 100_000, "{fast}");
    }

    #[test]
    #[ignore]
    fn link_fast_share() {
        let dir = std::env::var("LINKDIR").unwrap();
        let (mut n, mut f) = (0, 0);
        for e in std::fs::read_dir(dir).unwrap() {
            let mut d = std::fs::read(e.unwrap().path()).unwrap();
            let h = d.iter().position(|&b| b == b'\n').unwrap_or(d.len()) + 1;
            let len = d.len();
            d.resize(len + 128, 0);
            let mut pos = h.min(len);
            while pos < len {
                n += 1;
                let e = pos + d[pos..].iter().position(|&b| b == b'\n').unwrap();
                if unsafe { link_fast(d.as_ptr().add(pos), e - pos) }.is_some() {
                    f += 1;
                }
                pos = e + 1;
            }
        }
        eprintln!("lines {n} fast {f}");
        let files: Vec<Vec<u8>> = std::fs::read_dir(std::env::var("LINKDIR").unwrap()).unwrap().map(|e| {
            let mut d = std::fs::read(e.unwrap().path()).unwrap();
            d.resize(d.len() + 128, 0);
            d
        }).collect();
        let mut best = f64::MAX;
        let mut acc = 0u64;
        for _ in 0..20 {
            let t = std::time::Instant::now();
            for d in &files {
                let len = d.len() - 128;
                let mut pos = d.iter().position(|&b| b == b'\n').unwrap_or(len) + 1;
                while pos < len {
                    let e = pos + d[pos..].iter().position(|&b| b == b'\n').unwrap();
                    if let Some((a, b)) = unsafe { link_fast(d.as_ptr().add(pos), e - pos) } {
                        acc = acc.wrapping_add(crate::keyrec::unscramble(a, 0x1234) ^ crate::keyrec::unscramble(b, 0x1234));
                    }
                    pos = e + 1;
                }
            }
            best = best.min(t.elapsed().as_secs_f64());
        }
        eprintln!("fast loop {:.3} ms ({:.1} ns/line) {acc}", best * 1e3, best * 1e9 / n as f64);
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, m: u64) -> u64 {
            self.next() % m
        }
    }

    #[test]
    fn ev_vec_matches_scalar() {
        use crate::keyrec::{scramble, unscramble};
        let mut r = Rng(0x9E37_79B9_7F4A_7C15);
        let mut prod = vec![0u8; PBOUND + 1 + 64];
        for x in prod.iter_mut().take(PBOUND + 1) {
            *x = r.below(31) as u8;
        }
        let mut eb = EvBatch::new();
        let mut out = Out::new();
        let (mut n_ok, mut n_sp, mut n_bad) = (0, 0, 0);
        for round in 0..40_000 {
            let ws = (r.below(1 << 41) as i64) + 8 * DAY_MS;
            let wlen = (1 + r.below(24)) * DAY_MS as u64 - r.below(3);
            let p = EvP {
                ing_base: ws - DAY_MS,
                ws,
                wlen,
                ukey: r.next(),
                ekey: r.next(),
                ucap: 1 + r.below(1 << 26),
                ecap: 1 + r.below(1 << 28),
                ptab: prod.as_ptr(),
                lines: (r.below(1 << 40) << 6) as *const u8,
            };
            let n = 1 + r.below(64) as usize;
            let mut rows = Vec::new();
            for _ in 0..n {
                let eid = match r.below(8) {
                    0 => ID_ODD | r.below(1 << 40),
                    1 => (1 << 40) + r.below(1 << 42),
                    2 => scramble(p.ecap + r.below(1 << 20), p.ekey),
                    3 => r.below(1 << 40),
                    _ => scramble(r.below(p.ecap), p.ekey),
                };
                let uid = match r.below(10) {
                    0 => ID_EMPTY,
                    1 => ID_ODD | r.below(1 << 40),
                    2 => (1 << 40) + r.below(1 << 42),
                    3 => scramble(r.below(p.ucap) * 512 + 14 + r.below(498), p.ukey),
                    4 => scramble((p.ucap + r.below(100)) * 512 + r.below(14), p.ukey),
                    5 => scramble((1 << 39) + r.below(1000), p.ukey),
                    _ => scramble(r.below(p.ucap) * 512 + r.below(14), p.ukey),
                };
                let ts = match r.below(12) {
                    0 => TS_BAD,
                    1 => TS_HUGE,
                    2 => ws - 1 + r.below(3) as i64,
                    3 => ws + wlen as i64 - 2 + r.below(4) as i64,
                    4 => r.next() as i64,
                    _ => ws + r.below(wlen) as i64,
                };
                let base_ts = if ts == TS_BAD || ts == TS_HUGE { ws + r.below(wlen) as i64 } else { ts };
                let ingest = match r.below(400) {
                    0 => TS_BAD,
                    1 => p.ing_base - 1 - r.below(5) as i64,
                    2 => p.ing_base + (ING_MAX * 1000) as i64 + r.below(5) as i64,
                    _ => match r.below(6) {
                        0 => base_ts + DAY_MS - 1 + r.below(3) as i64,
                        1 => base_ts - r.below(100_000) as i64,
                        _ => base_ts + r.below(3_600_000) as i64,
                    },
                };
                let kind = r.below(4) as u8;
                let product = match r.below(10) {
                    0 => PROD_EMPTY,
                    1 => PROD_ODD,
                    2 => (PBOUND as u64 + r.below(1 << 27)) as u32,
                    3 => PBOUND as u32,
                    _ => r.below(PBOUND as u64) as u32,
                };
                let price = if kind == 2 { [0, 1, 10_000_000, r.below(10_000_000) as u32 + 1][r.below(4) as usize] } else { 0 };
                rows.push(EvRow { eid, uid, ts, ingest, kind, product, price });
            }
            for (i, row) in rows.iter().enumerate() {
                eb.eid[i] = row.eid;
                eb.uid[i] = row.uid;
                eb.ts[i] = row.ts;
                eb.ing[i] = row.ingest;
                eb.pp[i] = ev_pp(row.kind, row.product, row.price);
            }
            for i in n..NB {
                eb.ing[i] = TS_BAD;
                eb.eid[i] = r.next();
                eb.uid[i] = r.next();
                eb.pp[i] = r.next();
            }
            let (okb, spb, bad) = unsafe { ev_vec(&eb, &mut out, n, &p) };
            assert_eq!(okb & spb, 0);
            assert_eq!((okb | spb).count_ones() as usize, n, "round {round}");
            let mut any_bad = false;
            for (i, row) in rows.iter().enumerate() {
                let back = eb.row(i);
                let want_prod = if row.kind == 2 || row.product == PROD_EMPTY { row.product } else { PROD_ODD };
                assert_eq!(back, EvRow { product: want_prod, ..*row });
                let ing = row.ingest.wrapping_sub(p.ing_base) as u64;
                if row.ingest == TS_BAD || ing >= ING_MAX * 1000 {
                    any_bad = true;
                    continue;
                }
                let ing_s = ing / 1000;
                let rel = row.ts.wrapping_sub(p.ws) as u64;
                let valid = row.uid != ID_EMPTY && rel < p.wlen && row.kind <= 2 && row.product != PROD_EMPTY && (row.kind != 2 || row.price != 0) && row.ingest.wrapping_sub(row.ts) <= DAY_MS;
                let mut w0 = u64::MAX;
                let mut user = 0u64;
                let mut special = false;
                if valid {
                    let mut cat = 0u64;
                    if row.price != 0 {
                        let q = if (row.product as usize) < PBOUND { row.product as usize } else { PBOUND };
                        cat = prod[q] as u64;
                    }
                    let mut alias = 0u64;
                    let mut normal = false;
                    if row.uid < (1 << 40) {
                        let w = unscramble(row.uid, p.ukey);
                        if (w >> 9) < p.ucap && (w & 511) < NALIAS {
                            user = w >> 9;
                            alias = w & 511;
                            normal = true;
                        }
                    }
                    special |= !normal;
                    w0 = rel << 33 | (row.price as u64) << 9 | cat << 4 | alias;
                }
                let mut pushed = None;
                if row.eid < (1 << 40) {
                    let v = unscramble(row.eid, p.ekey);
                    if v < p.ecap {
                        pushed = Some(((v >> BB), w0, (v & (BSIZE as u64 - 1)) | ing_s << 16 | user << 37));
                    }
                }
                special |= pushed.is_none();
                if special {
                    assert!(spb >> i & 1 == 1, "round {round} row {i}: {row:?}");
                    n_sp += 1;
                } else {
                    assert!(okb >> i & 1 == 1, "round {round} row {i}: {row:?}");
                    assert_eq!(Some((out.ob[i], out.rec[i][0], out.rec[i][1])), pushed, "round {round} row {i}: {row:?}");
                    assert_eq!(out.lp[i], p.lines as u64 + 64 * out.ob[i]);
                    n_ok += 1;
                }
            }
            assert_eq!(bad, any_bad, "round {round}");
            n_bad += any_bad as u32;
        }
        assert!(n_ok > 100_000 && n_sp > 10_000 && n_bad > 1000, "{n_ok} {n_sp} {n_bad}");
    }

    #[test]
    fn ck_vec_matches_scalar() {
        use crate::keyrec::{scramble, unscramble};
        let mut r = Rng(0x2545_F491_4F6C_DD1D);
        let mut tabs = vec![vec![255u8; CBOUND + 64]; 3];
        for x in tabs[0].iter_mut().take(CBOUND) {
            *x = if r.below(4) == 0 { 255 } else { r.below(6) as u8 };
        }
        let ranges = [(0usize, 0usize), (1, 2000), (5000, CBOUND - 1)];
        for t in 1..3 {
            for c in ranges[t].0..=ranges[t].1 {
                tabs[t][c] = r.below(6) as u8;
            }
            assert_eq!(camp_range(&tabs[t][..CBOUND]), Some((ranges[t].0 as u64, (ranges[t].1 - ranges[t].0) as u64)));
        }
        assert_eq!(camp_range(&tabs[0][..CBOUND]), None);
        assert_eq!(camp_range(&vec![255u8; CBOUND]), None);
        let mut cb = CkBatch::new();
        let mut out = Out::new();
        let (mut n_ok, mut n_sp, mut n_drop) = (0, 0, 0);
        for round in 0..40_000 {
            let ws = (r.below(1 << 41) as i64) + 8 * DAY_MS;
            let we = ws + ((1 + r.below(24)) * DAY_MS as u64) as i64;
            let t = round % 3;
            let camp = &tabs[t];
            let p = CkP { cbase: ws - 7 * DAY_MS, clen: (we - ws + 7 * DAY_MS) as u64, ukey: r.next(), ucap: 1 + r.below(1 << 26), ckey: 0, ccap: 0, ctab: camp.as_ptr(), crange: camp_range(&camp[..CBOUND]), lines: (r.below(1 << 40) << 6) as *const u8 };
            let n = 1 + r.below(64) as usize;
            let mut rows = Vec::new();
            for _ in 0..n {
                let cid = match r.below(8) {
                    0 => ID_ODD | r.below(1 << 40),
                    1 => (1 << 40) + r.below(1 << 42),
                    _ => r.below(1 << 40),
                };
                let uid = match r.below(10) {
                    0 => ID_EMPTY,
                    1 => ID_ODD | r.below(1 << 40),
                    2 => (1 << 40) + r.below(1 << 42),
                    3 => scramble(r.below(p.ucap) * 512 + 14 + r.below(498), p.ukey),
                    4 => scramble((p.ucap + r.below(100)) * 512 + r.below(14), p.ukey),
                    _ => scramble(r.below(p.ucap) * 512 + r.below(14), p.ukey),
                };
                let campaign = match r.below(10) {
                    0 => CAMP_EMPTY,
                    1 => CAMP_ODD,
                    2 => (CBOUND as u64 + r.below(1 << 27)) as u32,
                    3 => (ranges[t].0 as u64 + r.below(3)).saturating_sub(1) as u32,
                    4 => (ranges[t].1 as u64 + r.below(3) - 1) as u32,
                    5 => (ranges[t].0 as u64 + r.below((ranges[t].1 - ranges[t].0 + 1) as u64)) as u32,
                    _ => r.below(CBOUND as u64) as u32,
                };
                let ts = match r.below(10) {
                    0 => TS_BAD,
                    1 => TS_HUGE,
                    2 => p.cbase - 1 + r.below(3) as i64,
                    3 => we - 2 + r.below(4) as i64,
                    4 => r.next() as i64,
                    _ => p.cbase + r.below(p.clen) as i64,
                };
                rows.push(ClRow { cid, uid, campaign, ts });
            }
            for (i, row) in rows.iter().enumerate() {
                cb.cid[i] = row.cid;
                cb.uid[i] = row.uid;
                cb.ts[i] = row.ts;
                cb.camp[i] = row.campaign as u64;
            }
            for i in n..NB {
                cb.camp[i] = r.below(CBOUND as u64);
                cb.ts[i] = p.cbase;
                cb.cid[i] = r.below(1 << 40);
                cb.uid[i] = r.below(1 << 40);
            }
            let (okb, spb) = unsafe { ck_vec(&cb, &mut out, n, &p) };
            assert_eq!(okb & spb, 0);
            assert_eq!((okb | spb).checked_shr(n as u32).unwrap_or(0), 0);
            for (i, row) in rows.iter().enumerate() {
                let rel = row.ts.wrapping_sub(p.cbase) as u64;
                let keep = !(row.uid == ID_EMPTY || rel >= p.clen || row.campaign as usize >= CBOUND) && camp[row.campaign as usize] != 255;
                if !keep {
                    assert!((okb | spb) >> i & 1 == 0, "round {round} row {i}");
                    n_drop += 1;
                    continue;
                }
                let key = rel << 32 | (row.campaign as u64) << 4;
                let mut pushed = None;
                if row.cid < (1 << 40) && row.uid < (1 << 40) {
                    let w = unscramble(row.uid, p.ukey);
                    if (w >> 9) < p.ucap && (w & 511) < NALIAS {
                        let u = w >> 9;
                        pushed = Some((u >> UB, row.cid | (u & (UBSIZE as u64 - 1)) << 43, key | (w & 511)));
                    }
                }
                match pushed {
                    Some(x) => {
                        assert!(okb >> i & 1 == 1, "round {round} row {i}");
                        assert_eq!((out.ob[i], out.rec[i][0], out.rec[i][1]), x);
                        assert_eq!(out.lp[i], p.lines as u64 + 64 * out.ob[i]);
                        n_ok += 1;
                    }
                    None => {
                        assert!(spb >> i & 1 == 1, "round {round} row {i}");
                        n_sp += 1;
                    }
                }
            }
        }
        assert!(n_ok > 100_000 && n_sp > 10_000 && n_drop > 10_000, "{n_ok} {n_sp} {n_drop}");
    }

    #[test]
    fn push_many_matches_push() {
        let mut r = Rng(0xDEAD_BEEF_1234_5678);
        for &nb in &[1usize, 2, 5, 37, 3000] {
            let a1 = Arena::new(64 << 20);
            let a2 = Arena::new(64 << 20);
            let mut p1 = Part::new(&a1, nb);
            let mut p2 = Part::new(&a2, nb);
            let mut out = Out::new();
            for _ in 0..3000 {
                let (m, n) = if r.below(3) == 0 {
                    (r.next(), 64)
                } else {
                    let sh = r.below(64);
                    (u64::MAX >> sh, 64 - sh as usize)
                };
                for i in 0..64 {
                    out.ob[i] = r.below(nb as u64);
                    out.lp[i] = p2.lines_ptr() as u64 + 64 * out.ob[i];
                    out.rec[i] = [r.next(), r.next()];
                }
                let mut mm = m;
                while mm != 0 {
                    let i = mm.trailing_zeros() as usize;
                    mm &= mm - 1;
                    p1.push(out.ob[i] as usize, out.rec[i][0], out.rec[i][1]);
                }
                unsafe { p2.push_many(m, n, out.ob.as_ptr(), out.lp.as_ptr(), out.rec.as_ptr()) };
                let b = r.below(nb as u64) as usize;
                let (x, y) = (r.next(), r.next());
                p1.push(b, x, y);
                p2.push(b, x, y);
            }
            p1.finish();
            p2.finish();
            for b in 0..nb {
                assert_eq!(p1.count(b), p2.count(b));
                let mut v1 = Vec::new();
                let mut v2 = Vec::new();
                p1.for_each(b, |c| v1.extend(c.iter().map(|x| (x.w0, x.w1))));
                p2.for_each(b, |c| v2.extend(c.iter().map(|x| (x.w0, x.w1))));
                assert_eq!(v1, v2, "nb {nb} b {b}");
                assert_eq!(v1.len(), p1.count(b));
            }
        }
    }
}

#[inline]
fn next_line_start(data: &[u8], from: usize) -> usize {
    match data[from..].iter().position(|&b| b == b'\n') {
        Some(i) => from + i + 1,
        None => data.len(),
    }
}

struct Drv {
    rd: usize,
    pin: u32,
    qord: u32,
    ch: usize,
    st2q: bool,
}
static DRV: std::sync::OnceLock<Drv> = std::sync::OnceLock::new();
fn drv() -> &'static Drv {
    DRV.get_or_init(|| {
        let n = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(d);
        Drv { rd: n("XL_RD", RD_DEFAULT) << 10, pin: n("XL_PIN", PIN_DEFAULT) as u32, qord: n("XL_QORD", QORD_DEFAULT) as u32, ch: n("XL_CH", CH_DEFAULT) << 10, st2q: n("XL_ST2Q", 1) == 1 }
    })
}
pub fn pin_mode() -> u32 {
    drv().pin
}
const RD_DEFAULT: usize = env_u32(option_env!("XL_RD_DEF"), 0) as usize;
const PIN_DEFAULT: usize = env_u32(option_env!("XL_PIN_DEF"), 2) as usize;
const QORD_DEFAULT: usize = env_u32(option_env!("XL_QORD_DEF"), 2) as usize;
const CH_DEFAULT: usize = env_u32(option_env!("XL_CH_DEF"), 8192) as usize;

pub static mut FDS: Vec<i32> = Vec::new();
#[allow(static_mut_refs)]
pub unsafe fn keep_fd(f: std::fs::File) {
    if drv().rd > 0 {
        if FDS.iter().filter(|&&x| x >= 0).count() < KEEP_FDS {
            FDS.push(std::os::fd::IntoRawFd::into_raw_fd(f));
            return;
        }
    }
    FDS.push(-1);
}
const KEEP_FDS: usize = 40;
#[allow(static_mut_refs)]
pub unsafe fn fds_init(n: usize) {
    FDS = vec![-1; n];
}
#[allow(static_mut_refs)]
pub unsafe fn set_fd(i: usize, f: std::fs::File) {
    if drv().rd > 0 && i < KEEP_FDS {
        *FDS.as_mut_ptr().add(i) = std::os::fd::IntoRawFd::into_raw_fd(f);
    }
}
extern "C" {
    fn pread(fd: i32, buf: *mut u8, n: usize, off: i64) -> isize;
    fn syscall(n: i64, ...) -> i64;
}

unsafe fn read_at(fd: i32, dst: *mut u8, n: usize, off: usize, flen: usize) -> usize {
    let mut got = 0usize;
    while got < n {
        let r = pread(fd, dst.add(got), n - got, (off + got) as i64);
        if r <= 0 {
            if r < 0 || off + got < flen {
                fallback("pread");
            }
            break;
        }
        got += r as usize;
    }
    got
}

pub(crate) fn cpu_order() -> &'static Vec<usize> {
    static O: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();
    O.get_or_init(|| {
        let mut m = [0u64; 16];
        let r = unsafe { syscall(204, 0i32, 128usize, m.as_mut_ptr()) };
        if r <= 0 {
            return Vec::new();
        }
        let allowed: Vec<usize> = (0..1024).filter(|&c| m[c / 64] >> (c % 64) & 1 != 0).collect();
        let mut seen = vec![false; 1024];
        let mut out = Vec::new();
        for &c in &allowed {
            if seen[c] {
                continue;
            }
            let sib = std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list")).unwrap_or_default();
            let mut grp: Vec<usize> = Vec::new();
            for part in sib.trim().split(',') {
                let mut it = part.split('-').filter_map(|x| x.parse::<usize>().ok());
                if let Some(a) = it.next() {
                    let b = it.next().unwrap_or(a);
                    for x in a..=b.min(1023) {
                        grp.push(x);
                    }
                }
            }
            if !grp.contains(&c) {
                grp = vec![c];
            }
            for x in grp {
                if x < 1024 && !seen[x] && allowed.contains(&x) {
                    seen[x] = true;
                    out.push(x);
                }
            }
        }
        out
    })
}

fn work_queue(g: &G) -> &'static (Vec<Chunk>, usize) {
    static Q: std::sync::OnceLock<(Vec<Chunk>, usize)> = std::sync::OnceLock::new();
    Q.get_or_init(|| {
        let v = work_queue_build(g);
        let n = v.iter().rposition(|c| g.files[c.file as usize].kind != Kind::Ev).map_or(0, |i| i + 1);
        (v, n)
    })
}
fn work_queue_build(g: &G) -> Vec<Chunk> {
    {
        let d = drv();
        let qord = d.qord;
        let kind = |f: u32| match (g.files[f as usize].kind, qord) {
            (Kind::Ev, 2) => 3,
            (Kind::Ev, _) => 0,
            (Kind::Ck, _) => 1,
            (Kind::Lk, _) => 2,
        };
        if d.ch == 0 {
            let mut v = g.queue.clone();
            if qord != 0 {
                v.sort_by_key(|c| kind(c.file));
            }
            return v;
        }
        let mut order: Vec<u32> = (0..g.files.len() as u32).collect();
        if qord != 0 {
            order.sort_by_key(|&f| kind(f));
        }
        let mut rem: usize = g.files.iter().map(|f| f.len).sum();
        let mut v = Vec::new();
        for &fi in &order {
            let f = &g.files[fi as usize];
            let mut a = 1;
            while a < f.len {
                let sz = (rem / (4 * g.nt)).clamp(256 << 10, d.ch.max(256 << 10));
                let b = (a + sz).min(f.len);
                v.push(Chunk { file: fi, a, b });
                rem = rem.saturating_sub(b - a);
                a = b;
            }
            rem = rem.saturating_sub(1);
        }
        v
    }
}

#[inline(always)]
fn parse_buf(g: &G, t: &mut TState, eb: &mut EvBatch, cb: &mut CkBatch, out: &mut Out, tb: &'static Tabs, tid: usize, f: &FileInfo, buf: &[u8], s: usize) {
    let mut ctx = Ctx { g, t, eb, cb, out, tabs: tb, tid: tid as u64, base: (f.off + s) as u64, fend: (f.off + f.len) as u64 };
    match f.kind {
        Kind::Ev => {
            if unsafe { SKIP } & 16384 == 0 {
                <K as Kernel>::events(buf, &mut ctx);
            }
            ctx.ev_batch();
        }
        Kind::Ck => {
            if unsafe { SKIP } & 8192 == 0 {
                <K as Kernel>::clicks(buf, &mut ctx);
            }
            ctx.ck_batch();
        }
        Kind::Lk => {
            if unsafe { SKIP } & 1024 == 0 {
                links_chunk(&mut ctx, buf)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn chunk_rd(g: &G, t: &mut TState, eb: &mut EvBatch, cb: &mut CkBatch, out: &mut Out, tb: &'static Tabs, tid: usize, f: &FileInfo, c: &Chunk, fd: i32, rb: *mut u8, rd: usize) {
    let flen = f.len;
    let lim = c.b.min(flen);
    let mut pos = c.a - 1;
    let mut n = read_at(fd, rb, rd, pos, flen);
    let mut o = loop {
        let s = std::slice::from_raw_parts(rb, n);
        if let Some(i) = s.iter().position(|&x| x == b'\n') {
            break i + 1;
        }
        if pos + n >= flen {
            return;
        }
        pos += n;
        n = read_at(fd, rb, rd, pos, flen);
    };
    loop {
        let st = pos + o;
        if st >= lim {
            return;
        }
        let s = std::slice::from_raw_parts(rb, n);
        let eof = pos + n >= flen;
        let li = lim - pos;
        let mut fin = None;
        if li <= n {
            if let Some(j) = s[li - 1..].iter().position(|&x| x == b'\n') {
                fin = Some(li + j);
            }
        }
        let (cut, done) = match fin {
            Some(e) => (e, true),
            None if eof => (n, true),
            None => match s[o..].iter().rposition(|&x| x == b'\n') {
                Some(j) => (o + j + 1, false),
                None => {
                    let data = std::slice::from_raw_parts(g.region.add(f.off), flen);
                    let e = if c.b >= flen { flen } else { next_line_start(data, c.b - 1) };
                    parse_buf(g, t, eb, cb, out, tb, tid, f, &data[st..e], st);
                    return;
                }
            },
        };
        parse_buf(g, t, eb, cb, out, tb, tid, f, &s[o..cut], st);
        if done {
            return;
        }
        pos += cut;
        o = 0;
        n = read_at(fd, rb, rd, pos, flen);
    }
}

pub fn stage1(g: &G, tid: usize) {
    #[cfg(feature = "pmu")]
    let pm = pmu::start();
    #[cfg(feature = "tl")]
    crate::sys::tl::mark(tid, 0);
    let d = drv();
    let mut saved = [0u64; 16];
    let pre = tid >= 1 && tid < 16 && crate::sys::PINNED[tid].load(Ordering::Acquire);
    let pinned = d.pin != 0 && !pre && {
        let o = cpu_order();
        !o.is_empty() && unsafe { syscall(204, 0i32, 128usize, saved.as_mut_ptr()) } > 0 && {
            let cpu = o[tid % o.len()];
            let mut m = [0u64; 16];
            m[cpu / 64] |= 1 << (cpu % 64);
            unsafe { syscall(203, 0i32, 128usize, m.as_ptr()) == 0 }
        }
    };
    #[cfg(feature = "tl")]
    crate::sys::tl::mark(tid, 6);
    let t = g.t(tid);
    t.ev = Part::new(&g.ev_arena, g.nb);
    t.us = Part::new(&g.us_arena, g.nub);
    #[cfg(feature = "tl")]
    crate::sys::tl::mark(tid, 7);
    if !CORE_READY.load(Ordering::Relaxed) {
        t.core = crate::core::Core::new(g.nc, g.nd, g.row_cap, g.ws_mod, g.day0);
    }
    #[cfg(feature = "tl")]
    crate::sys::tl::mark(tid, 8);
    let mut eb = EvBatch::new();
    let mut cb = CkBatch::new();
    let mut out = Out::new();
    let tb = tabs(g);
    #[cfg(feature = "tl")]
    crate::sys::tl::mark(tid, 9);
    let rd = d.rd;
    #[allow(static_mut_refs)]
    let fds: &[i32] = unsafe { &FDS };
    let use_rd = rd > 0 && fds.len() == g.files.len() && unsafe { SKIP } & 512 == 0;
    let rb = if use_rd { unsafe { anon(rd + 8192).add(4096) } } else { std::ptr::null_mut() };
    let (q, nckl) = work_queue(g);
    let nckl = *nckl;
    let inq = d.st2q;
    let mut us_open = true;
    let mut sc = crate::links::Scratch::default();
    #[cfg(feature = "tl")]
    crate::sys::tl::mark(tid, 1);
    #[cfg(feature = "tl")]
    let mut first = true;
    loop {
        if inq && !us_open {
            while crate::links::job(g, &mut *t, &mut sc) {}
        }
        let i = g.qnext.fetch_add(1, Ordering::Relaxed);
        if us_open && i >= nckl {
            us_open = false;
            t.us.finish();
            if g.ccap != 0 {
                unsafe { crate::ckix::flush(&mut out.pd, g.csr) };
            }
            crate::links::us_finished(g, out.pd.end, inq);
            if inq {
                while crate::links::job(g, &mut *t, &mut sc) {}
            }
        }
        if i >= q.len() {
            break;
        }
        #[cfg(feature = "tl")]
        if first {
            crate::sys::tl::mark(tid, 2);
        } else if unsafe { crate::sys::tl::MK[tid][3] } == 0 {
            crate::sys::tl::mark(tid, 3);
        }
        #[cfg(feature = "tl")]
        {
            first = false;
        }
        let c = q[i];
        let f = &g.files[c.file as usize];
        if use_rd && fds[c.file as usize] >= 0 {
            unsafe { chunk_rd(g, &mut *t, &mut eb, &mut cb, &mut out, tb, tid, f, &c, fds[c.file as usize], rb, rd) };
            continue;
        }
        let data = unsafe { std::slice::from_raw_parts(g.region.add(f.off), f.len) };
        let s = if c.a == f.hdr { c.a } else { next_line_start(data, c.a - 1) };
        let e = if c.b >= f.len { f.len } else { next_line_start(data, c.b - 1) };
        if s >= e {
            continue;
        }
        let buf = &data[s..e];
        if unsafe { SKIP } & 512 != 0 {
            let mut x = 0u64;
            let mut i = 0;
            while i < buf.len() {
                x = x.wrapping_add(unsafe { *buf.as_ptr().add(i) } as u64);
                i += 4096;
            }
            t.prof[7] = t.prof[7].wrapping_add(x);
        } else {
            parse_buf(g, &mut *t, &mut eb, &mut cb, &mut out, tb, tid, f, buf, s);
        }
        if unsafe { RELEASE } {
            let a = (f.off + s + 4095) & !4095;
            let b = (f.off + e) & !4095;
            if b > a {
                unsafe { madvise(g.region.add(a) as *mut u8, b - a, MADV_DONTNEED) };
            }
        }
    }
    #[cfg(feature = "tl")]
    crate::sys::tl::mark(tid, 4);
    t.ev.finish();
    if inq {
        loop {
            if crate::links::job(g, &mut *t, &mut sc) {
                continue;
            }
            if crate::links::done(g) {
                break;
            }
            std::hint::spin_loop();
        }
    }
    #[cfg(feature = "tl")]
    crate::sys::tl::mark(tid, 5);
    if !rb.is_null() {
        unsafe { madvise(rb.sub(4096), rd + 8192, MADV_DONTNEED) };
    }
    if pinned && (d.pin == 1 || tid == 0) {
        unsafe { syscall(203, 0i32, 128usize, saved.as_ptr()) };
    }
    #[cfg(feature = "pmu")]
    pmu::stop(pm, g.nt);
}

#[cfg(feature = "pmu")]
pub mod pmu {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    extern "C" {
        fn syscall(n: i64, ...) -> i64;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        fn close(fd: i32) -> i32;
        fn getrusage(who: i32, r: *mut i64) -> i32;
    }
    static RU: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
    const N: usize = 6;
    static SUM: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
    static MAXW: AtomicU64 = AtomicU64::new(0);
    static DONE: AtomicUsize = AtomicUsize::new(0);
    const EV: [(u32, u64, bool); N] = [(0, 1, true), (0, 0, true), (4, 0xC1, true), (1, 2, true), (1, 1, true), (0, 1, false)];
    pub struct Pmu {
        fd: [i32; N],
        t0: std::time::Instant,
    }
    pub fn start() -> Pmu {
        let mut fd = [-1; N];
        for (k, &(ty, cfg, exk)) in EV.iter().enumerate() {
            let mut a = [0u64; 16];
            a[0] = ty as u64 | (128u64 << 32);
            a[1] = cfg;
            a[5] = if exk { (1 << 5) | (1 << 6) } else { 1 << 6 };
            fd[k] = unsafe { syscall(298, a.as_ptr(), 0i32, -1i32, -1i32, 0u64) } as i32;
        }
        Pmu { fd, t0: std::time::Instant::now() }
    }
    pub fn stop(p: Pmu, nt: usize) {
        let w = p.t0.elapsed().as_nanos() as u64;
        let mut ru = [0i64; 18];
        unsafe { getrusage(1, ru.as_mut_ptr()) };
        RU[0].fetch_add((ru[0] * 1_000_000 + ru[1]) as u64, Ordering::Relaxed);
        RU[1].fetch_add((ru[2] * 1_000_000 + ru[3]) as u64, Ordering::Relaxed);
        RU[2].fetch_add(ru[16] as u64, Ordering::Relaxed);
        RU[3].fetch_add(ru[17] as u64, Ordering::Relaxed);
        MAXW.fetch_max(w, Ordering::Relaxed);
        for k in 0..N {
            let mut v = 0u64;
            if p.fd[k] >= 0 {
                unsafe {
                    read(p.fd[k], &mut v as *mut u64 as *mut u8, 8);
                    close(p.fd[k]);
                }
            }
            SUM[k].fetch_add(v, Ordering::Relaxed);
        }
        if DONE.fetch_add(1, Ordering::AcqRel) + 1 == nt {
            let s: Vec<u64> = SUM.iter().map(|a| a.load(Ordering::Relaxed)).collect();
            eprintln!("[pmu] parse: uinstr={:.1}M ucycles={:.1}M uops={:.1}M faults={} task_ms={:.1} allinstr={:.1}M (kinstr={:.1}M) max_thread_ms={:.1}", s[0] as f64 / 1e6, s[1] as f64 / 1e6, s[2] as f64 / 1e6, s[3], s[4] as f64 / 1e6, s[5] as f64 / 1e6, (s[5].saturating_sub(s[0])) as f64 / 1e6, MAXW.load(Ordering::Relaxed) as f64 / 1e6);
            eprintln!("[pmu] thread totals (whole thread life): utime_ms={:.1} stime_ms={:.1} nvcsw={} nivcsw={}", RU[0].load(Ordering::Relaxed) as f64 / 1e3, RU[1].load(Ordering::Relaxed) as f64 / 1e3, RU[2].load(Ordering::Relaxed), RU[3].load(Ordering::Relaxed));
        }
    }
}
