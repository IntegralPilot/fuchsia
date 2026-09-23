// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::future::Future;

/// An executor which futures can be spawned on.
pub trait Executor {
    /// A join handle which completes with the output of a future.
    ///
    /// `JoinHandle`s have detach-on-drop semantics.
    type JoinHandle<T>
    where
        T: 'static;

    /// Spawns the given future on this executor, returning a `JoinHandle` for the
    /// task.
    fn spawn<F>(&self, future: F) -> Self::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static;
}

/// A local executor which supports spawning non-`Send` futures.
pub trait LocalExecutor: Executor {
    /// Spawns the given non-`Send` future on this executor, returning a
    /// `JoinHandle` for the task.
    fn spawn_local<F>(&self, future: F) -> Self::JoinHandle<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static;
}

/// Identifies an executor as being able to run a transport.
///
/// Implementing `RunsTransport` is optional and only enables some more
/// convenient spawning APIs.
pub trait RunsTransport<T: ?Sized> {}

/// A transport which has an executor to spawn on.
///
/// Choosing an executor is optional and only enables some more convenient
/// spawning APIs.
pub trait HasExecutor {
    /// The executor to spawn on. It must be able to run this transport.
    type Executor: Executor + RunsTransport<Self>;

    /// Returns a reference to the executor for this transport.
    fn executor(&self) -> Self::Executor;
}

// Mpsc doesn't integrate with any executor internals, and so can run on any
// executor.
impl<E> RunsTransport<fidl_next_protocol::mpsc::Mpsc> for E {}
