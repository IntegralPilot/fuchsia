// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::File;

use bstr::ByteSlice;

use super::EvalOutcome;
use super::execution_context::ExecutionContext;
use super::expand::{
    ExpandedCommand, expand_alias, expand_argument, expand_assignment_value, needs_subshell_process,
};
use super::redirect::apply_redirects;
use super::simple::{
    ResolvedAlias, apply_assignments, expand_command_and_env, parse_simple_command_args,
    resolve_alias_loop,
};
use super::state::ShellState;
use crate::builtins::is_builtin;
use crate::errors::{io_err_str, zx_status_str};
use crate::fd::Fd;
use crate::parser::ast::{ASTBuilder, Command, CommandTag};
use crate::process::{clone_fd_to_action, make_pipe, spawn_command};
use crate::relative;
use crate::subshell::{SubshellScriptArgs, spawn_subshell_process};
use crate::tty::{ShellSignals, wait_for_process_with_interrupt};
use bstr::BString;

pub fn spawn_command_with_redirection(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    default_in: Option<&File>,
    default_out: Option<&File>,
    default_err: Option<&File>,
) -> Result<zx::Process, String> {
    let mut stage_context = ctx.try_clone()?;
    if let Some(file) = default_in {
        stage_context.set_fd(Fd::STDIN, file.try_clone().map_err(io_err_str)?);
    } else {
        stage_context.close_fd(Fd::STDIN);
    }
    if let Some(file) = default_out {
        stage_context.set_fd(Fd::STDOUT, file.try_clone().map_err(io_err_str)?);
    } else {
        stage_context.close_fd(Fd::STDOUT);
    }
    if let Some(file) = default_err {
        stage_context.set_fd(Fd::STDERR, file.try_clone().map_err(io_err_str)?);
    } else {
        stage_context.close_fd(Fd::STDERR);
    }

    let tag = builder.get_ref(cmd_ptr).tag;
    match tag {
        CommandTag::REDIRECT => {
            let (sub_cmd_ptr, redirects) = {
                let cmd = builder.get_ref(cmd_ptr);
                (cmd.left, cmd.redirects.as_slice(builder))
            };

            apply_redirects(redirects, state, &mut stage_context, builder)?;
            spawn_command_with_redirection(
                builder,
                sub_cmd_ptr,
                state,
                &stage_context,
                stage_context.stdin(),
                stage_context.stdout(),
                stage_context.stderr(),
            )
        }
        CommandTag::SIMPLE => {
            let (assignments_refs, cmd_args_refs) = parse_simple_command_args(builder, cmd_ptr);

            if cmd_args_refs.is_empty() {
                return Err("No command specified".to_string());
            }

            let cmd_args =
                match resolve_alias_loop(builder, cmd_args_refs, state, &mut stage_context)? {
                    ResolvedAlias::Command(new_cmd_ptr) => {
                        let env_guard =
                            apply_assignments(builder, &assignments_refs, state, &stage_context)?;
                        return spawn_command_with_redirection(
                            builder,
                            new_cmd_ptr,
                            env_guard.state,
                            &stage_context,
                            default_in,
                            default_out,
                            default_err,
                        );
                    }
                    ResolvedAlias::Words(words) => words,
                };

            let (env_guard, mut expanded_args) = expand_command_and_env(
                builder,
                &assignments_refs,
                &cmd_args,
                state,
                &stage_context,
            )?;

            if expanded_args.is_empty() {
                return Err("No command specified".to_string());
            }

            let name = &expanded_args[0];
            if is_builtin(name.as_bstr()) {
                return Err(format!(
                    "Internal error: builtin {} reached spawn_command_with_redirection",
                    name
                ));
            }

            if let Some(resolved_path) =
                env_guard.state.resolve_command_path(expanded_args[0].as_ref())
            {
                expanded_args[0] = resolved_path;
            }

            let mut actions = get_spawn_actions(&stage_context, None, None, None);

            let vars = env_guard.state.vars();
            drop(env_guard);

            spawn_command(&expanded_args, &vars, &mut actions).map_err(|status| {
                format!("Failed to spawn {}: {}", expanded_args[0], zx_status_str(status))
            })
        }
        _ => {
            let cmd = builder.get_ref(cmd_ptr);
            spawn_subshell_vmo(
                cmd,
                state,
                &stage_context,
                None,
                None,
                None,
                SubshellScriptArgs::DoNotPass,
                builder,
            )
        }
    }
}

