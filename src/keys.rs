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
    fn order() {
        let words: Vec<&[u8]> = vec![
            b"", b"-", b"--", b"-a", b"0", b"09", b"A", b"Z", b"_", b"a", b"a-", b"a0", b"aa", b"ab",
            b"abcdefghij", b"abcdefghijk", b"u1", b" u1", b"u1 ", b"zzzzzzzzzz", b"\x01", b"\xff",
        ];
        for &x in &words {
            for &y in &words {
                let (a, b) = (pack(x), pack(y));
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

/// A lookup table for small catalogs: raw keys for IDs of at most 7 bytes,
/// packed keys for the rest.
pub struct Catalog<V: Copy + Default> {
    pub raw: KMap<V>,
    pub long: KMap<V>,
}
impl<V: Copy + Default> Catalog<V> {
    pub fn new(n: usize) -> Self {
        Catalog { raw: KMap::with_capacity(n + 16), long: KMap::with_capacity(16) }
    }
    /// Inserts if absent; returns true if inserted.
    pub fn insert(&mut self, s: &[u8], v: V, overwrite: bool) -> bool {
        let (m, k) = match raw7(s, s.as_ptr()) {
            Some(k) => (&mut self.raw, k),
            None => (&mut self.long, pack(s)),
        };
        let (i, ins) = m.entry(k, v);
        if !ins && overwrite {
            m.slots[i].1 = v;
        }
        ins
    }
}
