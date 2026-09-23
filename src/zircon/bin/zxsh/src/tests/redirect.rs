// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::testing::{HEREDOC_INLINE_THRESHOLD, apply_redirects};
use crate::eval::{EvalOutcome, ExecutionContext, ShellState, eval_redirect};
use crate::fd::Fd;
use crate::parser::ast::{ASTBuilder, Redirect, RedirectTag, RedirectTemplate, ResolvedWordPart};
use crate::relative;
use bstr::{BStr, BString};

#[test]
fn test_redirect_to_dev_null() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let parts = vec![ResolvedWordPart::Literal(BString::from("/dev/null"))];
    let w_slice = builder.add_resolved_word(&parts);

    let redirect = Redirect {
        tag: RedirectTag::TO_FILE,
        src_fd: Fd(1),
        dest_fd: Fd(0),
        filename: w_slice,
        append: 0,
        clobber: 1,
        expand: 0,
        body: relative::BStr::empty(),
    };

    apply_redirects(&[redirect], &mut state, &mut ctx, &builder).unwrap();
    assert!(ctx.stdout().is_some());
}

#[test]
fn test_redirect_from_dev_null() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let parts = vec![ResolvedWordPart::Literal(BString::from("/dev/null"))];
    let w_slice = builder.add_resolved_word(&parts);

    let redirect = Redirect {
        tag: RedirectTag::FROM_FILE,
        src_fd: Fd(0),
        dest_fd: Fd(0),
        filename: w_slice,
        append: 0,
        clobber: 0,
        expand: 0,
        body: relative::BStr::empty(),
    };

    apply_redirects(&[redirect], &mut state, &mut ctx, &builder).unwrap();
    assert!(ctx.stdin().is_some());
}

#[test]
fn test_redirect_dup_and_close_fd() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let dup_redirect = Redirect {
        tag: RedirectTag::DUP_FD,
        src_fd: Fd(2),
        dest_fd: Fd(1),
        filename: relative::Slice::empty(),
        append: 0,
        clobber: 0,
        expand: 0,
        body: relative::BStr::empty(),
    };

    let close_redirect = Redirect {
        tag: RedirectTag::CLOSE_FD,
        src_fd: Fd(1),
        dest_fd: Fd(0),
        filename: relative::Slice::empty(),
        append: 0,
        clobber: 0,
        expand: 0,
        body: relative::BStr::empty(),
    };

    let builder = ASTBuilder::new();
    apply_redirects(&[dup_redirect, close_redirect], &mut state, &mut ctx, &builder).unwrap();
    assert!(ctx.stdout().is_none());
    assert!(ctx.stderr().is_some());
}

#[test]
fn test_redirect_heredoc_unexpanded_and_expanded() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.set_var(BStr::new("USER"), BStr::new("fuchsia"));

    let mut builder = ASTBuilder::new();
    let body_expanded = builder.add_bstr(b"Hello $USER");
    let body_unexpanded = builder.add_bstr(b"Hello $USER plain");

    let heredoc_exp = Redirect {
        tag: RedirectTag::HERE_DOC,
        src_fd: Fd(0),
        dest_fd: Fd(0),
        filename: relative::Slice::empty(),
        append: 0,
        clobber: 0,
        expand: 1,
        body: body_expanded,
    };

    let heredoc_unexp = Redirect {
        tag: RedirectTag::HERE_DOC,
        src_fd: Fd(0),
        dest_fd: Fd(0),
        filename: relative::Slice::empty(),
        append: 0,
        clobber: 0,
        expand: 0,
        body: body_unexpanded,
    };

    apply_redirects(&[heredoc_exp, heredoc_unexp], &mut state, &mut ctx, &builder).unwrap();
    assert!(ctx.stdin().is_some());
}

#[test]
fn test_redirect_heredoc_short_inline() {
    use std::io::Read;

    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let body_bstr = builder.add_bstr(b"Short heredoc content inline");

    let heredoc = Redirect {
        tag: RedirectTag::HERE_DOC,
        src_fd: Fd(0),
        dest_fd: Fd(0),
        filename: relative::Slice::empty(),
        append: 0,
        clobber: 0,
        expand: 0,
        body: body_bstr,
    };

    apply_redirects(&[heredoc], &mut state, &mut ctx, &builder).unwrap();
    let mut stdin_file = ctx.stdin().unwrap();
    let mut content = Vec::new();
    stdin_file.read_to_end(&mut content).unwrap();
    assert_eq!(content, b"Short heredoc content inline");
}

#[test]
fn test_redirect_heredoc_max_inline() {
    use std::io::Read;

    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let max_inline_body = vec![b'a'; HEREDOC_INLINE_THRESHOLD];
    let body_bstr = builder.add_bstr(&max_inline_body);

    let heredoc = Redirect {
        tag: RedirectTag::HERE_DOC,
        src_fd: Fd(0),
        dest_fd: Fd(0),
        filename: relative::Slice::empty(),
        append: 0,
        clobber: 0,
        expand: 0,
        body: body_bstr,
    };

    apply_redirects(&[heredoc], &mut state, &mut ctx, &builder).unwrap();
    let mut stdin_file = ctx.stdin().unwrap();
    let mut content = Vec::new();
    stdin_file.read_to_end(&mut content).unwrap();
    assert_eq!(content, max_inline_body);
}

#[test]
fn test_redirect_heredoc_large_threaded() {
    use std::io::Read;

    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let large_body = vec![b'x'; HEREDOC_INLINE_THRESHOLD + 1024];
    let body_bstr = builder.add_bstr(&large_body);

    let heredoc = Redirect {
        tag: RedirectTag::HERE_DOC,
        src_fd: Fd(0),
        dest_fd: Fd(0),
        filename: relative::Slice::empty(),
        append: 0,
        clobber: 0,
        expand: 0,
        body: body_bstr,
    };

    apply_redirects(&[heredoc], &mut state, &mut ctx, &builder).unwrap();
    let mut stdin_file = ctx.stdin().unwrap();
    let mut content = Vec::new();
    stdin_file.read_to_end(&mut content).unwrap();
    assert_eq!(content, large_body);
}

#[test]
fn test_redirect_ambiguous_filename_error() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let parts = vec![ResolvedWordPart::Var(BString::from("UNSET_VAR"))];
    let w_slice = builder.add_resolved_word(&parts);

    let redirect = Redirect {
        tag: RedirectTag::TO_FILE,
        src_fd: Fd(1),
        dest_fd: Fd(0),
        filename: w_slice,
        append: 0,
        clobber: 1,
        expand: 0,
        body: relative::BStr::empty(),
    };

    let res = apply_redirects(&[redirect], &mut state, &mut ctx, &builder);
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("ambiguous redirect"));
}

#[test]
fn test_redirect_from_file_nonexistent_error() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let parts = vec![ResolvedWordPart::Literal(BString::from("/nonexistent_file_xyz_123"))];
    let w_slice = builder.add_resolved_word(&parts);

    let redirect = Redirect {
        tag: RedirectTag::FROM_FILE,
        src_fd: Fd(0),
        dest_fd: Fd(0),
        filename: w_slice,
        append: 0,
        clobber: 0,
        expand: 0,
        body: relative::BStr::empty(),
    };

    let res = apply_redirects(&[redirect], &mut state, &mut ctx, &builder);
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("Failed to open"));
}

#[test]
fn test_eval_redirect_wrapper() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

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

    let res = eval_redirect(&mut builder, redirect_cmd, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}
