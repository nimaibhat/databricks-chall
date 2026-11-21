//! Exact u64 keys for short strings, hashing, an open-addressing map, prefetch.
//!
//! A string of at most 10 bytes over the 64-symbol alphabet
//! `-0-9A-Z_a-z` (ASCII order) is packed as 6-bit codes from the top bit down,
//! zero padded, with its length in the low 4 bits. For such strings u64 order
//! equals byte-wise string order (code 0 is `-`, the smallest symbol; a shorter
//! prefix ties on codes and loses on length). Every other string is interned
//! in a global table and gets the key `(index << 4) | 15`; comparisons that
//! involve one fall back to comparing bytes.
use std::cmp::Ordering as Cmp;
use std::collections::HashMap;
use std::sync::Mutex;

pub type Key = u64;
pub const EMPTY: Key = u64::MAX;

const ALPHA: &[u8; 64] = b"-0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ_abcdefghijklmnopqrstuvwxyz";
static CODE: [u8; 256] = {
    let mut t = [0xFFu8; 256];
    let mut i = 0;
    while i < 64 {
        t[ALPHA[i] as usize] = i as u8;
        i += 1;
    }
    t
};

struct LongTab {
    map: HashMap<Vec<u8>, u64>,
    strs: Vec<Vec<u8>>,
}
static LONG: Mutex<Option<LongTab>> = Mutex::new(None);

#[cold]
#[inline(never)]
fn pack_long(s: &[u8]) -> Key {
    let mut g = LONG.lock().unwrap();
    let t = g.get_or_insert_with(|| LongTab { map: HashMap::new(), strs: Vec::new() });
    let idx = match t.map.get(s) {
        Some(&i) => i,
        None => {
            let i = t.strs.len() as u64;
            t.strs.push(s.to_vec());
            t.map.insert(s.to_vec(), i);
            i
        }
    };
    (idx << 4) | 15
}

#[inline(always)]
pub fn pack(s: &[u8]) -> Key {
    let n = s.len();
    if n <= 10 {
        let mut v: u64 = 0;
        let mut bad: u8 = 0;
        for &b in s {
            let c = unsafe { *CODE.get_unchecked(b as usize) };
            bad |= c;
            v = (v << 6) | (c & 63) as u64;
        }
        if bad & 0x80 == 0 {
            return ((v << (60 - 6 * n as u32)) << 4) | n as u64;
        }
    }
    pack_long(s)
}

