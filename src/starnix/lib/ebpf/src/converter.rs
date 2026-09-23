// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use linux_uapi::sock_filter;
use std::collections::HashMap;

use crate::EbpfError;
use crate::EbpfError::*;
use crate::api::{
    BPF_A, BPF_ABS, BPF_ADD, BPF_ALU, BPF_ALU64, BPF_AND, BPF_B, BPF_DIV, BPF_EXIT, BPF_H, BPF_IMM,
    BPF_IND, BPF_JA, BPF_JEQ, BPF_JGE, BPF_JGT, BPF_JLE, BPF_JLT, BPF_JMP, BPF_JMP32, BPF_JNE,
    BPF_JSET, BPF_K, BPF_LD, BPF_LDX, BPF_LEN, BPF_LSH, BPF_MEM, BPF_MISC, BPF_MOD, BPF_MOV,
    BPF_MSH, BPF_MUL, BPF_NEG, BPF_OR, BPF_RET, BPF_RSH, BPF_ST, BPF_STX, BPF_SUB, BPF_TAX,
    BPF_TXA, BPF_W, BPF_X, BPF_XOR, EbpfInstruction,
};
use crate::program::{
    BpfProgramContext, EbpfProgram, ProgramArgument, StaticHelperSet, link_program,
};
use crate::verifier::{
    CallingContext, NullVerifierLogger, Type, VerifiedEbpfProgram, verify_program,
};

const CBPF_WORD_SIZE: u32 = 4;

// cBPF supports 16 words for scratch memory.
const CBPF_SCRATCH_SIZE: u32 = 16;

pub enum CbpfLenInstruction {
    Static { len: i32 },
    ContextField { offset: i16 },
}

pub struct CbpfConfig {
    pub len: CbpfLenInstruction,
    pub allow_msh: bool,
}

// These are accessors for bits in an BPF/EBPF instruction.
// Instructions are encoded in one byte.  The first 3 LSB represent
// the operation, and the other bits represent various modifiers.
// Brief comments are given to indicate what the functions broadly
// represent, but for the gory detail, consult a detailed guide to
// BPF, like the one at https://docs.kernel.org/bpf/instruction-set.html

/// The bpf_class is the instruction type.(e.g., load/store/jump/ALU).
pub fn bpf_class(filter: &sock_filter) -> u8 {
    (filter.code & 0x07) as u8
}

/// The bpf_size is the 4th and 5th bit of load and store
/// instructions.  It indicates the bit width of the load / store
/// target (8, 16, 32, 64 bits).
pub fn bpf_size(filter: &sock_filter) -> u8 {
    (filter.code & 0x18) as u8
}

/// The addressing mode is the most significant three bits of load and
/// store instructions.  They indicate whether the instrution accesses a
/// constant, accesses from memory, or accesses from memory atomically.
pub fn bpf_addressing_mode(filter: &sock_filter) -> u8 {
    (filter.code & 0xe0) as u8
}

/// Modifiers for jumps and alu operations.  For example, a jump can
/// be jeq, jtl, etc.  An ALU operation can be plus, minus, divide,
/// etc.
fn bpf_op(filter: &sock_filter) -> u8 {
    (filter.code & 0xf0) as u8
}

/// The source for the operation (either a register or an immediate).
fn bpf_src(filter: &sock_filter) -> u8 {
    (filter.code & 0x08) as u8
}

/// Similar to bpf_src, but also allows BPF_A - used for RET.
fn bpf_rval(filter: &sock_filter) -> u8 {
    (filter.code & 0x18) as u8
}

/// Returns offset for the scratch memory with the specified address.
fn cbpf_scratch_offset(addr: u32) -> Result<i16, EbpfError> {
    if addr < CBPF_SCRATCH_SIZE {
        Ok((-(CBPF_SCRATCH_SIZE as i16) + addr as i16) * CBPF_WORD_SIZE as i16)
    } else {
        Err(EbpfError::InvalidCbpfScratchOffset(addr))
    }
}

