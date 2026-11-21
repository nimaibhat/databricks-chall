//! IDs as the public generator writes them: a prefix letter and the lower
//! case base-36 form of `scramble(number, key)`, a bijection on 40 bits.
//! Undoing it turns an ID into a small dense number (a user's
//! `index * 512 + alias`, an event's or click's running index).
//!
//! Nothing here decides what the answer is. An ID that does not have the
//! canonical form, or whose number is out of the expected range, is simply
//! not "dense" and is handled by the general path on its bytes.

pub const BITS: u32 = 40;
pub const M40: u64 = (1 << BITS) - 1;
const MUL_A: u64 = 0x9E37_79B9_7F4B;
const MUL_B: u64 = 0xD6E8_FEB8_6659;
/// A user's alias numbers share one index: `number = index * 512 + alias`.
pub const ALIAS_BITS: u32 = 9;

const fn inverse(a: u64) -> u64 {
    // Newton's iteration for the inverse of an odd number modulo 2^64
    let mut x = a;
    let mut i = 0;
    while i < 6 {
        x = x.wrapping_mul(2u64.wrapping_sub(a.wrapping_mul(x)));
        i += 1;
    }
    x
}
const INV_A: u64 = inverse(MUL_A);
const INV_B: u64 = inverse(MUL_B);

/// The generator's mix (text.rs): multiply, xor the key, xor-shift, multiply.
#[inline(always)]
pub fn scramble(value: u64, key: u64) -> u64 {
    let mut m = value & M40;
    m = (m.wrapping_mul(MUL_A) ^ key) & M40;
    m ^= m >> 17;
    m.wrapping_mul(MUL_B) & M40
}

/// `y` with the last multiply and the xor-shift undone: `(number * A) ^ key`
/// in 40 bits. Key recovery works on these.
#[inline(always)]
pub fn unmix(y: u64) -> u64 {
    let m = y.wrapping_mul(INV_B) & M40;
    // x ^ (x >> 17) is undone by x ^ (x >> 17) ^ (x >> 34) in 40 bits
    m ^ (m >> 17) ^ (m >> 34)
}

#[inline(always)]
pub fn unscramble(y: u64, key: u64) -> u64 {
    ((unmix(y) ^ key) & M40).wrapping_mul(INV_A) & M40
}

/// The number written in `s` if `s` is exactly what the generator's `base36`
/// writes for a 40-bit value: 1 to 8 characters of `0-9a-z`, no leading zero
/// unless it is the single character `0`, value below 2^40. For such strings
/// decoding and encoding are inverse, so equal numbers mean equal bytes.
#[inline(always)]
pub fn dec36(s: &[u8]) -> Option<u64> {
    let n = s.len();
    if n == 0 || n > 8 || (s[0] == b'0' && n > 1) {
        return None;
    }
    let mut v: u64 = 0;
    for &c in s {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'z' => c - b'a' + 10,
            _ => return None,
        };
        v = v * 36 + d as u64;
    }
    if v > M40 {
        return None;
    }
    Some(v)
}

/// Appends the base-36 form of `v` (the generator's `base36`).
pub fn enc36(v: u64, out: &mut Vec<u8>) {
    const D: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf = [0u8; 13];
    let mut at = buf.len();
    let mut rest = v;
    loop {
        at -= 1;
        buf[at] = D[(rest % 36) as usize];
        rest /= 36;
        if rest == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[at..]);
}

