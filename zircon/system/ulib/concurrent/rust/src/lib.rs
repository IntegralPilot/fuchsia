// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

#[cfg(test)]
extern crate std;

mod common;
pub mod copy;
mod seqlock;
mod seqlock_payload;

pub use common::{SYNC_OPT_ACQ_REL_OPS, SYNC_OPT_FENCE, SYNC_OPT_NONE, SyncOpt};
pub use copy::{
    MAX_TRANSFER_GRANULARITY, WellDefinedCopyable, well_defined_copy_from, well_defined_copy_to,
};
pub use seqlock::{ReadTransactionToken, SeqLock, SequenceNumber, WriteGuard};
pub use seqlock_payload::SeqLockPayload;
