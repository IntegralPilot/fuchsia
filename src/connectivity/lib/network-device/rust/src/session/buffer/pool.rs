// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia netdevice buffer pool.

use fuchsia_sync::Mutex;
use futures::task::AtomicWaker;
use std::borrow::Borrow;
use std::collections::VecDeque;
use std::convert::TryInto as _;
use std::fmt::Debug;
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem::MaybeUninit;
use std::num::{NonZeroU16, TryFromIntError};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicBool, AtomicU64};
use std::task::Poll;

use arrayvec::ArrayVec;
use explicit::ResultExt as _;
use fidl_fuchsia_hardware_network as netdev;
use fuchsia_runtime::vmar_root_self;
use futures::channel::oneshot::{Receiver, Sender, channel};

use super::{ChainLength, DescId, DescRef, DescRefMut, Descriptors};
use crate::error::{Error, Result};
use crate::session::tx::TxVmoIndex;
use crate::session::{BufferLayout, Config, DEFAULT_VMO_ID, Pending, Port};

/// Responsible for managing [`Buffer`]s for a [`Session`](crate::session::Session).
pub(in crate::session) struct Pool {
    /// Base address of the pool.
    // Note: This field requires us to manually implement `Sync` and `Send`.
    base: NonNull<u8>,
    /// The descriptors allocated for the pool.
    descriptors: Descriptors,
    /// Shared state for allocation.
    pub(in crate::session) tx_alloc_state: Mutex<TxAllocState>,
    /// The free rx descriptors pending to be sent to driver.
    pub(in crate::session) rx_pending: Mutex<Pending<Rx>>,
    /// The buffer layout.
    buffer_layout: BufferLayout,
    /// State-keeping allowing sessions to handle rx leases.
    rx_leases: RxLeaseHandlingState,
    /// All VMOs are mapped contiguously starting from `base`. This list records
    /// offsets from `base` each VMO is mapped at. The last element is the size
    /// of the entire allocation.
    vmo_offsets: Vec<usize>,
    /// VMO IDs that support dynamic decommitment. The VMO IDs are contiguous
    /// and sorted. This list is guaranteed to be non-empty if
    /// `decommittable_tx_vmar` is [`Some`].
    pub(in crate::session) decommittable_tx_vmo_ids: Vec<netdev::VmoId>,
    /// Handle used to decommit Tx-only VMOs. [`None`] when single VMO mode is
    /// in use (ie. all data is in one VMO).
    decommittable_tx_vmar: Option<zx::Vmar>,
}

// `Pool` is `Send` and `Sync`, and this allows the compiler to deduce `Buffer`
// to be `Send`. These impls are safe because we can safely share `Pool` and
// `&Pool`: the implementation would never allocate the same buffer to two
// callers at the same time.
unsafe impl Send for Pool {}
unsafe impl Sync for Pool {}

/// The shared state which keeps track of available buffers and tx buffers.
pub(in crate::session) struct TxAllocState {
    /// All pending tx allocation requests.
    requests: VecDeque<TxAllocReq>,
    free_lists: Vec<TxFreeList>,
    /// Index into `free_lists` that holds the first free list with available
    /// buffers. If there are no available buffers, this is equal to
    /// `free_lists.len()`.
    first_available_index: usize,
    /// Current number of buffers in use.
    total_in_use: u16,
    /// Peak number of buffers in use during the last measuring window. Cleared
    /// when `sample_peak_buffer_usage` is called.
    peak_in_use: u16,
}

impl TxAllocState {
    fn try_alloc(
        &mut self,
        num_parts: ChainLength,
        descriptors: &Descriptors,
    ) -> Option<Chained<DescId<Tx>>> {
        for free_list in self.free_lists[self.first_available_index..].iter_mut() {
            if let Some(allocated) = free_list.try_alloc(num_parts, descriptors) {
                while self.first_available_index < self.free_lists.len()
                    && self.free_lists[self.first_available_index].free == 0
                {
                    self.first_available_index += 1;
                }
                self.total_in_use += u16::from(num_parts.get());
                self.peak_in_use = self.peak_in_use.max(self.total_in_use);
                return Some(allocated);
            }
        }
        None
    }

    pub(in crate::session) fn sample_peak_buffer_usage(&mut self) -> u16 {
        std::mem::replace(&mut self.peak_in_use, 0)
    }

    pub(in crate::session) fn is_tx_vmo_index_in_use(&self, idx: TxVmoIndex) -> bool {
        self.free_lists[idx].max_free > self.free_lists[idx].free
    }
}

/// We use a linked list to maintain the tx free descriptors - they are linked
/// through their `nxt` fields, note this differs from the chaining expected
/// by the network device protocol:
/// - You can chain more than [`netdev::MAX_DESCRIPTOR_CHAIN`] descriptors
///   together.
/// - the free-list ends when the `nxt` field is 0xff, while the normal chain
///   ends when `chain_length` becomes 0.
struct TxFreeList {
    /// The head of a linked list of available descriptors that can be allocated
    /// for tx.
    head: Option<DescId<Tx>>,
    /// How many free descriptors are there in this list.
    free: u16,
    /// Maximum possible number of free descriptors this list can have.
    max_free: u16,
}

/// The created [`Pool`] and its backing [`zx::Vmo`]s.
pub(in crate::session) struct CreatedPool {
    pub pool: Arc<Pool>,
    pub descriptors_vmo: zx::Vmo,
    pub data_vmos: Vec<zx::Vmo>,
}

impl Pool {
    /// Creates a new [`Pool`] and its backing [`zx::Vmo`]s.
    ///
    /// When `config.multi_vmo` is true, `data_vmos` contains one RX VMO
    /// followed by multiple TX VMOs. These VMOs are mapped to `base`
    /// contiguously, with the RX VMO starting at `base` and the TX VMOs
    /// starting after the RX VMO. `vmo_offsets` contains the offset of each VMO
    /// in the `base` address space: `vmo_offsets[0]` is 0 and
    /// `vmo_offset[vmo_offset.len()-1]` is the total length of the mapped
    /// address space in bytes.
    ///
    /// When `config.multi_vmo` is false, `data_vmos` contains only one VMO for
    /// both RX and TX.
    pub(in crate::session) fn new(config: Config) -> Result<CreatedPool> {
        let create_and_name_vmo = |size: u64, name: &zx::Name| -> Result<zx::Vmo> {
            let vmo = zx::Vmo::create(size).map_err(|status| Error::Vmo("create", status))?;
            vmo.set_name(&name).map_err(|status| Error::Vmo("set_name", status))?;
            Ok(vmo)
        };

        let descriptor_count = config
            .num_rx_buffers()
            .get()
            .checked_add(config.num_tx_buffers().get())
            .ok_or_else(|| Error::Config("too many descriptors".to_string()))?;
        let descriptor_vmo_size =
            u64::try_from(super::NETWORK_DEVICE_DESCRIPTOR_LENGTH * usize::from(descriptor_count))
                .expect("vmo_size overflows u64");
        const DESCRIPTORS_VMO_NAME: zx::Name =
            const_unwrap::const_unwrap_result(zx::Name::new("netdevice:descriptors"));
        let descriptors_vmo = create_and_name_vmo(descriptor_vmo_size, &DESCRIPTORS_VMO_NAME)?;

        let Config {
            buffer_stride,
            rx_vmos,
            tx_vmos,
            options,
            buffer_layout,
            buffer_usage_sample_interval: _,
        } = config;
        let single_data_vmo =
            tx_vmos.len() == 1 && rx_vmos.len() == 1 && tx_vmos[0].vmo_id == rx_vmos[0].vmo_id;
        let decommittable_tx_vmo_ids = if single_data_vmo {
            vec![]
        } else {
            tx_vmos.iter().map(|v| v.vmo_id).collect::<Vec<_>>()
        };

        let page_size = u64::from(zx::system_get_page_size());
        let data_vmos = if single_data_vmo {
            assert_eq!(tx_vmos[0].vmo_id, DEFAULT_VMO_ID);
            assert_eq!(rx_vmos[0].vmo_id, DEFAULT_VMO_ID);
            const VMO_NAME: zx::Name =
                const_unwrap::const_unwrap_result(zx::Name::new("netdevice:data"));
            let size =
                (buffer_stride.get() * u64::from(descriptor_count)).next_multiple_of(page_size);
            let data_vmo = create_and_name_vmo(size, &VMO_NAME)?;
            vec![data_vmo]
        } else {
            const RX_VMO_NAME: zx::Name =
                const_unwrap::const_unwrap_result(zx::Name::new("netdevice:rx_data"));
            const TX_VMO_NAME: zx::Name =
                const_unwrap::const_unwrap_result(zx::Name::new("netdevice:tx_data"));

            rx_vmos
                .iter()
                .map(|vmo_config| (vmo_config, &RX_VMO_NAME))
                .chain(tx_vmos.iter().map(|vmo_config| (vmo_config, &TX_VMO_NAME)))
                .map(|(vmo_config, name)| {
                    let size = (buffer_stride.get() * u64::from(vmo_config.num_buffers))
                        .next_multiple_of(page_size);
                    create_and_name_vmo(size, name)
                })
                .collect::<Result<Vec<_>>>()?
        };

        let (descriptors, mut tx_free, mut rx_free) =
            Descriptors::new(&rx_vmos, &tx_vmos, buffer_stride, &descriptors_vmo)?;

        for rx_desc in rx_free.iter_mut() {
            descriptors.borrow_mut(rx_desc).initialize(
                ChainLength::ZERO,
                0,
                buffer_layout.length.try_into().unwrap(),
                0,
            );
        }

        let mut total_len = 0;
        let mut vmo_offsets = vec![0];

        for vmo in &data_vmos {
            total_len +=
                usize::try_from(vmo.get_size().map_err(|status| Error::VmoSize("data", status))?)
                    .expect("usize must be able to hold u64");
            vmo_offsets.push(total_len);
        }

        // Use variable shadowing to prevent any modification from now on.
        let (total_len, vmo_offsets) = (total_len, vmo_offsets);

        let map_data_vmo = |vmar: &zx::Vmar,
                            data_vmo: usize,
                            vmar_start_offset: usize|
         -> Result<()> {
            let offset = vmo_offsets[data_vmo] - vmar_start_offset;
            let len = vmo_offsets[data_vmo + 1] - vmo_offsets[data_vmo];
            let _addr = vmar
                .map(
                    offset,
                    &data_vmos[data_vmo],
                    0,
                    len,
                    zx::VmarFlags::SPECIFIC | zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
                )
                .map_err(|status| Error::Map("map data vmo", status))?;
            Ok(())
        };

        let (vmar, vmar_start) = vmar_root_self()
            .allocate(
                0,
                total_len,
                zx::VmarFlags::CAN_MAP_READ
                    | zx::VmarFlags::CAN_MAP_WRITE
                    | zx::VmarFlags::CAN_MAP_SPECIFIC,
            )
            .map_err(|status| Error::Map("allocate vmar", status))?;
        let base = NonNull::new(vmar_start as *mut u8).expect("must not be null");

        for i in 0..rx_vmos.len() {
            map_data_vmo(&vmar, i, 0)?;
        }

        let decommittable_tx_vmar = (!single_data_vmo)
            .then(|| {
                let num_rx_vmo = rx_vmos.len();
                let rx_size = vmo_offsets[num_rx_vmo];
                let tx_vmar_len = total_len - rx_size;
                let (tx_vmar, _tx_vmar_base) = vmar
                    .allocate(
                        rx_size,
                        tx_vmar_len,
                        zx::VmarFlags::SPECIFIC
                            | zx::VmarFlags::CAN_MAP_READ
                            | zx::VmarFlags::CAN_MAP_WRITE
                            | zx::VmarFlags::CAN_MAP_SPECIFIC,
                    )
                    .map_err(|status| Error::Map("allocate tx-vmar", status))?;
                for i in num_rx_vmo..data_vmos.len() {
                    map_data_vmo(&tx_vmar, i, rx_size)?;
                }
                Ok::<_, Error>(tx_vmar)
            })
            .transpose()?;

        let mut free_lists = Vec::new();
        for vmo_config in &tx_vmos {
            let head = tx_free.drain(..usize::from(vmo_config.num_buffers)).rev().fold(
                None,
                |head, mut curr| {
                    descriptors.borrow_mut(&mut curr).set_nxt(head);
                    assert_eq!(descriptors.borrow(&curr).vmo_id(), vmo_config.vmo_id);
                    Some(curr)
                },
            );
            free_lists.push(TxFreeList {
                head,
                free: vmo_config.num_buffers,
                max_free: vmo_config.num_buffers,
            });
        }

        let tx_alloc_state = Mutex::new(TxAllocState {
            free_lists,
            requests: VecDeque::new(),
            first_available_index: 0,
            total_in_use: 0,
            peak_in_use: 0,
        });

        let pool = Arc::new(Pool {
            base,
            descriptors,
            tx_alloc_state,
            rx_pending: Mutex::new(Pending::new(rx_free)),
            buffer_layout,
            rx_leases: RxLeaseHandlingState::new_with_flags(options),
            vmo_offsets,
            decommittable_tx_vmo_ids,
            decommittable_tx_vmar,
        });

        Ok(CreatedPool { pool, descriptors_vmo, data_vmos })
    }

