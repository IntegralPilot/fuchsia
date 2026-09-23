// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::x86::{inp, inpd, inpw, outp, outpd, outpw};
use crate::platform_pc::acpi::global_acpi_lite_parser;
use debug::dprintf;
use zx_status::Status;

// PM1 control register constants. See ACPI v6.3 Section 4.8.3.2.1
const K_BITPOSITION_SLEEP_TYPE: u8 = 0x0A;
const K_BITMASK_SLEEP_TYPE: u16 = 0x1C00; // Bits 10-12
const K_BITMASK_SLEEP_ENABLE: u16 = 0x2000; // Bit 13
const K_BITMASK_PM1_CNT_WRITEONLY: u32 = 0x2004; // Bits 2, 13

// PM1 status register constants. See ACPI v6.3 Section 4.8.3.1.1
const K_BITPOSITION_WAKE_STATUS: u8 = 0x0F;
const K_BITMASK_WAKE_STATUS: u16 = 0x8000; // Bit 15

const K_ADR_SPACE_SYSTEM_IO: u8 = 1;
const K_MAX_IO_BIT_WIDTH: u8 = 32;
const K_MAX_IO_PORT: u64 = u16::MAX as u64;

// Assumes IO port address space which has a maximum access bit width of 32. Assumes the GAS format
// used by FADT which ignores access_size and instead uses register_bit_width to determine the
// access bit width. Although this behaviour is not in documentation, it can be observed in
// hardware.
fn get_access_bit_width(reg: &acpi_lite::structures::AcpiGenericAddress) -> u8 {
    if reg.register_bit_width < K_MAX_IO_BIT_WIDTH {
        reg.register_bit_width
    } else {
        K_MAX_IO_BIT_WIDTH
    }
}

// Check that the register uses IO port address space and is valid for the GAS format used by the
// FADT.
fn validate_register(reg: &acpi_lite::structures::AcpiGenericAddress) -> Result<(), Status> {
    if reg.address == 0 {
        return Err(Status::INVALID_ARGS);
    }

    if reg.address_space_id != K_ADR_SPACE_SYSTEM_IO {
        dprintf!(INFO, "Unsupported register address space: {}.\n", reg.address_space_id);
        return Err(Status::NOT_SUPPORTED);
    }

    // Validate that the register is in the legacy GAS format used by the FADT, indicated by a
    // bit_offset of 0 and register_bit_width of 8/16/32/64. Although this behaviour is not in
    // documentation, it can be observed in hardware.
    if reg.register_bit_offset != 0
        || !reg.register_bit_width.is_power_of_two()
        || reg.register_bit_width < 8
        || reg.register_bit_width > 64
    {
        dprintf!(INFO, "Register is not in the GAS format used by the FADT.\n");
        return Err(Status::NOT_SUPPORTED);
    }

    Ok(())
}

fn read_io_port(address: u64, width: u32) -> Result<u32, Status> {
    if address > K_MAX_IO_PORT {
        dprintf!(
            INFO,
            "Unable to read IO port. Requested address ({}) greater than maximum IO port.\n",
            address
        );
        return Err(Status::INVALID_ARGS);
    }

    let io_port = address as u16;

    // SAFETY: We validate that `io_port` is within `K_MAX_IO_PORT`. Reading from hardware I/O ports
    // is safe under this constraint because the address has been validated to be within the max
    // allowed I/O range, and the caller guarantees that accessing these standard PM registers does not
    // violate platform stability.
    unsafe {
        match width {
            8 => Ok(inp(io_port) as u32),
            16 => Ok(inpw(io_port) as u32),
            32 => Ok(inpd(io_port)),
            _ => {
                dprintf!(INFO, "Unable to read IO port. Invalid width requested: {}.\n", width);
                Err(Status::INVALID_ARGS)
            }
        }
    }
}

