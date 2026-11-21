use crate::pqw::*;
use crate::*;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

fn die(what: &str) -> ! {
    eprintln!("[v2b] write error: {what}");
    unsafe { sys::_exit(1) }
}

#[derive(Clone, Copy)]
struct View {
    keys: *const u32,
    day: *const i32,
    ses: *const i32,
    evs: *const i32,
    rev: *const i32,
    cat: *const u8,
    names: *const u8,
    nnames: usize,
    noff: *const u32,
    nnoff: usize,
    rows: usize,
}
unsafe impl Send for View {}
unsafe impl Sync for View {}
fn view(c: &crate::core::Core) -> View {
    View {
        keys: c.keys.as_ptr(),
        day: c.day.as_ptr(),
        ses: c.ses.as_ptr(),
        evs: c.evs.as_ptr(),
        rev: c.rev.as_ptr(),
        cat: c.cat.as_ptr(),
        names: c.names.as_ptr(),
        nnames: c.names.len(),
        noff: c.names_off.as_ptr(),
        nnoff: c.names_off.len(),
        rows: c.day.len(),
    }
}

fn write_piece(g: &G, t: usize, a: usize, b: usize, path: &Path, cats_off: &[u32], cats_data: &[u8], kbuf: &mut Vec<u32>) {
    write_rows(&view(&g.t(t).core), a, b, path, cats_off, cats_data, kbuf)
}

fn write_rows(v: &View, a: usize, b: usize, path: &Path, cats_off: &[u32], cats_data: &[u8], kbuf: &mut Vec<u32>) {
    use std::slice::from_raw_parts as sl;
    #[cfg(feature = "tl")]
    if std::env::var_os("XL_W6SKIP").is_some() { return; }
    let n = b - a;
    let keys = unsafe { sl(v.keys.add(a), n) };
    let _ = &kbuf;
    let (names, noff) = unsafe { (sl(v.names, v.nnames), sl(v.noff, v.nnoff)) };
    let (no, lo): (&[u32], u32) = if a == b { (&[], 0) } else { (noff, 0) };
    let cols = unsafe {
        [
            Column { name: "user_id", data: Data::Dict { dict: StrCol { offsets: no, data: names }, keys: Keys::U32Off(keys, lo), nullable: false, plain: false } },
            Column { name: "day", data: Data::Date(sl(v.day.add(a), n), IntEnc::Dense) },
            Column { name: "sessions", data: Data::I32(sl(v.ses.add(a), n), IntEnc::Dense) },
            Column { name: "events", data: Data::I32(sl(v.evs.add(a), n), IntEnc::Auto) },
            Column { name: "revenue_cents", data: Data::I32(sl(v.rev.add(a), n), IntEnc::Plain) },
            Column { name: "top_category", data: Data::Dict { dict: StrCol { offsets: cats_off, data: cats_data }, keys: Keys::U8(sl(v.cat.add(a), n)), nullable: true, plain: false } },
        ]
    };
    let opts = if RPG == 0 { Opts::default() } else { Opts { rows_per_group: RPG } };
    if write_file(path, &cols, &opts, Method::Stream(1 << 20)).is_err() {
        die("user_daily");
    }
}