    /// Allocates `num_parts` tx descriptors.
    ///
    /// It will block if there are not enough descriptors. Note that the
    /// descriptors are not initialized, you need to call [`AllocGuard::init()`]
    /// on the returned [`AllocGuard`] if you want to send it to the driver
    /// later.
    pub(in crate::session) async fn alloc_tx(
        self: &Arc<Self>,
        num_parts: ChainLength,
    ) -> AllocGuard<Tx> {
        let receiver = {
            let mut state = self.tx_alloc_state.lock();
            match state.try_alloc(num_parts, &self.descriptors) {
                Some(allocated) => {
                    return AllocGuard::new(allocated, self.clone());
                }
                None => {
                    let (request, receiver) = TxAllocReq::new(num_parts);
                    state.requests.push_back(request);
                    receiver
                }
            }
        };
        // The sender must not be dropped.
        receiver.await.unwrap()
    }

    /// Tries to allocate a [`SinglePartTxBuffer`].
    ///
    /// Returns `Ok(None)` if there is no available buffer, or `Err(Error::TxLength)`
    /// if the requested size cannot meet the device requirement.
    pub(in crate::session) fn try_alloc_single_part_tx_buffer(
        self: &Arc<Self>,
        num_bytes: usize,
    ) -> Result<Option<SinglePartTxBuffer>> {
        let BufferLayout { min_tx_data: _, min_tx_head, min_tx_tail, length: buffer_length } =
            self.buffer_layout;
        if num_bytes > buffer_length - usize::from(min_tx_head) - usize::from(min_tx_tail) {
            return Err(Error::TxLength);
        }
        self.tx_alloc_state
            .lock()
            .try_alloc(ChainLength::try_from(1u8).unwrap(), &self.descriptors)
            .map(|allocated| -> Result<SinglePartTxBuffer> {
                let mut alloc = AllocGuard::new(allocated, self.clone());
                alloc.init(num_bytes)?;
                let buffer = Buffer::from(alloc);
                Ok(SinglePartTxBuffer::new(buffer, num_bytes).expect("must be single part"))
            })
            .transpose()
    }

    /// Allocates a tx [`Buffer`].
    ///
    /// The returned buffer will have `num_bytes` as its capacity, the method
    /// will block if there are not enough buffers. An error will be returned if
    /// the requested size cannot meet the device requirement, for example, if
    /// the size of the head or tail region will become unrepresentable in u16.
    pub(in crate::session) async fn alloc_tx_buffer(
        self: &Arc<Self>,
        num_bytes: usize,
    ) -> Result<Buffer<Tx>> {
        self.alloc_tx_buffers(num_bytes).await?.next().unwrap()
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
    pub(in crate::session) async fn alloc_tx_buffers<'a>(
        self: &'a Arc<Self>,
        num_bytes: usize,
    ) -> Result<impl Iterator<Item = Result<Buffer<Tx>>> + 'a> {
        let BufferLayout { min_tx_data, min_tx_head, min_tx_tail, length: buffer_length } =
            self.buffer_layout;
        let tx_head = usize::from(min_tx_head);
        let tx_tail = usize::from(min_tx_tail);
        let total_bytes = num_bytes.max(min_tx_data) + tx_head + tx_tail;
        let num_parts = (total_bytes + buffer_length - 1) / buffer_length;
        let chain_length = ChainLength::try_from(num_parts)?;
        let first = self.alloc_tx(chain_length).await;
        let iter = std::iter::once(first)
            .chain(std::iter::from_fn(move || {
                let mut state = self.tx_alloc_state.lock();
                state
                    .try_alloc(chain_length, &self.descriptors)
                    .map(|allocated| AllocGuard::new(allocated, self.clone()))
            }))
            // Fuse afterwards so we're guaranteeing we can't see a new entry
            // after having yielded `None` once.
            .fuse()
            .map(move |mut alloc| {
                alloc.init(num_bytes)?;
                Ok(alloc.into())
            });
        Ok(iter)
    }

    /// Frees rx descriptors.
    pub(in crate::session) fn free_rx(&self, descs: impl IntoIterator<Item = DescId<Rx>>) {
        self.rx_pending.lock().extend(descs.into_iter().map(|mut desc| {
            self.descriptors.borrow_mut(&mut desc).initialize(
                ChainLength::ZERO,
                0,
                self.buffer_layout.length.try_into().unwrap(),
                0,
            );
            desc
        }));
    }

    /// Frees tx descriptors.
    ///
    /// # Panics
    ///
    /// Panics if given an empty chain.
    fn free_tx(self: &Arc<Self>, chain: Chained<DescId<Tx>>) {
        // We store any pending request that need to be fulfilled in the stack
        // here, to fulfill them only once we drop the lock, guaranteeing an
        // AllocGuard can't be dropped while the lock is held.
        let mut to_fulfill = ArrayVec::<
            (TxAllocReq, AllocGuard<Tx>),
            { netdev::MAX_DESCRIPTOR_CHAIN as usize },
        >::new();

        let mut state = self.tx_alloc_state.lock();
        state.total_in_use -= u16::from(chain.len.get());
        {
            let vmo_id =
                self.descriptors.borrow(chain.first().expect("chain is not empty")).vmo_id();
            let idx = self.free_list_index(vmo_id);
            if idx < state.first_available_index {
                state.first_available_index = idx;
            }

            let mut descs = chain.into_iter();
            state.free_lists[idx].free += u16::try_from(descs.len()).unwrap();
            let head = descs.next();
            let old_head = std::mem::replace(&mut state.free_lists[idx].head, head);
            let mut tail = descs.last();
            let mut tail_ref = self.descriptors.borrow_mut(
                tail.as_mut().unwrap_or_else(|| state.free_lists[idx].head.as_mut().unwrap()),
            );
            tail_ref.set_nxt(old_head);
        }

        // After putting the chain back into the free list, we try to fulfill
        // any pending tx allocation requests.
        while let Some(req) = state.requests.front() {
            // Skip a request that we know is canceled.
            //
            // This is an optimization for long-ago dropped requests, since the
            // receiver side can be dropped between here and fulfillment later.
            if req.sender.is_canceled() {
                let _cancelled: Option<TxAllocReq> = state.requests.pop_front();
                continue;
            }
            let size = req.size;
            match state.try_alloc(size, &self.descriptors) {
                Some(descs) => {
                    // The unwrap is safe because we know requests is not empty.
                    let req = state.requests.pop_front().unwrap();
                    to_fulfill.push((req, AllocGuard::new(descs, self.clone())));

                    // If we're full temporarily release the lock to go again
                    // later. Fulfilling a request must _always_ be done without
                    // holding the lock.
                    if to_fulfill.is_full() {
                        drop(state);
                        for (req, alloc) in to_fulfill.drain(..) {
                            req.fulfill(alloc)
                        }
                        state = self.tx_alloc_state.lock();
                    }
                }
                None => break,
            }
        }

        // Make sure we're not holding the state lock when fulfilling requests.
        drop(state);
        // Fulfill any ready requests.
        for (req, alloc) in to_fulfill {
            req.fulfill(alloc)
        }
    }

    /// Frees the completed tx descriptors chained by head to the pool.
    ///
    /// Call this function when the driver hands back a completed tx descriptor.
    pub(in crate::session) fn tx_completed(self: &Arc<Self>, head: DescId<Tx>) -> Result<()> {
        let chain = self.descriptors.chain(head).collect::<Result<Chained<_>>>()?;
        Ok(self.free_tx(chain))
    }

    /// Creates a [`Buffer<Rx>`] corresponding to the completed rx descriptors.
    ///
    /// Whenever the driver hands back a completed rx descriptor, this function
    /// can be used to create the buffer that is represented by those chained
    /// descriptors.
    pub(in crate::session) fn rx_completed(
        self: &Arc<Self>,
        head: DescId<Rx>,
    ) -> Result<Buffer<Rx>> {
        let descs = self.descriptors.chain(head).collect::<Result<Chained<_>>>()?;
        let alloc = AllocGuard::new(descs, self.clone());
        Ok(alloc.into())
    }

    fn get_slice_layout<K: AllocKind>(&self, desc: &super::Descriptor<K>) -> (usize, usize) {
        let vmo_id = usize::from(desc.vmo_id());
        if vmo_id >= self.vmo_offsets.len() - 1 {
            panic!("invalid vmo_id {} for vmo_offsets of len {}", vmo_id, self.vmo_offsets.len());
        }
        let vmo_offset = self.vmo_offsets[vmo_id];
        let next_vmo_offset = self.vmo_offsets[vmo_id + 1];

        let desc_offset = desc.offset();
        let head_len = u64::from(desc.head_length());
        let data_len = u64::from(desc.data_length());

        let total_offset = desc_offset
            .checked_add(head_len)
            .and_then(|o| usize::try_from(o).ok())
            .and_then(|o| o.checked_add(vmo_offset))
            .unwrap_or_else(|| panic!("offset calculation overflowed"));

        let len =
            usize::try_from(data_len).unwrap_or_else(|_| panic!("data_length overflowed usize"));

        let end = total_offset
            .checked_add(len)
            .unwrap_or_else(|| panic!("end offset calculation overflowed"));

        if end > next_vmo_offset {
            panic!("slice end {} out of VMO bounds {}", end, next_vmo_offset);
        }

        (total_offset, len)
    }

    fn get_slice<'a, K: AllocKind>(&self, desc: &'a DescId<K>) -> &'a [u8] {
        let desc = self.descriptors.borrow(desc);
        let (offset, len) = self.get_slice_layout(&desc);
        // Safety: The descriptor is describing a buffer from this pool. It must
        // be valid to create a slice into that region. We hold a immutable
        // reference to the underlying descriptor, this means no one else should
        // have mutable reference to this memory region.
        unsafe {
            let ptr = self.base.as_ptr().add(offset);
            std::slice::from_raw_parts(ptr, len)
        }
    }

    fn get_slice_mut<'a, K: AllocKind>(&self, desc: &'a mut DescId<K>) -> &'a mut [u8] {
        let desc = self.descriptors.borrow_mut(desc);
        let (offset, len) = self.get_slice_layout(&*desc);
        // Safety: The descriptor is describing a buffer from this pool. It must
        // be valid to create a slice into that region. We hold a mutable
        // reference to the underlying descriptor, this means we are currently
        // the only one has access to this memory region.
        unsafe {
            let ptr = self.base.as_ptr().add(offset);
            std::slice::from_raw_parts_mut(ptr, len)
        }
    }

    pub(in crate::session) fn decommit_tx_vmo(&self, tx_vmo_idx: TxVmoIndex) -> Result<()> {
        let tx_vmar = self
            .decommittable_tx_vmar
            .as_ref()
            .ok_or(Error::Vmo("decommit", zx::Status::NOT_SUPPORTED))?;
        let vmo_idx = tx_vmo_idx + usize::from(self.decommittable_tx_vmo_ids[0]);
        let start_offset = self.vmo_offsets[vmo_idx];
        let end_offset = self.vmo_offsets[vmo_idx + 1];
        let len = end_offset - start_offset;
        let addr = self.base.addr().get() + start_offset;
        tx_vmar
            .op_range(zx::VmarOp::DECOMMIT, addr, len)
            .map_err(|status| Error::Vmo("decommit", status))
    }

    pub(in crate::session) fn has_decommittable_tx_vmo(&self) -> bool {
        self.decommittable_tx_vmar.is_some()
    }

    fn free_list_index(&self, vmo_id: u8) -> usize {
        if self.decommittable_tx_vmo_ids.is_empty() {
            // If we are in the single data VMO mode, there is only one free list.
            0
        } else {
            // If we have multiple dedicated Tx VMOs, then free_lists[0] is used to
            // track free buffers from VMO `decommittable_tx_vmo_ids[0]`. The VMO
            // ids from `decommittable_tx_vmo_ids` are contiguous and sorted so
            // we can just offset the vmo_id by the first vmo_id to get the
            // index of the free list to use.
            usize::from(vmo_id - self.decommittable_tx_vmo_ids[0])
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let bytes = *self.vmo_offsets.last().unwrap();
        unsafe {
            vmar_root_self()
                .unmap(self.base.as_ptr() as usize, bytes)
                .expect("failed to unmap VMO for Pool")
        }
    }
}