/// Low-nibble and high-nibble classes for the key alphabet: a byte is in
/// `-0-9A-Z_a-z` iff `LO_CLASS[b & 15] & HI_CLASS[b >> 4] != 0`. One bit per
/// row of the ASCII table: 0x2_ (only `-`), 0x3_ (digits), 0x4_ (A-O),
/// 0x5_ (P-Z and `_`), 0x6_ (a-o), 0x7_ (p-z).
static LO_CLASS: [u8; 16] = [0x2A, 0x3E, 0x3E, 0x3E, 0x3E, 0x3E, 0x3E, 0x3E, 0x3E, 0x3E, 0x3C, 0x14, 0x14, 0x15, 0x14, 0x1C];
static HI_CLASS: [u8; 16] = [0, 0, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0, 0, 0, 0, 0, 0, 0, 0];
/// Added to a byte to get its 6-bit code, by high nibble (`_` needs 4 less).
static HI_OFFSET: [u8; 16] = [0, 0, 211, 209, 202, 202, 197, 197, 0, 0, 0, 0, 0, 0, 0, 0];
/// `KEEP[16 - n..]` = n bytes of 0xFF then zeros.
static KEEP: [u8; 32] = [
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// The 6-bit codes of the first `n` (<= 10) bytes at `p` as two little-endian
/// words (bytes past `n` are code 0), or None if a byte is outside the
/// alphabet. Reads 16 bytes at `p`.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn codes16(p: *const u8, n: usize) -> Option<(u64, u64)> {
    use std::arch::x86_64::*;
    let keep = _mm_loadu_si128(KEEP.as_ptr().add(16 - n) as *const __m128i);
    let dash = _mm_set1_epi8(b'-' as i8);
    // bytes past the ID become '-', which is valid and has code 0
    let v = _mm_or_si128(_mm_and_si128(_mm_loadu_si128(p as *const __m128i), keep), _mm_andnot_si128(keep, dash));
    let nib = _mm_set1_epi8(0x0F);
    let lo = _mm_and_si128(v, nib);
    let hi = _mm_and_si128(_mm_srli_epi16(v, 4), nib);
    let cls = _mm_and_si128(
        _mm_shuffle_epi8(_mm_loadu_si128(LO_CLASS.as_ptr() as *const __m128i), lo),
        _mm_shuffle_epi8(_mm_loadu_si128(HI_CLASS.as_ptr() as *const __m128i), hi),
    );
    if _mm_movemask_epi8(_mm_cmpeq_epi8(cls, _mm_setzero_si128())) != 0 {
        return None;
    }
    let off = _mm_shuffle_epi8(_mm_loadu_si128(HI_OFFSET.as_ptr() as *const __m128i), hi);
    let und = _mm_and_si128(_mm_cmpeq_epi8(v, _mm_set1_epi8(b'_' as i8)), _mm_set1_epi8(-4));
    let c = _mm_add_epi8(_mm_add_epi8(v, off), und);
    Some((_mm_cvtsi128_si64(c) as u64, _mm_extract_epi64(c, 1) as u64))
}
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn codes16(p: *const u8, n: usize) -> Option<(u64, u64)> {
    use std::arch::aarch64::*;
    let keep = vld1q_u8(KEEP.as_ptr().add(16 - n));
    let v = vbslq_u8(keep, vld1q_u8(p), vdupq_n_u8(b'-'));
    let lo = vandq_u8(v, vdupq_n_u8(0x0F));
    let hi = vshrq_n_u8(v, 4);
    let cls = vandq_u8(vqtbl1q_u8(vld1q_u8(LO_CLASS.as_ptr()), lo), vqtbl1q_u8(vld1q_u8(HI_CLASS.as_ptr()), hi));
    if vminvq_u8(cls) == 0 {
        return None;
    }
    let off = vqtbl1q_u8(vld1q_u8(HI_OFFSET.as_ptr()), hi);
    let und = vandq_u8(vceqq_u8(v, vdupq_n_u8(b'_')), vdupq_n_u8(252));
    let c = vreinterpretq_u64_u8(vaddq_u8(vaddq_u8(v, off), und));
    Some((vgetq_lane_u64(c, 0), vgetq_lane_u64(c, 1)))
}

/// Same as `pack`; `ceil` = one past the last readable byte after `s`.
/// The bytes are classified and turned into 6-bit codes 16 at a time, then
/// the codes are squeezed together with shifts.
#[inline(always)]
pub fn pack_fast(s: &[u8], ceil: *const u8) -> Key {
    let n = s.len();
    let p = s.as_ptr();
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    if n <= 10 && (p as usize) + 16 <= ceil as usize {
        let Some((lo, hi)) = (unsafe { codes16(p, n) }) else { return pack_long(s) };
        // first byte to the top, then 8 -> 6 bits per code: one bit-extract
        // where the CPU has it, three shift-merges elsewhere
        let x = lo.swap_bytes();
        #[cfg(all(target_arch = "x86_64", target_feature = "bmi2"))]
        let x = unsafe { std::arch::x86_64::_pext_u64(x, 0x3F3F_3F3F_3F3F_3F3F) };
        #[cfg(not(all(target_arch = "x86_64", target_feature = "bmi2")))]
        let x = {
            let x = ((x & 0x3F00_3F00_3F00_3F00) >> 2) | (x & 0x003F_003F_003F_003F);
            let x = ((x & 0x0FFF_0000_0FFF_0000) >> 4) | (x & 0x0000_0FFF_0000_0FFF);
            ((x & 0x00FF_FFFF_0000_0000) >> 8) | (x & 0x0000_0000_00FF_FFFF)
        };
        return (x << 16) | ((hi & 0x3F) << 10) | ((hi & 0x3F00) >> 4) | n as u64;
    }
    let _ = ceil;
    pack(s)
}

#[inline(always)]
pub fn is_long(k: Key) -> bool {
    (k & 15) == 15
}

