// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use stress_config_rust::Config;

use std::sync::Arc;
use std::time::{Duration, Instant};

fn thread_main(config: Arc<Config>) {
    let start = Instant::now();
    let mut count: u64 = 0;
    while start.elapsed() < Duration::from_millis(config.duration_ms) {
        count += 1;
        fuchsia_trace::duration!(
            "stress",
            "simulated_work",
            "iteration" => count,
            "arg_u64" => 0x1234567890abcdefu64,
            "arg_string" => "Blah blah blah");
        std::thread::sleep(Duration::from_millis(config.interval_ms));
        // Another trace event with a different size to make sure that records don't always exactly
        // overlap in streaming mode.
        fuchsia_trace::duration!("stress", "small_duration");
    }
}

fn main() -> Result<(), anyhow::Error> {
    println!("Hello trace thread stress!");
    fuchsia_trace_provider::trace_provider_create_with_fdio();

    let config = Arc::new(Config::take_from_startup_handle());

    let handles = (0..config.thread_count)
        .map(|_| {
            let config = config.clone();
            std::thread::spawn(move || thread_main(config))
        })
        .collect::<Vec<_>>();

    println!("Threads started");
    for h in handles {
        if let Err(e) = h.join() {
            println!("Thread panic: {:?}", e);
        }
    }

    println!("Trace thread stress test complete");

    Ok(())
}
