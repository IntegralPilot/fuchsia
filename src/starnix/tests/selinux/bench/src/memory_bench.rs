// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::File;
use std::io::Write;

const POLICY_BYTES: &'static [u8] =
    include_bytes!("../../../../lib/selinux/testdata/policies/aosp_sepolicy");

fn main() {
    println!("starting sestarnix_memory_bench");
    #[repr(C)]
    struct mallinfo2 {
        pub arena: usize,
        pub ordblks: usize,
        pub smblks: usize,
        pub hblks: usize,
        pub hblkhd: usize,
        pub usmblks: usize,
        pub fsmblks: usize,
        pub uordblks: usize,
        pub fordblks: usize,
        pub keepcost: usize,
    }
    unsafe extern "C" {
        fn mallinfo2() -> mallinfo2;
    }
    let get_mallinfo = || unsafe { mallinfo2() };

    let before = get_mallinfo().uordblks as u64;

    let server = selinux::SecurityServer::new_default();
    server.load_policy(POLICY_BYTES.to_vec()).expect("Failed to load policy");

    let after = get_mallinfo().uordblks as u64;

    let memory_used = after.saturating_sub(before);

    let result = fuchsiaperf::FuchsiaPerfBenchmarkResult {
        test_suite: "fuchsia.sestarnix_memory".to_string(),
        label: "load_policy_memory".to_string(),
        values: vec![memory_used as f64],
        unit: fuchsiaperf::Unit::Bytes,
        direction: fuchsiaperf::Direction::SmallerBetter,
    };

    let json_output_path = "/custom_artifacts/results.fuchsiaperf.json";

    let json = serde_json::to_string_pretty(&vec![result]).unwrap();
    let mut file = File::create(json_output_path).expect("failed to create output file");
    file.write_all(json.as_bytes()).expect("failed to write to output file");
}
