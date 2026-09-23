// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::utils::LogError;
use crate::{mem, regs, utils};
use mmio::{Mmio, MmioExt, ReadableRegister, WritableRegister};

pub fn power_on_l2(mmio: &mut impl Mmio) -> Result<(), zx::Status> {
    // These hash values were pulled from the device tree values for our hardware.
    regs::AddressSpaceHash0(0x36db6d).write(mmio);
    regs::AddressSpaceHash1(0x5b6db6).write(mmio);
    regs::AddressSpaceHash2(0).write(mmio);

    let mut l2_config = regs::L2Config(0);
    l2_config.set_cache_size(0x14);
    l2_config.set_hash_enable(1);
    l2_config.write(mmio);

    regs::L2Power::enable().write(mmio);

    // TODO(https://fxbug.dev/498571172): Instead of this sleep we could make this function async
    // on GpuIrq::power_changed_all. Consider doing that if powering on time becomes a problem.
    std::thread::sleep(std::time::Duration::from_millis(100));

    if regs::L2Ready::read(mmio).enabled() == 0 {
        log::error!("Failed to power on L2");
        return Err(zx::Status::INTERNAL);
    }
    Ok(())
}

pub fn request_device_reset(mmio: &mut impl Mmio) {
    let mut reset_complete = regs::GpuIrqMask(0);
    reset_complete.set_reset_completed(1);

    regs::GpuIrqClear(reset_complete.0).write(mmio);

    mmio.update_reg::<regs::GpuIrqMask, _>(|reg| {
        reg.set_reset_completed(1);
    });

    regs::GpuCommand::soft_reset().write(mmio);
}

pub fn clear_interrupts(mmio: &mut impl Mmio) {
    regs::GpuIrqClear(0xffffffff).write(mmio);
}

pub fn enable_interrupts(mmio: &mut impl Mmio) {
    let mut gpu = regs::GpuIrqMask(0xffffffff);
    // At the moment we don't need doorbells to trigger IRQs and setting this
    // creates an infinite IRQ storm.
    gpu.set_doorbell_mirror(0);
    gpu.write(mmio);
    regs::MmuIrqMask(0xffffffff).write(mmio);
    regs::JobIrqMask(0xffffffff).write(mmio);
}

pub fn initialize(mmio: &mut impl Mmio) -> Result<(), zx::Status> {
    regs::ShaderConfig(0).write(mmio);
    regs::TilerConfig(0).write(mmio);
    regs::L2MmuConfig(0).write(mmio);
    regs::CsfConfig(0).write(mmio);
    for address_space_number in 0..7 {
        disable_address_space(mmio, address_space_number)?;
    }
    Ok(())
}

pub fn enable_address_space(
    mmio: &mut impl Mmio,
    address_space_number: u64,
    translation_table_physical_address: u64,
    translation_config: u64,
    memory_attribute: regs::MemoryAttribute,
) -> Result<(), zx::Status> {
    let regs = regs::AddressSpaceRegs::new(address_space_number);
    let status = regs.read_status(mmio);
    if status.active() == 1 {
        log::error!("Address space {} is already active", address_space_number);
        return Err(zx::Status::INVALID_ARGS);
    }

    regs.write_command(mmio, regs::MmuCommand::FlushMemory);
    regs.write_translation_table(mmio, translation_table_physical_address);
    regs.write_translation_config(mmio, translation_config);
    regs.write_memory_attribute(mmio, memory_attribute);

    utils::do_until(0..100, |_| (regs.read_status(mmio).active() == 0).then_some(()))
        .ok_or(zx::Status::INTERNAL)
        .log_err("Failed to set address_space")?;

    regs.write_command(mmio, regs::MmuCommand::Update);
    utils::do_until(0..100, |_| (regs.read_status(mmio).active() == 0).then_some(()))
        .ok_or(zx::Status::INTERNAL)
        .log_err("Failed to set address_space")?;

    Ok(())
}

pub fn disable_address_space(
    mmio: &mut impl Mmio,
    address_space_number: u64,
) -> Result<(), zx::Status> {
    let regs = regs::AddressSpaceRegs::new(address_space_number);
    regs.write_translation_config(mmio, 0x42000001);
    regs.write_translation_table(mmio, 0x0);
    regs.write_memory_attribute(mmio, regs::MemoryAttribute(0));
    regs.write_command(mmio, regs::MmuCommand::Update);
    utils::do_until(0..100, |_| (regs.read_status(mmio).0 == 0).then_some(()))
        .ok_or(zx::Status::INTERNAL)
        .log_err("Failed to reset address space")?;
    Ok(())
}

pub fn ringbuffer_instructions_store_data_for_test(
    address: &mem::GpuAddress,
    data: u64,
) -> Vec<u8> {
    const OPCODE_MOV48: u64 = 0x1;
    const OPCODE_STORE_MULTIPLE: u64 = 0x15;

    let mut instructions: Vec<u8> = vec![];

    // Load r64-r65 with our address we are storing data in.
    let reg_num = 64;
    let immediate = address.0;
    let mov48: u64 = OPCODE_MOV48 << 56 | reg_num << 48 | immediate;
    instructions.extend_from_slice(&mov48.to_le_bytes());

    // Load our data into r66.
    let reg_num = 66;
    let immediate = data;
    let mov48: u64 = OPCODE_MOV48 << 56 | reg_num << 48 | immediate;
    instructions.extend_from_slice(&mov48.to_le_bytes());

    // Store the data in r66 in the address from r64-r65.
    let register_bitmap = 1;
    let data_register = 66;
    let address_register = 64;
    let offset = 0;
    let store_multiple: u64 = OPCODE_STORE_MULTIPLE << 56
        | data_register << 48
        | address_register << 40
        | register_bitmap << 16
        | offset;
    instructions.extend_from_slice(&store_multiple.to_le_bytes());

    instructions
}

#[cfg(test)]
mod tests {
    use super::*;
    use mock_mmio::{MockMemoryOps, new_mock_mmio};
    use mockall::predicate::eq;

    const GPU_IRQ_CLEAR_REG: u64 = 0x24;
    const GPU_IRQ_MASK_REG: u64 = 0x28;
    const GPU_COMMAND_REG: u64 = 0x30;

    enum Op {
        R32,
        W32,
    }

    type Operation = (Op, u64, u64);

    fn set_expect(mock: &mut MockMemoryOps, ops: &[Operation]) {
        for (op, offset, value) in ops {
            match op {
                Op::R32 => {
                    mock.expect_load32().with(eq(*offset as usize)).return_const(*value as u32);
                }
                Op::W32 => {
                    mock.expect_store32()
                        .with(eq(*offset as usize), eq(*value as u32))
                        .return_const(());
                }
            }
        }
    }

    #[fuchsia::test]
    fn test_clear_interrupts() {
        let mut ops = MockMemoryOps::new();
        set_expect(&mut ops, &[(Op::W32, GPU_IRQ_CLEAR_REG, 0xffffffff)]);
        clear_interrupts(&mut new_mock_mmio(&ops, 1024));
    }

    #[fuchsia::test]
    fn test_request_device_reset() {
        let mut ops = MockMemoryOps::new();
        set_expect(
            &mut ops,
            &[
                (Op::W32, GPU_IRQ_CLEAR_REG, 1 << 8),
                (Op::R32, GPU_IRQ_MASK_REG, 0),
                (Op::W32, GPU_IRQ_MASK_REG, 1 << 8),
                (Op::W32, GPU_COMMAND_REG, 0x101),
            ],
        );

        request_device_reset(&mut new_mock_mmio(&ops, 1024));
    }
}
