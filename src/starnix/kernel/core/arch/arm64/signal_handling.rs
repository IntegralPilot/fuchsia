// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::signals::{SignalDetail, SignalInfo, SignalState};
use crate::task::{ArchExtendedPstateStorage, CurrentTask, Task};
use starnix_logging::{log_debug, track_stub};
use starnix_registers::{RegisterState, RegisterStorageEnum};
use starnix_types::arch::ArchWidth;
use starnix_uapi::errors::Errno;
use starnix_uapi::math::round_down_to_increment;
use starnix_uapi::signals::SigSet;
use starnix_uapi::user_address::{ArchSpecific, UserAddress};
use starnix_uapi::{
    _aarch64_ctx, ESR_MAGIC, EXTRA_MAGIC, FPSIMD_MAGIC, errno, error, esr_context, fpsimd_context,
    sigaction_t, sigaltstack, uapi,
};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, transmute_ref};

/// The size of the red zone.
pub const RED_ZONE_SIZE: u64 = 0;

const fn max_const(i1: usize, i2: usize) -> usize {
    if i2 > i1 { i2 } else { i1 }
}

/// Maximum size between the siginfo struct for 32 and 64 bits.
const SIGINFO_RESERVED_DATA_SIZE: usize = max_const(
    std::mem::size_of::<uapi::siginfo_t>(),
    std::mem::size_of::<uapi::arch32::siginfo_t>(),
);

/// Maximum size between the context struct for 32 and 64 bits.
const UCONTEXT_RESERVED_DATA_SIZE: usize =
    max_const(std::mem::size_of::<uapi::ucontext>(), std::mem::size_of::<uapi::arch32::ucontext>());

/// A `SignalStackFrame` contains all the state that is stored on the stack prior to executing a
/// signal handler. The exact layout of this structure is part of the platform's ABI.
#[repr(C, align(16))]
#[derive(Clone, IntoBytes, FromBytes, KnownLayout, Immutable)]
pub struct SignalStackFrame {
    /// Bytes for the siginfo struct relevant to the correct architecture padded with 0.
    pub siginfo_bytes: [u8; SIGINFO_RESERVED_DATA_SIZE],
    /// Bytes for the context struct relevant to the correct architecture padded with 0.
    pub context: [u8; UCONTEXT_RESERVED_DATA_SIZE],
}

/// The size, in bytes, of the signal stack frame.
pub const SIG_STACK_SIZE: usize = std::mem::size_of::<SignalStackFrame>();

impl SignalStackFrame {
    pub fn new(
        task: &Task,
        arch_width: ArchWidth,
        registers: &mut RegisterState<RegisterStorageEnum>,
        extended_pstate: &ArchExtendedPstateStorage,
        signal_state: &SignalState,
        siginfo: &SignalInfo,
        _action: sigaction_t,
        _stack_pointer: UserAddress,
    ) -> Result<SignalStackFrame, Errno> {
        let fault_address = match siginfo.detail {
            SignalDetail::SigFault { addr } => addr,
            _ => 0,
        };
        let uc_stack = signal_state
            .alt_stack
            .map(|stack| sigaltstack {
                ss_sp: stack.ss_sp.into(),
                ss_flags: stack.ss_flags as i32,
                ss_size: stack.ss_size as u64,
                ..Default::default()
            })
            .unwrap_or_default();
        let mut context = [0_u8; UCONTEXT_RESERVED_DATA_SIZE];
        if arch_width.is_arch32() {
            let mut regs = [0_u32; 21];
            // 0, 1, 2 is trap_no, error_code, oldmask, keep at 0.
            for i in 0..16 {
                regs[i + 3] = registers.r[i] as u32;
            }
            regs[19] = registers.cpsr as u32;
            regs[20] = fault_address as u32;
            let context32 = uapi::arch32::ucontext {
                uc_flags: 0,
                uc_link: Default::default(),
                uc_stack: uc_stack.try_into().map_err(|_| errno!(EINVAL))?,
                uc_sigmask64: uapi::sigset_t::from(signal_state.mask()).into(),
                uc_mcontext: zerocopy::transmute!(regs),
                extended_pstate: get_pstate_extended_data(extended_pstate), // TODO(https://fxbug.dev/486225910): This isn't right for aarch32.
                ..Default::default()
            };
            context32.write_to_prefix(&mut context).unwrap();
        } else {
            let regs = registers.r.to_vec();
            let context64 = uapi::ucontext {
                uc_flags: 0,
                uc_link: Default::default(),
                uc_stack,
                uc_sigmask: signal_state.mask().into(),
                uc_mcontext: uapi::sigcontext {
                    regs: regs.try_into().unwrap(),
                    sp: registers.sp,
                    pc: registers.pc,
                    pstate: registers.cpsr as u64,
                    fault_address,
                    __reserved: get_pstate_extended_data(extended_pstate),
                    ..Default::default()
                },
                ..Default::default()
            };
            context64.write_to_prefix(&mut context).unwrap();
        }

        let vdso_sigreturn_offset = if arch_width.is_arch32() {
            task.kernel().vdso_arch32.as_ref().unwrap().sigreturn_offset
        } else {
            task.kernel().vdso.sigreturn_offset
        };
        let sigreturn_addr = task.mm()?.state.read().vdso_base.ptr() as u64 + vdso_sigreturn_offset;
        registers.r[30] = sigreturn_addr;
        if arch_width.is_arch32() {
            registers.r[14] = registers.r[30];
        }

        Ok(SignalStackFrame { context, siginfo_bytes: siginfo.as_siginfo_bytes(arch_width)? })
    }

