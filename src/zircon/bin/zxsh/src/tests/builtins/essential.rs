// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtins::essential::*;
use crate::builtins::{is_builtin, run_builtin};
use crate::collections::FlatMap;
use crate::eval::testing::Frame;
use crate::eval::{
    EXIT_NOT_FOUND, EXIT_SUCCESS, EXIT_SYNTAX_ERROR, EvalOutcome, ExecutionContext, ShellState,
};
use bstr::{BStr, BString, ByteSlice};
use std::io::Cursor;

#[test]
fn test_is_builtin_lookup() {
    assert!(is_builtin("export"));
    assert!(is_builtin("unset"));
    assert!(is_builtin("alias"));
    assert!(is_builtin("set"));
    assert!(is_builtin("cd"));
    assert!(is_builtin("exit"));
    assert!(is_builtin("umask"));
    assert!(is_builtin("ulimit"));
    assert!(!is_builtin("non_existent_builtin"));
}

#[test]
fn test_builtin_cd() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("cd", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let dash_args = vec![BString::from("-")];
    let res = run_builtin("cd", &dash_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let bad_args = vec![BString::from("/nonexistent_directory_xyz")];
    let res = run_builtin("cd", &bad_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));
}

#[test]
fn test_builtin_exit() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("exit", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Exit(0));

    let exit_args = vec![BString::from("42")];
    let res = run_builtin("exit", &exit_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Exit(42));

    let invalid_args = vec![BString::from("abc")];
    let res = run_builtin("exit", &invalid_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));
}

#[test]
fn test_builtin_export_and_unset() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("export", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let export_args = vec![BString::from("VAR=val"), BString::from("FLAG_ONLY")];
    run_builtin("export", &export_args, &mut state, &mut ctx).unwrap();
    assert_eq!(state.get_var("VAR").unwrap(), "val");
    assert!(state.vars().iter().any(|(k, _)| k == "VAR"));

    let unset_v_args = vec![BString::from("-v"), BString::from("VAR")];
    run_builtin("unset", &unset_v_args, &mut state, &mut ctx).unwrap();
    assert!(state.get_var("VAR").is_none());

    state.add_function(BString::from("myfunc"), vec![]);
    let unset_f_args = vec![BString::from("-f"), BString::from("myfunc")];
    run_builtin("unset", &unset_f_args, &mut state, &mut ctx).unwrap();
    assert!(state.get_function("myfunc").is_none());

    state.make_readonly("RO");
    let unset_ro = vec![BString::from("RO")];
    let res = run_builtin("unset", &unset_ro, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));
}

#[test]
fn test_builtin_local() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let local_args = vec![BString::from("VAR=val")];
    let res = run_builtin("local", &local_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res.exit_code(), 2);

    state.frames.push(Frame { local_vars: FlatMap::new(), args: vec![] });
    let res = run_builtin("local", &local_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("VAR").unwrap(), "val");
    state.frames.pop();
}

#[test]
fn test_builtin_set_and_shift() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("set", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let set_flags = vec![BString::from("-exuf"), BString::from("+exuf")];
    let res = run_builtin("set", &set_flags, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let set_args = vec![
        BString::from("--"),
        BString::from("arg1"),
        BString::from("arg2"),
        BString::from("arg3"),
    ];
    run_builtin("set", &set_args, &mut state, &mut ctx).unwrap();
    assert_eq!(
        state.args,
        vec![BString::from("arg1"), BString::from("arg2"), BString::from("arg3")]
    );

    let shift_args = vec![BString::from("2")];
    run_builtin("shift", &shift_args, &mut state, &mut ctx).unwrap();
    assert_eq!(state.args, vec![BString::from("arg3")]);

    let shift_bad = vec![BString::from("10")];
    let res = run_builtin("shift", &shift_bad, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));
}

