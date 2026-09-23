// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Helpers for writing tests using drivers and dispatchers

use std::sync::{Arc, mpsc};

use fdf_core::dispatcher::CurrentDispatcher;
use libasync_dispatcher::{AsAsyncDispatcherRef, OnDispatcher};

use super::*;

/// Runs the given closure under the a new root driver dispatcher. Use [`fdf::CurrentDispatcher`]
/// to reference the dispatcher if needed.
pub fn run_in_driver<T: Send + 'static>(name: &str, p: impl FnOnce() -> T + Send + 'static) -> T {
    run_in_driver_raw(name, true, false, |tx| tx.send(p()).unwrap())
}

/// Runs the given closure under a new root driver dispatcher, with some additional options.
/// Use [`fdf::CurrentDispatcher`] to reference the dispatcher if needed.
pub fn run_in_driver_etc<T: Send + 'static>(
    name: &str,
    allow_thread_blocking: bool,
    unsynchronized: bool,
    p: impl FnOnce() -> T + Send + 'static,
) -> T {
    run_in_driver_raw(name, allow_thread_blocking, unsynchronized, |tx| tx.send(p()).unwrap())
}

/// Runs the given future to completion under the a new root driver dispatcher. Use
/// [`fdf::CurrentDispatcher`] to reference the dispatcher if needed.
pub fn spawn_in_driver<T: Send + 'static>(
    name: &str,
    p: impl Future<Output = T> + Send + 'static,
) -> T {
    run_in_driver_raw(name, true, false, |tx| {
        CurrentDispatcher.spawn(async move { tx.send(p.await).unwrap() });
    })
}

/// Runs the given future to completion under a new root driver dispatcher, with some additional
/// options. Use [`fdf::CurrentDispatcher`] to reference the dispatcher if needed.
pub fn spawn_in_driver_etc<T: Send + 'static>(
    name: &str,
    allow_thread_blocking: bool,
    unsynchronized: bool,
    p: impl Future<Output = T> + Send + 'static,
) -> T {
    run_in_driver_raw(name, allow_thread_blocking, unsynchronized, |tx| {
        CurrentDispatcher.spawn(async move { tx.send(p.await).unwrap() });
    })
}

fn run_in_driver_raw<T: Send + 'static>(
    name: &str,
    allow_thread_blocking: bool,
    unsynchronized: bool,
    p: impl FnOnce(mpsc::Sender<T>) + Send + 'static,
) -> T {
    let env = Arc::new(Environment::start(0).unwrap());
    let env_clone = env.clone();

    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let driver_value: u32 = 0x1337;
    let driver_value_ptr = &driver_value as *const u32;
    let driver = env.new_driver(driver_value_ptr);
    let dispatcher = DispatcherBuilder::new().name(name);
    let dispatcher =
        if allow_thread_blocking { dispatcher.allow_thread_blocking() } else { dispatcher };
    let dispatcher = if unsynchronized { dispatcher.unsynchronized() } else { dispatcher };
    let dispatcher = dispatcher.shutdown_observer(move |dispatcher| {
        // We verify that the dispatcher has no tasks left queued in it,
        // just because this is testing code.
        assert!(!env_clone.dispatcher_has_queued_tasks(dispatcher.as_dispatcher_ref()));
    });
    let dispatcher = driver.new_dispatcher(dispatcher).unwrap().release();

    let (finished_tx, finished_rx) = mpsc::channel();
    dispatcher
        .post_task_sync(move |_| {
            p(finished_tx);
        })
        .unwrap();
    let res = finished_rx.recv().unwrap();

    driver.shutdown(move |driver| {
        // SAFETY: driver lives on the stack, it's safe to dereference it.
        assert!(unsafe { *driver.0 } == 0x1337);
        shutdown_tx.send(()).unwrap();
    });

    shutdown_rx.recv().unwrap();

    res
}
