// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::utils::LogError;
use crate::{address_space, interfaces, mem, regs, utils};
use address_space::AddressSpace;

use mmio::Mmio;

#[derive(Clone, Copy)]
pub struct FirmwareId(pub u64);
#[derive(Clone, Copy)]
pub struct DoorbellId(pub u64);

pub struct Group {
    pub address_space: address_space::AddressSpace,
    pub ringbuffers: Vec<Ringbuffer>,
    firmware_id: Option<FirmwareId>,
    address_space_slot: Option<u32>,
    // This is the address that the GPU will store suspend information in if the group is
    // suspended mid-task.
    suspend_buffer_address: mem::GpuAddress,
}

impl Group {
    pub fn new(
        bus_mapper: &dyn mem::BusMapper,
        fw_address_space: &mut AddressSpace,
        ringbuffer_count: usize,
        ringbuffer_size: usize,
        suspend_size: usize,
    ) -> Result<Self, zx::Status> {
        let mut address_space =
            address_space::AddressSpace::new(address_space::Coherency::NonCoherent, bus_mapper)
                .log_err("Failed to create address space")?;
        let ringbuffers = (0..ringbuffer_count)
            .map(|_| {
                Ringbuffer::new(&mut address_space, bus_mapper, fw_address_space, ringbuffer_size)
            })
            .collect::<Result<Vec<Ringbuffer>, zx::Status>>()?;

        let suspend_buffer = mem::Buffer::new(suspend_size, zx::CachePolicy::UnCached)?;
        let suspend_pin = suspend_buffer.pin(
            bus_mapper,
            0,
            suspend_size,
            zx::BtiOptions::PERM_READ | zx::BtiOptions::PERM_WRITE,
        )?;

        let mut access_flags = address_space::AccessFlags(0);
        access_flags.set_access_flag_no_exec(1);
        access_flags.set_access_flag_read(1);
        let suspend_buffer_address =
            fw_address_space.insert_buffer_auto_address(suspend_pin, &access_flags, bus_mapper)?;

        Ok(Self {
            address_space,
            ringbuffers,
            address_space_slot: None,
            firmware_id: None,
            suspend_buffer_address,
        })
    }

    pub fn bind(
        &mut self,
        address_space_slot: u32,
        firmware_id: FirmwareId,
        mmio: &mut impl Mmio,
    ) -> Result<(), zx::Status> {
        if self.address_space_slot.is_some() {
            return Err(zx::Status::ALREADY_BOUND);
        }
        self.address_space.bind_to_hardware(mmio, address_space_slot as u64)?;
        self.address_space_slot = Some(address_space_slot);

        // TODO(https://fxbug.dev/503729455): We could allocate different ringbuffers different doorbells.
        for ringbuffer in &mut self.ringbuffers {
            ringbuffer.doorbell = Some(DoorbellId(firmware_id.0 + 1));
        }

        self.firmware_id = Some(firmware_id);
        Ok(())
    }

    /// This sets the hardware interfaces correctly so that this Group can run on the hardware.
    pub fn configure_interfaces(
        &mut self,
        interface_memory: &mut mem::MappedMemory,
        mmio: &mut impl Mmio,
        interface: &mut interfaces::global::Interface,
    ) -> Result<(), zx::Status> {
        let id = self.firmware_id.ok_or(zx::Status::BAD_STATE)?;
        let address_space_slot = self.address_space_slot.ok_or(zx::Status::BAD_STATE)?;

        let group_interface = &interface.groups[id.0 as usize];
        let mut doorbell_bits = 0;
        for (index, ringbuffer) in self.ringbuffers.iter_mut().enumerate() {
            let queue_interface = &group_interface.queues[index];
            ringbuffer
                .configure_interfaces(interface_memory, queue_interface)
                .log_err("Failed to program firmware")?;
            doorbell_bits |= 1 << index;
        }

        let mut input = group_interface.read_input(interface_memory);

        const ENABLE_ALL_IRQS: u32 = u32::MAX;
        input.ack_irq_mask = ENABLE_ALL_IRQS;

        // TODO(https://fxbug.dev/503724049): Don't hardcode these numbers (it should come from the client).
        input.allow_compute = u64::MAX;
        input.allow_fragment = u64::MAX;
        input.allow_other = u32::MAX;
        input.csg_endpoints_requested = 0x10101;

        input.csg_config = address_space_slot;
        input.suspend_buffer = self.suspend_buffer_address.0;
        group_interface.write_input(interface_memory, &input);
        group_interface.set_state(interface_memory, interfaces::group::GroupState::Start);

        // Ring the doorbells so the hardware knows we have set the configuration.
        group_interface.doorbell_request().toggle_bits(interface_memory, doorbell_bits);
        interface.ring_csg_doorbell(interface_memory, mmio, id.0 as usize);

        // Wait until we see that our status has changed to Start.
        let group_interface = &interface.groups[id.0 as usize];
        let request = group_interface.request();
        // TODO(b/503722844): This is timing out occasionally (but the instructions still run).
        //  - Bumping the duration and cache flushing do not fix timeout
        //  - Acked bits are all 0 so maybe data getting stuck in GPU cache.
        let _ = request
            .wait_for_acked_bits(interface_memory, 0b111, std::time::Duration::from_millis(100))
            .log_err("Failed wait for group bit");

        Ok(())
    }
}