fn write_io_port(address: u64, width: u32, value: u32) -> Result<(), Status> {
    if address > K_MAX_IO_PORT {
        dprintf!(INFO, "Unable to write IO port. Invalid address: {}.\n", address);
        return Err(Status::INVALID_ARGS);
    }

    let io_port = address as u16;

    // SAFETY: We validate that `io_port` is within `K_MAX_IO_PORT`. Writing to hardware I/O ports
    // is safe under this constraint because the address has been validated to be within the max
    // allowed I/O range, and the caller guarantees that accessing these standard PM registers does not
    // violate platform stability.
    unsafe {
        match width {
            8 => {
                outp(io_port, value as u8);
                Ok(())
            }
            16 => {
                outpw(io_port, value as u16);
                Ok(())
            }
            32 => {
                outpd(io_port, value);
                Ok(())
            }
            _ => {
                dprintf!(INFO, "Unable to write IO port. Invalid width requested: {}.\n", width);
                Err(Status::INVALID_ARGS)
            }
        }
    }
}

fn read_register(reg: &acpi_lite::structures::AcpiGenericAddress) -> Result<u64, Status> {
    validate_register(reg)?;

    let mut value = 0u64;
    let access_width = get_access_bit_width(reg);
    let mut bits_to_read = reg.register_bit_width as u32;
    let mut index = 0;

    // Read |bits_to_read| bits from |reg->address| in chunks of |access_width| bits.
    while bits_to_read > 0 {
        let address = reg.address + (index * ((access_width >> 3) as u32)) as u64;
        let ioport_read_value = read_io_port(address, access_width as u32)?;

        let shift = index * (access_width as u32);
        if shift < 64 {
            let mask = (1u64 << access_width) - 1;
            value |= ((ioport_read_value as u64) & mask) << shift;
        }

        bits_to_read = bits_to_read.saturating_sub(access_width as u32);
        index += 1;
    }
    Ok(value)
}

fn write_register(
    reg: &acpi_lite::structures::AcpiGenericAddress,
    value: u64,
) -> Result<(), Status> {
    validate_register(reg)?;

    let access_width = get_access_bit_width(reg);
    let mut bits_to_write = reg.register_bit_width as u32;
    let mut index = 0;

    // Write |bits_to_write| bits to |reg->address| in chunks of |access_width| bits.
    while bits_to_write > 0 {
        // |access_width| is 32 at most so we can safely cast to uint32_t
        let shift = index * (access_width as u32);
        let write_bits =
            if shift >= 64 { 0 } else { ((value >> shift) & ((1u64 << access_width) - 1)) as u32 };

        let address = reg.address + (index * ((access_width >> 3) as u32)) as u64;
        write_io_port(address, access_width as u32, write_bits)?;

        bits_to_write = bits_to_write.saturating_sub(access_width as u32);
        index += 1;
    }
    Ok(())
}

fn read_ab_register(
    reg_a: &acpi_lite::structures::AcpiGenericAddress,
    reg_b: &acpi_lite::structures::AcpiGenericAddress,
) -> Result<u64, Status> {
    let value_a = read_register(reg_a)?;
    let value_b = if reg_b.address != 0 { read_register(reg_b)? } else { 0 };
    Ok(value_a | value_b)
}

fn write_ab_register(
    reg_a: &acpi_lite::structures::AcpiGenericAddress,
    reg_b: &acpi_lite::structures::AcpiGenericAddress,
    value_a: u64,
    value_b: u64,
) -> Result<(), Status> {
    write_register(reg_a, value_a).map_err(|_| Status::INTERNAL)?;
    if reg_b.address != 0 {
        write_register(reg_b, value_b).map_err(|_| Status::INTERNAL)?;
    }
    Ok(())
}

