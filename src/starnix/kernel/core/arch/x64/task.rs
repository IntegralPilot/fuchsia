// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::signals::{SignalDetail, SignalInfo};
use crate::task::{CurrentTask, ExceptionResult, PageFaultExceptionReport};
use starnix_uapi::signals::{SIGBUS, SIGFPE, SIGILL, SIGSEGV, SIGTRAP};

pub fn handle_hardware_exception(
    current_task: &CurrentTask,
    report: &zx::ExceptionReport,
) -> Option<ExceptionResult> {
    let ip = current_task.thread_state.registers.instruction_pointer_register();
    match report.ty {
        // See Intel® 64 and IA-32 Architectures Software Developer's Manual, Volume 3, Chapter 6
        // (Interrupt and exception handling).
        zx::ExceptionType::General => match report.arch.vector {
            // 0: Division by 0.
            0 => Some(ExceptionResult::Signal(SignalInfo::with_detail(
                SIGFPE,
                linux_uapi::FPE_INTDIV as i32,
                SignalDetail::SigFault { addr: ip },
            ))),

            // 16: FPU exception.
            // 19: SSE exception.
            16 | 19 => Some(ExceptionResult::Signal(SignalInfo::with_detail(
                SIGFPE,
                linux_uapi::FPE_FLTINV as i32,
                SignalDetail::SigFault { addr: ip },
            ))),

            // 13: General Protection Fault, e.g. `hlt` instruction.
            13 => Some(ExceptionResult::Signal(SignalInfo::kernel(SIGSEGV))),

            _ => None,
        },
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
                SignalDetail::SigFault { addr: report.arch.cr2 },
            )))
        }
        zx::ExceptionType::SoftwareBreakpoint => {
            // When generating a software breakpoint, x86 deviates from other
            // architectures, returns SI_KERNEL and does not populate si_addr.
            Some(ExceptionResult::Signal(SignalInfo::kernel(SIGTRAP)))
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

pub fn decode_page_fault_exception_report(
    data: &zx::ExceptionArchData,
) -> PageFaultExceptionReport {
    // [intel/vol3]: 6.15: Interrupt 14--Page-Fault Exception (#PF)
    let faulting_address = data.cr2;
    let not_present = data.err_code & 0x01 == 0; // Low bit means "present"
    let is_write = data.err_code & 0x02 != 0;
    let is_execute = data.err_code & 0xF0 != 0;

    PageFaultExceptionReport { faulting_address, not_present, is_write, is_execute }
}