pub struct Ringbuffer {
    doorbell: Option<DoorbellId>,
    // This buffer is the ringbuffer instruction data.
    buffer: mem::MappedMemory,
    buffer_gpu_address: mem::GpuAddress,

    // This buffer contains `RingbufferInput` and `RingbufferOutput` structures.
    // We use this to communicate how much data the ringbuffer contains.
    pub io_buffer: mem::MappedMemory,
    // Note: The output is at offset 0x1000 here.
    io_gpu_address: mem::GpuAddress,
}

impl Ringbuffer {
    fn new(
        address_space: &mut address_space::AddressSpace,
        bus_mapper: &dyn mem::BusMapper,
        fw_address_space: &mut address_space::AddressSpace,
        ringbuffer_size: usize,
    ) -> Result<Self, zx::Status> {
        assert!(
            ringbuffer_size.is_power_of_two(),
            "Ringbuffer size must be power of two: 0x{:x}",
            ringbuffer_size
        );

        let (_, buffer_mapping, buffer_pin) = mem::allocate_map_pin(ringbuffer_size, bus_mapper)
            .log_err("Failed to allocate ringbuffer")?;

        let mut access_flags = address_space::AccessFlags(0);
        access_flags.set_attribute_slot(regs::MemoryAttributeSlot::NonCacheable as u64);
        access_flags.set_access_flag_no_exec(1);
        access_flags.set_access_flag_read(1);

        let buffer_gpu_address =
            address_space.insert_buffer_auto_address(buffer_pin, &access_flags, bus_mapper)?;

        const IO_MEM_SIZE: usize = utils::PAGE_SIZE * 8;
        let (_, io_gpu_mapping, io_gpu_pin) =
            mem::allocate_map_pin(IO_MEM_SIZE, bus_mapper).log_err("Failed suspend")?;
        let io_gpu_address =
            fw_address_space.insert_buffer_auto_address(io_gpu_pin, &access_flags, bus_mapper)?;

        Ok(Self {
            doorbell: None,
            buffer: buffer_mapping,
            buffer_gpu_address,
            io_buffer: io_gpu_mapping,
            io_gpu_address,
        })
    }

    /// This sets the interfaces correctly so this ringbuffer will run on hardware.
    fn configure_interfaces(
        &mut self,
        interface_memory: &mut mem::MappedMemory,
        interface: &interfaces::queue::Interface,
    ) -> Result<(), zx::Status> {
        let doorbell = self.doorbell.ok_or(zx::Status::BAD_STATE)?;

        // Handle the ringbuffer interface.
        let output = self.read_output();
        let mut input = self.read_input();
        input.extract_init = output.extract;
        self.write_input(input);

        // Handle the queue interface.
        let mut input = interface.read_input(interface_memory);
        input.ringbuffer_base = self.buffer_gpu_address.0;
        input.ringbuffer_size = self.buffer.size as u32;
        input.ringbuffer_input = self.io_gpu_address.0;
        input.ringbuffer_output = self.io_gpu_address.0 + 0x1000;
        input.config |= (doorbell.0 << 8) as u32;
        input.ack_irq_mask = u32::MAX;
        interface.write_input(interface_memory, &input);

        let request = interface.request();
        let mut bits = interfaces::queue::RequestField(0);
        bits.set_state(interfaces::queue::RequestFieldState::Start as u32);
        bits.set_fatal(1);
        bits.set_fault(1);
        request.set_bits(interface_memory, bits.0, bits.0);

        Ok(())
    }

    pub fn add_instructions(&mut self, instructions: &[u8]) -> Result<(), zx::Status> {
        let mut input = self.read_input();
        let output = self.read_output();

        // Insert never wraps so we have to modulo it by the size.
        let insert = input.insert as usize & (self.buffer.size - 1);

        if instructions.len() > (self.buffer.size - (input.insert - output.extract) as usize) {
            return Err(zx::Status::NO_MEMORY);
        }

        // Handle a write that breaks over the end of the ringbuffer.
        {
            let first_write_size = std::cmp::min(instructions.len(), self.buffer.size - insert);
            self.buffer.write_bytes(insert, &instructions[0..first_write_size]);

            if first_write_size < instructions.len() {
                self.buffer.write_bytes(0, &instructions[first_write_size..instructions.len()]);
            }
        }
        crate::barriers::write();

        // Update our ringbuffer insert position.
        let output = self.read_output();
        input.extract_init = output.extract;
        input.insert += instructions.len() as u64;

        self.write_input(input);
        Ok(())
    }

