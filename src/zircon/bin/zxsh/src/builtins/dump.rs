// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{EXIT_FAILURE, EXIT_SUCCESS, ShellState};
use bstr::{BString, ByteSlice};
use std::io::{Read, Write};

pub fn builtin_dump(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.len() != 1 {
        let _ = writeln!(stderr, "usage: dump <filename>");
        return EXIT_FAILURE;
    }

    let path = match args[0].to_path() {
        Ok(p) => p,
        Err(_) => {
            let _ = writeln!(stderr, "error: cannot open '{}'", args[0]);
            return EXIT_FAILURE;
        }
    };

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            let _ = writeln!(stderr, "error: cannot open '{}'", args[0]);
            return EXIT_FAILURE;
        }
    };

    let mut buf = [0u8; 4096];
    let mut off: u64 = 0;

    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => {
                let _ = writeln!(stderr, "error: io");
                return EXIT_FAILURE;
            }
        };

        let mut chunk_offset = 0;
        while chunk_offset < n {
            let line_len = std::cmp::min(16, n - chunk_offset);
            let line_bytes = &buf[chunk_offset..chunk_offset + line_len];

            let mut hex_part = String::with_capacity(48);
            let mut ascii_part = String::with_capacity(16);

            for i in 0..16 {
                if i < line_len {
                    let b = line_bytes[i];
                    use std::fmt::Write as _;
                    let _ = write!(hex_part, "{:02x} ", b);
                    if b.is_ascii_graphic() || b == b' ' {
                        ascii_part.push(b as char);
                    } else {
                        ascii_part.push('.');
                    }
                } else {
                    hex_part.push_str("   ");
                }
            }

            let curr_addr = off + chunk_offset as u64;
            let _ = writeln!(stdout, "0x{:08x}: {}|{}", curr_addr, hex_part, ascii_part);
            chunk_offset += line_len;
        }

        off += n as u64;
    }

    EXIT_SUCCESS
}
