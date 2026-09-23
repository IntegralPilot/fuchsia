// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia netdevice session.

mod buffer;
mod tx;

use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::num::{NonZeroU16, NonZeroU32, NonZeroU64, NonZeroUsize, TryFromIntError};
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicUsize};
use std::task::Waker;

use explicit::{PollExt as _, ResultExt as _};
use fidl_fuchsia_hardware_network as netdev;
use fidl_fuchsia_hardware_network::DelegatedRxLease;
use fidl_table_validation::ValidFidlTable;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::future::{Future, poll_fn};
use futures::task::{Context, Poll};
use futures::{Stream, StreamExt as _, ready};

use crate::error::{Error, Result};
use buffer::pool::{CreatedPool, Pool, RxLeaseWatcher};
use buffer::{
    AllocKind, DescId, NETWORK_DEVICE_DESCRIPTOR_LENGTH, NETWORK_DEVICE_DESCRIPTOR_VERSION,
};
pub use buffer::{
    Buffer, ChecksumRxOffloading, Rx, RxMetadata, SinglePartTxBuffer, Tx, TxMetadataMut,
};
use tx::{BufferUsageEstimator, TxState};

// TODO(https://fxbug.dev/438527741): This is the VMO ID used for single VMO
// clients (Rx + Tx in the same VMO). When VMO split is applied everywhere,
// remove this constant.
const DEFAULT_VMO_ID: u8 = 0;

/// A session between network device client and driver.
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

impl Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { inner } = self;
        let Inner { name, .. } = &**inner;
        f.debug_struct("Session").field("name", &name).finish_non_exhaustive()
    }
}

impl Session {
    /// Creates a new session with the given `name` and `config`.
    pub async fn new(
        device: &netdev::DeviceProxy,
        name: &str,
        config: Config,
    ) -> Result<(Self, Task)> {
        let sample_interval = config.buffer_usage_sample_interval;
        let inner = Inner::new(device, name, config).await?;
        let buffer_usage_sampler = inner
            .pool
            .has_decommittable_tx_vmo()
            .then(|| (fasync::Interval::new(sample_interval.into()), BufferUsageEstimator::new()));

        Ok((Session { inner: Arc::clone(&inner) }, Task { inner, buffer_usage_sampler }))
    }

    /// Sends a [`Buffer`] to the network device in this session.
    pub fn send(&self, buffer: Buffer<Tx>) {
        self.inner.send(buffer)
    }

    /// Creates a new [`RxReadyStorage`] suitable for receiving buffers in this
    /// session.
    ///
    /// The buffer capacity defaults to the session's configured
    /// `num_rx_buffers`.
    pub fn new_rx_ready_storage(&self) -> RxReadyStorage {
        RxReadyStorage::new(self.inner.num_rx_buffers)
    }

    /// Receives completed [`Buffer`]s from the network device in this session
    /// into `ready_storage` and returns an iterator over the results.
    pub async fn recv<'a>(
        &'a self,
        ready_storage: &'a mut RxReadyStorage,
    ) -> Result<impl Iterator<Item = Result<Buffer<Rx>>> + 'a> {
        self.inner.recv(ready_storage).await
    }

    /// Allocates a [`Buffer`] that may later be queued to the network device.
    ///
    /// The returned buffer will have at least `num_bytes` as size.
    pub async fn alloc_tx_buffer(&self, num_bytes: usize) -> Result<Buffer<Tx>> {
        self.inner.pool.alloc_tx_buffer(num_bytes).await
    }

    /// Tries to allocate a [`SinglePartTxBuffer`].
    ///
    /// Returns `Ok(None)` if there is no available buffer, or `Err(Error::TxLength)`
    /// if the requested size cannot meet the device requirement.
    pub fn try_alloc_single_part_tx_buffer(
        &self,
        num_bytes: usize,
    ) -> Result<Option<SinglePartTxBuffer>> {
        self.inner.pool.try_alloc_single_part_tx_buffer(num_bytes)
    }

    /// Waits for at least one TX buffer to be available and returns an iterator
    /// of buffers with `num_bytes` as capacity.
    ///
    /// The returned iterator is guaranteed to yield at least one item (though
    /// it might be an error if the requested size cannot meet the device
    /// requirement).
    ///
    /// # Note
    ///
    /// Given a `Buffer<Tx>` is returned to the pool when it's dropped, the
    /// returned iterator will seemingly yield infinite items if the yielded
    /// `Buffer`s are dropped while iterating.
    pub async fn alloc_tx_buffers(
        &self,
        num_bytes: usize,
    ) -> Result<impl Iterator<Item = Result<Buffer<Tx>>> + '_> {
        self.inner.pool.alloc_tx_buffers(num_bytes).await
    }

    /// Attaches [`Session`] to a port.
    pub async fn attach(&self, port: Port, rx_frames: &[netdev::FrameType]) -> Result<()> {
        // NB: Need to bind the future returned by `proxy.attach` to a variable
        // otherwise this function's (`Session::attach`) returned future becomes
        // not `Send` and we get unexpected compiler errors at a distance.
        //
        // The dyn borrow in the signature of `proxy.attach` seems to be the
        // cause of the compiler's confusion.
        let fut = self.inner.proxy.attach(&port.into(), rx_frames);
        let () = fut.await?.map_err(|raw| Error::Attach(port, zx::Status::err_from_raw(raw)))?;
        Ok(())
    }

    /// Detaches a port from the [`Session`].
    pub async fn detach(&self, port: Port) -> Result<()> {
        let () = self
            .inner
            .proxy
            .detach(&port.into())
            .await?
            .map_err(|raw| Error::Detach(port, zx::Status::err_from_raw(raw)))?;
        Ok(())
    }

    /// Blocks until there are no more tx buffers in flight to the backing
    /// device.
    ///
    /// Note that this method does not prevent new buffers from being allocated
    /// and sent, it is up to the caller to prevent any races. This future will
    /// resolve as soon as it observes a tx idle event. That is, there are no
    /// frames in flight to the backing device at all and the session currently
    /// owns all allocated tx buffers.
    ///
    /// The synchronization guarantee provided by this method is that any
    /// buffers previously given to [`Session::send`] will be accounted as
    /// pending until the device has replied back.
    pub async fn wait_tx_idle(&self) {
        self.inner.tx_idle_listeners.wait().await;
    }

    /// Returns a stream of delegated rx leases from the device.
    ///
    /// Leases are yielded from the stream whenever the corresponding receive
    /// buffer is dropped or reused for tx, which marks the end of processing
    /// the marked buffer for the delegated lease.
    ///
    /// See [`fidl_fuchsia_hardware_network::DelegatedRxLease`] for more
    /// details.
    ///
    /// # Panics
    ///
    /// Panics if the session was not created with
    /// [`fidl_fuchsia_hardware_network::SessionFlags::RECEIVE_RX_POWER_LEASES`]
    /// or if `watch_rx_leases` has already been called for this session.
    pub fn watch_rx_leases(&self) -> impl Stream<Item = Result<RxLease>> + Send + Sync + use<> {
        let inner = Arc::clone(&self.inner);
        let watcher = RxLeaseWatcher::new(Arc::clone(&inner.pool));
        futures::stream::try_unfold((inner, watcher), |(inner, mut watcher)| async move {
            let DelegatedRxLease {
                hold_until_frame,
                handle,
                __source_breaking: fidl::marker::SourceBreaking,
            } = match inner.proxy.watch_delegated_rx_lease().await {
                Ok(lease) => lease,
                Err(e) => {
                    if e.is_closed() {
                        return Ok(None);
                    } else {
                        return Err(Error::Fidl(e));
                    }
                }
            };
            let hold_until_frame = hold_until_frame.ok_or(Error::InvalidLease)?;
            let handle = RxLease { handle: handle.ok_or(Error::InvalidLease)? };

            watcher.wait_until(hold_until_frame).await;
            Ok(Some((handle, (inner, watcher))))
        })
    }

    /// Closes the session.
    pub async fn close(&self) -> Result<()> {
        self.inner.proxy.close()?;
        // Synchronize by waiting for the channel to be closed.
        let mut event_stream = self.inner.proxy.take_event_stream();
        while let Some(event) = event_stream.next().await {
            match event {
                Err(fidl::Error::ClientChannelClosed { .. }) => break,
                Ok(_) => {} // Ignore other events.
                Err(e) => return Err(Error::Fidl(e)),
            }
        }
        Ok(())
    }
}

