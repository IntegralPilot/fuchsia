// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::wire;
use anyhow::{anyhow, Context, Error};
use fidl_fuchsia_virtualization_hardware::VirtioMemControlHandle;
use fuchsia_inspect as inspect;
use fuchsia_inspect::{NumericProperty, Property};

use std::io::{Read, Write};
use virtio_device::chain::{ReadableChain, Remaining, WritableChain};
use virtio_device::mem::DriverMem;
use virtio_device::queue::DriverNotify;
use zerocopy::{FromBytes, IntoBytes};

fn read_request<'a, 'b, N: DriverNotify, M: DriverMem>(
    chain: &mut ReadableChain<'a, 'b, N, M>,
) -> Result<wire::VirtioMemRequest, Error> {
    let mut arr = [0; std::mem::size_of::<wire::VirtioMemRequest>()];
    chain.read_exact(&mut arr)?;
    Ok(wire::VirtioMemRequest::read_from_bytes(arr.as_slice()).unwrap())
}

fn write_response<'a, 'b, N: DriverNotify, M: DriverMem>(
    chain: ReadableChain<'a, 'b, N, M>,
    response: wire::VirtioMemResponse,
) -> Result<(), Error> {
    let mut chain = if response.ty.get() == wire::VIRTIO_MEM_RESP_ERROR {
        // Since this is an error, assume it's possible some bytes may not have been read from the
        // readable portion of the chain.
        WritableChain::from_incomplete_readable(chain).context("Failed to get a writable chain")?
    } else {
        WritableChain::from_readable(chain)?
    };

    let Remaining { bytes, .. } = chain.remaining()?;
    if bytes < std::mem::size_of::<wire::VirtioMemResponse>() {
        return Err(anyhow!("Insufficient writable space to write message to the chain"));
    }
    // unwrap here because we already checked if there is space in the writable chain
    chain.write_all(response.as_bytes()).unwrap();
    Ok(())
}

pub trait MemBackend {
    fn decommit_range(&self, offset: u64, size: u64) -> Result<(), zx::Status>;
}

pub struct VmoMemoryBackend {
    vmo: zx::Vmo,
}

impl VmoMemoryBackend {
    pub fn new(vmo: zx::Vmo) -> Self {
        Self { vmo }
    }
}

impl MemBackend for VmoMemoryBackend {
    fn decommit_range(&self, offset: u64, size: u64) -> Result<(), zx::Status> {
        self.vmo.op_range(zx::VmoOp::ZERO, offset, size)
    }
}

pub struct MemBitmap {
    region_addr: u64,
    plugged_block_size: u64,
    region_size: u64,
    map: Vec<u8>,
}

const NUM_BITS_IN_BYTE: usize = 8;

impl MemBitmap {
    pub fn new(region_addr: u64, plugged_block_size: u64, region_size: u64) -> Self {
        Self {
            region_addr,
            plugged_block_size,
            region_size,
            map: vec![
                0;
                (((region_size + plugged_block_size - 1) / plugged_block_size
                    + (NUM_BITS_IN_BYTE as u64 - 1))
                    / NUM_BITS_IN_BYTE as u64) as usize
            ],
        }
    }

    pub fn test_blocks(&self, addr: u64, num_blocks: u64) -> Result<u64, Error> {
        if addr < self.region_addr
            || num_blocks == 0
            || addr + num_blocks * self.plugged_block_size > self.region_addr + self.region_size
            || addr % self.plugged_block_size != 0
        {
            return Err(anyhow!("Provided addr and num_blocks are not valid. addr = {} num_blocks = {} plugged_block_size = {} region_addr = {} region_size = {}",
            addr, num_blocks, self.plugged_block_size, self.region_addr, self.region_size));
        }

        let mut counter = 0;
        let base = (addr - self.region_addr) / self.plugged_block_size;
        for i in 0..num_blocks {
            let bit_index = (base + i) as usize;
            if self.map[bit_index / NUM_BITS_IN_BYTE] & (1 << (bit_index % NUM_BITS_IN_BYTE)) != 0 {
                counter += 1;
            }
        }
        Ok(counter)
    }

    pub fn set_blocks(&mut self, addr: u64, num_blocks: u64) -> Result<(), Error> {
        let num_already_plugged_blocks = self.test_blocks(addr, num_blocks)?;
        if num_already_plugged_blocks != 0 {
            return Err(anyhow!("Attempt to plug already plugged blocks. addr = {} num_blocks = {} num_already_plugged_blocks = {}", addr, num_blocks, num_already_plugged_blocks));
        }
        let base = (addr - self.region_addr) / self.plugged_block_size;
        for i in 0..num_blocks {
            let bit_index = (base + i) as usize;
            self.map[bit_index / NUM_BITS_IN_BYTE] |= 1 << (bit_index % NUM_BITS_IN_BYTE);
        }
        assert!(self.test_blocks(addr, num_blocks)? == num_blocks);
        Ok(())
    }