fn write_campaign(g: &G, dir: &Path) {
    let nd = g.nd;
    let n = (g.nc + 1) * nd;
    let mut agg = vec![[0u64; 4]; n];
    for tid in 0..g.nt {
        let c = &g.t(tid).core;
        for i in 0..n {
            agg[i][0] += c.agg[i][0] as u64;
            agg[i][3] += c.agg[i][1] as u64;
            agg[i][1] += c.aggp[i][0];
            agg[i][2] += c.aggp[i][1];
        }
    }
    #[cfg(feature = "tl")]
    unsafe {
        crate::sys::tl::MK[15][4] = crate::sys::tl::now();
    }
    let (mut id_off, mut id_data, mut ch_off, mut ch_data) = (vec![0u32], Vec::new(), vec![0u32], Vec::new());
    let mut day = Vec::new();
    let mut v: [Vec<i64>; 4] = Default::default();
    for cn in 0..=g.nc {
        for d in 0..nd {
            let a = agg[cn * nd + d];
            if a[0] == 0 && a[3] == 0 {
                continue;
            }
            if cn == g.nc {
                id_data.extend_from_slice(b"organic");
                ch_data.extend_from_slice(b"organic");
            } else {
                let mut d = [0u8; 24];
                let (mut i, mut x) = (d.len(), cn);
                loop {
                    i -= 1;
                    d[i] = b'0' + (x % 10) as u8;
                    x /= 10;
                    if x == 0 {
                        break;
                    }
                }
                i -= 2;
                d[i] = b'c';
                d[i + 1] = b'_';
                id_data.extend_from_slice(&d[i..]);
                ch_data.extend_from_slice(&g.channels[g.camp[cn] as usize]);
            }
            id_off.push(id_data.len() as u32);
            ch_off.push(ch_data.len() as u32);
            day.push(g.day0 + d as i32);
            for k in 0..4 {
                v[k].push(a[k] as i64);
            }
        }
    }
    let empty: &[u32] = &[];
    let rows = day.len();
    let cols = [
        Column { name: "campaign_id", data: Data::Str(StrCol { offsets: if rows == 0 { empty } else { &id_off }, data: &id_data }) },
        Column { name: "channel", data: Data::Str(StrCol { offsets: if rows == 0 { empty } else { &ch_off }, data: &ch_data }) },
        Column { name: "day", data: Data::Date(&day, IntEnc::Plain) },
        Column { name: "sessions", data: Data::I64(&v[0], IntEnc::Plain) },
        Column { name: "purchasing_sessions", data: Data::I64(&v[1], IntEnc::Plain) },
        Column { name: "revenue_cents", data: Data::I64(&v[2], IntEnc::Plain) },
        Column { name: "assisted_sessions", data: Data::I64(&v[3], IntEnc::Plain) },
    ];
    #[cfg(feature = "tl")]
    unsafe {
        crate::sys::tl::MK[15][5] = crate::sys::tl::now();
    }
    if write_file(&dir.join("part-0.parquet"), &cols, &Opts::default(), Method::Whole).is_err() {
        die("campaign_daily");
    }
    #[cfg(feature = "tl")]
    unsafe {
        crate::sys::tl::MK[15][6] = crate::sys::tl::now();
    }
    if g.check {
        let (mut us, mut ur) = (0i64, 0i64);
        for tid in 0..g.nt {
            let c = &g.t(tid).core;
            us += c.ses.as_slice().iter().map(|&x| x as i64).sum::<i64>();
            ur += c.rev.as_slice().iter().map(|&x| x as i64).sum::<i64>();
        }
        let cs: i64 = v[0].iter().sum();
        let cr: i64 = v[2].iter().sum();
        let ca: i64 = v[3].iter().sum();
        let organic: i64 = (0..nd).map(|d| agg[g.nc * nd + d][0] as i64).sum();
        eprintln!("[v2b] check: sessions user={us} campaign={cs} revenue user={ur} campaign={cr} assisted={ca} credited_non_organic={}", cs - organic);
        if us != cs || ur != cr || ca < cs - organic {
            eprintln!("[v2b] CHECK FAILED");
            unsafe { sys::_exit(5) }
        }
    }
}

