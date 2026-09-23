// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtins::msleep::builtin_msleep;
use crate::builtins::run_builtin;
use crate::eval::{EXIT_FAILURE, EXIT_SUCCESS, EvalOutcome, ExecutionContext, ShellState};
use bstr::BString;

#[test]
fn test_builtin_msleep() {
    let mut state = ShellState::new();

    let msleep_args = vec![BString::from("1")];
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin = std::io::empty();
    let res = builtin_msleep(&msleep_args, &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(res, EXIT_SUCCESS);

    let no_args = vec![];
    stderr.clear();
    let res = builtin_msleep(&no_args, &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(res, EXIT_FAILURE);
    assert_eq!(String::from_utf8_lossy(&stderr), "usage: msleep <msecs>\n");

    let too_many_args = vec![BString::from("1"), BString::from("2")];
    stderr.clear();
    let res = builtin_msleep(&too_many_args, &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(res, EXIT_FAILURE);
    assert_eq!(String::from_utf8_lossy(&stderr), "usage: msleep <msecs>\n");

    let invalid_arg = vec![BString::from("invalid")];
    stderr.clear();
    let res = builtin_msleep(&invalid_arg, &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(res, EXIT_FAILURE);
    assert_eq!(String::from_utf8_lossy(&stderr), "msleep: invalid duration\n");
}

#[test]
fn test_run_builtin_msleep() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("msleep", &[BString::from("1")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}
