// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{EvalOutcome, ExecutionContext, ShellState, run_exit_trap, run_pending_traps};
use crate::tty::ShellSignals;
use bstr::{BStr, BString};

#[test]
fn test_run_pending_traps_empty() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    let res = run_pending_traps(&mut state, &mut ctx).unwrap();
    assert!(res.is_none());
}

#[test]
fn test_run_pending_traps_with_handler() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.traps.insert(BString::from("INT"), BString::from("FOO=trapped"));

    // Signal SIGINT
    ctx.signal_state.set(ShellSignals::INT);

    let res = run_pending_traps(&mut state, &mut ctx).unwrap();
    assert_eq!(res, None);
    assert_eq!(state.get_var(BStr::new("FOO")).unwrap(), "trapped");
}

#[test]
fn test_run_pending_traps_empty_action() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.traps.insert(BString::from("INT"), BString::from(""));

    ctx.signal_state.set(ShellSignals::INT);

    let res = run_pending_traps(&mut state, &mut ctx).unwrap();
    assert_eq!(res, None);
}

#[test]
fn test_run_pending_traps_term_handler() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.traps.insert(BString::from("TERM"), BString::from("BAR=trapped"));

    ctx.signal_state.set(ShellSignals::TERM);

    let res = run_pending_traps(&mut state, &mut ctx).unwrap();
    assert_eq!(res, None);
    assert_eq!(state.get_var(BStr::new("BAR")).unwrap(), "trapped");
}

#[test]
fn test_run_pending_traps_interactive_sigint() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.opt_interactive = true;

    ctx.signal_state.set(ShellSignals::INT);

    let res = run_pending_traps(&mut state, &mut ctx);
    assert!(res.is_err());
    assert_eq!(res.unwrap_err(), "");
}

#[test]
fn test_run_pending_traps_default_sigint_exit() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.opt_interactive = false;

    ctx.signal_state.set(ShellSignals::INT);

    let res = run_pending_traps(&mut state, &mut ctx).unwrap();
    assert_eq!(res, Some(EvalOutcome::Exit(130)));
}

#[test]
fn test_run_pending_traps_default_sigterm_exit() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.opt_interactive = false;

    ctx.signal_state.set(ShellSignals::TERM);

    let res = run_pending_traps(&mut state, &mut ctx).unwrap();
    assert_eq!(res, Some(EvalOutcome::Exit(143)));
}

#[test]
fn test_run_pending_traps_other_signals() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.traps.insert(BString::from("HUP"), BString::from("HUP_VAR=1"));
    state.traps.insert(BString::from("QUIT"), BString::from("QUIT_VAR=1"));

    ctx.signal_state.set(ShellSignals::HUP);
    ctx.signal_state.set(ShellSignals::QUIT);

    let res = run_pending_traps(&mut state, &mut ctx).unwrap();
    assert_eq!(res, None);
    assert_eq!(state.get_var(BStr::new("HUP_VAR")).unwrap(), "1");
    assert_eq!(state.get_var(BStr::new("QUIT_VAR")).unwrap(), "1");
}

#[test]
fn test_run_exit_trap() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // Normal exit trap execution
    state.traps.insert(BString::from("EXIT"), BString::from("CLEANUP=done"));
    run_exit_trap(&mut state, &mut ctx);
    assert_eq!(state.get_var(BStr::new("CLEANUP")).unwrap(), "done");

    // Exit trap error handling
    state.traps.insert(BString::from("EXIT"), BString::from("syntax_error ( invalid"));
    run_exit_trap(&mut state, &mut ctx);
}
