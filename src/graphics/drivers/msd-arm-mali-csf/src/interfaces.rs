// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Interfaces are shared memory regions where the CPU and GPU firmware can communicate.
//! They are used by the CPU for setting Group and Command Stream information.
//! They are used by the GPU for setting idle and ringbuffer information.

use crate::utils::LogError;
use crate::{barriers, firmware, mem, regs, utils};
use bitfield::bitfield;
use mmio::{Mmio, ReadableRegister};

/// This represents any of the "request" types in an interface.
/// The CPU makes a "request" by toggling a bit in the request field.
/// The request completes when the GPU toggles the corresponding bit in the ack field.
/// Any bits that do not match between request and ack are said to be "pending".
pub struct InterfaceRequest {
    pub request_offset: usize,
    pub ack_offset: usize,
}

impl InterfaceRequest {
    fn new(request_offset: usize, ack_offset: usize) -> Self {
        Self { request_offset, ack_offset }
    }

    #[allow(unused)]
    pub fn dump(&self, interface_memory: &mem::MappedMemory) {
        let request = interface_memory.read32(self.request_offset);
        let ack = interface_memory.read32(self.ack_offset);
        log::info!("Interface request 0x{:x} val 0x{:x}", self.request_offset, request);
        log::info!("Interface ack     0x{:x} val 0x{:x}", self.ack_offset, ack);
        log::info!("Interface pending            0x{:x}", request ^ ack);
    }

    pub fn toggle_bits(&self, interface_memory: &mut mem::MappedMemory, bits: u32) {
        let request = interface_memory.read32(self.request_offset);
        interface_memory.write32(self.request_offset, request ^ bits);
    }

    pub fn set_bits(&self, interface_memory: &mut mem::MappedMemory, bits: u32, mask: u32) {
        let request = interface_memory.read32(self.request_offset);
        interface_memory.write32(self.request_offset, (request & !mask) | (bits & mask));
    }

    fn pending_bits(&self, interface_memory: &mem::MappedMemory) -> u32 {
        let request = interface_memory.read32(self.request_offset);
        let ack = interface_memory.read32(self.ack_offset);
        request ^ ack
    }

    pub fn wait_for_acked_bits(
        &self,
        interface_memory: &mem::MappedMemory,
        bits: u32,
        timeout: std::time::Duration,
    ) -> Result<(), zx::Status> {
        // TODO(https://fxbug.dev/498571172): We should be waiting on IRQs.
        let start_time = std::time::Instant::now();
        let sleep_interval = std::time::Duration::from_millis(1);
        while start_time.elapsed() < timeout {
            if bits & self.pending_bits(interface_memory) == 0 {
                return Ok(());
            }
            std::thread::sleep(sleep_interval);
        }
        if bits & self.pending_bits(interface_memory) == 0 {
            return Ok(());
        }
        Err(zx::Status::TIMED_OUT)
    }
}

// Structs with this trait can be read from memory.
// This is used for interfaces that are shared with hardware.
pub trait ReadableStruct {
    fn read_from_address(address: &mem::CpuAddress) -> Self;

    fn read(memory: &mem::MappedMemory, offset: u64) -> Self
    where
        Self: Sized,
    {
        assert!(offset + (std::mem::size_of::<Self>() as u64) <= memory.size as u64);
        Self::read_from_address(&memory.cpu_address.offset(offset))
    }
}

macro_rules! impl_readable_struct {
    ($type:ty) => {
        impl ReadableStruct for $type {
            fn read_from_address(address: &mem::CpuAddress) -> Self {
                // Force all writes before we read.
                crate::barriers::write();

                utils::assert_aligned::<Self>(address.as_u64());

                let ptr = address.as_address();
                // SAFETY: we know that this pointer is aligned and valid for reads for
                // at least size_of::<Self>() bytes.
                unsafe { core::ptr::read_volatile(ptr as *const Self) }
            }
        }
    };
}