fn write_campaign_part(g: &G, path: &Path, lo: usize, hi: usize, pre: Option<std::fs::File>, ugf: &mut Option<GroupFile>) -> [i64; 4] {
    #[cfg(feature = "tl")]
    let tc0 = crate::sys::tl::now();
    let nd = g.nd;
    let n = (hi - lo) * nd;
    let base = lo * nd;
    let mut sa = vec![[0u64; 2]; n];
    let mut pr = vec![[0u64; 2]; n];
    for tid in 0..g.nt {
        let c = &g.t(tid).core;
        if n > 0 {
            unsafe { add_cells(sa.as_mut_ptr() as *mut u64, pr.as_mut_ptr() as *mut u64, &c.agg[base] as *const [u32; 2] as *const u32, &c.aggp[base] as *const [u64; 2] as *const u64, n) };
        }
    }
    #[cfg(feature = "tl")]
    let tc1 = crate::sys::tl::now();
    #[cfg(feature = "tl")]
    ev::CA.fetch_add(tc1 - tc0, Ordering::Relaxed);
    let rows = n;
    let (mut id_off, mut id_data, mut ch_keys) = (Vec::with_capacity(rows + 1), Vec::with_capacity(rows * 8), Vec::with_capacity(rows));
    id_off.push(0u32);
    let mut day = Vec::with_capacity(rows);
    let mut v: [Vec<i64>; 4] = [Vec::with_capacity(rows), Vec::with_capacity(rows), Vec::with_capacity(rows), Vec::with_capacity(rows)];
    let mut organic = 0i64;
    let nch = g.channels.len();
    let (mut ch_off, mut ch_data) = (Vec::with_capacity(nch + 2), Vec::new());
    ch_off.push(0u32);
    for c in &g.channels {
        ch_data.extend_from_slice(c);
        ch_off.push(ch_data.len() as u32);
    }
    ch_data.extend_from_slice(b"organic");
    ch_off.push(ch_data.len() as u32);
    for cn in lo..hi {
        let cells = &sa[(cn - lo) * nd..(cn - lo + 1) * nd];
        if !unsafe { any_nonzero(cells.as_ptr() as *const u64, 2 * nd) } {
            continue;
        }
        let mut s = [0u8; 24];
        let name: &[u8] = if cn == g.nc {
            b"organic"
        } else {
            let (mut i, mut x) = (s.len(), cn);
            loop {
                i -= 1;
                s[i] = b'0' + (x % 10) as u8;
                x /= 10;
                if x == 0 {
                    break;
                }
            }
            i -= 2;
            s[i] = b'c';
            s[i + 1] = b'_';
            &s[i..]
        };
        let ck = if cn == g.nc { nch as u32 } else { g.camp[cn] as u32 };
        for d in 0..nd {
            let a = cells[d];
            if a[0] == 0 && a[1] == 0 {
                continue;
            }
            let p = pr[(cn - lo) * nd + d];
            if cn == g.nc {
                organic += a[0] as i64;
            }
            id_data.extend_from_slice(name);
            id_off.push(id_data.len() as u32);
            ch_keys.push(ck);
            day.push(g.day0 + d as i32);
            v[0].push(a[0] as i64);
            v[1].push(p[0] as i64);
            v[2].push(p[1] as i64);
            v[3].push(a[1] as i64);
        }
    }
    #[cfg(feature = "tl")]
    ev::CB.fetch_add(crate::sys::tl::now() - tc1, Ordering::Relaxed);
    if day.is_empty() && lo > 0 {
        if pre.is_some() {
            drop(pre);
            if std::fs::remove_file(path).is_err() {
                die("campaign_daily");
            }
        }
    } else {
        let cg = CampGroup { id: StrCol { offsets: &id_off, data: &id_data }, ch_dict: StrCol { offsets: &ch_off, data: &ch_data }, ch_keys: &ch_keys, day: &day, day0: g.day0, nd, v: [&v[0], &v[1], &v[2], &v[3]] };
        #[cfg(feature = "tl")]
        let tf0 = crate::sys::tl::now();
        let lent = match ugf.as_mut() {
            Some(u) => match u.take_buf() {
                Ok(b) => Some(b),
                Err(_) => die("user_daily"),
            },
            None => None,
        };
        let f = match (pre, lent) {
            (Some(f), Some(b)) => Ok(GroupFile::from_file_buf(f, CF_BUF, b)),
            (Some(f), None) => Ok(GroupFile::from_file(f, CF_BUF)),
            (None, Some(b)) => std::fs::File::create(path).map(|f| GroupFile::from_file_buf(f, CF_BUF, b)),
            (None, None) => GroupFile::create(path, CF_BUF),
        };
        #[cfg(feature = "tl")]
        let tf1 = crate::sys::tl::now();
        let r = f.and_then(|mut f| f.campaign_group(&cg).and_then(|_| f.finish_buf(&campaign_schema())));
        let r = r.map(|b| if let Some(u) = ugf.as_mut() { u.put_buf(b) });
        #[cfg(feature = "tl")]
        { ev::CC.fetch_add(tf1 - tf0, Ordering::Relaxed); ev::CD.fetch_add(crate::sys::tl::now() - tf1, Ordering::Relaxed); }
        if r.is_err() {
            die("campaign_daily");
        }
    }
    [v[0].iter().sum(), v[2].iter().sum(), v[3].iter().sum(), organic]
}

fn campaign_schema<'a>() -> [Column<'a>; 7] {
    let e: &[u32] = &[];
    [
        Column { name: "campaign_id", data: Data::Str(StrCol { offsets: e, data: &[] }) },
        Column { name: "channel", data: Data::Dict { dict: StrCol { offsets: e, data: &[] }, keys: Keys::U32(e), nullable: false, plain: false } },
        Column { name: "day", data: Data::Date(&[], IntEnc::Dense) },
        Column { name: "sessions", data: Data::I64(&[], IntEnc::Plain) },
        Column { name: "purchasing_sessions", data: Data::I64(&[], IntEnc::Plain) },
        Column { name: "revenue_cents", data: Data::I64(&[], IntEnc::Plain) },
        Column { name: "assisted_sessions", data: Data::I64(&[], IntEnc::Plain) },
    ]
}

unsafe fn any_nonzero(p: *const u64, m: usize) -> bool {
    use std::arch::x86_64::*;
    let mut acc = _mm256_setzero_si256();
    let mut i = 0;
    while i + 4 <= m {
        acc = _mm256_or_si256(acc, _mm256_loadu_si256(p.add(i) as *const __m256i));
        i += 4;
    }
    let mut s = (_mm256_testz_si256(acc, acc) == 0) as u64;
    while i < m {
        s |= *p.add(i);
        i += 1;
    }
    s != 0
}

