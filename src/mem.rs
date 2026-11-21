//! Memory: one huge-page anonymous arena carved into fixed 32 KiB blocks, and
//! append-only block lists for the radix-partition buffers. Blocks are
//! recycled through per-thread free lists, so buffers that are consumed in one
//! phase become the output buffers of the next without touching fresh pages.
use std::sync::atomic::{AtomicUsize, Ordering};

pub const BLK: usize = 32 * 1024;

pub struct Arena {
    base: *mut u8,
    cap: usize,
    off: AtomicUsize,
}
unsafe impl Sync for Arena {}
unsafe impl Send for Arena {}

impl Arena {
    /// Reserves `cap` bytes of address space (untouched pages cost nothing).
    /// Every block comes from this one region, so a record is addressable by
    /// a 32-bit index from `base`. Falls back to smaller reservations if the
    /// kernel refuses the large one.
    pub fn new(cap: usize, min_cap: usize) -> Arena {
        const ALIGN: usize = 2 << 20; // huge-page aligned, so every block is BLK-aligned too
        let mut cap = cap.div_ceil(BLK) * BLK;
        loop {
            unsafe {
                let p = libc::mmap(
                    std::ptr::null_mut(),
                    cap + ALIGN,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | map_noreserve(),
                    -1,
                    0,
                );
                if p != libc::MAP_FAILED {
                    let base = ((p as usize + ALIGN - 1) & !(ALIGN - 1)) as *mut u8;
                    #[cfg(target_os = "linux")]
                    libc::madvise(base as *mut _, cap, libc::MADV_HUGEPAGE);
                    return Arena { base, cap, off: AtomicUsize::new(0) };
                }
            }
            assert!(cap > min_cap, "arena reservation failed");
            cap = (cap / 2).max(min_cap).div_ceil(BLK) * BLK;
        }
    }
    #[inline(always)]
    pub fn base(&self) -> usize {
        self.base as usize
    }
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Frees the used part of the arena with `threads` parallel madvise calls.
    pub fn release_parallel(&self, threads: usize) {
        #[cfg(target_os = "linux")]
        {
            let used = self.used().min(self.cap);
            let per = (used / threads.max(1) + (2 << 20)) & !((2 << 20) - 1);
            let base = self.base as usize;
            std::thread::scope(|s| {
                for t in 0..threads {
                    let a = t * per;
                    if a >= used {
                        break;
                    }
                    let b = (a + per).min(used);
                    s.spawn(move || unsafe {
                        libc::madvise((base + a) as *mut _, b - a, libc::MADV_DONTNEED);
                    });
                }
            });
        }
        let _ = threads;
    }
    /// Faults in (and so zeroes) the next not yet populated piece of the
    /// first `limit` bytes; false when there is nothing left to do or the
    /// kernel cannot do it. Meant for threads with nothing else to do: the
    /// pages are the ones the block allocator hands out first, so the
    /// workers that fill them later do not stop to have them zeroed.
    pub fn populate_step(&self, next: &AtomicUsize, limit: usize) -> bool {
        #[cfg(target_os = "linux")]
        {
            const STEP: usize = 8 << 20;
            let at = next.fetch_add(STEP, Ordering::Relaxed);
            if at >= limit.min(self.cap) {
                return false;
            }
            let len = STEP.min(limit.min(self.cap) - at);
            // 23 = MADV_POPULATE_WRITE (Linux 5.14); an older kernel says EINVAL
            unsafe { libc::madvise(self.base.add(at) as *mut _, len, 23) == 0 }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (next, limit);
            false
        }
    }
    pub fn used(&self) -> usize {
        self.off.load(Ordering::Relaxed)
    }
    /// `bytes` of fresh, zeroed, 64-byte aligned memory that is never
    /// handed out again (rounded up to whole blocks).
    pub fn alloc(&self, bytes: usize) -> *mut u8 {
        let len = bytes.div_ceil(BLK).max(1) * BLK;
        let o = self.off.fetch_add(len, Ordering::Relaxed);
        assert!(o + len <= self.cap, "arena exhausted");
        unsafe { self.base.add(o) }
    }
    /// A fresh block (never handed out before).
    #[inline]
    pub fn block(&self) -> *mut u8 {
        let o = self.off.fetch_add(BLK, Ordering::Relaxed);
        assert!(o + BLK <= self.cap, "arena exhausted");
        unsafe { self.base.add(o) }
    }
}

