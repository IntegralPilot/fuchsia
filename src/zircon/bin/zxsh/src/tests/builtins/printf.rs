// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtins::printf::builtin_printf;
use crate::builtins::run_builtin;
use crate::eval::{EvalOutcome, ExecutionContext, ShellState};
use bstr::BString;

#[test]
fn test_builtin_printf_dash_semantics() {
    let mut state = ShellState::new();

    // 1. Missing format string argument -> return 1 and usage message
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(&[], &mut state, &mut std::io::empty(), &mut stdout, &mut stderr);
    assert_eq!(code, 1);
    assert_eq!(String::from_utf8(stderr).unwrap(), "printf: usage: printf format [arg ...]\n");

    // 2. Basic format string with literals and newline
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[BString::from(r"hello world\n")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(stdout).unwrap(), "hello world\n");

    // 3. Format loop reuse across positional arguments
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[
            BString::from(r"%s\n"),
            BString::from("one"),
            BString::from("two"),
            BString::from("three"),
        ],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(stdout).unwrap(), "one\ntwo\nthree\n");

    // 4. %b SysV escape expansion and \c halt
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[BString::from(r"%b\n"), BString::from(r"foo\tbar\cignored")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(stdout).unwrap(), "foo\tbar");

    // 5. Numeric character conversion ('A and "B)
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[BString::from(r"%d %d\n"), BString::from("'A"), BString::from("\"B")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(stdout).unwrap(), "65 66\n");

    // 6. Integer format specifiers %d, %i, %o, %u, %x, %X with flags & width/precision
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[
            BString::from(r"%+05d %#o %#x %#X\n"),
            BString::from("42"),
            BString::from("255"),
            BString::from("255"),
            BString::from("255"),
        ],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(stdout).unwrap(), "+0042 0377 0xff 0XFF\n");

    // 7. Width and Precision from '*' arguments
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[BString::from(r"%*.*d\n"), BString::from("5"), BString::from("2"), BString::from("7")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(stdout).unwrap(), "   07\n");

    // 8. Warning messages and exit status 1 for invalid / truncated numeric arguments
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[BString::from(r"%d %d\n"), BString::from("invalid"), BString::from("123xyz")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);
    assert_eq!(String::from_utf8(stdout).unwrap(), "0 123\n");
    let err_msg = String::from_utf8(stderr).unwrap();
    assert!(
        err_msg.contains("printf: invalid: expected numeric value"),
        "err_msg was: {:?}",
        err_msg
    );
    assert!(
        err_msg.contains("printf: 123xyz: value may be truncated"),
        "err_msg was: {:?}",
        err_msg
    );
}

#[test]
fn test_run_builtin_printf() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("printf", &[BString::from(r"test\n")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_printf_format_escapes_and_trailing_backslash() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[BString::from(r"\a\b\f\r\v\101\z")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, &[7, 8, 12, 13, 11, b'A', b'\\', b'z']);

    stdout.clear();
    let code = builtin_printf(
        &[BString::from(r"trailing\")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, b"trailing\\");
}

#[test]
fn test_printf_percent_percent_and_errors() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[BString::from(r"100%%")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, b"100%");

    stderr.clear();
    let code = builtin_printf(
        &[BString::from(r"%")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);
    assert!(String::from_utf8(stderr.clone()).unwrap().contains("missing format character"));

    stderr.clear();
    let code = builtin_printf(
        &[BString::from(r"%Z")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);
    assert!(String::from_utf8(stderr.clone()).unwrap().contains("invalid directive"));
}

#[test]
fn test_printf_flags_width_precision_all_types() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[
            BString::from(r"% d|%05d|%-5s|%.0d|%c|%u|%o|%x|%X|%f|%F|%e|%E|%g|%G|%a|%A"),
            BString::from("42"),
            BString::from("42"),
            BString::from("hi"),
            BString::from("0"),
            BString::from("X"),
            BString::from("100"),
            BString::from("100"),
            BString::from("255"),
            BString::from("255"),
            BString::from("1.5"),
            BString::from("1.5"),
            BString::from("1.5"),
            BString::from("1.5"),
            BString::from("1.200"),
            BString::from("1.200"),
            BString::from("1.5"),
            BString::from("1.5"),
        ],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
}

#[test]
fn test_printf_dynamic_width_prec_negative_and_char_conv() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[
            BString::from(r"%*s|%.*d"),
            BString::from("-5"),
            BString::from("hi"),
            BString::from("-3"),
            BString::from("42"),
        ],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(stdout, b"hi   |42");
}

#[test]
fn test_printf_number_and_float_edge_cases() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_printf(
        &[
            BString::from(r"%d|%d|%d|%f|%f|%f"),
            BString::from(""),
            BString::from("999999999999999999999999999999"),
            BString::from("+42"),
            BString::from(""),
            BString::from("3.14abc"),
            BString::from("not_a_float"),
        ],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);
}
