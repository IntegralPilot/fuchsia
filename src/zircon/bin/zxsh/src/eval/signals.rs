// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{EvalOutcome, ExecutionContext, ShellState, eval_string};
use crate::tty::ShellSignals;
use bstr::BStr;

pub fn run_pending_traps(
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<Option<EvalOutcome>, String> {
    let pending = ctx.signal_state.take_pending();
    if pending.is_empty() {
        return Ok(None);
    }
    for &(sig, sig_name) in ShellSignals::ALL {
        if pending.contains(sig) {
            if let Some(action) = state.traps.get(BStr::new(sig_name)).cloned() {
                if action.is_empty() {
                    continue;
                }
                let outcome = eval_string(action.as_ref(), state, ctx)?;
                if !matches!(outcome, EvalOutcome::Code(_)) {
                    return Ok(Some(outcome));
                }
            } else if sig == ShellSignals::INT && state.opt_interactive {
                return Err("".to_string());
            } else if let Some(exit_code) = sig.exit_code() {
                return Ok(Some(EvalOutcome::Exit(exit_code)));
            }
        }
    }
    Ok(None)
}

pub fn run_exit_trap(state: &mut ShellState, ctx: &mut ExecutionContext) {
    if let Some(action) = state.traps.get(BStr::new(b"EXIT")).cloned() {
        if let Err(err) = eval_string(action.as_ref(), state, ctx) {
            eprintln!("trap EXIT error: {}", err);
        }
    }
}
