// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::env;
use std::process::Command;

#[test]
fn test_binary_prints_backtrace() {
    // First, check that RUST_BACKTRACE=1 is in the current environment.
    match env::var("RUST_BACKTRACE") {
        Ok(val) => assert!(val == "1", "Variable RUST_BACKTRACE is not set to 1"),
        Err(env::VarError::NotPresent) => panic!("RUST_BACKTRACE is missing!"),
        Err(env::VarError::NotUnicode(_)) => panic!("RUST_BACKTRACE contains invalid data!"),
    }

    // Second, call ./panic_trigger, then gets its stderr.
    let output = Command::new("./panic_trigger").output().expect("Failed to execute command");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Third, verify that the backtrace was generated.
    assert!(stderr.contains("Intentional panic for testing"));
    assert!(
        stderr.contains("stack backtrace:"),
        "Backtrace was missing from stderr! Got:\n{}",
        stderr
    );
}
