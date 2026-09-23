// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::state_machine::{rcu_read_lock, rcu_read_unlock};
use std::marker::PhantomData;

/// A scope that holds a read lock on the RCU state machine.
///
/// This scope is used to ensure that the object referenced by the pointer remains valid until the
/// scope is dropped.
pub struct RcuReadScope {
    // We need to call `rcu_read_lock` and `rcu_read_unlock` from the same thread, so
    // we need to make `RcuReadScope` non-Send.
    _marker: PhantomData<*const ()>,
}

impl RcuReadScope {
    /// Create a new read scope.
    ///
    /// This function acquires a read lock on the RCU state machine. The read lock is held until the
    /// scope is dropped.
    pub fn new() -> Self {
        rcu_read_lock();
        Self { _marker: PhantomData }
    }
}

impl Drop for RcuReadScope {
    fn drop(&mut self) {
        rcu_read_unlock();
    }
}
