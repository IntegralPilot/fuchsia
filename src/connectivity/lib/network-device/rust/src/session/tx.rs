// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::task::{Context, Poll};

use fidl_fuchsia_hardware_network as netdev;
use fuchsia_async as fasync;
use futures::FutureExt as _;

use crate::error::{Error, Result};
use crate::session::buffer::pool::Pool;
use crate::session::{Buffer, DescId, Pending, Tx};

/// Estimator for peak buffer usage.
///
/// Uses an exponentially weighted moving average (EWMA) with a safety margin
/// based on positive deviation from the mean to estimate peak buffer usage.
pub(super) struct BufferUsageEstimator {
    /// Exponentially weighted moving average of peak buffer usage, scaled by
    /// `FRAC_BITS`.
    mean_scaled: u32,
    /// Exponentially weighted moving average of the positive deviation of peak
    /// buffer usage, scaled by `FRAC_BITS`.
    ///
    /// This is an approximation of standard deviation using a variant of
    /// "mean absolute deviation". The difference is that it is not penalized
    /// when the buffer usage is below the mean because we already have a dual
    /// alpha EWMA for the mean itself. Using the "absolute deviation" poses a
    /// double penalty on falling buffer usage.
    deviation_scaled: u32,
}

impl BufferUsageEstimator {
    pub(super) fn new() -> Self {
        Self { mean_scaled: 0, deviation_scaled: 0 }
    }

    pub(super) fn update(&mut self, sample: u16) -> u16 {
        // Number of bits that are used to represent the fractional part.
        const FRAC_BITS: usize = 4;
        // Safety margin, the peak buffer usage is estimated to be `mean + K * deviation`.
        const K: u32 = 4;
        // All the arguments below are shifted right by `FRAC_BITS` when used in calculation.
        // Used to update the mean peak buffer usage on rise (attack).
        const ALPHA_UP: u32 = 1 << (FRAC_BITS - 1); // 0.5
        // Used to update the mean peak buffer usage on decline (release).
        const ALPHA_DOWN: u32 = 1 << (FRAC_BITS - 3); // 0.125
        // Used to update the deviation.
        const BETA: u32 = 1 << (FRAC_BITS - 2); // 0.25

        let update_ema = |target: &mut u32, value: u32, a: u32| {
            *target = (a * value + ((1 << FRAC_BITS) - a) * *target) >> FRAC_BITS;
        };

        let sample_scaled = u32::from(sample) << FRAC_BITS;
        let deviation_scaled = sample_scaled.saturating_sub(self.mean_scaled);
        if sample_scaled > self.mean_scaled {
            update_ema(&mut self.mean_scaled, sample_scaled, ALPHA_UP);
        } else {
            update_ema(&mut self.mean_scaled, sample_scaled, ALPHA_DOWN);
        }
        update_ema(&mut self.deviation_scaled, deviation_scaled, BETA);
        let estimate_scaled = self.mean_scaled + K * self.deviation_scaled;
        // Round it to the nearest integer.
        u16::try_from((estimate_scaled + (1 << (FRAC_BITS - 1))) >> FRAC_BITS).unwrap_or(u16::MAX)
    }
}

pub(super) type TxVmoIndex = usize;

/// The result of registering or unregistering VMOs for Tx.
///
/// Under the hood, this is a tuple `(successful_count, status_raw)` returned
/// by the FIDL protocol.
pub(super) type VmoOperationResult = (u8, i32);

pub(super) struct TxState {
    // Indexes into the [`Pool::decommittable_tx_vmo_ids`]. Any VMOs from 0 to
    // `max_registered_index` (inclusive) are registered to the driver for Tx and
    // pinned by the driver.
    max_registered_index: TxVmoIndex,
    // Indexes into the [`Pool::decommittable_tx_vmo_ids`].
    //
    // If a VMO is in the process of being registered to the driver, they are from
    // `max_registered_index` + 1 to `max_pending_registration_index` (inclusive)
    // (so `max_pending_registration_index > max_registered_index`).
    //
    // If a VMO is in the process of being unregistered, `max_pending_registration_index`
    // is decremented early to initiate the unregistration (so
    // `max_pending_registration_index < max_registered_index`).
    //
    // If no VMOs are currently in the process of being registered/unregistered,
    // this is equal to `max_registered_index`.
    max_pending_registration_index: TxVmoIndex,
    // Cumulative number of buffers for each VMO.
    tx_vmo_cumulative_buffers: Vec<u16>,
    // `UnregisterForTx` in progress. Cannot happen while `registration_fut` is
    // pending.
    unregistration_fut: Option<fidl::client::QueryResponseFut<VmoOperationResult>>,
    // `RegisterForTx` in progress. Cannot happen while `unregistration_fut` is
    // pending.
    registration_fut: Option<fidl::client::QueryResponseFut<VmoOperationResult>>,
    // Pending buffer descriptors to be sent to the driver. We will only send
    // the batch to the driver if all the VMOs in the pending descriptors have
    // been registered to the driver.
    tx_pending: Pending<Tx>,
}

