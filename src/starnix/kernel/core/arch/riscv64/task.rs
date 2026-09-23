// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::signals::{SignalDetail, SignalInfo};
use crate::task::{CurrentTask, ExceptionResult, PageFaultExceptionReport};
use starnix_uapi::signals::{SIGBUS, SIGILL, SIGTRAP};

// See "4.1.8 Supervisor Cause Register" in "The RISC-V Instruction Set Manual, Volume II:
// Privileged Architecture".
const RISCV64_EXCEPTION_STORE_PAGE_FAULT: u64 = 15;
const RISCV64_EXCEPTION_INSTRUCTION_PAGE_FAULT: u64 = 12;

pub fn decode_page_fault_exception_report(
    arch: &zx::ExceptionArchData,
) -> PageFaultExceptionReport {
    let faulting_address = arch.tval;

    // TODO(https://fxbug.dev/42079018): Is there a way to distinguish access and page-not-present faults?
    let not_present = true;

    let is_write = arch.cause == RISCV64_EXCEPTION_STORE_PAGE_FAULT;
    let is_execute = arch.cause == RISCV64_EXCEPTION_INSTRUCTION_PAGE_FAULT;

    PageFaultExceptionReport { faulting_address, not_present, is_write, is_execute }
}

pub fn handle_hardware_exception(
    current_task: &CurrentTask,
    report: &zx::ExceptionReport,
) -> Option<ExceptionResult> {
    let ip = current_task.thread_state.registers.instruction_pointer_register();
    match report.ty {
        zx::ExceptionType::General => {
            // TODO(https://fxbug.dev/42079018) Return SIGFPE for FP exceptions.
            None
        }
        zx::ExceptionType::FatalPageFault { status } => {
            let decoded = decode_page_fault_exception_report(&report.arch);
            Some(current_task.handle_page_fault(decoded, status))
        }
        zx::ExceptionType::UndefinedInstruction => {
            Some(ExceptionResult::Signal(SignalInfo::with_detail(
                SIGILL,
                linux_uapi::ILL_ILLOPC as i32,
                SignalDetail::SigFault { addr: ip },
            )))
        }
        zx::ExceptionType::UnalignedAccess => {
            Some(ExceptionResult::Signal(SignalInfo::with_detail(
                SIGBUS,
                linux_uapi::BUS_ADRALN as i32,
                SignalDetail::SigFault { addr: report.arch.tval },
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