#[cfg(target_os = "linux")]
fn map_noreserve() -> i32 {
    libc::MAP_NORESERVE
}
#[cfg(all(unix, not(target_os = "linux")))]
fn map_noreserve() -> i32 {
    0
}

/// Per-thread source of blocks: recycled ones first, then fresh arena blocks.
pub struct Blocks<'a> {
    pub arena: &'a Arena,
    pub free: Vec<*mut u8>,
}
unsafe impl Send for Blocks<'_> {}
impl<'a> Blocks<'a> {
    pub fn new(arena: &'a Arena) -> Self {
        Blocks { arena, free: Vec::new() }
    }
    #[inline]
    pub fn get(&mut self) -> *mut u8 {
        match self.free.pop() {
            Some(b) => b,
            None => self.arena.block(),
        }
    }
}

/// Append-only list of T in 32 KiB blocks.
pub struct BList<T: Copy> {
    pub blocks: Vec<*mut T>,
    cur: *mut T,
    left: usize,
    pub len: usize,
}
unsafe impl<T: Copy + Send> Send for BList<T> {}
unsafe impl<T: Copy + Sync> Sync for BList<T> {}

impl<T: Copy> Default for BList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy> BList<T> {
    pub const PER: usize = BLK / std::mem::size_of::<T>();
    pub fn new() -> Self {
        BList { blocks: Vec::new(), cur: std::ptr::null_mut(), left: 0, len: 0 }
    }
    /// The filled parts of each block.
    #[inline]
    pub fn for_each_slice(&self, mut f: impl FnMut(&[T])) {
        let n = self.blocks.len();
        for (i, &b) in self.blocks.iter().enumerate() {
            let l = if i + 1 == n { Self::PER - self.left } else { Self::PER };
            f(unsafe { std::slice::from_raw_parts(b, l) });
        }
    }
    /// Gives the blocks back for reuse (contents become garbage).
    pub fn release(&mut self, free: &mut Vec<*mut u8>) {
        for &b in &self.blocks {
            free.push(b as *mut u8);
        }
        self.blocks.clear();
        self.cur = std::ptr::null_mut();
        self.left = 0;
        self.len = 0;
    }
}

/// Bytes staged per partition before they are written out: two cache lines.
pub const STG: usize = 128;

/// A radix scatter into per-partition block lists.
///
/// The only per-partition state on the hot path is `cur`, the final address
/// of the next record. On x86-64 records are first collected in a small
/// cache-resident stage per partition and each full stage is written to its
/// block with non-temporal stores, so the destination lines are never read
/// into the cache (software write-combining). Elsewhere records are written
/// directly. `finish` writes out the partial stages and returns the lists.
pub struct Scatter<T: Copy> {
    /// per partition: where its next record goes; in the arena, so on huge
    /// pages like the stages (both are hit at random by every push)
    cur: *mut *mut T,
    parts: usize,
    #[allow(dead_code)]
    stage: *mut u8,
    blocks: Vec<Vec<*mut T>>,
}
unsafe impl<T: Copy + Send> Send for Scatter<T> {}