impl TxState {
    pub(super) fn new(tx_vmo_cumulative_buffers: Vec<u16>) -> Self {
        Self {
            tx_pending: Pending::new(Vec::new()),
            max_registered_index: 0,
            max_pending_registration_index: 0,
            tx_vmo_cumulative_buffers,
            registration_fut: None,
            unregistration_fut: None,
        }
    }

    pub(super) fn total_registered_buffers(&self) -> u16 {
        self.tx_vmo_cumulative_buffers[self.max_registered_index]
    }

    /// Starts a `RegisterForTx` operation with the driver if not already in
    /// progress, and returns a mutable reference to the future.
    ///
    /// Panics if `max_registered_index` >= `max_pending_registration_index` or
    /// `max_pending_registration_index` >= `pool.decommittable_tx_vmo_ids.len()`.
    pub(super) fn start_or_get_pending_vmo_registration(
        &mut self,
        session_proxy: &netdev::SessionProxy,
        pool: &Pool,
    ) -> &mut fidl::client::QueryResponseFut<VmoOperationResult> {
        let pending = &pool.decommittable_tx_vmo_ids
            [self.max_registered_index + 1..=self.max_pending_registration_index];
        self.registration_fut.get_or_insert_with(|| session_proxy.register_for_tx(pending))
    }

    pub(super) fn send(
        &mut self,
        session_proxy: &netdev::SessionProxy,
        pool: &Pool,
        buffer: Buffer<Tx>,
    ) {
        let vmo_id = buffer.vmo_id();
        self.tx_pending.extend(std::iter::once(buffer.leak()));
        if pool.decommittable_tx_vmo_ids.is_empty() {
            return;
        }
        if vmo_id <= pool.decommittable_tx_vmo_ids[self.max_pending_registration_index] {
            return;
        }
        while pool.decommittable_tx_vmo_ids[self.max_pending_registration_index] < vmo_id {
            self.max_pending_registration_index += 1;
        }

        if self.unregistration_fut.is_none() {
            let _: &mut _ = self.start_or_get_pending_vmo_registration(session_proxy, pool);
        }
    }

    pub(super) fn attempt_unregister(
        &mut self,
        buffer_usage_estimate: u16,
        session_proxy: &netdev::SessionProxy,
        pool: &Pool,
    ) -> bool {
        if self.registration_fut.is_some() || self.unregistration_fut.is_some() {
            return false;
        }

        if self.max_registered_index != self.max_pending_registration_index {
            return false;
        }

        if self.max_registered_index == 0 {
            return false;
        }

        if pool.tx_alloc_state.lock().is_tx_vmo_index_in_use(self.max_registered_index) {
            return false;
        }

        if buffer_usage_estimate * 2 >= self.total_registered_buffers() {
            return false;
        }

        self.max_pending_registration_index -= 1;
        self.unregistration_fut = Some(session_proxy.unregister_for_tx(
            &pool.decommittable_tx_vmo_ids[self.max_registered_index..=self.max_registered_index],
        ));
        true
    }