struct Inner {
    pool: Arc<Pool>,
    proxy: netdev::SessionProxy,
    name: String,
    rx: fasync::Fifo<DescId<Rx>>,
    tx: fasync::Fifo<DescId<Tx>>,
    num_rx_buffers: usize,
    tx_ready: Mutex<ReadyStorage<DescId<Tx>>>,
    tx_idle_listeners: TxIdleListeners,
    tx_state: Mutex<TxState>,
}

impl Inner {
    /// Creates a new session.
    async fn new(device: &netdev::DeviceProxy, name: &str, config: Config) -> Result<Arc<Self>> {
        let CreatedPool { pool, descriptors_vmo, data_vmos } = Pool::new(config.clone())?;
        let tx_vmo_cumulative_buffers = {
            let mut cumulative = 0u16;
            config
                .tx_vmos
                .iter()
                .map(|vmo_config| {
                    cumulative += vmo_config.num_buffers;
                    cumulative
                })
                .collect::<Vec<_>>()
        };
        let tx_state = Mutex::new(TxState::new(tx_vmo_cumulative_buffers));

        let session_info = {
            // The following two constants are not provided by user, panic
            // instead of returning an error.
            let descriptor_length =
                u8::try_from(NETWORK_DEVICE_DESCRIPTOR_LENGTH / std::mem::size_of::<u64>())
                    .expect("descriptor length in 64-bit words not representable by u8");
            let data = data_vmos
                .into_iter()
                .enumerate()
                .map(|(idx, vmo)| {
                    let vmo_id = netdev::VmoId::try_from(idx).expect("invalid vmo id");
                    let num_rx_buffers = config
                        .rx_vmos
                        .iter()
                        .find(|v| v.vmo_id == vmo_id)
                        .map(|v| v.num_buffers)
                        .unwrap_or(0);
                    fidl_fuchsia_hardware_network::DataVmo {
                        id: Some(vmo_id),
                        vmo: Some(vmo),
                        num_rx_buffers: Some(num_rx_buffers),
                        __source_breaking: fidl::marker::SourceBreaking,
                    }
                })
                .collect::<Vec<_>>();
            netdev::SessionInfo {
                descriptors: Some(descriptors_vmo),
                data: Some(data),
                descriptor_version: Some(NETWORK_DEVICE_DESCRIPTOR_VERSION),
                descriptor_length: Some(descriptor_length),
                descriptor_count: Some(
                    config.num_tx_buffers().get() + config.num_rx_buffers().get(),
                ),
                options: Some(config.options),
                ..Default::default()
            }
        };

        let (client, netdev::Fifos { rx, tx }) = device
            .open_session(name, session_info)
            .await?
            .map_err(|raw| Error::Open(name.to_owned(), zx::Status::err_from_raw(raw)))?;
        let proxy = client.into_proxy();

        let rx = fasync::Fifo::from_fifo(rx);
        let tx = fasync::Fifo::from_fifo(tx);

        let vmos_to_register = if pool.has_decommittable_tx_vmo() {
            // We keep the smallest VMO always registered for Tx.
            &pool.decommittable_tx_vmo_ids[0..=0]
        } else {
            &[DEFAULT_VMO_ID]
        };
        let (s, status) = proxy.register_for_tx(vmos_to_register).await.map_err(Error::Fidl)?;
        zx::Status::ok(status).map_err(Error::RegisterForTx)?;
        assert_eq!(s, 1);

        let num_rx_buffers = config.num_rx_buffers().get().into();

        Ok(Arc::new(Self {
            pool,
            proxy,
            name: name.to_owned(),
            rx,
            tx,
            num_rx_buffers,
            tx_ready: Mutex::new(ReadyStorage::new(config.num_tx_buffers().get().into())),
            tx_idle_listeners: TxIdleListeners::new(),
            tx_state,
        }))
    }

    /// Polls to submit available rx descriptors from pool to driver.
    ///
    /// Returns the number of rx descriptors that are submitted.
    fn poll_submit_rx(&self, cx: &mut Context<'_>) -> Poll<Result<usize>> {
        self.pool.rx_pending.lock().poll_submit(&self.rx, cx)
    }

    /// Polls to submit tx descriptors that are pending to the driver.
    ///
    /// Returns the number of tx descriptors that are successfully submitted.
    fn poll_submit_tx(&self, cx: &mut Context<'_>) -> Poll<Result<usize>> {
        self.tx_state.lock().poll_submit_tx(&self.proxy, &self.pool, &self.tx, cx)
    }

    /// Polls completed tx descriptors from the driver then puts them in pool.
    fn poll_complete_tx(&self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let (count, result) = {
            let mut tx_ready = self.tx_ready.lock();
            ready!(tx_ready.poll_fifo(cx, &self.tx))
                .map_err(|status| Error::Fifo("read", "tx", status))?;
            // TODO(https://github.com/rust-lang/rust/issues/63569): Provide entire
            // chain of completed descriptors to the pool at once when slice of
            // MaybeUninit is stabilized.
            tx_ready
                .drain()
                .try_fold(0, |count, desc| match self.pool.tx_completed(desc) {
                    Ok(()) => Ok(count + 1),
                    Err(e) => Err((count, e)),
                })
                .map_or_else(|(count, e)| (count, Err(e)), |count| (count, Ok(())))
        };
        self.tx_idle_listeners.tx_complete(count);
        result?;
        Poll::Ready(Ok(()))
    }

