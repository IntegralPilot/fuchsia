// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::LazyLock;

// This may be already handled by something, but I don't want new deps.
const USEC_IN_NANOS: i64 = 1000;
pub const MSEC_IN_NANOS: i64 = 1000 * USEC_IN_NANOS;
const SEC_IN_NANOS: i64 = 1000 * MSEC_IN_NANOS;
const MIN_IN_NANOS: i64 = SEC_IN_NANOS * 60;
const HOUR_IN_NANOS: i64 = MIN_IN_NANOS * 60;
const DAY_IN_NANOS: i64 = HOUR_IN_NANOS * 24;
const WEEK_IN_NANOS: i64 = DAY_IN_NANOS * 7;
const YEAR_IN_NANOS: i64 = DAY_IN_NANOS * 365; // Approximate.

static UNITS: LazyLock<Vec<(i64, &'static str)>> = LazyLock::new(|| {
    vec![
        (YEAR_IN_NANOS, "year(s)"),
        (WEEK_IN_NANOS, "week(s)"),
        (DAY_IN_NANOS, "day(s)"),
        (HOUR_IN_NANOS, "h"),
        (MIN_IN_NANOS, "min"),
        (SEC_IN_NANOS, "s"),
        (MSEC_IN_NANOS, "ms"),
        (USEC_IN_NANOS, "μs"),
        (1, "ns"),
    ]
});

// Formats a time value into a simplistic human-readable string.  This is meant
// to be a human-friendly, but not an impeccable format.
pub fn format_common(mut value: i64) -> String {
    let value_copy = value;
    let mut repr: Vec<String> = vec![];
    for (unit_value, unit_str) in UNITS.iter() {
        if value == 0 {
            break;
        }
        let num_units = value / unit_value;
        if num_units.abs() > 0 {
            repr.push(format!("{}{}", num_units, unit_str));
            value = value % unit_value;
        }
    }
    if repr.len() == 0 {
        repr.push("0ns".to_string());
    }
    // 1year(s)_3week(s)_4day(s)_1h_2m_340ms. Not ideal but user-friendly enough.
    let repr = repr.join("_");

    let mut ret = vec![];
    ret.push(repr);
    // Also add the full nanosecond value too.
    ret.push(format!("({})", value_copy));
    ret.join(" ")
}

// Pretty prints a timer value into a simplistic format.
pub fn format_timer<T: zx::Timeline>(timer: zx::Instant<T>) -> String {
    format_common(timer.into_nanos())
}

// Pretty prints a duration into a simplistic format.
pub fn format_duration<T: zx::Timeline>(duration: zx::Duration<T>) -> String {
    format_common(duration.into_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    // Human readable duration formatting is useful.
    #[test_case(0, "0ns (0)" ; "zero")]
    #[test_case(1000, "1μs (1000)" ; "1us positive")]
    #[test_case(-1000, "-1μs (-1000)"; "1us negative")]
    #[test_case(YEAR_IN_NANOS, "1year(s) (31536000000000000)"; "A year")]
    #[test_case(YEAR_IN_NANOS + 8 * DAY_IN_NANOS + 1,
        "1year(s)_1week(s)_1day(s)_1ns (32227200000000001)" ; "A weird duration")]
    #[test_case(2 * HOUR_IN_NANOS + 8 * MIN_IN_NANOS + 32 * SEC_IN_NANOS + 1,
        "2h_8min_32s_1ns (7712000000001)" ; "A reasonable long duration")]
    fn test_format_common(value: i64, repr: &str) {
        assert_eq!(format_common(value), repr.to_string());
    }
}