/// The scrambled value of an ID with the given prefix letter, if it has the
/// generator's form.
#[inline(always)]
pub fn id_value(field: &[u8], prefix: u8) -> Option<u64> {
    if field.len() < 2 || field[0] != prefix {
        return None;
    }
    dec36(&field[1..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: u64) -> impl FnMut() -> u64 {
        let mut x = seed;
        move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        }
    }

    #[test]
    fn scramble_round_trips() {
        let mut r = rng(0x9E3779B97F4A7C15);
        for i in 0..2_000_000u64 {
            let key = r();
            let v = if i < 4096 { i } else { r() & M40 };
            let y = scramble(v, key);
            assert!(y <= M40);
            assert_eq!(unscramble(y, key), v);
            assert_eq!(unmix(y), (v.wrapping_mul(MUL_A) ^ key) & M40);
        }
    }

    #[test]
    fn base36_round_trips_and_rejects() {
        let mut r = rng(77);
        let mut s = Vec::new();
        for i in 0..2_000_000u64 {
            let v = if i < 100_000 { i } else { (r() & M40) >> (i % 41) };
            s.clear();
            enc36(v, &mut s);
            assert_eq!(dec36(&s), Some(v));
            // any change that keeps the bytes different must not decode to v
            let mut t = s.clone();
            let at = (r() % t.len() as u64) as usize;
            t[at] = t[at].to_ascii_uppercase();
            if t != s {
                assert_eq!(dec36(&t), None);
            }
            let mut z = b"0".to_vec();
            z.extend_from_slice(&s);
            assert_eq!(dec36(&z), None, "leading zero");
        }
        assert_eq!(dec36(b""), None);
        assert_eq!(dec36(b"zzzzzzzzz"), None);
        assert_eq!(dec36(b"zzzzzzzz"), None, "above 2^40");
        assert_eq!(dec36(b"a b"), None);
        assert_eq!(dec36(b"0"), Some(0));
        assert_eq!(id_value(b"u0", b'u'), Some(0));
        assert_eq!(id_value(b"U0", b'u'), None);
        assert_eq!(id_value(b"u", b'u'), None);
    }
}

// ---------------------------------------------------------------------------
// Key recovery
// ---------------------------------------------------------------------------
/// One sampled event row: the unmixed event ID, the unmixed user ID if it
/// has the generator's form, and the timestamp if it parsed.
#[derive(Clone, Copy)]
pub struct SampleRow {
    pub ev: u64,
    pub user: Option<u64>,
    pub ts: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keys {
    pub user: u64,
    pub event: u64,
}

/// Largest user index and event number the fast path is prepared for.
pub const USER_INDEX_LIMIT: u64 = 1 << 23;
pub const EVENT_LIMIT: u64 = 1 << 28;
/// Kiosk (shared device) IDs come from numbers at and above this.
pub const KIOSK_BASE: u64 = 1 << 39;
/// A person has at most this many aliases in the dense tables.
pub const MAX_ALIAS: u64 = 14;

#[inline(always)]
fn key_of(x: u64, number: u64) -> u64 {
    (x ^ number.wrapping_mul(MUL_A)) & M40
}
#[inline(always)]
fn number_of(x: u64, key: u64) -> u64 {
    ((x ^ key) & M40).wrapping_mul(INV_A) & M40
}
fn user_plausible(v: u64) -> bool {
    ((v >> ALIAS_BITS) < USER_INDEX_LIMIT && (v & ((1 << ALIAS_BITS) - 1)) < 64) || (KIOSK_BASE..KIOSK_BASE + 256).contains(&v)
}

/// The keys, if the sample looks like the generator's output; `None` means
/// "use the general path", never a wrong answer.
///
/// User key: the busiest users are the first few indexes (the whales), and
/// rows mostly carry a person's first ID, so one of the most frequent user
/// IDs in the sample is number `i * 512` for a small `i`; each such guess
/// gives a key, and the right one makes every sampled user plausible.
/// Event key: user 0's events are numbers 0, 1, 2, ... in time order, so
/// its earliest sampled event has a small number; search it upward.
pub fn recover_keys(rows: &[SampleRow], window_start: i64) -> Option<Keys> {
    let mut users: Vec<u64> = rows.iter().filter_map(|r| r.user).collect();
    users.sort_unstable();
    let mut runs: Vec<(usize, u64)> = Vec::new();
    for &u in &users {
        match runs.last_mut() {
            Some(l) if l.1 == u => l.0 += 1,
            _ => runs.push((1, u)),
        }
    }
    if runs.len() < 20 {
        return None;
    }
    let distinct: Vec<u64> = runs.iter().map(|r| r.1).collect();
    runs.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut user_key: Option<u64> = None;
    for &(_, x) in runs.iter().take(16) {
        for i in 0..64u64 {
            let key = key_of(x, i << ALIAS_BITS);
            if distinct.iter().all(|&d| user_plausible(number_of(d, key))) {
                if user_key.is_some_and(|k| k != key) {
                    return None; // two keys fit: not the data we think it is
                }
                user_key = Some(key);
            }
        }
    }
    let user_key = user_key?;

    let mut evs: Vec<u64> = rows.iter().map(|r| r.ev).collect();
    evs.sort_unstable();
    evs.dedup();
    if evs.len() < 20 {
        return None;
    }
    // sampled events of user index 0, earliest first
    let mut zero: Vec<(i64, u64)> = rows
        .iter()
        .filter(|r| r.user.is_some_and(|u| number_of(u, user_key) >> ALIAS_BITS == 0))
        .map(|r| (r.ts.filter(|&t| t >= window_start).unwrap_or(i64::MAX), r.ev))
        .collect();
    zero.sort_unstable();
    zero.dedup_by_key(|z| z.1);
    let slack = evs.len() / 1000;
    let fits = |key: u64| evs.iter().filter(|&&e| number_of(e, key) >= EVENT_LIMIT).count() <= slack;
    let search = |x1: u64, limit: u64| -> Option<u64> {
        let (p2, p3) = (*evs.iter().find(|&&e| e != x1)?, *evs.iter().rev().find(|&&e| e != x1)?);
        (0..limit).map(|j| key_of(x1, j)).find(|&key| number_of(p2, key) < EVENT_LIMIT && number_of(p3, key) < EVENT_LIMIT && fits(key))
    };
    for (take, limit) in [(4usize, 1u64 << 16), (2, 1 << 27)] {
        for &(_, x1) in zero.iter().take(take) {
            if let Some(event) = search(x1, limit) {
                return Some(Keys { user: user_key, event });
            }
        }
    }
    None
}

#[cfg(test)]
mod key_tests {
    use super::*;