// Structs with this trait can be written to memory.
// This is used for interfaces that are shared with hardware.
pub trait WritableStruct {
    fn write_to_address(&self, address: &mut mem::CpuAddress);

    fn write(&self, memory: &mem::MappedMemory, offset: u64)
    where
        Self: Sized,
    {
        assert!(offset + (std::mem::size_of::<Self>() as u64) < memory.size as u64);
        self.write_to_address(&mut memory.cpu_address.offset(offset));
        // TODO(https://fxbug.dev/503722844): Remove this flush.
        memory.flush_cache_bytes(offset as usize, std::mem::size_of::<Self>());
    }
}

macro_rules! impl_writable_struct {
    ($type:ty) => {
        impl WritableStruct for $type {
            fn write_to_address(&self, address: &mut mem::CpuAddress) {
                // Force all earlier writes before we write to hardware.
                crate::barriers::write();

                utils::assert_aligned::<Self>(address.as_u64());

                let ptr = address.as_mut_address() as *mut u32;
                let self_ptr = self as *const Self as *const u32;
                let words = std::mem::size_of::<Self>() / 4;
                for i in 0..words {
                    unsafe {
                        core::ptr::write_volatile(ptr.add(i), self_ptr.add(i).read());
                    }
                }
            }
        }
    };
}

pub mod global {
    use super::*;

    #[derive(Debug)]
    pub struct Interface {
        pub input_offset: usize,
        pub output_offset: usize,
        pub groups: Vec<group::Interface>,
    }

    impl Interface {
        pub fn new(interface_memory: &mem::MappedMemory) -> Self {
            let control = Self::read_control(interface_memory);
            let input_offset =
                control.input_virtual_address as usize - firmware::SHARED_REGION_START;
            let output_offset =
                control.output_virtual_address as usize - firmware::SHARED_REGION_START;
            let groups: Vec<_> = (0..control.groups_supported)
                .map(|i| {
                    group::Interface::new(interface_memory, 0x1000 + (i * control.group_stride))
                })
                .collect();
            Self { input_offset, output_offset, groups }
        }

        pub fn read_control(interface_memory: &mem::MappedMemory) -> Control {
            Control::read(interface_memory, 0)
        }

        pub fn read_input(&self, interface_memory: &mem::MappedMemory) -> Input {
            Input::read(interface_memory, self.input_offset as u64)
        }

        pub fn write_input(&self, interface_memory: &mem::MappedMemory, input: &Input) {
            input.write(interface_memory, self.input_offset as u64)
        }

        #[allow(unused)]
        pub fn read_output(&self, interface_memory: &mem::MappedMemory) -> Output {
            Output::read(interface_memory, self.output_offset as u64)
        }

        pub fn wait_until_ready(interface_memory: &mem::MappedMemory) -> Result<u32, zx::Status> {
            utils::do_until(0..100, |_| {
                let control = Self::read_control(interface_memory);
                if control.version == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    return None;
                }
                Some(control.version)
            })
            .ok_or(zx::Status::TIMED_OUT)
        }

        pub fn initialize(
            &self,
            interface_memory: &mut mem::MappedMemory,
            mmio: &mut impl Mmio,
        ) -> Result<(), zx::Status> {
            // This should be 5s at 500 mHz.
            const PROGRESS_TIMER_CYCLES: u32 = 5 * 500 * 1024 * 1024;
            const IDLE_TIMER_US: u32 = 800;

            let mut input = self.read_input(interface_memory);
            input.shaders_enabled = regs::GpuShaderPresentLow::read(mmio).0 as u32;
            input.poweroff_timer = 0;
            input.progress_timer = PROGRESS_TIMER_CYCLES;
            input.idle_timer = IDLE_TIMER_US;

            let mut request = RequestField(0);
            request.set_idle_enable(1);
            request.set_allocation_endpoint(1);
            request.set_poweroff_timer(1);
            request.set_progress_timer(1);
            input.request = request.0;

            let mut irq_mask = RequestField(0);
            irq_mask.set_allocation_endpoint(1);
            irq_mask.set_ping(1);
            irq_mask.set_idle_enable(1);
            irq_mask.set_idle_event(1);
            input.ack_irq_mask = irq_mask.0;

            self.write_input(interface_memory, &input);

            ring_global_doorbell(mmio);

            self.ping(interface_memory, mmio).log_err("Failed to ping firmware")?;
            Ok(())
        }

