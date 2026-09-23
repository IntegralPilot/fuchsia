// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtins::dump::builtin_dump;
use crate::builtins::run_builtin;
use crate::eval::{EvalOutcome, ExecutionContext, ShellState};
use bstr::BString;

fn to_bstr(p: &std::path::Path) -> BString {
    BString::from(p.to_str().unwrap())
}

#[test]
fn test_run_builtin_dump() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let temp_file = std::env::temp_dir().join("zxsh_dump_mod_test.txt");
    std::fs::write(&temp_file, b"dump contents\n").unwrap();
    let dump_path = to_bstr(&temp_file);

    let res = run_builtin("dump", &[dump_path], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    let _ = std::fs::remove_file(&temp_file);
}

#[test]
fn test_dump_formatting_and_binary_data() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin = std::io::empty();

    let temp_file = std::env::temp_dir().join("zxsh_dump_formatting_test.bin");
    // Write 20 bytes including non-printable characters (< 32, control codes, non-ascii)
    let mut test_bytes = vec![0u8, 1, 9, 10, 13, 32, b'A', b'B', b'C', 127];
    test_bytes.extend_from_slice(b"1234567890"); // total 20 bytes (> 16 bytes for padding)
    std::fs::write(&temp_file, &test_bytes).unwrap();
    let dump_path = to_bstr(&temp_file);

    let code = builtin_dump(&[dump_path], &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(code, 0);

    let out_str = String::from_utf8(stdout).unwrap();
    assert!(out_str.contains("0x00000000:"));
    assert!(out_str.contains("0x00000010:"));
    assert!(out_str.contains("|"));

    let _ = std::fs::remove_file(&temp_file);
}

#[test]
fn test_dump_errors_and_usage() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin = std::io::empty();

    // 0 args -> usage error
    let code = builtin_dump(&[], &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&stderr).contains("usage: dump"));

    // >1 args -> usage error
    stderr.clear();
    let code = builtin_dump(
        &[BString::from("f1"), BString::from("f2")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);

    // Non-existent file -> error cannot open
    stderr.clear();
    let code = builtin_dump(
        &[BString::from("/nonexistent_dump_file_xyz")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&stderr).contains("cannot open"));
}