    /// Sends the [`Buffer`] to the driver.
    ///
    /// Note: Transmit is completely infallible because the buffer layout and
    /// zero-padding are already fully resolved and verified upfront during
    /// buffer allocation (see `AllocGuard::init` in `pool.rs` for details
    /// and design tradeoffs).
    fn send(&self, buffer: Buffer<Tx>) {
        self.tx_idle_listeners.tx_started();
        let mut state = self.tx_state.lock();
        state.send(&self.proxy, &self.pool, buffer);
    }

    /// Receives [`Buffer`]s from the driver into `ready_storage`.
    ///
    /// Waits until there are completed rx buffers from the driver.
    async fn recv<'a>(
        &'a self,
        ready_storage: &'a mut RxReadyStorage,
    ) -> Result<impl Iterator<Item = Result<Buffer<Rx>>> + 'a> {
        poll_fn(|cx| ready_storage.poll_fifo(cx, &self.rx))
            .await
            .map_err(|status| Error::Fifo("read", "rx", status))?;
        Ok(ready_storage.drain().map(move |head| self.pool.rx_completed(head)))
    }
}

/// The backing task that drives the session.
///
/// A session will stop making progress if this task is not polled continuously.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Task {
    inner: Arc<Inner>,
    buffer_usage_sampler: Option<(fasync::Interval, BufferUsageEstimator)>,
}

impl Future for Task {
    type Output = Result<()>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().get_mut();
        let inner = &*this.inner;
        loop {
            let mut all_pending = true;

            // TODO(https://fxbug.dev/42158458): poll once for all completed
            // descriptors if this becomes a performance bottleneck.
            while inner.poll_complete_tx(cx)?.is_ready_checked::<()>() {
                all_pending = false;
            }
            if inner.poll_submit_rx(cx)?.is_ready_checked::<usize>() {
                all_pending = false;
            }
            if inner.poll_submit_tx(cx)?.is_ready_checked::<usize>() {
                all_pending = false;
            }

            if !all_pending {
                continue;
            }

            if let Some((interval, estimator)) = &mut this.buffer_usage_sampler {
                while interval.poll_next_unpin(cx).is_ready_checked::<Option<()>>() {
                    let buffer_usage_estimate = {
                        let mut state = inner.pool.tx_alloc_state.lock();
                        estimator.update(state.sample_peak_buffer_usage())
                    };
                    if inner.tx_state.lock().attempt_unregister(
                        buffer_usage_estimate,
                        &inner.proxy,
                        &inner.pool,
                    ) {
                        all_pending = false;
                    }
                }
            }

            if all_pending {
                return Poll::Pending;
            }
        }
    }
}

/// Configuration for a single VMO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VmoConfig {
    /// The VMO ID.
    pub(crate) vmo_id: netdev::VmoId,
    /// Number of buffers to allocate in this VMO.
    pub(crate) num_buffers: u16,
}

/// Session configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Buffer stride on VMO, in bytes.
    buffer_stride: NonZeroU64,
    /// Collection of rx VMO configurations.
    rx_vmos: Vec<VmoConfig>,
    /// Collection of tx VMO configurations.
    tx_vmos: Vec<VmoConfig>,
    /// Session flags.
    options: netdev::SessionFlags,
    /// Buffer layout.
    buffer_layout: BufferLayout,
    buffer_usage_sample_interval: std::time::Duration,
}

impl Config {
    /// Returns the total number of Rx buffers across all VMOs.
    pub fn num_rx_buffers(&self) -> NonZeroU16 {
        let sum: u16 = self.rx_vmos.iter().map(|v| v.num_buffers).sum();
        NonZeroU16::new(sum).expect("num_rx_buffers is zero")
    }

    /// Returns the total number of Tx buffers across all VMOs.
    pub fn num_tx_buffers(&self) -> NonZeroU16 {
        let sum: u16 = self.tx_vmos.iter().map(|v| v.num_buffers).sum();
        NonZeroU16::new(sum).expect("num_tx_buffers is zero")
    }
}

/// Describes the buffer layout that [`Pool`] needs to know.
#[derive(Debug, Clone, Copy)]
struct BufferLayout {
    /// Minimum tx buffer data length.
    min_tx_data: usize,
    /// Minimum tx buffer head length.
    min_tx_head: u16,
    /// Minimum tx buffer tail length.
    min_tx_tail: u16,
    /// The length of a buffer.
    length: usize,
}

/// Network device base info with all required fields.
#[derive(Debug, Clone, ValidFidlTable)]
#[fidl_table_src(netdev::DeviceBaseInfo)]
#[fidl_table_strict]
pub struct DeviceBaseInfo {
    /// Maximum number of items in rx FIFO (per session).
    pub rx_depth: u16,
    /// Maximum number of items in tx FIFO (per session).
    pub tx_depth: u16,
    /// Alignment requirement for buffers in the data VMO.
    pub buffer_alignment: u32,
    /// Maximum supported length of buffers in the data VMO, in bytes.
    #[fidl_field_type(optional)]
    pub max_buffer_length: Option<NonZeroU32>,
    /// The minimum rx buffer length required for device.
    pub min_rx_buffer_length: u32,
    /// The minimum tx buffer length required for the device.
    pub min_tx_buffer_length: u32,
    /// The number of bytes the device requests be free as `head` space in a tx buffer.
    pub min_tx_buffer_head: u16,
    /// The amount of bytes the device requests be free as `tail` space in a tx buffer.
    pub min_tx_buffer_tail: u16,
    /// Maximum descriptor chain length accepted by the device.
    pub max_buffer_parts: u8,
    /// Minimum amount of Rx buffers the client needs to prepare for the
    /// network device.
    #[fidl_field_type(optional)]
    pub min_rx_buffers: Option<NonZeroU16>,
}

/// Network device information with all required fields.
#[derive(Debug, Clone, ValidFidlTable)]
#[fidl_table_src(netdev::DeviceInfo)]
#[fidl_table_strict]
pub struct DeviceInfo {
    /// Minimum descriptor length, in 64-bit words.
    pub min_descriptor_length: u8,
    /// Accepted descriptor version.
    pub descriptor_version: u8,
    /// Device base info.
    pub base_info: DeviceBaseInfo,
}

