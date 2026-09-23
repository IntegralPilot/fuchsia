// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtins::echo::builtin_echo;
use crate::builtins::run_builtin;
use crate::eval::{ExecutionContext, ShellState};
use bstr::BString;

#[test]
fn test_builtin_echo_and_pwd() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let echo_args = vec![BString::from("hello"), BString::from("world")];
    let res = run_builtin("echo", &echo_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(0));

    let pwd_args = vec![];
    let res = run_builtin("pwd", &pwd_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(0));
}

#[test]
fn test_echo_no_newline_flag() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_echo(
        &[BString::from("-n"), BString::from("no"), BString::from("newline")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, b"no newline");
}

#[test]
fn test_echo_sysv_escapes() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_echo(
        &[BString::from(r"\a\b\f\n\r\t\v\\")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, &[7, 8, 12, 10, 13, 9, 11, b'\\', b'\n']);
}

#[test]
fn test_echo_halt_on_c_escape() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_echo(
        &[BString::from(r"first\cignored"), BString::from("second")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, b"first");
}

#[test]
fn test_echo_octal_escapes() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_echo(
        &[
            BString::from(r"\0101"),
            BString::from(r"\102"),
            BString::from(r"\07"),
            BString::from(r"\00"),
        ],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, &[b'A', b' ', b'B', b' ', 7, b' ', 0, b'\n']);
}

#[test]
fn test_echo_unknown_escape_and_trailing_backslash() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_echo(
        &[BString::from(r"\z"), BString::from(r"end\")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, &[b'\\', b'z', b' ', b'e', b'n', b'd', b'\\', b'\n']);
}