    pub(super) fn poll_submit_tx(
        &mut self,
        session_proxy: &netdev::SessionProxy,
        pool: &Pool,
        tx: &fasync::Fifo<DescId<Tx>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<usize>> {
        // Fast path: no pending operations and we have all buffers registered to the driver.
        if self.max_registered_index == self.max_pending_registration_index
            && self.registration_fut.is_none()
            && self.unregistration_fut.is_none()
        {
            return self.tx_pending.poll_submit(tx, cx);
        }

        if let Some(unreg_fut) = self.unregistration_fut.as_mut() {
            let (successful, status) =
                futures::ready!(unreg_fut.poll_unpin(cx)).map_err(Error::Fidl)?;
            self.unregistration_fut = None;
            let unregistering_vmo_idx = self.max_registered_index;
            self.max_registered_index -= usize::from(successful);
            zx::Status::ok(status).map_err(Error::UnregisterForTx)?;
            // Make sure no tx buffers are allocated after we unregistered
            // with the driver.
            let tx_alloc_state = pool.tx_alloc_state.lock();
            if !tx_alloc_state.is_tx_vmo_index_in_use(unregistering_vmo_idx) {
                pool.decommit_tx_vmo(unregistering_vmo_idx)?;
            }
        }

        while self.max_registered_index < self.max_pending_registration_index {
            let fut = self.start_or_get_pending_vmo_registration(session_proxy, pool);
            let (successful, status) = futures::ready!(fut.poll_unpin(cx)).map_err(Error::Fidl)?;
            self.registration_fut = None;
            self.max_registered_index += usize::from(successful);
            zx::Status::ok(status).map_err(Error::RegisterForTx)?;
        }
        self.tx_pending.poll_submit(tx, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::buffer::pool::CreatedPool;
    use crate::session::tests::{DEFAULT_DEVICE_BASE_INFO, DEFAULT_DEVICE_INFO, make_fifos};
    use crate::session::{
        DerivableConfig, DeviceBaseInfo, DeviceInfo, Inner, Rx, Task, TxIdleListeners,
    };
    use assert_matches::assert_matches;
    use fuchsia_async::DurationExt as _;
    use fuchsia_sync::Mutex;
    use futures::StreamExt as _;
    use std::sync::Arc;

    const TICK: std::time::Duration = std::time::Duration::from_millis(10);

    async fn read_fifo(fifo: &zx::Fifo<DescId<Tx>>) -> DescId<Tx> {
        loop {
            match fifo.read_one() {
                Ok(desc) => return desc,
                Err(zx::Status::SHOULD_WAIT) => {
                    fasync::Timer::new(TICK.after_now()).await;
                }
                Err(e) => panic!("fifo read error: {:?}", e),
            }
        }
    }

    fn setup_fake_session_server() -> (
        netdev::SessionProxy,
        futures::channel::mpsc::UnboundedReceiver<Vec<u8>>,
        futures::channel::mpsc::UnboundedReceiver<Vec<u8>>,
        fasync::Task<()>,
    ) {
        let (session_proxy, mut session_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<netdev::SessionMarker>();

        let (tx_reg_sender, tx_reg_receiver) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        let (tx_unreg_sender, tx_unreg_receiver) = futures::channel::mpsc::unbounded::<Vec<u8>>();

        let server_task = fasync::Task::spawn(async move {
            while let Some(request) = session_request_stream.next().await {
                match request.expect("fidl error") {
                    netdev::SessionRequest::RegisterForTx { vmos, responder } => {
                        let count = u8::try_from(vmos.len()).unwrap();
                        tx_reg_sender.unbounded_send(vmos).unwrap();
                        responder.send(count, zx::sys::ZX_OK).unwrap();
                    }
                    netdev::SessionRequest::UnregisterForTx { vmos, responder } => {
                        let count = u8::try_from(vmos.len()).unwrap();
                        tx_unreg_sender.unbounded_send(vmos).unwrap();
                        responder.send(count, zx::sys::ZX_OK).unwrap();
                    }
                    req => {
                        unimplemented!("unexpected request {req:?}")
                    }
                }
            }
        });

        (session_proxy, tx_reg_receiver, tx_unreg_receiver, server_task)
    }

    async fn setup_test_session(
        session_proxy: netdev::SessionProxy,
    ) -> (Arc<Inner>, zx::Fifo<DescId<Tx>>, zx::Fifo<DescId<Rx>>, fasync::Task<()>) {
        let derivable_config = DerivableConfig { multi_vmo: true, ..Default::default() };
        let config = DeviceInfo {
            base_info: DeviceBaseInfo { tx_depth: 8, ..DEFAULT_DEVICE_BASE_INFO },
            ..DEFAULT_DEVICE_INFO
        }
        .make_config(derivable_config)
        .expect("is valid");

        let CreatedPool { pool, descriptors_vmo: _descriptors, data_vmos } =
            Pool::new(config).expect("Pool::new works");
        assert_eq!(data_vmos.len(), 4);

        let (rx, rx_sender) = make_fifos();
        let (tx, tx_receiver) = make_fifos();

        let tx_vmo_cumulative_buffers = vec![2, 4, 8];

        let tx_state = Mutex::new(TxState::new(tx_vmo_cumulative_buffers));

        let inner = Arc::new(Inner::new_test(
            pool,
            session_proxy,
            "test_dynamic".to_string(),
            rx,
            tx,
            10,
            Mutex::new(crate::session::ReadyStorage::new(10)),
            TxIdleListeners::new(),
            tx_state,
        ));

        let task = Task::new_test(inner.clone(), Some(TICK));
        let task_handle = fasync::Task::spawn(async move {
            assert_matches!(task.await, Err(Error::Fidl(fidl::Error::ClientChannelClosed { .. })));
        });

        (inner, tx_receiver, rx_sender, task_handle)
    }

    #[fuchsia::test]
    async fn test_dynamic_registration_and_reclamation() {
        let (session_proxy, mut tx_reg_receiver, mut tx_unreg_receiver, _server_task) =
            setup_fake_session_server();

        let (inner, tx_receiver, _rx_sender, _task_handle) =
            setup_test_session(session_proxy).await;

        {
            let state = inner.tx_state().lock();
            assert_eq!(state.max_registered_index, 0);
            assert_eq!(state.max_pending_registration_index, 0);
        }

        let buf1 = inner.pool().alloc_tx_buffer(1).await.expect("alloc first");
        assert_eq!(buf1.vmo_id(), 1);

        let buf2 = inner.pool().alloc_tx_buffer(1).await.expect("alloc second");
        assert_eq!(buf2.vmo_id(), 1);

        let buf3 = inner.pool().alloc_tx_buffer(1).await.expect("alloc third");
        assert_eq!(buf3.vmo_id(), 2);

        inner.send(buf3);

        // Verify that the server received a registration request for VMO 2
        let reg_vmos = tx_reg_receiver.next().await.expect("received registration");
        assert_eq!(reg_vmos, vec![2]);

        let buf3_desc_id = read_fifo(&tx_receiver).await;

        assert_eq!(inner.tx_state().lock().max_registered_index, 1);

        let buf4 = inner.pool().alloc_tx_buffer(1).await.expect("alloc fourth");
        assert_eq!(buf4.vmo_id(), 2);

        let buf5 = inner.pool().alloc_tx_buffer(1).await.expect("alloc fifth");
        assert_eq!(buf5.vmo_id(), 3);

        inner.send(buf5);

        let reg_vmos = tx_reg_receiver.next().await.expect("received registration for VMO 3");
        assert_eq!(reg_vmos, vec![3]);

        let buf5_desc_id = read_fifo(&tx_receiver).await;
        assert_eq!(inner.tx_state().lock().max_registered_index, 2);

        // Simulate driver completing buf3 and buf5.
        assert_eq!(
            tx_receiver.write(&[buf3_desc_id, buf5_desc_id]).expect("write to fifo").get(),
            2
        );
        drop(buf4);

        // Verify server received unregistration for VMO 3 automatically.
        let unreg_vmos = tx_unreg_receiver.next().await.expect("received unregistration");
        assert_eq!(unreg_vmos, vec![3]);

        drop(buf2);

        let unreg_vmos_2 =
            tx_unreg_receiver.next().await.expect("received unregistration for VMO 2");
        assert_eq!(unreg_vmos_2, vec![2]);
    }

    #[fuchsia::test]
    async fn test_batch_registration() {
        let (session_proxy, mut tx_reg_receiver, _tx_unreg_receiver, _server_task) =
            setup_fake_session_server();

        let (inner, tx_receiver, _rx_sender, _task_handle) =
            setup_test_session(session_proxy).await;

        let buf1 = inner.pool().alloc_tx_buffer(1).await.expect("alloc first");
        assert_eq!(buf1.vmo_id(), 1);
        let buf2 = inner.pool().alloc_tx_buffer(1).await.expect("alloc second");
        assert_eq!(buf2.vmo_id(), 1);

        let buf3 = inner.pool().alloc_tx_buffer(1).await.expect("alloc third");
        assert_eq!(buf3.vmo_id(), 2);

        let buf4 = inner.pool().alloc_tx_buffer(1).await.expect("alloc fourth");
        assert_eq!(buf4.vmo_id(), 2);

        let buf5 = inner.pool().alloc_tx_buffer(1).await.expect("alloc fifth");
        assert_eq!(buf5.vmo_id(), 3);

        inner.send(buf5);

        // Verify that the server received a registration request for [2, 3] in batch.
        let reg_vmos_batch = tx_reg_receiver.next().await.expect("received batch registration");
        assert_eq!(reg_vmos_batch, vec![2, 3]);

        let _buf5_desc_id = read_fifo(&tx_receiver).await;

        assert_eq!(inner.tx_state().lock().max_registered_index, 2);
    }

    // Note: This change is sensitive to the arguments used, this is just to
    // make sure the arithmetic makes basic sense, if the implementation
    // changes, the test needs to be updated.
    #[test]
    fn test_buffer_usage_estimator() {
        let mut estimator = BufferUsageEstimator::new();
        assert_eq!(estimator.update(10), 15);
        assert_eq!(estimator.update(10), 20);
        assert_eq!(estimator.update(5), 16);
        assert_eq!(estimator.update(5), 14);
        assert_eq!(estimator.update(5), 12);
    }
}
