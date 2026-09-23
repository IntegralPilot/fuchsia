// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::testing::{
    ResolvedAlias, apply_assignments, is_assignment_flat, parse_simple_command_args,
    resolve_alias_loop, split_assignment_flat,
};
use crate::eval::{EvalOutcome, ExecutionContext, ShellState, eval_simple};
use crate::parser::ast::{ASTBuilder, ResolvedWordPart};
use bstr::{BStr, BString};

#[test]
fn test_is_assignment_flat() {
    let mut builder = ASTBuilder::new();

    // Empty parts
    assert!(!is_assignment_flat(&[], &builder));

    // Non-literal tag
    let parts_non_literal = vec![ResolvedWordPart::Var(BString::from("VAR"))];
    let w_non_lit = builder.add_resolved_word(&parts_non_literal);
    let slice_non_lit = builder.get_slice(w_non_lit);
    assert!(!is_assignment_flat(slice_non_lit, &builder));

    // Literal with no '='
    let parts_no_eq = vec![ResolvedWordPart::Literal(BString::from("FOO"))];
    let w_no_eq = builder.add_resolved_word(&parts_no_eq);
    assert!(!is_assignment_flat(builder.get_slice(w_no_eq), &builder));

    // Empty variable name before '='
    let parts_empty_name = vec![ResolvedWordPart::Literal(BString::from("=VAL"))];
    let w_empty_name = builder.add_resolved_word(&parts_empty_name);
    assert!(!is_assignment_flat(builder.get_slice(w_empty_name), &builder));

    // Invalid start character (digit)
    let parts_digit_start = vec![ResolvedWordPart::Literal(BString::from("1VAR=VAL"))];
    let w_digit_start = builder.add_resolved_word(&parts_digit_start);
    assert!(!is_assignment_flat(builder.get_slice(w_digit_start), &builder));

    // Invalid inner character (hyphen)
    let parts_hyphen = vec![ResolvedWordPart::Literal(BString::from("VAR-NAME=VAL"))];
    let w_hyphen = builder.add_resolved_word(&parts_hyphen);
    assert!(!is_assignment_flat(builder.get_slice(w_hyphen), &builder));

    // Valid assignments
    let parts_valid1 = vec![ResolvedWordPart::Literal(BString::from("VAR=VAL"))];
    let w_valid1 = builder.add_resolved_word(&parts_valid1);
    assert!(is_assignment_flat(builder.get_slice(w_valid1), &builder));

    let parts_valid2 = vec![ResolvedWordPart::Literal(BString::from("_VAR123=VAL"))];
    let w_valid2 = builder.add_resolved_word(&parts_valid2);
    assert!(is_assignment_flat(builder.get_slice(w_valid2), &builder));
}

#[test]
fn test_split_assignment_flat() {
    let mut builder = ASTBuilder::new();
    let parts = vec![
        ResolvedWordPart::Literal(BString::from("MY_VAR=hello")),
        ResolvedWordPart::Var(BString::from("SUFFIX")),
    ];
    let w_slice = builder.add_resolved_word(&parts);
    let (name, val_start, remaining) = split_assignment_flat(builder.get_slice(w_slice), &builder);

    assert_eq!(name, "MY_VAR");
    assert_eq!(val_start, "hello");
    assert_eq!(remaining.len(), 1);
}

#[test]
fn test_parse_simple_command_args() {
    let mut builder = ASTBuilder::new();

    let arg_assign1 = builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("A=1"))]);
    let arg_assign2 = builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("B=2"))]);
    let arg_empty = builder.add_resolved_word(&[]);
    let arg_cmd = builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("echo"))]);
    let arg_assign3 = builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("C=3"))]);

    let cmd_ptr =
        builder.add_simple_command(&[arg_assign1, arg_assign2, arg_empty, arg_cmd, arg_assign3]);

    let (assignments, cmd_args) = parse_simple_command_args(&builder, cmd_ptr);
    assert_eq!(assignments.len(), 2);
    assert_eq!(cmd_args.len(), 3);
}

#[test]
fn test_apply_assignments_readonly_error() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    state.make_readonly(BStr::new("RO_VAR"));

    let mut builder = ASTBuilder::new();
    let arg_assign =
        builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("RO_VAR=123"))]);

    let res = apply_assignments(&builder, &[arg_assign], &mut state, &ctx);
    assert!(res.is_err());
}

#[test]
fn test_apply_assignments_and_backup_restoration() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    state.set_var(BStr::new("EXISTING"), BStr::new("old_val"));

    let mut builder = ASTBuilder::new();
    let arg_assign =
        builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("EXISTING=new_val"))]);

    {
        let guard = apply_assignments(&builder, &[arg_assign], &mut state, &ctx).unwrap();
        assert_eq!(guard.state.get_var(BStr::new("EXISTING")).unwrap(), "new_val");
    }
    assert_eq!(state.get_var(BStr::new("EXISTING")).unwrap(), "old_val");
}

#[test]
fn test_eval_simple_assignments_only() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let arg_assign1 =
        builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("X=100"))]);
    let arg_assign2 =
        builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("Y=200"))]);
    let cmd_ptr = builder.add_simple_command(&[arg_assign1, arg_assign2]);

    let res = eval_simple(&mut builder, cmd_ptr, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var(BStr::new("X")).unwrap(), "100");
    assert_eq!(state.get_var(BStr::new("Y")).unwrap(), "200");
}

#[test]
fn test_eval_simple_readonly_error() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.make_readonly(BStr::new("RO_VAR"));

    let mut builder = ASTBuilder::new();
    let arg_assign =
        builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("RO_VAR=test"))]);
    let cmd_ptr = builder.add_simple_command(&[arg_assign]);

    let res = eval_simple(&mut builder, cmd_ptr, &mut state, &mut ctx);
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("is read only"));
}

#[test]
fn test_eval_simple_function_execution() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let mut func_builder = ASTBuilder::new();
    let body_cmd = func_builder.add_empty_simple_command();
    let serialized = func_builder.get_ref(body_cmd).serialize(&func_builder);
    state.add_function(BString::from("my_func"), serialized);

    let mut builder = ASTBuilder::new();
    let arg_fn = builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("my_func"))]);
    let arg_param = builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("arg1"))]);
    let cmd_ptr = builder.add_simple_command(&[arg_fn, arg_param]);

    let res = eval_simple(&mut builder, cmd_ptr, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_resolve_alias_loop_words_and_command() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    state.aliases.insert(BString::from("myalias"), BString::from("echo hello"));

    let mut builder = ASTBuilder::new();
    let arg_alias =
        builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("myalias"))]);

    let resolved = resolve_alias_loop(&mut builder, vec![arg_alias], &mut state, &mut ctx).unwrap();
    match resolved {
        ResolvedAlias::Words(words) => {
            assert!(!words.is_empty());
        }
        _ => panic!("Expected words"),
    }
}
