// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// A trait for types that represent a lock level in the lock dependency tracker.
pub trait LockLevel: fuchsia_rcu::RcuDroppable {
    /// The unique identifier for this lock level.
    const LOCK_ID: usize;
    /// The name of the lock level.
    const NAME: &'static str;
}
