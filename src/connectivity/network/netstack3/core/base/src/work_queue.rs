// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Types for dealing with collaborative work queues.

/// A common type returned by functions that perform bounded amounts of work.
///
/// This exists so cooperative task execution and yielding can be sensibly
/// performed when dealing with long work queues.
#[derive(Debug, Eq, PartialEq)]
pub enum WorkQueueReport {
    /// All the available work was done.
    AllDone,
    /// There's still pending work to do, execution was cut short.
    Pending,
}
