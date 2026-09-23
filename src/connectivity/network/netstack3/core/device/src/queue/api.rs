// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Transmit and Receive queue API objects.

use core::marker::PhantomData;

use netstack3_base::{
    ContextPair, Device, DeviceIdContext, ResourceCounterContext, WorkQueueReport,
};

use crate::DeviceCounters;
use crate::internal::base::DeviceSendFrameError;
use crate::internal::queue::rx::{
    ReceiveDequeContext, ReceiveDequeFrameContext as _, ReceiveQueueBindingsContext,
    ReceiveQueueContext as _, ReceiveQueueState,
};
use crate::internal::queue::tx::{
    self, TransmitDequeueContext, TransmitQueueBindingsContext, TransmitQueueCommon,
    TransmitQueueConfiguration, TransmitQueueContext as _, TransmitQueueState,
};
use crate::internal::queue::{BatchSize, DequeueResult, DequeueState, DeviceBufferSpec, fifo};
use crate::internal::socket::DeviceSocketHandler;
use log::debug;

/// An API to interact with device `D` transmit queues.
pub struct TransmitQueueApi<D, C>(C, PhantomData<D>);

impl<D, C> TransmitQueueApi<D, C> {
    /// Creates a new [`TransmitQueueApi`] from `ctx`.
    pub fn new(ctx: C) -> Self {
        Self(ctx, PhantomData)
    }
}

impl<D, C> TransmitQueueApi<D, C>
where
    D: DeviceBufferSpec<C::BindingsContext>,
    C: ContextPair,
    C::CoreContext:
        TransmitDequeueContext<D, C::BindingsContext> + DeviceSocketHandler<D, C::BindingsContext>,
    for<'a> <C::CoreContext as TransmitDequeueContext<D, C::BindingsContext>>::TransmitQueueCtx<'a>:
        ResourceCounterContext<<C::CoreContext as DeviceIdContext<D>>::DeviceId, DeviceCounters>,
    C::BindingsContext:
        TransmitQueueBindingsContext<<C::CoreContext as DeviceIdContext<D>>::DeviceId>,
{
    fn contexts(&mut self) -> (&mut C::CoreContext, &mut C::BindingsContext) {
        let Self(pair, PhantomData) = self;
        pair.contexts()
    }

    fn core_ctx(&mut self) -> &mut C::CoreContext {
        self.contexts().0
    }

    /// Transmits any queued frames.
    ///
    /// Up to `batch_size` frames will attempt to be dequeued and sent in this
    /// call.
    ///
    /// `dequeue_context` is directly given to the context to operate on each
    /// individual frame.
    pub fn transmit_queued_frames(
        &mut self,
        device_id: &<C::CoreContext as DeviceIdContext<D>>::DeviceId,
        batch_size: BatchSize,
        dequeue_context: &mut <
            C::CoreContext as TransmitQueueCommon<D, C::BindingsContext>
        >::DequeueContext,
    ) -> Result<WorkQueueReport, DeviceSendFrameError> {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.with_dequed_packets_and_tx_queue_ctx(
            device_id,
            |DequeueState { dequeued_frames: dequed_packets }, tx_queue_ctx| {
                assert!(
                    dequed_packets.is_empty(),
                    "should never have left packets after attempting to dequeue"
                );

                let ret = tx_queue_ctx.with_transmit_queue_mut(
                    device_id,
                    |TransmitQueueState { allocator: _, queue, checksum_offload_spec: _ }| {
                        queue.as_mut().map(|q| q.dequeue_into(dequed_packets, batch_size.into()))
                    },
                );

                // If we don't have a transmit queue installed, report no work
                // left to be done.
                let Some(ret) = ret else { return Ok(WorkQueueReport::AllDone) };

                while let Some((meta, p)) = dequed_packets.pop_front() {
                    tx::deliver_to_device_sockets(tx_queue_ctx, bindings_ctx, device_id, &p, &meta);

                    match tx_queue_ctx.send_frame(
                        bindings_ctx,
                        device_id,
                        Some(dequeue_context),
                        meta,
                        p,
                    ) {
                        Ok(()) => {}
                        Err(e) => {
                            tx_queue_ctx.increment_both(device_id, |c| &c.send_dropped_dequeue);
                            // We failed to send the frame so requeue the rest
                            // and try again later. The failed packet is lost.
                            // We shouldn't requeue it because it's already been
                            // delivered to packet sockets.
                            tx_queue_ctx.with_transmit_queue_mut(
                                device_id,
                                |TransmitQueueState {
                                     allocator: _,
                                     queue,
                                     checksum_offload_spec: _,
                                 }| {
                                    queue.as_mut().unwrap().requeue_items(dequed_packets);
                                },
                            );
                            return Err(e);
                        }
                    }
                }

                Ok(ret.into())
            },
        )
    }

    /// Returns the number of frames in `device_id`'s TX queue.
    ///
    /// Returns `None` if the device doesn't have a queue configured.
    pub fn count(
        &mut self,
        device_id: &<C::CoreContext as DeviceIdContext<D>>::DeviceId,
    ) -> Option<usize> {
        self.with_count(device_id, |count| count)
    }

    /// Calls `f` with the number of frames in `device_id`'s TX queue while the
    /// Tx queue is locked.
    pub fn with_count<R>(
        &mut self,
        device_id: &<C::CoreContext as DeviceIdContext<D>>::DeviceId,
        f: impl FnOnce(Option<usize>) -> R,
    ) -> R {
        let count = self
            .core_ctx()
            .with_transmit_queue(device_id, |TransmitQueueState { queue, .. }| {
                queue.as_ref().map(|q| q.len())
            });
        f(count)
    }

    /// Sets the queue configuration for the device.
    pub fn set_configuration(
        &mut self,
        device_id: &<C::CoreContext as DeviceIdContext<D>>::DeviceId,
        config: TransmitQueueConfiguration,
    ) {
        let (core_ctx, bindings_ctx) = self.contexts();
        // We take the dequeue lock as well to make sure we finish any current
        // dequeuing before changing the configuration.
        core_ctx.with_dequed_packets_and_tx_queue_ctx(
            device_id,
            |DequeueState { dequeued_frames: dequed_packets }, tx_queue_ctx| {
                assert!(
                    dequed_packets.is_empty(),
                    "should never have left packets after attempting to dequeue"
                );

                let prev_queue = tx_queue_ctx.with_transmit_queue_mut(
                    device_id,
                    |TransmitQueueState { allocator: _, queue, checksum_offload_spec: _ }| {
                        match config {
                            TransmitQueueConfiguration::None => core::mem::take(queue),
                            TransmitQueueConfiguration::Fifo => {
                                match queue {
                                    None => *queue = Some(fifo::Queue::default()),
                                    // Already a FiFo queue.
                                    Some(_) => {}
                                }

                                None
                            }
                        }
                    },
                );

                let Some(mut prev_queue) = prev_queue else { return };

                loop {
                    let ret = prev_queue.dequeue_into(dequed_packets, BatchSize::MAX);

                    while let Some((meta, p)) = dequed_packets.pop_front() {
                        tx::deliver_to_device_sockets(
                            tx_queue_ctx,
                            bindings_ctx,
                            device_id,
                            &p,
                            &meta,
                        );
                        match tx_queue_ctx.send_frame(bindings_ctx, device_id, None, meta, p) {
                            Ok(()) => {}
                            Err(err) => {
                                // We swapped to no-queue and device cannot send
                                // the frame so we just drop it.
                                debug!("frame dropped during queue reconfiguration: {err:?}");
                            }
                        }
                    }

                    match ret {
                        DequeueResult::NoMoreLeft => break,
                        DequeueResult::MoreStillQueued => {}
                    }
                }
            },
        )
    }
}