impl<T: Copy> Scatter<T> {
    const SZ: usize = std::mem::size_of::<T>();
    pub fn new(parts: usize, arena: &Arena) -> Self {
        assert!(STG % Self::SZ == 0 && BLK % STG == 0);
        // fresh arena memory is zero: every cursor starts as null
        let cur = arena.alloc(parts * std::mem::size_of::<*mut T>()) as *mut *mut T;
        let stage = arena.alloc(parts * STG);
        Scatter { cur, parts, stage, blocks: (0..parts).map(|_| Vec::new()).collect() }
    }
    #[cold]
    #[inline(never)]
    fn new_block(&mut self, p: usize, src: &mut Blocks) -> *mut T {
        let b = src.get() as *mut T;
        self.blocks[p].push(b);
        b
    }
    /// Appends `x` to partition `p`; returns the address the record will
    /// have once written out (valid for reads only after `finish`).
    #[inline(always)]
    pub fn push(&mut self, p: usize, x: T, src: &mut Blocks) -> *mut T {
        unsafe {
            let mut at = *self.cur.add(p);
            if (at as usize) & (BLK - 1) == 0 {
                // no block yet, or the current one is full
                at = self.new_block(p, src);
            }
            #[cfg(target_arch = "x86_64")]
            {
                let off = (at as usize) & (STG - 1);
                let st = self.stage.add(p * STG);
                (st.add(off) as *mut T).write(x);
                let next = at.add(1);
                *self.cur.add(p) = next;
                if (next as usize) & (STG - 1) == 0 {
                    use std::arch::x86_64::*;
                    let dst = (next as *mut u8).sub(STG);
                    let mut i = 0;
                    while i < STG {
                        _mm_stream_si128(dst.add(i) as *mut __m128i, _mm_load_si128(st.add(i) as *const __m128i));
                        _mm_stream_si128(dst.add(i + 16) as *mut __m128i, _mm_load_si128(st.add(i + 16) as *const __m128i));
                        i += 32;
                    }
                }
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                at.write(x);
                *self.cur.add(p) = at.add(1);
            }
            at
        }
    }
    /// Makes the non-temporal stores of this thread visible to others.
    #[inline(always)]
    pub fn fence() {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            std::arch::x86_64::_mm_sfence()
        };
    }
    /// Writes out the partial stages and returns one list per partition.
    pub fn finish(mut self) -> Vec<BList<T>> {
        let parts = self.parts;
        let mut out = Vec::with_capacity(parts);
        for p in 0..parts {
            let cur = unsafe { *self.cur.add(p) };
            let blocks = std::mem::take(&mut self.blocks[p]);
            if blocks.is_empty() {
                out.push(BList::new());
                continue;
            }
            #[cfg(target_arch = "x86_64")]
            unsafe {
                let off = (cur as usize) & (STG - 1);
                std::ptr::copy_nonoverlapping(self.stage.add(p * STG), (cur as *mut u8).sub(off), off);
            }
            let last = *blocks.last().unwrap();
            let used = (cur as usize - last as usize) / Self::SZ;
            let len = (blocks.len() - 1) * BList::<T>::PER + used;
            out.push(BList { blocks, cur, left: BList::<T>::PER - used, len });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Scatter against a plain Vec per partition: same records, same order,
    /// and every returned address holds its record after `finish`.
    #[test]
    fn scatter_matches_vecs() {
        let arena = Arena::new(64 << 20, 64 << 20);
        let mut x: u64 = 0xABCDEF;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for &(parts, n) in &[(1usize, 0usize), (1, 1), (4, 7), (8, 5000), (64, 300_000), (512, 100_000), (3, 70_000)] {
            let mut bs = Blocks::new(&arena);
            let mut s16 = Scatter::<[u64; 2]>::new(parts, &arena);
            let mut s32 = Scatter::<[u64; 4]>::new(parts, &arena);
            let mut want16: Vec<Vec<[u64; 2]>> = vec![Vec::new(); parts];
            let mut want32: Vec<Vec<[u64; 4]>> = vec![Vec::new(); parts];
            let mut addrs = Vec::new();
            for i in 0..n {
                // skewed partition choice so some partitions fill many blocks
                let p = if rnd() % 3 == 0 { 0 } else { (rnd() % parts as u64) as usize };
                let a = [rnd(), i as u64];
                let at = s16.push(p, a, &mut bs);
                addrs.push((at, a));
                want16[p].push(a);
                let q = (rnd() % parts as u64) as usize;
                let b = [rnd(), rnd(), i as u64, 7];
                s32.push(q, b, &mut bs);
                want32[q].push(b);
            }
            Scatter::<[u64; 2]>::fence();
            let l16 = s16.finish();
            let l32 = s32.finish();
            for p in 0..parts {
                let mut got = Vec::new();
                l16[p].for_each_slice(|sl| got.extend_from_slice(sl));
                assert_eq!(got, want16[p]);
                assert_eq!(l16[p].len, want16[p].len());
                let mut got = Vec::new();
                l32[p].for_each_slice(|sl| got.extend_from_slice(sl));
                assert_eq!(got, want32[p]);
                assert_eq!(l32[p].len, want32[p].len());
            }
            for (at, a) in addrs {
                assert_eq!(unsafe { *at }, a);
            }
        }
    }
}
