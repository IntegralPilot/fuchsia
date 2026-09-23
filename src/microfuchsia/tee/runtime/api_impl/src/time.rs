// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Implementation of TEE Internal Core API Specification 7 Time API
// Functions return specific error codes where applicable and panic on all other errors.

use tee_internal::Result;
use tee_internal::binding::{TEE_TIMEOUT_INFINITE, TEE_Time};

fn nanos_to_time(nanos: i64) -> TEE_Time {
    const NANOSECONDS_PER_MILLISECOND: i64 = 1000 * 1000;
    const MILLISECONDS_PER_SECOND: i64 = 1000;
    const NANOSECONDS_PER_SECOND: i64 = NANOSECONDS_PER_MILLISECOND * MILLISECONDS_PER_SECOND;

    let seconds = nanos / NANOSECONDS_PER_SECOND;
    let nanos_remainder = nanos - seconds * NANOSECONDS_PER_SECOND;
    let millis = nanos_remainder / NANOSECONDS_PER_MILLISECOND;
    TEE_Time { seconds: seconds as u32, millis: millis as u32 }
}

pub fn get_system_time() -> TEE_Time {
    inspect_stubs::track_stub!(TODO("https://fxbug.dev/384101082"), "get_system_time");
    let now_utc = fuchsia_runtime::utc_time();
    nanos_to_time(now_utc.into_nanos())
}

pub fn wait(timeout: u32) -> Result {
    inspect_stubs::track_stub!(TODO("https://fxbug.dev/370103570"), "wait should be cancelable");
    if timeout == TEE_TIMEOUT_INFINITE {
        std::thread::sleep(std::time::Duration::MAX);
    } else {
        std::thread::sleep(std::time::Duration::from_millis(timeout as u64));
    }
    Ok(())
}

pub fn get_ta_persistent_time() -> Result<TEE_Time> {
    inspect_stubs::track_stub!(TODO("https://fxbug.dev/384101082"), "get_ta_persistent_time");
    // For now, just return our system time.
    Ok(get_system_time())
}

pub fn set_ta_persistent_time(_time: &TEE_Time) -> Result {
    inspect_stubs::track_stub!(TODO("https://fxbug.dev/384101082"), "set_ta_persistent_time");
    // For now, ignore.
    Ok(())
}

pub fn get_ree_time() -> TEE_Time {
    inspect_stubs::track_stub!(TODO("https://fxbug.dev/384101082"), "get_ree_time");
    // For now, just return our system time.
    get_system_time()
}