/// Enter a system sleep state via the FADT PM registers.
///
/// This function puts the system in a sleep state using the ACPI FADT PM registers. It is intended
/// to replace the second half of the ACPICA function AcpiHwLegacySleep which must be called with
/// interrupts disabled, however in Fuchsia ACPICA runs in usermode and there is no mechanism for it
/// to disable interrupts. Instead it calls on the kernel which first disables interrupts before
/// calling on this function to transition the system to the sleep state. The function returns ZX_OK
/// when the system wakes.
///
/// # Safety
///
/// The caller must ensure that:
/// 1. Interrupts are disabled prior to calling this function.
/// 2. The global ACPI parser state has been initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn set_suspend_registers(
    sleep_state: u8,
    sleep_type_a: u8,
    sleep_type_b: u8,
) -> zx_types::zx_status_t {
    let _ = sleep_state;
    assert!(crate::arch_rs::ints_disabled());

    let acpi_fadt = match acpi_lite::get_table_by_type::<acpi_lite::structures::AcpiFadt>(
        global_acpi_lite_parser(),
    ) {
        Some(fadt) => fadt,
        None => {
            dprintf!(INFO, "Failed to get FADT.\n");
            return Status::INTERNAL.into_raw();
        }
    };

    // Read PM1 control register
    let mut pm1a_control =
        match read_ab_register(&acpi_fadt.x_pm1a_cnt_blk, &acpi_fadt.x_pm1b_cnt_blk) {
            Ok(val) => val,
            Err(_) => {
                dprintf!(INFO, "Failed to read PM1 control register.\n");
                return Status::INTERNAL.into_raw();
            }
        };

    // Clear the write-only bits
    pm1a_control &= !(K_BITMASK_PM1_CNT_WRITEONLY as u64);
    // Clear the sleep type and sleep enable bits
    pm1a_control &= !((K_BITMASK_SLEEP_TYPE | K_BITMASK_SLEEP_ENABLE) as u64);
    // Set the sleep type bits and write them to the registers
    let mut pm1b_control = pm1a_control;
    pm1a_control |= (sleep_type_a as u64) << K_BITPOSITION_SLEEP_TYPE;
    pm1b_control |= (sleep_type_b as u64) << K_BITPOSITION_SLEEP_TYPE;

    if write_ab_register(
        &acpi_fadt.x_pm1a_cnt_blk,
        &acpi_fadt.x_pm1b_cnt_blk,
        pm1a_control,
        pm1b_control,
    )
    .is_err()
    {
        dprintf!(INFO, "Failed to write sleep type to PM1 control register.\n");
        return Status::INTERNAL.into_raw();
    }

    // Flush CPU cache
    // SAFETY: Executing `wbinvd` is safe during sleep state transitions to ensure cache coherency
    // before the system power is cut or altered.
    unsafe {
        core::arch::asm!("wbinvd", options(nostack, preserves_flags));
    }

    // Add the sleep enable bit and write to the registers
    pm1a_control |= K_BITMASK_SLEEP_ENABLE as u64;
    pm1b_control |= K_BITMASK_SLEEP_ENABLE as u64;

    if write_ab_register(
        &acpi_fadt.x_pm1a_cnt_blk,
        &acpi_fadt.x_pm1b_cnt_blk,
        pm1a_control,
        pm1b_control,
    )
    .is_err()
    {
        dprintf!(INFO, "Failed to write sleep type and sleep enable to PM1 control register.\n");
        return Status::INTERNAL.into_raw();
    }

    // Wait for resume
    loop {
        let wake_status =
            match read_ab_register(&acpi_fadt.x_pm1a_evt_blk, &acpi_fadt.x_pm1b_evt_blk) {
                Ok(val) => val,
                Err(_) => {
                    dprintf!(INFO, "Failed to read wake status from PM1 event register.\n");
                    return Status::INTERNAL.into_raw();
                }
            };
        let wake_status =
            (wake_status & (K_BITMASK_WAKE_STATUS as u64)) >> K_BITPOSITION_WAKE_STATUS;
        if wake_status != 0 {
            break;
        }
    }

    zx_status::sys::ZX_OK
}
