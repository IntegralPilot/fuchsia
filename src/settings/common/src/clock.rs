// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

const TIMESTAMP_DIVIDEND: i64 = 1_000_000_000;

#[cfg(not(test))]
pub fn now() -> zx::MonotonicInstant {
    zx::MonotonicInstant::get()
}

#[cfg(not(test))]
pub fn inspect_format_now() -> String {
    // follows syslog timestamp format: [seconds.nanos]
    let timestamp = now().into_nanos();
    let seconds = timestamp / TIMESTAMP_DIVIDEND;
    let nanos = timestamp % TIMESTAMP_DIVIDEND;
    format!("{seconds}.{nanos:09}")
}

#[cfg(test)]
pub(crate) use mock::inspect_format_now;

// Exported so other crates can use for testing.
pub mod mock {
    use super::*;
    use std::cell::RefCell;

    thread_local!(static MOCK_TIME: RefCell<zx::MonotonicInstant> = RefCell::new(zx::MonotonicInstant::get()));

    pub fn now() -> zx::MonotonicInstant {
        MOCK_TIME.with(|time| *time.borrow())
    }

    pub fn set(new_time: zx::MonotonicInstant) {
        MOCK_TIME.with(|time| *time.borrow_mut() = new_time);
    }

    pub fn inspect_format_now() -> String {
        let timestamp = now().into_nanos();
        let seconds = timestamp / TIMESTAMP_DIVIDEND;
        let nanos = timestamp % TIMESTAMP_DIVIDEND;
        format!("{seconds}.{nanos:09}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_inspect_format() {
        mock::set(zx::MonotonicInstant::from_nanos(0));
        assert_eq!(String::from("0.000000000"), inspect_format_now());

        mock::set(zx::MonotonicInstant::from_nanos(123));
        assert_eq!(String::from("0.000000123"), inspect_format_now());

        mock::set(zx::MonotonicInstant::from_nanos(123_000_000_000));
        assert_eq!(String::from("123.000000000"), inspect_format_now());

        mock::set(zx::MonotonicInstant::from_nanos(123_000_000_123));
        assert_eq!(String::from("123.000000123"), inspect_format_now());

        mock::set(zx::MonotonicInstant::from_nanos(123_001_230_000));
        assert_eq!(String::from("123.001230000"), inspect_format_now());
    }
}