/// Basic session configuration that can be given to [`DeviceInfo`] to generate
/// [`Config`]s.
#[derive(Debug, Copy, Clone)]
pub struct DerivableConfig {
    /// The desired default buffer length for the session.
    pub default_buffer_length: usize,
    /// Enable rx lease watching.
    pub watch_rx_leases: bool,
    /// Enable multi VMO Tx.
    pub multi_vmo: bool,
    /// Sampling interval for the buffer usage estimator.
    pub buffer_usage_sample_interval: std::time::Duration,
}

impl DerivableConfig {
    /// A sensibly common default buffer length to be used in
    /// [`DerivableConfig`]. Provided to ease test writing.
    ///
    /// Chosen to be the next power of two after the default Ethernet MTU.
    ///
    /// This is the value of the buffer length in the `Default` impl.
    pub const DEFAULT_BUFFER_LENGTH: usize = 2048;
    /// The value returned by the `Default` impl.
    pub const DEFAULT: Self = Self {
        default_buffer_length: Self::DEFAULT_BUFFER_LENGTH,
        watch_rx_leases: false,
        multi_vmo: false,
        buffer_usage_sample_interval: std::time::Duration::from_secs(1),
    };
}

impl Default for DerivableConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl DeviceInfo {
    /// Create a new session config from the device information.
    ///
    /// This method also does the boundary checks so that data_length/offset fields read
    /// from descriptors are safe to convert to [`usize`].
    pub fn make_config(&self, config: DerivableConfig) -> Result<Config> {
        let DeviceInfo {
            min_descriptor_length,
            descriptor_version,
            base_info:
                DeviceBaseInfo {
                    rx_depth,
                    tx_depth,
                    buffer_alignment,
                    max_buffer_length,
                    min_rx_buffer_length,
                    min_tx_buffer_length,
                    min_tx_buffer_head,
                    min_tx_buffer_tail,
                    max_buffer_parts: _,
                    min_rx_buffers,
                },
        } = self;
        if NETWORK_DEVICE_DESCRIPTOR_VERSION != *descriptor_version {
            return Err(Error::Config(format!(
                "descriptor version mismatch: {} != {}",
                NETWORK_DEVICE_DESCRIPTOR_VERSION, descriptor_version
            )));
        }
        if NETWORK_DEVICE_DESCRIPTOR_LENGTH < usize::from(*min_descriptor_length) {
            return Err(Error::Config(format!(
                "descriptor length too small: {} < {}",
                NETWORK_DEVICE_DESCRIPTOR_LENGTH, min_descriptor_length
            )));
        }

        let DerivableConfig {
            default_buffer_length,
            watch_rx_leases,
            multi_vmo,
            buffer_usage_sample_interval,
        } = config;

        let num_rx_buffers =
            NonZeroU16::new(*rx_depth).ok_or_else(|| Error::Config("no RX buffers".to_owned()))?;
        let num_tx_buffers =
            NonZeroU16::new(*tx_depth).ok_or_else(|| Error::Config("no TX buffers".to_owned()))?;

        let max_buffer_length = max_buffer_length
            .and_then(|max| {
                // The error case is the case where max_buffer_length can't fix in a
                // usize, but we use it to compare it to usizes, so that's
                // equivalent to no limit.
                usize::try_from(max.get()).ok_checked::<TryFromIntError>()
            })
            .unwrap_or(usize::MAX);
        let min_buffer_length = usize::try_from(*min_rx_buffer_length)
            .ok_checked::<TryFromIntError>()
            .unwrap_or(usize::MAX);

        let buffer_length =
            usize::min(max_buffer_length, usize::max(min_buffer_length, default_buffer_length));

        let buffer_alignment = usize::try_from(*buffer_alignment).map_err(
            |std::num::TryFromIntError { .. }| {
                Error::Config(format!(
                    "buffer_alignment not representable within usize: {}",
                    buffer_alignment,
                ))
            },
        )?;

        let buffer_stride = buffer_length
            .checked_add(buffer_alignment - 1)
            .map(|x| x / buffer_alignment * buffer_alignment)
            .ok_or_else(|| {
                Error::Config(format!(
                    "not possible to align {} to {} under usize::MAX",
                    buffer_length, buffer_alignment,
                ))
            })?;

        if buffer_stride < buffer_length {
            return Err(Error::Config(format!(
                "buffer stride too small {} < {}",
                buffer_stride, buffer_length
            )));
        }

        if buffer_length < usize::from(*min_tx_buffer_head) + usize::from(*min_tx_buffer_tail) {
            return Err(Error::Config(format!(
                "buffer length {} does not meet minimum tx buffer head/tail requirement {}/{}",
                buffer_length, min_tx_buffer_head, min_tx_buffer_tail,
            )));
        }

        let num_buffers = num_rx_buffers
            .get()
            .checked_add(num_tx_buffers.get())
            .filter(|num| *num != u16::MAX)
            .ok_or_else(|| {
                Error::Config(format!(
                    "too many buffers requested: {} + {} > u16::MAX",
                    num_rx_buffers, num_tx_buffers
                ))
            })?;

        let buffer_stride =
            u64::try_from(buffer_stride).map_err(|std::num::TryFromIntError { .. }| {
                Error::Config(format!("buffer_stride too big: {} > u64::MAX", buffer_stride))
            })?;

        // This is following the practice of rust stdlib to ensure allocation
        // size never reaches isize::MAX.
        // https://doc.rust-lang.org/std/primitive.pointer.html#method.add-1.
        match buffer_stride.checked_mul(num_buffers.into()).map(isize::try_from) {
            None | Some(Err(std::num::TryFromIntError { .. })) => {
                return Err(Error::Config(format!(
                    "too much memory required for the buffers: {} * {} > isize::MAX",
                    buffer_stride, num_buffers
                )));
            }
            Some(Ok(_total)) => (),
        };

        let buffer_stride = NonZeroU64::new(buffer_stride)
            .ok_or_else(|| Error::Config("buffer_stride is zero".to_owned()))?;

        let min_tx_data = match usize::try_from(*min_tx_buffer_length)
            .map(|min_tx| (min_tx <= buffer_length).then_some(min_tx))
        {
            Ok(Some(min_tx_buffer_length)) => min_tx_buffer_length,
            // Either the conversion or the comparison failed.
            Ok(None) | Err(std::num::TryFromIntError { .. }) => {
                return Err(Error::Config(format!(
                    "buffer_length smaller than minimum TX requirement: {} < {}",
                    buffer_length, *min_tx_buffer_length
                )));
            }
        };

        let mut options = netdev::SessionFlags::empty();
        options.set(netdev::SessionFlags::RECEIVE_RX_POWER_LEASES, watch_rx_leases);

        let page_size = u64::from(zx::system_get_page_size());
        let min_buffers_per_vmo = u16::try_from(std::cmp::max(1, page_size / buffer_stride.get()))
            .map_err(|TryFromIntError { .. }| {
                Error::Config("Too many buffers per VMO".to_owned())
            })?;

        let allocate_vmos =
            |total_depth: u16, min_buffers: u16, start_vmo_id: u8| -> Result<Vec<VmoConfig>> {
                let mut vmos = Vec::new();
                let mut current_buffers = min_buffers;
                let mut total_allocated = 0u16;
                let mut vmo_id = start_vmo_id;
                let mut vmos_allocated = 0;
                while total_allocated < total_depth {
                    let num_buffers = std::cmp::min(current_buffers, total_depth - total_allocated);
                    vmos.push(VmoConfig { vmo_id, num_buffers });
                    total_allocated += num_buffers;
                    vmo_id = vmo_id
                        .checked_add(1)
                        .ok_or_else(|| Error::Config("too many vmos".to_string()))?;
                    vmos_allocated += 1;
                    if vmos_allocated > 1 {
                        current_buffers = current_buffers.saturating_mul(2);
                    }
                }
                Ok(vmos)
            };

        let (rx_vmos, tx_vmos) = if multi_vmo {
            let min_rx_buffers = min_rx_buffers.unwrap_or(num_rx_buffers);
            let rx_vmos =
                allocate_vmos(num_rx_buffers.get(), min_rx_buffers.get(), DEFAULT_VMO_ID)?;
            let next_vmo_id = u8::try_from(rx_vmos.len())
                .map_err(|_| Error::Config("too many rx vmos".to_string()))?;
            let tx_vmos = allocate_vmos(num_tx_buffers.get(), min_buffers_per_vmo, next_vmo_id)?;
            (rx_vmos, tx_vmos)
        } else {
            (
                vec![VmoConfig { vmo_id: DEFAULT_VMO_ID, num_buffers: num_rx_buffers.get() }],
                vec![VmoConfig { vmo_id: DEFAULT_VMO_ID, num_buffers: num_tx_buffers.get() }],
            )
        };

        Ok(Config {
            buffer_stride,
            rx_vmos,
            tx_vmos,
            options,
            buffer_layout: BufferLayout {
                length: buffer_length,
                min_tx_head: *min_tx_buffer_head,
                min_tx_tail: *min_tx_buffer_tail,
                min_tx_data,
            },
            buffer_usage_sample_interval,
        })
    }
}