        pub fn ping(
            &self,
            interface_memory: &mut mem::MappedMemory,
            mmio: &mut impl Mmio,
        ) -> Result<(), zx::Status> {
            let request = self.request();
            let mut ping = RequestField(0);
            ping.set_ping(1);

            request.toggle_bits(interface_memory, ping.0);
            ring_global_doorbell(mmio);

            utils::do_until(0..50, |_| {
                if RequestField(request.pending_bits(interface_memory)).ping() == 0 {
                    Some(())
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    None
                }
            })
            .ok_or(zx::Status::TIMED_OUT)
        }

        pub fn ring_csg_doorbell(
            &mut self,
            interface_memory: &mut mem::MappedMemory,
            mmio: &mut impl Mmio,
            index: usize,
        ) {
            self.doorbell_request().toggle_bits(interface_memory, 1 << index);
            ring_global_doorbell(mmio);
        }

        pub fn request(&self) -> InterfaceRequest {
            InterfaceRequest::new(
                self.input_offset as usize + core::mem::offset_of!(Input, request),
                self.output_offset as usize + core::mem::offset_of!(Output, ack),
            )
        }

        pub fn doorbell_request(&self) -> InterfaceRequest {
            InterfaceRequest::new(
                self.input_offset + core::mem::offset_of!(Input, doorbell_request),
                self.output_offset + core::mem::offset_of!(Output, doorbell_ack),
            )
        }
    }

    bitfield! {
        pub struct RequestField(u32);
        impl Debug;
        pub halt, set_halt: 0, 0;
        pub progress_timer, set_progress_timer: 1, 1;
        pub allocation_endpoint, set_allocation_endpoint: 2, 2;
        pub poweroff_timer, set_poweroff_timer: 3, 3;
        pub ping, set_ping: 8, 8;
        pub idle_enable, set_idle_enable: 10, 10;
        pub idle_event, set_idle_event: 26, 26;
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone)]
    pub struct Control {
        pub version: u32,
        pub features: u32,
        pub input_virtual_address: u32,
        pub output_virtual_address: u32,
        pub groups_supported: u32,
        pub group_stride: u32,
        pub performance_counters_size: u32,
        pub instrumentation_features: u32,
    }
    impl_readable_struct!(Control);
    impl_writable_struct!(Control);

    #[repr(C)]
    #[derive(Debug, Copy, Clone, PartialEq)]
    pub struct Input {
        pub request: u32,
        pub ack_irq_mask: u32,
        pub doorbell_request: u32,
        pub reserved1: u32,
        pub progress_timer: u32,
        pub poweroff_timer: u32,
        pub shaders_enabled: u32,
        pub reserved2: u32,
        pub performance_counters_address_space: u32,
        pub performance_counters_base: u64,
        pub performance_counters_extract: u32,
        pub reserved3: [u32; 3],
        pub performance_counters_config: u32,
        pub performance_counters_csg_select: u32,
        pub performance_counters_fw_enable: u32,
        pub performance_counters_csg_enable: u32,
        pub performance_counters_csf_enable: u32,
        pub performance_counters_shader_enable: u32,
        pub performance_counters_tiler_enable: u32,
        pub performance_counters_mmu_l2_enable: u32,
        pub reserved4: [u32; 8],
        pub idle_timer: u32,
    }
    impl_readable_struct!(Input);
    impl_writable_struct!(Input);

    #[repr(C)]
    #[derive(Debug, Copy, Clone, PartialEq)]
    /// The output area for the global interface
    pub struct Output {
        pub ack: u32,
        pub reserved1: u32,
        pub doorbell_ack: u32,
        pub reserved2: u32,
        pub halt_status: u32,
        pub performance_counters_status: u32,
        pub performance_counters_insert: u32,
    }
    impl_readable_struct!(Output);
    impl_writable_struct!(Output);

    pub fn ring_global_doorbell(mmio: &mut impl Mmio) {
        barriers::write();
        regs::Doorbell::new(regs::GLOBAL_DOORBELL_ID).ring(mmio);
    }
}

