// Copyright 2019 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

use std::time::SystemTime;

#[cfg(not(test))]
pub fn now() -> SystemTime {
    SystemTime::now()
}

#[cfg(test)]
pub use mock::now;

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::cell::RefCell;

    thread_local!(static MOCK_TIME: RefCell<SystemTime> = RefCell::new(SystemTime::now()));

    pub fn now() -> SystemTime {
        MOCK_TIME.with(|time| *time.borrow())
    }

    pub fn set(new_time: SystemTime) {
        MOCK_TIME.with(|time| *time.borrow_mut() = new_time);
    }
}
