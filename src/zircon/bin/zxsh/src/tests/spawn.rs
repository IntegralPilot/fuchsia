// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::testing::spawn_command_with_redirection;
use crate::eval::{ExecutionContext, ShellState, eval_pipeline};
use crate::fd::Fd;
use crate::parser::ast::{ASTBuilder, CommandTag, RedirectTag, RedirectTemplate, ResolvedWordPart};
use bstr::BString;

#[test]
fn test_spawn_command_empty_args_error() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let cmd_ptr = builder.add_simple_command(&[]);

    let res =
        spawn_command_with_redirection(&mut builder, cmd_ptr, &mut state, &ctx, None, None, None);
    assert!(res.is_err());
    assert_eq!(res.unwrap_err(), "No command specified");
}

#[test]
fn test_spawn_command_with_redirect_nesting() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let simple_cmd = builder.add_simple_command(&[]);

    let dev_null =
        builder.add_resolved_word(&[ResolvedWordPart::Literal(BString::from("/dev/null"))]);
    let template = RedirectTemplate {
        tag: RedirectTag::TO_FILE,
        append: 0,
        clobber: 1,
        expand: 0,
        src_fd: Fd(1),
        dest_fd: Fd(0),
        filename: Some(dev_null),
        body: None,
    };
    let redirects_slice = builder.add_redirects_from_templates(&[template]);
    let redirect_cmd = builder.add_redirect_command(simple_cmd, redirects_slice);

    let res = spawn_command_with_redirection(
        &mut builder,
        redirect_cmd,
        &mut state,
        &ctx,
        None,
        None,
        None,
    );
    assert!(res.is_err());
    assert_eq!(res.unwrap_err(), "No command specified");
}

#[test]
fn test_eval_pipeline_simple() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let cmd1 = builder.add_simple_command(&[]);
    let cmd2 = builder.add_simple_command(&[]);
    let (pipe_cmd_mut, pipe_cmd_ptr) = builder.add_command_uninit(CommandTag::PIPELINE);
    pipe_cmd_mut.left = cmd1;
    pipe_cmd_mut.right = cmd2;

    let res = eval_pipeline(&mut builder, pipe_cmd_ptr, &mut state, &mut ctx);
    assert!(res.is_err());
}
