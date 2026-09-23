// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{EXIT_FAILURE, EXIT_SUCCESS, ShellState};
use bstr::{BString, ByteSlice};
use std::io::{Read, Write};

pub fn builtin_list(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.len() != 1 {
        let _ = writeln!(stderr, "usage: list <filename>");
        return EXIT_FAILURE;
    }

    let file_path = &args[0];
    let path = match file_path.to_path() {
        Ok(p) => p,
        Err(_) => {
            let _ = writeln!(stderr, "error: cannot open '{}'", file_path);
            return EXIT_FAILURE;
        }
    };

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            let _ = writeln!(stderr, "error: cannot open '{}'", file_path);
            return EXIT_FAILURE;
        }
    };

    let reader = std::io::BufReader::new(file);
    use std::io::BufRead;
    for (idx, line_res) in reader.lines().enumerate() {
        let line = match line_res {
            Ok(l) => l,
            Err(_) => break,
        };
        let _ = writeln!(stdout, "{:>5} | {}", idx + 1, line);
    }

    EXIT_SUCCESS
}
