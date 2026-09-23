// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtins::list::builtin_list;
use crate::builtins::run_builtin;
use crate::eval::{EvalOutcome, ExecutionContext, ShellState};
use bstr::BString;

fn to_bstr(p: &std::path::Path) -> BString {
    BString::from(p.to_str().unwrap())
}

#[test]
fn test_builtin_list_dash_semantics() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let temp_dir = std::env::temp_dir().join("zxsh_test_list_dir");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();

    let file_path = temp_dir.join("test_list.txt");
    std::fs::write(&file_path, b"first line\nsecond line\nthird line\n").unwrap();

    let file_bstr = to_bstr(&file_path);

    // 1. Missing argument (0 args) -> usage error
    let res = run_builtin("list", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // 2. Extra argument (>1 args) -> usage error
    let res =
        run_builtin("list", &[file_bstr.clone(), BString::from("extra")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // 3. Nonexistent file -> error cannot open
    let bad_file = BString::from("/nonexistent_file_xyz_999.txt");
    let res = run_builtin("list", &[bad_file], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // 4. Valid file -> line numbered output
    let res = run_builtin("list", &[file_bstr], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_list_output_formatting_and_errors() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin = std::io::empty();

    let temp_file = std::env::temp_dir().join("zxsh_list_format_test.txt");
    std::fs::write(&temp_file, b"alpha\nbeta\ngamma\n").unwrap();
    let list_path = to_bstr(&temp_file);

    let code = builtin_list(&[list_path], &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(code, 0);

    let out_str = String::from_utf8(stdout).unwrap();
    assert!(out_str.contains("    1 | alpha"));
    assert!(out_str.contains("    2 | beta"));
    assert!(out_str.contains("    3 | gamma"));

    let _ = std::fs::remove_file(&temp_file);
}