unsafe fn add_cells(sa: *mut u64, pr: *mut u64, a: *const u32, p: *const u64, n: usize) {
    use std::arch::x86_64::*;
    let m = 2 * n;
    let mut i = 0;
    while i + 4 <= m {
        let x = _mm256_cvtepu32_epi64(_mm_loadu_si128(a.add(i) as *const __m128i));
        let s = _mm256_loadu_si256(sa.add(i) as *const __m256i);
        _mm256_storeu_si256(sa.add(i) as *mut __m256i, _mm256_add_epi64(s, x));
        let y = _mm256_loadu_si256(p.add(i) as *const __m256i);
        let q = _mm256_loadu_si256(pr.add(i) as *const __m256i);
        _mm256_storeu_si256(pr.add(i) as *mut __m256i, _mm256_add_epi64(q, y));
        i += 4;
    }
    while i < m {
        *sa.add(i) += *a.add(i) as u64;
        *pr.add(i) += *p.add(i);
        i += 1;
    }
}

pub fn stage5(g: &mut G, outdir: &str) {
    let ud = Path::new(outdir).join("user_daily");
    let cd = Path::new(outdir).join("campaign_daily");
    if std::fs::create_dir_all(&ud).is_err() || std::fs::create_dir_all(&cd).is_err() {
        die("cannot create the output folders");
    }
    let mut cats_off = vec![0u32];
    let mut cats_data = Vec::new();
    for c in &g.cats {
        cats_data.extend_from_slice(c);
        cats_off.push(cats_data.len() as u32);
    }
    let nt = g.nt;
    let mut base = vec![0usize; nt + 1];
    for t in 0..nt {
        base[t + 1] = base[t] + g.t(t).core.day.len();
    }
    let total = base[nt];
    let np = (2 * nt).min(total).max(1);
    let mut pieces: Vec<(usize, usize, usize)> = Vec::new();
    for p in 0..np {
        let (ga, gb) = (total * p / np, total * (p + 1) / np);
        for t in 0..nt {
            let (a, b) = (ga.max(base[t]), gb.min(base[t + 1]));
            if a < b {
                pieces.push((t, a - base[t], b - base[t]));
            }
        }
    }
    if pieces.is_empty() {
        pieces.push((0, 0, 0));
    }
    let gr: &G = g;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let (pr, nx) = (&pieces, &next);
    sys::run_par(nt, |_| {
        let mut kbuf = Vec::new();
        loop {
            let i = nx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if i == 0 {
                write_campaign(gr, &cd);
            } else if i <= pr.len() {
                let (t, a, b) = pr[i - 1];
                write_piece(gr, t, a, b, &ud.join(format!("part-{}.parquet", i - 1)), &cats_off, &cats_data, &mut kbuf);
            } else {
                break;
            }
        }
    });
}

fn cut(rows: &[(usize, usize)], np: usize) -> Vec<(usize, usize, usize)> {
    let mut base = vec![0usize; rows.len() + 1];
    for (t, &(a, b)) in rows.iter().enumerate() {
        base[t + 1] = base[t] + (b - a);
    }
    let total = base[rows.len()];
    let np = np.min(total).max(1);
    let mut v = Vec::new();
    for p in 0..np {
        let (ga, gb) = (total * p / np, total * (p + 1) / np);
        for t in 0..rows.len() {
            let (a, b) = (ga.max(base[t]), gb.min(base[t + 1]));
            if a < b {
                v.push((t, rows[t].0 + a - base[t], rows[t].0 + b - base[t]));
            }
        }
    }
    v
}

const RPG: usize = crate::env_u32(option_env!("XL_RPG_DEF"), 0) as usize;

const PIECE_MAX: usize = crate::env_u32(option_env!("XL_PIECE_DEF"), 65536) as usize;

fn cut_guided(rows: &[(usize, usize)], nt: usize) -> Vec<(usize, usize, usize)> {
    let total: usize = rows.iter().map(|&(a, b)| b - a).sum();
    let min = (total / (8 * nt)).max(4096);
    let mut v = Vec::new();
    let mut rem = total;
    let (mut t, mut at) = (0usize, rows.first().map_or(0, |r| r.0));
    while rem > 0 {
        let mut want = (rem / (2 * nt)).max(min).min(PIECE_MAX).min(rem);
        rem -= want;
        while want > 0 {
            while at >= rows[t].1 {
                t += 1;
                at = rows[t].0;
            }
            let take = want.min(rows[t].1 - at);
            v.push((t, at, at + take));
            at += take;
            want -= take;
        }
    }
    v
}




pub const EARLY_ROWS: usize = crate::env_u32(option_env!("XL_EARLY_DEF"), 0) as usize;