/// Transforms a program in classic BPF (cbpf, as stored in struct
/// sock_filter) to extended BPF (stored as `EbpfInstruction`).
/// The bpf_code parameter is kept as an array for easy transfer
/// via FFI.  This currently only allows the subset of BPF permitted
/// by seccomp(2).
fn cbpf_to_ebpf(
    bpf_code: &[sock_filter],
    config: &CbpfConfig,
) -> Result<Vec<EbpfInstruction>, EbpfError> {
    // There are only two BPF registers, A and X. There are 10
    // EBPF registers, numbered 0-9.  We map between the two as
    // follows:

    // R0: Mapped to A.
    const REG_A: u8 = 0;

    // R1: Incoming argument pointing at the packet context (e.g. `sk_buff`). Moved to R6.
    const REG_ARG1: u8 = 1;

    // R6: Pointer to the program context. Initially passed as the first argument. Implicitly
    //     used by eBPF when executing the legacy packet access instructions (`BPF_LD | BPF_ABS`
    //     and `BPF_LD | BPF_IND`).
    const REG_CONTEXT: u8 = 6;

    // R7: Temp register used in the `BPF_MSH` implementation.
    const REG_TMP: u8 = 7;

    // R9: Mapped to X
    const REG_X: u8 = 9;

    // R10: Const stack pointer. cBFP scratch memory (16 words) is stored on top of the stack.
    const REG_STACK: u8 = 10;

    // Map from jump targets in the cbpf to a list of jump instructions in the epbf that target
    // it. When you figure out what the offset of the target is in the ebpf, you need to patch the
    // jump instructions to target it correctly.
    let mut to_be_patched: HashMap<usize, Vec<usize>> = HashMap::new();

    let mut ebpf_code: Vec<EbpfInstruction> = vec![];
    ebpf_code.reserve(bpf_code.len() * 2 + 3);

    // Save the arguments to registers that won't get clobbered by `BPF_LD`.
    ebpf_code.push(EbpfInstruction::new(BPF_ALU64 | BPF_MOV | BPF_X, REG_CONTEXT, REG_ARG1, 0, 0));

    // Reset A to 0. This is necessary in case one of the load operation exits prematurely.
    ebpf_code.push(EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_K, REG_A, 0, 0, 0));
    // Reset X to 0. Linux does it and some programs might not validate otherwise.
    ebpf_code.push(EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_K, REG_X, 0, 0, 0));

    for (i, bpf_instruction) in bpf_code.iter().enumerate() {
        // Update instructions processed previously that jump to the current one.
        if let Some((_, entries)) = to_be_patched.remove_entry(&i) {
            for index in entries {
                let offset = (ebpf_code.len() - index - 1) as i16;
                ebpf_code[index].set_offset(offset);
            }
        }

        // Helper to queue a new entry into `to_be_patched`.
        let mut prep_patch = |cbpf_offset: usize, ebpf_source: usize| -> Result<(), EbpfError> {
            let cbpf_target = i + 1 + cbpf_offset;
            if cbpf_target >= bpf_code.len() {
                return Err(EbpfError::InvalidCbpfJumpOffset(cbpf_offset as u32));
            }
            to_be_patched.entry(cbpf_target).or_insert_with(Vec::new).push(ebpf_source);
            Ok(())
        };

        match bpf_class(bpf_instruction) {
            BPF_ALU => match bpf_op(bpf_instruction) {
                op @ (BPF_ADD | BPF_SUB | BPF_MUL | BPF_DIV | BPF_MOD | BPF_AND | BPF_OR
                | BPF_XOR | BPF_LSH | BPF_RSH) => {
                    if bpf_src(bpf_instruction) == BPF_K {
                        // Division and remainder by 0 are rejected.
                        if (op == BPF_DIV || op == BPF_MOD) && bpf_instruction.k == 0 {
                            return Err(EbpfError::ProgramVerifyError("Division by 0".to_string()));
                        }
                        // LSH and RSH by more than 31 are rejected.
                        if (op == BPF_LSH || op == BPF_RSH) && bpf_instruction.k >= 32 {
                            return Err(EbpfError::ProgramVerifyError(
                                "Shift by 32 or more".to_string(),
                            ));
                        }
                        ebpf_code.push(EbpfInstruction::new(
                            bpf_instruction.code as u8,
                            REG_A,
                            0,
                            0,
                            bpf_instruction.k as i32,
                        ));
                    } else {
                        // Division and remainder by 0 must stop the execution and return 0.
                        if op == BPF_DIV || op == BPF_MOD {
                            // If X != 0, skip 1 instruction.
                            ebpf_code.push(EbpfInstruction::new(
                                BPF_JMP32 | BPF_JNE | BPF_K,
                                REG_X,
                                0,
                                2,
                                0,
                            ));
                            // Return 0.
                            ebpf_code.push(EbpfInstruction::new(
                                BPF_ALU | BPF_MOV | BPF_IMM,
                                REG_A,
                                0,
                                0,
                                0,
                            ));
                            ebpf_code.push(EbpfInstruction::new(BPF_JMP | BPF_EXIT, 0, 0, 0, 0));
                        }

                        ebpf_code.push(EbpfInstruction::new(
                            bpf_instruction.code as u8,
                            REG_A,
                            REG_X,
                            0,
                            0,
                        ));
                    };
                }
                BPF_NEG => {
                    ebpf_code.push(EbpfInstruction::new(BPF_ALU | BPF_NEG, REG_A, REG_A, 0, 0));
                }
                _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
            },
            class @ (BPF_LD | BPF_LDX) => {
                let dst_reg = if class == BPF_LDX { REG_X } else { REG_A };

                let mode = bpf_addressing_mode(bpf_instruction);
                let size = bpf_size(bpf_instruction);

                // Half-word (`BPF_H`) and byte (`BPF_B`) loads are allowed only for `BPD_ABS` and
                // `BPD_IND`. Also `BPD_ABS` and `BPD_IND` are not allowed with `BPD_LDX`.
                // `BPF_LEN`, `BPF_IMM` and `BPF_MEM` loads should be word-sized (i.e. `BPF_W`).
                // `BPF_MSH` is allowed only with `BPF_B` and `BPF_LDX`.
                match (size, mode, class) {
                    (BPF_H | BPF_B | BPF_W, BPF_ABS | BPF_IND, BPF_LD) => (),
                    (BPF_W, BPF_LEN | BPF_IMM | BPF_MEM, BPF_LD | BPF_LDX) => (),
                    (BPF_B, BPF_MSH, BPF_LDX) if config.allow_msh => (),
                    _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
                };

                let k = bpf_instruction.k;

                match mode {
                    BPF_ABS => {
                        ebpf_code.push(EbpfInstruction::new(
                            BPF_LD | BPF_ABS | size,
                            REG_A,
                            0,
                            0,
                            k as i32,
                        ));
                    }
                    BPF_IND => {
                        ebpf_code.push(EbpfInstruction::new(
                            BPF_LD | BPF_IND | size,
                            REG_A,
                            REG_X,
                            0,
                            k as i32,
                        ));
                    }
                    BPF_IMM => {
                        let imm = k as i32;
                        ebpf_code.push(EbpfInstruction::new(
                            BPF_ALU | BPF_MOV | BPF_K,
                            dst_reg,
                            0,
                            0,
                            imm,
                        ));
                    }
                    BPF_MEM => {
                        // cBPF's scratch memory is stored in the stack referenced by R10.
                        let offset = cbpf_scratch_offset(k)?;
                        ebpf_code.push(EbpfInstruction::new(
                            BPF_LDX | BPF_MEM,
                            dst_reg,
                            REG_STACK,
                            offset,
                            0,
                        ));
                    }
                    BPF_LEN => {
                        ebpf_code.push(match config.len {
                            CbpfLenInstruction::Static { len } => {
                                EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_K, dst_reg, 0, 0, len)
                            }
                            CbpfLenInstruction::ContextField { offset } => EbpfInstruction::new(
                                BPF_LDX | BPF_MEM | BPF_W,
                                dst_reg,
                                REG_CONTEXT,
                                offset,
                                0,
                            ),
                        });
                    }
                    BPF_MSH => {
                        // `BPF_MSH` loads `4 * (P[k:1] & 0xf)`, which translates to 6 instructions.
                        ebpf_code.extend_from_slice(&[
                            // mov TMP, A
                            EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_X, REG_TMP, REG_A, 0, 0),
                            // ldpb [k]
                            EbpfInstruction::new(BPF_LD | BPF_ABS | BPF_B, REG_A, 0, 0, k as i32),
                            // and A, 0xf
                            EbpfInstruction::new(BPF_ALU | BPF_AND | BPF_K, REG_A, 0, 0, 0x0f),
                            // mul A, 4
                            EbpfInstruction::new(BPF_ALU | BPF_MUL | BPF_K, REG_A, 0, 0, 4),
                            // mov X, A
                            EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_X, REG_X, REG_A, 0, 0),
                            // mov A, TMP
                            EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_X, REG_A, REG_TMP, 0, 0),
                        ]);
                    }
                    _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
                }
            }
            BPF_JMP => {
                match bpf_op(bpf_instruction) {
                    BPF_JA => {
                        ebpf_code.push(EbpfInstruction::new(BPF_JMP | BPF_JA, 0, 0, -1, 0));
                        prep_patch(bpf_instruction.k as usize, ebpf_code.len() - 1)?;
                    }
                    op @ (BPF_JGT | BPF_JGE | BPF_JEQ | BPF_JSET) => {
                        // In cBPF, JMPs have a jump-if-true and jump-if-false branch. eBPF only
                        // has jump-if-true. In most cases only one of the two branches actually
                        // jumps (the other one is set to 0). In these cases the instruction can
                        // be translated to 1 eBPF instruction. Otherwise two instructions are
                        // produced in the output.

                        let src = bpf_src(bpf_instruction);
                        let sock_filter { k, jt, jf, .. } = *bpf_instruction;
                        let (src_reg, imm) = if src == BPF_K { (0, k as i32) } else { (REG_X, 0) };

                        // When jumping only for the false case we can negate the comparison
                        // operator to achieve the same effect with a single jump-if-true eBPF
                        // instruction. That doesn't work for `BPF_JSET`. It is handled below
                        // using 2 instructions.
                        if jt == 0 && op != BPF_JSET {
                            let op = match op {
                                BPF_JGT => BPF_JLE,
                                BPF_JGE => BPF_JLT,
                                BPF_JEQ => BPF_JNE,
                                _ => panic!("Unexpected operation: {op:?}"),
                            };

                            ebpf_code.push(EbpfInstruction::new(
                                BPF_JMP32 | op | src,
                                REG_A,
                                src_reg,
                                -1,
                                imm,
                            ));
                            prep_patch(jf as usize, ebpf_code.len() - 1)?;
                        } else {
                            // Jump if true.
                            ebpf_code.push(EbpfInstruction::new(
                                BPF_JMP32 | op | src,
                                REG_A,
                                src_reg,
                                -1,
                                imm,
                            ));
                            prep_patch(jt as usize, ebpf_code.len() - 1)?;

                            // Jump if false. Jumps with 0 offset are no-op and can be omitted.
                            if jf > 0 {
                                ebpf_code.push(EbpfInstruction::new(BPF_JMP | BPF_JA, 0, 0, -1, 0));
                                prep_patch(jf as usize, ebpf_code.len() - 1)?;
                            }
                        }
                    }
                    _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
                }
            }
            BPF_MISC => match bpf_op(bpf_instruction) {
                BPF_TAX => {
                    ebpf_code.push(EbpfInstruction::new(
                        BPF_ALU | BPF_MOV | BPF_X,
                        REG_X,
                        REG_A,
                        0,
                        0,
                    ));
                }
                BPF_TXA => {
                    ebpf_code.push(EbpfInstruction::new(
                        BPF_ALU | BPF_MOV | BPF_X,
                        REG_A,
                        REG_X,
                        0,
                        0,
                    ));
                }
                _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
            },

            class @ (BPF_ST | BPF_STX) => {
                if bpf_addressing_mode(bpf_instruction) != 0 || bpf_size(bpf_instruction) != 0 {
                    return Err(InvalidCbpfInstruction(bpf_instruction.code));
                }

                // cBPF's scratch memory is stored in the stack referenced by R10.
                let src_reg = if class == BPF_STX { REG_X } else { REG_A };
                let offset = cbpf_scratch_offset(bpf_instruction.k)?;
                ebpf_code.push(EbpfInstruction::new(
                    BPF_STX | BPF_MEM | BPF_W,
                    REG_STACK,
                    src_reg,
                    offset,
                    0,
                ));
            }
            BPF_RET => {
                match bpf_rval(bpf_instruction) {
                    BPF_K => {
                        // We're returning a particular value instead of the contents of the
                        // return register, so load that value into the return register.
                        let imm = bpf_instruction.k as i32;
                        ebpf_code.push(EbpfInstruction::new(
                            BPF_ALU | BPF_MOV | BPF_IMM,
                            REG_A,
                            0,
                            0,
                            imm,
                        ));
                    }
                    BPF_A => (),
                    _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
                };

                ebpf_code.push(EbpfInstruction::new(BPF_JMP | BPF_EXIT, 0, 0, 0, 0));
            }
            _ => return Err(InvalidCbpfInstruction(bpf_instruction.code)),
        }
    }

    assert!(to_be_patched.is_empty());

    Ok(ebpf_code)
}