impl TxFreeList {
    /// Tries to allocate tx descriptors.
    ///
    /// Returns [`None`] if there are not enough descriptors.
    fn try_alloc(
        &mut self,
        num_parts: ChainLength,
        descriptors: &Descriptors,
    ) -> Option<Chained<DescId<Tx>>> {
        if u16::from(num_parts.get()) > self.free {
            return None;
        }

        let free_list = std::iter::from_fn(|| -> Option<DescId<Tx>> {
            let new_head = self.head.as_ref().and_then(|head| {
                let nxt = descriptors.borrow(head).nxt();
                nxt.map(|id| unsafe {
                    // Safety: This is the nxt field of head of the free list,
                    // it must be a tx descriptor id.
                    DescId::from_raw(id)
                })
            });
            std::mem::replace(&mut self.head, new_head)
        });
        let allocated = free_list.take(num_parts.get().into()).collect::<Chained<_>>();
        assert_eq!(allocated.len(), usize::from(num_parts));
        self.free -= u16::from(num_parts.get());
        Some(allocated)
    }
}

/// The buffer that can be used by the [`Session`](crate::session::Session).
pub struct Buffer<K: AllocKind> {
    /// The descriptors allocation.
    alloc: AllocGuard<K>,
}

impl<K: AllocKind> Buffer<K> {
    pub(in crate::session) fn vmo_id(&self) -> u8 {
        // Safety: Must not have an empty allocation.
        let desc_id = unsafe { self.alloc.descs.storage[0].assume_init_ref() };
        self.alloc.pool.descriptors.borrow(desc_id).vmo_id()
    }

    /// Returns the length of data region of the buffer.
    pub fn len(&self) -> usize {
        self.parts().map(|s| s.len()).sum()
    }

    /// Returns an iterator over the data slices of the buffer parts.
    fn parts(&self) -> impl Iterator<Item = &[u8]> + '_ {
        self.alloc.descs.iter().map(|desc| self.alloc.pool.get_slice(desc))
    }

    /// Returns an iterator over the mutable valid data slices of the buffer parts.
    fn parts_mut(&mut self) -> impl Iterator<Item = &mut [u8]> + '_ {
        self.alloc.descs.iter_mut().map(|desc| self.alloc.pool.get_slice_mut(desc))
    }

    /// Leaks the underlying buffer descriptors to the driver.
    pub(in crate::session) fn leak(mut self) -> DescId<K> {
        let descs = std::mem::replace(&mut self.alloc.descs, Chained::empty());
        descs.into_iter().next().unwrap()
    }

    /// Returns the buffer data as a slice.
    pub fn as_slice(&self) -> Option<&[u8]> {
        if self.alloc.len() != 1 {
            return None;
        }
        self.parts().next()
    }

    /// Returns the buffer data as a mutable slice.
    pub fn as_slice_mut(&mut self) -> Option<&mut [u8]> {
        if self.alloc.len() != 1 {
            return None;
        }
        self.parts_mut().next()
    }

    /// Returns a wrapper for read-only operations.
    pub fn io(&self) -> BufferIORef<'_, K> {
        let mut len = 0;
        let parts: Chained<&[u8]> = self.parts().inspect(|s| len += s.len()).collect();
        BufferIO { parts, pos: 0, len, _marker: std::marker::PhantomData }
    }

    /// Returns a wrapper for read-write operations.
    pub fn io_mut(&mut self) -> BufferIOMut<'_, K> {
        let mut len = 0;
        let parts: Chained<&mut [u8]> = self.parts_mut().inspect(|s| len += s.len()).collect();
        BufferIO { parts, pos: 0, len, _marker: std::marker::PhantomData }
    }
}

/// A guard for mutating metadata on a Tx buffer.
///
/// Holds an exclusive borrow of the underlying descriptor (`DescRefMut`),
/// ensuring that atomic reference counting is performed only once when obtaining
/// and dropping this guard, rather than on every individual field update.
pub struct TxMetadataMut<'a> {
    desc: DescRefMut<'a, Tx>,
}

impl<'a> TxMetadataMut<'a> {
    /// Sets the buffer's destination port.
    pub fn set_port(&mut self, port: Port) {
        self.desc.set_port(port);
    }

    /// Sets the frame type of the buffer.
    pub fn set_frame_type(&mut self, frame_type: netdev::FrameType) {
        self.desc.set_frame_type(frame_type);
    }

    /// Sets TxFlags of a Tx buffer.
    pub fn set_tx_flags(&mut self, flags: netdev::TxFlags) {
        self.desc.set_tx_flags(flags);
    }

    /// Sets the generic checksum offload metadata for this buffer.
    pub fn set_generic_csum_offload(&mut self, start: u16, offset: u16) {
        self.desc.set_generic_csum_offload(start, offset);
    }
}

impl Buffer<Tx> {
    /// Returns a guard for mutating TX buffer metadata.
    pub fn meta_mut(&mut self) -> TxMetadataMut<'_> {
        TxMetadataMut { desc: self.alloc.descriptor_mut() }
    }

    /// Shrinks the buffer.
    ///
    /// This method shrinks the buffer length to the larger of
    ///   - requested new length
    ///   - device required minimum Tx data length
    ///
    /// It is an error to try to increase the buffer length.
    pub fn shrink_to(&mut self, mut new_len: usize) -> Result<()> {
        let current_len = self.len();

        if new_len > current_len {
            return Err(Error::TxLength);
        }

        let min_tx_data = usize::from(self.alloc.pool.buffer_layout.min_tx_data);
        new_len = new_len.max(min_tx_data);

        let layouts = self.alloc.calculate_descriptor_layouts(new_len)?;

        for (desc_id, DescriptorLayout { data_length, tail_length, .. }) in
            self.alloc.descs.iter_mut().zip(layouts)
        {
            let mut descriptor = self.alloc.pool.descriptors.borrow_mut(desc_id);
            descriptor.set_data_length(data_length);
            descriptor.set_tail_length(tail_length);
        }
        Ok(())
    }
}

/// The rx checksum offloading information.
pub enum ChecksumRxOffloading {
    /// N checksums were fully verified by the device.
    Offloaded(NonZeroU16),
}

/// A guard for reading metadata on an Rx buffer.
///
/// Holds a shared borrow of the underlying descriptor (`DescRef`),
/// ensuring that atomic reference counting is performed only once when obtaining
/// and dropping this guard, rather than on every individual field access.
pub struct RxMetadata<'a> {
    desc: DescRef<'a, Rx>,
}

impl<'a> RxMetadata<'a> {
    /// Retrieves RxFlags of an Rx Buffer.
    pub fn rx_flags(&self) -> Result<netdev::RxFlags> {
        self.desc.rx_flags()
    }

    /// Retrieves the checksum offloading information.
    pub fn rx_checksum_offloading(&self) -> Option<ChecksumRxOffloading> {
        self.desc.rx_checksum_offloading()
    }

    /// Retrieves the frame type of the buffer.
    pub fn frame_type(&self) -> Result<netdev::FrameType> {
        self.desc.frame_type()
    }

    /// Retrieves the buffer's source port.
    pub fn port(&self) -> Port {
        self.desc.port()
    }
}

impl Buffer<Rx> {
    /// Returns a guard for reading RX buffer metadata.
    pub fn meta(&self) -> RxMetadata<'_> {
        RxMetadata { desc: self.alloc.descriptor() }
    }
}

impl<K: AllocKind> Debug for Buffer<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { alloc } = self;
        f.debug_struct("Buffer").field("alloc", alloc).finish()
    }
}

/// A witness type that proves the buffer is backed by one part only
/// and thus can be converted into `&[u8]`.
pub struct SinglePartTxBuffer(Buffer<Tx>);

impl SinglePartTxBuffer {
    /// Creates a new [`SinglePartTxBuffer`] from a [`Buffer<Tx>`] if it is
    /// backed by one part only.
    pub fn new(buffer: Buffer<Tx>, len: usize) -> Option<Self> {
        if buffer.alloc.len() != 1 {
            None
        } else {
            let cap = usize::try_from(buffer.alloc.descriptor().data_length())
                .expect("u32 must fit in a usize");
            if cap < len { None } else { Some(Self(buffer)) }
        }
    }

    /// Converts back to a Tx buffer.
    pub fn into_inner(self) -> Buffer<Tx> {
        let Self(buffer) = self;
        buffer
    }
}

impl AsRef<[u8]> for SinglePartTxBuffer {
    fn as_ref(&self) -> &[u8] {
        // Safety: `SinglePartTxBuffer` is guaranteed to have exactly one part
        // (verified on creation), so the first descriptor is always initialized.
        let desc = unsafe { self.0.alloc.descs.storage[0].assume_init_ref() };
        self.0.alloc.pool.get_slice(desc)
    }
}

impl AsMut<[u8]> for SinglePartTxBuffer {
    fn as_mut(&mut self) -> &mut [u8] {
        // Safety: `SinglePartTxBuffer` is guaranteed to have exactly one part
        // (verified on creation), so the first descriptor is always initialized.
        let desc = unsafe { self.0.alloc.descs.storage[0].assume_init_mut() };
        self.0.alloc.pool.get_slice_mut(desc)
    }
}

impl packet::FragmentedBuffer for SinglePartTxBuffer {
    fn len(&self) -> usize {
        let desc = self.0.alloc.descriptor();
        usize::try_from(desc.data_length()).expect("u32 must fit in a usize")
    }

    fn with_bytes<'a, R, F>(&'a self, f: F) -> R
    where
        F: for<'b> FnOnce(packet::FragmentedBytes<'b, 'a>) -> R,
    {
        f(packet::FragmentedBytes::new(&mut [self.as_ref()][..]))
    }
}

/// A wrapper around [`Buffer`] for sequential I/O.
///
/// `T` must be a slice reference type, typically `&'a [u8]` for read-only
/// operations, or `&'a mut [u8]` for read-write operations.
pub struct BufferIO<T, K: AllocKind> {
    parts: Chained<T>,
    pos: usize,
    len: usize,
    _marker: std::marker::PhantomData<K>,
}

pub type BufferIORef<'a, K> = BufferIO<&'a [u8], K>;
pub type BufferIOMut<'a, K> = BufferIO<&'a mut [u8], K>;

