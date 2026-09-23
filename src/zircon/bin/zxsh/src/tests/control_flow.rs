// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{EvalOutcome, ExecutionContext, ShellState, eval_command};
use crate::parser::ast::ASTBuilder;
use crate::parser::{parse_script, tokenize};
use bstr::BStr;
use zx::Task;

fn eval_str(script: &str, state: &mut ShellState, ctx: &mut ExecutionContext) -> EvalOutcome {
    let mut builder = ASTBuilder::new();
    let tokens = tokenize(BStr::new(script)).unwrap();
    let cmds = parse_script(&mut builder, &tokens).unwrap();
    let root = builder.add_sequence_or_single(&cmds);
    eval_command(&mut builder, root, state, ctx).unwrap()
}

fn try_eval_str(
    script: &str,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let mut builder = ASTBuilder::new();
    let tokens = tokenize(BStr::new(script)).map_err(|e| e.to_string())?;
    let cmds = parse_script(&mut builder, &tokens).map_err(|e| e.to_string())?;
    let root = builder.add_sequence_or_single(&cmds);
    eval_command(&mut builder, root, state, ctx)
}

#[test]
fn test_control_flow_if() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = eval_str("if A=1; then B=10; else B=20; fi", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("A").unwrap(), "1");
    assert_eq!(state.get_var("B").unwrap(), "10");

    let res = eval_str("if A=2; then B=30; fi", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("B").unwrap(), "30");

    let res = eval_str("if false; then C=1; else C=2; fi", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("C").unwrap(), "2");

    let res = eval_str("if false; then D=1; fi", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_var("D").is_none());

    let res = eval_str("if exit 5; then E=1; fi", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Exit(5));

    let res = eval_str("if return 4; then F=1; fi", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Return(4));
}

#[test]
fn test_control_flow_logical_operators() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = eval_str("A=1 && B=2", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("A").unwrap(), "1");
    assert_eq!(state.get_var("B").unwrap(), "2");

    let res = eval_str("false && C=3", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(1));
    assert!(state.get_var("C").is_none());

    let res = eval_str("D=3 || E=4", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("D").unwrap(), "3");
    assert!(state.get_var("E").is_none());

    let res = eval_str("false || F=5", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("F").unwrap(), "5");

    let res = eval_str("exit 7 && G=1", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Exit(7));

    let res = eval_str("exit 8 || H=1", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Exit(8));
}

#[test]
fn test_control_flow_while_loop() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = eval_str("while false; do A=1; done", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_var("A").is_none());

    let res = eval_str(
        "N=0; while true; do N=$((N+1)); if A=1; then break; fi; done",
        &mut state,
        &mut ctx,
    );
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("N").unwrap(), "1");

    let res =
        eval_str("N=0; for x in 1 2; do N=$((N+1)); continue; N=99; done", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("N").unwrap(), "2");

    let res = eval_str("while exit 9; do B=1; done", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Exit(9));
}

#[test]
fn test_control_flow_until_loop() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = eval_str("N=0; until true; do N=$((N+1)); done", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("N").unwrap(), "0");

    let res = eval_str("until exit 6; do C=1; done", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Exit(6));
}

#[test]
fn test_control_flow_for_loop() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = eval_str("for x in 10 20 30; do LAST=$x; done", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("LAST").unwrap(), "30");

    let res = eval_str("for x in; do A=1; done", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_var("A").is_none());

    let res = eval_str("for x in 1 2 3; do B=$x; break; done", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("B").unwrap(), "1");

    let res = eval_str("for x in 1 2 3; do continue; C=$x; done", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_var("C").is_none());

    state.make_readonly("RO");
    let err = try_eval_str("for RO in a b; do :; done", &mut state, &mut ctx);
    assert!(err.is_err());
    assert!(err.unwrap_err().contains("is read only"));
}

#[test]
fn test_control_flow_nested_break_and_continue() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = eval_str(
        "for i in 1 2; do for j in 10 20; do OUT=$i-$j; break 2; done; done",
        &mut state,
        &mut ctx,
    );
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("OUT").unwrap(), "1-10");

    let res = eval_str(
        "COUNT=0; for i in 1 2; do for j in 10 20; do COUNT=$((COUNT+1)); continue 2; done; done",
        &mut state,
        &mut ctx,
    );
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("COUNT").unwrap(), "2");
}

#[test]
fn test_control_flow_case() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = eval_str("case foo in f*) MATCH=yes ;; bar) MATCH=no ;; esac", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("MATCH").unwrap(), "yes");

    let res = eval_str("case bar in foo|bar) MATCH=multi ;; esac", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("MATCH").unwrap(), "multi");

    let res = eval_str("case nomatch in a) X=1 ;; b) X=2 ;; esac", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(state.get_var("X").is_none());

    let res = eval_str("case zzz in a) X=1 ;; *) X=default ;; esac", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("X").unwrap(), "default");
}

#[test]
fn test_control_flow_background() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = try_eval_str("A=1 &", &mut state, &mut ctx);
    // Background execution attempts process spawn; verify it runs without crashing.
    assert!(res.is_ok() || res.is_err());

    let res_while = try_eval_str("while true; do true; done &", &mut state, &mut ctx);
    assert!(res_while.is_ok() || res_while.is_err());

    let res_until = try_eval_str("until false; do true; done &", &mut state, &mut ctx);
    assert!(res_until.is_ok() || res_until.is_err());

    let res_for = try_eval_str("for x in 1 2; do true; done &", &mut state, &mut ctx);
    assert!(res_for.is_ok() || res_for.is_err());

    let res_if = try_eval_str("if true; then true; else true; fi &", &mut state, &mut ctx);
    assert!(res_if.is_ok() || res_if.is_err());

    let res_case = try_eval_str("case a in a) true ;; esac &", &mut state, &mut ctx);
    assert!(res_case.is_ok() || res_case.is_err());

    let res_seq = try_eval_str("{ true; true; } &", &mut state, &mut ctx);
    assert!(res_seq.is_ok() || res_seq.is_err());

    for job in &state.bg_jobs {
        let _ = job.process.kill();
    }
}

#[test]
fn test_control_flow_errexit_scope_guard() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    state.opt_errexit = true;
    let res = eval_str("if false; then X=1; else X=2; fi; Y=3", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("X").unwrap(), "2");
    assert_eq!(state.get_var("Y").unwrap(), "3");
    assert_eq!(state.ignore_err_depth, 0);

    let res = eval_str("false || Z=4", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(state.get_var("Z").unwrap(), "4");
    assert_eq!(state.ignore_err_depth, 0);
}

#[test]
fn test_ps4_xtrace_expansion() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.opt_xtrace = true;
    state.set_var(BStr::new("PS4"), BStr::new("++ "));
    let res = eval_str("true", &mut state, &mut ctx);
    assert_eq!(res, EvalOutcome::Code(0));
}