struct Early {
    done: usize,
    gf: Option<GroupFile>,
}
static mut EARLY: Vec<Early> = Vec::new();
static mut EARLY_DIR: Option<std::path::PathBuf> = None;
static mut EARLY_CATS: (Vec<u32>, Vec<u8>) = (Vec::new(), Vec::new());

pub fn early_init(g: &G, outdir: &str) {
    if EARLY_ROWS == 0 {
        return;
    }
    let ud = Path::new(outdir).join("user_daily");
    if std::fs::create_dir_all(&ud).is_err() {
        return;
    }
    unsafe {
        #[allow(static_mut_refs)]
        {
            EARLY = (0..g.nt).map(|_| Early { done: 0, gf: None }).collect();
            EARLY_DIR = Some(ud);
            let (o, d) = &mut EARLY_CATS;
            o.push(0);
            for c in &g.cats {
                d.extend_from_slice(c);
                o.push(d.len() as u32);
            }
        }
    }
}

fn thread_path(ud: &Path, tid: usize) -> std::path::PathBuf {
    static NEXT: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
    let k = NEXT[tid & 63].fetch_add(1, Ordering::Relaxed);
    if k == 0 { ud.join(format!("part-{tid}.parquet")) } else { ud.join(format!("part-{tid}-{k}.parquet")) }
}

#[inline]
pub fn early(g: &G, tid: usize) {
    #[allow(static_mut_refs)]
    unsafe {
        if EARLY_ROWS == 0 || EARLY.len() <= tid {
            return;
        }
        let e = &mut EARLY[tid];
        let c = &g.t(tid).core;
        let n = c.day.len();
        if n - e.done < EARLY_ROWS {
            return;
        }
        #[cfg(feature = "tl")]
        let t0 = crate::sys::tl::now();
        let ud = EARLY_DIR.as_ref().unwrap();
        let (co, cd) = &EARLY_CATS;
        put_rows(&mut e.gf, &|| thread_path(ud, tid), &view(c), e.done, n, StrCol { offsets: co, data: cd }, g.day0, g.nd);
        e.done = n;
        #[cfg(feature = "tl")]
        { ev::EA.fetch_add(crate::sys::tl::now() - t0, Ordering::Relaxed); ev::EN.fetch_add(1, Ordering::Relaxed); }
    }
}

fn user_schema<'a>() -> [Column<'a>; 6] {
    let e: &[u32] = &[];
    [
        Column { name: "user_id", data: Data::Dict { dict: StrCol { offsets: e, data: &[] }, keys: Keys::U32Off(e, 0), nullable: false, plain: false } },
        Column { name: "day", data: Data::Date(&[], IntEnc::Dense) },
        Column { name: "sessions", data: Data::I32(&[], IntEnc::Dense) },
        Column { name: "events", data: Data::I32(&[], IntEnc::Auto) },
        Column { name: "revenue_cents", data: Data::I32(&[], IntEnc::Plain) },
        Column { name: "top_category", data: Data::Dict { dict: StrCol { offsets: e, data: &[] }, keys: Keys::U8(&[]), nullable: true, plain: false } },
    ]
}

fn put_rows(gf: &mut Option<GroupFile>, path: &dyn Fn() -> std::path::PathBuf, v: &View, a: usize, b: usize, cats: StrCol, day0: i32, nd: usize) {
    use std::slice::from_raw_parts as sl;
    if a >= b {
        return;
    }
    if ROT_BYTES > 0 && gf.as_ref().is_some_and(|f| f.size() >= ROT_BYTES) {
        let f = gf.take().unwrap();
        if f.finish(&user_schema()).is_err() {
            die("user_daily");
        }
    }
    if gf.is_none() {
        match GroupFile::create(&path(), GF_BUF) {
            Ok(f) => *gf = Some(f),
            Err(_) => die("user_daily"),
        }
    }
    let n = b - a;
    let ug = unsafe {
        UserGroup {
            keys: sl(v.keys.add(a), n),
            names: StrCol { offsets: sl(v.noff, v.nnoff), data: sl(v.names, v.nnames) },
            day: sl(v.day.add(a), n),
            ses: sl(v.ses.add(a), n),
            evs: sl(v.evs.add(a), n),
            rev: sl(v.rev.add(a), n),
            cat: sl(v.cat.add(a), n),
            cats,
            day0,
            nd,
        }
    };
    #[cfg(feature = "tl")]
    if std::env::var_os("XL_W6HOT").is_some() {
        let t0 = crate::sys::tl::now();
        let mut s = 0u64;
        unsafe {
            for c in [v.keys as *const u8, v.day as *const u8, v.ses as *const u8, v.evs as *const u8, v.rev as *const u8] {
                let p = c.add(4 * a);
                let mut i = 0;
                while i < 4 * n { s = s.wrapping_add(*p.add(i) as u64); i += 64; }
            }
            let mut i = 0;
            while i < n { s = s.wrapping_add(*v.cat.add(a + i) as u64); i += 64; }
            let (k0, k1) = (*v.keys.add(a) as usize, *v.keys.add(b - 1) as usize);
            if k1 >= k0 && k1 + 1 < v.nnoff {
                let (x, y) = (*v.noff.add(k0) as usize, *v.noff.add(k1 + 1) as usize);
                let mut i = x;
                while i < y { s = s.wrapping_add(*v.names.add(i) as u64); i += 64; }
                let mut i = k0;
                while i <= k1 { s = s.wrapping_add(*v.noff.add(i) as u64); i += 16; }
            }
        }
        std::hint::black_box(s);
        ev::CA.fetch_add(crate::sys::tl::now() - t0, Ordering::Relaxed);
    }
    if gf.as_mut().unwrap().user_group(&ug).is_err() {
        die("user_daily");
    }
}

