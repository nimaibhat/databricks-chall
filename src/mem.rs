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
    pub fn new(cap: usize) -> Arena {
        let cap = (cap + BLK - 1) / BLK * BLK;
        #[cfg(unix)]
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
        Arena { base: std::ptr::null_mut(), cap: 0, off: AtomicUsize::new(0) }
    }

    pub fn used(&self) -> usize {
        self.off.load(Ordering::Relaxed)
    }
    /// A fresh block (never handed out before).
    #[inline]
    pub fn block(&self) -> *mut u8 {
        let o = self.off.fetch_add(BLK, Ordering::Relaxed);
        if o + BLK <= self.cap {
            unsafe { self.base.add(o) }
        } else {
            // arena exhausted: fall back to the heap (leaked, like the arena)
            let layout = std::alloc::Layout::from_size_align(BLK, 64).unwrap();
            unsafe { std::alloc::alloc(layout) }
        }
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
