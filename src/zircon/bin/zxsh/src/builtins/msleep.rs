// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{EXIT_FAILURE, EXIT_SUCCESS, ShellState};
use crate::string::parse_int;
use bstr::{BString, ByteSlice};
use std::io::{Read, Write};

pub fn builtin_msleep(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.len() != 1 {
        let _ = writeln!(stderr, "usage: msleep <msecs>");
        return EXIT_FAILURE;
    }
    let ms = match parse_int::<u64>(args[0].as_bytes()) {
        Some(val) => val,
        None => {
            let _ = writeln!(stderr, "msleep: invalid duration");
            return EXIT_FAILURE;
        }
    };
    std::thread::sleep(std::time::Duration::from_millis(ms));
    EXIT_SUCCESS
}
