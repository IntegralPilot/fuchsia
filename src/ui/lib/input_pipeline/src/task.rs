// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::future::LocalBoxFuture;
use futures::prelude::*;

/// A helper structure that manages the execution of all asynchronous tasks
/// in the input pipeline concurrently.
pub struct InputPipelineTasks {
    pub watcher: LocalBoxFuture<'static, ()>,
    pub runner: LocalBoxFuture<'static, ()>,
    pub display_ownership: Option<LocalBoxFuture<'static, ()>>,
    pub focus_listener: Option<LocalBoxFuture<'static, ()>>,
    pub forwarder: LocalBoxFuture<'static, ()>,
}

impl InputPipelineTasks {
    /// Runs all tasks concurrently. Returns when any of the critical tasks completes.
    pub async fn run(self) {
        let InputPipelineTasks { watcher, runner, display_ownership, focus_listener, forwarder } =
            self;

        let mut watcher = watcher.fuse();
        let mut runner = runner.fuse();
        let mut display_ownership =
            display_ownership.unwrap_or_else(|| Box::pin(futures::future::pending())).fuse();
        let mut focus_listener =
            focus_listener.unwrap_or_else(|| Box::pin(futures::future::pending())).fuse();
        let mut forwarder = forwarder.fuse();

        loop {
            futures::select! {
                _ = watcher => {
                    // Watcher finished (e.g. break_on_idle in tests).
                    // This is non-fatal; continue running remaining tasks.
                },
                _ = runner => break,
                _ = display_ownership => break,
                _ = focus_listener => break,
                _ = forwarder => break,
                complete => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::oneshot;

    #[fuchsia::test]
    async fn run_does_not_exit_on_watcher_completion() {
        let (forwarder_tx, forwarder_rx) = oneshot::channel::<()>();
        let tasks = InputPipelineTasks {
            watcher: Box::pin(async {}),
            runner: Box::pin(futures::future::pending()),
            display_ownership: None,
            focus_listener: None,
            forwarder: Box::pin(async move {
                let _ = forwarder_rx.await;
            }),
        };

        let mut run_fut = tasks.run().boxed_local();

        // Watcher finishes immediately, but run_fut shouldn't exit until a critical task finishes.
        assert!(futures::poll!(&mut run_fut).is_pending());

        // Send signal to forwarder: now run_fut should complete.
        let _ = forwarder_tx.send(());
        run_fut.await;
    }

    #[fuchsia::test]
    async fn run_exits_on_runner_completion() {
        let (runner_tx, runner_rx) = oneshot::channel::<()>();
        let tasks = InputPipelineTasks {
            watcher: Box::pin(async {}),
            runner: Box::pin(async move {
                let _ = runner_rx.await;
            }),
            display_ownership: None,
            focus_listener: None,
            forwarder: Box::pin(futures::future::pending()),
        };

        let mut run_fut = tasks.run().boxed_local();
        assert!(futures::poll!(&mut run_fut).is_pending());

        let _ = runner_tx.send(());
        run_fut.await;
    }
}