impl<T> BufferIO<T, Tx>
where
    T: AsMut<[u8]>,
{
    /// Writes data from `src` into the TX buffer starting at the specified `offset`.
    ///
    /// This method is infallible. It returns the number of bytes successfully written.
    ///
    /// If the specified `offset` is greater than or equal to the total length of the
    /// buffer, or if the buffer has no remaining capacity at the offset, `0` bytes
    /// will be written.
    ///
    /// If `src` is larger than the remaining capacity of the buffer starting at
    /// `offset`, a short write occurs: only the bytes that fit within the buffer
    /// are written, and the returned value will be less than `src.len()`.
    pub fn write_at(&mut self, mut offset: usize, src: &[u8]) -> usize {
        let mut total = 0;

        for slice in self.parts.iter_mut() {
            let slice = slice.as_mut();
            if offset < slice.len() {
                let available = slice.len() - offset;
                let to_copy = std::cmp::min(src.len() - total, available);
                slice[offset..offset + to_copy].copy_from_slice(&src[total..total + to_copy]);
                total += to_copy;
                offset = 0;
                if total == src.len() {
                    break;
                }
            } else {
                offset -= slice.len();
            }
        }
        total
    }
}

impl<T, K: AllocKind> BufferIO<T, K>
where
    T: AsRef<[u8]>,
{
    /// Reads data from the buffer starting at the specified `offset` into `dst`.
    ///
    /// This method is infallible. It returns the number of bytes successfully read.
    ///
    /// If the specified `offset` is greater than or equal to the total length of the
    /// buffer, `0` bytes will be read.
    ///
    /// If the remaining data in the buffer starting at `offset` is less than the
    /// size of `dst`, a short read occurs: only the available bytes are copied,
    /// and the returned value will be less than `dst.len()`.
    pub fn read_at(&self, mut offset: usize, dst: &mut [u8]) -> usize {
        let mut total = 0;

        for slice in self.parts.iter() {
            let slice = slice.as_ref();
            if offset < slice.len() {
                let available = slice.len() - offset;
                let to_copy = std::cmp::min(dst.len() - total, available);
                dst[total..total + to_copy].copy_from_slice(&slice[offset..offset + to_copy]);
                total += to_copy;
                offset = 0;
                if total == dst.len() {
                    break;
                }
            } else {
                offset -= slice.len();
            }
        }
        total
    }
}

/// A non-empty container that has at most [`netdev::MAX_DESCRIPTOR_CHAIN`] elements.
struct Chained<T> {
    storage: [MaybeUninit<T>; netdev::MAX_DESCRIPTOR_CHAIN as usize],
    len: ChainLength,
}

impl<T> Deref for Chained<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        // Safety: `self.storage[..self.len]` is already initialized.
        unsafe { std::mem::transmute(&self.storage[..self.len.into()]) }
    }
}

impl<T> DerefMut for Chained<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: `self.storage[..self.len]` is already initialized.
        unsafe { std::mem::transmute(&mut self.storage[..self.len.into()]) }
    }
}

impl<T> Drop for Chained<T> {
    fn drop(&mut self) {
        // Safety: `self.deref_mut()` is a slice of all initialized elements.
        unsafe {
            std::ptr::drop_in_place(self.deref_mut());
        }
    }
}

impl<T: Debug> Debug for Chained<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T> Chained<T> {
    #[allow(clippy::uninit_assumed_init)]
    fn empty() -> Self {
        // Create an uninitialized array of `MaybeUninit`. The `assume_init` is
        // safe because the type we are claiming to have initialized here is a
        // bunch of `MaybeUninit`s, which do not require initialization.
        // TODO(https://fxbug.dev/42160423): use MaybeUninit::uninit_array once it
        // is stablized.
        // https://doc.rust-lang.org/std/mem/union.MaybeUninit.html#method.uninit_array
        Self { storage: unsafe { MaybeUninit::uninit().assume_init() }, len: ChainLength::ZERO }
    }
}

impl<T> FromIterator<T> for Chained<T> {
    /// # Panics
    ///
    /// if the iterator can yield more than MAX_DESCRIPTOR_CHAIN elements.
    fn from_iter<I: IntoIterator<Item = T>>(elements: I) -> Self {
        let mut result = Self::empty();
        let mut len = 0u8;
        for (idx, e) in elements.into_iter().enumerate() {
            result.storage[idx] = MaybeUninit::new(e);
            len += 1;
        }
        // `len` can not be larger than `MAX_DESCRIPTOR_CHAIN`, otherwise we can't
        // get here due to the bound checks on `result.storage`.
        result.len = ChainLength::try_from(len).unwrap();
        result
    }
}

impl<T> IntoIterator for Chained<T> {
    type Item = T;
    type IntoIter = ChainedIter<T>;

    fn into_iter(mut self) -> Self::IntoIter {
        let len = self.len;
        self.len = ChainLength::ZERO;
        // Safety: we have reset the length to zero, it is now safe to move out
        // the values and set them to be uninitialized. The `assume_init` is
        // safe because the type we are claiming to have initialized here is a
        // bunch of `MaybeUninit`s, which do not require initialization.
        // TODO(https://fxbug.dev/42160423): use MaybeUninit::uninit_array once it
        // is stablized.
        #[allow(clippy::uninit_assumed_init)]
        let storage =
            std::mem::replace(&mut self.storage, unsafe { MaybeUninit::uninit().assume_init() });
        ChainedIter { storage, len, consumed: 0 }
    }
}

struct ChainedIter<T> {
    storage: [MaybeUninit<T>; netdev::MAX_DESCRIPTOR_CHAIN as usize],
    len: ChainLength,
    consumed: u8,
}

impl<T> Iterator for ChainedIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.consumed < self.len.get() {
            // Safety: it is safe now to replace that slot with an uninitialized
            // value because we will advance consumed by 1.
            let value = unsafe {
                std::mem::replace(
                    &mut self.storage[usize::from(self.consumed)],
                    MaybeUninit::uninit(),
                )
                .assume_init()
            };
            self.consumed += 1;
            Some(value)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = usize::from(self.len.get() - self.consumed);
        (len, Some(len))
    }
}

impl<T> ExactSizeIterator for ChainedIter<T> {}

impl<T> Drop for ChainedIter<T> {
    fn drop(&mut self) {
        // Safety: `self.storage[self.consumed..self.len]` is initialized.
        unsafe {
            std::ptr::drop_in_place(std::mem::transmute::<_, &mut [T]>(
                &mut self.storage[self.consumed.into()..self.len.into()],
            ));
        }
    }
}

/// Guards the allocated descriptors; they will be freed when dropped.
pub(in crate::session) struct AllocGuard<K: AllocKind> {
    descs: Chained<DescId<K>>,
    pool: Arc<Pool>,
}

impl<K: AllocKind> Debug for AllocGuard<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { descs, pool: _ } = self;
        f.debug_struct("AllocGuard").field("descs", descs).finish()
    }
}

impl<K: AllocKind> AllocGuard<K> {
    fn new(descs: Chained<DescId<K>>, pool: Arc<Pool>) -> Self {
        Self { descs, pool }
    }

    /// Iterates over references to the descriptors.
    fn descriptors(&self) -> impl Iterator<Item = DescRef<'_, K>> + '_ {
        self.descs.iter().map(move |desc| self.pool.descriptors.borrow(desc))
    }

    /// Iterates over mutable references to the descriptors.
    fn descriptors_mut(&mut self) -> impl Iterator<Item = DescRefMut<'_, K>> + '_ {
        let descriptors = &self.pool.descriptors;
        self.descs.iter_mut().map(move |desc| descriptors.borrow_mut(desc))
    }

    /// Gets a reference to the head descriptor.
    fn descriptor(&self) -> DescRef<'_, K> {
        self.descriptors().next().expect("descriptors must not be empty")
    }

    /// Gets a mutable reference to the head descriptor.
    fn descriptor_mut(&mut self) -> DescRefMut<'_, K> {
        self.descriptors_mut().next().expect("descriptors must not be empty")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DescriptorLayout {
    chain_length: ChainLength,
    head_length: u16,
    data_length: u32,
    tail_length: u16,
}

impl AllocGuard<Tx> {
    /// Calculates the layout for each descriptor in this allocation chain.
    ///
    /// The layouts are calculated to satisfy the requested `target_len`, while
    /// ensuring the session's `min_tx_head` and `min_tx_tail` requirements are
    /// met.
    ///
    /// Returns `Err(Error::TxLength)` if the requirements cannot be met (e.g. if the
    /// required tail padding overflows `u16`).
    fn calculate_descriptor_layouts(&self, target_len: usize) -> Result<Chained<DescriptorLayout>> {
        let len = self.len();
        let BufferLayout { min_tx_head, min_tx_tail, length: buffer_length, .. } =
            self.pool.buffer_layout;

        let mut remaining_target = target_len;
        (0..len)
            .rev()
            .map(|clen| {
                let chain_length = ChainLength::try_from(clen).unwrap();
                let head_length = if clen + 1 == len { min_tx_head } else { 0 };
                let mut tail_length = if clen == 0 { min_tx_tail } else { 0 };

                // head_length and tail_length. The check was done when the config
                // for pool was created, so the subtraction won't overflow.
                let available_bytes = u32::try_from(
                    buffer_length - usize::from(head_length) - usize::from(tail_length),
                )
                .unwrap();

                let data_length = match u32::try_from(remaining_target) {
                    Ok(target) => {
                        if target < available_bytes {
                            // The target bytes are less than what is available,
                            // we need to put the excess in the tail so that the
                            // user cannot write more than they requested (or padded).
                            let excess = available_bytes - target;
                            tail_length = u16::try_from(excess)
                                .ok_checked::<TryFromIntError>()
                                .and_then(|tail_adjustment| {
                                    tail_length.checked_add(tail_adjustment)
                                })
                                .ok_or(Error::TxLength)?;
                        }
                        target.min(available_bytes)
                    }
                    Err(TryFromIntError { .. }) => available_bytes,
                };

                let data_length_usize =
                    usize::try_from(data_length).expect("u32 must fit in a usize");
                remaining_target = remaining_target.saturating_sub(data_length_usize);

                Ok::<_, Error>(DescriptorLayout {
                    chain_length,
                    head_length,
                    data_length,
                    tail_length,
                })
            })
            .collect()
    }

    /// Initializes descriptors of a tx allocation.
    ///
    /// We choose to enforce and satisfy the `min_tx_data` layout requirement
    /// (imposed by the device/driver) immediately during buffer allocation and
    /// initialization here.
    ///
    /// Consequently, the allocated buffer's capacity (`target_len`) may be
    /// larger than the `requested_bytes` if `requested_bytes` is smaller than
    /// `min_tx_data`.
    ///
    /// While this means we might spend CPU cycles zero-padding buffers that are
    /// subsequently dropped without being sent (a rare occurrence in typical
    /// usage), this guarantees that buffer is always suitable for sending. This
    /// also makes the transmit path (`Session::send`) infallible.
    fn init(&mut self, requested_bytes: usize) -> Result<()> {
        let min_tx_data = self.pool.buffer_layout.min_tx_data;
        let target_len = requested_bytes.max(usize::from(min_tx_data));
        let layouts = self.calculate_descriptor_layouts(target_len)?;

        let mut remaining_requested = requested_bytes;

        for (desc_id, DescriptorLayout { chain_length, head_length, data_length, tail_length }) in
            self.descs.iter_mut().zip(layouts)
        {
            // Initialize the descriptor.
            {
                let mut descriptor = self.pool.descriptors.borrow_mut(desc_id);
                descriptor.initialize(chain_length, head_length, data_length, tail_length);
            }

            let data_length_usize = usize::try_from(data_length).expect("u32 must fit in a usize");
            let requested_in_part = std::cmp::min(remaining_requested, data_length_usize);
            let pad_in_part = data_length_usize - requested_in_part;

            // Zero-pad any excess capacity in this buffer part that was allocated
            // to satisfy the `min_tx_data` layout requirement but not requested by
            // the caller.
            //
            // We decided to pad the buffer on initialization because the lazy commit
            // model can only avoid padding for the following 2 cases:
            // 1) User only allocates but never sends.
            // 2) User writes past their requested size and meets the min_tx_data
            //    requirement.
            // Both should be uncommon, and in case 2) we can fix the client by
            // requesting a larger size to avoid padding.
            if pad_in_part > 0 {
                let slice = self.pool.get_slice_mut(desc_id);
                slice[requested_in_part..requested_in_part + pad_in_part].fill(0);
            }

            remaining_requested -= requested_in_part;
        }
        Ok(())
    }
}