/// A port of the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Port {
    pub(crate) base: u8,
    pub(crate) salt: u8,
}

impl TryFrom<netdev::PortId> for Port {
    type Error = Error;
    fn try_from(netdev::PortId { base, salt }: netdev::PortId) -> Result<Self> {
        if base <= netdev::MAX_PORTS {
            Ok(Self { base, salt })
        } else {
            Err(Error::InvalidPortId(base))
        }
    }
}

impl From<Port> for netdev::PortId {
    fn from(Port { base, salt }: Port) -> Self {
        Self { base, salt }
    }
}

/// Pending descriptors to be sent to driver.
struct Pending<K: AllocKind> {
    storage: Vec<DescId<K>>,
    waker: Option<Waker>,
}

impl<K: AllocKind> Pending<K> {
    fn new(descs: Vec<DescId<K>>) -> Self {
        Self { storage: descs, waker: None }
    }

    /// Extends the pending descriptors buffer.
    fn extend(&mut self, descs: impl IntoIterator<Item = DescId<K>>) {
        let Self { storage, waker } = self;
        storage.extend(descs);
        if let Some(waker) = waker.take() {
            waker.wake();
        }
    }

    /// Submits the pending buffer to the driver through [`zx::Fifo`].
    ///
    /// It will return [`Poll::Pending`] if any of the following happens:
    ///   - There are no descriptors pending.
    ///   - The fifo is not ready for write.
    fn poll_submit(
        &mut self,
        fifo: &fasync::Fifo<DescId<K>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<usize>> {
        let Self { storage, waker } = self;
        if storage.is_empty() {
            if waker.as_ref().is_none_or(|waker| !waker.will_wake(cx.waker())) {
                *waker = Some(cx.waker().clone());
            }
            return Poll::Pending;
        }

        // TODO(https://fxbug.dev/42107145): We're assuming that writing to the
        // FIFO here is a sufficient memory barrier for the other end to access
        // the data. That is currently true but not really guaranteed by the
        // API.
        let submitted = ready!(fifo.try_write(cx, &storage[..]))
            .map_err(|status| Error::Fifo("write", K::REFL.as_str(), status))?
            .get();
        let _drained = storage.drain(0..submitted);
        Poll::Ready(Ok(submitted))
    }
}

/// Intermediate storage used to batch-receive rx buffers from a session.
///
/// Instances are constructed by calling [`Session::new_rx_ready_storage`].
pub struct RxReadyStorage {
    inner: ReadyStorage<DescId<Rx>>,
}

impl RxReadyStorage {
    /// Creates an [`RxReadyStorage`] that can hold `capacity` rx descriptor
    /// IDs.
    pub fn new(capacity: usize) -> Self {
        Self { inner: ReadyStorage::new(capacity) }
    }

    fn poll_fifo(
        &mut self,
        cx: &mut Context<'_>,
        fifo: &fasync::Fifo<DescId<Rx>>,
    ) -> Poll<std::result::Result<(), zx::Status>> {
        let Self { inner } = self;
        inner.poll_fifo(cx, fifo)
    }

    fn drain(&mut self) -> Drain<'_, DescId<Rx>> {
        let Self { inner } = self;
        inner.drain()
    }
}

/// Intermediate storage used to reduce syscall overhead by acting as a proxy to
/// read entries from a FIFO.
///
/// `ReadyStorage` caches read entries from a FIFO in pre-allocated memory.
pub(in crate::session) struct ReadyStorage<T> {
    // NB: A boxed slice of `MaybeUninit` here allows us to give a transparent
    // memory layout to the FIFO object but still move objects out of storage
    // without needing a `T: Default` implementation. There's a small added
    // benefit of not paying for memory initialization on creation as well, but
    // that's mostly negligible given all allocation is performed upfront.
    data: Box<[MaybeUninit<T>]>,
    available: Range<usize>,
}

impl<T> Drop for ReadyStorage<T> {
    fn drop(&mut self) {
        let _ = self.drain();
    }
}

struct Drain<'a, T> {
    ready: &'a mut ReadyStorage<T>,
}

impl<'a, T> Iterator for Drain<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        let Self { ready: ReadyStorage { data, available: Range { start, end } } } = self;
        if start != end {
            let desc = std::mem::replace(&mut data[*start], MaybeUninit::uninit());
            *start += 1;
            // SAFETY: Descriptor was in the initialized section, it was
            // initialized.
            Some(unsafe { desc.assume_init() })
        } else {
            None
        }
    }
}