pub mod group {
    use super::*;

    #[derive(Debug)]
    pub struct Interface {
        control_offset: u32,
        input_offset: u32,
        output_offset: u32,
        pub queues: Vec<queue::Interface>,
    }

    impl Interface {
        pub fn new(interface_memory: &mem::MappedMemory, control_offset: u32) -> Self {
            let control = Control::read(interface_memory, control_offset as u64);
            let input_offset = control.input_virtual_address - firmware::SHARED_REGION_START as u32;
            let output_offset =
                control.output_virtual_address - firmware::SHARED_REGION_START as u32;
            let queues = (0..control.stream_num)
                .map(|i| {
                    queue::Interface::new(
                        interface_memory,
                        control_offset + 0x40 + (i * control.stride),
                    )
                })
                .collect();
            Self { control_offset, input_offset, output_offset, queues }
        }

        pub fn read_input(&self, interface_memory: &mem::MappedMemory) -> Input {
            Input::read(interface_memory, self.input_offset as u64)
        }

        pub fn write_input(&self, interface_memory: &mem::MappedMemory, input: &Input) {
            input.write(interface_memory, self.input_offset as u64)
        }

        #[allow(unused)]
        pub fn read_output(&self, interface_memory: &mem::MappedMemory) -> Output {
            Output::read(interface_memory, self.output_offset as u64)
        }

        #[allow(unused)]
        pub fn read_control(&self, interface_memory: &mem::MappedMemory) -> Control {
            Control::read(interface_memory, self.control_offset as u64)
        }

        // NOTE: Doorbell needs to be rung after this.
        pub fn set_state(&self, interface_memory: &mut mem::MappedMemory, state: GroupState) {
            let request = self.request();
            request.set_bits(interface_memory, state as u32, 0b111);
            if state == GroupState::Start {
                // CFG REQUEST bit.
                request.toggle_bits(interface_memory, 1 << 4);
                // STATUS UPDATE bit.
                request.toggle_bits(interface_memory, 1 << 5);
            }
        }

        pub fn request(&self) -> InterfaceRequest {
            InterfaceRequest::new(
                self.input_offset as usize + core::mem::offset_of!(Input, request),
                self.output_offset as usize + core::mem::offset_of!(Output, ack),
            )
        }

        pub fn doorbell_request(&self) -> InterfaceRequest {
            InterfaceRequest::new(
                self.input_offset as usize + core::mem::offset_of!(Input, doorbell_request),
                self.output_offset as usize + core::mem::offset_of!(Output, doorbell_ack),
            )
        }
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone)]
    pub struct Control {
        pub features: u32,
        pub input_virtual_address: u32,
        pub output_virtual_address: u32,
        pub suspend_size: u32,
        pub protected_suspend_size: u32,
        pub stream_num: u32,
        pub stride: u32,
    }
    impl_readable_struct!(Control);
    impl_writable_struct!(Control);

    #[repr(C)]
    #[derive(Debug, Copy, Clone)]
    pub struct Input {
        pub request: u32,
        pub ack_irq_mask: u32,
        pub doorbell_request: u32,
        pub irq_ack: u32,
        pub reserved1: [u32; 4],
        pub allow_compute: u64,
        pub allow_fragment: u64,
        pub allow_other: u32,
        pub csg_endpoints_requested: u32,
        pub reserved2: [u32; 2],
        pub suspend_buffer: u64,
        pub protected_suspend_buffer: u64,
        pub csg_config: u32,
        pub reserved3: u32,
    }
    impl_readable_struct!(Input);
    impl_writable_struct!(Input);

    #[repr(C)]
    #[derive(Debug, Copy, Clone)]
    pub struct Output {
        pub ack: u32,
        pub reserved1: u32,
        pub doorbell_ack: u32,
        pub irq_request: u32,
        pub status_endpoint_current: u32,
        pub status_endpoint_request: u32,
        pub status_state: u32,
        pub resource_dependencies: u32,
    }
    impl_readable_struct!(Output);

    #[allow(unused)]
    #[derive(Copy, Clone, Debug, PartialEq)]
    pub enum GroupState {
        Terminate = 0,
        Start = 1,
        Suspend = 2,
        Resume = 3,
    }
}

