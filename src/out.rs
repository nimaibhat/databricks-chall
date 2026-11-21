//! Output accumulators and the two Parquet tables.
use std::path::Path;

use crate::keys::{key_bytes, Key};
use crate::pq::{self, Column, Data};
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

fn write_users(path: &Path, rows: &[URow], cats: &[Vec<u8>]) {
    let n = rows.len();
    let mut ub = Vec::with_capacity(n * 10);
    let mut ue = Vec::with_capacity(n);
    let mut cb = Vec::with_capacity(n * 4);
    let mut ce = Vec::with_capacity(n);
    let mut def = Vec::with_capacity(n);
    let mut days = Vec::with_capacity(n);
    let mut se = Vec::with_capacity(n);
    let mut ev = Vec::with_capacity(n);
    let mut rv = Vec::with_capacity(n);
    for r in rows {
        key_bytes(r.person, &mut ub);
        ue.push(ub.len() as u32);
        days.push(r.day);
        se.push(r.sessions as i32);
        ev.push(r.events as i32);
        rv.push(r.revenue as i64);
        if r.top == u16::MAX {
            def.push(false);
        } else {
            def.push(true);
            cb.extend_from_slice(&cats[r.top as usize]);
            ce.push(cb.len() as u32);
        }
    }
    let cols = [
        Column { name: "user_id", data: Data::Str { bytes: ub, ends: ue, def: None } },
        Column { name: "day", data: Data::Date(days) },
        Column { name: "sessions", data: Data::I32(se) },
        Column { name: "events", data: Data::I32(ev) },
        Column { name: "revenue_cents", data: Data::I64(rv) },
        Column { name: "top_category", data: Data::Str { bytes: cb, ends: ce, def: Some(def) } },
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
            let (mut ib, mut ie, mut chb, mut che) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let (mut days, mut c0, mut c1, mut c2, mut c3) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for c in 0..=ncamp {
                for d in 0..ndays {
                    let v = camp[c * ndays + d];
                    if v[0] == 0 && v[3] == 0 {
                        continue;
                    }
                    if c == ncamp {
                        ib.extend_from_slice(b"organic");
                        chb.extend_from_slice(b"organic");
                    } else {
                        ib.extend_from_slice(&ctx.camp_names[c]);
                        chb.extend_from_slice(&ctx.camp_channels[c]);
                    }
                    ie.push(ib.len() as u32);
                    che.push(chb.len() as u32);
                    days.push((ctx.day0 + d as i64) as i32);
                    c0.push(v[0] as i64);
                    c1.push(v[1] as i64);
                    c2.push(v[2] as i64);
                    c3.push(if c == ncamp { 0 } else { v[3] as i64 });
                }
            }
            let n = days.len();
            let cols = [
                Column { name: "campaign_id", data: Data::Str { bytes: ib, ends: ie, def: None } },
                Column { name: "channel", data: Data::Str { bytes: chb, ends: che, def: None } },
                Column { name: "day", data: Data::Date(days) },
                Column { name: "sessions", data: Data::I64(c0) },
                Column { name: "purchasing_sessions", data: Data::I64(c1) },
                Column { name: "revenue_cents", data: Data::I64(c2) },
                Column { name: "assisted_sessions", data: Data::I64(c3) },
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