/// Instantiates an EbpfProgram given a cbpf original that will work with a packet of the
/// specified type.
pub fn convert_and_verify_cbpf(
    bpf_code: &[sock_filter],
    packet_type: Type,
    config: &CbpfConfig,
) -> Result<VerifiedEbpfProgram, EbpfError> {
    let context = CallingContext {
        maps: vec![],
        helpers: HashMap::new(),
        args: vec![packet_type.clone()],
        packet_type: Some(packet_type),
    };
    let ebpf_code = cbpf_to_ebpf(bpf_code, config)?;
    verify_program(ebpf_code, context, &mut NullVerifierLogger)
}

/// Converts, verifies and links a cBPF program for execution in the specified context.
pub fn convert_and_link_cbpf<C: BpfProgramContext + StaticHelperSet>(
    bpf_code: &[sock_filter],
) -> Result<EbpfProgram<C>, EbpfError> {
    let verified = convert_and_verify_cbpf(
        bpf_code,
        <C as BpfProgramContext>::Packet::get_type().clone(),
        C::CBPF_CONFIG,
    )?;
    link_program(&verified, vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryId, NoMap, empty_static_helper_set};
    use linux_uapi::{
        AUDIT_ARCH_AARCH64, AUDIT_ARCH_X86_64, SECCOMP_RET_ALLOW, SECCOMP_RET_TRAP, seccomp_data,
        sock_filter,
    };
    use std::mem::offset_of;
    use std::sync::LazyLock;
    use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

    pub const TEST_CBPF_CONFIG: CbpfConfig = CbpfConfig {
        len: CbpfLenInstruction::Static { len: size_of::<seccomp_data>() as i32 },
        allow_msh: true,
    };

    #[test]
    fn test_cbpf_to_ebpf() {
        // Jump to the next instruction.
        assert_eq!(
            cbpf_to_ebpf(
                &vec![
                    sock_filter { code: (BPF_JMP | BPF_JA) as u16, jt: 0, jf: 0, k: 0 },
                    sock_filter { code: (BPF_RET | BPF_A) as u16, jt: 0, jf: 0, k: 0 },
                ],
                &TEST_CBPF_CONFIG
            ),
            Ok(vec![
                EbpfInstruction::new(BPF_ALU64 | BPF_MOV | BPF_X, 6, 1, 0, 0),
                EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_K, 0, 0, 0, 0),
                EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_K, 9, 0, 0, 0),
                EbpfInstruction::new(BPF_JMP | BPF_JA, 0, 0, 0, 0),
                EbpfInstruction::new(BPF_JMP | BPF_EXIT, 0, 0, 0, 0),
            ]),
        );

        // Jump after last instruction.
        assert_eq!(
            cbpf_to_ebpf(
                &vec![
                    sock_filter { code: (BPF_JMP | BPF_JA) as u16, jt: 0, jf: 0, k: 1 },
                    sock_filter { code: (BPF_RET | BPF_A) as u16, jt: 0, jf: 0, k: 0 },
                ],
                &TEST_CBPF_CONFIG
            ),
            Err(EbpfError::InvalidCbpfJumpOffset(1)),
        );

        // Jump out of bounds.
        assert_eq!(
            cbpf_to_ebpf(
                &vec![sock_filter { code: (BPF_JMP | BPF_JA) as u16, jt: 0, jf: 0, k: 0xffffffff }],
                &TEST_CBPF_CONFIG
            ),
            Err(EbpfError::InvalidCbpfJumpOffset(0xffffffff)),
        );

        // BPF_JNE is allowed only in eBPF.
        assert_eq!(
            cbpf_to_ebpf(
                &vec![
                    sock_filter { code: (BPF_JMP | BPF_JNE) as u16, jt: 0, jf: 0, k: 0 },
                    sock_filter { code: (BPF_RET | BPF_A) as u16, jt: 0, jf: 0, k: 0 },
                ],
                &TEST_CBPF_CONFIG
            ),
            Err(EbpfError::InvalidCbpfInstruction((BPF_JMP | BPF_JNE) as u16)),
        );

        // BPF_JEQ is supported in BPF.
        assert_eq!(
            cbpf_to_ebpf(
                &vec![
                    sock_filter { code: (BPF_JMP | BPF_JEQ) as u16, jt: 1, jf: 0, k: 0 },
                    sock_filter { code: (BPF_RET | BPF_A) as u16, jt: 0, jf: 0, k: 0 },
                    sock_filter { code: (BPF_RET | BPF_A) as u16, jt: 0, jf: 0, k: 0 },
                ],
                &TEST_CBPF_CONFIG
            ),
            Ok(vec![
                EbpfInstruction::new(BPF_ALU64 | BPF_MOV | BPF_X, 6, 1, 0, 0),
                EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_K, 0, 0, 0, 0),
                EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_K, 9, 0, 0, 0),
                EbpfInstruction::new(BPF_JMP32 | BPF_JEQ, 0, 0, 1, 0),
                EbpfInstruction::new(BPF_JMP | BPF_EXIT, 0, 0, 0, 0),
                EbpfInstruction::new(BPF_JMP | BPF_EXIT, 0, 0, 0, 0),
            ]),
        );

        // Make sure the jump is translated correctly when the jump target produces 2 instructions.
        assert_eq!(
            cbpf_to_ebpf(
                &vec![
                    sock_filter { code: (BPF_JMP | BPF_JA) as u16, jt: 0, jf: 0, k: 0 },
                    sock_filter { code: (BPF_RET | BPF_K) as u16, jt: 0, jf: 0, k: 1 },
                ],
                &TEST_CBPF_CONFIG
            ),
            Ok(vec![
                EbpfInstruction::new(BPF_ALU64 | BPF_MOV | BPF_X, 6, 1, 0, 0),
                EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_K, 0, 0, 0, 0),
                EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_K, 9, 0, 0, 0),
                EbpfInstruction::new(BPF_JMP | BPF_JA, 0, 0, 0, 0),
                EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_IMM, 0, 0, 0, 1),
                EbpfInstruction::new(BPF_JMP | BPF_EXIT, 0, 0, 0, 0),
            ]),
        );

        // BPF_MEM access.
        assert_eq!(
            cbpf_to_ebpf(
                &vec![
                    sock_filter { code: (BPF_LD | BPF_MEM) as u16, jt: 0, jf: 0, k: 0 },
                    sock_filter { code: (BPF_LDX | BPF_MEM) as u16, jt: 0, jf: 0, k: 15 },
                    sock_filter { code: BPF_ST as u16, jt: 0, jf: 0, k: 0 },
                    sock_filter { code: BPF_STX as u16, jt: 0, jf: 0, k: 15 },
                ],
                &TEST_CBPF_CONFIG
            ),
            Ok(vec![
                EbpfInstruction::new(BPF_ALU64 | BPF_MOV | BPF_X, 6, 1, 0, 0),
                EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_K, 0, 0, 0, 0),
                EbpfInstruction::new(BPF_ALU | BPF_MOV | BPF_K, 9, 0, 0, 0),
                EbpfInstruction::new(BPF_LDX | BPF_MEM | BPF_W, 0, 10, -64, 0),
                EbpfInstruction::new(BPF_LDX | BPF_MEM | BPF_W, 9, 10, -4, 0),
                EbpfInstruction::new(BPF_STX | BPF_MEM | BPF_W, 10, 0, -64, 0),
                EbpfInstruction::new(BPF_STX | BPF_MEM | BPF_W, 10, 9, -4, 0),
            ]),
        );

        // BPF_MEM access out of bounds.
        assert_eq!(
            cbpf_to_ebpf(
                &vec![sock_filter { code: (BPF_LD | BPF_MEM) as u16, jt: 0, jf: 0, k: 17 }],
                &TEST_CBPF_CONFIG
            ),
            Err(EbpfError::InvalidCbpfScratchOffset(17)),
        );
    }

    const BPF_ALU_ADD_K: u16 = (BPF_ALU | BPF_ADD | BPF_K) as u16;
    const BPF_ALU_SUB_K: u16 = (BPF_ALU | BPF_SUB | BPF_K) as u16;
    const BPF_ALU_MUL_K: u16 = (BPF_ALU | BPF_MUL | BPF_K) as u16;
    const BPF_ALU_DIV_K: u16 = (BPF_ALU | BPF_DIV | BPF_K) as u16;
    const BPF_ALU_AND_K: u16 = (BPF_ALU | BPF_AND | BPF_K) as u16;
    const BPF_ALU_OR_K: u16 = (BPF_ALU | BPF_OR | BPF_K) as u16;
    const BPF_ALU_XOR_K: u16 = (BPF_ALU | BPF_XOR | BPF_K) as u16;
    const BPF_ALU_LSH_K: u16 = (BPF_ALU | BPF_LSH | BPF_K) as u16;
    const BPF_ALU_RSH_K: u16 = (BPF_ALU | BPF_RSH | BPF_K) as u16;

    const BPF_ALU_OR_X: u16 = (BPF_ALU | BPF_OR | BPF_X) as u16;

    const BPF_LD_W_ABS: u16 = (BPF_LD | BPF_ABS | BPF_W) as u16;
    const BPF_LD_W_MEM: u16 = (BPF_LD | BPF_MEM | BPF_W) as u16;
    const BPF_JEQ_K: u16 = (BPF_JMP | BPF_JEQ | BPF_K) as u16;
    const BPF_JSET_K: u16 = (BPF_JMP | BPF_JSET | BPF_K) as u16;
    const BPF_RET_K: u16 = (BPF_RET | BPF_K) as u16;
    const BPF_RET_A: u16 = (BPF_RET | BPF_A) as u16;
    const BPF_ST_REG: u16 = BPF_ST as u16;
    const BPF_MISC_TAX: u16 = (BPF_MISC | BPF_TAX) as u16;

    struct TestProgramContext {}

    impl BpfProgramContext for TestProgramContext {
        type RunContext<'a> = ();
        type Packet<'a> = &'a seccomp_data;
        type Map = NoMap;
        const CBPF_CONFIG: &'static CbpfConfig = &TEST_CBPF_CONFIG;
    }

    empty_static_helper_set!(TestProgramContext);

    static SECCOMP_DATA_TYPE: LazyLock<Type> = LazyLock::new(|| Type::PtrToMemory {
        id: MemoryId::new(),
        offset: 0.into(),
        buffer_size: 0,
    });

    impl ProgramArgument for &'_ seccomp_data {
        fn get_type() -> &'static Type {
            &*SECCOMP_DATA_TYPE
        }
    }

    fn with_prg_assert_result(
        prg: &EbpfProgram<TestProgramContext>,
        mut data: seccomp_data,
        result: u32,
        msg: &str,
    ) {
        let return_value = prg.run(&mut (), &mut data);
        assert_eq!(return_value, result as u64, "{}: filter return value is {}", msg, return_value);
    }

    #[test]
    fn test_filter_with_dw_load() {
        let test_prg = [
            // Check data.arch
            sock_filter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 4 },
            sock_filter { code: BPF_JEQ_K, jt: 1, jf: 0, k: AUDIT_ARCH_X86_64 },
            // Return 1 if arch is wrong
            sock_filter { code: BPF_RET_K, jt: 0, jf: 0, k: 1 },
            // Load data.nr (the syscall number)
            sock_filter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 0 },
            // Always allow 41
            sock_filter { code: BPF_JEQ_K, jt: 0, jf: 1, k: 41 },
            sock_filter { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_ALLOW },
            // Don't allow 115
            sock_filter { code: BPF_JEQ_K, jt: 0, jf: 1, k: 115 },
            sock_filter { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_TRAP },
            // For other syscalls, check the args
            // A common hack to deal with 64-bit numbers in BPF: deal
            // with 32 bits at a time.
            // First, Load arg0's most significant 32 bits in M[0]
            sock_filter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 16 },
            sock_filter { code: BPF_ST_REG, jt: 0, jf: 0, k: 0 },
            // Load arg0's least significant 32 bits into M[1]
            sock_filter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 20 },
            sock_filter { code: BPF_ST_REG, jt: 0, jf: 0, k: 1 },
            // JSET is A & k.  Check the first 32 bits.  If the test
            // is successful, jump, otherwise, check the next 32 bits.
            sock_filter { code: BPF_LD_W_MEM, jt: 0, jf: 0, k: 0 },
            sock_filter { code: BPF_JSET_K, jt: 2, jf: 0, k: 4294967295 },
            sock_filter { code: BPF_LD_W_MEM, jt: 0, jf: 0, k: 1 },
            sock_filter { code: BPF_JSET_K, jt: 0, jf: 1, k: 4294967292 },
            sock_filter { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_TRAP },
            sock_filter { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_ALLOW },
        ];

        let prg =
            convert_and_link_cbpf::<TestProgramContext>(&test_prg).expect("Error parsing program");

        with_prg_assert_result(
            &prg,
            seccomp_data { arch: AUDIT_ARCH_AARCH64, ..Default::default() },
            1,
            "Did not reject incorrect arch",
        );

        with_prg_assert_result(
            &prg,
            seccomp_data { arch: AUDIT_ARCH_X86_64, nr: 41, ..Default::default() },
            SECCOMP_RET_ALLOW,
            "Did not pass simple RET_ALLOW",
        );

        with_prg_assert_result(
            &prg,
            seccomp_data {
                arch: AUDIT_ARCH_X86_64,
                nr: 100,
                args: [0xFF00000000, 0, 0, 0, 0, 0],
                ..Default::default()
            },
            SECCOMP_RET_TRAP,
            "Did not treat load of first 32 bits correctly",
        );

        with_prg_assert_result(
            &prg,
            seccomp_data {
                arch: AUDIT_ARCH_X86_64,
                nr: 100,
                args: [0x4, 0, 0, 0, 0, 0],
                ..Default::default()
            },
            SECCOMP_RET_TRAP,
            "Did not correctly reject load of second 32 bits",
        );

        with_prg_assert_result(
            &prg,
            seccomp_data {
                arch: AUDIT_ARCH_X86_64,
                nr: 100,
                args: [0x0, 0, 0, 0, 0, 0],
                ..Default::default()
            },
            SECCOMP_RET_ALLOW,
            "Did not correctly accept load of second 32 bits",
        );
    }

    #[test]
    fn test_alu_insns() {
        {
            let test_prg = [
                // Load data.nr (the syscall number)
                sock_filter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 0 }, // = 1, 11
                // Do some math.
                sock_filter { code: BPF_ALU_ADD_K, jt: 0, jf: 0, k: 3 }, // = 4, 14
                sock_filter { code: BPF_ALU_SUB_K, jt: 0, jf: 0, k: 2 }, // = 2, 12
                sock_filter { code: BPF_MISC_TAX, jt: 0, jf: 0, k: 0 },  // 2, 12 -> X
                sock_filter { code: BPF_ALU_MUL_K, jt: 0, jf: 0, k: 8 }, // = 16, 96
                sock_filter { code: BPF_ALU_DIV_K, jt: 0, jf: 0, k: 2 }, // = 8, 48
                sock_filter { code: BPF_ALU_AND_K, jt: 0, jf: 0, k: 15 }, // = 8, 0
                sock_filter { code: BPF_ALU_OR_K, jt: 0, jf: 0, k: 16 }, // = 24, 16
                sock_filter { code: BPF_ALU_XOR_K, jt: 0, jf: 0, k: 7 }, // = 31, 23
                sock_filter { code: BPF_ALU_LSH_K, jt: 0, jf: 0, k: 2 }, // = 124, 92
                sock_filter { code: BPF_ALU_OR_X, jt: 0, jf: 0, k: 1 },  // = 127, 92
                sock_filter { code: BPF_ALU_RSH_K, jt: 0, jf: 0, k: 1 }, // = 63, 46
                sock_filter { code: BPF_RET_A, jt: 0, jf: 0, k: 0 },
            ];

            let prg = convert_and_link_cbpf::<TestProgramContext>(&test_prg)
                .expect("Error parsing program");

            with_prg_assert_result(
                &prg,
                seccomp_data { nr: 1, ..Default::default() },
                63,
                "BPF math does not work",
            );

            with_prg_assert_result(
                &prg,
                seccomp_data { nr: 11, ..Default::default() },
                46,
                "BPF math does not work",
            );
        }

        {
            // Negative numbers simple check
            let test_prg = [
                // Load data.nr (the syscall number)
                sock_filter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 0 }, // = -1
                sock_filter { code: BPF_ALU_SUB_K, jt: 0, jf: 0, k: 2 }, // = -3
                sock_filter { code: BPF_RET_A, jt: 0, jf: 0, k: 0 },
            ];

            let prg = convert_and_link_cbpf::<TestProgramContext>(&test_prg)
                .expect("Error parsing program");

            with_prg_assert_result(
                &prg,
                seccomp_data { nr: -1, ..Default::default() },
                u32::MAX - 2,
                "BPF math does not work",
            );
        }
    }

    // Test BPF_MSH cBPF instruction.
    #[test]
    fn test_ld_msh() {
        let test_prg = [
            // X <- 4 * (P[0] & 0xf)
            sock_filter { code: (BPF_LDX | BPF_MSH | BPF_B) as u16, jt: 0, jf: 0, k: 0 },
            // A <- X
            sock_filter { code: (BPF_MISC | BPF_TXA) as u16, jt: 0, jf: 0, k: 0 },
            // ret A
            sock_filter { code: BPF_RET_A, jt: 0, jf: 0, k: 0 },
        ];

        let prg =
            convert_and_link_cbpf::<TestProgramContext>(&test_prg).expect("Error parsing program");

        for i in [0x00, 0x01, 0x07, 0x15, 0xff].iter() {
            with_prg_assert_result(
                &prg,
                seccomp_data { nr: *i, ..Default::default() },
                4 * (*i & 0xf) as u32,
                "BPF math does not work",
            )
        }
    }

    #[test]
    fn test_static_packet_len() {
        let test_prg = [
            // A <- packet_len
            sock_filter { code: (BPF_LD | BPF_LEN | BPF_W) as u16, jt: 0, jf: 0, k: 0 },
            // ret A
            sock_filter { code: BPF_RET_A, jt: 0, jf: 0, k: 0 },
        ];

        let prg =
            convert_and_link_cbpf::<TestProgramContext>(&test_prg).expect("Error parsing program");

        let data = seccomp_data::default();
        assert_eq!(prg.run(&mut (), &data), size_of::<seccomp_data>() as u64);
    }

    // A packet used by `test_variable_packet_len()` below to verify the case when the packet
    // length is stored as a struct field.
    #[repr(C)]
    #[derive(Debug, Default, IntoBytes, Immutable, KnownLayout, FromBytes)]
    struct VariableLengthPacket {
        foo: u32,
        len: i32,
        bar: u64,
    }

    static VARIABLE_LENGTH_PACKET_TYPE: LazyLock<Type> = LazyLock::new(|| Type::PtrToMemory {
        id: MemoryId::new(),
        offset: 0.into(),
        buffer_size: size_of::<VariableLengthPacket>() as u64,
    });

    impl ProgramArgument for &'_ VariableLengthPacket {
        fn get_type() -> &'static Type {
            &*VARIABLE_LENGTH_PACKET_TYPE
        }
    }

    pub const VARIABLE_LENGTH_CBPF_CONFIG: CbpfConfig = CbpfConfig {
        len: CbpfLenInstruction::ContextField {
            offset: offset_of!(VariableLengthPacket, len) as i16,
        },
        allow_msh: true,
    };

    struct VariableLengthPacketContext {}

    impl BpfProgramContext for VariableLengthPacketContext {
        type RunContext<'a> = ();
        type Packet<'a> = &'a VariableLengthPacket;
        type Map = NoMap;
        const CBPF_CONFIG: &'static CbpfConfig = &VARIABLE_LENGTH_CBPF_CONFIG;
    }

    empty_static_helper_set!(VariableLengthPacketContext);

    #[test]
    fn test_variable_packet_len() {
        let test_prg = [
            // A <- packet_len
            sock_filter { code: (BPF_LD | BPF_LEN | BPF_W) as u16, jt: 0, jf: 0, k: 0 },
            // ret A
            sock_filter { code: BPF_RET_A, jt: 0, jf: 0, k: 0 },
        ];

        let prg = convert_and_link_cbpf::<VariableLengthPacketContext>(&test_prg)
            .expect("Error parsing program");
        let data = VariableLengthPacket { len: 42, ..VariableLengthPacket::default() };
        assert_eq!(prg.run(&mut (), &data), data.len as u64);
    }
}
