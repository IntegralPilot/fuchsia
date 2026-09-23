// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::wire;
use anyhow::{anyhow, Error};
use async_trait::async_trait;
use futures::future::{FusedFuture, OptionFuture};
use futures::stream::{Fuse, Stream};
use futures::{FutureExt, StreamExt};
use std::collections::VecDeque;
use std::io::Write;
use virtio_device::chain::WritableChain;
use virtio_device::mem::DriverMem;
use virtio_device::queue::{DescChain, DriverNotify};
use zerocopy::IntoBytes;

#[async_trait(?Send)]
pub trait InputHandler {
    async fn run(&mut self) -> Result<(), Error>;
}

pub struct InputDevice<
    'a,
    'b,
    N: DriverNotify,
    M: DriverMem,
    Q: Stream<Item = DescChain<'a, 'b, N>> + Unpin,
> {
    event_stream: Q,
    status_stream: Option<Fuse<Q>>,
    chain_buffer: VecDeque<DescChain<'a, 'b, N>>,
    mem: &'a M,
}

impl<'a, 'b, N: DriverNotify, M: DriverMem, Q: Stream<Item = DescChain<'a, 'b, N>> + Unpin>
    InputDevice<'a, 'b, N, M, Q>
{
    pub fn new(mem: &'a M, event_stream: Q, status_stream: Option<Q>) -> Self {
        Self {
            event_stream,
            status_stream: status_stream.map(StreamExt::fuse),
            chain_buffer: VecDeque::new(),
            mem,
        }
    }

    pub fn statusq_message(
        &mut self,
    ) -> impl FusedFuture<Output = Option<DescChain<'a, 'b, N>>> + '_ {
        OptionFuture::from(self.status_stream.as_mut().map(StreamExt::select_next_some))
    }

    pub fn write_events_to_queue(
        &mut self,
        events: &[wire::VirtioInputEvent],
    ) -> Result<(), Error> {
        // Acquire a descriptor for each event. Do this first because we want to send all the
        // events as part of a group or none at all.
        //
        // 5.8.6.2 Device Requirements: Device Operation
        //
        // A device MAY drop input events if the eventq does not have enough available buffers. It
        // SHOULD NOT drop individual input events if they are part of a sequence forming one input
        // device update.
        while self.chain_buffer.len() < events.len() {
            // Don't block on the stream. We prefer instead to drop input events instead of sending
            // them with a long delay.
            match self.event_stream.next().now_or_never() {
                Some(Some(chain)) => self.chain_buffer.push_back(chain),
                _ => {
                    return Err(anyhow!("Not enough descriptors available"));
                }
            }
        }

        // Write all events to the queue.
        for event in events {
            // Unwrap here because we already checked that we have a chain for each event.
            let chain = self.chain_buffer.pop_front().unwrap();
            let mut chain = WritableChain::new(chain, self.mem)?;
            chain.write_all(event.as_bytes())?;
        }
        Ok(())
    }
}
