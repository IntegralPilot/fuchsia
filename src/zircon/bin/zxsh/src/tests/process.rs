// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::ShellEnv;
use crate::fd::Fd;
use crate::process::{clone_fd_to_action, make_pipe, read_fd_to_end, spawn_command};
use bstr::BString;
use std::io::Write;
use std::os::fd::{AsRawFd, BorrowedFd};

#[test]
fn test_pipe_and_read() {
    let (read_fd, mut write_fd) = make_pipe().expect("make_pipe failed");
    write_fd.write_all(b"hello pipe").expect("write failed");
    drop(write_fd);

    let bytes = read_fd_to_end(read_fd).expect("read_fd_to_end failed");
    assert_eq!(bytes, b"hello pipe");
}

#[test]
fn test_clone_fd_to_action() {
    let (read_fd, _write_fd) = make_pipe().expect("make_pipe failed");
    let raw_fd = read_fd.as_raw_fd();
    let action = clone_fd_to_action(&read_fd, Fd::STDIN);
    assert!(action.is_some());

    // Close the file descriptor so it becomes invalid for duplication
    drop(read_fd);
    drop(_write_fd);

    let invalid_fd = unsafe { BorrowedFd::borrow_raw(raw_fd) };
    assert!(clone_fd_to_action(&invalid_fd, Fd::STDIN).is_none());
}

#[test]
fn test_spawn_command_errors() {
    let mut actions = Vec::new();
    let res = spawn_command(&[], &ShellEnv::default(), &mut actions);
    assert_eq!(res.unwrap_err(), zx::Status::INVALID_ARGS);

    let null_arg = vec![BString::from("foo\0bar")];
    let res = spawn_command(&null_arg, &ShellEnv::default(), &mut actions);
    assert_eq!(res.unwrap_err(), zx::Status::INVALID_ARGS);

    let valid_arg = vec![BString::from("ls")];
    let null_env = ShellEnv::from(vec![(BString::from("VAR\0NAME"), BString::from("val"))]);
    let res = spawn_command(&valid_arg, &null_env, &mut actions);
    assert_eq!(res.unwrap_err(), zx::Status::INVALID_ARGS);

    let nonexistent = vec![BString::from("/definitely/nonexistent/binary")];
    let valid_env = ShellEnv::from(vec![(BString::from("VALID_VAR"), BString::from("valid_val"))]);
    let res = spawn_command(&nonexistent, &valid_env, &mut actions);
    assert!(res.is_err());
}

#[test]
fn test_spawn_command_exceeds_max_size() {
    let mut actions = Vec::new();
    let binary = BString::from("/pkg/bin/sh");
    let huge_val = BString::from("x".repeat(100_000));
    let huge_env = ShellEnv::from(vec![(BString::from("HUGE_VAR"), huge_val.clone())]);
    let argv = vec![binary.clone(), huge_val];

    let res = spawn_command(&argv, &huge_env, &mut actions);
    assert!(res.is_err(), "spawning with args/env exceeding max size should fail");
}