pub mod queue {
    use super::*;

    #[derive(Debug)]
    pub struct Interface {
        control_offset: u32,
        input_offset: u32,
        output_offset: u32,
    }

    impl Interface {
        pub fn new(interface_memory: &mem::MappedMemory, control_offset: u32) -> Self {
            let control = Control::read(interface_memory, control_offset as u64);
            let input_offset = control.input_virtual_address - firmware::SHARED_REGION_START as u32;
            let output_offset =
                control.output_virtual_address - firmware::SHARED_REGION_START as u32;
            Self { control_offset, input_offset, output_offset }
        }

        pub fn read_input(&self, interface_memory: &mem::MappedMemory) -> Input {
            Input::read(interface_memory, self.input_offset as u64)
        }

        pub fn write_input(&self, interface_memory: &mem::MappedMemory, input: &Input) {
            input.write(interface_memory, self.input_offset as u64)
        }

        #[allow(unused)]
        pub fn read_output(&self, interface_memory: &mem::MappedMemory) -> Output {
            Output::read(interface_memory, self.output_offset as u64)
        }

        #[allow(unused)]
        pub fn read_control(&self, interface_memory: &mem::MappedMemory) -> Control {
            Control::read(interface_memory, self.control_offset as u64)
        }

        pub fn request(&self) -> InterfaceRequest {
            InterfaceRequest::new(
                self.input_offset as usize + core::mem::offset_of!(Input, request),
                self.output_offset as usize + core::mem::offset_of!(Output, ack),
            )
        }
    }

    #[allow(unused)]
    #[repr(u32)]
    pub enum RequestFieldState {
        Stop = 0,
        Start = 1,
    }

