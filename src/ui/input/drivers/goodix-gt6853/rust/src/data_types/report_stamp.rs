// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Report stamp data type for tracking input reports.
//!
//! The code in this module is useful, but does not meet the team's quality
//! bar. See go/fuchsia-display-rough

// TODO(https://fxbug.dev/559080325): Replace with shared input-report-reader crate once available.

/// A type-safe identifier and monotonic counter for an input report.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReportStamp(pub u64);

impl ReportStamp {
    /// Placeholder value for an invalid / unassigned report stamp (0).
    pub const INVALID: Self = Self(0);

    /// Returns the next sequential report stamp.
    pub fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    /// Returns the underlying `u64` value.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for ReportStamp {
    fn from(stamp: u64) -> Self {
        Self(stamp)
    }
}

impl From<ReportStamp> for u64 {
    fn from(stamp: ReportStamp) -> Self {
        stamp.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_stamp() {
        let stamp = ReportStamp::INVALID;
        assert_eq!(stamp.get(), 0);
        let next_stamp = stamp.next();
        assert_eq!(next_stamp.get(), 1);
        assert_eq!(ReportStamp::from(42u64).get(), 42);
        assert_eq!(u64::from(ReportStamp(42)), 42);
    }
}
