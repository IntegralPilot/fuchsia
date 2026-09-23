// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::main]
async fn main() {
    monotonic::monotonic_examples();
    utc::utc_examples().await;
}

mod monotonic {
    // [START monotonic]

    pub fn monotonic_examples() {
        // Read monotonic time.
        let monotonic_time = zx::MonotonicInstant::get();
        println!("The monotonic time is {:?}.", monotonic_time);
    }
    // [END monotonic]
}

mod utc {
    // [START utc]
    use fuchsia_async as fasync;
    use fuchsia_runtime::duplicate_utc_clock_handle;

    pub async fn utc_examples() {
        // Obtain a UTC handle.
        let utc_clock = duplicate_utc_clock_handle(zx::Rights::SAME_RIGHTS)
            .expect("Failed to duplicate UTC clock handle.");

        // Wait for the UTC clock to start.  The clock may never start on a device
        // that does not have a RTC or a network connection.
        // A started clock may, but also may not have a valid UTC actual.
        fasync::OnSignals::new(&utc_clock, zx::Signals::CLOCK_STARTED)
            .await
            .expect("Failed to wait for ZX_CLOCK_STARTED.");
        println!("UTC clock is started.");

        // Wait for the UTC clock to be externally synchronized.  Once that happens,
        // the clock is known to correspond to UTC actual (with error bounds available through
        // `zx::Clock::get_details`).
        fasync::OnSignals::new(&utc_clock, zx::Signals::USER_0)
            .await
            .expect("Failed to wait for ZX_SIGNAL_USER_0.");
        println!("UTC clock is externally synchronized.");

        // Wait for the UTC clock to be of "logging quality".  Logging quality UTC
        // clock is started, but not necessarily corresponding to UTC actual. This
        // clock is to be used only for logging timestamps.
        fasync::OnSignals::new(&utc_clock, zx::Signals::USER_1)
            .await
            .expect("Failed to wait for ZX_SIGNAL_USER_1.");
        println!("UTC clock is of logging quality.");

        // Read the UTC clock.
        let utc_time = utc_clock.read().expect("Failed to read UTC clock.");
        println!("The UTC time is {:?}.", utc_time);

        // Read UTC clock details.
        let clock_details = utc_clock.get_details().expect("Failed to read UTC clock details.");
        println!("The UTC clock's backstop time is {:?}.", clock_details.backstop);
    }
    // [END utc]
}
