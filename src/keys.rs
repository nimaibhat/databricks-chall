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

/// Same as `pack`; `ceil` = one past the last readable byte after `s`.
/// Codes of up to 10 bytes are looked up independently and combined in
/// parallel instead of a serial shift chain.
#[inline(always)]
pub fn pack_fast(s: &[u8], ceil: *const u8) -> Key {
    let n = s.len();
    let p = s.as_ptr();
    if n <= 10 && (p as usize) + 16 <= ceil as usize {
        let lo = u64::from_le(unsafe { (p as *const u64).read_unaligned() });
        let hi = u64::from_le(unsafe { (p.add(8) as *const u64).read_unaligned() });
        let mut v: u64 = 0;
        let mut bad: u8 = 0;
        let mut i = 0;
        while i < 10 {
            let b = if i < 8 { (lo >> (8 * i)) as u8 } else { (hi >> (8 * (i - 8))) as u8 };
            let c = unsafe { *CODE.get_unchecked(b as usize) };
            let m = if i < n { 0xFFu8 } else { 0 };
            let c = c & m;
            bad |= c;
            v |= ((c & 63) as u64) << (58 - 6 * i);
            i += 1;
        }
        if bad & 0x80 == 0 {
            return v | n as u64;
        }
        return pack_long(s);
    }
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

/// murmur3 fmix64: good high and low bits.
#[inline(always)]
pub fn hk(k: Key) -> u64 {
    let mut x = k;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
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
/// themselves, so a miss means "this ID is its own person". Buckets of four
/// 16-byte slots (one cache line), key 0 = empty (no ID packs to 0 except the
/// empty string, which is never looked up). Inserts are lock-free so the table
/// is filled in parallel; lookups happen only after the build.
pub struct LinkTab {
    base: *mut [u64; 2],
    nb_mask: usize,
    shift: u32,
}
unsafe impl Sync for LinkTab {}
unsafe impl Send for LinkTab {}

impl LinkTab {
    pub fn new(n: usize) -> LinkTab {
        let slots = (n * 2).next_power_of_two().max(64);
        let bytes = slots * 16;
        let base = unsafe {
            let p = libc::mmap(
                std::ptr::null_mut(),
                bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );
            assert!(p != libc::MAP_FAILED, "link table mmap");
            #[cfg(target_os = "linux")]
            libc::madvise(p, bytes, libc::MADV_HUGEPAGE);
            p as *mut [u64; 2]
        };
        let nb = slots / 4;
        LinkTab { base, nb_mask: nb - 1, shift: 64 - nb.trailing_zeros() }
    }
    #[inline(always)]
    pub fn prefetch(&self, h: u64) {
        prefetch(unsafe { self.base.add(((h >> self.shift) as usize) * 4) });
    }
    /// Concurrent insert of a key that is not in the table yet.
    pub fn insert(&self, k: Key, v: Key) {
        use std::sync::atomic::{AtomicU64, Ordering};
        assert!(k != 0);
        let mut b = (hk(k) >> self.shift) as usize;
        loop {
            for i in 0..4 {
                let slot = unsafe { self.base.add(b * 4 + i) };
                let key = unsafe { &*(slot as *const AtomicU64) };
                if key.load(Ordering::Relaxed) == 0 && key.compare_exchange(0, k, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                    unsafe { (*(slot as *const [AtomicU64; 2]))[1].store(v, Ordering::Relaxed) };
                    return;
                }
            }
            b = (b + 1) & self.nb_mask;
        }
    }
    /// The person of `k` if it differs from `k`. `h` = hk(k).
    #[inline(always)]
    pub fn get(&self, k: Key, h: u64) -> Option<Key> {
        let mut b = (h >> self.shift) as usize;
        loop {
            for i in 0..4 {
                let s = unsafe { *self.base.add(b * 4 + i) };
                if s[0] == k {
                    return Some(s[1]);
                }
                if s[0] == 0 {
                    return None;
                }
            }
            b = (b + 1) & self.nb_mask;
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
                assert_eq!(t.get(k, hk(k)), Some(v));
            }
            for _ in 0..20_000 {
                let k = if rnd() % 2 == 0 { 1 + rnd() % 200 } else { rnd() | 1 };
                assert_eq!(t.get(k, hk(k)), m.get(&k).copied(), "n {n} k {k}");
            }
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
