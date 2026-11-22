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
    }
}
