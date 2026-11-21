//! CSV dialect, timestamps, prices, file mapping and chunking.
use std::fs::File;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// CSV dialect
// ---------------------------------------------------------------------------
/// Splits a line into exactly N fields. Quotes must wrap a whole field.
#[inline(always)]
pub fn split<'a, const N: usize>(line: &'a [u8], out: &mut [&'a [u8]; N]) -> bool {
    let len = line.len();
    if len == 0 {
        return false;
    }
    let mut f = 0usize;
    let mut start = 0usize;
    loop {
        let mut end = start;
        let mut quotes = 0u32;
        while end < len {
            let c = unsafe { *line.get_unchecked(end) };
            if c == b',' {
                break;
            }
            quotes += (c == b'"') as u32;
            end += 1;
        }
        if f >= N {
            return false;
        }
        let field = unsafe { line.get_unchecked(start..end) };
        out[f] = if quotes == 0 {
            field
        } else if quotes == 2 && field.len() >= 2 && field[0] == b'"' && field[field.len() - 1] == b'"' {
            &field[1..field.len() - 1]
        } else {
            return false;
        };
        f += 1;
        if end == len {
            break;
        }
        start = end + 1;
    }
    f == N
}

pub fn for_each_line<'a>(data: &'a [u8], mut f: impl FnMut(&'a [u8])) {
    let mut start = 0;
    for nl in memchr::memchr_iter(b'\n', data) {
        if nl > start {
            f(&data[start..nl]);
        }
        start = nl + 1;
    }
    if start < data.len() {
        f(&data[start..]);
    }
}

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------
#[inline(always)]
fn d2(s: &[u8], i: usize) -> u32 {
    let a = s[i].wrapping_sub(b'0') as u32;
    let b = s[i + 1].wrapping_sub(b'0') as u32;
    if a < 10 && b < 10 {
        a * 10 + b
    } else {
        1000
    }
}

#[inline(always)]
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[inline(always)]
fn civil_secs(s: &[u8]) -> Option<i64> {
    // s has at least 19 bytes: YYYY-MM-DDTHH:MM:SS
    let y1 = d2(s, 0);
    let y2 = d2(s, 2);
    let mo = d2(s, 5);
    let da = d2(s, 8);
    let h = d2(s, 11);
    let mi = d2(s, 14);
    let se = d2(s, 17);
    if s[4] != b'-' || s[7] != b'-' || s[10] != b'T' || s[13] != b':' || s[16] != b':' {
        return None;
    }
    if y1 > 99 || y2 > 99 || mo == 0 || mo > 12 || da == 0 || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    let y = (y1 * 100 + y2) as i64;
    let dim = match mo {
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if da > dim {
        return None;
    }
    let days = days_from_civil(y, mo as i64, da as i64);
    Some(days * 86400 + (h * 3600 + mi * 60 + se) as i64)
}

/// Parses any valid `ts` form to epoch ms. None = bad (incl. F3 overflow,
/// which is necessarily outside every window).
#[inline(always)]
pub fn parse_ts(s: &[u8]) -> Option<i64> {
    let n = s.len();
    if n >= 19 && s[4] == b'-' {
        if n == 20 {
            if s[19] != b'Z' {
                return None;
            }
            return civil_secs(s).map(|v| v * 1000);
        }
        if n == 25 {
            let sign = s[19];
            if (sign != b'+' && sign != b'-') || s[22] != b':' {
                return None;
            }
            let oh = d2(s, 20);
            let om = d2(s, 23);
            if oh > 23 || om > 59 {
                return None;
            }
            let off = (oh * 60 + om) as i64 * 60;
            let base = civil_secs(s)?;
            return Some(if sign == b'+' { base - off } else { base + off } * 1000);
        }
        return None;
    }
    parse_f3(s)
}

#[inline(always)]
fn parse_f3(s: &[u8]) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    let mut v: u64 = 0;
    let mut sig = 0u32;
    for &c in s {
        let d = c.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        if sig > 0 || d != 0 {
            sig += 1;
            if sig > 19 {
                return None;
            }
            v = v * 10 + d as u64;
        }
    }
    if v > i64::MAX as u64 {
        None
    } else {
        Some(v as i64)
    }
}

#[inline(always)]
pub fn parse_price(s: &[u8]) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut v: u64 = 0;
    for &c in s {
        let d = c.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        v = v * 10 + d as u64;
        if v > 10_000_000 {
            return None;
        }
    }
    if v == 0 {
        None
    } else {
        Some(v as u32)
    }
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------
pub fn list_csv(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".csv") && !name.starts_with('.') {
                let p = e.path();
                if p.is_file() {
                    v.push(p);
                }
            }
        }
    }
    v.sort();
    v
}

pub struct Mapped {
    pub maps: Vec<memmap2::Mmap>,
    pub files: Vec<File>,
}

/// I/O strategy, from env IOMODE: 0 none, 1 whole-file WILLNEED at open,
/// 2 per-chunk WILLNEED, 3 per-chunk MADV_POPULATE_READ, 4 per-chunk pread
/// of a mapped file, 5 (default) pread only: events and clicks are never mapped.
pub fn io_mode() -> u32 {
    static M: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *M.get_or_init(|| std::env::var("IOMODE").ok().and_then(|v| v.parse().ok()).unwrap_or(5))
}

/// Makes chunk bytes available. Returns (data, start, end) to scan.
pub fn chunk_data<'a>(m: &'a Mapped, file: usize, start: usize, end: usize, buf: &'a mut Vec<u8>) -> (&'a [u8], usize, usize) {
    let map: &[u8] = &m.maps[file];
    let mode = io_mode();
    #[cfg(target_os = "linux")]
    {
        if mode == 2 || mode == 3 {
            let a = start & !4095;
            let advice = if mode == 2 { libc::MADV_WILLNEED } else { 22 /* MADV_POPULATE_READ */ };
            unsafe {
                libc::madvise(map.as_ptr().add(a) as *mut _, end - a, advice);
            }
        }
        if mode == 4 {
            use std::os::unix::fs::FileExt;
            // keep 8 bytes of the previous line so raw7 loads stay in the buffer
            let pre = start.min(8);
            let n = end - start + pre;
            buf.clear();
            buf.reserve(n);
            unsafe { buf.set_len(n) };
            let mut got = 0;
            while got < n {
                let r = m.files[file].read_at(&mut buf[got..], (start - pre + got) as u64).expect("pread");
                if r == 0 {
                    break;
                }
                got += r;
            }
            return (&buf[..], pre, n);
        }
    }
    let _ = (mode, buf);
    (map, start, end)
}
/// After a chunk is parsed: drop its page-table entries now, in parallel,
/// instead of in the single-threaded teardown at exit. Re-touching a dropped
/// page just maps the page-cache page again, so this is always safe.
pub fn chunk_done(m: &Mapped, file: usize, start: usize, end: usize) {
    #[cfg(target_os = "linux")]
    {
        static D: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if io_mode() != 4 && *D.get_or_init(|| std::env::var("DROP").map(|v| v == "1").unwrap_or(false)) {
            let map: &[u8] = &m.maps[file];
            let a = (start + 4095) & !4095;
            let b = end & !4095;
            if b > a {
                unsafe {
                    libc::madvise(map.as_ptr().add(a) as *mut _, b - a, libc::MADV_DONTNEED);
                }
            }
        }
    }
    let _ = (m, file, start, end);
}