impl<'a, T> Drop for Drain<'a, T> {
    fn drop(&mut self) {
        let Self { ready: ReadyStorage { data, available } } = self;
        let range = available.clone();
        // TODO(https://github.com/rust-lang/rust/issues/63569): When slice of
        // MaybeUninit is stabilized we can just drop the whole slice.
        for initialized in &mut data[range] {
            // SAFETY: the available range keeps track of initialized buffers,
            // we must drop them on drop to uphold `MaybeUninit` expectations.
            unsafe { initialized.assume_init_drop() }
        }
        *available = 0..0;
    }
}

impl<T> ReadyStorage<T> {
    pub(crate) fn new(capacity: usize) -> Self {
        let data = std::iter::from_fn(|| Some(MaybeUninit::uninit())).take(capacity).collect();
        Self { data, available: 0..0 }
    }

    fn poll_fifo(
        &mut self,
        cx: &mut Context<'_>,
        fifo: &fasync::Fifo<T>,
    ) -> Poll<std::result::Result<(), zx::Status>>
    where
        T: fasync::FifoEntry,
    {
        let Self { data, available: Range { start, end } } = self;
        if *start == *end {
            let count: NonZeroUsize = ready!(fifo.try_read(cx, &mut data[..]))?;
            *start = 0;
            *end = count.get();
        }
        Poll::Ready(Ok(()))
    }

    /// Returns an iterator that drains all of the elements from the buffer,
    /// even if dropped.
    fn drain(&mut self) -> Drain<'_, T> {
        Drain { ready: self }
    }
}

struct TxIdleListeners {
    event: event_listener::Event,
    tx_in_flight: AtomicUsize,
}

impl TxIdleListeners {
    fn new() -> Self {
        Self { event: event_listener::Event::new(), tx_in_flight: AtomicUsize::new(0) }
    }

    /// Decreases the number of outstanding tx buffers by `count`.
    ///
    /// Notifies any tx idle listeners if the number reaches 0.
    fn tx_complete(&self, count: usize) {
        let Self { event, tx_in_flight } = self;
        let old_value = tx_in_flight.fetch_sub(count, atomic::Ordering::SeqCst);
        debug_assert!(old_value >= count);
        if old_value == count {
            let _notified: usize = event.notify(usize::MAX);
        }
    }

    /// Increases the number of outstanding tx buffers by 1.
    fn tx_started(&self) {
        let Self { event: _, tx_in_flight } = self;
        let _: usize = tx_in_flight.fetch_add(1, atomic::Ordering::SeqCst);
    }

    async fn wait(&self) {
        let Self { event, tx_in_flight } = self;
        // This is _the correct way_ of holding an `event_listener::Listener`.
        // We check the condition before installing the listener in the fast
        // case, then we must check the condition again after creating the
        // listener in case we've raced with the condition updating. Finally we
        // must loop and check the condition again because we're not fully
        // guaranteed to not have spurious wakeups.
        //
        // See the event_listener crate documentation for more details.
        loop {
            if tx_in_flight.load(atomic::Ordering::SeqCst) == 0 {
                return;
            }

            event_listener::listener!(event => listener);

            if tx_in_flight.load(atomic::Ordering::SeqCst) == 0 {
                return;
            }

            listener.await;
        }
    }
}

/// An RAII lease possibly keeping the system from suspension.
///
/// Yielded from [`Session::watch_rx_leases`].
///
/// Dropping an `RxLease` relinquishes it.
#[derive(Debug)]
pub struct RxLease {
    handle: netdev::DelegatedRxLeaseHandle,
}

impl Drop for RxLease {
    fn drop(&mut self) {
        let Self { handle } = self;
        // Change detector in case we need any evolution on how to relinquish
        // leases.
        match handle {
            netdev::DelegatedRxLeaseHandle::Channel(_channel) => {
                // Dropping the channel is enough to relinquish the lease.
            }
            netdev::DelegatedRxLeaseHandle::Eventpair(_eventpair) => {
                // Dropping the eventpair is enough to relinquish the lease.
            }
            netdev::DelegatedRxLeaseHandle::__SourceBreaking { .. } => {}
        }
    }
}