    pub fn clear_blocks(&mut self, addr: u64, num_blocks: u64) -> Result<(), Error> {
        let num_already_plugged_blocks = self.test_blocks(addr, num_blocks)?;
        if self.test_blocks(addr, num_blocks)? != num_blocks {
            return Err(anyhow!("Attempt to unplug already unplugged blocks. addr = {} num_blocks = {} num_already_plugged_blocks = {}", addr, num_blocks, num_already_plugged_blocks));
        }
        let base = (addr - self.region_addr) / self.plugged_block_size;
        for i in 0..num_blocks {
            let bit_index = (base + i) as usize;
            self.map[bit_index / NUM_BITS_IN_BYTE] &= !(1 << (bit_index % NUM_BITS_IN_BYTE));
        }
        assert!(self.test_blocks(addr, num_blocks)? == 0);
        assert!(
            addr >= self.region_addr
                && addr + num_blocks * self.plugged_block_size
                    <= self.region_addr + self.region_size
        );
        Ok(())
    }

    pub fn clear_all(&mut self) {
        self.map.fill(0)
    }
}

pub struct MemDevice<B: MemBackend> {
    backend: B,
    control_handle: VirtioMemControlHandle,
    plugged_size_bytes: inspect::UintProperty,
    plugged_size_bytes_value: u64,
    bitmap: MemBitmap,
}

impl<B: MemBackend> MemDevice<B> {
    pub fn new(
        backend: B,
        control_handle: VirtioMemControlHandle,
        inspect_node: &inspect::Node,
        region_addr: u64,
        plugged_block_size: u64,
        region_size: u64,
    ) -> Self {
        let plugged_size_bytes_value = 0;
        let plugged_size_bytes =
            inspect_node.create_uint("plugged_size_bytes", plugged_size_bytes_value);
        Self {
            backend,
            control_handle,
            plugged_size_bytes,
            plugged_size_bytes_value,
            bitmap: MemBitmap::new(region_addr, plugged_block_size, region_size),
        }
    }

    pub fn plug_memory(&mut self, request: wire::VirtioMemRequest) -> Result<u16, Error> {
        let req_num_blocks = request.nb_blocks.get() as u64;
        self.bitmap.set_blocks(request.addr.get(), req_num_blocks)?;
        self.plugged_size_bytes_value = self
            .plugged_size_bytes
            .add(req_num_blocks * self.bitmap.plugged_block_size)
            .ok_or(inspect::Error::NoOp("plugged_size_bytes"))?;
        self.control_handle.send_on_config_changed(self.plugged_size_bytes_value)?;
        Ok(wire::VIRTIO_MEM_RESP_ACK)
    }

    pub fn unplug_memory(&mut self, request: wire::VirtioMemRequest) -> Result<u16, Error> {
        let req_num_blocks = request.nb_blocks.get() as u64;
        self.bitmap.clear_blocks(request.addr.get(), req_num_blocks)?;
        self.backend.decommit_range(
            u64::from(request.addr),
            req_num_blocks * self.bitmap.plugged_block_size,
        )?;
        self.plugged_size_bytes_value = self
            .plugged_size_bytes
            .subtract(req_num_blocks * self.bitmap.plugged_block_size)
            .ok_or(inspect::Error::NoOp("plugged_size_bytes"))?;
        self.control_handle.send_on_config_changed(self.plugged_size_bytes_value)?;
        Ok(wire::VIRTIO_MEM_RESP_ACK)
    }

    pub fn unplug_all_memory(&mut self) -> Result<u16, Error> {
        self.bitmap.clear_all();
        self.plugged_size_bytes_value = 0;
        self.plugged_size_bytes.set(self.plugged_size_bytes_value);
        self.backend.decommit_range(self.bitmap.region_addr, self.bitmap.region_size)?;
        self.control_handle.send_on_config_changed(self.plugged_size_bytes_value)?;
        Ok(wire::VIRTIO_MEM_RESP_ACK)
    }

