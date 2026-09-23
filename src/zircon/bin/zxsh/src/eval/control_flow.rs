// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::execution_context::ExecutionContext;
use crate::eval::expand::{
    FieldSplitMode, TildeColonMode, expand_argument, expand_argument_no_split,
    expand_argument_to_word_chars,
};
use crate::eval::glob::match_segment_glob;
use crate::eval::spawn::spawn_pipeline_stage;
use crate::eval::state::{BgJob, IgnoreErrGuard, LoopNestGuard, ShellState};
use crate::eval::{EvalOutcome, eval_command};
use crate::parser::ast::{ASTBuilder, Command};
use crate::relative;

enum LoopControl {
    Break,
    Continue,
    Code(i32),
    Propagate(EvalOutcome),
}

fn handle_loop_body_outcome(outcome: EvalOutcome) -> LoopControl {
    match outcome {
        EvalOutcome::Code(code) => LoopControl::Code(code),
        EvalOutcome::Break(n) => {
            if n <= 1 {
                LoopControl::Break
            } else {
                LoopControl::Propagate(EvalOutcome::Break(n - 1))
            }
        }
        EvalOutcome::Continue(n) => {
            if n <= 1 {
                LoopControl::Continue
            } else {
                LoopControl::Propagate(EvalOutcome::Continue(n - 1))
            }
        }
        other => LoopControl::Propagate(other),
    }
}

pub fn eval_if(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let cond_outcome = {
        let mut guard = IgnoreErrGuard::new(state);
        let cond_ptr = builder.get_ref(cmd_ptr).cond;
        eval_command(builder, cond_ptr, &mut *guard, ctx)?
    };
    match cond_outcome {
        EvalOutcome::Exit(code) => Ok(EvalOutcome::Exit(code)),
        EvalOutcome::Code(0) => {
            let then_ptr = builder.get_ref(cmd_ptr).then_branch;
            eval_command(builder, then_ptr, state, ctx)
        }
        EvalOutcome::Code(_) => {
            let else_branch = builder.get_ref(cmd_ptr).else_branch;
            if !else_branch.is_null() {
                eval_command(builder, else_branch, state, ctx)
            } else {
                Ok(EvalOutcome::Code(0))
            }
        }
        non_code => Ok(non_code),
    }
}

pub fn eval_loop(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
    run_on_success: bool,
) -> Result<EvalOutcome, String> {
    let mut loop_guard = LoopNestGuard::new(state);
    let state = &mut *loop_guard;
    let mut last_code = 0;
    loop {
        if let Some(code) = ctx.signal_state.pending_exit_code() {
            return Ok(EvalOutcome::Code(code));
        }
        let cond_outcome = {
            let mut guard = IgnoreErrGuard::new(state);
            let cond_ptr = builder.get_ref(cmd_ptr).cond;
            eval_command(builder, cond_ptr, &mut *guard, ctx)?
        };
        match cond_outcome {
            EvalOutcome::Exit(code) => return Ok(EvalOutcome::Exit(code)),
            EvalOutcome::Code(code) => {
                let is_success = code == 0;
                if is_success == run_on_success {
                    let then_ptr = builder.get_ref(cmd_ptr).then_branch;
                    let body_outcome = eval_command(builder, then_ptr, state, ctx)?;
                    match handle_loop_body_outcome(body_outcome) {
                        LoopControl::Break => break,
                        LoopControl::Continue => {}
                        LoopControl::Code(code) => last_code = code,
                        LoopControl::Propagate(outcome) => return Ok(outcome),
                    }
                } else {
                    break;
                }
            }
            non_code => return Ok(non_code),
        }
    }
    Ok(EvalOutcome::Code(last_code))
}

pub fn eval_for(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let mut loop_guard = LoopNestGuard::new(state);
    let state = &mut *loop_guard;
    let mut last_code = 0;
    let mut expanded_items = Vec::new();
    let items = builder.get_ref(cmd_ptr).for_items;
    for i in 0..items.len() {
        let item = items.as_slice(builder)[i];
        expanded_items.extend(expand_argument(item.as_slice(builder), state, ctx, builder)?);
    }

    let var_name = builder.get_ref(cmd_ptr).for_var.to_bstring(builder);

    for item in expanded_items {
        if let Some(code) = ctx.signal_state.pending_exit_code() {
            return Ok(EvalOutcome::Code(code));
        }
        if state.is_readonly(&var_name) {
            return Err(format!("{}: is read only", var_name));
        }
        state.set_var(&var_name, &item);
        let then_ptr = builder.get_ref(cmd_ptr).then_branch;
        let outcome = eval_command(builder, then_ptr, state, ctx)?;
        match handle_loop_body_outcome(outcome) {
            LoopControl::Break => break,
            LoopControl::Continue => continue,
            LoopControl::Code(code) => last_code = code,
            LoopControl::Propagate(outcome) => return Ok(outcome),
        }
    }
    Ok(EvalOutcome::Code(last_code))
}