    pub fn kick(&self, mmio: &mut impl Mmio) -> Option<()> {
        regs::Doorbell::new(self.doorbell.as_ref()?.0).ring(mmio);
        Some(())
    }

    pub fn read_input(&self) -> RingbufferInput {
        let ptr = self.io_buffer.cpu_address.as_address();
        // SAFETY: We are sure this memory exists and is the correct size.
        unsafe { core::ptr::read_volatile(ptr as *const RingbufferInput) }
    }

    fn write_input(&mut self, input: RingbufferInput) {
        let ptr = self.io_buffer.cpu_address.as_address();
        // SAFETY: We are sure this memory exists and is the correct size.
        unsafe { core::ptr::write_volatile(ptr as *mut RingbufferInput, input) }
        crate::barriers::write();
        // TODO(https://fxbug.dev/503722844): Remove this flush.
        self.io_buffer.flush_cache_bytes(0, std::mem::size_of::<RingbufferInput>());
    }

    pub fn read_output(&self) -> RingbufferOutput {
        let ptr = self.io_buffer.cpu_address.offset(0x1000).as_address();
        // SAFETY: We are sure this memory exists and is the correct size.
        unsafe { core::ptr::read_volatile(ptr as *const RingbufferOutput) }
    }

    #[cfg(test)]
    pub fn write_output(&self, output: RingbufferOutput) {
        let ptr = self.io_buffer.cpu_address.offset(0x1000).as_address() as *mut RingbufferOutput;
        // SAFETY: We are sure this memory exists and is the correct size.
        unsafe { core::ptr::write_volatile(ptr, output) }
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct RingbufferInput {
    /// The insertion point of the ring buffer.
    /// This is where the CPU adds new instructions.
    /// When `insert == extract` the ringbuffer is empty.
    pub insert: u64,

    /// The hardware reads this value when it comes back from suspend to set `extract`.
    pub extract_init: u64,
}

#[repr(C)]
#[derive(Debug)]
pub struct RingbufferOutput {
    /// This is where the hardware is reading instructions from.
    extract: u64,

    /// This is 1 when the hardware is active.
    active: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use mem::tests::FakeBusMapper;

    #[fuchsia::test]
    fn test_ringbuffer_add_instructions() {
        let bus_mapper = FakeBusMapper::new(0x1000);
        let mut address_space =
            address_space::AddressSpace::new(address_space::Coherency::Coherent, &bus_mapper)
                .unwrap();
        let mut fw_address_space =
            address_space::AddressSpace::new(address_space::Coherency::Coherent, &bus_mapper)
                .unwrap();

        let mut ringbuffer =
            Ringbuffer::new(&mut address_space, &bus_mapper, &mut fw_address_space, 0x1000)
                .unwrap();

        let instructions = vec![1, 2, 3, 4];
        ringbuffer.add_instructions(&instructions).unwrap();

        let buffer_content = ringbuffer.buffer.as_u8();
        assert_eq!(&buffer_content[0..4], &[1, 2, 3, 4]);

        let input = ringbuffer.read_input();
        assert_eq!(input.insert, 4);
    }

    #[fuchsia::test]
    fn test_ringbuffer_wrap() {
        let bus_mapper = FakeBusMapper::new(0x1000);
        let mut address_space =
            address_space::AddressSpace::new(address_space::Coherency::Coherent, &bus_mapper)
                .unwrap();
        let mut fw_address_space =
            address_space::AddressSpace::new(address_space::Coherency::Coherent, &bus_mapper)
                .unwrap();

        let ringbuffer_size = utils::PAGE_SIZE;
        let mut ringbuffer = Ringbuffer::new(
            &mut address_space,
            &bus_mapper,
            &mut fw_address_space,
            ringbuffer_size,
        )
        .unwrap();

        // Write 4000 bytes.
        let instructions = vec![1; 4000];
        ringbuffer.add_instructions(&instructions).unwrap();

        // Simulate hardware consuming 128 bytes so we have space for the next write.
        ringbuffer.write_output(RingbufferOutput { extract: 128, active: 0 });

        // Write 200 bytes, causing a wrap of 104 bytes.
        // We've written 4200 bytes and ringbuffer has 4096.
        let instructions2 = vec![2; 200];
        ringbuffer.add_instructions(&instructions2).unwrap();

        let buffer_content = ringbuffer.buffer.as_u8();

        // Wrapped 104 bytes should be at the beginning.
        assert_eq!(&buffer_content[0..104], &[2; 104]);
        // Remaining 3896 bytes from first write.
        assert_eq!(&buffer_content[104..4000], &[1; 3896]);
        // Next 96 bytes should be 2 (filling the buffer to 4096).
        assert_eq!(&buffer_content[4000..4096], &[2; 96]);

        let input = ringbuffer.read_input();
        assert_eq!(input.insert, 4200);
    }
}
