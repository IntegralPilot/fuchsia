// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_trace::*;

#[fuchsia::main]
fn main() {
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    // Make sure our provider knows about the trace that's already running before we emit events.
    fuchsia_trace_provider::trace_provider_wait_for_init();

    instant!("test_puppet", "puppet_instant", Scope::Thread);

    counter!(c"test_puppet", c"puppet_counter", 0, "somedataseries" => 1);
    counter!(c"test_puppet", c"puppet_counter2", 1, "someotherdataseries" => u64::MAX - 1);

    duration_begin!(c"test_puppet", c"puppet_duration");
    duration_end!(c"test_puppet", c"puppet_duration");

    duration!("test_puppet", "puppet_duration_raii");

    {
        let async_id = Id::new();
        let _guard = async_enter!(async_id, c"test_puppet", c"puppet_async");

        std::thread::spawn(move || {
            async_instant!(async_id, c"test_puppet", c"puppet_async_instant1");
        })
        .join()
        .unwrap();
    }

    let flow_id = Id::new();
    flow_begin!("test_puppet", "puppet_flow", flow_id);

    std::thread::spawn(move || {
        duration!("test_puppet", "flow_thread");
        flow_step!(c"test_puppet", c"puppet_flow_step1", flow_id);
    })
    .join()
    .unwrap();

    flow_end!("test_puppet", "puppet_flow", flow_id);

    instant!("test_puppet", "puppet_instant_args", Scope::Thread, "SomeNullArg" => ());
    instant!("test_puppet", "puppet_instant_args", Scope::Thread, "SomeUint32" => 2145u32);
    instant!("test_puppet", "puppet_instant_args", Scope::Thread, "SomeUint64" => 423621626134123415u64);
    instant!("test_puppet", "puppet_instant_args", Scope::Thread, "SomeInt32" => -7i32);
    instant!("test_puppet", "puppet_instant_args", Scope::Thread, "SomeInt64" => -234516543631231i64);
    instant!("test_puppet", "puppet_instant_args", Scope::Thread, "SomeDouble" => std::f64::consts::PI);
    instant!("test_puppet", "puppet_instant_args", Scope::Thread, "SomeString" => "pong");
    instant!("test_puppet", "puppet_instant_args", Scope::Thread, "SomeBool" => true);
    instant!("test_puppet", "puppet_instant_args", Scope::Thread, "SomePointer" => 4096usize as *const u8);
    instant!("test_puppet", "puppet_instant_args", Scope::Thread, "SomeKoid" => zx::Koid::from_raw(10));
}