    pub fn as_bytes(&self) -> &[u8; SIG_STACK_SIZE] {
        zerocopy::transmute_ref!(self)
    }

    pub fn from_bytes(bytes: [u8; SIG_STACK_SIZE]) -> SignalStackFrame {
        zerocopy::transmute!(bytes)
    }

    pub fn get_signal_mask(&self, is_arch32: bool) -> SigSet {
        if is_arch32 {
            uapi::sigset_t::from(self.get_ucontext32().uc_sigmask64).into()
        } else {
            self.get_ucontext64().uc_sigmask.into()
        }
    }

    fn get_ucontext64(&self) -> &uapi::ucontext {
        uapi::ucontext::ref_from_prefix(&self.context).unwrap().0
    }

    fn get_ucontext32(&self) -> &uapi::arch32::ucontext {
        uapi::arch32::ucontext::ref_from_prefix(&self.context).unwrap().0
    }
}

pub fn restore_registers(
    current_task: &mut CurrentTask,
    signal_stack_frame: &SignalStackFrame,
    _stack_pointer: UserAddress,
) -> Result<(), Errno> {
    if current_task.is_arch32() {
        restore_registers_32(current_task, signal_stack_frame)
    } else {
        restore_registers_64(current_task, signal_stack_frame)
    }
}

fn restore_registers_32(
    current_task: &mut CurrentTask,
    signal_stack_frame: &SignalStackFrame,
) -> Result<(), Errno> {
    let uctx = &signal_stack_frame.get_ucontext32().uc_mcontext;
    let regs: &[u32; 21] = transmute_ref!(uctx);
    const NUM_REGS: usize = 31;
    let mut registers = [0u64; NUM_REGS];
    for i in 0..16 {
        registers[i] = regs[i + 3].into();
    }

    let mut restored_regs = zx::sys::zx_restricted_state_t::default();
    restored_regs.r = registers;
    restored_regs.sp = registers[13];
    restored_regs.pc = registers[15];
    restored_regs.cpsr = regs[19].into();
    restored_regs.tpidr_el0 = current_task.thread_state.registers.tpidr_el0;
    current_task.thread_state.registers.load(restored_regs);

    parse_pstate_extended_data(
        &signal_stack_frame.get_ucontext32().extended_pstate,
        &mut current_task.thread_state.extended_pstate,
    )
}

fn restore_registers_64(
    current_task: &mut CurrentTask,
    signal_stack_frame: &SignalStackFrame,
) -> Result<(), Errno> {
    let uctx = &signal_stack_frame.get_ucontext64().uc_mcontext;
    const NUM_REGS: usize = 31;
    let mut registers = [0; NUM_REGS];
    registers.copy_from_slice(&uctx.regs[..NUM_REGS]);

    // Restore the register state from before executing the signal handler.
    let mut restored_regs = zx::sys::zx_restricted_state_t::default();
    restored_regs.r = registers;
    restored_regs.sp = uctx.sp;
    restored_regs.pc = uctx.pc;
    restored_regs.cpsr = uctx.pstate as u32;
    restored_regs.tpidr_el0 = current_task.thread_state.registers.tpidr_el0;
    current_task.thread_state.registers.load(restored_regs);

    parse_pstate_extended_data(&uctx.__reserved, &mut current_task.thread_state.extended_pstate)
}

pub fn align_stack_pointer(pointer: u64) -> u64 {
    // When the stack grows down, alignment must be to the next lowest value.
    // arm64/starnix does not support upward growth.
    round_down_to_increment(pointer, 16).expect("Failed to round up stack pointer")
}

// Size of `sigcontext::__reserved`.
const SIGCONTEXT_EXTENDED_PSTATE_DATA_SIZE: usize = 4096;