#[test]
fn test_builtin_alias_and_unalias() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("alias", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let alias_args = vec![BString::from("ll=ls -la")];
    run_builtin("alias", &alias_args, &mut state, &mut ctx).unwrap();
    assert_eq!(state.aliases.get(BStr::new("ll")).unwrap(), "ls -la");

    let alias_query = vec![BString::from("ll")];
    let res = run_builtin("alias", &alias_query, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let unalias_args = vec![BString::from("ll")];
    run_builtin("unalias", &unalias_args, &mut state, &mut ctx).unwrap();
    assert!(state.aliases.get(BStr::new("ll")).is_none());

    let unalias_all = vec![BString::from("-a")];
    let res = run_builtin("unalias", &unalias_all, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_trap() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let trap_set = vec![BString::from("echo trapped"), BString::from("INT")];
    let res = run_builtin("trap", &trap_set, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin("trap", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let trap_clear = vec![BString::from("-"), BString::from("INT")];
    let res = run_builtin("trap", &trap_clear, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_eval_and_exec() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let eval_args = vec![BString::from("EVAL_VAR=hello")];
    let res = run_builtin("eval", &eval_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("EVAL_VAR").unwrap(), "hello");

    let res = run_builtin("exec", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_type() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let type_builtin = vec![BString::from("export")];
    let res = run_builtin("type", &type_builtin, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    state.add_function(BString::from("foo"), vec![]);
    let type_func = vec![BString::from("foo")];
    let res = run_builtin("type", &type_func, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let type_bad = vec![BString::from("unknown_command_name_xyz")];
    let res = run_builtin("type", &type_bad, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_NOT_FOUND));
}

#[test]
fn test_builtin_return_break_continue() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("return", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Return(0));

    let ret_val = vec![BString::from("5")];
    let res = run_builtin("return", &ret_val, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Return(5));

    let res = run_builtin("break", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let break_2 = vec![BString::from("2")];
    let res = run_builtin("break", &break_2, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(BStr::new("continue"), &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // Invalid argument tests return EXIT_SYNTAX_ERROR (2)
    let bad_arg = vec![BString::from("invalid_num")];
    let res = run_builtin(BStr::new("return"), &bad_arg, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SYNTAX_ERROR));

    let res = run_builtin(BStr::new("break"), &bad_arg, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SYNTAX_ERROR));

    let res = run_builtin(BStr::new("continue"), &bad_arg, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SYNTAX_ERROR));

    let zero_arg = vec![BString::from("0")];
    let res = run_builtin(BStr::new("break"), &zero_arg, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SYNTAX_ERROR));

    let res = run_builtin(BStr::new("continue"), &zero_arg, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SYNTAX_ERROR));
}

#[test]
fn test_builtin_umask() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("umask", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let umask_s = vec![BString::from("-S")];
    let res = run_builtin("umask", &umask_s, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let umask_octal = vec![BString::from("022")];
    let res = run_builtin("umask", &umask_octal, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.umask(), 0o022);

    let umask_sym = vec![BString::from("u=rwx,g=rx,o=rx")];
    let res = run_builtin("umask", &umask_sym, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_readonly_and_hash() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("readonly", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let ro_args = vec![BString::from("RO_VAR=constant")];
    run_builtin("readonly", &ro_args, &mut state, &mut ctx).unwrap();
    assert!(state.is_readonly("RO_VAR"));

    let res = run_builtin("hash", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let hash_r = vec![BString::from("-r")];
    let res = run_builtin("hash", &hash_r, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_ulimit_and_command() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let ulimit_a = vec![BString::from("-a")];
    let res = run_builtin("ulimit", &ulimit_a, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let ulimit_help = vec![BString::from("--help")];
    let res = run_builtin("ulimit", &ulimit_help, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));

    let cmd_v = vec![BString::from("-v"), BString::from("export")];
    let res = run_builtin("command", &cmd_v, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_getopts() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let getopts_args = vec![
        BString::from("ab:"),
        BString::from("OPT"),
        BString::from("-a"),
        BString::from("-b"),
        BString::from("foo"),
    ];
    let res = run_builtin(BStr::new("getopts"), &getopts_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("OPT").unwrap(), "a");
}

#[test]
fn test_builtin_dot() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // Missing operand returns EXIT_SYNTAX_ERROR (2)
    let res = run_builtin(BStr::new("."), &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SYNTAX_ERROR));

    // Missing script file returns EXIT_NOT_FOUND (127)
    let bad_dot = vec![BString::from("/nonexistent_script_xyz.sh")];
    let res = run_builtin(BStr::new("."), &bad_dot, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_NOT_FOUND));
}

#[test]
fn test_builtin_cd_home_fallback_and_dash() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // Test cd without HOME set defaults to /
    state.unset_var("HOME");
    let res = run_builtin("cd", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // Test cd - with OLDPWD set
    state.set_var("OLDPWD", "/tmp");
    let res = run_builtin("cd", &[BString::from("-")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_export_additional_paths() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("export", &[BString::from("-p")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    state.make_readonly("RO_EXP");
    let res = run_builtin("export", &[BString::from("RO_EXP=val")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));
}

#[test]
fn test_builtin_unset_additional_paths() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("unset", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "unset",
        &[BString::from("-v"), BString::from("NONEXISTENT")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_local_additional_paths() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    state.frames.push(Frame { local_vars: FlatMap::new(), args: vec![] });

    state.make_readonly("RO_LOC");
    let res = run_builtin("local", &[BString::from("RO_LOC=val")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));

    let res = run_builtin("local", &[BString::from("UNINIT_LOC")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_set_unknown_option() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("set", &[BString::from("-z")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));
}

#[test]
fn test_builtin_shift_numeric_arg_required() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("shift", &[BString::from("invalid_num")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));
}

#[test]
fn test_builtin_trap_invalid_signal() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin(
        "trap",
        &[BString::from("echo hi"), BString::from("INVALID-SIG-123")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(2));

    let res = run_builtin("trap", &[BString::from("-l")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));
}

#[test]
fn test_builtin_read_eof() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("read", &[BString::from("VAR")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));
}

#[test]
fn test_builtin_wait_additional_paths() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("wait", &[BString::from("%999")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(127));

    let res = run_builtin("wait", &[BString::from("--")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_command_flags() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("command", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "command",
        &[BString::from("-v"), BString::from("export")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "command",
        &[BString::from("-V"), BString::from("export")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin("command", &[BString::from("export")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_alias_unalias_errors() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("alias", &[BString::from("NONEXISTENT")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    let res =
        run_builtin("unalias", &[BString::from("NONEXISTENT")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    let res = run_builtin("unalias", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));
}

#[test]
fn test_builtin_umask_invalid_mode() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("umask", &[BString::from("invalid_mode")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));
}

#[test]
fn test_builtin_readonly_errors_and_flags() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    state.make_readonly("RO_VAR");
    let res =
        run_builtin("readonly", &[BString::from("RO_VAR=new_val")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));

    let res = run_builtin("readonly", &[BString::from("NEW_RO")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.is_readonly("NEW_RO"));
}

#[test]
fn test_builtin_hash_options_and_errors() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("hash", &[BString::from("-invalid")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    let res =
        run_builtin("hash", &[BString::from("nonexistent_cmd_xyz")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));
}

#[test]
fn test_builtin_ulimit_flags_and_errors() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("ulimit", &[BString::from("-c")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin("ulimit", &[BString::from("-n")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "ulimit",
        &[BString::from("-f"), BString::from("unlimited")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res =
        run_builtin("ulimit", &[BString::from("-a"), BString::from("100")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(2));

    let res = run_builtin("ulimit", &[BString::from("-z")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));

    let res = run_builtin(
        "ulimit",
        &[BString::from("-f"), BString::from("invalid_num")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(2));
}

#[test]
fn test_essential_read_stream() {
    let mut state = ShellState::new();
    let mut out = Vec::new();
    let mut err = Vec::new();

    let mut input = Cursor::new(b"hello world\n");
    let code = builtin_read(&[BString::from("REPLY")], &mut state, &mut input, &mut out, &mut err);
    assert_eq!(code, 0);
    assert_eq!(state.get_var("REPLY").unwrap(), "hello world");

    let mut input2 = Cursor::new(b"foo\\ bar baz\n");
    let code = builtin_read(
        &[BString::from("-r"), BString::from("A"), BString::from("B")],
        &mut state,
        &mut input2,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
    assert_eq!(state.get_var("A").unwrap(), "foo\\");
    assert_eq!(state.get_var("B").unwrap(), "bar baz");

    let mut input3 = Cursor::new(b"line1\\\nline2\n");
    let code = builtin_read(&[BString::from("C")], &mut state, &mut input3, &mut out, &mut err);
    assert_eq!(code, 0);
    assert_eq!(state.get_var("C").unwrap(), "line1line2");

    state.make_readonly("RO_READ");
    let mut input4 = Cursor::new(b"data\n");
    let code =
        builtin_read(&[BString::from("RO_READ")], &mut state, &mut input4, &mut out, &mut err);
    assert_eq!(code, 2);
}

#[test]
fn test_essential_type_stream() {
    let mut state = ShellState::new();
    let mut in_stream = Cursor::new(b"");
    let mut out = Vec::new();
    let mut err = Vec::new();

    state.add_function(BString::from("my_fn"), vec![]);
    let code = builtin_type(
        &[BString::from("my_fn"), BString::from("cd"), BString::from("nonexistent_cmd_12345")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, EXIT_NOT_FOUND);
}

#[test]
fn test_essential_alias_unalias_stream() {
    let mut state = ShellState::new();
    let mut in_stream = Cursor::new(b"");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = builtin_alias(&[], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code = builtin_alias(
        &[BString::from("myalias=echo 1")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);

    let code =
        builtin_alias(&[BString::from("myalias")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code = builtin_alias(
        &[BString::from("no_alias_found")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 1);

    let code = builtin_unalias(
        &[BString::from("myalias")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);

    let code = builtin_unalias(&[], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 1);

    let code = builtin_unalias(
        &[BString::from("no_alias_found")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 1);

    let code =
        builtin_unalias(&[BString::from("-a")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);
}

#[test]
fn test_essential_trap_stream() {
    let mut state = ShellState::new();
    let mut in_stream = Cursor::new(b"");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = builtin_trap(&[], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code = builtin_trap(&[BString::from("-l")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 2);

    let code = builtin_trap(
        &[BString::from("echo trap_triggered"), BString::from("INT")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);

    let code = builtin_trap(
        &[BString::from("-"), BString::from("INT")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);

    let code =
        builtin_trap(&[BString::from("SIGINT")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code = builtin_trap(
        &[BString::from("echo hi"), BString::from("INVALID-SIG-999")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 2);
}

#[test]
fn test_essential_getopts_stream() {
    let mut state = ShellState::new();
    let mut in_stream = Cursor::new(b"");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = builtin_getopts(&[], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, EXIT_SYNTAX_ERROR);

    state.set_args(vec![BString::from("-a"), BString::from("-b")]);
    let code = builtin_getopts(
        &[BString::from("ab"), BString::from("OPT")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
    assert_eq!(state.get_var("OPT").unwrap(), "a");

    state.set_var("OPTIND", "1");
    state.optopt_offset = 1;
    let code = builtin_getopts(
        &[BString::from(":b:"), BString::from("OPT"), BString::from("-b")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);

    state.set_var("OPTIND", "1");
    state.optopt_offset = 1;
    let code = builtin_getopts(
        &[BString::from("b:"), BString::from("OPT"), BString::from("-b")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);

    state.set_var("OPTIND", "1");
    state.optopt_offset = 1;
    let code = builtin_getopts(
        &[BString::from("a"), BString::from("OPT"), BString::from("-z")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
}

#[test]
fn test_essential_hash_stream() {
    let mut state = ShellState::new();
    let mut in_stream = Cursor::new(b"");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = builtin_hash(&[], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code = builtin_hash(&[BString::from("-r")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code = builtin_hash(&[BString::from("-z")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 1);

    let code = builtin_hash(
        &[BString::from("nonexistent_cmd_12345")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 1);
}

#[test]
fn test_essential_ulimit_stream() {
    let mut state = ShellState::new();
    let mut in_stream = Cursor::new(b"");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code =
        builtin_ulimit(&[BString::from("--help")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 2);

    let code =
        builtin_ulimit(&[BString::from("-a")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code =
        builtin_ulimit(&[BString::from("-f")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code =
        builtin_ulimit(&[BString::from("-c")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code =
        builtin_ulimit(&[BString::from("-n")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code = builtin_ulimit(
        &[BString::from("-f"), BString::from("1024")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);

    let code = builtin_ulimit(
        &[BString::from("-f"), BString::from("unlimited")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);

    let code = builtin_ulimit(
        &[BString::from("-a"), BString::from("100")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 2);

    let code =
        builtin_ulimit(&[BString::from("-z")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 2);

    let code = builtin_ulimit(
        &[BString::from("-f"), BString::from("invalid_num")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 2);
}

#[test]
fn test_essential_dot_script() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = builtin_dot(&[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SYNTAX_ERROR));

    let res = builtin_dot(&[BString::from("/nonexistent_file_xyz_12345.sh")], &mut state, &mut ctx)
        .unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_NOT_FOUND));

    let tmp_dir = std::env::temp_dir();
    let script_path = tmp_dir.join("test_script_dot.sh");
    std::fs::write(&script_path, b"TEST_DOT_VAR=100\n").unwrap();

    let script_bstr = BString::from(<[u8]>::from_path(&script_path).unwrap());
    let res = builtin_dot(&[script_bstr], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("TEST_DOT_VAR").unwrap(), "100");

    let _ = std::fs::remove_file(script_path);
}

#[test]
fn test_essential_wait_stream() {
    let mut state = ShellState::new();
    let mut in_stream = Cursor::new(b"");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = builtin_wait(&[], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code = builtin_wait(&[BString::from("--")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code = builtin_wait(&[BString::from("%1")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 127);

    let code = builtin_wait(
        &[BString::from("invalid_pid")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 2);
}

#[test]
fn test_essential_eval_exec_and_flow_control() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = builtin_eval(
        &[BString::from("FOO_EVAL=123"), BString::from("BAR_EVAL=456")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("FOO_EVAL").unwrap(), "123");

    let res = builtin_exec(&[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = builtin_return(&[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Return(0));

    let res = builtin_return(&[BString::from("7")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Return(7));

    let res = builtin_return(&[BString::from("invalid")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SYNTAX_ERROR));

    let res = builtin_break(&[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = builtin_break(&[BString::from("2")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = builtin_break(&[BString::from("invalid")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SYNTAX_ERROR));

    let res = builtin_continue(&[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = builtin_continue(&[BString::from("3")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = builtin_continue(&[BString::from("invalid")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SYNTAX_ERROR));
}

#[test]
fn test_builtin_cd_and_pwd_dash_semantics() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("pwd", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin("pwd", &[BString::from("-L")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin("pwd", &[BString::from("-P")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res =
        run_builtin("cd", &[BString::from("-L"), BString::from("/tmp")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin("cd", &[BString::from("-P"), BString::from("/")], &mut state, &mut ctx)
        .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    state.set_var("CDPATH", "/");
    let res = run_builtin("cd", &[BString::from("tmp")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_eval_dash_semantics() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // 1. Empty arguments returns status 0
    let res = builtin_eval(&[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // 2. Space-joining of positional arguments and mutating shell environment directly
    let args = vec![
        BString::from("EVAL_A=foo;"),
        BString::from("EVAL_B=bar;"),
        BString::from("export EVAL_C=baz"),
    ];
    let res = builtin_eval(&args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("EVAL_A").unwrap(), "foo");
    assert_eq!(state.get_var("EVAL_B").unwrap(), "bar");
    assert_eq!(state.get_var("EVAL_C").unwrap(), "baz");

    // 3. Status propagation from evaluated string
    let exit_args = vec![BString::from("exit 42")];
    let res = builtin_eval(&exit_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Exit(42));

    let return_args = vec![BString::from("return 15")];
    let res = builtin_eval(&return_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Return(15));
}

#[test]
fn test_builtin_exec_dash_semantics() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // 1. 0-arg exec returns Code(0)
    let res = builtin_exec(&[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // 2. exec -- returns Code(0)
    let res = builtin_exec(&[BString::from("--")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // 3. Invalid option returns Code(2)
    let res = builtin_exec(&[BString::from("-z")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));

    // 4. Missing argument for -a returns Code(2)
    let res = builtin_exec(&[BString::from("-a")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));

    // 5. Non-existent command returns Exit(127)
    let res =
        builtin_exec(&[BString::from("non_existent_command_12345")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Exit(127));

    // 6. 0-arg exec with redirection persists redirections on ctx
    let outcome =
        crate::eval::eval_string(b"exec > /dev/null".as_bstr(), &mut state, &mut ctx).unwrap();
    assert_eq!(outcome, EvalOutcome::Code(0));
}

#[test]
fn test_builtin_jobs_bg_fg_errors() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // No jobs present errors
    let res = run_builtin("fg", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(2));

    let res = run_builtin("bg", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(2));

    let bad_job = vec![BString::from("%1")];
    let res = run_builtin("jobs", &bad_job, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(1));

    let res = run_builtin("fg", &bad_job, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(2));

    let res = run_builtin("bg", &bad_job, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(2));

    // Illegal option errors
    let bad_opt = vec![BString::from("-z")];
    let res = run_builtin("jobs", &bad_opt, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(2));

    let res = run_builtin("fg", &bad_opt, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(2));

    let res = run_builtin("bg", &bad_opt, &mut state, &mut ctx).unwrap();
    assert_eq!(res, crate::eval::EvalOutcome::Code(2));
}

#[test]
fn test_builtin_set_dash_option_semantics() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // -o and +o listing options
    let res = run_builtin("set", &[BString::from("-o")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin("set", &[BString::from("+o")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // set -o nounset
    let res =
        run_builtin("set", &[BString::from("-o"), BString::from("nounset")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.opt_nounset);

    // set +o nounset
    let res =
        run_builtin("set", &[BString::from("+o"), BString::from("nounset")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(!state.opt_nounset);

    // set - disables -x and -v without changing positional arguments
    state.opt_xtrace = true;
    state.opt_verbose = true;
    state.set_args(vec![BString::from("p1")]);
    let res = run_builtin("set", &[BString::from("-")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(!state.opt_xtrace);
    assert!(!state.opt_verbose);
    assert_eq!(state.args, vec![BString::from("p1")]);

    // set -- resets positional parameters when no positional args follow
    let res = run_builtin("set", &[BString::from("--")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.args.is_empty());
}

#[test]
fn test_builtin_trap_dash_semantics() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // Set traps using signal numbers and names with/without SIG prefix
    let res = run_builtin(
        "trap",
        &[BString::from("echo hi"), BString::from("1"), BString::from("SIGINT")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SUCCESS));
    assert_eq!(state.traps.get(BStr::new("HUP")).unwrap(), "echo hi");
    assert_eq!(state.traps.get(BStr::new("INT")).unwrap(), "echo hi");

    // Single argument trap resets signal
    let res = run_builtin("trap", &[BString::from("HUP")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SUCCESS));
    assert!(state.traps.get(BStr::new("HUP")).is_none());

    // Reset with '-' action
    let res = run_builtin("trap", &[BString::from("-"), BString::from("2")], &mut state, &mut ctx)
        .unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SUCCESS));
    assert!(state.traps.get(BStr::new("INT")).is_none());

    // EXIT trap (signal 0 / EXIT / SIGEXIT)
    let res =
        run_builtin("trap", &[BString::from("cleanup"), BString::from("0")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SUCCESS));
    assert_eq!(state.traps.get(BStr::new("EXIT")).unwrap(), "cleanup");

    // Listing format with 0 arguments or --
    let res = run_builtin("trap", &[BString::from("--")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SUCCESS));
}

#[test]
fn test_builtin_true_false_colon() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // true returns 0 regardless of args or option flags
    assert_eq!(run_builtin("true", &[], &mut state, &mut ctx).unwrap(), EvalOutcome::Code(0));
    assert_eq!(
        run_builtin("true", &[BString::from("--help"), BString::from("-x")], &mut state, &mut ctx)
            .unwrap(),
        EvalOutcome::Code(0)
    );
    assert_eq!(
        run_builtin("true", &[BString::from("arg1"), BString::from("arg2")], &mut state, &mut ctx)
            .unwrap(),
        EvalOutcome::Code(0)
    );

    // : returns 0 regardless of args or option flags
    assert_eq!(run_builtin(":", &[], &mut state, &mut ctx).unwrap(), EvalOutcome::Code(0));
    assert_eq!(
        run_builtin(":", &[BString::from("-a"), BString::from("--foo")], &mut state, &mut ctx)
            .unwrap(),
        EvalOutcome::Code(0)
    );
    assert_eq!(
        run_builtin(":", &[BString::from("some"), BString::from("args")], &mut state, &mut ctx)
            .unwrap(),
        EvalOutcome::Code(0)
    );

    // false returns 1 regardless of args or option flags
    assert_eq!(run_builtin("false", &[], &mut state, &mut ctx).unwrap(), EvalOutcome::Code(1));
    assert_eq!(
        run_builtin("false", &[BString::from("--help"), BString::from("-v")], &mut state, &mut ctx)
            .unwrap(),
        EvalOutcome::Code(1)
    );
    assert_eq!(
        run_builtin("false", &[BString::from("arg1"), BString::from("arg2")], &mut state, &mut ctx)
            .unwrap(),
        EvalOutcome::Code(1)
    );
}

#[test]
fn test_builtin_umask_illegal_option() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("umask", &[BString::from("-x")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(EXIT_SYNTAX_ERROR));
}

#[test]
fn test_builtin_unset_dash_semantics() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // Default without flags unsets variable
    state.set_var("VAR1", "val1");
    let res = run_builtin("unset", &[BString::from("VAR1")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_var("VAR1").is_none());

    // Option -v unsets variable
    state.set_var("VAR2", "val2");
    let res =
        run_builtin("unset", &[BString::from("-v"), BString::from("VAR2")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_var("VAR2").is_none());

    // Option -f unsets function
    state.add_function(BString::from("func1"), vec![]);
    let res =
        run_builtin("unset", &[BString::from("-f"), BString::from("func1")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_function("func1").is_none());

    // Combined options (-vf: last option -f wins)
    state.set_var("BOTH", "val");
    state.add_function(BString::from("BOTH"), vec![]);
    let res =
        run_builtin("unset", &[BString::from("-vf"), BString::from("BOTH")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_function("BOTH").is_none());
    assert_eq!(state.get_var("BOTH").unwrap(), "val");

    // Combined options (-fv: last option -v wins)
    state.add_function(BString::from("BOTH2"), vec![]);
    state.set_var("BOTH2", "val2");
    let res =
        run_builtin("unset", &[BString::from("-fv"), BString::from("BOTH2")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_var("BOTH2").is_none());
    assert!(state.get_function("BOTH2").is_some());

    // Option parsing -- terminator
    state.set_var("-v", "val_dash_v");
    let res =
        run_builtin("unset", &[BString::from("--"), BString::from("-v")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_var("-v").is_none());

    // Illegal option -x returns EXIT_SYNTAX_ERROR (2)
    let res = run_builtin("unset", &[BString::from("-x")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));

    // Read-only variable protection returns EXIT_SYNTAX_ERROR (2)
    state.make_readonly("RO_VAR");
    let res = run_builtin("unset", &[BString::from("RO_VAR")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(2));

    // Read-only variable present, but unsetting function -f RO_VAR succeeds
    state.add_function(BString::from("RO_VAR"), vec![]);
    let res =
        run_builtin("unset", &[BString::from("-f"), BString::from("RO_VAR")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_function("RO_VAR").is_none());
    assert!(state.is_readonly("RO_VAR"));
}

#[test]
fn test_export_and_readonly_dash_semantics() {
    use crate::builtins::essential::{builtin_export, builtin_readonly};

    let mut state = ShellState::new();

    // Set some variables
    state.set_and_export_var("FOO", "hello");
    state.export_var("UNSET_EXP");

    state.set_var("RO_SET", "world");
    state.make_readonly("RO_SET");
    state.make_readonly("UNSET_RO");

    // Test export listing output format with single quotes and unset vars
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_export(&[], &mut state, &mut std::io::empty(), &mut stdout, &mut stderr);
    assert_eq!(code, 0);
    let out_str = String::from_utf8(stdout).unwrap();
    assert!(out_str.contains("export FOO='hello'\n"));
    assert!(out_str.contains("export UNSET_EXP\n"));

    // Test export -p (with or without extra operand)
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_export(
        &[BString::from("-p"), BString::from("FOO")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    let out_str = String::from_utf8(stdout).unwrap();
    assert!(out_str.contains("export FOO='hello'\n"));

    // Test export invalid option
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_export(
        &[BString::from("-z")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 2);
    let err_str = String::from_utf8(stderr).unwrap();
    assert!(err_str.contains("export: Illegal option -z"));

    // Test export bad variable name
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_export(
        &[BString::from("123INVALID=val")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 2);
    let err_str = String::from_utf8(stderr).unwrap();
    assert!(err_str.contains("export: 123INVALID: bad variable name"));

    // Test export attempt to modify readonly variable
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_export(
        &[BString::from("RO_SET=new_val")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 2);
    let err_str = String::from_utf8(stderr).unwrap();
    assert!(err_str.contains("export: RO_SET: is read only"));

    // Test export on existing readonly variable WITHOUT equals sign (should succeed)
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_export(
        &[BString::from("RO_SET")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);

    // Test readonly listing output format
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_readonly(&[], &mut state, &mut std::io::empty(), &mut stdout, &mut stderr);
    assert_eq!(code, 0);
    let out_str = String::from_utf8(stdout).unwrap();
    assert!(out_str.contains("readonly RO_SET='world'\n"));
    assert!(out_str.contains("readonly UNSET_RO\n"));

    // Test readonly invalid option
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_readonly(
        &[BString::from("-z")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 2);
    let err_str = String::from_utf8(stderr).unwrap();
    assert!(err_str.contains("readonly: Illegal option -z"));

    // Test readonly bad variable name
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_readonly(
        &[BString::from("a-b=val")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 2);
    let err_str = String::from_utf8(stderr).unwrap();
    assert!(err_str.contains("readonly: a-b: bad variable name"));

    // Test readonly attempt to modify readonly variable
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = builtin_readonly(
        &[BString::from("RO_SET=new_val")],
        &mut state,
        &mut std::io::empty(),
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 2);
    let err_str = String::from_utf8(stderr).unwrap();
    assert!(err_str.contains("readonly: RO_SET: is read only"));
}

#[test]
fn test_is_builtin_job_control() {
    assert!(is_builtin("jobs"));
    assert!(is_builtin("bg"));
    assert!(is_builtin("fg"));
}

#[test]
fn test_ulimit_soft_hard_and_all_options() {
    let mut state = ShellState::new();
    let mut in_stream = Cursor::new(b"");
    let mut out = Vec::new();
    let mut err = Vec::new();

    let code = builtin_ulimit(
        &[BString::from("-S"), BString::from("-f")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(out.clone()).unwrap(), "unlimited\n");
    out.clear();

    let code = builtin_ulimit(
        &[BString::from("-H"), BString::from("-f")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(out.clone()).unwrap(), "unlimited\n");
    out.clear();

    let code = builtin_ulimit(
        &[BString::from("-S"), BString::from("1024")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);

    let code = builtin_ulimit(
        &[BString::from("-S"), BString::from("-f")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(out.clone()).unwrap(), "1024\n");
    out.clear();

    let code = builtin_ulimit(
        &[BString::from("-H"), BString::from("-f")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(out.clone()).unwrap(), "unlimited\n");
    out.clear();

    let code =
        builtin_ulimit(&[BString::from("500")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);

    let code = builtin_ulimit(
        &[BString::from("-S"), BString::from("1000")],
        &mut state,
        &mut in_stream,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 2);
    assert!(String::from_utf8(err.clone()).unwrap().contains("Operation not permitted"));
    err.clear();

    let code =
        builtin_ulimit(&[BString::from("-p")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(out.clone()).unwrap(), "unlimited\n");
    out.clear();

    let code =
        builtin_ulimit(&[BString::from("-u")], &mut state, &mut in_stream, &mut out, &mut err);
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(out.clone()).unwrap(), "unlimited\n");
    out.clear();

    for flag in &["-t", "-d", "-s", "-c", "-m", "-l", "-n", "-v", "-w", "-r"] {
        let code =
            builtin_ulimit(&[BString::from(*flag)], &mut state, &mut in_stream, &mut out, &mut err);
        assert_eq!(code, 0, "flag {} failed", flag);
        out.clear();
    }
}
