// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Device queues.

use alloc::collections::VecDeque;

use netstack3_base::{ErrorAndSerializer, WorkQueueReport};
use packet::SerializeError;

use crate::internal::base::DeviceSendFrameError;
use crate::internal::queue::tx::TxBufferAllocator;

pub(crate) mod api;
mod fifo;
pub(crate) mod rx;
pub(crate) mod tx;

/// The maximum number of elements that can be in the RX queue.
const MAX_RX_QUEUED_LEN: usize = 10000;
/// The maximum number of elements that can be in the TX queue.
const MAX_TX_QUEUED_LEN: usize = 10000;

/// Error returned when the receive queue is full.
#[derive(Debug, PartialEq, Eq)]
pub struct ReceiveQueueFullError<T>(pub T);

#[derive(Debug, PartialEq, Eq)]
pub enum TransmitQueueFrameError<S> {
    NoQueue(DeviceSendFrameError),
    QueueFull(S),
    SerializeError(ErrorAndSerializer<SerializeError<()>, S>),
}

/// The state used to dequeue and handle frames from the device queue.
pub struct DequeueState<Meta, Buffer> {
    dequeued_frames: VecDeque<(Meta, Buffer)>,
}

impl<Meta, Buffer> Default for DequeueState<Meta, Buffer> {
    fn default() -> DequeueState<Meta, Buffer> {
        DequeueState {
            // Make sure we can dequeue up to `BatchSize` frames without
            // needing to reallocate.
            dequeued_frames: VecDeque::with_capacity(BatchSize::MAX),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EnqueueResult {
    QueueWasPreviouslyEmpty,
    QueuePreviouslyWasOccupied,
}

#[derive(Debug)]
enum DequeueResult {
    MoreStillQueued,
    NoMoreLeft,
}

impl From<DequeueResult> for WorkQueueReport {
    fn from(value: DequeueResult) -> Self {
        match value {
            DequeueResult::MoreStillQueued => Self::Pending,
            DequeueResult::NoMoreLeft => Self::AllDone,
        }
    }
}

/// A type representing an operation count in a queue (e.g. the number of
/// packets allowed to be dequeued in a single operation).
///
/// `BatchSize` is always capped at the maximum supported batch size for the
/// queue and defaults to that value.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct BatchSize(usize);

impl BatchSize {
    /// The maximum usize value that `BatchSize` can assume.
    ///
    /// Restricting the amount of work that can be done at once in a queue
    /// serves two purposes:
    ///
    /// - It sets an upper bound on the amount of work that can be done before
    ///   yielding the thread to other work.
    /// - It sets an upper bound of memory consumption set aside for dequeueing,
    ///   because dequeueing takes frames from the queue and "stages" them in a
    ///   separate context.
    pub const MAX: usize = 100;

    /// Creates a new `BatchSize` with a value of `v` saturating to
    /// [`BatchSize::MAX`].
    pub fn new_saturating(v: usize) -> Self {
        Self(v.min(Self::MAX))
    }
}

impl From<BatchSize> for usize {
    fn from(BatchSize(v): BatchSize) -> Self {
        v
    }
}

impl Default for BatchSize {
    fn default() -> Self {
        Self(Self::MAX)
    }
}

/// Specifies the buffer types for a device.
pub trait DeviceBufferSpec<BT>: netstack3_base::Device + Send + Sync {
    /// Used to for Tx Buffers.
    type TxBuffer: packet::FragmentedBuffer + AsMut<[u8]>;
    /// Used to allocate Tx Buffers.
    type TxAllocator: TxBufferAllocator<Self::TxBuffer>;
}