// Returns the array to be saved in `sigcontext.__reserved`. It contains a sequence of sections
// each identified with a `_aarch64_ctx` header. The end is indicated with both fields in the
// header set to 0.
fn get_pstate_extended_data(
    extended_pstate: &ArchExtendedPstateStorage,
) -> [u8; SIGCONTEXT_EXTENDED_PSTATE_DATA_SIZE] {
    match extended_pstate {
        ArchExtendedPstateStorage::State64(extended_pstate) => {
            let mut result = [0u8; SIGCONTEXT_EXTENDED_PSTATE_DATA_SIZE];

            let fpsimd = fpsimd_context {
                head: _aarch64_ctx {
                    magic: FPSIMD_MAGIC,
                    size: std::mem::size_of::<fpsimd_context>() as u32,
                },
                fpsr: extended_pstate.get_arm64_fpsr(),
                fpcr: extended_pstate.get_arm64_fpcr(),
                vregs: *extended_pstate.get_arm64_qregs(),
            };
            let _ = fpsimd.write_to_prefix(&mut result);

            // TODO(b/313465152): Save ESR with `esr_context` and `ESR_MAGIC`. The register is read-only,
            // but the signal handler may still need to read it from `sigcontext`.
            result
        }
        ArchExtendedPstateStorage::State32(_extended_aarch32_pstate) => {
            let result = [0u8; SIGCONTEXT_EXTENDED_PSTATE_DATA_SIZE];
            // TODO(https://fxbug.dev/486225910): We should parse aarch32 specific data.
            result
        }
    }
}

fn parse_pstate_extended_data(
    data: &[u8; SIGCONTEXT_EXTENDED_PSTATE_DATA_SIZE],
    extended_pstate: &mut ArchExtendedPstateStorage,
) -> Result<(), Errno> {
    const FPSIMD_CONTEXT_SIZE: u32 = std::mem::size_of::<fpsimd_context>() as u32;
    const ESR_CONTEXT_SIZE: u32 = std::mem::size_of::<esr_context>() as u32;

    match extended_pstate {
        ArchExtendedPstateStorage::State64(extended_pstate) => {
            let mut found_fpsimd = false;
            let mut offset: usize = 0;
            loop {
                match _aarch64_ctx::read_from_prefix(&data[offset..]) {
                    Ok((_aarch64_ctx { magic: 0, size: 0 }, _)) => break,

                    Ok((_aarch64_ctx { magic: FPSIMD_MAGIC, size: FPSIMD_CONTEXT_SIZE }, _))
                        if found_fpsimd =>
                    {
                        log_debug!("Found duplicate `fpsimd_context` in `sigcontext`");
                        return error!(EINVAL);
                    }

                    Ok((_aarch64_ctx { magic: FPSIMD_MAGIC, size: FPSIMD_CONTEXT_SIZE }, _)) => {
                        found_fpsimd = true;

                        // Set Q registers.
                        let (fpsimd, _) = fpsimd_context::read_from_prefix(&data[offset..])
                            .expect("Failed to get fpsimd_context from array");
                        extended_pstate.set_arm64_state(&fpsimd.vregs, fpsimd.fpsr, fpsimd.fpcr);

                        offset += FPSIMD_CONTEXT_SIZE as usize;
                    }

                    Ok((_aarch64_ctx { magic: FPSIMD_MAGIC, size }, _)) => {
                        log_debug!("Invalid size for `fpsimd_context` in `sigcontext`: {}", size);
                        return error!(EINVAL);
                    }

                    Ok((_aarch64_ctx { magic: ESR_MAGIC, size: ESR_CONTEXT_SIZE }, _)) => {
                        // ESR register is read-only so we can skip it.
                        offset += ESR_CONTEXT_SIZE as usize;
                    }

                    Ok((_aarch64_ctx { magic: ESR_MAGIC, size }, _)) => {
                        log_debug!("Invalid size for `fpsimd_context` in `sigcontext`: {}", size);
                        return error!(EINVAL);
                    }

                    Ok((_aarch64_ctx { magic: EXTRA_MAGIC, size }, _)) => {
                        if size as usize <= std::mem::size_of::<_aarch64_ctx>() {
                            log_debug!("Invalid size for `EXTRA_MAGIC` section in `sigcontext`");
                            return error!(EINVAL);
                        }

                        track_stub!(TODO("https://fxbug.dev/322873793"), "sigcontext EXTRA_MAGIC");
                        offset += ESR_CONTEXT_SIZE as usize;
                    }

                    Ok((_aarch64_ctx { magic, size }, _)) => {
                        log_debug!(
                            "Unrecognized sectionin `sigcontext` (magic: 0x{:x}. size: {})",
                            magic,
                            size
                        );
                        return error!(EINVAL);
                    }

                    Err(_) => return error!(EINVAL),
                };
            }

            if !found_fpsimd {
                log_debug!("Couldn't find `fpsimd_context` in `sigcontext`");
                return error!(EINVAL);
            }
        }
        ArchExtendedPstateStorage::State32(_extended_aarch32_pstate) => {
            // TODO(https://fxbug.dev/486225910): We should parse aarch32 specific data.
        }
    }

    Ok(())
}
