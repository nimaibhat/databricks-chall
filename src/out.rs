//! Output accumulators and the two Parquet tables.
use std::path::Path;

use crate::keys::{key_bytes, KMap, Key};
use crate::pq::{self, Column, Enc, Idx, Logical, Values};
use crate::Ctx;

pub struct URow {
    pub person: Key,
    pub day: i32,
    pub sessions: u32,
    pub events: u32,
    pub revenue: u64,
    pub top: u16,
}

pub struct Acc {
    pub camp: Vec<[u64; 4]>, // sessions, purchasing, revenue, assisted
    pub ndays: usize,
    pub users: Vec<URow>,
}

impl Acc {
    pub fn new(ncamp: usize, ndays: usize) -> Acc {
        Acc { camp: vec![[0; 4]; ncamp * ndays], ndays, users: Vec::new() }
    }
    #[inline(always)]
    pub fn camp_cell(&mut self, c: usize, d: usize) -> &mut [u64; 4] {
        unsafe { self.camp.get_unchecked_mut(c * self.ndays + d) }
    }
}

/// Largest count that still gets the identity dictionary (value = index).
const DICT_MAX: u32 = 1 << 20;

/// A count column: dictionary 0..=max with the value as its own index when
/// the values are small (the indices are then bit-packed per page), PLAIN
/// otherwise.
fn count_column(name: &'static str, vals: Vec<u32>) -> Column {
    let max = vals.iter().copied().max().unwrap_or(0);
    let enc = if max <= DICT_MAX {
        Enc::Dict { dict: Values::I32((0..=max as i32).collect()), idx: Idx::Each(vals) }
    } else {
        Enc::Plain(Values::I32(vals.into_iter().map(|v| v as i32).collect()))
    };
    Column { name, logical: Logical::None, def: None, enc }
}

fn write_users(path: &Path, rows: &[URow], cats: &[Vec<u8>]) {
    let n = rows.len();
    // user_id: one dictionary entry per run of rows of the same person
    let mut ub = Vec::with_capacity(n * 4);
    let mut ue = Vec::with_capacity(n / 2);
    let mut runs: Vec<(u32, u32)> = Vec::with_capacity(n / 2);
    let mut prev: Option<Key> = None;
    // day: dictionary of the days between the smallest and largest present
    let dmin = rows.iter().map(|r| r.day).min().unwrap_or(0);
    let dmax = rows.iter().map(|r| r.day).max().unwrap_or(0);
    let mut days = Vec::with_capacity(n);
    let mut se = Vec::with_capacity(n);
    let mut ev = Vec::with_capacity(n);
    let mut rv = Vec::with_capacity(n);
    let mut ri: Vec<u32> = Vec::with_capacity(n);
    let mut rdict: Vec<i64> = vec![0];
    let mut rmap = KMap::<u32>::with_capacity(n / 8 + 16);
    rmap.entry(0, 0);
    let mut def = Vec::with_capacity(n);
    let mut ci = Vec::new();
    for r in rows {
        if prev == Some(r.person) {
            runs.last_mut().unwrap().1 += 1;
        } else {
            key_bytes(r.person, &mut ub);
            ue.push(ub.len() as u32);
            runs.push((ue.len() as u32 - 1, 1));
            prev = Some(r.person);
        }
        days.push((r.day - dmin) as u32);
        se.push(r.sessions);
        ev.push(r.events);
        // revenue: 0 for most rows; a dictionary in order of first appearance
        let (slot, new) = rmap.entry(r.revenue, rdict.len() as u32);
        if new {
            rdict.push(r.revenue as i64);
        }
        ri.push(*rmap.val_mut(slot));
        rv.push(r.revenue as i64);
        if r.top == u16::MAX {
            def.push(false);
        } else {
            def.push(true);
            ci.push(r.top as u32);
        }
    }
    let mut cb = Vec::new();
    let mut ce = Vec::new();
    for c in cats {
        cb.extend_from_slice(c);
        ce.push(cb.len() as u32);
    }
    let cols = [
        Column { name: "user_id", logical: Logical::Utf8, def: None, enc: Enc::Dict { dict: Values::Str { bytes: ub, ends: ue }, idx: Idx::Runs(runs) } },
        Column { name: "day", logical: Logical::Date, def: None, enc: Enc::Dict { dict: Values::I32((dmin..=dmax).collect()), idx: Idx::Each(days) } },
        count_column("sessions", se),
        count_column("events", ev),
        // dictionary only when it is clearly smaller than 8 bytes a row
        if rdict.len() * 8 + n * 3 < n * 6 {
            Column { name: "revenue_cents", logical: Logical::None, def: None, enc: Enc::Dict { dict: Values::I64(rdict), idx: Idx::Each(ri) } }
        } else {
            Column { name: "revenue_cents", logical: Logical::None, def: None, enc: Enc::Plain(Values::I64(rv)) }
        },
        Column { name: "top_category", logical: Logical::Utf8, def: Some(def), enc: Enc::Dict { dict: Values::Str { bytes: cb, ends: ce }, idx: Idx::Each(ci) } },
    ];
    pq::write(path, &cols, n).expect("write user_daily");
}

