// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::os::unix::fs::OpenOptionsExt;

use super::execution_context::ExecutionContext;
use super::expand::{expand_argument, expand_string, get_literal_command_name};
use super::simple::parse_simple_command_args;
use super::state::ShellState;
use super::{EvalOutcome, eval_command};
use crate::errors::io_err_str;
use crate::parser::ast::{ASTBuilder, Command, CommandTag, Redirect, RedirectTag, WordPart};
use crate::process::make_pipe;
use crate::relative;
use bstr::{BStr, BString, ByteSlice};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRedirectionMode {
    Truncate,
    Append,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClobberOption {
    AllowClobber,
    PreventClobber,
}

enum ExpandedPath {
    NullFd(std::fs::File),
    Path(std::path::PathBuf, BString),
}

/// Creates a null file descriptor on Fuchsia, which lacks a native `/dev/null` special file.
fn open_null_fd() -> Result<std::fs::File, String> {
    let null_fd = fdio::create_fd_null().ok_or_else(|| "Failed to create null fd".to_string())?;
    Ok(std::fs::File::from(null_fd))
}

fn expand_redirection_path(
    filename: &[WordPart],
    state: &mut ShellState,
    ctx: &ExecutionContext,
    buf: &relative::Buffer,
) -> Result<ExpandedPath, String> {
    let expanded = expand_argument(filename, state, ctx, buf)?;
    if expanded.len() != 1 {
        return Err(format!("ambiguous redirect: {:?}", filename));
    }
    let expanded_filename = &expanded[0];
    if expanded_filename == "/dev/null" {
        return Ok(ExpandedPath::NullFd(open_null_fd()?));
    }
    let path = expanded_filename
        .to_path()
        .map_err(|e| format!("Invalid path {}: {}", expanded_filename, e))?
        .to_path_buf();
    Ok(ExpandedPath::Path(path, expanded_filename.clone()))
}

fn is_exec_command(builder: &ASTBuilder, mut cmd_ptr: relative::Ptr<Command>) -> bool {
    loop {
        let cmd = builder.get_ref(cmd_ptr);
        match cmd.tag {
            CommandTag::REDIRECT => {
                cmd_ptr = cmd.left;
            }
            CommandTag::SIMPLE => {
                let (_, cmd_args_refs) = parse_simple_command_args(builder, cmd_ptr);
                if cmd_args_refs.is_empty() {
                    return false;
                }
                let arg0 = builder.get_slice(cmd_args_refs[0]);
                return get_literal_command_name(arg0, builder).as_deref().map(|v| v.as_slice())
                    == Some(b"exec");
            }
            _ => return false,
        }
    }
}

pub fn eval_redirect(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let (sub_cmd_ptr, redirects) = {
        let cmd = builder.get_ref(cmd_ptr);
        (cmd.left, cmd.redirects.as_slice(builder))
    };

    let is_exec = is_exec_command(builder, sub_cmd_ptr);
    let mut new_context = ctx.try_clone()?;
    apply_redirects(redirects, state, &mut new_context, builder)?;
    let outcome = eval_command(builder, sub_cmd_ptr, state, &mut new_context)?;
    if is_exec && matches!(outcome, EvalOutcome::Code(0)) {
        *ctx = new_context;
    }
    Ok(outcome)
}

/// The maximum size of a write to a pipe that is guaranteed to be atomic and not block.
/// Heredocs smaller than or equal to this capacity can be written inline into the pipe
/// without spawning a background worker thread.
pub const HEREDOC_INLINE_THRESHOLD: usize = libc::PIPE_BUF;

pub fn apply_redirects(
    redirects: &[Redirect],
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
    buf: &relative::Buffer,
) -> Result<(), String> {
    for redirect in redirects {
        match redirect.tag {
            RedirectTag::TO_FILE => {
                let word_parts = redirect.filename.as_slice(buf);
                let mode = if redirect.append != 0 {
                    FileRedirectionMode::Append
                } else {
                    FileRedirectionMode::Truncate
                };
                let clobber = if redirect.clobber != 0 {
                    ClobberOption::AllowClobber
                } else {
                    ClobberOption::PreventClobber
                };
                let file = setup_to_file_redirection(word_parts, mode, clobber, state, ctx, buf)?;
                ctx.set_fd(redirect.src_fd, file);
            }
            RedirectTag::FROM_FILE => {
                let word_parts = redirect.filename.as_slice(buf);
                let file = setup_from_file_redirection(word_parts, state, ctx, buf)?;
                ctx.set_fd(redirect.src_fd, file);
            }
            RedirectTag::DUP_FD => {
                let dup = ctx.dup_fd(redirect.dest_fd)?;
                ctx.set_fd(redirect.src_fd, dup);
            }
            RedirectTag::CLOSE_FD => {
                ctx.close_fd(redirect.src_fd);
            }
            RedirectTag::HERE_DOC => {
                let body_bytes = redirect.body.as_slice(buf);
                let final_body = if redirect.expand != 0 {
                    expand_string(BStr::new(body_bytes), state, ctx)?
                } else {
                    BString::from(body_bytes)
                };
                let (read_fd, mut write_fd) = make_pipe()?;

                use std::io::Write;
                if final_body.len() <= HEREDOC_INLINE_THRESHOLD {
                    write_fd.write_all(final_body.as_bytes()).map_err(|e| {
                        format!("Failed to write heredoc to pipe: {}", io_err_str(e))
                    })?;
                } else {
                    // Spawning a worker thread to populate the heredoc pipe avoids deadlocks if the
                    // heredoc content exceeds the OS kernel pipe buffer capacity before the reader
                    // consumes it.  The thread terminates and drops write_fd as soon as the write
                    // completes.
                    std::thread::spawn(move || {
                        let _ = write_fd.write_all(final_body.as_bytes());
                    });
                }

                ctx.set_fd(redirect.src_fd, read_fd);
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn setup_to_file_redirection(
    filename: &[WordPart],
    mode: FileRedirectionMode,
    clobber: ClobberOption,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    buf: &relative::Buffer,
) -> Result<std::fs::File, String> {
    let (target_path, expanded_filename) = match expand_redirection_path(filename, state, ctx, buf)?
    {
        ExpandedPath::NullFd(file) => return Ok(file),
        ExpandedPath::Path(path, name) => (path, name),
    };

    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    options.mode(0o666 & !state.umask());

    match mode {
        FileRedirectionMode::Append => {
            options.create(true).append(true);
        }
        FileRedirectionMode::Truncate => {
            if state.opt_noclobber && clobber == ClobberOption::PreventClobber {
                options.create_new(true);
            } else {
                options.create(true).truncate(true);
            }
        }
    }

    options.open(&target_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            format!("{}: File exists (noclobber)", expanded_filename)
        } else {
            format!("Failed to open {}: {}", expanded_filename, io_err_str(e))
        }
    })
}

fn setup_from_file_redirection(
    filename: &[WordPart],
    state: &mut ShellState,
    ctx: &ExecutionContext,
    buf: &relative::Buffer,
) -> Result<std::fs::File, String> {
    let (target_path, expanded_filename) = match expand_redirection_path(filename, state, ctx, buf)?
    {
        ExpandedPath::NullFd(file) => return Ok(file),
        ExpandedPath::Path(path, name) => (path, name),
    };

    std::fs::File::open(&target_path)
        .map_err(|e| format!("Failed to open {}: {}", expanded_filename, io_err_str(e)))
}
