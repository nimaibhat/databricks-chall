use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use parquet::basic::Compression;
use parquet::data_type::{ByteArray, ByteArrayType, Int32Type, Int64Type};
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;
use parquet::schema::types::Type;

use crate::{key_bytes, Ctx, Key};

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
        &mut self.camp[c * self.ndays + d]
    }
}

fn props() -> Arc<WriterProperties> {
    Arc::new(
        WriterProperties::builder()
            .set_writer_version(WriterVersion::PARQUET_1_0)
            .set_compression(Compression::UNCOMPRESSED)
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .set_data_page_size_limit(8 << 20)
            .set_write_batch_size(64 * 1024)
            .build(),
    )
}

fn user_schema() -> Arc<Type> {
    Arc::new(
        parse_message_type(
            "message user_daily {
                REQUIRED BYTE_ARRAY user_id (UTF8);
                REQUIRED INT32 day (DATE);
                REQUIRED INT64 sessions;
                REQUIRED INT64 events;
                REQUIRED INT64 revenue_cents;
                OPTIONAL BYTE_ARRAY top_category (UTF8);
            }",
        )
        .unwrap(),
    )
}

fn camp_schema() -> Arc<Type> {
    Arc::new(
        parse_message_type(
            "message campaign_daily {
                REQUIRED BYTE_ARRAY campaign_id (UTF8);
                REQUIRED BYTE_ARRAY channel (UTF8);
                REQUIRED INT32 day (DATE);
                REQUIRED INT64 sessions;
                REQUIRED INT64 purchasing_sessions;
                REQUIRED INT64 revenue_cents;
                REQUIRED INT64 assisted_sessions;
            }",
        )
        .unwrap(),
    )
}

enum Col {
    Bytes(Vec<ByteArray>, Option<Vec<i16>>),
    I32(Vec<i32>),
    I64(Vec<i64>),
}

fn write_file(path: &Path, schema: Arc<Type>, cols: Vec<Col>) {
    let f = File::create(path).expect("create parquet");
    let w = BufWriter::with_capacity(1 << 20, f);
    let mut fw = SerializedFileWriter::new(w, schema, props()).expect("writer");
    let mut rg = fw.next_row_group().expect("rg");
    let mut it = cols.into_iter();
    while let Some(mut c) = rg.next_column().expect("col") {
        match it.next().expect("column data") {
            Col::Bytes(v, def) => {
                c.typed::<ByteArrayType>().write_batch(&v, def.as_deref(), None).expect("w");
            }
            Col::I32(v) => {
                c.typed::<Int32Type>().write_batch(&v, None, None).expect("w");
            }
            Col::I64(v) => {
                c.typed::<Int64Type>().write_batch(&v, None, None).expect("w");
            }
        }
        c.close().expect("close col");
    }
    rg.close().expect("close rg");
    fw.close().expect("close file");
}

pub fn write_all(outdir: &Path, ctx: &Ctx, mut accs: Vec<Acc>, threads: usize) {
    let ud = outdir.join("user_daily");
    let cd = outdir.join("campaign_daily");
    std::fs::create_dir_all(&ud).expect("mkdir");
    std::fs::create_dir_all(&cd).expect("mkdir");

    // merge campaign accumulators
    let mut camp = std::mem::take(&mut accs[0].camp);
    for a in accs.iter().skip(1) {
        for (x, y) in camp.iter_mut().zip(a.camp.iter()) {
            for k in 0..4 {
                x[k] += y[k];
            }
        }
    }
    let ndays = ctx.ndays;
    let cat_bytes: Vec<ByteArray> =
        ctx.cat_names.iter().map(|c| ByteArray::from(Bytes::from(c.clone()))).collect();

    let users: Vec<Vec<URow>> = accs.into_iter().map(|a| a.users).collect();
    std::thread::scope(|s| {
        // campaign_daily
        let camp = &camp;
        let cd = &cd;
        s.spawn(move || {
            let ncamp = ctx.camp_names.len();
            let mut ids = Vec::new();
            let mut chans = Vec::new();
            let mut days = Vec::new();
            let mut c0 = Vec::new();
            let mut c1 = Vec::new();
            let mut c2 = Vec::new();
            let mut c3 = Vec::new();
            let organic = ByteArray::from(Bytes::from_static(b"organic"));
            let names: Vec<(ByteArray, ByteArray)> = ctx
                .camp_names
                .iter()
                .zip(ctx.camp_channels.iter())
                .map(|(n, c)| (ByteArray::from(Bytes::from(n.clone())), ByteArray::from(Bytes::from(c.clone()))))
                .collect();
            for c in 0..=ncamp {
                for d in 0..ndays {
                    let v = camp[c * ndays + d];
                    if v[0] == 0 && v[3] == 0 {
                        continue;
                    }
                    if c == ncamp {
                        ids.push(organic.clone());
                        chans.push(organic.clone());
                    } else {
                        ids.push(names[c].0.clone());
                        chans.push(names[c].1.clone());
                    }
                    days.push((ctx.day0 + d as i64) as i32);
                    c0.push(v[0] as i64);
                    c1.push(v[1] as i64);
                    c2.push(v[2] as i64);
                    c3.push(if c == ncamp { 0 } else { v[3] as i64 });
                }
            }
            write_file(
                &cd.join("part-0.parquet"),
                camp_schema(),
                vec![
                    Col::Bytes(ids, None),
                    Col::Bytes(chans, None),
                    Col::I32(days),
                    Col::I64(c0),
                    Col::I64(c1),
                    Col::I64(c2),
                    Col::I64(c3),
                ],
            );
        });
        // user_daily, one file per accumulator
        let _ = threads;
        for (i, rows) in users.iter().enumerate() {
            if rows.is_empty() && i != 0 {
                continue;
            }
            let ud = &ud;
            let cat_bytes = &cat_bytes;
            s.spawn(move || {
                let n = rows.len();
                let mut buf: Vec<u8> = Vec::with_capacity(n * 10);
                let mut ends: Vec<usize> = Vec::with_capacity(n);
                for r in rows {
                    key_bytes(r.person, &mut buf);
                    ends.push(buf.len());
                }
                let big = Bytes::from(buf);
                let mut ids = Vec::with_capacity(n);
                let mut st = 0;
                for &e in &ends {
                    ids.push(ByteArray::from(big.slice(st..e)));
                    st = e;
                }
                let mut days = Vec::with_capacity(n);
                let mut se = Vec::with_capacity(n);
                let mut ev = Vec::with_capacity(n);
                let mut rv = Vec::with_capacity(n);
                let mut tops = Vec::new();
                let mut def = Vec::with_capacity(n);
                for r in rows {
                    days.push(r.day);
                    se.push(r.sessions as i64);
                    ev.push(r.events as i64);
                    rv.push(r.revenue as i64);
                    if r.top == u16::MAX {
                        def.push(0i16);
                    } else {
                        def.push(1i16);
                        tops.push(cat_bytes[r.top as usize].clone());
                    }
                }
                write_file(
                    &ud.join(format!("part-{i}.parquet")),
                    user_schema(),
                    vec![
                        Col::Bytes(ids, None),
                        Col::I32(days),
                        Col::I64(se),
                        Col::I64(ev),
                        Col::I64(rv),
                        Col::Bytes(tops, Some(def)),
                    ],
                );
            });
        }
    });
}