pub fn key_bytes(k: Key, out: &mut Vec<u8>) {
    if is_long(k) {
        let g = LONG.lock().unwrap();
        out.extend_from_slice(&g.as_ref().unwrap().strs[(k >> 4) as usize]);
    } else {
        let n = (k & 15) as u32;
        for i in 0..n {
            out.push(ALPHA[((k >> (58 - 6 * i)) & 63) as usize]);
        }
    }
}

#[inline(always)]
pub fn key_cmp(a: Key, b: Key) -> Cmp {
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

/// Fold, one multiply, fold. Packed IDs keep their characters in the high
/// bits, and a multiply only carries information upward, so the first fold
/// brings the high half down before the multiply spreads it; the second fold
/// makes the low bits (open-addressing slots) as good as the top ones
/// (partitions, table buckets). A hash only steers where things go, never
/// what the answer is.
#[inline(always)]
pub fn hk(k: Key) -> u64 {
    let x = (k ^ (k >> 32)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^ (x >> 32)
}

#[inline(always)]
pub fn prefetch<T>(p: *const T) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::x86_64::_mm_prefetch(p as *const i8, std::arch::x86_64::_MM_HINT_T0);
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!("prfm pldl1keep, [{0}]", in(reg) p, options(nostack, readonly, preserves_flags));
    }
}

/// Global user -> person table. Holds only IDs whose person differs from
/// themselves, so a miss means "this ID is its own person". A bucket is one
/// cache line: four keys, then their four people; key 0 = empty (no ID packs
/// to 0 except the empty string, which is never looked up). A lookup compares
/// the four keys at once. Inserts are lock-free so the table is filled in
/// parallel; lookups happen only after the build.
pub struct LinkTab {
    base: *mut [u64; 8],
    nb_mask: usize,
    shift: u32,
}
unsafe impl Sync for LinkTab {}
unsafe impl Send for LinkTab {}

impl LinkTab {
    pub fn new(n: usize) -> LinkTab {
        let slots = (n * 2).next_power_of_two().max(64);
        let bytes = slots * 16;
        const ALIGN: usize = 2 << 20;
        let base = unsafe {
            let p = libc::mmap(
                std::ptr::null_mut(),
                bytes + ALIGN,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );
            assert!(p != libc::MAP_FAILED, "link table mmap");
            let b = (p as usize + ALIGN - 1) & !(ALIGN - 1);
            #[cfg(target_os = "linux")]
            libc::madvise(b as *mut _, bytes, libc::MADV_HUGEPAGE);
            b as *mut [u64; 8]
        };
        let nb = slots / 4;
        LinkTab { base, nb_mask: nb - 1, shift: 64 - nb.trailing_zeros() }
    }
    #[inline(always)]
    pub fn prefetch(&self, h: u64) {
        prefetch(unsafe { self.base.add((h >> self.shift) as usize) });
    }
    /// Concurrent insert of a key that is not in the table yet.
    pub fn insert(&self, k: Key, v: Key) {
        use std::sync::atomic::{AtomicU64, Ordering};
        assert!(k != 0);
        let mut b = (hk(k) >> self.shift) as usize;
        loop {
            let line = unsafe { &*(self.base.add(b) as *const [AtomicU64; 8]) };
            for i in 0..4 {
                if line[i].load(Ordering::Relaxed) == 0 && line[i].compare_exchange(0, k, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                    line[4 + i].store(v, Ordering::Relaxed);
                    return;
                }
            }
            b = (b + 1) & self.nb_mask;
        }
    }
    /// The person of `k` (which is `k` itself when it is not in the table).
    /// `h` = hk(k).
    #[inline(always)]
    pub fn person(&self, k: Key, h: u64) -> Key {
        let mut b = (h >> self.shift) as usize;
        loop {
            let line = unsafe { &*self.base.add(b) };
            #[cfg(target_arch = "x86_64")]
            let (hit, free) = unsafe {
                use std::arch::x86_64::*;
                let keys = _mm256_loadu_si256(line.as_ptr() as *const __m256i);
                let hit = _mm256_movemask_pd(_mm256_castsi256_pd(_mm256_cmpeq_epi64(keys, _mm256_set1_epi64x(k as i64)))) as u32;
                let free = _mm256_movemask_pd(_mm256_castsi256_pd(_mm256_cmpeq_epi64(keys, _mm256_setzero_si256()))) as u32;
                (hit, free)
            };
            #[cfg(not(target_arch = "x86_64"))]
            let (hit, free) = {
                let (mut hit, mut free) = (0u32, 0u32);
                for i in 0..4 {
                    hit |= ((line[i] == k) as u32) << i;
                    free |= ((line[i] == 0) as u32) << i;
                }
                (hit, free)
            };
            // which of the four it is (slot 0 when none: the value is then unused)
            let v = unsafe { *line.get_unchecked(4 + (hit.trailing_zeros() & 3) as usize) };
            if (hit | free) != 0 {
                // hits and misses alternate unpredictably: select, do not branch
                return std::hint::select_unpredictable(hit != 0, v, k);
            }
            b = (b + 1) & self.nb_mask; // a full line without the key: rare
        }
    }
}