    /// A sample shaped like the generator's: whales at the first indexes
    /// with many rows, everyone else few; events numbered per user in
    /// consecutive ranges.
    fn sample(seed: u64, n: usize) -> (Vec<SampleRow>, Keys) {
        let mut x = seed | 1;
        let mut r = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let keys = Keys { user: r() & M40, event: r() & M40 };
        let users = 200_000u64;
        let per_whale = 30_000u64;
        let mut rows = Vec::new();
        for _ in 0..n {
            let (index, ev) = if r() % 100 < 8 {
                let w = r() % 5;
                (w, w * per_whale + r() % per_whale)
            } else {
                let u = 5 + r() % (users - 5);
                (u, 5 * per_whale + u * 30 + r() % 30)
            };
            let alias = if r() % 2 == 0 { 0 } else { r() % 4 };
            rows.push(SampleRow {
                ev: unmix(scramble(ev, keys.event)),
                user: if r() % 500 == 0 { None } else { Some(unmix(scramble((index << ALIAS_BITS) + alias, keys.user))) },
                ts: Some(1_000_000 + ev as i64),
            });
        }
        (rows, keys)
    }

    #[test]
    fn keys_are_recovered() {
        for seed in 1..40u64 {
            let (rows, keys) = sample(seed * 7919, 5000);
            assert_eq!(recover_keys(&rows, 0), Some(keys), "seed {seed}");
        }
    }

    #[test]
    fn no_keys_from_other_ids() {
        // IDs that are valid base 36 but not the generator's numbers
        let mut x = 0xDEADBEEFu64;
        let mut r = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..20 {
            let rows: Vec<SampleRow> = (0..5000).map(|i| SampleRow { ev: unmix(r() & M40), user: Some(unmix((r() % 3000) * 0x9E37_79B9 & M40)), ts: Some(i) }).collect();
            assert_eq!(recover_keys(&rows, 0), None);
        }
        // sequential IDs (u1, u2, ...) as hand-made data has
        let rows: Vec<SampleRow> = (0..5000u64).map(|i| SampleRow { ev: unmix(i), user: Some(unmix(i % 300)), ts: Some(i as i64) }).collect();
        assert_eq!(recover_keys(&rows, 0), None);
    }
}
