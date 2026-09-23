// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtins::fuchsia::{builtin_dm, builtin_k, builtin_power};
use crate::builtins::run_builtin;
use crate::eval::{ExecutionContext, ShellState};
use bstr::{BStr, BString};
use std::io::Cursor;

#[test]
fn test_builtin_dm_subcommands() {
    let mut state = ShellState::new();
    let mut stdin = std::io::empty();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let _ =
        builtin_dm(&[BString::from("reboot")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    let _ = builtin_dm(&[BString::from("rb")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    let _ = builtin_dm(&[BString::from("rr")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    let _ =
        builtin_dm(&[BString::from("poweroff")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    let _ =
        builtin_dm(&[BString::from("shutdown")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    let code = builtin_dm(
        &[BString::from("unknown_cmd")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);
}

#[test]
fn test_builtin_fuchsia_dm_and_power_help() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let dm_help = vec![BString::from("help")];
    let res = run_builtin(BStr::new("dm"), &dm_help, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(0));

    let dm_unknown = vec![BString::from("invalid_command")];
    let res = run_builtin(BStr::new("dm"), &dm_unknown, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(1));

    let power_help = vec![BString::from("help")];
    let res = run_builtin(BStr::new("power"), &power_help, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(0));
}

#[test]
fn test_builtin_fuchsia_dm_dash_semantics() {
    let mut state = ShellState::new();
    let mut in_stream = Cursor::new(b"");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = builtin_dm(&[], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 1);
    assert_eq!(String::from_utf8_lossy(&out), "usage: dm <command>\n");
    assert!(err.is_empty());

    out.clear();
    let code = builtin_dm(&[BString::from("help")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);
    assert!(String::from_utf8_lossy(&out).contains("poweroff             - power off the system"));

    out.clear();
    let code =
        builtin_dm(&[BString::from("help extra")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    out.clear();
    let code =
        builtin_dm(&[BString::from("unknown_cmd")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&out).contains("Unknown command 'unknown_cmd'"));
    assert!(String::from_utf8_lossy(&out).contains("Valid commands:"));
}

#[test]
fn test_builtin_fuchsia_k_dash_semantics() {
    let mut state = ShellState::new();
    let mut in_stream = Cursor::new(b"");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = builtin_k(&[], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 1);
    assert_eq!(String::from_utf8_lossy(&out), "usage: k <command>\n");
    assert!(err.is_empty());

    out.clear();
    let long_arg = "a".repeat(256);
    let code =
        builtin_k(&[BString::from(long_arg)], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 1);
    assert_eq!(String::from_utf8_lossy(&err), "error: kernel debug command too long\n");

    out.clear();
    err.clear();
    let code = builtin_k(
        &[BString::from("reboot"), BString::from("extra")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 1);
    assert_eq!(String::from_utf8_lossy(&out), "usage: dm <command>\n");
}

#[test]
fn test_builtin_fuchsia_k_invalid_args() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let k_no_args = vec![];
    let res = run_builtin(BStr::new("k"), &k_no_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(1));
}

#[test]
fn test_builtin_fuchsia_power_dash_semantics() {
    let mut state = ShellState::new();
    let mut in_stream = Cursor::new(b"");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = builtin_power(&[], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 1);
    assert_eq!(String::from_utf8_lossy(&out), "usage: power <command>\n");
    assert!(err.is_empty());

    out.clear();
    let code =
        builtin_power(&[BString::from("help")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);
    assert!(String::from_utf8_lossy(&out).contains("off                  - power off the system"));

    out.clear();
    let code = builtin_power(
        &[BString::from("unknown_cmd")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&out).contains("Unknown command 'unknown_cmd'"));
    assert!(String::from_utf8_lossy(&out).contains("Valid commands:"));
}

#[test]
fn test_builtin_k_subcommands_and_errors() {
    let mut state = ShellState::new();
    let mut stdin = std::io::empty();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let _ = builtin_k(&[BString::from("reboot")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    let _ = builtin_k(
        &[BString::from("help"), BString::from("subcommand")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );

    // Command length >= 256 bytes
    let long_arg = BString::from("a".repeat(300));
    stderr.clear();
    let code = builtin_k(&[long_arg], &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&stderr).contains("kernel debug command too long"));

    // Invalid UTF-8 command bytes
    let invalid_utf8 = BString::from(vec![0xff, 0xfe, 0xfd]);
    stderr.clear();
    let code = builtin_k(&[invalid_utf8], &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&stderr).contains("invalid UTF-8 in command"));
}

#[test]
fn test_builtin_power_subcommands() {
    let mut state = ShellState::new();
    let mut stdin = std::io::empty();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let _ =
        builtin_power(&[BString::from("reboot")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    let _ = builtin_power(&[BString::from("rb")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    let _ = builtin_power(&[BString::from("rr")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    let _ =
        builtin_power(&[BString::from("off")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    let _ = builtin_power(
        &[BString::from("shutdown")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    let code = builtin_power(
        &[BString::from("unknown_cmd")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);
}
