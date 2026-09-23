// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use std::thread;
use zx::Status;

#[fuchsia::test]
fn test_set_role_for_this_thread() -> Result<()> {
    // Test that setting a nonexistent role for this thread fails.
    let result = fuchsia_scheduler::set_role_for_this_thread("test.nonexistent.role");
    assert_eq!(result.is_err(), true);
    let root_cause = result.unwrap_err().downcast::<Status>()?;
    assert_eq!(root_cause, Status::NOT_FOUND);

    // Test that setting a valid role for this thread succeeds.
    fuchsia_scheduler::set_role_for_this_thread("test.core.a")
}

#[fuchsia::test]
fn test_set_role_for_thread() -> Result<()> {
    // Spawn a thread to set roles on.
    let (handle_send, handle_recv) = ::std::sync::mpsc::channel();
    let (exit_send, exit_recv) = ::std::sync::mpsc::channel();
    let child_thread = thread::spawn(move || {
        fuchsia_runtime::with_thread_self(|thread| {
            handle_send.send(thread.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
        })
        .expect("failed to send this thread's handle");
        exit_recv.recv().expect("failed to receive exit signal");
    });
    let thread = handle_recv.recv()?;

    // Test that setting a nonexistent role for that thread fails.
    let result = fuchsia_scheduler::set_role_for_thread(&thread, "test.nonexistent.role");
    assert_eq!(result.is_err(), true);
    let root_cause = result.unwrap_err().downcast::<Status>()?;
    assert_eq!(root_cause, Status::NOT_FOUND);

    // Test that setting a valid role for that thread succeeds.
    assert_eq!(fuchsia_scheduler::set_role_for_thread(&thread, "test.core.a").is_ok(), true);

    // Tell the child thread to terminate.
    exit_send.send("exit".to_owned()).expect("failed to send exit signal");
    child_thread.join().expect("failed to join child thread");

    Ok(())
}

#[fuchsia::test]
fn test_set_role_for_root_vmar() {
    fuchsia_scheduler::set_role_for_root_vmar("test.core.a.memory").unwrap();
    assert_eq!(
        fuchsia_scheduler::set_role_for_root_vmar("test.nonexistent.role")
            .unwrap_err()
            .downcast::<Status>()
            .unwrap(),
        Status::NOT_FOUND
    );
}

#[fuchsia::test]
fn test_set_role_for_nested_vmar() {
    let (nested, _base_addr) =
        fuchsia_runtime::vmar_root_self().allocate(0, 4096, zx::VmarFlags::CAN_MAP_READ).unwrap();

    fuchsia_scheduler::set_role_for_vmar(&nested, "test.core.a.memory").unwrap();
    assert_eq!(
        fuchsia_scheduler::set_role_for_vmar(&nested, "test.nonexistent.role")
            .unwrap_err()
            .downcast::<Status>()
            .unwrap(),
        Status::NOT_FOUND
    );
}
