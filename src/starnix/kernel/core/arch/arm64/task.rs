// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::MemoryAccessorExt;
use crate::signals::{SignalDetail, SignalInfo};
use crate::task::{CurrentTask, ExceptionResult, PageFaultExceptionReport};
use starnix_uapi::signals::{SIGBUS, SIGFPE, SIGILL, SIGTRAP};
use starnix_uapi::user_address::{ArchSpecific, UserAddress};

// On ARM32 Linux, some undefined instructions are treated as software breakpoints.
// Read the instruction that caused the exception to handle it appropriately.
fn is_arm32_breakpoint(current_task: &CurrentTask) -> bool {
    if current_task.thread_state.arch_width().is_arch32() {
        let ip = current_task.thread_state.registers.instruction_pointer_register();
        let user_addr = UserAddress::from(ip);

        if current_task.thread_state.registers.is_thumb() {
            // Read 2 bytes first to check the narrow Thumb instruction.
            if let Ok(insn_bytes_16) = current_task.read_memory_to_array::<2>(user_addr) {
                let insn_u16 = u16::from_le_bytes(insn_bytes_16);
                if insn_u16 == 0xde01 {
                    return true;
                }

                // Next, read 4 bytes to check the wide Thumb instruction.
                if let Ok(insn_bytes_32) = current_task.read_memory_to_array::<4>(user_addr) {
                    let insn_u32 = u32::from_le_bytes(insn_bytes_32);
                    if insn_u32 == 0xa000f7f0 {
                        return true;
                    }
                }
            }
        } else {
            if let Ok(insn_bytes_32) = current_task.read_memory_to_array::<4>(user_addr) {
                let insn_u32 = u32::from_le_bytes(insn_bytes_32);
                if insn_u32 == 0xe7f001f0 {
                    return true;
                }
            }
        }
    }
    false
}

pub fn handle_hardware_exception(
    current_task: &CurrentTask,
    report: &zx::ExceptionReport,
) -> Option<ExceptionResult> {
    let ip = current_task.thread_state.registers.instruction_pointer_register();
    match report.ty {
        zx::ExceptionType::General => match get_ec_from_exception_context(&report.arch) {
            // Floating point exception.
            0b101000 | 0b101100 => Some(ExceptionResult::Signal(SignalInfo::with_detail(
                SIGFPE,
                linux_uapi::FPE_FLTINV as i32,
                SignalDetail::SigFault { addr: ip },
            ))),
            _ => None,
        },
        zx::ExceptionType::FatalPageFault { status } => {
            let decoded = decode_page_fault_exception_report(&report.arch);
            Some(current_task.handle_page_fault(decoded, status))
        }
        zx::ExceptionType::UndefinedInstruction => {
            if is_arm32_breakpoint(current_task) {
                Some(ExceptionResult::Signal(SignalInfo::with_detail(
                    SIGTRAP,
                    linux_uapi::TRAP_BRKPT as i32,
                    SignalDetail::SigFault { addr: ip },
                )))
            } else {
                Some(ExceptionResult::Signal(SignalInfo::with_detail(
                    SIGILL,
                    linux_uapi::ILL_ILLOPC as i32,
                    SignalDetail::SigFault { addr: ip },
                )))
            }
        }
        zx::ExceptionType::UnalignedAccess => {
            Some(ExceptionResult::Signal(SignalInfo::with_detail(
                SIGBUS,
                linux_uapi::BUS_ADRALN as i32,
                SignalDetail::SigFault { addr: report.arch.far },
            )))
        }
        zx::ExceptionType::SoftwareBreakpoint => {
            Some(ExceptionResult::Signal(SignalInfo::with_detail(
                SIGTRAP,
                linux_uapi::TRAP_BRKPT as i32,
                SignalDetail::SigFault { addr: ip },
            )))
        }
        zx::ExceptionType::HardwareBreakpoint => {
            Some(ExceptionResult::Signal(SignalInfo::with_detail(
                SIGTRAP,
                linux_uapi::TRAP_HWBKPT as i32,
                SignalDetail::SigFault { addr: ip },
            )))
        }
        _ => None,
    }
}

// See https://developer.arm.com/documentation/ddi0601/2022-03/AArch64-Registers/ESR-EL1--Exception-Syndrome-Register--EL1-
// for details about the values used in this file.

// Returns "Exception Class" from the exception context.
fn get_ec_from_exception_context(arch: &zx::ExceptionArchData) -> u8 {
    // Exception Class is bits 26-31 (inclusive).
    ((arch.esr >> 26) & 0b111111u32) as u8
}

// Returns "Instruction Specific Syndrome" from the exception context.
fn get_iss_from_exception_context(arch: &zx::ExceptionArchData) -> u32 {
    // ISS is bits 0-24 (inclusive).
    arch.esr & 0xffffffu32
}

pub fn decode_page_fault_exception_report(
    arch: &zx::ExceptionArchData,
) -> PageFaultExceptionReport {
    let faulting_address = arch.far;

    let ec = get_ec_from_exception_context(arch);
    let iss = get_iss_from_exception_context(arch);

    let is_execute = ec == 0b100000 || ec == 0b100001; // Instruction abort exceptions.
    let data_abort = ec == 0b100100 || ec == 0b100101; // Data abort exceptions.

    // Data Fault Status Code or Instruction Fault Status Code (bits [0:5] of ISS).
    let dfsc = iss & 0b111111;

    // Translation faults, level 0-3.
    let not_present =
        (dfsc == 0b000100) || (dfsc == 0b000101) || (dfsc == 0b000110) || (dfsc == 0b000111);

    // Note that the Zircon exception handler arm64_data_abort_handler() adds some
    // extra checking of the "cache maintenance" bit which causes it to treat more
    // things as reads. We may need similar handling.
    let is_write = data_abort && (arch.esr & 0b1000000 != 0); // WnR "write not read" = bit 6.

    PageFaultExceptionReport { faulting_address, not_present, is_write, is_execute }
}