impl Mapped {
    pub fn open(paths: &[PathBuf]) -> Mapped {
        let mut maps = Vec::new();
        let mut files = Vec::new();
        for p in paths {
            let f = File::open(p).expect("open");
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if len == 0 {
                continue;
            }
            let m = unsafe { memmap2::MmapOptions::new().map(&f).expect("mmap") };
            #[cfg(target_os = "linux")]
            if io_mode() == 1 {
                unsafe {
                    libc::madvise(m.as_ptr() as *mut _, m.len(), libc::MADV_WILLNEED);
                }
            }
            maps.push(m);
            files.push(f);
        }
        Mapped { maps, files }
    }
}

/// Starts a detached thread that asks the kernel to read every mapped file
/// ahead, in 32 MiB steps and in parse order, so a cold page cache is filled
/// at full device queue depth while earlier phases run. Cheap when warm.
pub fn background_readahead(ms: &[&Mapped]) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        if std::env::var("RA").map(|v| v == "0").unwrap_or(false) {
            return;
        }
        let files: Vec<(i32, i64)> =
            ms.iter().flat_map(|m| m.files.iter().zip(m.maps.iter()).map(|(f, mm)| (f.as_raw_fd(), mm.len() as i64))).collect();
        std::thread::spawn(move || {
            const STEP: i64 = 32 << 20;
            for (fd, len) in files {
                let mut off = 0;
                while off < len {
                    unsafe {
                        libc::posix_fadvise(fd, off, STEP.min(len - off), libc::POSIX_FADV_WILLNEED);
                    }
                    off += STEP;
                }
            }
        });
    }
    let _ = ms;
}

/// A page-aligned, huge-page backed read buffer. Reads are issued at
/// page-aligned file offsets into it, so the kernel copies page to page: a
/// source and destination that differ by a few bytes modulo 4096 make the
/// copy several times slower (4K aliasing).
pub struct IoBuf {
    ptr: *mut u8,
    cap: usize,
    len: usize,
}
unsafe impl Send for IoBuf {}

impl IoBuf {
    pub fn new() -> IoBuf {
        IoBuf { ptr: std::ptr::NonNull::<u8>::dangling().as_ptr(), cap: 0, len: 0 }
    }
    fn reserve(&mut self, want: usize) {
        if want <= self.cap {
            return;
        }
        const ALIGN: usize = 2 << 20;
        let cap = (want.max(self.cap * 2) + ALIGN - 1) & !(ALIGN - 1);
        unsafe {
            let p = libc::mmap(std::ptr::null_mut(), cap + ALIGN, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0);
            assert!(p != libc::MAP_FAILED, "read buffer mmap");
            let b = ((p as usize + ALIGN - 1) & !(ALIGN - 1)) as *mut u8;
            #[cfg(target_os = "linux")]
            libc::madvise(b as *mut _, cap, libc::MADV_HUGEPAGE);
            std::ptr::copy_nonoverlapping(self.ptr, b, self.len);
            // the old mapping (if any) is left to process exit: growth is rare
            self.ptr = b;
            self.cap = cap;
        }
    }
    #[inline(always)]
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

/// Input files read with `pread`, never mapped (IOMODE=5).
pub struct Inputs {
    pub files: Vec<File>,
    pub sizes: Vec<usize>,
}

impl Inputs {
    pub fn open(paths: &[PathBuf]) -> Inputs {
        let mut files = Vec::new();
        let mut sizes = Vec::new();
        for p in paths {
            let f = File::open(p).expect("open");
            let len = f.metadata().map(|m| m.len()).unwrap_or(0) as usize;
            if len == 0 {
                continue;
            }
            files.push(f);
            sizes.push(len);
        }
        Inputs { files, sizes }
    }
    pub fn bytes(&self) -> usize {
        self.sizes.iter().sum()
    }
    /// Cuts every file into ranges of `target` bytes. A range owns the lines
    /// whose first byte lies in it (the header line is owned by nobody).
    pub fn chunks(&self, kind: u8, target: usize, out: &mut Vec<Chunk>) {
        for (fi, &size) in self.sizes.iter().enumerate() {
            let mut s = 0;
            while s < size {
                let e = (s + target).min(size);
                out.push(Chunk { kind, file: fi as u32, start: s, end: e });
                s = e;
            }
        }
    }
    /// Appends file bytes [from + buf.len, upto) to `buf`.
    fn fill(&self, file: usize, from: usize, upto: usize, buf: &mut IoBuf) {
        use std::os::unix::fs::FileExt;
        let want = upto - from;
        if want <= buf.len {
            return;
        }
        buf.reserve(want);
        let mut got = buf.len;
        while got < want {
            let dst = unsafe { std::slice::from_raw_parts_mut(buf.ptr.add(got), want - got) };
            let r = self.files[file].read_at(dst, (from + got) as u64).expect("pread");
            if r == 0 {
                break; // the file shrank: treat the rest as absent
            }
            got += r;
        }
        buf.len = got;
    }
    /// Reads the lines a range owns into `buf`; returns (data, start, end)
    /// for `scan_lines`. The read starts at a page boundary at least 16 bytes
    /// before the range, so backward word loads stay inside the buffer.
    pub fn read<'a>(&self, c: &Chunk, buf: &'a mut IoBuf) -> (&'a [u8], usize, usize) {
        const PRE: usize = 16;
        const STEP: usize = 64 << 10;
        let file = c.file as usize;
        let size = self.sizes[file];
        let rs = c.start.saturating_sub(PRE) & !4095;
        let mut re = (c.end + 512).min(size);
        buf.len = 0;
        self.fill(file, rs, re, buf);
        // first newline at or after buffer offset `from`, reading on as needed
        let mut newline_from = |buf: &mut IoBuf, mut from: usize| -> Option<usize> {
            loop {
                let have = buf.len;
                if let Some(p) = memchr::memchr(b'\n', &buf.as_slice()[from.min(have)..]) {
                    return Some(from.min(have) + p);
                }
                if re >= size || have < re - rs {
                    return None;
                }
                from = have;
                re = (re + STEP).min(size);
                self.fill(file, rs, re, buf);
            }
        };
        // where the first owned line starts
        let s = if c.start == 0 {
            match newline_from(buf, 0) {
                Some(p) => p + 1, // after the header
                None => return (buf.as_slice(), 0, 0),
            }
        } else if buf.as_slice().get(c.start - 1 - rs) == Some(&b'\n') {
            c.start - rs
        } else {
            match newline_from(buf, c.start - rs) {
                Some(p) => p + 1,
                None => return (buf.as_slice(), 0, 0),
            }
        };
        // where the first line owned by the next range starts
        let e = if c.end >= size {
            buf.len
        } else if buf.as_slice().get(c.end - 1 - rs) == Some(&b'\n') {
            c.end - rs
        } else {
            match newline_from(buf, c.end - rs) {
                Some(p) => p + 1,
                None => buf.len,
            }
        };
        if s >= e {
            return (buf.as_slice(), 0, 0);
        }
        (buf.as_slice(), s, e)
    }
}

