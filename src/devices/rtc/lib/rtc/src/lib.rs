// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_hardware_rtc as frtc;
use log::warn;

// Time conversion constants (same as pl031-rtc)
pub const LOCAL_EPOCH: u64 = 946684800;
pub const LOCAL_EPOCH_YEAR: u16 = 2000;
pub const DEFAULT_YEAR: u16 = 2020;
pub const MAX_YEAR: u16 = 2099;

pub const DAYS_IN_MONTH: [u64; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

pub const DEFAULT_RTC: frtc::Time = frtc::Time {
    seconds: 0,
    minutes: 0,
    hours: 0,
    day: 1,
    month: 1, // JANUARY
    year: DEFAULT_YEAR,
};

pub fn is_leap_year(year: u16) -> bool {
    ((year % 4) == 0 && (year % 100) != 0) || ((year % 400) == 0)
}

pub fn days_in_year(year: u16) -> u64 {
    if is_leap_year(year) { 366 } else { 365 }
}

pub fn days_in_month(month: u8, year: u16) -> u64 {
    let mut days = DAYS_IN_MONTH[month as usize];
    if month == 2 && is_leap_year(year) {
        days += 1;
    }
    days
}

pub fn is_rtc_valid(rtc: &frtc::Time) -> bool {
    if rtc.year < LOCAL_EPOCH_YEAR || rtc.year > MAX_YEAR {
        return false;
    }
    if rtc.month < 1 || rtc.month > 12 {
        return false;
    }
    if rtc.day as u64 > days_in_month(rtc.month, rtc.year) {
        return false;
    }
    if rtc.day == 0 {
        return false;
    }
    if rtc.hours > 23 || rtc.minutes > 59 || rtc.seconds > 59 {
        return false;
    }
    true
}

pub fn seconds_to_rtc(seconds: u64) -> frtc::Time {
    if seconds < LOCAL_EPOCH {
        warn!("SecondsToRtc: Seconds value is out of range, returning default");
        return DEFAULT_RTC;
    }

    let mut epoch = seconds - LOCAL_EPOCH;
    let mut rtc = frtc::Time {
        seconds: (epoch % 60) as u8,
        minutes: 0,
        hours: 0,
        day: 0,
        month: 0,
        year: LOCAL_EPOCH_YEAR,
    };
    epoch /= 60;
    rtc.minutes = (epoch % 60) as u8;
    epoch /= 60;
    rtc.hours = (epoch % 24) as u8;
    epoch /= 24;

    for year in LOCAL_EPOCH_YEAR.. {
        let days = days_in_year(year);
        if epoch < days {
            rtc.year = year;
            break;
        }
        epoch -= days;
    }

    for month in 1..=12 {
        let days = days_in_month(month, rtc.year);
        if epoch < days {
            rtc.month = month;
            break;
        }
        epoch -= days;
    }

    rtc.day = (epoch + 1) as u8;
    rtc
}

pub fn seconds_since_epoch(rtc: &frtc::Time) -> u64 {
    let mut days_since_local_epoch = 0;
    for year in LOCAL_EPOCH_YEAR..rtc.year {
        days_since_local_epoch += days_in_year(year);
    }
    for month in 1..rtc.month {
        days_since_local_epoch += days_in_month(month, rtc.year);
    }
    days_since_local_epoch += (rtc.day - 1) as u64;

    let hours = (days_since_local_epoch * 24) + rtc.hours as u64;
    let minutes = (hours * 60) + rtc.minutes as u64;
    let seconds = (minutes * 60) + rtc.seconds as u64;

    LOCAL_EPOCH + seconds
}

pub fn sanitize_rtc(rtc: frtc::Time) -> frtc::Time {
    if !is_rtc_valid(&rtc) || rtc.year < DEFAULT_YEAR {
        warn!("RTC is sanitized to constant default");
        return DEFAULT_RTC;
    }
    rtc
}
