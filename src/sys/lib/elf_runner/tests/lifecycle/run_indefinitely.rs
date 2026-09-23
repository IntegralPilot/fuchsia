// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async as fasync;

#[fuchsia::main]
/// Simple program that never exits and logs every 5 minutes
async fn main() {
    println!("Child created!");
    loop {
        // Why 5 minutes? It is the typical test timeout so we only expect this
        // loop to run once and pollute the logs a little.
        fasync::Timer::new(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(
            5 * 60,
        )))
        .await;
        println!("Waiting longer");
    }
}