#[derive(Clone, Copy)]
pub struct Chunk {
    pub kind: u8,
    pub file: u32,
    pub start: usize,
    pub end: usize,
}

pub fn chunks_of(kind: u8, m: &Mapped, target: usize, out: &mut Vec<Chunk>) {
    for (fi, map) in m.maps.iter().enumerate() {
        let d: &[u8] = map;
        let Some(h) = memchr::memchr(b'\n', d) else { continue };
        let mut s = h + 1;
        while s < d.len() {
            let mut e = s + target;
            if e >= d.len() {
                e = d.len();
            } else {
                match memchr::memchr(b'\n', &d[e..]) {
                    Some(p) => e = e + p + 1,
                    None => e = d.len(),
                }
            }
            out.push(Chunk { kind, file: fi as u32, start: s, end: e });
            s = e;
        }
    }
}

pub fn read_small_csv(path: &Path) -> Vec<Vec<Vec<u8>>> {
    let data = std::fs::read(path).unwrap_or_default();
    let mut rows = Vec::new();
    let mut first = true;
    for_each_line(&data, |line| {
        if first {
            first = false;
            return;
        }
        let mut f: [&[u8]; 2] = [&[]; 2];
        if split(line, &mut f) {
            rows.push(vec![f[0].to_vec(), f[1].to_vec()]);
        }
    });
    // handle header-only/first-line logic for files whose first line is empty
    rows
}


// ---------------------------------------------------------------------------
// SIMD line scanner
// ---------------------------------------------------------------------------
/// Bitmasks of ',', '"', '\n' over 64 bytes at p (bit i = byte i).
#[inline(always)]
unsafe fn masks64(p: *const u8) -> (u64, u64, u64) {
    #[cfg(all(target_arch = "x86_64", target_feature = "avx512bw"))]
    {
        use std::arch::x86_64::*;
        let v = _mm512_loadu_si512(p as *const _);
        let c = _mm512_cmpeq_epi8_mask(v, _mm512_set1_epi8(b',' as i8));
        let q = _mm512_cmpeq_epi8_mask(v, _mm512_set1_epi8(b'"' as i8));
        let n = _mm512_cmpeq_epi8_mask(v, _mm512_set1_epi8(b'\n' as i8));
        return (c, q, n);
    }
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2", not(target_feature = "avx512bw")))]
    {
        use std::arch::x86_64::*;
        let a = _mm256_loadu_si256(p as *const __m256i);
        let b = _mm256_loadu_si256(p.add(32) as *const __m256i);
        let cm = _mm256_set1_epi8(b',' as i8);
        let qm = _mm256_set1_epi8(b'"' as i8);
        let nm = _mm256_set1_epi8(b'\n' as i8);
        let f = |x: __m256i, y: __m256i| -> u64 {
            let lo = _mm256_movemask_epi8(_mm256_cmpeq_epi8(x, y)) as u32 as u64;
            lo
        };
        let c = f(a, cm) | (f(b, cm) << 32);
        let q = f(a, qm) | (f(b, qm) << 32);
        let n = f(a, nm) | (f(b, nm) << 32);
        (c, q, n)
    }
    #[cfg(target_arch = "aarch64")]
    {
        use std::arch::aarch64::*;
        let v0 = vld1q_u8(p);
        let v1 = vld1q_u8(p.add(16));
        let v2 = vld1q_u8(p.add(32));
        let v3 = vld1q_u8(p.add(48));
        const W: [u8; 16] = [1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128];
        let w = vld1q_u8(W.as_ptr());
        let mm = |ch: u8| -> u64 {
            let c = vdupq_n_u8(ch);
            let m0 = vandq_u8(vceqq_u8(v0, c), w);
            let m1 = vandq_u8(vceqq_u8(v1, c), w);
            let m2 = vandq_u8(vceqq_u8(v2, c), w);
            let m3 = vandq_u8(vceqq_u8(v3, c), w);
            let s0 = vpaddq_u8(m0, m1);
            let s1 = vpaddq_u8(m2, m3);
            let s = vpaddq_u8(s0, s1);
            let s = vpaddq_u8(s, s);
            vgetq_lane_u64(vreinterpretq_u64_u8(s), 0)
        };
        (mm(b','), mm(b'"'), mm(b'\n'))
    }
    #[cfg(not(any(all(target_arch = "x86_64", target_feature = "avx2"), target_arch = "aarch64")))]
    {
        let (mut c, mut q, mut n) = (0u64, 0u64, 0u64);
        for i in 0..64 {
            let b = *p.add(i);
            c |= ((b == b',') as u64) << i;
            q |= ((b == b'"') as u64) << i;
            n |= ((b == b'\n') as u64) << i;
        }
        (c, q, n)
    }
}

/// Mask words per segment: 4 KiB of input, 1.6 KiB of masks, all in L1.
const SEGW: usize = 64;

