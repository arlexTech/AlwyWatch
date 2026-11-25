// SPDX-License-Identifier: AGPL-3.0-or-later
//! The local time of day, from the C library (no time-zone crate for one clock).

use std::time::{SystemTime, UNIX_EPOCH};

/// Hour and minute in the local time zone, `secs_from_now` seconds from now.
pub fn hour_minute_in(secs_from_now: u64) -> (u32, u32) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let t = (now + secs_from_now) as i64;
    // `struct tm` starts with int tm_sec, tm_min, tm_hour on every C library; the buffer is
    // larger than the whole struct.
    let mut tm = [0i32; 16];
    // SAFETY: valid pointers; the C library writes at most sizeof(struct tm) bytes.
    let ok = unsafe { local(&t, tm.as_mut_ptr()) };
    if ok {
        (tm[2] as u32, tm[1] as u32)
    } else {
        (((t / 3600) % 24) as u32, ((t / 60) % 60) as u32)
    }
}

#[cfg(unix)]
unsafe fn local(t: &i64, tm: *mut i32) -> bool {
    unsafe extern "C" {
        fn localtime_r(t: *const i64, tm: *mut i32) -> *mut i32;
    }
    // SAFETY: see the caller.
    !unsafe { localtime_r(t, tm) }.is_null()
}

#[cfg(windows)]
unsafe fn local(t: &i64, tm: *mut i32) -> bool {
    unsafe extern "C" {
        fn _localtime64_s(tm: *mut i32, t: *const i64) -> i32;
    }
    // SAFETY: see the caller.
    unsafe { _localtime64_s(tm, t) == 0 }
}

/// The local `struct tm` fields of unix time `t` (sec, min, hour, mday, mon, year…).
fn fields(t: i64) -> Option<[i32; 16]> {
    let mut tm = [0i32; 16];
    // SAFETY: valid pointers; the C library writes at most sizeof(struct tm) bytes.
    unsafe { local(&t, tm.as_mut_ptr()) }.then_some(tm)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// The local time zone's offset from UTC now, in minutes (east positive).
pub fn utc_offset_minutes() -> i32 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let Some(tm) = fields(now) else { return 0 };
    let local = days_from_civil(tm[5] as i64 + 1900, tm[4] as i64 + 1, tm[3] as i64) * 86_400
        + tm[2] as i64 * 3600
        + tm[1] as i64 * 60
        + tm[0] as i64;
    ((local - now) / 60) as i32
}

/// The local date of unix time `t`, `YYYY-MM-DD`.
pub fn date(t: i64) -> String {
    match fields(t) {
        Some(tm) => format!("{:04}-{:02}-{:02}", tm[5] + 1900, tm[4] + 1, tm[3]),
        None => String::new(),
    }
}

/// "21:47", `secs_from_now` seconds from now.
pub fn clock_in(secs_from_now: u64) -> String {
    let (h, m) = hour_minute_in(secs_from_now);
    format!("{h}:{m:02}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_time_of_day() {
        let (h, m) = super::hour_minute_in(0);
        assert!(h < 24 && m < 60);
        assert!(super::clock_in(0).len() >= 4);
        assert!(super::utc_offset_minutes().abs() <= 14 * 60);
        assert_eq!(super::date(0).len(), 10);
    }
}
