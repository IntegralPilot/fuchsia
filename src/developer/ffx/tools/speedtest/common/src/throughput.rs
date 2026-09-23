// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::Serialize;
use std::fmt::{self, Debug, Display};
use std::time::Duration;

/// Internal representation keeps a value in bits per second (bps).
#[derive(Copy, Clone, PartialEq, PartialOrd, Serialize, JsonSchema)]
pub struct Throughput(f64);
impl Throughput {
    pub fn from_len_and_duration(len: u32, duration: Duration) -> Self {
        let len = f64::from(len);
        let secs = duration.as_secs_f64();

        Self(len * 8f64 / secs)
    }

    pub fn as_f64(&self) -> f64 {
        self.0
    }
}

impl Debug for Throughput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl Display for Throughput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(bits_per_second) = self;
        let (value, suffix) = f64_to_value_and_suffix(*bits_per_second);
        write!(f, "{value:.1} {suffix}bps")
    }
}

fn f64_to_value_and_suffix(v: f64) -> (f64, &'static str) {
    for (m, n) in [(1e9, "G"), (1e6, "M"), (1e3, "K")] {
        if v >= m {
            return (v / m, n);
        }
    }
    (v, "")
}

#[derive(Copy, Clone)]
pub struct BytesFormatter(pub u64);

impl Display for BytesFormatter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(v) = self;
        let (value, suffix) = f64_to_value_and_suffix(*v as f64);
        write!(f, "{value:.1} {suffix}B")
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use test_case::test_case;

    #[test]
    fn value_and_suffix() {
        assert_eq!(f64_to_value_and_suffix(1e12), (1e3, "G"));
        assert_eq!(f64_to_value_and_suffix(2e9), (2.0, "G"));
        assert_eq!(f64_to_value_and_suffix(2e6), (2.0, "M"));
        assert_eq!(f64_to_value_and_suffix(2e3), (2.0, "K"));
        assert_eq!(f64_to_value_and_suffix(2.0), (2.0, ""));
    }

    #[test_case(10, Duration::from_secs(1) => Throughput(80.0))]
    #[test_case(1000, Duration::from_millis(8) => Throughput(1e6))]
    #[test_case(1_000_000_000, Duration::from_secs(1) => Throughput(8e9))]
    fn throughput_from_len_and_duration(len: u32, duration: Duration) -> Throughput {
        Throughput::from_len_and_duration(len, duration)
    }

    #[test]
    fn throughput_display() {
        assert_eq!(Throughput(1.0).to_string(), "1.0 bps");
        assert_eq!(Throughput(10e3).to_string(), "10.0 Kbps");
    }

    #[test]
    fn bytes_display() {
        assert_eq!(BytesFormatter(1).to_string(), "1.0 B");
        assert_eq!(BytesFormatter(1000).to_string(), "1.0 KB");
    }
}