const ROT_BYTES: u64 = (crate::env_u32(option_env!("XL_ROT_DEF"), 4096) as u64) << 10;
const CF_BUF: usize = 256 << 10;
const GF_BUF: usize = (crate::env_u32(option_env!("XL_GFBUF_DEF"), 1024) as usize) << 10;

#[cfg(feature = "tl")]
pub mod ev {
    pub static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    pub static mut E: [(u8, u16, u8, u64, u64); 512] = [(0, 0, 0, 0, 0); 512];
    pub fn rec(k: u8, i: usize, tid: usize, a: u64) {
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 512 {
            unsafe { E[n] = (k, i as u16, tid as u8, a, crate::sys::tl::now()) };
        }
    }
    pub static WN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub static WB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub static FN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub static FT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub static FA: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub static FE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub static CA: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub static CB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub static NOIO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pub static CC: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub static CD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub static EA: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    pub static EN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
}

pub fn stage45(g: &mut G, outdir: &str) {
    #[cfg(feature = "tl")]
    let (tl_t0, tl_i0) = (crate::sys::tl::now(), std::time::Instant::now());
    #[cfg(feature = "tl")]
    ev::NOIO.store(std::env::var_os("XL_W6NOIO").is_some(), Ordering::Relaxed);
    crate::complex::stage4_pre(g);
    #[cfg(feature = "tl")]
    ev::rec(9, 0, 0, tl_t0);
    let ud = Path::new(outdir).join("user_daily");
    let cd = Path::new(outdir).join("campaign_daily");
    if std::fs::create_dir_all(&ud).is_err() || std::fs::create_dir_all(&cd).is_err() {
        die("cannot create the output folders");
    }
    let mut cats_off = vec![0u32];
    let mut cats_data = Vec::new();
    for c in &g.cats {
        cats_data.extend_from_slice(c);
        cats_off.push(cats_data.len() as u32);
    }
    let nt = g.nt;
    let v3: Vec<View> = (0..nt).map(|t| view(&g.t(t).core)).collect();
    #[allow(static_mut_refs)]
    let done0: Vec<usize> = (0..nt).map(|t| unsafe { EARLY.get(t).map_or(0, |e| e.done) }).collect();
    let pieces3 = cut_guided(&v3.iter().enumerate().map(|(t, v)| (done0[t], v.rows)).collect::<Vec<_>>(), nt);
    let order = crate::complex::shard_order(g);
    g.next.store(0, Ordering::Relaxed);
    let gr: &G = g;
    let gr_nc_plus1 = |g: &G| g.nc + 1;
    let (ca_done, sh_next, sh_done, pq, fq) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    let (ns, np) = (order.len(), pieces3.len());
    let nf = 2 * nt;
    let csum: Vec<std::sync::Mutex<[i64; 4]>> = (0..nt).map(|_| std::sync::Mutex::new([0; 4])).collect();
    let ncp = gr_nc_plus1(g);
    let any_file = std::sync::atomic::AtomicBool::new(false);
    let cfiles: Vec<std::sync::Mutex<(bool, Option<std::fs::File>)>> = (0..nt).map(|_| std::sync::Mutex::new((false, None))).collect();
    run_par(nt, |tid| {
        let mut pre_done = false;
        #[cfg(feature = "tl")]
        let tca = crate::sys::tl::now();
        crate::complex::clicks_a(gr, tid);
        #[cfg(feature = "tl")]
        ev::rec(0, 0, tid, tca);
        ca_done.fetch_add(1, Ordering::AcqRel);
        let mut sc = crate::complex::PScratch::default();
        #[allow(static_mut_refs)]
        let mut gf: Option<GroupFile> = unsafe { EARLY.get_mut(tid).and_then(|e| e.gf.take()) };
        let upath = || thread_path(&ud, tid);
        let cats = StrCol { offsets: &cats_off, data: &cats_data };
        loop {
            if ca_done.load(Ordering::Acquire) == nt && sh_next.load(Ordering::Relaxed) < ns {
                let i = sh_next.fetch_add(1, Ordering::Relaxed);
                if i < ns {
                    #[cfg(feature = "tl")]
                    let t0 = crate::sys::tl::now();
                    crate::complex::person_shard(gr, tid, order[i], &mut sc);
                    #[cfg(feature = "tl")]
                    ev::rec(1, i, tid, t0);
                    #[cfg(feature = "tl")]
                    unsafe {
                        if i < 8 {
                            crate::sys::tl::TASK[i] = (t0, crate::sys::tl::now(), tid as u64);
                        }
                        crate::sys::tl::TASK[63] = (crate::sys::tl::TASK[63].0.max(crate::sys::tl::now()), 0, 0);
                    }
                    sh_done.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
            }
            let shards_done = ca_done.load(Ordering::Acquire) == nt && sh_done.load(Ordering::Acquire) == ns;
            if !shards_done || fq.load(Ordering::Relaxed) >= nf {
                if pq.load(Ordering::Relaxed) < np {
                    let i = pq.fetch_add(1, Ordering::Relaxed);
                    if i < np {
                        let (t, a, b) = pieces3[i];
                        #[cfg(feature = "tl")]
                        let t0 = crate::sys::tl::now();
                        put_rows(&mut gf, &upath, &v3[t], a, b, cats, gr.day0, gr.nd);
                        if !pre_done {
                            pre_done = true;
                            let mut st = cfiles[tid].lock().unwrap();
                            if !st.0 && st.1.is_none() {
                                if let Ok(f) = std::fs::File::create(cd.join(format!("part-{tid}.parquet"))) {
                                    st.1 = Some(f);
                                }
                            }
                        }
                        #[cfg(feature = "tl")]
                        ev::rec(2, i, tid, t0);
                        #[cfg(feature = "tl")]
                        unsafe {
                            crate::sys::tl::TASK[(16 + i).min(47)] = (t0, crate::sys::tl::now(), tid as u64);
                        }
                        continue;
                    }
                }
                if !shards_done {
                    std::hint::spin_loop();
                    continue;
                }
            }
            let i = fq.fetch_add(1, Ordering::Relaxed);
            if i >= nf {
                if pq.load(Ordering::Relaxed) < np {
                    continue;
                }
                if let Some(f) = gf.take() {
                    #[cfg(feature = "tl")]
                    let t0 = crate::sys::tl::now();
                    if f.finish(&user_schema()).is_err() {
                        die("user_daily");
                    }
                    #[cfg(feature = "tl")]
                    ev::rec(5, tid, tid, t0);
                    any_file.store(true, Ordering::Relaxed);
                }
                break;
            }
            #[cfg(feature = "tl")]
            let t0 = crate::sys::tl::now();
            if i < nt {
                let (lo, hi) = (ncp * i / nt, ncp * (i + 1) / nt);
                let pre = {
                    let mut st = cfiles[i].lock().unwrap();
                    st.0 = true;
                    st.1.take()
                };
                let s = write_campaign_part(gr, &cd.join(format!("part-{i}.parquet")), lo, hi, pre, &mut gf);

                *csum[i].lock().unwrap() = s;
                #[cfg(feature = "tl")]
                ev::rec(3, i, tid, t0);
                #[cfg(feature = "tl")]
                unsafe {
                    crate::sys::tl::TASK[8 + i.min(7)] = (t0, crate::sys::tl::now(), tid as u64);
                }
            } else {
                let t = i - nt;
                let v = view(&gr.t(t).core);
                put_rows(&mut gf, &upath, &v, v3[t].rows, v.rows, cats, gr.day0, gr.nd);
                #[cfg(feature = "tl")]
                ev::rec(4, t, tid, t0);
            }
        }
    });
    #[cfg(feature = "tl")]
    {
        let t1 = crate::sys::tl::now();
        let tpm = (t1 - tl_t0) as f64 / (tl_i0.elapsed().as_secs_f64() * 1e3);
        let n = ev::N.load(Ordering::Relaxed).min(512);
        let mut v: Vec<_> = unsafe { ev::E[..n].to_vec() };
        v.sort_by_key(|e| (e.2, e.3));
        let names = ["clk", "shd", "pc", "cmp", "fin", "end", "", "", "", "pre"];
        let mut s = String::new();
        let mut busy = [[0f64; 5]; 16];
        let mut last = 0u8;
        for e in &v {
            if e.2 != last || s.is_empty() { s.push_str(&format!("\n[ev] t{}:", e.2)); last = e.2; }
            let (a, b) = ((e.3 - tl_t0) as f64 / tpm, (e.4 - tl_t0) as f64 / tpm);
            s.push_str(&format!(" {}{} {:.2}-{:.2}", names[e.0 as usize], e.1, a, b));
            if e.0 < 5 { busy[e.2 as usize][e.0 as usize] += b - a; } else if e.0 == 5 { busy[e.2 as usize][4] += b - a; }
        }
        let mut tot = [0f64; 5];
        for t in 0..16 { for k in 0..5 { tot[k] += busy[t][k]; } }
        let ld = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed) as f64 / tpm;
        eprintln!("[ev] early groups {} thread-ms {:.2} | campaign agg {:.2} build {:.2} create {:.2} encode+write {:.2}", ev::EN.load(Ordering::Relaxed), ld(&ev::EA), ld(&ev::CA), ld(&ev::CB), ld(&ev::CC), ld(&ev::CD));
        eprintln!("[ev] stage45 wall {:.3} ms; thread-ms clk {:.2} shd {:.2} pc {:.2} cmp {:.2} fin {:.2}; writes {} bytes {} files {}{s}", (t1 - tl_t0) as f64 / tpm, tot[0], tot[1], tot[2], tot[3], tot[4], ev::WN.load(Ordering::Relaxed), ev::WB.load(Ordering::Relaxed), ev::FN.load(Ordering::Relaxed));
    }
    #[cfg(feature = "tl")]
    if let Some(reps) = std::env::var("XL_W6BENCH").ok().and_then(|s| s.parse::<usize>().ok()) {
        let cats = StrCol { offsets: &cats_off, data: &cats_data };
        let mut gf = GroupFile::mem(64 << 20);
        let mut best = [u64::MAX; 8];
        for _ in 0..reps {
            for j in 0..8 { crate::sys::tl::WR_COL[j].store(0, Ordering::Relaxed); }
            for t in 0..nt {
                let v = view(&gr.t(t).core);
                let mut a = 0;
                while a < v.rows {
                    let b = (a + 32768).min(v.rows);
                    let n = b - a;
                    let ug = unsafe { use std::slice::from_raw_parts as sl; UserGroup { keys: sl(v.keys.add(a), n), names: StrCol { offsets: sl(v.noff, v.nnoff), data: sl(v.names, v.nnames) }, day: sl(v.day.add(a), n), ses: sl(v.ses.add(a), n), evs: sl(v.evs.add(a), n), rev: sl(v.rev.add(a), n), cat: sl(v.cat.add(a), n), cats, day0: gr.day0, nd: gr.nd } };
                    let _ = gf.user_group(&ug);
                    a = b;
                    if gf.o_len() > (48 << 20) { gf.reset(); }
                }
            }
            gf.reset();
            for j in 0..8 { best[j] = best[j].min(crate::sys::tl::WR_COL[j].load(Ordering::Relaxed)); }
        }
        let tpm = { let t0 = crate::sys::tl::now(); let i0 = std::time::Instant::now(); while i0.elapsed().as_micros() < 2000 {} (crate::sys::tl::now() - t0) as f64 / 2.0 };
        eprintln!("[bench] hot encode, min of {reps} (thread-ms): user_id {:.2} day {:.2} ses+evs {:.2} rev {:.2} cat {:.2} stats {:.2} check {:.2}", best[0] as f64 / tpm, best[1] as f64 / tpm, best[2] as f64 / tpm, best[4] as f64 / tpm, best[5] as f64 / tpm, best[7] as f64 / tpm, best[6] as f64 / tpm);
    }
    if !any_file.load(Ordering::Relaxed) {
        if GroupFile::create(&ud.join("part-0.parquet"), 1 << 16).and_then(|f| f.finish(&user_schema())).is_err() {
            die("user_daily");
        }
    }
    if gr.check {
        let (mut us, mut ur) = (0i64, 0i64);
        for tid in 0..nt {
            let c = &gr.t(tid).core;
            us += c.ses.as_slice().iter().map(|&x| x as i64).sum::<i64>();
            ur += c.rev.as_slice().iter().map(|&x| x as i64).sum::<i64>();
        }
        let mut s = [0i64; 4];
        for m in &csum {
            let x = *m.lock().unwrap();
            for k in 0..4 {
                s[k] += x[k];
            }
        }
        let [cs, cr, ca, organic] = s;
        eprintln!("[v2b] check: sessions user={us} campaign={cs} revenue user={ur} campaign={cr} assisted={ca} credited_non_organic={}", cs - organic);
        if us != cs || ur != cr || ca < cs - organic {
            eprintln!("[v2b] CHECK FAILED");
            unsafe { sys::_exit(5) }
        }
    }
}