impl RxLease {
    /// Peeks the internal lease.
    pub fn inner(&self) -> &netdev::DelegatedRxLeaseHandle {
        &self.handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU32;
    use std::ops::Deref;
    use std::sync::Arc;
    use std::task::Poll;

    use assert_matches::assert_matches;
    use fuchsia_async::Fifo;
    use test_case::test_case;
    use zerocopy::{FromBytes, Immutable, IntoBytes};

    use crate::session::DerivableConfig;

    use super::buffer::{
        AllocKind, DescId, NETWORK_DEVICE_DESCRIPTOR_LENGTH, NETWORK_DEVICE_DESCRIPTOR_VERSION,
    };
    use super::{
        BufferLayout, BufferUsageEstimator, Config, DeviceBaseInfo, DeviceInfo, Error, Inner,
        Mutex, Pool, ReadyStorage, Task, TxIdleListeners, TxState,
    };

    impl Inner {
        pub(super) fn new_test(
            pool: Arc<Pool>,
            proxy: netdev::SessionProxy,
            name: String,
            rx: Fifo<DescId<Rx>>,
            tx: Fifo<DescId<Tx>>,
            num_rx_buffers: usize,
            tx_ready: Mutex<ReadyStorage<DescId<Tx>>>,
            tx_idle_listeners: TxIdleListeners,
            tx_state: Mutex<TxState>,
        ) -> Self {
            Self {
                pool,
                proxy,
                name,
                rx,
                tx,
                num_rx_buffers,
                tx_ready,
                tx_idle_listeners,
                tx_state,
            }
        }

        pub(super) fn tx_state(&self) -> &Mutex<TxState> {
            &self.tx_state
        }

        pub(super) fn pool(&self) -> &Arc<Pool> {
            &self.pool
        }
    }

    impl Task {
        pub(super) fn new_test(
            inner: Arc<Inner>,
            sample_interval: Option<std::time::Duration>,
        ) -> Self {
            let buffer_usage_sampler = sample_interval
                .map(|i| (fasync::Interval::new(i.into()), BufferUsageEstimator::new()));
            Self { inner, buffer_usage_sampler }
        }
    }

    pub(super) const DEFAULT_DEVICE_BASE_INFO: DeviceBaseInfo = DeviceBaseInfo {
        rx_depth: 1,
        tx_depth: 1,
        buffer_alignment: 1,
        max_buffer_length: None,
        min_rx_buffer_length: 0,
        min_tx_buffer_head: 0,
        min_tx_buffer_length: 0,
        min_tx_buffer_tail: 0,
        max_buffer_parts: fidl_fuchsia_hardware_network::MAX_DESCRIPTOR_CHAIN,
        min_rx_buffers: None,
    };

    pub(super) const DEFAULT_DEVICE_INFO: DeviceInfo = DeviceInfo {
        min_descriptor_length: 0,
        descriptor_version: 1,
        base_info: DEFAULT_DEVICE_BASE_INFO,
    };

    const DEFAULT_BUFFER_LENGTH: usize = 2048;

    #[test_case(DEFAULT_BUFFER_LENGTH, DeviceInfo {
        min_descriptor_length: u8::MAX,
        ..DEFAULT_DEVICE_INFO
    }, format!("descriptor length too small: {} < {}", NETWORK_DEVICE_DESCRIPTOR_LENGTH, u8::MAX))]
    #[test_case(DEFAULT_BUFFER_LENGTH, DeviceInfo {
        descriptor_version: 42,
        ..DEFAULT_DEVICE_INFO
    }, format!("descriptor version mismatch: {} != {}", NETWORK_DEVICE_DESCRIPTOR_VERSION, 42))]
    #[test_case(DEFAULT_BUFFER_LENGTH, DeviceInfo {
        base_info: DeviceBaseInfo {
            tx_depth: 0,
            ..DEFAULT_DEVICE_BASE_INFO
        },
        ..DEFAULT_DEVICE_INFO
    }, "no TX buffers")]
    #[test_case(DEFAULT_BUFFER_LENGTH, DeviceInfo {
        base_info: DeviceBaseInfo {
            rx_depth: 0,
            ..DEFAULT_DEVICE_BASE_INFO
        },
        ..DEFAULT_DEVICE_INFO
    }, "no RX buffers")]
    #[test_case(DEFAULT_BUFFER_LENGTH, DeviceInfo {
        base_info: DeviceBaseInfo {
            tx_depth: u16::MAX,
            rx_depth: u16::MAX,
            ..DEFAULT_DEVICE_BASE_INFO
        },
        ..DEFAULT_DEVICE_INFO
    }, format!("too many buffers requested: {} + {} > u16::MAX", u16::MAX, u16::MAX))]
    #[test_case(DEFAULT_BUFFER_LENGTH, DeviceInfo {
        base_info: DeviceBaseInfo {
            min_tx_buffer_length: DEFAULT_BUFFER_LENGTH as u32 + 1,
            ..DEFAULT_DEVICE_BASE_INFO
        },
        ..DEFAULT_DEVICE_INFO
    }, format!(
        "buffer_length smaller than minimum TX requirement: {} < {}",
        DEFAULT_BUFFER_LENGTH, DEFAULT_BUFFER_LENGTH + 1))]
    #[test_case(DEFAULT_BUFFER_LENGTH, DeviceInfo {
        base_info: DeviceBaseInfo {
            min_tx_buffer_head: DEFAULT_BUFFER_LENGTH as u16 + 1,
            ..DEFAULT_DEVICE_BASE_INFO
        },
        ..DEFAULT_DEVICE_INFO
    }, format!(
        "buffer length {} does not meet minimum tx buffer head/tail requirement {}/0",
        DEFAULT_BUFFER_LENGTH, DEFAULT_BUFFER_LENGTH + 1))]
    #[test_case(DEFAULT_BUFFER_LENGTH, DeviceInfo {
        base_info: DeviceBaseInfo {
            min_tx_buffer_tail: DEFAULT_BUFFER_LENGTH as u16 + 1,
            ..DEFAULT_DEVICE_BASE_INFO
        },
        ..DEFAULT_DEVICE_INFO
    }, format!(
        "buffer length {} does not meet minimum tx buffer head/tail requirement 0/{}",
        DEFAULT_BUFFER_LENGTH, DEFAULT_BUFFER_LENGTH + 1))]
    #[test_case(0, DEFAULT_DEVICE_INFO, "buffer_stride is zero")]
    #[test_case(usize::MAX, DEFAULT_DEVICE_INFO,
    format!(
        "too much memory required for the buffers: {} * {} > isize::MAX",
        usize::MAX, 2))]
    #[test_case(usize::MAX, DeviceInfo {
        base_info: DeviceBaseInfo {
            buffer_alignment: 2,
            ..DEFAULT_DEVICE_BASE_INFO
        },
        ..DEFAULT_DEVICE_INFO
    }, format!(
        "not possible to align {} to {} under usize::MAX",
        usize::MAX, 2))]
    fn configs_from_device_info_err(
        buffer_length: usize,
        info: DeviceInfo,
        expected: impl Deref<Target = str>,
    ) {
        let config = DerivableConfig { default_buffer_length: buffer_length, ..Default::default() };
        assert_matches!(
            info.make_config(config),
            Err(Error::Config(got)) if got.as_str() == expected.deref()
        );
    }

    #[test_case(DeviceInfo {
        base_info: DeviceBaseInfo {
            min_rx_buffer_length: DEFAULT_BUFFER_LENGTH as u32 + 1,
            ..DEFAULT_DEVICE_BASE_INFO
        },
        ..DEFAULT_DEVICE_INFO
    }, DEFAULT_BUFFER_LENGTH + 1; "default below min")]
    #[test_case(DeviceInfo {
        base_info: DeviceBaseInfo {
            max_buffer_length: NonZeroU32::new(DEFAULT_BUFFER_LENGTH as u32 - 1),
            ..DEFAULT_DEVICE_BASE_INFO
        },
        ..DEFAULT_DEVICE_INFO
    }, DEFAULT_BUFFER_LENGTH - 1; "default above max")]
    #[test_case(DeviceInfo {
        base_info: DeviceBaseInfo {
            min_rx_buffer_length: DEFAULT_BUFFER_LENGTH as u32 - 1,
            max_buffer_length: NonZeroU32::new(DEFAULT_BUFFER_LENGTH as u32 + 1),
            ..DEFAULT_DEVICE_BASE_INFO
        },
        ..DEFAULT_DEVICE_INFO
    }, DEFAULT_BUFFER_LENGTH; "default in bounds")]
    fn configs_from_device_buffer_length(info: DeviceInfo, expected_length: usize) {
        let config = info
            .make_config(DerivableConfig {
                default_buffer_length: DEFAULT_BUFFER_LENGTH,
                ..Default::default()
            })
            .expect("is valid");
        let Config {
            buffer_layout: BufferLayout { length, min_tx_data: _, min_tx_head: _, min_tx_tail: _ },
            buffer_stride: _,
            rx_vmos: _,
            tx_vmos: _,
            options: _,
            buffer_usage_sample_interval: _,
        } = config;
        assert_eq!(length, expected_length);
    }

    #[test]
    fn multi_vmo_allocation_scheme() {
        let info = DeviceInfo {
            base_info: DeviceBaseInfo { rx_depth: 16, tx_depth: 16, ..DEFAULT_DEVICE_BASE_INFO },
            ..DEFAULT_DEVICE_INFO
        };
        let config = info
            .make_config(DerivableConfig { multi_vmo: true, ..Default::default() })
            .expect("is valid");
        let Config { rx_vmos, tx_vmos, .. } = config;
        let expected_rx = vec![VmoConfig { vmo_id: 0, num_buffers: 16 }];
        let expected_tx = vec![
            VmoConfig { vmo_id: 1, num_buffers: 2 },
            VmoConfig { vmo_id: 2, num_buffers: 2 },
            VmoConfig { vmo_id: 3, num_buffers: 4 },
            VmoConfig { vmo_id: 4, num_buffers: 8 },
        ];
        assert_eq!(rx_vmos, expected_rx);
        assert_eq!(tx_vmos, expected_tx);
    }

    #[test]
    fn multi_vmo_allocation_scheme_with_min_rx_buffers() {
        let info = DeviceInfo {
            base_info: DeviceBaseInfo {
                rx_depth: 16,
                tx_depth: 16,
                min_rx_buffers: NonZeroU16::new(2),
                ..DEFAULT_DEVICE_BASE_INFO
            },
            ..DEFAULT_DEVICE_INFO
        };
        let config = info
            .make_config(DerivableConfig { multi_vmo: true, ..Default::default() })
            .expect("is valid");
        let Config { rx_vmos, tx_vmos, .. } = config;
        let expected_rx = vec![
            VmoConfig { vmo_id: 0, num_buffers: 2 },
            VmoConfig { vmo_id: 1, num_buffers: 2 },
            VmoConfig { vmo_id: 2, num_buffers: 4 },
            VmoConfig { vmo_id: 3, num_buffers: 8 },
        ];
        let expected_tx = vec![
            VmoConfig { vmo_id: 4, num_buffers: 2 },
            VmoConfig { vmo_id: 5, num_buffers: 2 },
            VmoConfig { vmo_id: 6, num_buffers: 4 },
            VmoConfig { vmo_id: 7, num_buffers: 8 },
        ];
        assert_eq!(rx_vmos, expected_rx);
        assert_eq!(tx_vmos, expected_tx);
    }

    pub(super) fn make_fifos<K: AllocKind>() -> (Fifo<DescId<K>>, zx::Fifo<DescId<K>>) {
        let (handle, other_end) = zx::Fifo::create(256).unwrap();
        (Fifo::from_fifo(handle), other_end)
    }

    fn remove_rights<T: FromBytes + IntoBytes + Immutable>(
        fifo: Fifo<T>,
        rights_to_remove: zx::Rights,
    ) -> Fifo<T> {
        let fifo = zx::Fifo::from(fifo);
        let rights = fifo.as_handle_ref().basic_info().expect("can retrieve info").rights;

        let fifo = fifo.replace_handle(rights ^ rights_to_remove).expect("can replace");
        Fifo::from_fifo(fifo)
    }

    enum TxOrRx {
        Tx,
        Rx,
    }
    #[test_case(TxOrRx::Tx, zx::Rights::READ; "tx read")]
    #[test_case(TxOrRx::Tx, zx::Rights::WRITE; "tx write")]
    #[test_case(TxOrRx::Rx, zx::Rights::WRITE; "rx read")]
    #[fuchsia::test]
    async fn task_as_future_poll_error(which_fifo: TxOrRx, right_to_remove: zx::Rights) {
        // This is a regression test for https://fxbug.dev/42072513. The flake
        // that caused that bug occurred because the Zircon channel was closed
        // but the error returned by a failed attempt to write to it wasn't
        // being propagated upwards. This test produces a similar situation by
        // altering the right on the FIFOs the task uses so as to cause either
        // an attempt to write or to read to fail. For completeness, it
        // exercises all the FIFO polls that comprise Task::poll.
        let config = DEFAULT_DEVICE_INFO
            .make_config(DerivableConfig {
                default_buffer_length: DEFAULT_BUFFER_LENGTH,
                ..Default::default()
            })
            .expect("is valid");
        let CreatedPool { pool, descriptors_vmo: _descriptors_vmo, data_vmos: _data_vmos } =
            Pool::new(config).expect("is valid");
        let (session_proxy, _session_server) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_hardware_network::SessionMarker>();

        let (rx, _rx_sender) = make_fifos();
        let (tx, _tx_receiver) = make_fifos();

        // Attenuate rights on one of the FIFOs.
        let (tx, rx) = match which_fifo {
            TxOrRx::Tx => (remove_rights(tx, right_to_remove), rx),
            TxOrRx::Rx => (tx, remove_rights(rx, right_to_remove)),
        };

        let tx_state = Mutex::new(TxState::new(vec![]));

        let buf = pool.alloc_tx_buffer(1).await.expect("can allocate");
        let inner = Arc::new(Inner {
            pool,
            proxy: session_proxy,
            name: "fake_task".to_string(),
            rx,
            tx,
            num_rx_buffers: 10,
            tx_ready: Mutex::new(ReadyStorage::new(10)),
            tx_idle_listeners: TxIdleListeners::new(),
            tx_state,
        });

        inner.send(buf);

        let task = Task { inner, buffer_usage_sampler: None };
        futures::pin_mut!(task);

        // The task should not be able to continue because it can't read from or
        // write to one of the FIFOs.
        assert_matches!(futures::poll!(task.as_mut()), Poll::Ready(Err(Error::Fifo(_, _, _))));
    }

    #[test_case(1; "drain first")]
    #[test_case(2; "drain first two")]
    #[test_case(3; "drain all")]
    #[fuchsia::test]
    async fn ready_storage_batch_iterator(drain_first_n: usize) {
        let (handle, fifo_server) = zx::Fifo::<u32>::create(256).unwrap();
        let fifo_client = Fifo::from_fifo(handle);
        let items = vec![10u32, 20u32, 30u32];
        let written = fifo_server.write(&items[..]).expect("write to fifo");
        assert_eq!(written.get(), 3);

        let mut ready_storage = ReadyStorage::<u32>::new(10);

        // First fetch reads all 3 items into ReadyStorage.
        poll_fn(|cx| ready_storage.poll_fifo(cx, &fifo_client)).await.expect("fetch from fifo");

        let mut drain = ready_storage.drain();
        for i in 0..drain_first_n {
            assert_eq!(drain.next(), Some(items[i]));
        }
        if drain_first_n == items.len() {
            assert_eq!(drain.next(), None);
        }
        // Remaining uniterated items should be dropped and `available` reset to
        // `0..0`.
        std::mem::drop(drain);

        // Since `available` is an empty range, `poll_fifo` should block on an
        // empty FIFO.
        let fetch_fut = poll_fn(|cx| ready_storage.poll_fifo(cx, &fifo_client));
        futures::pin_mut!(fetch_fut);
        assert_matches!(futures::poll!(fetch_fut), Poll::Pending);
    }
}
