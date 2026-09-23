// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::actor_runner::ActorRunner;
use async_trait::async_trait;
use std::fmt::Debug;

/// Every stress test must implement this trait exactly once and pass it
/// to run_test(). The test loop uses these methods to drive the stress test.
///
/// The environment is responsible for:
/// * creating actors that run during the stress test
/// * creating a new instance when an actor requests one
/// * specifying success criteria for the test (num operations or timeout)
#[async_trait]
pub trait Environment: Send + Sync + Debug {
    /// Returns the target number of operations to complete before exiting
    fn target_operations(&self) -> Option<u64>;

    /// Returns the number of seconds to wait before exiting
    fn timeout_seconds(&self) -> Option<u64>;

    /// Return the runners for all the actors
    async fn actor_runners(&mut self) -> Vec<ActorRunner>;

    /// Reset the environment, when an actor requests one
    async fn reset(&mut self);

    /// An optional hook to invoke when panicking.  Can be used to dump additional state.
    /// No reference to &self is taken by the function to make it feasible to invoke this hook while
    /// a &mut exists on the Environment.  Since this is invoked during panic, the function should
    /// avoid blocking on locks or other resources.
    fn panic_hook(&self) -> Option<Box<dyn Fn() + 'static + Sync + Send>> {
        None
    }
}