pub fn write_all(outdir: &Path, ctx: &Ctx, mut accs: Vec<Acc>) {
    let ud = outdir.join("user_daily");
    let cd = outdir.join("campaign_daily");
    std::fs::create_dir_all(&ud).expect("mkdir");
    std::fs::create_dir_all(&cd).expect("mkdir");

    let mut camp = std::mem::take(&mut accs[0].camp);
    for a in accs.iter().skip(1) {
        for (x, y) in camp.iter_mut().zip(a.camp.iter()) {
            for k in 0..4 {
                x[k] += y[k];
            }
        }
    }
    let ndays = ctx.ndays;
    let users: Vec<Vec<URow>> = accs.into_iter().map(|a| a.users).collect();
    std::thread::scope(|s| {
        let camp = &camp;
        let cd = &cd;
        s.spawn(move || {
            let ncamp = ctx.camp_names.len();
            // rows come campaign by campaign: campaign_id and channel are
            // dictionary runs, day a small dictionary; counts are INT32 when
            // they fit (any integer width is accepted)
            let (mut ib, mut ie, mut iruns) = (Vec::new(), Vec::new(), Vec::<(u32, u32)>::new());
            let (mut chb, mut che, mut chruns) = (Vec::new(), Vec::new(), Vec::<(u32, u32)>::new());
            let mut chan_ix: Vec<(Vec<u8>, u32)> = Vec::new();
            let (mut days, mut c0, mut c1, mut c2, mut c3) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for c in 0..=ncamp {
                let mut first = true;
                for d in 0..ndays {
                    let v = camp[c * ndays + d];
                    if v[0] == 0 && v[3] == 0 {
                        continue;
                    }
                    let (name, chan): (&[u8], &[u8]) = if c == ncamp { (b"organic", b"organic") } else { (&ctx.camp_names[c], &ctx.camp_channels[c]) };
                    if first {
                        first = false;
                        ib.extend_from_slice(name);
                        ie.push(ib.len() as u32);
                        iruns.push((ie.len() as u32 - 1, 0));
                        let ci = match chan_ix.iter().find(|x| x.0 == chan) {
                            Some(x) => x.1,
                            None => {
                                chb.extend_from_slice(chan);
                                che.push(chb.len() as u32);
                                chan_ix.push((chan.to_vec(), che.len() as u32 - 1));
                                che.len() as u32 - 1
                            }
                        };
                        match chruns.last_mut() {
                            Some(r) if r.0 == ci => {}
                            _ => chruns.push((ci, 0)),
                        }
                    }
                    iruns.last_mut().unwrap().1 += 1;
                    chruns.last_mut().unwrap().1 += 1;
                    days.push(d as u32);
                    c0.push(v[0] as i64);
                    c1.push(v[1] as i64);
                    c2.push(v[2] as i64);
                    c3.push(if c == ncamp { 0 } else { v[3] as i64 });
                }
            }
            let n = days.len();
            let ints = |name: &'static str, v: Vec<i64>| -> Column {
                let enc = if v.iter().all(|&x| (0..=i32::MAX as i64).contains(&x)) {
                    Enc::Plain(Values::I32(v.into_iter().map(|x| x as i32).collect()))
                } else {
                    Enc::Plain(Values::I64(v))
                };
                Column { name, logical: Logical::None, def: None, enc }
            };
            let cols = [
                Column { name: "campaign_id", logical: Logical::Utf8, def: None, enc: Enc::Dict { dict: Values::Str { bytes: ib, ends: ie }, idx: Idx::Runs(iruns) } },
                Column { name: "channel", logical: Logical::Utf8, def: None, enc: Enc::Dict { dict: Values::Str { bytes: chb, ends: che }, idx: Idx::Runs(chruns) } },
                Column { name: "day", logical: Logical::Date, def: None, enc: Enc::Dict { dict: Values::I32((0..ndays as i32).map(|d| ctx.day0 as i32 + d).collect()), idx: Idx::Each(days) } },
                ints("sessions", c0),
                ints("purchasing_sessions", c1),
                ints("revenue_cents", c2),
                ints("assisted_sessions", c3),
            ];
            pq::write(&cd.join("part-0.parquet"), &cols, n).expect("write campaign_daily");
        });
        for (i, rows) in users.iter().enumerate() {
            if rows.is_empty() && i != 0 {
                continue;
            }
            let ud = &ud;
            s.spawn(move || write_users(&ud.join(format!("part-{i}.parquet")), rows, &ctx.cat_names));
        }
    });
}
