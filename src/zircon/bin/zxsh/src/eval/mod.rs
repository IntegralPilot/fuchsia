// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(unused_imports)]

use bstr::BStr;
use std::io::Write;

use crate::parser::ast::{ASTBuilder, Command, CommandTag};
use crate::parser::{parse_script, tokenize};
use crate::relative;

mod arithmetic;
mod control_flow;
mod execution_context;
mod expand;
mod format;
mod glob;
mod redirect;
mod signals;
mod simple;
mod spawn;
mod state;

// Imports for the implementation in this file
use crate::subshell::SubshellScriptArgs;

use control_flow::{
    eval_background, eval_case, eval_for, eval_if, eval_logical_and, eval_logical_or, eval_loop,
    eval_sequence,
};
pub use simple::eval_simple;
pub use spawn::{eval_pipeline, spawn_subshell_vmo, wait_for_process_to_exit};

// Public re-exports
pub use crate::process::clone_fd_to_action;
pub use crate::tty::{ShellSignalState, ShellSignals};
pub use execution_context::{ClosedReader, ClosedWriter, ExecutionContext};
pub use expand::{expand_prompt, expand_string};
pub use format::command_to_bstring;
pub use redirect::eval_redirect;
pub use signals::{run_exit_trap, run_pending_traps};
pub use state::{
    BgJob, EXIT_CANNOT_EXEC, EXIT_FAILURE, EXIT_NOT_FOUND, EXIT_SUCCESS, EXIT_SYNTAX_ERROR,
    RLIM_INFINITY, RLIMIT_AS, RLIMIT_CORE, RLIMIT_CPU, RLIMIT_DATA, RLIMIT_FSIZE, RLIMIT_LOCKS,
    RLIMIT_MEMLOCK, RLIMIT_NOFILE, RLIMIT_NPROC, RLIMIT_RSS, RLIMIT_RTPRIO, RLIMIT_STACK, Rlimit,
    ShellEnv, ShellPath, ShellState, VarName,
};

#[cfg(test)]
pub mod testing {
    pub use super::arithmetic::evaluate_arithmetic;
    pub use super::expand::{
        ExpandedCommand, append_args_to_command, expand_alias, expand_argument,
        expand_assignment_value, expand_var_with_modifiers, get_literal_command_name,
        needs_subshell_process,
    };
    pub use super::glob::{WordChar, expand_glob, match_glob, match_segment_glob};
    pub use super::redirect::{HEREDOC_INLINE_THRESHOLD, apply_redirects};
    pub use super::simple::{
        ResolvedAlias, apply_assignments, is_assignment_flat, parse_simple_command_args,
        resolve_alias_loop, split_assignment_flat,
    };
    pub use super::spawn::spawn_command_with_redirection;
    pub use super::state::Frame;
}

/// Represents the outcome of evaluating a shell command or statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalOutcome {
    /// Normal command execution completion with an exit status code.
    Code(i32),
    /// Explicit shell exit request (e.g. via `exit` builtin) with a status code.
    Exit(i32),
    /// Return from a shell function or sourced script with a status code.
    Return(i32),
    /// Break out of `N` enclosing loop levels.
    Break(u32),
    /// Continue execution at the next iteration of `N` enclosing loop levels.
    Continue(u32),
}

impl EvalOutcome {
    /// Returns the effective numeric exit status code for this outcome.
    pub fn exit_code(&self) -> i32 {
        match self {
            EvalOutcome::Code(code) | EvalOutcome::Exit(code) | EvalOutcome::Return(code) => *code,
            EvalOutcome::Break(_) | EvalOutcome::Continue(_) => 0,
        }
    }
}

pub fn eval_string(
    command_string: &BStr,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let mut builder = ASTBuilder::new();
    let tokens = tokenize(command_string).map_err(|err| err.to_string())?;
    let cmds = parse_script(&mut builder, &tokens).map_err(|err| err.to_string())?;
    let cmd_ptr = builder.add_sequence_or_single(&cmds);
    eval_command(&mut builder, cmd_ptr, state, ctx)
}

pub fn eval_command(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    if state.opt_xtrace {
        if let Some(ref mut stderr) = ctx.stderr() {
            let cmd = builder.get_ref(cmd_ptr);
            if cmd.tag.is_traceable() {
                let formatted = command_to_bstring(cmd, builder);
                let ps4_prefix =
                    expand::expand_prompt(BStr::new(b"PS4"), BStr::new(b"+ "), state, ctx);
                let _ = writeln!(stderr, "{}{}", ps4_prefix, formatted);
            }
        }
    }

    if state.opt_noexec && !state.opt_interactive {
        return Ok(EvalOutcome::Code(0));
    }

    if let Some(outcome) = run_pending_traps(state, ctx)? {
        return Ok(outcome);
    }
    let outcome = eval_command_inner(builder, cmd_ptr, state, ctx);
    if let Ok(EvalOutcome::Code(exit_code)) = &outcome {
        state.set_last_status(*exit_code);
    }
    outcome
}

fn eval_command_inner(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    if let Some(code) = ctx.signal_state.pending_exit_code() {
        return Ok(EvalOutcome::Code(code));
    }

    let tag = builder.get_ref(cmd_ptr).tag;
    match tag {
        CommandTag::SIMPLE => eval_simple(builder, cmd_ptr, state, ctx),
        CommandTag::PIPELINE => eval_pipeline(builder, cmd_ptr, state, ctx),
        CommandTag::REDIRECT => eval_redirect(builder, cmd_ptr, state, ctx),
        CommandTag::IF => eval_if(builder, cmd_ptr, state, ctx),
        CommandTag::WHILE => eval_loop(builder, cmd_ptr, state, ctx, true),
        CommandTag::UNTIL => eval_loop(builder, cmd_ptr, state, ctx, false),
        CommandTag::FOR => eval_for(builder, cmd_ptr, state, ctx),
        CommandTag::CASE => eval_case(builder, cmd_ptr, state, ctx),
        CommandTag::LOGICAL_AND => eval_logical_and(builder, cmd_ptr, state, ctx),
        CommandTag::LOGICAL_OR => eval_logical_or(builder, cmd_ptr, state, ctx),
        CommandTag::BACKGROUND => eval_background(builder, cmd_ptr, state, ctx),
        CommandTag::SEQUENCE => eval_sequence(builder, cmd_ptr, state, ctx),
        CommandTag::SUBSHELL => {
            let sub_cmd = {
                let cmd = builder.get_ref(cmd_ptr);
                cmd.left.as_ref(builder)
            };
            let proc = spawn_subshell_vmo(
                sub_cmd,
                state,
                ctx,
                None,
                None,
                None,
                SubshellScriptArgs::Pass,
                builder,
            )?;
            let exit_code = wait_for_process_to_exit(&proc, ctx)?;
            Ok(state.handle_outcome(EvalOutcome::Code(exit_code)))
        }
        CommandTag::FUNCTION_DEF => {
            let (name, body_bytes) = {
                let cmd = builder.get_ref(cmd_ptr);
                let name = cmd.name.to_bstring(builder);
                let body_bytes = cmd.then_branch.as_ref(builder).serialize(builder);
                (name, body_bytes)
            };
            state.add_function(name, body_bytes);
            Ok(EvalOutcome::Code(0))
        }
        _ => unreachable!(),
    }
}