impl<K: AllocKind> Drop for AllocGuard<K> {
    fn drop(&mut self) {
        if self.is_empty() {
            return;
        }
        K::free(private::Allocation(self));
    }
}

impl<K: AllocKind> Deref for AllocGuard<K> {
    type Target = [DescId<K>];

    fn deref(&self) -> &Self::Target {
        self.descs.deref()
    }
}

impl<K: AllocKind> From<AllocGuard<K>> for Buffer<K> {
    fn from(alloc: AllocGuard<K>) -> Self {
        Self { alloc }
    }
}

impl<T, K: AllocKind> Read for BufferIO<T, K>
where
    T: AsRef<[u8]>,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read_len = self.read_at(self.pos, buf);
        self.pos += read_len;
        Ok(read_len)
    }
}

impl<T> Write for BufferIO<T, Tx>
where
    T: AsMut<[u8]>,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let write_len = self.write_at(self.pos, buf);
        self.pos += write_len;
        Ok(write_len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<T, K: AllocKind> Seek for BufferIO<T, K> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let pos = match pos {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) => {
                let end = i64::try_from(self.len).unwrap();
                u64::try_from(end.wrapping_add(offset)).unwrap()
            }
            SeekFrom::Current(offset) => {
                let current = i64::try_from(self.pos).map_err(|TryFromIntError { .. }| {
                    std::io::Error::from(std::io::ErrorKind::InvalidInput)
                })?;
                u64::try_from(current.wrapping_add(offset)).unwrap()
            }
        };
        self.pos = usize::try_from(pos).map_err(|TryFromIntError { .. }| {
            std::io::Error::from(std::io::ErrorKind::InvalidInput)
        })?;
        Ok(pos)
    }
}

/// A pending tx allocation request.
struct TxAllocReq {
    sender: Sender<AllocGuard<Tx>>,
    size: ChainLength,
}

impl TxAllocReq {
    fn new(size: ChainLength) -> (Self, Receiver<AllocGuard<Tx>>) {
        let (sender, receiver) = channel();
        (TxAllocReq { sender, size }, receiver)
    }

    /// Fulfills the pending request with an `AllocGuard`.
    ///
    /// If the request is already closed, the guard is simply dropped and
    /// returned to the queue.
    ///
    /// `fulfill` must *not* be called when the `guard`'s pool is holding the tx
    /// lock, since we may deadlock/panic upon the double tx lock acquisition.
    fn fulfill(self, guard: AllocGuard<Tx>) {
        let Self { sender, size: _ } = self;
        match sender.send(guard) {
            Ok(()) => (),
            Err(guard) => {
                // It's ok to just drop the guard here, it'll be returned to the
                // pool.
                drop(guard);
            }
        }
    }
}

/// A module for sealed traits so that the user of this crate can not implement
/// [`AllocKind`] for anything than [`Rx`] and [`Tx`].
mod private {
    use super::{AllocKind, Rx, Tx};
    pub trait Sealed: 'static + Sized {}
    impl Sealed for Rx {}
    impl Sealed for Tx {}

    // We can't leak a private type in a public trait, create an opaque private
    // new type for &mut super::AllocGuard so that we can mention it in the
    // AllocKind trait.
    pub struct Allocation<'a, K: AllocKind>(pub(super) &'a mut super::AllocGuard<K>);
}

/// An allocation can have two kinds, this trait provides a way to project a
/// type ([`Rx`] or [`Tx`]) into a value.
pub trait AllocKind: private::Sealed {
    /// The reflected value of Self.
    const REFL: AllocKindRefl;

    /// frees an allocation of the given kind.
    fn free(alloc: private::Allocation<'_, Self>);
}

/// A tag to related types for Tx allocations.
pub enum Tx {}
/// A tag to related types for Rx allocations.
pub enum Rx {}

/// The reflected value that allows inspection on an [`AllocKind`] type.
pub enum AllocKindRefl {
    Tx,
    Rx,
}

impl AllocKindRefl {
    pub(in crate::session) fn as_str(&self) -> &'static str {
        match self {
            AllocKindRefl::Tx => "Tx",
            AllocKindRefl::Rx => "Rx",
        }
    }
}

impl AllocKind for Tx {
    const REFL: AllocKindRefl = AllocKindRefl::Tx;

    fn free(alloc: private::Allocation<'_, Self>) {
        let private::Allocation(AllocGuard { pool, descs }) = alloc;
        pool.free_tx(std::mem::replace(descs, Chained::empty()));
    }
}

impl AllocKind for Rx {
    const REFL: AllocKindRefl = AllocKindRefl::Rx;

    fn free(alloc: private::Allocation<'_, Self>) {
        let private::Allocation(AllocGuard { pool, descs }) = alloc;
        pool.free_rx(std::mem::replace(descs, Chained::empty()));
        pool.rx_leases.rx_complete();
    }
}

/// An extracted struct containing state pertaining to watching rx leases.
pub(in crate::session) struct RxLeaseHandlingState {
    can_watch_rx_leases: AtomicBool,
    /// Keeps a rolling counter of received rx frames MINUS the target frame
    /// number of the current outstanding lease.
    ///
    /// When no leases are pending (via [`RxLeaseWatcher::wait_until`]),
    /// then this matches exactly the number of received frames.
    ///
    /// Otherwise, the lease is currently waiting for remaining `u64::MAX -
    /// rx_Frame_counter` frames. The logic depends on `AtomicU64` wrapping
    /// around as part of completing rx buffers.
    rx_frame_counter: AtomicU64,
    rx_lease_waker: AtomicWaker,
}

impl RxLeaseHandlingState {
    fn new_with_flags(flags: netdev::SessionFlags) -> Self {
        Self::new_with_enabled(flags.contains(netdev::SessionFlags::RECEIVE_RX_POWER_LEASES))
    }

    fn new_with_enabled(enabled: bool) -> Self {
        Self {
            can_watch_rx_leases: AtomicBool::new(enabled),
            rx_frame_counter: AtomicU64::new(0),
            rx_lease_waker: AtomicWaker::new(),
        }
    }

    /// Increments the total receive frame counter and possibly wakes up a
    /// waiting lease yielder.
    fn rx_complete(&self) {
        let Self { can_watch_rx_leases: _, rx_frame_counter, rx_lease_waker } = self;
        let prev = rx_frame_counter.fetch_add(1, atomic::Ordering::SeqCst);

        // See wait_until for details. We need to hit a waker whenever our add
        // wrapped the u64 back around to 0.
        if prev == u64::MAX {
            rx_lease_waker.wake();
        }
    }
}

/// A trait allowing [`RxLeaseWatcher`] to be agnostic over how to get an
/// [`RxLeaseHandlingState`].
pub(in crate::session) trait RxLeaseHandlingStateContainer {
    fn lease_handling_state(&self) -> &RxLeaseHandlingState;
}

impl<T: Borrow<RxLeaseHandlingState>> RxLeaseHandlingStateContainer for T {
    fn lease_handling_state(&self) -> &RxLeaseHandlingState {
        self.borrow()
    }
}

impl RxLeaseHandlingStateContainer for Arc<Pool> {
    fn lease_handling_state(&self) -> &RxLeaseHandlingState {
        &self.rx_leases
    }
}

/// A type safe-wrapper around a single lease watcher per `Pool`.
pub(in crate::session) struct RxLeaseWatcher<T> {
    state: T,
}

impl<T: RxLeaseHandlingStateContainer> RxLeaseWatcher<T> {
    /// Creates a new lease watcher.
    ///
    /// # Panics
    ///
    /// Panics if an [`RxLeaseWatcher`] has already been created for the given
    /// pool or the pool was not configured for it.
    pub(in crate::session) fn new(state: T) -> Self {
        assert!(
            state.lease_handling_state().can_watch_rx_leases.swap(false, atomic::Ordering::SeqCst),
            "can't watch rx leases"
        );
        Self { state }
    }

    /// Called by sessions to wait until `hold_until_frame` is fulfilled to
    /// yield leases out.
    ///
    /// Blocks until `hold_until_frame`-th rx buffer has been released.
    ///
    /// Note that this method takes `&mut self` because only one
    /// [`RxLeaseWatcher`] may be created by lease handling state, and exclusive
    /// access to it is required to watch lease completion.
    pub(in crate::session) async fn wait_until(&mut self, hold_until_frame: u64) {
        // A note about wrap-arounds.
        //
        // We're assuming the frame counter will never wrap around for
        // correctness here. This should be fine, even assuming a packet
        // rate of 1 million pps it'd take almost 600k years for this counter
        // to wrap around:
        // - 2^64 / 1e6 / 60 / 60 / 24 / 365 ~ 584e3.

        let RxLeaseHandlingState { can_watch_rx_leases: _, rx_frame_counter, rx_lease_waker } =
            self.state.lease_handling_state();

        let prev = rx_frame_counter.fetch_sub(hold_until_frame, atomic::Ordering::SeqCst);
        // After having subtracted the waiting value we *must always restore the
        // value* on return, even if the future is not polled to completion.
        let _guard = scopeguard::guard((), |()| {
            let _: u64 = rx_frame_counter.fetch_add(hold_until_frame, atomic::Ordering::SeqCst);
        });

        // Lease is ready to be fulfilled.
        if prev >= hold_until_frame {
            return;
        }
        // Threshold is a wrapped around subtraction. So now we must wait
        // until the read value from the atomic is LESS THAN the threshold.
        let threshold = prev.wrapping_sub(hold_until_frame);
        futures::future::poll_fn(|cx| {
            let v = rx_frame_counter.load(atomic::Ordering::SeqCst);
            if v < threshold {
                return Poll::Ready(());
            }
            rx_lease_waker.register(cx.waker());
            let v = rx_frame_counter.load(atomic::Ordering::SeqCst);
            if v < threshold {
                return Poll::Ready(());
            }
            Poll::Pending
        })
        .await;
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use futures::future::FutureExt;
    use test_case::test_case;

    use std::collections::HashSet;
    use std::num::{NonZeroU16, NonZeroU64, NonZeroUsize};
    use std::pin::pin;
    use std::task::Poll;

    use crate::session::VmoConfig;

    const DEFAULT_MIN_TX_BUFFER_HEAD: u16 = 4;
    const DEFAULT_MIN_TX_BUFFER_TAIL: u16 = 8;
    // Safety: These are safe because none of the values are zero.
    const DEFAULT_BUFFER_LENGTH: NonZeroUsize = NonZeroUsize::new(64).unwrap();
    const DEFAULT_TX_BUFFERS: NonZeroU16 = NonZeroU16::new(8).unwrap();
    const DEFAULT_RX_BUFFERS: NonZeroU16 = NonZeroU16::new(8).unwrap();
    const MAX_BUFFER_BYTES: usize = DEFAULT_BUFFER_LENGTH.get()
        * netdev::MAX_DESCRIPTOR_CHAIN as usize
        - DEFAULT_MIN_TX_BUFFER_HEAD as usize
        - DEFAULT_MIN_TX_BUFFER_TAIL as usize;

    const SENTINEL_BYTE: u8 = 0xab;
    const WRITE_BYTE: u8 = 1;
    const PAD_BYTE: u8 = 0;

    fn default_config() -> Config {
        Config {
            buffer_stride: NonZeroU64::new(DEFAULT_BUFFER_LENGTH.get() as u64).unwrap(),
            rx_vmos: vec![VmoConfig {
                vmo_id: DEFAULT_VMO_ID,
                num_buffers: DEFAULT_RX_BUFFERS.get(),
            }],
            tx_vmos: vec![VmoConfig {
                vmo_id: DEFAULT_VMO_ID,
                num_buffers: DEFAULT_TX_BUFFERS.get(),
            }],
            options: netdev::SessionFlags::empty(),
            buffer_layout: BufferLayout {
                length: DEFAULT_BUFFER_LENGTH.get(),
                min_tx_head: DEFAULT_MIN_TX_BUFFER_HEAD,
                min_tx_tail: DEFAULT_MIN_TX_BUFFER_TAIL,
                min_tx_data: 0,
            },
            buffer_usage_sample_interval: std::time::Duration::from_secs(1),
        }
    }

    impl Pool {
        fn new_test_pool(config: Config) -> (Arc<Self>, zx::Vmo, Vec<zx::Vmo>) {
            let CreatedPool { pool, descriptors_vmo, data_vmos } =
                Pool::new(config).expect("failed to create pool");
            (pool, descriptors_vmo, data_vmos)
        }

        fn new_test_default() -> Arc<Self> {
            let (pool, _descriptors, _data) = Pool::new_test_pool(default_config());
            pool
        }

        fn tx_alloc_state_lock(&self) -> fuchsia_sync::MutexGuard<'_, TxAllocState> {
            self.tx_alloc_state.lock()
        }

        async fn alloc_tx_checked(self: &Arc<Self>, n: u8) -> AllocGuard<Tx> {
            self.alloc_tx(ChainLength::try_from(n).expect("failed to convert to chain length"))
                .await
        }

        fn alloc_tx_now_or_never(self: &Arc<Self>, n: u8) -> Option<AllocGuard<Tx>> {
            self.alloc_tx_checked(n).now_or_never()
        }

        fn alloc_tx_all(self: &Arc<Self>, n: u8) -> Vec<AllocGuard<Tx>> {
            std::iter::from_fn(|| self.alloc_tx_now_or_never(n)).collect()
        }

        fn alloc_tx_buffer_now_or_never(self: &Arc<Self>, num_bytes: usize) -> Option<Buffer<Tx>> {
            self.alloc_tx_buffer(num_bytes)
                .now_or_never()
                .transpose()
                .expect("invalid arguments for alloc_tx_buffer")
        }

        fn set_min_tx_buffer_length(self: &mut Arc<Self>, length: usize) {
            Arc::get_mut(self).unwrap().buffer_layout.min_tx_data = length;
        }

        fn fill_sentinel_bytes(&mut self) {
            // Safety: We have mut reference to Pool, so we get to modify the
            // VMO pointed by self.base.
            let bytes = *self.vmo_offsets.last().unwrap();
            unsafe { std::ptr::write_bytes(self.base.as_ptr(), SENTINEL_BYTE, bytes) };
        }
    }