/// Calls `f` with the N fields of every well-formed line in `data[start..end]`.
/// Lines start at `start` and none crosses `end` (the last one may lack its
/// newline). Bytes of `data` after `end` are never interpreted.
///
/// Stage 1 keeps the comma, quote and newline bitmasks of a 4 KiB segment.
/// Stage 2 handles one line at a time from a 128-bit window of those masks
/// taken at the line start: the newline bit gives the length, the comma count
/// the field count, and one expression decides whether every quote wraps a
/// whole field. Lines of 128 bytes or more take the scalar `split`.
#[inline(always)]
pub fn scan_lines<'a, const N: usize>(data: &'a [u8], start: usize, end: usize, mut f: impl FnMut(&[&'a [u8]; N])) {
    let len = end - start;
    if len == 0 {
        return;
    }
    let base = unsafe { data.as_ptr().add(start) };
    let avail = data.len() - start; // readable bytes from `base`
    let nblocks = len.div_ceil(64);
    let mut fields: [&'a [u8]; N] = [&[]; N];
    let mut cm = [0u64; SEGW + 3];
    let mut qm = [0u64; SEGW + 3];
    let mut nm = [0u64; SEGW + 3];
    let mut tail = [0u8; 64];
    let mut seg0 = usize::MAX; // block held in word 0 of the masks
    let mut ls = 0usize; // current line start
    while ls < len {
        let blk = ls >> 6;
        if seg0 == usize::MAX || blk - seg0 >= SEGW {
            seg0 = blk;
            for i in 0..SEGW + 3 {
                let b = seg0 + i;
                let (mut c, mut q, mut n) = (0u64, 0u64, 0u64);
                if b < nblocks {
                    let off = b * 64;
                    (c, q, n) = if off + 64 <= avail {
                        unsafe { masks64(base.add(off)) }
                    } else {
                        let rem = avail - off;
                        unsafe { std::ptr::copy_nonoverlapping(base.add(off), tail.as_mut_ptr(), rem) };
                        for x in tail[rem..].iter_mut() {
                            *x = 0;
                        }
                        unsafe { masks64(tail.as_ptr()) }
                    };
                    if off + 64 > len {
                        // bytes at and after `end` belong to someone else
                        let keep = (1u64 << (len - off)) - 1;
                        c &= keep;
                        q &= keep;
                        n &= keep;
                    }
                }
                if b == len >> 6 {
                    n |= 1u64 << (len & 63); // the end terminates an unfinished last line
                }
                cm[i] = c;
                qm[i] = q;
                nm[i] = n;
            }
        }
        let w = blk - seg0;
        let sh = (ls & 63) as u32;
        // 128 mask bits starting at the line start
        macro_rules! win {
            ($m:ident) => {{
                let (a, b, c) = unsafe { (*$m.get_unchecked(w), *$m.get_unchecked(w + 1), *$m.get_unchecked(w + 2)) };
                let lo = (a >> sh) | ((b << 1) << (63 - sh));
                let hi = (b >> sh) | ((c << 1) << (63 - sh));
                (lo as u128) | ((hi as u128) << 64)
            }};
        }
        let nw = win!(nm);
        if nw == 0 {
            // a line of 128 bytes or more (rare): the scalar splitter, then
            // the same single call site as the fast path so `f` is inlined once
            let rest = unsafe { std::slice::from_raw_parts(base.add(ls), len - ls) };
            let l = memchr::memchr(b'\n', rest).unwrap_or(rest.len());
            ls += l + 1;
            if !split(&rest[..l], &mut fields) {
                continue;
            }
        } else {
            let ll = nw.trailing_zeros(); // line length, 0..=127
            let below = (1u128 << ll) - 1;
            let c = win!(cm) & below;
            let q = win!(qm) & below;
            let lp = unsafe { base.add(ls) };
            ls += ll as usize + 1;
            // Quotes: with `d` = delimiters (commas and the line end), a field
            // starts after each delimiter and ends before one. Every quote must
            // be the first or last byte of a field (1), a one-byte field cannot
            // be a quote (2), and a field opens with a quote iff it closes with
            // one: subtracting the opening bits from the closing bits (moved onto
            // the delimiter that follows) leaves a delimiter bit set exactly when
            // some field has one without the other (3).
            let d = c | (1u128 << ll);
            let st = (d << 1) | 1;
            let en = d >> 1;
            let x = q & st;
            let y = (q << 1) & d;
            let bad = (q & !(st | en)) | (x & en) | (y.wrapping_sub(x) & d);
            if c.count_ones() as usize != N - 1 || bad != 0 {
                continue; // wrong field count (or an empty line), or a misplaced quote
            }
            // x86-64 with BMI2: every field's start and length as bytes of two
            // words, with a fixed number of rounds on both mask words (the
            // comma count per word varies line to line, so a loop over set
            // bits would mispredict) and the quoted flags through pext/pdep.
            #[cfg(all(target_arch = "x86_64", target_feature = "bmi2"))]
            unsafe {
                use std::arch::x86_64::{_pdep_u64, _pext_u64};
                const { assert!(N >= 2 && N <= 8) };
                const ONES: u64 = 0x0101_0101_0101_0101;
                let (c0, c1) = (c as u64, (c >> 64) as u64);
                let n0 = c0.count_ones();
                let (mut a, mut b, mut p0, mut p1) = (c0, c1, 0u64, 0u64);
                for j in 0..N - 1 {
                    p0 |= (a.trailing_zeros() as u64) << (8 * j);
                    a &= a.wrapping_sub(1);
                    p1 |= (64 + b.trailing_zeros() as u64) << (8 * j);
                    b &= b.wrapping_sub(1);
                }
                // the first n0 commas come from the low word, the rest from the high one
                let lowm = if n0 >= 8 { !0u64 } else { (1u64 << (8 * n0)) - 1 };
                let hi = if n0 >= 8 { 0 } else { p1 << (8 * n0) };
                let allm = if N - 1 >= 8 { !0u64 } else { (1u64 << (8 * (N - 1))) - 1 };
                let commas = ((p0 & lowm) | hi) & allm;
                let ends = commas | (ll as u64) << (8 * (N - 1));
                let starts = (ends << 8).wrapping_add(ONES << 8);
                let (x0, x1, s0, s1) = (x as u64, (x >> 64) as u64, st as u64, (st >> 64) as u64);
                let k0 = s0.count_ones();
                let qbits = _pext_u64(x0, s0) | if k0 >= 64 { 0 } else { _pext_u64(x1, s1) << k0 };
                let quoted = _pdep_u64(qbits, ONES);
                let at = starts.wrapping_add(quoted);
                let ln = ends.wrapping_sub(at).wrapping_sub(quoted);
                for i in 0..N {
                    fields[i] = std::slice::from_raw_parts(lp.add((at >> (8 * i)) as u8 as usize), (ln >> (8 * i)) as u8 as usize);
                }
            }
            #[cfg(not(all(target_arch = "x86_64", target_feature = "bmi2")))]
            {
                let mut m = c;
                let mut fs = 0u32;
                for i in 0..N {
                    let fe = if i == N - 1 {
                        ll
                    } else {
                        let p = m.trailing_zeros();
                        m &= m.wrapping_sub(1);
                        p
                    };
                    let qa = ((q >> fs) as u32) & 1; // quoted: drop the first and last byte
                    fields[i] = unsafe { std::slice::from_raw_parts(lp.add((fs + qa) as usize), (fe - fs - 2 * qa) as usize) };
                    fs = fe + 1;
                }
            }
        }
        f(&fields);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scalar<const N: usize>(d: &[u8]) -> Vec<Vec<Vec<u8>>> {
        let mut v = Vec::new();
        for_each_line(d, |l| {
            let mut f: [&[u8]; N] = [&[]; N];
            if split(l, &mut f) {
                v.push(f.iter().map(|x| x.to_vec()).collect());
            }
        });
        v
    }
    fn simd<const N: usize>(d: &[u8]) -> Vec<Vec<Vec<u8>>> {
        let mut v = Vec::new();
        scan_lines::<N>(d, 0, d.len(), |f| v.push(f.iter().map(|x| x.to_vec()).collect()));
        v
    }
    fn check<const N: usize>(d: &[u8]) {
        assert_eq!(scalar::<N>(d), simd::<N>(d), "{:?}", String::from_utf8_lossy(d));
    }
    fn xs(seed: u64) -> impl FnMut(u64) -> u64 {
        let mut x = seed;
        move |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        }
    }
    #[test]
    fn fuzz_scan() {
        let mut rnd = xs(0x1234567);
        let alphabet = b"ab,\"\n\r 1";
        for _round in 0..3000 {
            let mut d = Vec::new();
            let lines = 1 + rnd(200);
            for _ in 0..lines {
                let kind = rnd(10);
                if kind < 6 {
                    // mostly well-formed 4-field lines with random quoting and lengths
                    let nf = if rnd(8) == 0 { 3 + rnd(3) } else { 4 };
                    for i in 0..nf {
                        if i > 0 {
                            d.push(b',');
                        }
                        let q = rnd(4) == 0;
                        if q {
                            d.push(b'"');
                        }
                        let lim = if rnd(20) == 0 { 90 } else { 12 };
                        for _ in 0..rnd(lim) {
                            let ch = rnd(12) as usize;
                            d.push(b"abcxyz019\r -"[ch]);
                        }
                        if q && rnd(10) != 0 {
                            d.push(b'"');
                        }
                    }
                    d.push(b'\n');
                } else {
                    for _ in 0..rnd(150) {
                        let ch = rnd(alphabet.len() as u64) as usize;
                        d.push(alphabet[ch]);
                    }
                    d.push(b'\n');
                }
            }
            if rnd(2) == 0 {
                d.pop();
            }
            check::<4>(&d);
            check::<2>(&d);
            check::<7>(&d);
        }
    }
    /// Event-shaped lines: 7 fields, 20% quoted, lengths that put the line
    /// end on every side of the 64- and 128-byte marks, plus damaged lines.
    #[test]
    fn fuzz_scan_events() {
        let mut rnd = xs(0xC0FFEE);
        for round in 0..4000 {
            let mut d = Vec::new();
            for _ in 0..(1 + rnd(120)) {
                let nf = if rnd(25) == 0 { 5 + rnd(5) } else { 7 };
                let stretch = match rnd(6) {
                    0 => rnd(70),
                    1 => 40 + rnd(60),
                    _ => 0,
                };
                let mut line = Vec::new();
                for i in 0..nf {
                    if i > 0 {
                        line.push(b',');
                    }
                    let q = rnd(5) == 0;
                    if q {
                        line.push(b'"');
                    }
                    let n = match i {
                        0 | 1 => rnd(11),
                        2 | 3 => [0, 13, 20, 25][rnd(4) as usize],
                        4 => [4, 8, 0][rnd(3) as usize],
                        5 => rnd(7) + if i == 5 { stretch } else { 0 },
                        _ => rnd(6),
                    };
                    for _ in 0..n {
                        line.push(b"e1u9-:TZ+ax0\r _"[rnd(15) as usize]);
                    }
                    if q {
                        line.push(b'"');
                    }
                }
                match rnd(40) {
                    0 => {
                        let i = rnd(line.len() as u64 + 1) as usize;
                        line.insert(i, b'"');
                    }
                    1 => {
                        let i = rnd(line.len() as u64 + 1) as usize;
                        line.insert(i, b',');
                    }
                    2 if !line.is_empty() => {
                        let i = rnd(line.len() as u64) as usize;
                        line.remove(i);
                    }
                    3 => line.clear(),
                    4 => line = b"\"".to_vec(),
                    5 => line = b"\"\",\"\",\"\",\"\",\"\",\"\",\"\"".to_vec(),
                    6 => line = b",,,,,,".to_vec(),
                    _ => {}
                }
                d.extend_from_slice(&line);
                d.push(b'\n');
            }
            if round % 3 == 0 {
                d.pop();
            }
            check::<7>(&d);
            check::<4>(&d);
        }
    }
    /// Every string over a small alphabet up to 9 bytes, as a 2- and 3-field line set.
    #[test]
    fn exhaustive_small() {
        let al = b"a,\"\n";
        for n in 0..=9u32 {
            for code in 0..4u32.pow(n) {
                let mut d = Vec::with_capacity(n as usize);
                let mut c = code;
                for _ in 0..n {
                    d.push(al[(c & 3) as usize]);
                    c >>= 2;
                }
                check::<2>(&d);
                check::<3>(&d);
            }
        }
    }
    /// Reading a file in byte ranges with `Inputs` yields exactly the lines
    /// after the header, once each, whatever the range size.
    #[test]
    fn pread_ranges_match_whole_file() {
        let mut rnd = xs(0xFACE);
        let dir = std::env::temp_dir().join(format!("ss_pread_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for case in 0..300 {
            let mut d = Vec::new();
            let nlines = rnd(40);
            for li in 0..=nlines {
                // line 0 is the header; some lines are long, some empty
                let len = match rnd(8) {
                    0 => 0,
                    1 => 200 + rnd(3000),
                    2 if case % 10 == 0 => 3000 + rnd(9000),
                    _ => rnd(60),
                };
                for _ in 0..len {
                    d.push(b"abc,\"1 "[rnd(7) as usize]);
                }
                if li < nlines || rnd(2) == 0 {
                    d.push(b'\n');
                }
            }
            let path = dir.join(format!("f{case}.csv"));
            std::fs::write(&path, &d).unwrap();
            let want: Vec<Vec<u8>> = {
                let mut v = Vec::new();
                let mut first = true;
                let mut start = 0;
                let mut push = |l: &[u8]| {
                    if first {
                        first = false;
                    } else {
                        v.push(l.to_vec());
                    }
                };
                for nl in memchr::memchr_iter(b'\n', &d) {
                    push(&d[start..nl]);
                    start = nl + 1;
                }
                if start < d.len() {
                    push(&d[start..]);
                }
                v
            };
            let inp = Inputs::open(&[path.clone()]);
            for target in [1usize, 2, 3, 7, 16, 17, 64, 100, 1000, 4096, 5000, 8192, 1 << 20] {
                let mut chunks = Vec::new();
                inp.chunks(0, target, &mut chunks);
                let mut got: Vec<Vec<u8>> = Vec::new();
                let mut buf = IoBuf::new();
                for c in &chunks {
                    let (data, s, e) = inp.read(c, &mut buf);
                    let mut start = s;
                    for nl in memchr::memchr_iter(b'\n', &data[s..e]) {
                        got.push(data[start..s + nl].to_vec());
                        start = s + nl + 1;
                    }
                    if start < e {
                        got.push(data[start..e].to_vec());
                    }
                }
                assert_eq!(got, want, "case {case} target {target} file {:?}", String::from_utf8_lossy(&d));
            }
            std::fs::remove_file(&path).unwrap();
        }
        let _ = std::fs::remove_dir(&dir);
    }
    /// A scan of a sub-range must not look at bytes outside it, wherever the
    /// range starts and ends relative to the 64-byte blocks and the buffer end.
    #[test]
    fn scan_subrange() {
        let mut rnd = xs(0xBEEF);
        for _ in 0..3000 {
            let mut d = Vec::new();
            let mut starts = vec![0usize];
            for _ in 0..(2 + rnd(60)) {
                for i in 0..4 {
                    if i > 0 {
                        d.push(b',');
                    }
                    let q = rnd(4) == 0;
                    if q {
                        d.push(b'"');
                    }
                    let lim = if rnd(9) == 0 { 60 } else { 14 };
                    for _ in 0..rnd(lim) {
                        let al = if rnd(30) == 0 { 8 } else { 6 };
                        d.push(b"abc019\" "[rnd(al) as usize]);
                    }
                    if q {
                        d.push(b'"');
                    }
                }
                d.push(b'\n');
                starts.push(d.len());
            }
            let a = starts[rnd(starts.len() as u64) as usize];
            let b = starts[rnd(starts.len() as u64) as usize];
            let (a, b) = (a.min(b), a.max(b));
            let mut got = Vec::new();
            scan_lines::<4>(&d, a, b, |f| got.push(f.iter().map(|x| x.to_vec()).collect::<Vec<_>>()));
            assert_eq!(got, scalar::<4>(&d[a..b]));
        }
    }
}

// ---------------------------------------------------------------------------
// SWAR fast paths. Each falls back to the exact scalar parser when unsure.
// ---------------------------------------------------------------------------
#[inline(always)]
pub fn ld8(p: *const u8) -> u64 {
    u64::from_le(unsafe { (p as *const u64).read_unaligned() })
}

/// Every byte is an ASCII digit (fast_float's check).
#[inline(always)]
pub fn digits8(x: u64) -> bool {
    ((x & 0xF0F0_F0F0_F0F0_F0F0) | (((x.wrapping_add(0x0606_0606_0606_0606)) & 0xF0F0_F0F0_F0F0_F0F0) >> 4))
        == 0x3333_3333_3333_3333
}

/// The value of 8 ASCII digits, first byte most significant.
#[inline(always)]
pub fn val8(x: u64) -> u64 {
    let v = x.wrapping_sub(0x3030_3030_3030_3030);
    let v = (v.wrapping_mul(10)).wrapping_add(v >> 8) & 0x00FF_00FF_00FF_00FF;
    let v = (v.wrapping_mul(100)).wrapping_add(v >> 16) & 0x0000_FFFF_0000_FFFF;
    (v.wrapping_mul(10000)).wrapping_add(v >> 32) & 0xFFFF_FFFF
}

/// Pairs: byte 2k of the result = 10 * digit(2k) + digit(2k+1).
#[inline(always)]
fn pairs(x: u64) -> u64 {
    let v = x.wrapping_sub(0x3030_3030_3030_3030) & 0x0F0F_0F0F_0F0F_0F0F;
    v.wrapping_mul(2561) >> 8
}

#[inline(always)]
fn days_from_civil_u(y: u64, m: u64, d: u64) -> i64 {
    // y in 0..=9999; shift by 400 years to stay non-negative
    let y = y + 400 - (m <= 2) as u64;
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe) as i64 - 719_468 - 146_097
}

/// Seconds of an F1/F2 civil prefix "YYYY-MM-DDTHH:MM:SS" at p (20+ readable bytes).
#[inline(always)]
fn civil_swar(p: *const u8) -> Option<i64> {
    let a = ld8(p);
    let b = ld8(unsafe { p.add(8) });
    let c = ld8(unsafe { p.add(12) });
    const SA: u64 = 0xFF00_00FF_0000_0000;
    const SB: u64 = 0x0000_FF00_00FF_0000;
    const SC: u64 = 0x0000_00FF_0000_FF00;
    let sep = ((a & SA) ^ 0x2D00_002D_0000_0000) | ((b & SB) ^ 0x0000_3A00_0054_0000) | ((c & SC) ^ 0x0000_003A_0000_3A00);
    let a2 = (a & !SA) | 0x3000_0030_0000_0000;
    let b2 = (b & !SB) | 0x0000_3000_0030_0000;
    let c2 = (c & 0x00FF_FFFF_FFFF_FFFF & !SC) | 0x3000_0030_0000_3000;
    if sep != 0 || !digits8(a2) || !digits8(b2) || !digits8(c2) {
        return None;
    }
    let pa = pairs(a2);
    let pa8 = pairs(a2 >> 8 | 0x3000_0000_0000_0000);
    let pb = pairs(b2);
    let pb8 = pairs(b2 >> 8 | 0x3000_0000_0000_0000);
    let pc8 = pairs(c2 >> 8 | 0x3000_0000_0000_0000);
    let y = (pa & 0xFF) * 100 + ((pa >> 16) & 0xFF);
    let mo = (pa8 >> 32) & 0xFF;
    let d = pb & 0xFF;
    let h = (pb8 >> 16) & 0xFF;
    let mi = (pb >> 48) & 0xFF;
    let s = (pc8 >> 32) & 0xFF;
    if mo.wrapping_sub(1) >= 12 || d == 0 || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    let dim = match mo {
        2 => 28 + ((y % 4 == 0 && y % 100 != 0) || y % 400 == 0) as u64,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if d > dim {
        return None;
    }
    Some(days_from_civil_u(y, mo, d) * 86400 + (h * 3600 + mi * 60 + s) as i64)
}

/// Dates already validated by this thread: the 11 bytes `YYYY-MM-DDT` and
/// the epoch seconds of that midnight, indexed by the day-of-month digits. A
/// window touches about 30 dates, so nearly every timestamp is a hit: two
/// word compares instead of digit checks, range checks and the calendar math.
pub struct DateMemo {
    e: [(u64, u64, i64); 64],
    #[cfg(test)]
    pub hits: u64,
}
impl DateMemo {
    pub fn new() -> DateMemo {
        // no timestamp starts with eight NUL bytes
        DateMemo {
            e: [(0, 0, 0); 64],
            #[cfg(test)]
            hits: 0,
        }
    }
}

/// Seconds of day of `HH:MM:SS` at p (8 readable bytes); None unless it is
/// exactly two digits, a colon, two digits, a colon, two digits, in range.
#[inline(always)]
fn tod_checked(p: *const u8) -> Option<i64> {
    const SEP: u64 = 0x0000_FF00_00FF_0000;
    let w = ld8(p);
    let w2 = (w & !SEP) | 0x0000_3000_0030_0000;
    if (w & SEP) != 0x0000_3A00_003A_0000 || !digits8(w2) {
        return None;
    }
    let pa = pairs(w2);
    let h = pa & 0xFF;
    let mi = (pairs(w2 >> 8) >> 16) & 0xFF;
    let sec = (pa >> 48) & 0xFF;
    if h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    Some((h * 3600 + mi * 60 + sec) as i64)
}

/// Same result as `civil_swar` for every input (20+ readable bytes at p).
#[inline(always)]
fn civil_memo(p: *const u8, memo: &mut DateMemo) -> Option<i64> {
    let a = ld8(p);
    let b = ld8(unsafe { p.add(8) }) & 0x00FF_FFFF; // "DDT"
    let idx = (((b & 3) << 4) | ((b >> 8) & 15)) as usize;
    let e = unsafe { *memo.e.get_unchecked(idx) };
    if e.0 == a && e.1 == b {
        // the date bytes equal a date that passed every check before
        #[cfg(test)]
        {
            memo.hits += 1;
        }
        return tod_checked(unsafe { p.add(11) }).map(|t| e.2 + t);
    }
    let v = civil_swar(p)?;
    // fully valid: remember its midnight
    memo.e[idx] = (a, b, v - v.rem_euclid(86400));
    Some(v)
}

/// `ingest_ts` is always valid F1 (rules): seconds since the epoch of a
/// 20-byte "YYYY-MM-DDTHH:MM:SSZ" without validation; anything of another
/// shape goes through the checked parser.
#[inline(always)]
pub fn parse_ingest_s(s: &[u8], ceil: *const u8, memo: &mut DateMemo) -> Option<i64> {
    let p = s.as_ptr();
    if s.len() == 20 && (p as usize) + 20 <= ceil as usize && s[19] == b'Z' {
        // a known date with a well-formed time of day (the value is then the
        // same as the unvalidated arithmetic below, which stays for the rest)
        let ka = ld8(p);
        let kb = ld8(unsafe { p.add(8) }) & 0x00FF_FFFF;
        let e = unsafe { *memo.e.get_unchecked((((kb & 3) << 4) | ((kb >> 8) & 15)) as usize) };
        if e.0 == ka && e.1 == kb {
            if let Some(t) = tod_checked(unsafe { p.add(11) }) {
                #[cfg(test)]
                {
                    memo.hits += 1;
                }
                return Some(e.2 + t);
            }
        }
        const SA: u64 = 0xFF00_00FF_0000_0000;
        const SB: u64 = 0x0000_FF00_00FF_0000;
        const SC: u64 = 0x0000_00FF_0000_0000;
        let a = (ld8(p) & !SA) | 0x3000_0030_0000_0000;
        let b = (ld8(unsafe { p.add(8) }) & !SB) | 0x0000_3000_0030_0000;
        let c = (ld8(unsafe { p.add(12) }) & 0x00FF_FFFF_FFFF_FFFF & !SC) | 0x3000_0030_0000_0000;
        let pa = pairs(a);
        let pa8 = pairs(a >> 8 | 0x3000_0000_0000_0000);
        let pb = pairs(b);
        let pb8 = pairs(b >> 8 | 0x3000_0000_0000_0000);
        let pc8 = pairs(c >> 8 | 0x3000_0000_0000_0000);
        let y = (pa & 0xFF) * 100 + ((pa >> 16) & 0xFF);
        let mo = (pa8 >> 32) & 0xFF;
        let d = pb & 0xFF;
        let h = (pb8 >> 16) & 0xFF;
        let mi = (pb >> 48) & 0xFF;
        let sec = (pc8 >> 32) & 0xFF;
        let v = days_from_civil_u(y, mo, d) * 86400 + (h * 3600 + mi * 60 + sec) as i64;
        // remember the date only if the whole timestamp is valid
        if let Some(cv) = civil_swar(p) {
            memo.e[(((kb & 3) << 4) | ((kb >> 8) & 15)) as usize] = (ka, kb, cv - cv.rem_euclid(86400));
        }
        return Some(v);
    }
    parse_ts(s).map(|v| v.div_euclid(1000))
}

/// Same result as `parse_ts`, faster for the common shapes. `floor` is the
/// first readable byte before `s`; `s` must be followed by >= 8 readable bytes
/// (true inside a scanned line: at least the newline or 64-byte tail padding
/// is not guaranteed, so callers pass `ceil` = one past the last readable byte).
#[inline(always)]
pub fn parse_ts_fast(s: &[u8], floor: *const u8, ceil: *const u8, memo: &mut DateMemo) -> Option<i64> {
    let n = s.len();
    let p = s.as_ptr();
    if n == 20 && (p as usize) + 20 <= ceil as usize {
        if s[19] == b'Z' {
            if let Some(v) = civil_memo(p, memo) {
                return Some(v * 1000);
            }
        }
        return parse_ts(s);
    }
    if n == 25 && (p as usize) + 25 <= ceil as usize {
        let sign = s[19];
        let oh = d2(s, 20);
        let om = d2(s, 23);
        if (sign == b'+' || sign == b'-') && s[22] == b':' && oh <= 23 && om <= 59 {
            if let Some(base) = civil_memo(p, memo) {
                let off = (oh * 60 + om) as i64 * 60;
                return Some(if sign == b'+' { base - off } else { base + off } * 1000);
            }
        }
        return parse_ts(s);
    }
    let e = p as usize + n;
    if n >= 9 && n <= 16 && e >= floor as usize + 16 {
        let lo = ld8((e - 16) as *const u8);
        let hi = ld8((e - 8) as *const u8);
        let keep = 16 - n; // leading bytes of `lo` outside the field
        let lo = (lo & (!0u64 << (8 * keep))) | (0x3030_3030_3030_3030 & !(!0u64 << (8 * keep)));
        if digits8(lo) && digits8(hi) {
            return Some((val8(lo) * 100_000_000 + val8(hi)) as i64);
        }
        return parse_ts(s);
    }
    parse_ts(s)
}

#[cfg(test)]
mod swar_tests {
    use super::*;
    #[test]
    fn civil_swar_fuzz() {
        let mut x: u64 = 88172645463325252;
        let mut rnd = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        let mut hits = 0;
        let mut memo = DateMemo::new();
        let mut hits_memo = 0u64;
        for i in 0..3_000_000u32 {
            let mut s = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", 1990 + rnd(60), rnd(14), rnd(33), rnd(26), rnd(62), rnd(62)).into_bytes();
            if i % 3 == 0 {
                let k = rnd(20) as usize;
                s[k] = b"0123456789:-TZtz +/.9\r,\""[rnd(24) as usize];
            }
            let mut buf = vec![b'x'; 16];
            buf.extend_from_slice(&s);
            buf.extend_from_slice(&[b','; 16]);
            let f = &buf[16..36];
            // a fresh table every 1000 cases: misses, then hits, on the same dates
            if i % 1000 == 0 {
                hits_memo += memo.hits;
                memo = DateMemo::new();
            }
            let r = parse_ts_fast(f, buf.as_ptr(), unsafe { buf.as_ptr().add(buf.len()) }, &mut memo);
            assert_eq!(r, parse_ts(&s), "{:?}", String::from_utf8_lossy(&s));
            let r2 = parse_ts_fast(f, buf.as_ptr(), unsafe { buf.as_ptr().add(buf.len()) }, &mut memo);
            assert_eq!(r2, parse_ts(&s), "warm {:?}", String::from_utf8_lossy(&s));
            if f[19] == b'Z' {
                // ingest path: unvalidated for malformed input, exact for valid input, and
                // always equal to what it returns with an empty table
                let cold = parse_ingest_s(f, unsafe { buf.as_ptr().add(buf.len()) }, &mut DateMemo::new());
                assert_eq!(parse_ingest_s(f, unsafe { buf.as_ptr().add(buf.len()) }, &mut memo), cold, "ingest {:?}", String::from_utf8_lossy(&s));
                if let Some(v) = parse_ts(&s) {
                    assert_eq!(cold, Some(v / 1000));
                }
            }
            if f[19] == b'Z' {
                let c = civil_swar(f.as_ptr());
                assert_eq!(c.map(|v| v * 1000), parse_ts(&s), "{:?}", String::from_utf8_lossy(&s));
                hits += c.is_some() as u32;
            }
        }
        assert!(hits > 100_000);
        assert!(hits_memo > 1_000_000, "memo hits {hits_memo}");
    }
    #[test]
    fn civil_swar_hits() {
        let b = b"2027-12-18T22:22:54Z,xxxxxxxxxxxx";
        assert_eq!(civil_swar(b.as_ptr()).map(|v| v * 1000), parse_ts(&b[..20]));
        assert!(civil_swar(b.as_ptr()).is_some());
    }
    #[test]
    fn ts_fuzz() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let tmpl: [&[u8]; 6] = [
            b"2027-12-13T10:11:44Z", b"2027-02-28T23:59:59+05:30", b"1828126192000", b"0001828126192000",
            b"2028-02-29T00:00:00-10:00", b"2000-12-31T00:00:00Z",
        ];
        let alpha = b"0123456789-+:TZtz .e";
        let mut memo = DateMemo::new();
        for _ in 0..300_000 {
            let t = tmpl[(rnd() % 6) as usize];
            let mut v = t.to_vec();
            let muts = rnd() % 3;
            for _ in 0..muts {
                let i = (rnd() as usize) % v.len();
                match rnd() % 4 {
                    0 => v[i] = alpha[(rnd() as usize) % alpha.len()],
                    1 => v[i] = b'0' + (rnd() % 10) as u8,
                    2 => {
                        v.remove(i);
                    }
                    _ => v.insert(i, b'0' + (rnd() % 10) as u8),
                }
                if v.is_empty() {
                    break;
                }
            }
            let mut buf = vec![b'7'; 32];
            buf.extend_from_slice(&v);
            buf.extend_from_slice(&[b','; 32]);
            let sl = &buf[32..32 + v.len()];
            let (fl, ce) = (buf.as_ptr(), unsafe { buf.as_ptr().add(buf.len()) });
            let want = parse_ts(&v);
            assert_eq!(parse_ts_fast(sl, fl, ce, &mut memo), want, "fast {:?}", String::from_utf8_lossy(&v));
            assert_eq!(parse_ts_fast(sl, fl, ce, &mut DateMemo::new()), want, "cold {:?}", String::from_utf8_lossy(&v));
        }
    }
    #[test]
    fn ts_fast_matches() {
        let mut cases: Vec<String> = vec![
            "2026-10-03T23:58:10Z", "2026-10-03T19:58:10-04:00", "1791071890000", "0001791071890000",
            "2028-02-29T00:00:00Z", "2027-02-29T00:00:00Z", "2026-13-01T00:00:00Z", "2026-02-30T00:00:00Z",
            "2026-10-03T24:00:00Z", "2026-10-03T23:60:00Z", "2026-10-03T23:59:60Z", "2026-10-03t23:58:10Z",
            "2026-10-03 23:58:10Z", "2026-10-03T23:58:10z", "2026-10-03T23:58:10", "1791071890000.0",
            "1.79e12", "-5", "86400000", "", "0000-01-01T00:00:00Z", "9999-12-31T23:59:59Z",
            "2026-1a-03T23:58:10Z", "2026/10/03T23:58:10Z", "123456789012345678901", "1234567890123456",
            "12345678901234567", "2026-00-10T00:00:00Z", "2026-10-00T00:00:00Z", "1900-02-29T00:00:00Z",
            "2000-02-29T00:00:00Z", "2026-10-03T23:58:1\rZ", "00000000086400000", "999999999",
        ].into_iter().map(String::from).collect();
        let mut memo = DateMemo::new();
        // all F1 seconds of a few days plus random digit strings
        for c in ["2026-10-03T19:58:10+00:00", "2026-10-03T19:58:10-00:00", "2026-10-03T19:58:10+23:59", "2026-10-03T19:58:10+24:00",
                  "2026-10-03T19:58:10+05:60", "2026-10-03T19:58:10+0500", "2026-10-03T19:58:10 05:00", "2026-02-29T19:58:10+05:00",
                  "2028-02-29T00:00:00-10:00", "2026-10-03T19:58:10*05:00", "2026-10-03T19:58:10+0a:00", "2026-10-03T19:58:10+05:0a"] {
            cases.push(c.to_string());
        }
        for d in 0..40 {
            cases.push(format!("2027-12-{:02}T{:02}:{:02}:{:02}Z", 1 + d % 31, d % 24, (d * 7) % 60, (d * 13) % 60));
        }
        for c in &cases {
            let mut buf = vec![b'x'; 32];
            buf.extend_from_slice(c.as_bytes());
            buf.extend_from_slice(&[b','; 32]);
            let s = &buf[32..32 + c.len()];
            for _ in 0..2 {
                let r = parse_ts_fast(s, buf.as_ptr(), unsafe { buf.as_ptr().add(buf.len()) }, &mut memo);
                assert_eq!(r, parse_ts(c.as_bytes()), "{c:?}");
            }
            if let Some(v) = parse_ts(c.as_bytes()) {
                if c.len() == 20 && c.ends_with('Z') {
                    assert_eq!(parse_ingest_s(s, unsafe { buf.as_ptr().add(buf.len()) }, &mut memo), Some(v.div_euclid(1000)), "{c:?}");
                }
            }
        }
    }
}
