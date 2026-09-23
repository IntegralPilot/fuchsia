// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ffi::CString;
use std::process::ExitCode;
use zx::{Name, Vmo};

// Must match "debugdata.h" values.
static SINKNAME: &str = "test";
static TESTNAME: Name = Name::new_lossy(SINKNAME);
const TESTDATA: [u8; 4] = [0, 0x11, 0x22, 0x33];

fn main() -> ExitCode {
    let arg_strings: Vec<String> = std::env::args().collect();
    let args: Vec<&str> = arg_strings.iter().map(|s| s.as_str()).collect();
    let expect_invalid = match args[1..] {
        ["publish_data"] => false,
        ["publish_data_fail"] => true,
        _ => return ExitCode::FAILURE,
    };

    let vmo = Vmo::create(TESTDATA.len() as u64).expect("cannot create VMO");
    vmo.write(&TESTDATA, 0).expect("cannot write to VMO");
    vmo.set_name(&TESTNAME).expect("cannot set VMO name");

    let token = zx_libc::sanitizer::publish_data(&CString::new(SINKNAME).unwrap(), vmo);
    assert_eq!(token.is_invalid(), expect_invalid);

    ExitCode::SUCCESS
}