    impl Buffer<Tx> {
        // Write a byte at offset, the result buffer should be pad_size long, with
        // 0..offset being the SENTINEL_BYTE, offset being the WRITE_BYTE and the
        // rest being PAD_BYTE.
        fn check_write_and_pad(&mut self, offset: usize, pad_size: usize) {
            {
                let mut io = self.io_mut();
                assert_eq!(io.write_at(offset, &[WRITE_BYTE][..]), 1);
            }
            assert_eq!(self.len(), pad_size);
            // An arbitrary value that is not SENTINAL/WRITE/PAD_BYTE so that
            // we can make sure the write really happened.
            const INIT_BYTE: u8 = 42;
            let mut read_buf = vec![INIT_BYTE; pad_size];
            assert_eq!(self.io().read_at(0, &mut read_buf[..]), read_buf.len());
            for (idx, byte) in read_buf.iter().enumerate() {
                if idx < offset {
                    assert_eq!(*byte, SENTINEL_BYTE);
                } else if idx == offset {
                    assert_eq!(*byte, WRITE_BYTE);
                } else {
                    assert_eq!(*byte, PAD_BYTE);
                }
            }
        }
    }

    impl<K, I, T> PartialEq<T> for Chained<DescId<K>>
    where
        K: AllocKind,
        I: ExactSizeIterator<Item = u16>,
        T: Copy + IntoIterator<IntoIter = I>,
    {
        fn eq(&self, other: &T) -> bool {
            let iter = other.into_iter();
            if usize::from(self.len) != iter.len() {
                return false;
            }
            self.iter().zip(iter).all(|(l, r)| l.get() == r)
        }
    }