pub fn eval_case(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let expanded_word = {
        let cmd = builder.get_ref(cmd_ptr);
        expand_argument_no_split(cmd.case_word.as_slice(builder), state, ctx, builder)?
    };

    let cases = builder.get_ref(cmd_ptr).case_items;

    for i in 0..cases.len() {
        let mut matched = false;
        let patterns = cases.as_slice(builder)[i].patterns;

        for j in 0..patterns.len() {
            let pat_arg = patterns.as_slice(builder)[j];
            let word_chars_list = expand_argument_to_word_chars(
                pat_arg.as_slice(builder),
                state,
                ctx,
                TildeColonMode::DoNotExpandAfterColons,
                FieldSplitMode::DoNotSplit,
                builder,
            )?;
            if !word_chars_list.is_empty() {
                let pat_word = &word_chars_list[0];
                if match_segment_glob(pat_word, expanded_word.as_ref()) {
                    matched = true;
                    break;
                }
            }
        }
        if matched {
            let body_ptr = cases.as_slice(builder)[i].body;
            return eval_command(builder, body_ptr, state, ctx);
        }
    }
    Ok(EvalOutcome::Code(0))
}

pub fn eval_logical_and(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let outcome = {
        let mut guard = IgnoreErrGuard::new(state);
        let left_ptr = builder.get_ref(cmd_ptr).left;
        eval_command(builder, left_ptr, &mut *guard, ctx)?
    };
    match outcome {
        EvalOutcome::Exit(code) => Ok(EvalOutcome::Exit(code)),
        EvalOutcome::Code(0) => {
            let right_ptr = builder.get_ref(cmd_ptr).right;
            eval_command(builder, right_ptr, state, ctx)
        }
        EvalOutcome::Code(code) => Ok(EvalOutcome::Code(code)),
        non_code => Ok(non_code),
    }
}

pub fn eval_logical_or(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let outcome = {
        let mut guard = IgnoreErrGuard::new(state);
        let left_ptr = builder.get_ref(cmd_ptr).left;
        eval_command(builder, left_ptr, &mut *guard, ctx)?
    };
    match outcome {
        EvalOutcome::Exit(code) => Ok(EvalOutcome::Exit(code)),
        EvalOutcome::Code(0) => Ok(EvalOutcome::Code(0)),
        EvalOutcome::Code(_) => {
            let right_ptr = builder.get_ref(cmd_ptr).right;
            eval_command(builder, right_ptr, state, ctx)
        }
        non_code => Ok(non_code),
    }
}

pub fn eval_background(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let left_ptr = builder.get_ref(cmd_ptr).left;
    let proc = spawn_pipeline_stage(
        builder,
        left_ptr,
        state,
        ctx,
        ctx.stdin(),
        ctx.stdout(),
        ctx.stderr(),
    )?;
    if let Ok(koid) = proc.koid() {
        let raw_koid = koid.raw_koid();
        state.last_bg_pid = Some(raw_koid);
    }
    let cmd = crate::eval::format::command_to_bstring(builder.get_ref(left_ptr), builder);
    state.bg_jobs.push(BgJob { process: proc, cmd });
    Ok(EvalOutcome::Code(0))
}

pub fn eval_sequence(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let mut last_code = 0;
    let seq = builder.get_ref(cmd_ptr).sequence;
    for i in 0..seq.len() {
        if let Some(code) = ctx.signal_state.pending_exit_code() {
            return Ok(EvalOutcome::Code(code));
        }
        let child_ptr = seq.as_slice(builder)[i];
        match eval_command(builder, child_ptr, state, ctx)? {
            EvalOutcome::Code(code) => last_code = code,
            non_code => return Ok(non_code),
        }
    }
    Ok(EvalOutcome::Code(last_code))
}
