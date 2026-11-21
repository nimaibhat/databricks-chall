//! Memory: one huge-page anonymous arena carved into fixed 64 KiB blocks, and
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
        let mut cap = (cap + BLK - 1) / BLK * BLK;
        loop {
            unsafe {
                let p = libc::mmap(
                    std::ptr::null_mut(),
                    cap,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | map_noreserve(),
                    -1,
                    0,
                );
                if p != libc::MAP_FAILED {
                    #[cfg(target_os = "linux")]
                    libc::madvise(p, cap, libc::MADV_HUGEPAGE);
                    return Arena { base: p as *mut u8, cap, off: AtomicUsize::new(0) };
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
    pub fn used(&self) -> usize {
        self.off.load(Ordering::Relaxed)
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

/// Append-only list of T in 64 KiB blocks.
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
    #[inline(always)]
    pub fn push(&mut self, x: T, src: &mut Blocks) {
        if self.left == 0 {
            let b = src.get() as *mut T;
            self.blocks.push(b);
            self.cur = b;
            self.left = Self::PER;
        }
        unsafe {
            self.cur.write(x);
            self.cur = self.cur.add(1);
        }
        self.left -= 1;
        self.len += 1;
    }
    /// Appends and returns where the record now lives (stable until release).
    #[inline(always)]
    pub fn push_at(&mut self, x: T, src: &mut Blocks) -> *mut T {
        if self.left == 0 {
            let b = src.get() as *mut T;
            self.blocks.push(b);
            self.cur = b;
            self.left = Self::PER;
        }
        let at = self.cur;
        unsafe {
            at.write(x);
            self.cur = at.add(1);
        }
        self.left -= 1;
        self.len += 1;
        at
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