/// /// An API to interact with device `D` receive queues.
pub struct ReceiveQueueApi<D, C>(C, PhantomData<D>);

impl<D, C> ReceiveQueueApi<D, C> {
    /// Creates a new [`ReceiveQueueApi`] from `ctx`.
    pub fn new(ctx: C) -> Self {
        Self(ctx, PhantomData)
    }
}

impl<D, C> ReceiveQueueApi<D, C>
where
    D: Device,
    C: ContextPair,
    C::BindingsContext:
        ReceiveQueueBindingsContext<<C::CoreContext as DeviceIdContext<D>>::DeviceId>,
    C::CoreContext: ReceiveDequeContext<D, C::BindingsContext>,
{
    fn contexts(&mut self) -> (&mut C::CoreContext, &mut C::BindingsContext) {
        let Self(pair, PhantomData) = self;
        pair.contexts()
    }

    /// Handle a batch of queued RX packets for the device.
    ///
    /// If packets remain in the RX queue after a batch of RX packets has been
    /// handled, the RX task will be scheduled to run again so the next batch of
    /// RX packets may be handled. See
    /// [`ReceiveQueueBindingsContext::wake_rx_task`] for more details.
    pub fn handle_queued_frames(
        &mut self,
        device_id: &<C::CoreContext as DeviceIdContext<D>>::DeviceId,
    ) -> WorkQueueReport {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.with_dequed_frames_and_rx_queue_ctx(
            device_id,
            |DequeueState { dequeued_frames }, rx_queue_ctx| {
                assert_eq!(
                    dequeued_frames.len(),
                    0,
                    "should not keep dequeued frames across calls to this fn"
                );

                let ret = rx_queue_ctx.with_receive_queue_mut(
                    device_id,
                    |ReceiveQueueState { queue }| {
                        queue.dequeue_into(dequeued_frames, BatchSize::MAX)
                    },
                );

                while let Some((meta, p)) = dequeued_frames.pop_front() {
                    rx_queue_ctx.handle_frame(bindings_ctx, device_id, meta, p);
                }

                ret.into()
            },
        )
    }
}