/// Open addressing, linear probing, EMPTY marks a free slot. The backing
/// storage can be larger than the active capacity so one map is reused.
pub struct KMap<V: Copy> {
    pub slots: Vec<(Key, V)>,
    mask: usize,
    pub len: usize,
}

impl<V: Copy + Default> KMap<V> {
    pub fn with_capacity(n: usize) -> Self {
        let cap = (n * 2).next_power_of_two().max(16);
        KMap { slots: vec![(EMPTY, V::default()); cap], mask: cap - 1, len: 0 }
    }
    #[inline(always)]
    pub fn cap(&self) -> usize {
        self.mask + 1
    }
    /// Empties the map and sizes it for about n keys, reusing storage.
    pub fn reset(&mut self, n: usize) {
        let cap = (n * 2).next_power_of_two().max(16);
        if self.slots.len() < cap {
            self.slots = vec![(EMPTY, V::default()); cap];
        } else {
            for s in &mut self.slots[..cap] {
                s.0 = EMPTY;
            }
        }
        self.mask = cap - 1;
        self.len = 0;
    }
    /// Occupied slots of the active capacity.
    #[inline]
    pub fn active(&self) -> &[(Key, V)] {
        &self.slots[..self.cap()]
    }
    #[inline]
    pub fn active_mut(&mut self) -> &mut [(Key, V)] {
        let c = self.cap();
        &mut self.slots[..c]
    }
    #[inline(always)]
    pub fn slot_ptr(&self, h: u64) -> *const (Key, V) {
        unsafe { self.slots.as_ptr().add(h as usize & self.mask) }
    }
    #[inline(always)]
    pub fn get_h(&self, k: Key, h: u64) -> Option<V> {
        let mut i = h as usize & self.mask;
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
    #[inline(always)]
    pub fn get(&self, k: Key) -> Option<V> {
        self.get_h(k, hk(k))
    }
    /// (slot index, inserted?)
    #[inline(always)]
    pub fn entry_h(&mut self, k: Key, h: u64, v: V) -> (usize, bool) {
        if (self.len + 1) * 2 > self.cap() {
            self.grow();
        }
        let mut i = h as usize & self.mask;
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
    #[inline(always)]
    pub fn entry(&mut self, k: Key, v: V) -> (usize, bool) {
        self.entry_h(k, hk(k), v)
    }
    #[inline(always)]
    pub fn val_mut(&mut self, i: usize) -> &mut V {
        unsafe { &mut self.slots.get_unchecked_mut(i).1 }
    }
    fn grow(&mut self) {
        let old: Vec<(Key, V)> = self.active().iter().copied().filter(|s| s.0 != EMPTY).collect();
        let cap = self.cap() * 2;
        if self.slots.len() < cap {
            self.slots = vec![(EMPTY, V::default()); cap];
        } else {
            for s in &mut self.slots[..cap] {
                s.0 = EMPTY;
            }
        }
        self.mask = cap - 1;
        for (k, v) in old {
            let mut i = hk(k) as usize & self.mask;
            while self.slots[i].0 != EMPTY {
                i = (i + 1) & self.mask;
            }
            self.slots[i] = (k, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The hash must spread generator-shaped IDs (a letter plus base-36 of a
    /// 40-bit number) evenly over partitions (top bits) and table slots (low bits).
    #[test]
    fn hash_spreads_ids() {
        let mut x: u64 = 0x1234_5678_9ABC_DEF1;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let b36 = |mut v: u64| -> Vec<u8> {
            let mut o = Vec::new();
            loop {
                o.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(v % 36) as usize]);
                v /= 36;
                if v == 0 {
                    break;
                }
            }
            o.reverse();
            o
        };
        for (prefix, sequential) in [(b'u', false), (b'e', false), (b'k', true)] {
            let n = 400_000usize;
            let (mut top, mut low, mut mid) = (vec![0u32; 512], vec![0u32; 512], vec![0u32; 512]);
            for i in 0..n {
                let v = if sequential { i as u64 } else { rnd() & ((1 << 40) - 1) };
                let mut id = vec![prefix];
                id.extend_from_slice(&b36(v));
                let h = hk(pack(&id));
                top[(h >> 55) as usize] += 1;
                low[(h & 511) as usize] += 1;
                mid[((h << 9) >> 55) as usize] += 1;
            }
            let mean = (n / 512) as u32;
            for part in [&top, &low, &mid] {
                let (mn, mx) = (*part.iter().min().unwrap(), *part.iter().max().unwrap());
                assert!(mn > mean * 7 / 10 && mx < mean * 13 / 10, "prefix {} seq {sequential}: min {mn} max {mx} mean {mean}", prefix as char);
            }
        }
    }
    #[test]
    fn linktab_matches_hashmap() {
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for &n in &[0usize, 1, 3, 31, 32, 33, 1000, 50_000] {
            let mut m: HashMap<u64, u64> = HashMap::new();
            while m.len() < n {
                // clustered keys too, to force bucket overflow
                let k = if rnd() % 4 == 0 { 1 + rnd() % 64 } else { rnd() | 1 };
                let v = rnd();
                m.entry(k).or_insert(v);
            }
            let t = LinkTab::new(n);
            let items: Vec<(u64, u64)> = m.iter().map(|(a, b)| (*a, *b)).collect();
            std::thread::scope(|s| {
                for part in items.chunks(items.len() / 4 + 1) {
                    let t = &t;
                    s.spawn(move || {
                        for &(k, v) in part {
                            t.insert(k, v);
                        }
                    });
                }
            });
            for (&k, &v) in &m {
                assert_eq!(t.person(k, hk(k)), v);
            }
            for _ in 0..20_000 {
                let k = if rnd() % 2 == 0 { 1 + rnd() % 200 } else { rnd() | 1 };
                assert_eq!(t.person(k, hk(k)), m.get(&k).copied().unwrap_or(k), "n {n} k {k}");
            }
        }
    }
    /// `pack_fast` against `pack`: every length 0..=11 with every byte value
    /// at every position, IDs at every distance from the buffer end, and
    /// random IDs over the alphabet and over all bytes.
    #[test]
    fn pack_fast_matches_pack() {
        let fast = |id: &[u8], slack: usize| -> Key {
            let mut buf = vec![0xEEu8; 7];
            buf.extend_from_slice(id);
            buf.extend(std::iter::repeat(b'z').take(slack));
            pack_fast(&buf[7..7 + id.len()], unsafe { buf.as_ptr().add(buf.len()) })
        };
        let mut hits = 0u32;
        for n in 0..=11usize {
            let base: Vec<u8> = (0..n).map(|i| ALPHA[(i * 7 + 3) % 64]).collect();
            for slack in [0usize, 1, 5, 6, 15, 16, 17, 40] {
                assert_eq!(fast(&base, slack), pack(&base), "{:?} slack {slack}", base);
            }
            for pos in 0..n {
                for b in 0..=255u8 {
                    let mut id = base.clone();
                    id[pos] = b;
                    let (f, r) = (fast(&id, 20), pack(&id));
                    assert_eq!(f, r, "{:?}", id);
                    hits += !is_long(f) as u32;
                }
            }
        }
        assert!(hits > 3000);
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for i in 0..2_000_000u32 {
            let n = (rnd() % 12) as usize;
            let id: Vec<u8> = (0..n).map(|_| if i % 5 == 0 { rnd() as u8 } else { ALPHA[(rnd() % 64) as usize] }).collect();
            let (f, r) = (fast(&id, (rnd() % 24) as usize), pack(&id));
            assert_eq!(f, r, "{:?}", id);
            let mut o = Vec::new();
            key_bytes(f, &mut o);
            assert_eq!(o, id);
        }
    }
    #[test]
    fn order() {
        let words: Vec<&[u8]> = vec![
            b"", b"-", b"--", b"-a", b"0", b"09", b"A", b"Z", b"_", b"a", b"a-", b"a0", b"aa", b"ab",
            b"abcdefghij", b"abcdefghijk", b"u1", b" u1", b"u1 ", b"zzzzzzzzzz", b"\x01", b"\xff",
        ];
        for &x in &words {
            for &y in &words {
                let (a, b) = (pack(x), pack(y));
                let mut buf = x.to_vec();
                buf.extend_from_slice(&[b'\xff'; 16]);
                assert_eq!(pack_fast(&buf[..x.len()], unsafe { buf.as_ptr().add(buf.len()) }), a);
                assert_eq!(key_cmp(a, b), x.cmp(y), "{:?} {:?}", x, y);
                assert_eq!(a == b, x == y);
                let mut o = Vec::new();
                key_bytes(a, &mut o);
                assert_eq!(o, x);
            }
        }
    }
}

/// Exact key for strings of at most 7 bytes: bytes little-endian, length in
/// the top byte. Reads the 8 bytes ending at the end of `s`, so the caller
/// guarantees that `s.as_ptr() + len - 8 >= floor` (bytes before `s` are readable).
#[inline(always)]
pub fn raw7(s: &[u8], floor: *const u8) -> Option<u64> {
    let n = s.len();
    if n > 7 {
        return None;
    }
    let endp = unsafe { s.as_ptr().add(n) };
    let v = if (endp as usize) >= (floor as usize) + 8 {
        let w = unsafe { (endp.sub(8) as *const u64).read_unaligned() }.to_le();
        if n == 0 {
            0
        } else {
            w >> (8 * (8 - n))
        }
    } else {
        let mut b = [0u8; 8];
        b[..n].copy_from_slice(s);
        u64::from_le_bytes(b)
    };
    Some(v | ((n as u64) << 56))
}

/// A lookup table for small catalogs (values u32): a dense array for IDs of
/// the canonical form `<prefix><decimal without leading zeros>` (the most
/// common prefix in the catalog), raw keys for other IDs of at most 7 bytes,
/// packed keys for the rest. Exact: every catalog ID of canonical form with a
/// number below `dense.len()` is in `dense`, so a miss there is a real miss.
pub struct Catalog {
    pub raw: KMap<u32>,
    pub long: KMap<u32>,
    ids: Vec<Vec<u8>>,
    prefix: Vec<u8>,
    dense: Vec<u32>, // value + 1, 0 = absent
    pl: usize,  // prefix length; usize::MAX when there is no dense table
    pval: u64,  // prefix bytes as a little-endian word
    pmask: u64, // mask of the prefix bytes
}

/// (prefix length, number) if `s` is non-digits followed by 1..=8 digits
/// without a leading zero.
fn canon_split(s: &[u8]) -> Option<(usize, u64)> {
    let p = s.iter().position(|b| b.is_ascii_digit())?;
    let d = &s[p..];
    if d.is_empty() || d.len() > 8 || !d.iter().all(|b| b.is_ascii_digit()) || (d[0] == b'0' && d.len() > 1) {
        return None;
    }
    Some((p, d.iter().fold(0u64, |n, &b| n * 10 + (b - b'0') as u64)))
}

impl Catalog {
    pub fn new(n: usize) -> Self {
        Catalog {
            raw: KMap::with_capacity(n + 16),
            long: KMap::with_capacity(16),
            ids: Vec::new(),
            prefix: Vec::new(),
            dense: Vec::new(),
            pl: usize::MAX,
            pval: 0,
            pmask: 0,
        }
    }
    /// Inserts if absent; returns true if inserted.
    pub fn insert(&mut self, s: &[u8], v: u32, overwrite: bool) -> bool {
        let (m, k) = match raw7(s, s.as_ptr()) {
            Some(k) => (&mut self.raw, k),
            None => (&mut self.long, pack(s)),
        };
        let (i, ins) = m.entry(k, v);
        if !ins && overwrite {
            m.slots[i].1 = v;
        }
        if ins {
            self.ids.push(s.to_vec());
        }
        ins
    }
    fn map_get(&self, s: &[u8], floor: *const u8) -> Option<u32> {
        match raw7(s, floor) {
            Some(k) => self.raw.get(k),
            None => self.long.get(pack(s)),
        }
    }
    /// Builds the dense table; call after the last insert.
    pub fn finish(&mut self) {
        let mut cnt: HashMap<Vec<u8>, (usize, u64)> = HashMap::new();
        for id in &self.ids {
            if let Some((p, n)) = canon_split(id) {
                let e = cnt.entry(id[..p].to_vec()).or_insert((0, 0));
                e.0 += 1;
                e.1 = e.1.max(n);
            }
        }
        let Some((prefix, (c, maxn))) = cnt.into_iter().max_by_key(|(k, v)| (v.0, std::cmp::Reverse(k.clone()))) else {
            return;
        };
        if maxn as usize > 4 * c + 4096 {
            return; // too sparse
        }
        let mut dense = vec![0u32; maxn as usize + 1];
        for id in &self.ids {
            if let Some((p, n)) = canon_split(id) {
                if id[..p] == prefix[..] {
                    dense[n as usize] = self.map_get(id, id.as_ptr()).unwrap() + 1;
                }
            }
        }
        if prefix.len() <= 7 {
            let mut b = [0u8; 8];
            b[..prefix.len()].copy_from_slice(&prefix);
            self.pl = prefix.len();
            self.pval = u64::from_le_bytes(b);
            self.pmask = (1u64 << (8 * prefix.len())) - 1;
            self.dense = dense;
        }
        self.prefix = prefix;
    }
    /// The value for `s`; bytes from `floor` up to `s` are readable.
    #[inline(always)]
    pub fn get(&self, s: &[u8], floor: *const u8) -> Option<u32> {
        let pl = self.pl;
        let n = s.len();
        let e = unsafe { s.as_ptr().add(n) };
        // dense path: prefix + 1..=8 digits, at most 8 bytes in all, compared
        // and converted from the single word that ends at the field end
        if n > pl && n <= 8 && e as usize >= floor as usize + 8 {
            let x = crate::parse::ld8(unsafe { e.sub(8) });
            let k = n - pl; // 1..=8 digits
            let pre = (x >> (8 * (8 - n))) & self.pmask;
            let first = (x >> (8 * (8 - k))) as u8;
            let keep = 8 - k;
            let dg = if keep == 0 { x } else { (x & (!0u64 << (8 * keep))) | (0x3030_3030_3030_3030 >> (8 * k)) };
            if pre == self.pval && crate::parse::digits8(dg) && (first != b'0' || k == 1) {
                let v = crate::parse::val8(dg) as usize;
                if v < self.dense.len() {
                    let d = unsafe { *self.dense.get_unchecked(v) };
                    return d.checked_sub(1);
                }
            }
        }
        self.map_get(s, floor)
    }
}

#[cfg(test)]
mod cat_tests {
    use super::*;
    #[test]
    fn dense_exact() {
        let mut c = Catalog::new(100);
        for i in 0..200u32 {
            c.insert(format!("p{i}").as_bytes(), i * 3, true);
        }
        c.insert(b"p0007", 999, true);
        c.insert(b"x5", 555, true);
        c.insert(b"averyveryverylongid", 777, true);
        c.finish();
        assert!(!c.dense.is_empty());
        let probes: Vec<&[u8]> = vec![
            b"p0", b"p7", b"p07", b"p0007", b"p199", b"p200", b"p1999", b"P7", b"p", b"", b"x5", b"p7 ",
            b" p7", b"p-7", b"p12345678", b"p123456789", b"averyveryverylongid", b"pp7", b"p1a", b"p00",
        ];
        for pr in probes {
            let mut buf = vec![b'#'; 16];
            buf.extend_from_slice(pr);
            buf.extend_from_slice(b",,,,,,,,");
            let s = &buf[16..16 + pr.len()];
            let exp = c.map_get(pr, pr.as_ptr());
            assert_eq!(c.get(s, buf.as_ptr()), exp, "{:?}", std::str::from_utf8(pr));
        }
        assert_eq!(c.get(b"p7", b"p7".as_ptr()), Some(21));
    }
}