    impl Debug for TxAllocReq {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let TxAllocReq { sender: _, size } = self;
            f.debug_struct("TxAllocReq").field("size", &size).finish_non_exhaustive()
        }
    }

    #[test]
    fn alloc_tx_distinct() {
        let pool = Pool::new_test_default();
        let allocated = pool.alloc_tx_all(1);
        assert_eq!(allocated.len(), usize::from(DEFAULT_TX_BUFFERS.get()));
        let distinct = allocated
            .iter()
            .map(|alloc| {
                assert_eq!(alloc.descs.len(), 1);
                alloc.descs[0].get()
            })
            .collect::<HashSet<u16>>();
        assert_eq!(allocated.len(), distinct.len());
    }

    #[test]
    fn alloc_tx_free_len() {
        let pool = Pool::new_test_default();
        {
            let allocated = pool.alloc_tx_all(2);
            assert_eq!(
                allocated.iter().fold(0, |acc, a| { acc + a.descs.len() }),
                usize::from(DEFAULT_TX_BUFFERS.get())
            );
            assert_eq!(pool.tx_alloc_state_lock().free_lists[0].free, 0);
        }
        assert_eq!(pool.tx_alloc_state_lock().free_lists[0].free, DEFAULT_TX_BUFFERS.get());
    }

    #[test]
    fn alloc_tx_chain() {
        let pool = Pool::new_test_default();
        let allocated = pool.alloc_tx_all(3);
        assert_eq!(allocated.len(), usize::from(DEFAULT_TX_BUFFERS.get()) / 3);
        assert_matches!(pool.alloc_tx_now_or_never(3), None);
        assert_matches!(pool.alloc_tx_now_or_never(2), Some(a) if a.descs.len() == 2);
    }

    #[test]
    fn alloc_tx_many() {
        let pool = Pool::new_test_default();
        let data_len = u32::try_from(DEFAULT_BUFFER_LENGTH.get()).unwrap()
            - u32::from(DEFAULT_MIN_TX_BUFFER_HEAD)
            - u32::from(DEFAULT_MIN_TX_BUFFER_TAIL);
        let data_len = usize::try_from(data_len).unwrap();
        let mut buffers = pool
            .alloc_tx_buffers(data_len)
            .now_or_never()
            .expect("failed to alloc")
            .unwrap()
            // Collect into a vec so we keep the buffers alive, otherwise they
            // are immediately returned to the pool.
            .collect::<Result<Vec<_>>>()
            .expect("buffer error");
        assert_eq!(buffers.len(), usize::from(DEFAULT_TX_BUFFERS.get()));

        // We have all the buffers, which means allocating more should not
        // resolve.
        assert!(pool.alloc_tx_buffers(data_len).now_or_never().is_none());

        // If we release a single buffer we should be able to retrieve it again.
        assert_matches!(buffers.pop(), Some(_));
        let mut more_buffers =
            pool.alloc_tx_buffers(data_len).now_or_never().expect("failed to alloc").unwrap();
        let buffer = assert_matches!(more_buffers.next(), Some(Ok(b)) => b);
        assert_matches!(more_buffers.next(), None);
        // The iterator is fused, so None is yielded even after dropping the
        // buffer.
        drop(buffer);
        assert_matches!(more_buffers.next(), None);
    }

    #[test]
    fn alloc_tx_after_free() {
        let pool = Pool::new_test_default();
        let mut allocated = pool.alloc_tx_all(1);
        assert_matches!(pool.alloc_tx_now_or_never(2), None);
        {
            let _drained = allocated.drain(..2);
        }
        assert_matches!(pool.alloc_tx_now_or_never(2), Some(a) if a.descs.len() == 2);
    }

    #[test]
    fn blocking_alloc_tx() {
        let mut executor = fasync::TestExecutor::new();
        let pool = Pool::new_test_default();
        let mut allocated = pool.alloc_tx_all(1);
        let alloc_fut = pool.alloc_tx_checked(1);
        let mut alloc_fut = pin!(alloc_fut);
        // The allocation should block.
        assert_matches!(executor.run_until_stalled(&mut alloc_fut), Poll::Pending);
        // And the allocation request should be queued.
        assert!(!pool.tx_alloc_state_lock().requests.is_empty());
        let freed = allocated
            .pop()
            .expect("no fulfulled allocations")
            .iter()
            .map(|x| x.get())
            .collect::<Chained<_>>();
        let same_as_freed =
            |descs: &Chained<DescId<Tx>>| descs.iter().map(|x| x.get()).eq(freed.iter().copied());
        // Now the task should be able to continue.
        assert_matches!(
            &executor.run_until_stalled(&mut alloc_fut),
            Poll::Ready(AllocGuard{ descs, pool: _ }) if same_as_freed(descs)
        );
        // And the queued request should now be removed.
        assert!(pool.tx_alloc_state_lock().requests.is_empty());
    }

    #[test]
    fn blocking_alloc_tx_cancel_before_free() {
        let mut executor = fasync::TestExecutor::new();
        let pool = Pool::new_test_default();
        let mut allocated = pool.alloc_tx_all(1);
        {
            let alloc_fut = pool.alloc_tx_checked(1);
            let mut alloc_fut = pin!(alloc_fut);
            assert_matches!(executor.run_until_stalled(&mut alloc_fut), Poll::Pending);
            assert_matches!(
                pool.tx_alloc_state_lock().requests.as_slices(),
                (&[ref req1, ref req2], &[]) if req1.size.get() == 1 && req2.size.get() == 1
            );
        }
        assert_matches!(
            allocated.pop(),
            Some(AllocGuard { ref descs, pool: ref p })
                if descs == &[DEFAULT_TX_BUFFERS.get() - 1] && Arc::ptr_eq(p, &pool)
        );
        let state = pool.tx_alloc_state_lock();
        assert_eq!(state.free_lists[0].free, 1);
        assert!(state.requests.is_empty());
    }

    #[test]
    fn blocking_alloc_tx_cancel_after_free() {
        let mut executor = fasync::TestExecutor::new();
        let pool = Pool::new_test_default();
        let mut allocated = pool.alloc_tx_all(1);
        {
            let alloc_fut = pool.alloc_tx_checked(1);
            let mut alloc_fut = pin!(alloc_fut);
            assert_matches!(executor.run_until_stalled(&mut alloc_fut), Poll::Pending);
            assert_matches!(
                pool.tx_alloc_state_lock().requests.as_slices(),
                (&[ref req1, ref req2], &[]) if req1.size.get() == 1 && req2.size.get() == 1
            );
            assert_matches!(
                allocated.pop(),
                Some(AllocGuard { ref descs, pool: ref p })
                    if descs == &[DEFAULT_TX_BUFFERS.get() - 1] && Arc::ptr_eq(p, &pool)
            );
        }
        let state = pool.tx_alloc_state_lock();
        assert_eq!(state.free_lists[0].free, 1);
        assert!(state.requests.is_empty());
    }

    #[test]
    fn multiple_blocking_alloc_tx_fulfill_order() {
        const TASKS_TOTAL: usize = 3;
        let mut executor = fasync::TestExecutor::new();
        let pool = Pool::new_test_default();
        let mut allocated = pool.alloc_tx_all(1);
        let mut alloc_futs = (1..=TASKS_TOTAL)
            .rev()
            .map(|x| {
                let pool = pool.clone();
                (x, Box::pin(async move { pool.alloc_tx_checked(x.try_into().unwrap()).await }))
            })
            .collect::<Vec<_>>();

        for (idx, (req_size, task)) in alloc_futs.iter_mut().enumerate() {
            assert_matches!(executor.run_until_stalled(task), Poll::Pending);
            // assert that the tasks are sorted decreasing on the requested size.
            assert_eq!(idx + *req_size, TASKS_TOTAL);
        }
        {
            let state = pool.tx_alloc_state_lock();
            // The first pending request was introduced by `alloc_tx_all`.
            assert_eq!(state.requests.len(), TASKS_TOTAL + 1);
            let mut requests = state.requests.iter();
            // It should already be cancelled because the requesting future is
            // already dropped.
            assert!(requests.next().unwrap().sender.is_canceled());
            // The rest of the requests must not be cancelled.
            assert!(requests.all(|req| !req.sender.is_canceled()))
        }

        let mut to_free = Vec::new();
        let mut freed = 0;
        for free_size in (1..=TASKS_TOTAL).rev() {
            let (_req_size, mut task) = alloc_futs.remove(0);
            for _ in 1..free_size {
                freed += 1;
                assert_matches!(
                    allocated.pop(),
                    Some(AllocGuard { ref descs, pool: ref p })
                        if descs == &[DEFAULT_TX_BUFFERS.get() - freed] && Arc::ptr_eq(p, &pool)
                );
                assert_matches!(executor.run_until_stalled(&mut task), Poll::Pending);
            }
            freed += 1;
            assert_matches!(
                allocated.pop(),
                Some(AllocGuard { ref descs, pool: ref p })
                    if descs == &[DEFAULT_TX_BUFFERS.get() - freed] && Arc::ptr_eq(p, &pool)
            );
            match executor.run_until_stalled(&mut task) {
                Poll::Ready(alloc) => {
                    assert_eq!(alloc.len(), free_size);
                    // Don't return the allocation to the pool now.
                    to_free.push(alloc);
                }
                Poll::Pending => panic!("The request should be fulfilled"),
            }
            // The rest of requests can not be fulfilled.
            for (_req_size, task) in alloc_futs.iter_mut() {
                assert_matches!(executor.run_until_stalled(task), Poll::Pending);
            }
        }
        assert!(pool.tx_alloc_state_lock().requests.is_empty());
    }

    #[test]
    fn singleton_tx_layout() {
        let pool = Pool::new_test_default();
        let buffers = std::iter::from_fn(|| {
            let data_len = u32::try_from(DEFAULT_BUFFER_LENGTH.get()).unwrap()
                - u32::from(DEFAULT_MIN_TX_BUFFER_HEAD)
                - u32::from(DEFAULT_MIN_TX_BUFFER_TAIL);
            pool.alloc_tx_buffer_now_or_never(usize::try_from(data_len).unwrap()).map(|buffer| {
                assert_eq!(buffer.alloc.descriptors().count(), 1);
                let offset = u64::try_from(DEFAULT_BUFFER_LENGTH.get()).unwrap()
                    * u64::from(buffer.alloc[0].get());
                {
                    let descriptor = buffer.alloc.descriptor();
                    assert_matches!(descriptor.chain_length(), Ok(ChainLength::ZERO));
                    assert_eq!(descriptor.head_length(), DEFAULT_MIN_TX_BUFFER_HEAD);
                    assert_eq!(descriptor.tail_length(), DEFAULT_MIN_TX_BUFFER_TAIL);
                    assert_eq!(descriptor.data_length(), data_len);
                    assert_eq!(descriptor.offset(), offset);
                }

                {
                    let mut slices = buffer.parts();
                    let slice = slices.next().expect("should have one slice");
                    assert_matches!(slices.next(), None);
                    assert_eq!(slice.len(), usize::try_from(data_len).unwrap());
                    assert_eq!(
                        slice.as_ptr(),
                        pool.base.as_ptr().wrapping_add(
                            usize::try_from(offset).unwrap()
                                + usize::from(DEFAULT_MIN_TX_BUFFER_HEAD),
                        )
                    );
                }
                buffer
            })
        })
        .collect::<Vec<_>>();
        assert_eq!(buffers.len(), usize::from(DEFAULT_TX_BUFFERS.get()));
    }

    #[test]
    fn chained_tx_layout() {
        let pool = Pool::new_test_default();
        let alloc_len = 4 * DEFAULT_BUFFER_LENGTH.get()
            - usize::from(DEFAULT_MIN_TX_BUFFER_HEAD)
            - usize::from(DEFAULT_MIN_TX_BUFFER_TAIL);
        let buffers = std::iter::from_fn(|| {
            pool.alloc_tx_buffer_now_or_never(alloc_len).map(|buffer| {
                assert_eq!(buffer.parts().count(), 4);
                for (idx, (descriptor, slice)) in
                    buffer.alloc.descriptors().zip(buffer.parts()).enumerate()
                {
                    let chain_length = ChainLength::try_from(buffer.alloc.len() - idx - 1).unwrap();
                    let head_length = if idx == 0 { DEFAULT_MIN_TX_BUFFER_HEAD } else { 0 };
                    let tail_length = if chain_length == ChainLength::ZERO {
                        DEFAULT_MIN_TX_BUFFER_TAIL
                    } else {
                        0
                    };
                    let data_len = u32::try_from(DEFAULT_BUFFER_LENGTH.get()).unwrap()
                        - u32::from(head_length)
                        - u32::from(tail_length);
                    let offset = u64::try_from(DEFAULT_BUFFER_LENGTH.get()).unwrap()
                        * u64::from(buffer.alloc[idx].get());
                    assert_eq!(descriptor.chain_length().unwrap(), chain_length);
                    assert_eq!(descriptor.head_length(), head_length);
                    assert_eq!(descriptor.tail_length(), tail_length);
                    assert_eq!(descriptor.offset(), offset);
                    assert_eq!(descriptor.data_length(), data_len);
                    if chain_length != ChainLength::ZERO {
                        assert_eq!(descriptor.nxt(), Some(buffer.alloc[idx + 1].get()));
                    }

                    assert_eq!(slice.len(), usize::try_from(data_len).unwrap());
                    assert_eq!(
                        slice.as_ptr(),
                        pool.base.as_ptr().wrapping_add(
                            usize::try_from(offset).unwrap() + usize::from(head_length),
                        )
                    );
                }
                buffer
            })
        })
        .collect::<Vec<_>>();
        assert_eq!(buffers.len(), usize::from(DEFAULT_TX_BUFFERS.get()) / 4);
    }

    #[test]
    fn rx_distinct() {
        let pool = Pool::new_test_default();
        let mut guard = pool.rx_pending.lock();
        let descs = &mut guard.storage;
        assert_eq!(descs.len(), usize::from(DEFAULT_RX_BUFFERS.get()));
        let distinct = descs.iter().map(|desc| desc.get()).collect::<HashSet<u16>>();
        assert_eq!(descs.len(), distinct.len());
    }

    #[test]
    fn alloc_rx_layout() {
        let pool = Pool::new_test_default();
        let mut guard = pool.rx_pending.lock();
        let descs = &mut guard.storage;
        assert_eq!(descs.len(), usize::from(DEFAULT_RX_BUFFERS.get()));
        for desc in descs.iter() {
            let descriptor = pool.descriptors.borrow(desc);
            let offset =
                u64::try_from(DEFAULT_BUFFER_LENGTH.get()).unwrap() * u64::from(desc.get());
            assert_matches!(descriptor.chain_length(), Ok(ChainLength::ZERO));
            assert_eq!(descriptor.head_length(), 0);
            assert_eq!(descriptor.tail_length(), 0);
            assert_eq!(descriptor.offset(), offset);
            assert_eq!(
                descriptor.data_length(),
                u32::try_from(DEFAULT_BUFFER_LENGTH.get()).unwrap()
            );
        }
    }

    #[test]
    fn buffer_read_at_write_at() {
        let pool = Pool::new_test_default();
        let alloc_bytes = DEFAULT_BUFFER_LENGTH.get();
        let mut buffer =
            pool.alloc_tx_buffer_now_or_never(alloc_bytes).expect("failed to allocate");
        // Because we have to accommodate the space for head and tail, there
        // would be 2 parts instead of 1.
        assert_eq!(buffer.parts().count(), 2);
        assert_eq!(buffer.len(), alloc_bytes);
        let write_buf = (0..u8::try_from(DEFAULT_BUFFER_LENGTH.get()).unwrap()).collect::<Vec<_>>();
        assert_eq!(buffer.io_mut().write_at(0, &write_buf[..]), write_buf.len());
        let mut read_buf = [0xff; DEFAULT_BUFFER_LENGTH.get()];
        assert_eq!(buffer.io().read_at(0, &mut read_buf[..]), read_buf.len());
        for (idx, byte) in read_buf.iter().enumerate() {
            assert_eq!(*byte, write_buf[idx]);
        }
    }

    #[test]
    fn buffer_write_at_short() {
        let pool = Pool::new_test_default();
        let alloc_bytes = DEFAULT_BUFFER_LENGTH.get();
        let mut buffer =
            pool.alloc_tx_buffer_now_or_never(alloc_bytes).expect("failed to allocate");
        assert_eq!(buffer.parts().count(), 2);
        assert_eq!(buffer.len(), alloc_bytes);

        let write_buf = vec![WRITE_BYTE; alloc_bytes + 10];

        // Test short write (writing more than buffer capacity)
        assert_eq!(buffer.io_mut().write_at(0, &write_buf[..]), alloc_bytes);

        // Verify short write
        let mut read_buf = vec![0; alloc_bytes];
        assert_eq!(buffer.io().read_at(0, &mut read_buf[..]), alloc_bytes);
        for byte in read_buf.iter() {
            assert_eq!(*byte, WRITE_BYTE);
        }

        // Test write with offset past end
        assert_eq!(buffer.io_mut().write_at(alloc_bytes + 1, &write_buf[..]), 0);

        // Test write with offset inside buffer but src extending past end
        let offset = alloc_bytes / 2;
        let expected_write = alloc_bytes - offset;
        let write_buf = vec![2; alloc_bytes]; // Different byte to distinguish
        assert_eq!(buffer.io_mut().write_at(offset, &write_buf[..]), expected_write);

        // Verify the write
        let mut read_buf = vec![0; alloc_bytes];
        assert_eq!(buffer.io().read_at(0, &mut read_buf[..]), alloc_bytes);
        for (idx, byte) in read_buf.iter().enumerate() {
            if idx < offset {
                assert_eq!(*byte, WRITE_BYTE);
            } else {
                assert_eq!(*byte, 2);
            }
        }
    }

    #[test]
    fn buffer_read_at_short() {
        let pool = Pool::new_test_default();
        let alloc_bytes = DEFAULT_BUFFER_LENGTH.get();
        let mut buffer =
            pool.alloc_tx_buffer_now_or_never(alloc_bytes).expect("failed to allocate");
        assert_eq!(buffer.parts().count(), 2);
        assert_eq!(buffer.len(), alloc_bytes);

        let write_buf = vec![WRITE_BYTE; alloc_bytes];
        assert_eq!(buffer.io_mut().write_at(0, &write_buf[..]), alloc_bytes);

        // Test short read (reading more than buffer capacity)
        let mut read_buf = vec![0xff; alloc_bytes + 10];
        assert_eq!(buffer.io().read_at(0, &mut read_buf[..]), alloc_bytes);
        for (idx, byte) in read_buf.iter().enumerate() {
            if idx < alloc_bytes {
                assert_eq!(*byte, WRITE_BYTE);
            } else {
                assert_eq!(*byte, 0xff);
            }
        }

        // Test read with offset past end
        assert_eq!(buffer.io().read_at(alloc_bytes + 1, &mut read_buf[..]), 0);

        // Test read with offset inside buffer but dst extending past end
        let offset = alloc_bytes / 2;
        let expected_read = alloc_bytes - offset;
        let mut read_buf = vec![0xff; alloc_bytes];
        assert_eq!(buffer.io().read_at(offset, &mut read_buf[..]), expected_read);
        for (idx, byte) in read_buf.iter().enumerate() {
            if idx < expected_read {
                assert_eq!(*byte, WRITE_BYTE);
            } else {
                assert_eq!(*byte, 0xff);
            }
        }
    }

    #[test]
    fn buffer_read_write_seek() {
        let pool = Pool::new_test_default();
        let alloc_bytes = DEFAULT_BUFFER_LENGTH.get();
        let mut buffer =
            pool.alloc_tx_buffer_now_or_never(alloc_bytes).expect("failed to allocate");
        // Because we have to accommodate the space for head and tail, there
        // would be 2 parts instead of 1.
        assert_eq!(buffer.parts().count(), 2);
        assert_eq!(buffer.len(), alloc_bytes);
        let write_buf = (0..u8::try_from(DEFAULT_BUFFER_LENGTH.get()).unwrap()).collect::<Vec<_>>();

        let mut io = buffer.io_mut();

        assert_eq!(io.write(&write_buf[..]).expect("failed to write into buffer"), write_buf.len());
        const SEEK_FROM_END: usize = 64;
        const READ_LEN: usize = 12;
        assert_eq!(
            io.seek(SeekFrom::End(-i64::try_from(SEEK_FROM_END).unwrap())).unwrap(),
            u64::try_from(io.len - SEEK_FROM_END).unwrap()
        );
        let mut read_buf = [0xff; READ_LEN];
        assert_eq!(io.read(&mut read_buf[..]).expect("failed to read from buffer"), read_buf.len());
        assert_eq!(&write_buf[..READ_LEN], &read_buf[..]);
    }

    #[test_case(32; "single buffer part")]
    #[test_case(MAX_BUFFER_BYTES; "multiple buffer parts")]
    fn buffer_pad(pad_size: usize) {
        let mut pool = Pool::new_test_default();
        pool.set_min_tx_buffer_length(pad_size);
        for offset in 0..pad_size {
            Arc::get_mut(&mut pool)
                .expect("there are multiple owners of the underlying VMO")
                .fill_sentinel_bytes();
            let mut buffer =
                pool.alloc_tx_buffer_now_or_never(offset + 1).expect("failed to allocate buffer");
            buffer.check_write_and_pad(offset, pad_size);
        }
    }

    #[test]
    fn buffer_pad_grow() {
        const BUFFER_PARTS: u8 = 3;
        let mut pool = Pool::new_test_default();
        let pad_size = u32::try_from(DEFAULT_BUFFER_LENGTH.get()).unwrap()
            * u32::from(BUFFER_PARTS)
            - u32::from(DEFAULT_MIN_TX_BUFFER_HEAD)
            - u32::from(DEFAULT_MIN_TX_BUFFER_TAIL);
        pool.set_min_tx_buffer_length(pad_size.try_into().unwrap());
        for offset in 0..pad_size - u32::try_from(DEFAULT_BUFFER_LENGTH.get()).unwrap() {
            Arc::get_mut(&mut pool)
                .expect("there are multiple owners of the underlying VMO")
                .fill_sentinel_bytes();
            let mut alloc =
                pool.alloc_tx_now_or_never(BUFFER_PARTS).expect("failed to alloc descriptors");
            alloc
                .init(usize::try_from(offset).unwrap() + 1)
                .expect("head/body/tail sizes are representable with u16/u32/u16");
            let mut buffer = Buffer::try_from(alloc).unwrap();
            buffer.check_write_and_pad(offset.try_into().unwrap(), pad_size.try_into().unwrap());
        }
    }

    #[test_case(  0; "writes at the beginning")]
    #[test_case( 15; "writes in the first part")]
    #[test_case( 75; "writes in the second part")]
    #[test_case(135; "writes in the third part")]
    #[test_case(195; "writes in the last part")]
    fn buffer_used(write_offset: usize) {
        let pool = Pool::new_test_default();
        let mut buffer =
            pool.alloc_tx_buffer_now_or_never(MAX_BUFFER_BYTES).expect("failed to allocate buffer");
        let expected_caps = (0..netdev::MAX_DESCRIPTOR_CHAIN).map(|i| {
            if i == 0 {
                DEFAULT_BUFFER_LENGTH.get() - usize::from(DEFAULT_MIN_TX_BUFFER_HEAD)
            } else if i < netdev::MAX_DESCRIPTOR_CHAIN - 1 {
                DEFAULT_BUFFER_LENGTH.get()
            } else {
                DEFAULT_BUFFER_LENGTH.get() - usize::from(DEFAULT_MIN_TX_BUFFER_TAIL)
            }
        });
        assert_eq!(buffer.alloc.len(), usize::from(netdev::MAX_DESCRIPTOR_CHAIN));
        assert_eq!(buffer.io_mut().write_at(write_offset, &[WRITE_BYTE][..]), 1);
        // The accumulator is Some if we haven't found the part where the byte
        // was written, None if we've already found it.
        assert_eq!(
            buffer.parts().zip(expected_caps).fold(
                Some(write_offset),
                |offset, (slice, expected_cap)| {
                    assert_eq!(slice.len(), expected_cap);
                    match offset {
                        Some(offset) => {
                            if offset >= expected_cap {
                                Some(offset - slice.len())
                            } else {
                                assert_eq!(slice[offset], WRITE_BYTE);
                                None
                            }
                        }
                        None => None,
                    }
                }
            ),
            None
        );
    }

    #[test]
    fn allocate_under_device_minimum() {
        const MIN_TX_DATA: usize = 32;
        const ALLOC_SIZE: usize = 16;
        const WRITE_BYTE: u8 = 0xff;
        const WRITE_SENTINAL_BYTE: u8 = 0xee;
        const READ_SENTINAL_BYTE: u8 = 0xdd;
        let mut config = default_config();
        config.buffer_layout.min_tx_data = MIN_TX_DATA;
        let (pool, _descriptors, _vmo) = Pool::new_test_pool(config);
        for mut buffer in Vec::from_iter(std::iter::from_fn({
            let pool = pool.clone();
            move || pool.alloc_tx_buffer_now_or_never(MIN_TX_DATA)
        })) {
            assert_eq!(
                buffer.io_mut().write_at(0, &[WRITE_SENTINAL_BYTE; MIN_TX_DATA]),
                MIN_TX_DATA
            );
        }
        let mut allocated =
            pool.alloc_tx_buffer_now_or_never(16).expect("failed to allocate buffer");
        assert_eq!(allocated.len(), MIN_TX_DATA);
        const WRITE_BUF_SIZE: usize = MIN_TX_DATA + 1;
        assert_eq!(allocated.io_mut().write_at(0, &[WRITE_BYTE; WRITE_BUF_SIZE]), MIN_TX_DATA);
        assert_eq!(allocated.io_mut().write_at(0, &[WRITE_BYTE; ALLOC_SIZE]), ALLOC_SIZE);
        assert_eq!(allocated.len(), MIN_TX_DATA);
        const READ_BUF_SIZE: usize = MIN_TX_DATA + 1;
        let mut read_buf = [READ_SENTINAL_BYTE; READ_BUF_SIZE];
        assert_eq!(allocated.io().read_at(0, &mut read_buf[..]), MIN_TX_DATA);
        assert_eq!(allocated.io().read_at(0, &mut read_buf[..MIN_TX_DATA]), MIN_TX_DATA);
        assert_eq!(&read_buf[..ALLOC_SIZE], &[WRITE_BYTE; ALLOC_SIZE][..]);
        assert_eq!(&read_buf[ALLOC_SIZE..MIN_TX_DATA], &[WRITE_BYTE; ALLOC_SIZE][..]);
        assert_eq!(&read_buf[MIN_TX_DATA..], &[READ_SENTINAL_BYTE; 1][..]);
    }

    #[test]
    fn invalid_tx_length() {
        let mut config = default_config();
        config.buffer_layout.length = usize::from(u16::MAX) + 2;
        config.buffer_layout.min_tx_head = 0;
        let (pool, _descriptors, _vmo) = Pool::new_test_pool(config);
        assert_matches!(pool.alloc_tx_buffer(1).now_or_never(), Some(Err(Error::TxLength)));
    }

    #[test]
    fn vmo_indices_tracking() {
        let mut config = default_config();
        config.rx_vmos = vec![VmoConfig { vmo_id: 0, num_buffers: DEFAULT_RX_BUFFERS.get() }];
        config.tx_vmos =
            vec![VmoConfig { vmo_id: 1, num_buffers: 2 }, VmoConfig { vmo_id: 2, num_buffers: 4 }];

        let (pool, _descriptors, _vmos) = Pool::new_test_pool(config);

        // Check initial state:
        // - first VMO index with free buffers should be 0
        {
            let state = pool.tx_alloc_state_lock();
            assert_eq!(state.first_available_index, 0);
        }

        // Allocate 1 buffer (takes from VMO 0)
        let alloc1 = pool.alloc_tx_now_or_never(1).expect("alloc 1");
        {
            let state = pool.tx_alloc_state_lock();
            // VMO 0 still has 1 free buffer
            assert_eq!(state.first_available_index, 0);
        }

        // Allocate second buffer from VMO 0
        let alloc2 = pool.alloc_tx_now_or_never(1).expect("alloc 2");
        {
            let state = pool.tx_alloc_state_lock();
            // VMO 0 has 0 free, so VMO 1 is the first with free.
            assert_eq!(state.first_available_index, 1);
        }

        // Allocate 1 buffer from VMO 1
        let alloc3 = pool.alloc_tx_now_or_never(1).expect("alloc 3");
        {
            let state = pool.tx_alloc_state_lock();
            // VMO 1 still has 3 free buffers.
            assert_eq!(state.first_available_index, 1);
        }

        // Allocate remaining buffers from VMO 1 to allocate all buffers
        let _alloc4 = pool.alloc_tx_now_or_never(1).expect("alloc 4");
        let _alloc5 = pool.alloc_tx_now_or_never(1).expect("alloc 5");
        let _alloc6 = pool.alloc_tx_now_or_never(1).expect("alloc 6");
        {
            let state = pool.tx_alloc_state_lock();
            assert_eq!(state.first_available_index, state.free_lists.len());
        }

        // Free the first buffer (belongs to VMO 0)
        drop(alloc1);
        {
            let state = pool.tx_alloc_state_lock();
            // VMO 0 now has 1 free buffer again
            assert_eq!(state.first_available_index, 0);
        }

        // Free the third buffer (belongs to VMO 1)
        drop(alloc3);
        {
            let state = pool.tx_alloc_state_lock();
            // VMO 0 still has 1 free buffer, VMO 1 has 4 free.
            assert_eq!(state.first_available_index, 0);
        }

        // Free the second buffer (belongs to VMO 0)
        drop(alloc2);
        {
            let state = pool.tx_alloc_state_lock();
            // VMO 0 still has 2 free.
            assert_eq!(state.first_available_index, 0);
        }
    }

    #[test]
    fn rx_leases() {
        let mut executor = fuchsia_async::TestExecutor::new();
        let state = RxLeaseHandlingState::new_with_enabled(true);
        let mut watcher = RxLeaseWatcher { state: &state };

        {
            let mut fut = pin!(watcher.wait_until(0));
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Ready(()));
        }
        {
            state.rx_complete();
            let mut fut = pin!(watcher.wait_until(1));
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Ready(()));
        }
        {
            let mut fut = pin!(watcher.wait_until(0));
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Ready(()));
        }
        {
            let mut fut = pin!(watcher.wait_until(3));
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
            state.rx_complete();
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
            state.rx_complete();
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Ready(()));
        }
        // Dropping the wait future without seeing it complete restores the
        // value.
        let counter_before = state.rx_frame_counter.load(atomic::Ordering::SeqCst);
        {
            let mut fut = pin!(watcher.wait_until(10000));
            assert_eq!(executor.run_until_stalled(&mut fut), Poll::Pending);
        }
        let counter_after = state.rx_frame_counter.load(atomic::Ordering::SeqCst);
        assert_eq!(counter_before, counter_after);
    }

    #[test]
    #[should_panic(expected = "slice end")]
    fn get_slice_out_of_bounds_panic() {
        let mut config = default_config();
        config.buffer_layout.length = 64;
        config.buffer_stride = NonZeroU64::new(64).unwrap();
        config.tx_vmos = vec![VmoConfig { vmo_id: DEFAULT_VMO_ID, num_buffers: 1 }];

        let (mut pool, _descriptors_vmo, _data_vmos) = Pool::new_test_pool(config);
        Arc::get_mut(&mut pool)
            .expect("there are multiple owners of the underlying VMO")
            .fill_sentinel_bytes();

        let mut buffer = pool.alloc_tx_buffer_now_or_never(10).expect("failed to alloc buffer");
        {
            let mut desc = buffer.alloc.descriptor_mut();
            desc.set_data_length(4093);
        }
        let _slice = buffer.as_slice_mut();
    }
}