    pub fn get_memory_state(&self, request: wire::VirtioMemRequest) -> Result<u16, Error> {
        let req_num_blocks = request.nb_blocks.get() as u64;
        let num_set_blocks = self.bitmap.test_blocks(request.addr.get(), req_num_blocks)?;
        if num_set_blocks == req_num_blocks {
            Ok(wire::VIRTIO_MEM_STATE_PLUGGED)
        } else if num_set_blocks == 0 {
            Ok(wire::VIRTIO_MEM_STATE_UNPLUGGED)
        } else {
            Ok(wire::VIRTIO_MEM_STATE_MIXED)
        }
    }

    pub fn process_guest_request_chain<'a, 'b, N: DriverNotify, M: DriverMem>(
        &mut self,
        mut chain: ReadableChain<'a, 'b, N, M>,
    ) -> Result<(), Error> {
        // Handle request parsing and processing errors by logging and sending back VIRTIO_MEM_RESP_ERROR response
        // Propogate send response errors up to the caller
        let request = match read_request(&mut chain) {
            Ok(request) => request,
            Err(e) => {
                log::warn!(
                    "Failed to parse guest request header chain_remaining = {:?}",
                    chain.remaining()
                );
                return write_response(
                    chain,
                    wire::VirtioMemResponse {
                        ty: wire::VIRTIO_MEM_RESP_ERROR.into(),
                        ..Default::default()
                    },
                )
                .with_context(|| format!("Failed to read request from queue: {}", e));
            }
        };
        let response = match request.ty.get() {
            wire::VIRTIO_MEM_REQ_PLUG => self.plug_memory(request),
            wire::VIRTIO_MEM_REQ_UNPLUG => self.unplug_memory(request),
            wire::VIRTIO_MEM_REQ_UNPLUG_ALL => self.unplug_all_memory(),
            wire::VIRTIO_MEM_REQ_STATE => self.get_memory_state(request),
            _ => Err(anyhow!("Invalid response type = {}", request.ty.get())),
        };

        match response {
            Ok(state) => {
                let ty = request.ty.get();
                let addr = request.addr.get();
                let nb_blocks = request.nb_blocks.get();
                let plugged_size_bytes = self.plugged_size_bytes_value;
                log::trace!(ty:?, addr:?, nb_blocks:?, plugged_size_bytes:?; "");
                write_response(
                    chain,
                    wire::VirtioMemResponse {
                        ty: wire::VIRTIO_MEM_RESP_ACK.into(),
                        state: state.into(),
                        ..Default::default()
                    },
                )
            }
            Err(e) => {
                log::error!(
                    "{} ty = {} addr = {} nb_blocks = {}",
                    e,
                    request.ty.get(),
                    request.addr.get(),
                    request.nb_blocks.get()
                );
                write_response(
                    chain,
                    wire::VirtioMemResponse {
                        ty: wire::VIRTIO_MEM_RESP_ERROR.into(),
                        ..Default::default()
                    },
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fidl::endpoints::{create_proxy_and_stream, RequestStream};
    use fidl_fuchsia_virtualization_hardware::{VirtioMemMarker, VirtioMemProxy};
    use fuchsia_inspect::Inspector;
    use futures::StreamExt;
    use std::cell::RefCell;
    use std::ops::Range;
    use virtio_device::fake_queue::{ChainBuilder, IdentityDriverMem, TestQueue};

    const ONE_MIB: u64 = 1 * 1024 * 1024;

    const DEFAULT_PLUGGED_BLOCK_SIZE: u64 = ONE_MIB;
    const DEFAULT_REGION_SIZE: u64 = 35 * DEFAULT_PLUGGED_BLOCK_SIZE;

    pub struct TestMemBackend {
        inner: VmoMemoryBackend,
        calls: RefCell<Vec<Range<u64>>>,
    }

    impl TestMemBackend {
        pub fn new(inner: VmoMemoryBackend) -> Self {
            Self { inner, calls: RefCell::new(Vec::new()) }
        }
    }

    impl MemBackend for TestMemBackend {
        fn decommit_range(&self, offset: u64, size: u64) -> Result<(), zx::Status> {
            self.calls.borrow_mut().push(offset..offset + size);
            self.inner.decommit_range(offset, size)
        }
    }

    struct TestFixture<'a> {
        state: TestQueue<'a>,
        device: MemDevice<TestMemBackend>,
        inspector: Inspector,
        region_addr: u64,
        region_size: u64,
        plugged_block_size: u64,
        proxy: VirtioMemProxy,
        expected_plugged_bytes: u64,
    }

    impl<'a> TestFixture<'a> {
        pub fn new(mem: &'a IdentityDriverMem) -> Self {
            let vmo_virt_queues_aligned_size: u64 = ONE_MIB;
            let region_addr = vmo_virt_queues_aligned_size;
            let region_size = DEFAULT_REGION_SIZE;
            let plugged_block_size = DEFAULT_PLUGGED_BLOCK_SIZE;

            let inspector = Inspector::default();

            let state = TestQueue::new(32, mem);
            let (proxy, stream) = create_proxy_and_stream::<VirtioMemMarker>();
            let vmo_size = region_size + vmo_virt_queues_aligned_size;
            let vmo = zx::Vmo::create(vmo_size).unwrap();
            let device = MemDevice::new(
                TestMemBackend::new(VmoMemoryBackend::new(vmo)),
                stream.control_handle(),
                &inspector.root(),
                region_addr,
                plugged_block_size,
                region_size,
            );
            Self {
                state,
                device,
                inspector,
                region_addr,
                region_size,
                plugged_block_size,
                proxy,
                expected_plugged_bytes: 0,
            }
        }

        pub fn read_response(&mut self) -> wire::VirtioMemResponse {
            let returned = self.state.fake_queue.next_used().unwrap();
            assert_eq!(std::mem::size_of::<wire::VirtioMemResponse>(), returned.written() as usize);

            let (data, len) = returned.data_iter().next().unwrap();
            let slice = unsafe {
                std::slice::from_raw_parts::<u8>(data as usize as *const u8, len as usize)
            };
            wire::VirtioMemResponse::read_from_bytes(slice)
                .expect("Failed to read result from returned chain")
        }
        // Send a single mem request and get a response
        pub fn send_command(
            &mut self,
            mem: &'a IdentityDriverMem,
            ty: u16,
            addr: u64,
            num_blocks: u16,
        ) -> Result<u16, u16> {
            self.state
                .fake_queue
                .publish(
                    ChainBuilder::new()
                        .readable(
                            std::slice::from_ref(&wire::VirtioMemRequest {
                                ty: ty.into(),
                                addr: addr.into(),
                                nb_blocks: num_blocks.into(),
                                ..Default::default()
                            }),
                            &mem,
                        )
                        .writable(std::mem::size_of::<wire::VirtioMemResponse>() as u32, &mem)
                        .build(),
                )
                .unwrap();
            self.device
                .process_guest_request_chain(ReadableChain::new(
                    self.state.queue.next_chain().unwrap(),
                    mem,
                ))
                .expect("Failed to process test chain");

            let response = self.read_response();
            if response.ty.get() != wire::VIRTIO_MEM_RESP_ACK {
                Err(response.ty.get())
            } else {
                Ok(response.state.get())
            }
        }

        // Convenience wrappers over the send_command
        pub fn get_state(&mut self, mem: &'a IdentityDriverMem, addr: u64, num_blocks: u16) -> u16 {
            self.send_command(mem, wire::VIRTIO_MEM_REQ_STATE, addr, num_blocks).unwrap()
        }

        pub fn get_state_expect_fail(
            &mut self,
            mem: &'a IdentityDriverMem,
            addr: u64,
            num_blocks: u16,
        ) -> u16 {
            let res = self.send_command(mem, wire::VIRTIO_MEM_REQ_STATE, addr, num_blocks);
            assert!(res.is_err());
            res.err().unwrap()
        }

        pub async fn plug(
            &mut self,
            mem: &'a IdentityDriverMem,
            addr: u64,
            num_blocks: u16,
        ) -> u16 {
            let num_expected_decommits = self.device.backend.calls.borrow().len();
            let res = self.send_command(mem, wire::VIRTIO_MEM_REQ_PLUG, addr, num_blocks);
            assert_eq!(num_expected_decommits, self.device.backend.calls.borrow().len());
            if res.is_ok() {
                self.expected_plugged_bytes += num_blocks as u64 * self.plugged_block_size;
                assert_eq!(
                    self.proxy
                        .take_event_stream()
                        .into_future()
                        .await
                        .0
                        .unwrap()
                        .unwrap()
                        .into_on_config_changed()
                        .unwrap(),
                    self.expected_plugged_bytes
                );
                assert_data_tree!(self.inspector, root: { plugged_size_bytes:  self.expected_plugged_bytes});
            }
            res.err().unwrap_or(wire::VIRTIO_MEM_RESP_ACK)
        }

        pub async fn unplug(
            &mut self,
            mem: &'a IdentityDriverMem,
            addr: u64,
            num_blocks: u16,
        ) -> u16 {
            let mut num_expected_decommits = self.device.backend.calls.borrow().len();
            let res = self.send_command(mem, wire::VIRTIO_MEM_REQ_UNPLUG, addr, num_blocks);
            if res.is_ok() {
                num_expected_decommits += 1;
                self.expected_plugged_bytes -= num_blocks as u64 * self.plugged_block_size;
                assert_eq!(
                    self.proxy
                        .take_event_stream()
                        .into_future()
                        .await
                        .0
                        .unwrap()
                        .unwrap()
                        .into_on_config_changed()
                        .unwrap(),
                    self.expected_plugged_bytes
                );
                assert_data_tree!(self.inspector, root: { plugged_size_bytes:  self.expected_plugged_bytes});
                assert_eq!(
                    self.device.backend.calls.borrow().last().unwrap(),
                    &std::ops::Range {
                        start: addr,
                        end: addr + num_blocks as u64 * self.plugged_block_size
                    }
                );
            }
            assert_eq!(num_expected_decommits, self.device.backend.calls.borrow().len());
            res.err().unwrap_or(wire::VIRTIO_MEM_RESP_ACK)
        }

        pub async fn unplug_all(&mut self, mem: &'a IdentityDriverMem) -> u16 {
            let mut num_expected_decommits = self.device.backend.calls.borrow().len();
            let res = self.send_command(mem, wire::VIRTIO_MEM_REQ_UNPLUG_ALL, 0, 0);
            if res.is_ok() {
                self.expected_plugged_bytes = 0;
                assert_eq!(
                    self.proxy
                        .take_event_stream()
                        .into_future()
                        .await
                        .0
                        .unwrap()
                        .unwrap()
                        .into_on_config_changed()
                        .unwrap(),
                    self.expected_plugged_bytes
                );
                assert_data_tree!(self.inspector, root: { plugged_size_bytes:  self.expected_plugged_bytes});
                num_expected_decommits += 1;
                assert_eq!(
                    self.device.backend.calls.borrow().last().unwrap(),
                    &std::ops::Range {
                        start: self.region_addr,
                        end: self.region_addr + self.region_size
                    }
                );
            }
            assert_eq!(num_expected_decommits, self.device.backend.calls.borrow().len());
            assert_eq!(self.device.backend.inner.vmo.info().unwrap().committed_bytes, 0);
            res.err().unwrap_or(wire::VIRTIO_MEM_RESP_ACK)
        }
    }

    #[fuchsia::test]
    async fn test_request_state_all_unplugged() {
        let mem = IdentityDriverMem::new();
        let mut test = TestFixture::new(&mem);
        assert_eq!(
            test.get_state(
                &mem,
                test.region_addr,
                (test.region_size / test.plugged_block_size) as u16
            ),
            wire::VIRTIO_MEM_STATE_UNPLUGGED
        );
        assert_eq!(test.get_state(&mem, test.region_addr, 1), wire::VIRTIO_MEM_STATE_UNPLUGGED);
        assert_data_tree!(test.inspector, root: { plugged_size_bytes:  0u64 });
    }

    #[fuchsia::test]
    async fn test_request_two_plugs_leave_hole() {
        // unplugged blocks marked  '.'
        // plugged blocks marked    'x'
        // test blocks marked as    '^'
        let mem = IdentityDriverMem::new();
        let mut test = TestFixture::new(&mem);
        // [.................................]
        // [^^^^^^^^^^^^^^^^^^^^.............]
        assert_eq!(test.get_state(&mem, test.region_addr, 20), wire::VIRTIO_MEM_STATE_UNPLUGGED);

        let plug_offset = test.plugged_block_size * 15;
        let plug_addr: u64 = test.region_addr + plug_offset;
        // plug 9 blocks ( 15 - 23 )
        // [..............xxxxxxxxx..........]
        assert_eq!(test.plug(&mem, plug_addr, 9).await, wire::VIRTIO_MEM_RESP_ACK);
        // test blocks
        // [..............xxxxxxxxx..........]
        // [............^^^^^^^^^^...........]
        assert_eq!(
            test.get_state(&mem, plug_addr - test.plugged_block_size, 9),
            wire::VIRTIO_MEM_STATE_MIXED
        );
        // [..............xxxxxxxxx..........]
        // [..^^^^^^^^^......................]
        assert_eq!(
            test.get_state(&mem, plug_addr - test.plugged_block_size * 10, 9),
            wire::VIRTIO_MEM_STATE_UNPLUGGED
        );
        // [..............xxxxxxxxx..........]
        // [..............^^^^^^^^^..........]
        assert_eq!(test.get_state(&mem, plug_addr, 9), wire::VIRTIO_MEM_STATE_PLUGGED);
        // [..............xxxxxxxxx..........]
        // [...............^^^^^^^^^.........]
        assert_eq!(test.get_state(&mem, plug_addr, 10), wire::VIRTIO_MEM_STATE_MIXED);
        // [..............xxxxxxxxx..........]
        // [..............^^^^^^^^...........]
        assert_eq!(test.get_state(&mem, plug_addr, 8), wire::VIRTIO_MEM_STATE_PLUGGED);
        // [..............xxxxxxxxx..........]
        // [.................^^^.............]
        assert_eq!(
            test.get_state(&mem, plug_addr + test.plugged_block_size * 3, 3),
            wire::VIRTIO_MEM_STATE_PLUGGED
        );
        // [..............xxxxxxxxx..........]
        // [..............^..................]
        assert_eq!(test.get_state(&mem, plug_addr, 1), wire::VIRTIO_MEM_STATE_PLUGGED);
        // [..............xxxxxxxxx..........]
        // [......................^..........]
        assert_eq!(
            test.get_state(&mem, plug_addr + test.plugged_block_size * 8, 1),
            wire::VIRTIO_MEM_STATE_PLUGGED
        );
        // [..............xxxxxxxxx..........]
        // [.......................^.........]
        assert_eq!(
            test.get_state(&mem, plug_addr + test.plugged_block_size * 9, 1),
            wire::VIRTIO_MEM_STATE_UNPLUGGED
        );
        // plug 10 more blocks leaving a one block hole in between
        // plugged blocks are in the (25, 34) range
        assert_eq!(
            test.plug(&mem, plug_addr + test.plugged_block_size * 10, 10).await,
            wire::VIRTIO_MEM_RESP_ACK
        );
        // validate that get state can detect a hole
        // [..............xxxxxxxxx.xxxxxxxxxx]
        // [..............^^^^^^^^^^^^^^^^^^^^]
        assert_eq!(test.get_state(&mem, plug_addr, 20), wire::VIRTIO_MEM_STATE_MIXED);
        // validate that get state can detect a hole
        // [...............xxxxxxxxx.xxxxxxxxxx]
        // [........................^..........]
        assert_eq!(
            test.get_state(&mem, plug_addr + test.plugged_block_size * 9, 1),
            wire::VIRTIO_MEM_STATE_UNPLUGGED
        );
    }

    #[fuchsia::test]
    async fn test_request_unplug() {
        // unplugged blocks marked  '.'
        // plugged blocks marked    'x'
        // test blocks marked as    '^'
        let mem = IdentityDriverMem::new();
        let mut test = TestFixture::new(&mem);
        let plug_offset = test.plugged_block_size * 3;
        let plug_addr: u64 = test.region_addr + plug_offset;
        // plug 7 blocks ( 3 to 9 )
        // [...xxxxxxx.........................]
        assert_eq!(test.plug(&mem, plug_addr, 7).await, wire::VIRTIO_MEM_RESP_ACK);
        assert_data_tree!(test.inspector, root: { plugged_size_bytes:  7 * test.plugged_block_size });
        // unplug 2 blocks in the middle of the previously plugged block
        // [...x..xxxx.........................]
        assert_eq!(
            test.unplug(&mem, plug_addr + test.plugged_block_size, 2).await,
            wire::VIRTIO_MEM_RESP_ACK
        );

        assert_data_tree!(test.inspector, root: { plugged_size_bytes:  5 * test.plugged_block_size });
        // test blocks
        // [...x..xxxx.........................]
        // [...^^^^^^^.........................]
        assert_eq!(test.get_state(&mem, plug_addr, 7), wire::VIRTIO_MEM_STATE_MIXED);
        // [...x..xxxx.........................]
        // [...^...............................]
        assert_eq!(test.get_state(&mem, plug_addr, 1), wire::VIRTIO_MEM_STATE_PLUGGED);
        // [...x..xxxx.........................]
        // [....^..............................]
        assert_eq!(
            test.get_state(&mem, plug_addr + test.plugged_block_size, 1),
            wire::VIRTIO_MEM_STATE_UNPLUGGED
        );
        // [...x..xxxx.........................]
        // [.....^.............................]
        assert_eq!(
            test.get_state(&mem, plug_addr + test.plugged_block_size, 2),
            wire::VIRTIO_MEM_STATE_UNPLUGGED
        );
        // [...x..xxxx.........................]
        // [....^^^^^..........................]
        assert_eq!(
            test.get_state(&mem, plug_addr + test.plugged_block_size, 6),
            wire::VIRTIO_MEM_STATE_MIXED
        );
        // [...x..xxxx.........................]
        // [......^^^..........................]
        assert_eq!(
            test.get_state(&mem, plug_addr + test.plugged_block_size * 3, 3),
            wire::VIRTIO_MEM_STATE_PLUGGED
        );
        // unplug the rest of the plugged blocks
        // [......xxxx.........................]
        assert_eq!(test.unplug(&mem, plug_addr, 1).await, wire::VIRTIO_MEM_RESP_ACK);
        // [...................................]
        assert_eq!(
            test.unplug(&mem, plug_addr + test.plugged_block_size * 3, 4).await,
            wire::VIRTIO_MEM_RESP_ACK
        );
        assert_data_tree!(test.inspector, root: { plugged_size_bytes:  0u64});
        // [...................................]
        // [^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^]
        assert_eq!(
            test.get_state(
                &mem,
                test.region_addr,
                (test.region_size / test.plugged_block_size) as u16
            ),
            wire::VIRTIO_MEM_STATE_UNPLUGGED
        );
    }

    #[fuchsia::test]
    async fn test_request_unplug_all_plug_again() {
        // unplugged blocks marked  '.'
        // plugged blocks marked    'x'
        // test blocks marked as    '^'
        let mem = IdentityDriverMem::new();
        let mut test = TestFixture::new(&mem);
        let plug_offset = test.plugged_block_size * 7;
        let plug_addr: u64 = test.region_addr + plug_offset;
        // [.......xxxxxxxxxxx.................]
        // [...................................]
        assert_eq!(test.plug(&mem, plug_addr, 11).await, wire::VIRTIO_MEM_RESP_ACK);
        // [.......xxxxxxxxxxx............xxxxx]
        // [...................................]
        assert_eq!(
            test.plug(&mem, test.region_addr + 30 * test.plugged_block_size, 5).await,
            wire::VIRTIO_MEM_RESP_ACK
        );
        // [.......xxxxxxxxxxx............xxxxx]
        // [^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^]
        assert_eq!(
            test.get_state(
                &mem,
                test.region_addr,
                (test.region_size / test.plugged_block_size) as u16
            ),
            wire::VIRTIO_MEM_STATE_MIXED
        );
        assert_data_tree!(test.inspector, root: { plugged_size_bytes:  16 * test.plugged_block_size});

        // [...................................]
        assert_eq!(test.unplug_all(&mem).await, wire::VIRTIO_MEM_RESP_ACK);
        // [...................................]
        // [^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^]
        assert_eq!(
            test.get_state(
                &mem,
                test.region_addr,
                (test.region_size / test.plugged_block_size) as u16
            ),
            wire::VIRTIO_MEM_STATE_UNPLUGGED
        );
        assert_data_tree!(test.inspector, root: { plugged_size_bytes:  0u64});

        // [........xxx........................]
        assert_eq!(
            test.plug(&mem, plug_addr + test.plugged_block_size * 1, 3).await,
            wire::VIRTIO_MEM_RESP_ACK
        );
        // [........xxx........................]
        // [^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^]
        assert_eq!(
            test.get_state(
                &mem,
                test.region_addr,
                (test.region_size / test.plugged_block_size) as u16
            ),
            wire::VIRTIO_MEM_STATE_MIXED
        );
        assert_data_tree!(test.inspector, root: { plugged_size_bytes:  3 * test.plugged_block_size});
    }

    #[fuchsia::test]
    async fn test_request_plug_all_unplug_all() {
        // unplugged blocks marked  '.'
        // plugged blocks marked    'x'
        // test blocks marked as    '^'
        let mem = IdentityDriverMem::new();
        let mut test = TestFixture::new(&mem);
        // Plug the entire range and test it
        // [xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx]
        assert_eq!(
            test.plug(&mem, test.region_addr, (test.region_size / test.plugged_block_size) as u16)
                .await,
            wire::VIRTIO_MEM_RESP_ACK
        );
        // [xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx]
        // [^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^]
        assert_eq!(
            test.get_state(
                &mem,
                test.region_addr,
                (test.region_size / test.plugged_block_size) as u16
            ),
            wire::VIRTIO_MEM_STATE_PLUGGED
        );
        // Unplug all and test it
        // [...................................]
        assert_eq!(test.unplug_all(&mem).await, wire::VIRTIO_MEM_RESP_ACK);
        // [...................................]
        // [^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^]
        assert_eq!(
            test.get_state(
                &mem,
                test.region_addr,
                (test.region_size / test.plugged_block_size) as u16
            ),
            wire::VIRTIO_MEM_STATE_UNPLUGGED
        );
        // Do it again, this time use the full range unplug command
        // [xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx]
        assert_eq!(
            test.plug(&mem, test.region_addr, (test.region_size / test.plugged_block_size) as u16)
                .await,
            wire::VIRTIO_MEM_RESP_ACK
        );
        // [xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx]
        // [^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^]
        assert_eq!(
            test.get_state(
                &mem,
                test.region_addr,
                (test.region_size / test.plugged_block_size) as u16
            ),
            wire::VIRTIO_MEM_STATE_PLUGGED
        );
        // [...................................]
        // [^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^]
        assert_eq!(
            test.unplug(
                &mem,
                test.region_addr,
                (test.region_size / test.plugged_block_size) as u16
            )
            .await,
            wire::VIRTIO_MEM_RESP_ACK
        );
        // [...................................]
        // [^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^]
        assert_eq!(
            test.get_state(
                &mem,
                test.region_addr,
                (test.region_size / test.plugged_block_size) as u16
            ),
            wire::VIRTIO_MEM_STATE_UNPLUGGED
        );
    }

    #[fuchsia::test]
    async fn test_invalid_plug() {
        let mem = IdentityDriverMem::new();
        let mut test = TestFixture::new(&mem);
        // unaligned plug
        assert_eq!(test.plug(&mem, test.region_addr + 8, 3).await, wire::VIRTIO_MEM_RESP_ERROR);
        // out of bounds plugs
        assert_eq!(
            test.plug(
                &mem,
                test.region_addr + DEFAULT_PLUGGED_BLOCK_SIZE,
                (DEFAULT_REGION_SIZE / DEFAULT_PLUGGED_BLOCK_SIZE) as u16
            )
            .await,
            wire::VIRTIO_MEM_RESP_ERROR
        );
        assert_eq!(
            test.plug(&mem, test.region_addr + DEFAULT_REGION_SIZE, 1).await,
            wire::VIRTIO_MEM_RESP_ERROR
        );
        // normal plug to check the double plug
        assert_eq!(test.plug(&mem, test.region_addr, 3).await, wire::VIRTIO_MEM_RESP_ACK);
        // attempt to plug an already plugged block
        assert_eq!(test.plug(&mem, test.region_addr, 1).await, wire::VIRTIO_MEM_RESP_ERROR);
        // attempt to plug an already plugged block
        assert_eq!(test.plug(&mem, test.region_addr, 1).await, wire::VIRTIO_MEM_RESP_ERROR);
    }

    #[fuchsia::test]
    async fn test_invalid_unplug() {
        let mem = IdentityDriverMem::new();
        let mut test = TestFixture::new(&mem);
        // plug the entire memory region
        assert_eq!(
            test.plug(&mem, test.region_addr, (test.region_size / test.plugged_block_size) as u16)
                .await,
            wire::VIRTIO_MEM_RESP_ACK
        );
        // unaligned unplug
        assert_eq!(test.unplug(&mem, test.region_addr + 32, 1).await, wire::VIRTIO_MEM_RESP_ERROR);
        // out of bounds unplugs
        assert_eq!(
            test.unplug(
                &mem,
                test.region_addr + DEFAULT_PLUGGED_BLOCK_SIZE,
                (DEFAULT_REGION_SIZE / DEFAULT_PLUGGED_BLOCK_SIZE) as u16
            )
            .await,
            wire::VIRTIO_MEM_RESP_ERROR
        );
        assert_eq!(
            test.unplug(&mem, test.region_addr + DEFAULT_REGION_SIZE, 1).await,
            wire::VIRTIO_MEM_RESP_ERROR
        );
        // unplug all to test the the unplugging of the non-plugged block
        assert_eq!(test.unplug_all(&mem).await, wire::VIRTIO_MEM_RESP_ACK);
        // attempt to unplug non-plugged block
        assert_eq!(test.unplug(&mem, test.region_addr, 3).await, wire::VIRTIO_MEM_RESP_ERROR);
    }

    #[fuchsia::test]
    async fn test_invalid_get_state() {
        let mem = IdentityDriverMem::new();
        let mut test = TestFixture::new(&mem);
        // unaligned get state
        assert_eq!(
            test.get_state_expect_fail(&mem, test.region_addr + 32, 1),
            wire::VIRTIO_MEM_RESP_ERROR
        );
        // out of bounds get state
        assert_eq!(
            test.get_state_expect_fail(&mem, test.region_addr + DEFAULT_REGION_SIZE, 1),
            wire::VIRTIO_MEM_RESP_ERROR
        );
        assert_eq!(
            test.get_state_expect_fail(&mem, test.region_addr - DEFAULT_PLUGGED_BLOCK_SIZE, 1),
            wire::VIRTIO_MEM_RESP_ERROR
        );
    }
}