    bitfield! {
        pub struct RequestField(u32);
        impl Debug;
        pub state, set_state: 2, 0;
        pub fatal, set_fatal: 30, 30;
        pub fault, set_fault: 31, 31;
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone)]
    pub struct Control {
        pub features: u32,
        pub input_virtual_address: u32,
        pub output_virtual_address: u32,
    }
    impl_readable_struct!(Control);
    impl_writable_struct!(Control);

    #[repr(C)]
    #[derive(Debug, Copy, Clone)]
    pub struct Input {
        pub request: u32,
        pub config: u32,
        pub reserved1: u32,
        pub ack_irq_mask: u32,
        pub ringbuffer_base: u64,
        pub ringbuffer_size: u32,
        pub reserved2: u32,
        pub heap_start: u64,
        pub heap_end: u64,
        pub ringbuffer_input: u64,
        pub ringbuffer_output: u64,
        pub instrumentation_config: u32,
        pub instrumentation_buffer_size: u32,
        pub instrumentation_buffer_base: u64,
        pub instrumentation_buffer_offset_address: u64,
    }
    impl_readable_struct!(Input);
    impl_writable_struct!(Input);

    #[repr(C)]
    #[derive(Debug, Copy, Clone)]
    pub struct Output {
        pub ack: u32,
        pub reserved1: [u32; 15],
        pub status_cmd_ptr: u64,
        pub status_wait: u32,
        pub status_requested_resource: u32,
        pub status_wait_sync_ptr: u64,
        pub status_wait_sync_value: u32,
        pub status_scoreboards: u32,
        pub status_blocked_reason: u32,
        pub status_wait_sync_value_hi: u32,
        pub reserved2: [u32; 6],
        pub fault: u32,
        pub fatal: u32,
        pub fault_info: u64,
        pub fatal_info: u64,
        pub reserved3: [u32; 10],
        pub heap_vt_start: u32,
        pub heap_vt_end: u32,
        pub reserved4: u32,
        pub heap_fragment_end: u32,
        pub heap_address: u64,
    }
    impl_readable_struct!(Output);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem::tests::FakeBusMapper;

    #[fuchsia::test]
    fn test_read_write() {
        let bus_mapper = FakeBusMapper::new(0x1000);
        let (_, mapping, _) = mem::allocate_map_pin(0x1000, &bus_mapper).unwrap();
        let interface = global::Interface {
            input_offset: 0,
            output_offset: std::mem::size_of::<global::Input>(),
            groups: Vec::new(),
        };
        let test_input = global::Input {
            request: 1,
            ack_irq_mask: 2,
            doorbell_request: 3,
            reserved1: 4,
            progress_timer: 5,
            poweroff_timer: 6,
            shaders_enabled: 7,
            reserved2: 0xdead,
            performance_counters_address_space: 8,
            performance_counters_base: 9,
            performance_counters_extract: 10,
            reserved3: [11; 3],
            performance_counters_config: 12,
            performance_counters_csg_select: 13,
            performance_counters_fw_enable: 14,
            performance_counters_csg_enable: 15,
            performance_counters_csf_enable: 16,
            performance_counters_shader_enable: 17,
            performance_counters_tiler_enable: 18,
            performance_counters_mmu_l2_enable: 19,
            reserved4: [20; 8],
            idle_timer: 21,
        };
        interface.write_input(&mapping, &test_input);
        let input = interface.read_input(&mapping);
        assert_eq!(test_input, input);

        let test_output = global::Output {
            ack: 1,
            reserved1: 2,
            doorbell_ack: 3,
            reserved2: 4,
            halt_status: 5,
            performance_counters_status: 6,
            performance_counters_insert: 7,
        };
        global::Output::write(&test_output, &mapping, interface.output_offset as u64);
        let output = interface.read_output(&mapping);
        assert_eq!(test_output, output);

        // Test input again to make sure we didn't overwrite it.
        let input = interface.read_input(&mapping);
        assert_eq!(test_input, input);
    }
    #[fuchsia::test]
    fn test_interface_request() {
        let bus_mapper = mem::tests::FakeBusMapper::new(0x1000);
        let (_, mut mapping, _) = mem::allocate_map_pin(0x1000, &bus_mapper).unwrap();

        let req_offset = 0;
        let ack_offset = 4;
        let request = InterfaceRequest::new(req_offset, ack_offset);

        // Initial state: all 0.
        assert_eq!(request.pending_bits(&mapping), 0);

        // Test toggle_bits.
        request.toggle_bits(&mut mapping, 0b101);
        assert_eq!(mapping.read32(req_offset), 0b101);
        assert_eq!(request.pending_bits(&mapping), 0b101);

        request.toggle_bits(&mut mapping, 0b110);
        assert_eq!(mapping.read32(req_offset), 0b011);

        // Test set_bits.
        request.set_bits(&mut mapping, 0b1111, 0b1100);
        assert_eq!(mapping.read32(req_offset), 0b1111);

        request.set_bits(&mut mapping, 0b0000, 0b0011);
        assert_eq!(mapping.read32(req_offset), 0b1100);

        // Test pending_bits.
        mapping.write32(ack_offset, 0b0100);
        assert_eq!(request.pending_bits(&mapping), 0b1000);

        // Test wait_for_acked_bits.
        assert!(
            request
                .wait_for_acked_bits(&mapping, 0b0010, std::time::Duration::from_millis(10))
                .is_ok()
        );
        assert_eq!(
            request.wait_for_acked_bits(&mapping, 0b1000, std::time::Duration::from_millis(10)),
            Err(zx::Status::TIMED_OUT)
        );
    }
}
