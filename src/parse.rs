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
/// 2 per-chunk WILLNEED, 3 per-chunk MADV_POPULATE_READ, 4 per-chunk pread.
pub fn io_mode() -> u32 {
    static M: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *M.get_or_init(|| std::env::var("IOMODE").ok().and_then(|v| v.parse().ok()).unwrap_or(3))
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
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
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

pub fn masks64_pub(b: &[u8]) -> (u64, u64, u64) {
    assert!(b.len() >= 64);
    unsafe { masks64(b.as_ptr()) }
}

/// Calls `f` with the N fields of every well-formed line in `data[start..end]`.
/// Lines start at `start` and none crosses `end`.
///
/// Structural iteration: for each 64-byte block, the set bits of
/// (comma | newline) are visited in order; a line's separators are recorded and
/// on its newline the fields are cut and quote-checked with bit tests on a
/// small ring of quote masks. Lines longer than the ring take the scalar path.
#[inline(always)]
pub fn scan_lines<'a, const N: usize>(data: &'a [u8], start: usize, end: usize, mut f: impl FnMut(&[&'a [u8]; N])) {
    let len = end - start;
    if len == 0 {
        return;
    }
    let base = unsafe { data.as_ptr().add(start) };
    let mut fields: [&'a [u8]; N] = [&[]; N];
    let mut seps = [0u32; N]; // positions of the commas, then the newline
    let mut k = 0usize; // commas seen in the current line
    let mut line_start = 0usize;
    let mut qr = [0u64; 4]; // quote masks of the last 4 blocks
    let nblocks = len.div_ceil(64);
    let mut tail = [0u8; 64];
    let qbit = |qr: &[u64; 4], p: usize| -> u64 { (qr[(p >> 6) & 3] >> (p & 63)) & 1 };
    for b in 0..nblocks {
        let off = b * 64;
        let (c, q, n) = if off + 64 <= len {
            unsafe { masks64(base.add(off)) }
        } else {
            let rem = len - off;
            unsafe { std::ptr::copy_nonoverlapping(base.add(off), tail.as_mut_ptr(), rem) };
            for x in tail[rem..].iter_mut() {
                *x = 0;
            }
            unsafe { masks64(tail.as_ptr()) }
        };
        qr[b & 3] = q;
        let mut sep = c | n;
        while sep != 0 {
            let bit = sep.trailing_zeros();
            sep &= sep - 1;
            let p = off + bit as usize;
            if (n >> bit) & 1 == 0 {
                if k < N {
                    seps[k] = p as u32;
                }
                k += 1;
                continue;
            }
            // newline at p: line [line_start, p)
            let ls = line_start;
            line_start = p + 1;
            let kk = k;
            k = 0;
            if kk != N - 1 || p == ls {
                continue;
            }
            seps[N - 1] = p as u32;
            if p - ls > 190 {
                // too long for the quote ring
                let line = unsafe { std::slice::from_raw_parts(base.add(ls), p - ls) };
                if split(line, &mut fields) {
                    f(&fields);
                }
                continue;
            }
            // quotes in the line
            let mut qcount = 0u32;
            {
                let mut w = ls >> 6;
                let last = (p - 1) >> 6;
                while w <= last {
                    let mut m = qr[w & 3];
                    if w == ls >> 6 {
                        m &= !0u64 << (ls & 63);
                    }
                    if w == last {
                        let e = ((p - 1) & 63) as u32;
                        m &= if e == 63 { !0 } else { (1u64 << (e + 1)) - 1 };
                    }
                    qcount += m.count_ones();
                    w += 1;
                }
            }
            let mut fs = ls;
            let mut bad = 0u64;
            let mut seen = 0u32;
            for i in 0..N {
                let fe = seps[i] as usize;
                let flen = fe - fs;
                let nz = (flen != 0) as u64;
                let a = qbit(&qr, fs) & nz;
                let z = qbit(&qr, fe.wrapping_sub(1).max(fs)) & nz;
                bad |= (a ^ z) | (a & (flen < 2) as u64);
                seen += 2 * a as u32;
                let a = a as usize;
                fields[i] = unsafe { std::slice::from_raw_parts(base.add(fs + a), flen - 2 * a.min(flen / 2)) };
                fs = fe + 1;
            }
            if bad == 0 && seen == qcount {
                f(&fields);
            }
        }
    }
    // a final line without a newline
    if line_start < len {
        let line = unsafe { std::slice::from_raw_parts(base.add(line_start), len - line_start) };
        if split(line, &mut fields) {
            f(&fields);
        }
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
    #[test]
    fn fuzz_scan() {
        let mut x: u64 = 0x1234567;
        let mut rnd = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
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
            assert_eq!(scalar::<4>(&d), simd::<4>(&d));
            assert_eq!(scalar::<2>(&d), simd::<2>(&d));
        }
    }
}