pub fn spawn_pipeline_stage(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    stdin: Option<&File>,
    stdout: Option<&File>,
    stderr: Option<&File>,
) -> Result<zx::Process, String> {
    let is_subshell = {
        let cmd = builder.get_ref(cmd_ptr);
        needs_subshell_process(cmd, state, builder)
    };
    if is_subshell {
        let cmd = builder.get_ref(cmd_ptr);
        spawn_subshell_vmo(
            cmd,
            state,
            ctx,
            stdin,
            stdout,
            stderr,
            SubshellScriptArgs::DoNotPass,
            builder,
        )
    } else {
        spawn_command_with_redirection(builder, cmd_ptr, state, ctx, stdin, stdout, stderr)
    }
}

pub fn spawn_subshell_vmo(
    cmd: &Command,
    state: &ShellState,
    ctx: &ExecutionContext,
    stdin_override: Option<&File>,
    stdout_override: Option<&File>,
    stderr_override: Option<&File>,
    script_args: SubshellScriptArgs,
    source_buf: &relative::Buffer,
) -> Result<zx::Process, String> {
    let mut actions = get_spawn_actions(ctx, stdin_override, stdout_override, stderr_override);
    spawn_subshell_process(cmd, state, &mut actions, script_args, source_buf)
}

fn get_spawn_actions(
    ctx: &ExecutionContext,
    stdin_override: Option<&File>,
    stdout_override: Option<&File>,
    stderr_override: Option<&File>,
) -> Vec<fdio::SpawnAction<'static>> {
    let stdin = stdin_override.or_else(|| ctx.stdin());
    let stdout = stdout_override.or_else(|| ctx.stdout());
    let stderr = stderr_override.or_else(|| ctx.stderr());

    let mut actions = Vec::new();
    for (fd_opt, target) in [(stdin, Fd::STDIN), (stdout, Fd::STDOUT), (stderr, Fd::STDERR)] {
        if let Some(fd) = fd_opt {
            if let Some(action) = clone_fd_to_action(fd, target) {
                actions.push(action);
            }
        }
    }

    actions
}

pub fn wait_for_process_to_exit(
    proc: &zx::Process,
    ctx: &mut ExecutionContext,
) -> Result<i32, String> {
    let pty = ctx.pty_control();
    let wait_res = wait_for_process_with_interrupt(proc, pty.as_deref(), &mut ctx.signal_state);

    wait_res.map_err(|err| format!("Wait failed: {}", zx_status_str(err)))?;

    let exit_code = if let Some(code) = ctx.signal_state.pending_exit_code() {
        ctx.signal_state.take_pending();
        code
    } else {
        proc.info()
            .map_err(|err| format!("Failed to get process info: {}", zx_status_str(err)))?
            .return_code as i32
    };
    Ok(exit_code)
}

pub fn eval_pipeline(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let mut stages = Vec::new();
    let mut current_ptr = cmd_ptr;
    loop {
        let tag = builder.get_ref(current_ptr).tag;
        if tag == CommandTag::PIPELINE {
            let (left, right) = {
                let cmd = builder.get_ref(current_ptr);
                (cmd.left, cmd.right)
            };
            stages.push(left);
            current_ptr = right;
        } else {
            stages.push(current_ptr);
            break;
        }
    }

    let mut pipes = Vec::new();
    for _ in 0..stages.len() - 1 {
        pipes.push(make_pipe()?);
    }

    let mut processes = Vec::new();
    for (i, stage_ptr) in stages.iter().enumerate() {
        let stdin = if i == 0 { ctx.stdin() } else { Some(&pipes[i - 1].0) };
        let stdout = if i == stages.len() - 1 { ctx.stdout() } else { Some(&pipes[i].1) };
        let stderr = ctx.stderr();
        processes
            .push(spawn_pipeline_stage(builder, *stage_ptr, state, ctx, stdin, stdout, stderr)?);
    }

    // Close pipe fds in parent
    drop(pipes);

    let mut last_code = 0;
    for proc in processes {
        let code = wait_for_process_to_exit(&proc, ctx)?;
        last_code = code;
    }

    Ok(state.handle_outcome(EvalOutcome::Code(last_code)))
}
