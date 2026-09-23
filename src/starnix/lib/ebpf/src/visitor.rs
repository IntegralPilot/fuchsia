// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{
    BPF_ABS, BPF_ADD, BPF_ALU, BPF_ALU64, BPF_AND, BPF_ARSH, BPF_ATOMIC, BPF_B, BPF_CALL,
    BPF_CMPXCHG, BPF_DIV, BPF_DW, BPF_END, BPF_EXIT, BPF_FETCH, BPF_H, BPF_IND, BPF_JA, BPF_JEQ,
    BPF_JGE, BPF_JGT, BPF_JLE, BPF_JLT, BPF_JMP, BPF_JMP32, BPF_JNE, BPF_JSET, BPF_JSGE, BPF_JSGT,
    BPF_JSLE, BPF_JSLT, BPF_K, BPF_LD, BPF_LDDW, BPF_LDX, BPF_LSH, BPF_MEM, BPF_MOD, BPF_MOV,
    BPF_MUL, BPF_NEG, BPF_OR, BPF_RSH, BPF_ST, BPF_STX, BPF_SUB, BPF_TO_BE, BPF_TO_LE, BPF_W,
    BPF_X, BPF_XCHG, BPF_XOR, DataWidth, EbpfInstruction,
};

/// The index into the registers. 10 is the stack pointer.
pub type Register = u8;

/// The index into the program
pub type ProgramCounter = usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Reg(Register),
    Value(u64),
}

pub trait BpfVisitor {
    type Context<'a>;

    fn add<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn add64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn and<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn and64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn arsh<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn arsh64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn div<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn div64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn lsh<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn lsh64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn r#mod<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn mod64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn mov<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn mov64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn mul<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn mul64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn or<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn or64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn rsh<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn rsh64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn sub<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn sub64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn xor<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;
    fn xor64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String>;

    fn neg<'a>(&mut self, context: &mut Self::Context<'a>, dst: Register) -> Result<(), String>;
    fn neg64<'a>(&mut self, context: &mut Self::Context<'a>, dst: Register) -> Result<(), String>;

    fn be<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        width: DataWidth,
    ) -> Result<(), String>;
    fn le<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        width: DataWidth,
    ) -> Result<(), String>;

    fn call_external<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        index: u32,
    ) -> Result<(), String>;

    fn exit<'a>(&mut self, context: &mut Self::Context<'a>) -> Result<(), String>;

    fn jump<'a>(&mut self, context: &mut Self::Context<'a>, offset: i16) -> Result<(), String>;

    fn jeq<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jeq64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jne<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jne64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jge<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jge64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jgt<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jgt64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jle<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jle64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jlt<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jlt64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jsge<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jsge64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jsgt<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jsgt64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jsle<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jsle64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jslt<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jslt64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jset<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;
    fn jset64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String>;

    fn atomic_add<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn atomic_add64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn atomic_and<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn atomic_and64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn atomic_or<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn atomic_or64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn atomic_xor<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn atomic_xor64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn atomic_xchg<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn atomic_xchg64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn atomic_cmpxchg<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn atomic_cmpxchg64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String>;

    fn load<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        offset: i16,
        src: Register,
        width: DataWidth,
    ) -> Result<(), String>;

    fn load64<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: u8,
        lower: u32,
    ) -> Result<(), String>;

    fn load_from_packet<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        src: Register,
        offset: i32,
        register_offset: Option<Register>,
        width: DataWidth,
    ) -> Result<(), String>;

    fn store<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst: Register,
        offset: i16,
        src: Source,
        width: DataWidth,
    ) -> Result<(), String>;

    #[inline(always)]
    fn visit<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        inst: EbpfInstruction,
    ) -> Result<(), String> {
        let dst = inst.dst_reg();
        let src_imm = || Source::Value(inst.imm() as u64);
        let src_reg = || Source::Reg(inst.src_reg());

        let width_from_imm = || match inst.imm() {
            16 => Ok(DataWidth::U16),
            32 => Ok(DataWidth::U32),
            64 => Ok(DataWidth::U64),
            _ => Err(format!("invalid byte swap width: {}", inst.imm())),
        };

        const BPF_ALU_ADD_K: u8 = BPF_ALU | BPF_ADD | BPF_K;
        const BPF_ALU_ADD_X: u8 = BPF_ALU | BPF_ADD | BPF_X;
        const BPF_ALU_SUB_K: u8 = BPF_ALU | BPF_SUB | BPF_K;
        const BPF_ALU_SUB_X: u8 = BPF_ALU | BPF_SUB | BPF_X;
        const BPF_ALU_MUL_K: u8 = BPF_ALU | BPF_MUL | BPF_K;
        const BPF_ALU_MUL_X: u8 = BPF_ALU | BPF_MUL | BPF_X;
        const BPF_ALU_DIV_K: u8 = BPF_ALU | BPF_DIV | BPF_K;
        const BPF_ALU_DIV_X: u8 = BPF_ALU | BPF_DIV | BPF_X;
        const BPF_ALU_OR_K: u8 = BPF_ALU | BPF_OR | BPF_K;
        const BPF_ALU_OR_X: u8 = BPF_ALU | BPF_OR | BPF_X;
        const BPF_ALU_AND_K: u8 = BPF_ALU | BPF_AND | BPF_K;
        const BPF_ALU_AND_X: u8 = BPF_ALU | BPF_AND | BPF_X;
        const BPF_ALU_LSH_K: u8 = BPF_ALU | BPF_LSH | BPF_K;
        const BPF_ALU_LSH_X: u8 = BPF_ALU | BPF_LSH | BPF_X;
        const BPF_ALU_RSH_K: u8 = BPF_ALU | BPF_RSH | BPF_K;
        const BPF_ALU_RSH_X: u8 = BPF_ALU | BPF_RSH | BPF_X;
        const BPF_ALU_MOD_K: u8 = BPF_ALU | BPF_MOD | BPF_K;
        const BPF_ALU_MOD_X: u8 = BPF_ALU | BPF_MOD | BPF_X;
        const BPF_ALU_XOR_K: u8 = BPF_ALU | BPF_XOR | BPF_K;
        const BPF_ALU_XOR_X: u8 = BPF_ALU | BPF_XOR | BPF_X;
        const BPF_ALU_MOV_K: u8 = BPF_ALU | BPF_MOV | BPF_K;
        const BPF_ALU_MOV_X: u8 = BPF_ALU | BPF_MOV | BPF_X;
        const BPF_ALU_ARSH_K: u8 = BPF_ALU | BPF_ARSH | BPF_K;
        const BPF_ALU_ARSH_X: u8 = BPF_ALU | BPF_ARSH | BPF_X;
        const BPF_ALU_NEG: u8 = BPF_ALU | BPF_NEG;

        const BPF_ALU64_ADD_K: u8 = BPF_ALU64 | BPF_ADD | BPF_K;
        const BPF_ALU64_ADD_X: u8 = BPF_ALU64 | BPF_ADD | BPF_X;
        const BPF_ALU64_SUB_K: u8 = BPF_ALU64 | BPF_SUB | BPF_K;
        const BPF_ALU64_SUB_X: u8 = BPF_ALU64 | BPF_SUB | BPF_X;
        const BPF_ALU64_MUL_K: u8 = BPF_ALU64 | BPF_MUL | BPF_K;
        const BPF_ALU64_MUL_X: u8 = BPF_ALU64 | BPF_MUL | BPF_X;
        const BPF_ALU64_DIV_K: u8 = BPF_ALU64 | BPF_DIV | BPF_K;
        const BPF_ALU64_DIV_X: u8 = BPF_ALU64 | BPF_DIV | BPF_X;
        const BPF_ALU64_OR_K: u8 = BPF_ALU64 | BPF_OR | BPF_K;
        const BPF_ALU64_OR_X: u8 = BPF_ALU64 | BPF_OR | BPF_X;
        const BPF_ALU64_AND_K: u8 = BPF_ALU64 | BPF_AND | BPF_K;
        const BPF_ALU64_AND_X: u8 = BPF_ALU64 | BPF_AND | BPF_X;
        const BPF_ALU64_LSH_K: u8 = BPF_ALU64 | BPF_LSH | BPF_K;
        const BPF_ALU64_LSH_X: u8 = BPF_ALU64 | BPF_LSH | BPF_X;
        const BPF_ALU64_RSH_K: u8 = BPF_ALU64 | BPF_RSH | BPF_K;
        const BPF_ALU64_RSH_X: u8 = BPF_ALU64 | BPF_RSH | BPF_X;
        const BPF_ALU64_MOD_K: u8 = BPF_ALU64 | BPF_MOD | BPF_K;
        const BPF_ALU64_MOD_X: u8 = BPF_ALU64 | BPF_MOD | BPF_X;
        const BPF_ALU64_XOR_K: u8 = BPF_ALU64 | BPF_XOR | BPF_K;
        const BPF_ALU64_XOR_X: u8 = BPF_ALU64 | BPF_XOR | BPF_X;
        const BPF_ALU64_MOV_K: u8 = BPF_ALU64 | BPF_MOV | BPF_K;
        const BPF_ALU64_MOV_X: u8 = BPF_ALU64 | BPF_MOV | BPF_X;
        const BPF_ALU64_ARSH_K: u8 = BPF_ALU64 | BPF_ARSH | BPF_K;
        const BPF_ALU64_ARSH_X: u8 = BPF_ALU64 | BPF_ARSH | BPF_X;
        const BPF_ALU64_NEG: u8 = BPF_ALU64 | BPF_NEG;

        const BPF_ALU_END_TO_BE: u8 = BPF_ALU | BPF_END | BPF_TO_BE;
        const BPF_ALU_END_TO_LE: u8 = BPF_ALU | BPF_END | BPF_TO_LE;
        const BPF_JMP32_JEQ_K: u8 = BPF_JMP32 | BPF_JEQ | BPF_K;
        const BPF_JMP32_JEQ_X: u8 = BPF_JMP32 | BPF_JEQ | BPF_X;
        const BPF_JMP32_JGT_K: u8 = BPF_JMP32 | BPF_JGT | BPF_K;
        const BPF_JMP32_JGT_X: u8 = BPF_JMP32 | BPF_JGT | BPF_X;
        const BPF_JMP32_JGE_K: u8 = BPF_JMP32 | BPF_JGE | BPF_K;
        const BPF_JMP32_JGE_X: u8 = BPF_JMP32 | BPF_JGE | BPF_X;
        const BPF_JMP32_JSET_K: u8 = BPF_JMP32 | BPF_JSET | BPF_K;
        const BPF_JMP32_JSET_X: u8 = BPF_JMP32 | BPF_JSET | BPF_X;
        const BPF_JMP32_JNE_K: u8 = BPF_JMP32 | BPF_JNE | BPF_K;
        const BPF_JMP32_JNE_X: u8 = BPF_JMP32 | BPF_JNE | BPF_X;
        const BPF_JMP32_JSGT_K: u8 = BPF_JMP32 | BPF_JSGT | BPF_K;
        const BPF_JMP32_JSGT_X: u8 = BPF_JMP32 | BPF_JSGT | BPF_X;
        const BPF_JMP32_JSGE_K: u8 = BPF_JMP32 | BPF_JSGE | BPF_K;
        const BPF_JMP32_JSGE_X: u8 = BPF_JMP32 | BPF_JSGE | BPF_X;
        const BPF_JMP32_JLT_K: u8 = BPF_JMP32 | BPF_JLT | BPF_K;
        const BPF_JMP32_JLT_X: u8 = BPF_JMP32 | BPF_JLT | BPF_X;
        const BPF_JMP32_JLE_K: u8 = BPF_JMP32 | BPF_JLE | BPF_K;
        const BPF_JMP32_JLE_X: u8 = BPF_JMP32 | BPF_JLE | BPF_X;
        const BPF_JMP32_JSLT_K: u8 = BPF_JMP32 | BPF_JSLT | BPF_K;
        const BPF_JMP32_JSLT_X: u8 = BPF_JMP32 | BPF_JSLT | BPF_X;
        const BPF_JMP32_JSLE_K: u8 = BPF_JMP32 | BPF_JSLE | BPF_K;
        const BPF_JMP32_JSLE_X: u8 = BPF_JMP32 | BPF_JSLE | BPF_X;
        const BPF_JMP_JEQ_K: u8 = BPF_JMP | BPF_JEQ | BPF_K;
        const BPF_JMP_JEQ_X: u8 = BPF_JMP | BPF_JEQ | BPF_X;
        const BPF_JMP_JGT_K: u8 = BPF_JMP | BPF_JGT | BPF_K;
        const BPF_JMP_JGT_X: u8 = BPF_JMP | BPF_JGT | BPF_X;
        const BPF_JMP_JGE_K: u8 = BPF_JMP | BPF_JGE | BPF_K;
        const BPF_JMP_JGE_X: u8 = BPF_JMP | BPF_JGE | BPF_X;
        const BPF_JMP_JSET_K: u8 = BPF_JMP | BPF_JSET | BPF_K;
        const BPF_JMP_JSET_X: u8 = BPF_JMP | BPF_JSET | BPF_X;
        const BPF_JMP_JNE_K: u8 = BPF_JMP | BPF_JNE | BPF_K;
        const BPF_JMP_JNE_X: u8 = BPF_JMP | BPF_JNE | BPF_X;
        const BPF_JMP_JSGT_K: u8 = BPF_JMP | BPF_JSGT | BPF_K;
        const BPF_JMP_JSGT_X: u8 = BPF_JMP | BPF_JSGT | BPF_X;
        const BPF_JMP_JSGE_K: u8 = BPF_JMP | BPF_JSGE | BPF_K;
        const BPF_JMP_JSGE_X: u8 = BPF_JMP | BPF_JSGE | BPF_X;
        const BPF_JMP_JLT_K: u8 = BPF_JMP | BPF_JLT | BPF_K;
        const BPF_JMP_JLT_X: u8 = BPF_JMP | BPF_JLT | BPF_X;
        const BPF_JMP_JLE_K: u8 = BPF_JMP | BPF_JLE | BPF_K;
        const BPF_JMP_JLE_X: u8 = BPF_JMP | BPF_JLE | BPF_X;
        const BPF_JMP_JSLT_K: u8 = BPF_JMP | BPF_JSLT | BPF_K;
        const BPF_JMP_JSLT_X: u8 = BPF_JMP | BPF_JSLT | BPF_X;
        const BPF_JMP_JSLE_K: u8 = BPF_JMP | BPF_JSLE | BPF_K;
        const BPF_JMP_JSLE_X: u8 = BPF_JMP | BPF_JSLE | BPF_X;
        const BPF_JMP_JA: u8 = BPF_JMP | BPF_JA;
        const BPF_JMP_CALL: u8 = BPF_JMP | BPF_CALL;
        const BPF_JMP_EXIT: u8 = BPF_JMP | BPF_EXIT;

        const BPF_LD_B_ABS: u8 = BPF_LD | BPF_B | BPF_ABS;
        const BPF_LD_B_IND: u8 = BPF_LD | BPF_B | BPF_IND;
        const BPF_LD_H_ABS: u8 = BPF_LD | BPF_H | BPF_ABS;
        const BPF_LD_H_IND: u8 = BPF_LD | BPF_H | BPF_IND;
        const BPF_LD_W_ABS: u8 = BPF_LD | BPF_W | BPF_ABS;
        const BPF_LD_W_IND: u8 = BPF_LD | BPF_W | BPF_IND;
        const BPF_LD_DW_ABS: u8 = BPF_LD | BPF_DW | BPF_ABS;
        const BPF_LD_DW_IND: u8 = BPF_LD | BPF_DW | BPF_IND;
        const BPF_LDX_B_MEM: u8 = BPF_LDX | BPF_B | BPF_MEM;
        const BPF_LDX_H_MEM: u8 = BPF_LDX | BPF_H | BPF_MEM;
        const BPF_LDX_W_MEM: u8 = BPF_LDX | BPF_W | BPF_MEM;
        const BPF_LDX_DW_MEM: u8 = BPF_LDX | BPF_DW | BPF_MEM;
        const BPF_STX_B_MEM: u8 = BPF_STX | BPF_B | BPF_MEM;
        const BPF_STX_H_MEM: u8 = BPF_STX | BPF_H | BPF_MEM;
        const BPF_STX_W_MEM: u8 = BPF_STX | BPF_W | BPF_MEM;
        const BPF_STX_DW_MEM: u8 = BPF_STX | BPF_DW | BPF_MEM;
        const BPF_ST_B_MEM: u8 = BPF_ST | BPF_B | BPF_MEM;
        const BPF_ST_H_MEM: u8 = BPF_ST | BPF_H | BPF_MEM;
        const BPF_ST_W_MEM: u8 = BPF_ST | BPF_W | BPF_MEM;
        const BPF_ST_DW_MEM: u8 = BPF_ST | BPF_DW | BPF_MEM;
        const BPF_STX_ATOMIC_W: u8 = BPF_STX | BPF_ATOMIC | BPF_W;
        const BPF_STX_ATOMIC_DW: u8 = BPF_STX | BPF_ATOMIC | BPF_DW;

        const BPF_ADD_FETCH: u8 = BPF_ADD | BPF_FETCH;
        const BPF_AND_FETCH: u8 = BPF_AND | BPF_FETCH;
        const BPF_OR_FETCH: u8 = BPF_OR | BPF_FETCH;
        const BPF_XOR_FETCH: u8 = BPF_XOR | BPF_FETCH;

        match inst.code() {
            // 32-bit ALU instructions.
            BPF_ALU_ADD_K => self.add(context, dst, src_imm()),
            BPF_ALU_ADD_X => self.add(context, dst, src_reg()),
            BPF_ALU_SUB_K => self.sub(context, dst, src_imm()),
            BPF_ALU_SUB_X => self.sub(context, dst, src_reg()),
            BPF_ALU_MUL_K => self.mul(context, dst, src_imm()),
            BPF_ALU_MUL_X => self.mul(context, dst, src_reg()),
            BPF_ALU_DIV_K => self.div(context, dst, src_imm()),
            BPF_ALU_DIV_X => self.div(context, dst, src_reg()),
            BPF_ALU_OR_K => self.or(context, dst, src_imm()),
            BPF_ALU_OR_X => self.or(context, dst, src_reg()),
            BPF_ALU_AND_K => self.and(context, dst, src_imm()),
            BPF_ALU_AND_X => self.and(context, dst, src_reg()),
            BPF_ALU_LSH_K => self.lsh(context, dst, src_imm()),
            BPF_ALU_LSH_X => self.lsh(context, dst, src_reg()),
            BPF_ALU_RSH_K => self.rsh(context, dst, src_imm()),
            BPF_ALU_RSH_X => self.rsh(context, dst, src_reg()),
            BPF_ALU_MOD_K => self.r#mod(context, dst, src_imm()),
            BPF_ALU_MOD_X => self.r#mod(context, dst, src_reg()),
            BPF_ALU_XOR_K => self.xor(context, dst, src_imm()),
            BPF_ALU_XOR_X => self.xor(context, dst, src_reg()),
            BPF_ALU_MOV_K => self.mov(context, dst, src_imm()),
            BPF_ALU_MOV_X => self.mov(context, dst, src_reg()),
            BPF_ALU_ARSH_K => self.arsh(context, dst, src_imm()),
            BPF_ALU_ARSH_X => self.arsh(context, dst, src_reg()),
            BPF_ALU_NEG => self.neg(context, dst),

            // 64-bit ALU instructions.
            BPF_ALU64_ADD_K => self.add64(context, dst, src_imm()),
            BPF_ALU64_ADD_X => self.add64(context, dst, src_reg()),
            BPF_ALU64_SUB_K => self.sub64(context, dst, src_imm()),
            BPF_ALU64_SUB_X => self.sub64(context, dst, src_reg()),
            BPF_ALU64_MUL_K => self.mul64(context, dst, src_imm()),
            BPF_ALU64_MUL_X => self.mul64(context, dst, src_reg()),
            BPF_ALU64_DIV_K => self.div64(context, dst, src_imm()),
            BPF_ALU64_DIV_X => self.div64(context, dst, src_reg()),
            BPF_ALU64_OR_K => self.or64(context, dst, src_imm()),
            BPF_ALU64_OR_X => self.or64(context, dst, src_reg()),
            BPF_ALU64_AND_K => self.and64(context, dst, src_imm()),
            BPF_ALU64_AND_X => self.and64(context, dst, src_reg()),
            BPF_ALU64_LSH_K => self.lsh64(context, dst, src_imm()),
            BPF_ALU64_LSH_X => self.lsh64(context, dst, src_reg()),
            BPF_ALU64_RSH_K => self.rsh64(context, dst, src_imm()),
            BPF_ALU64_RSH_X => self.rsh64(context, dst, src_reg()),
            BPF_ALU64_MOD_K => self.mod64(context, dst, src_imm()),
            BPF_ALU64_MOD_X => self.mod64(context, dst, src_reg()),
            BPF_ALU64_XOR_K => self.xor64(context, dst, src_imm()),
            BPF_ALU64_XOR_X => self.xor64(context, dst, src_reg()),
            BPF_ALU64_MOV_K => self.mov64(context, dst, src_imm()),
            BPF_ALU64_MOV_X => self.mov64(context, dst, src_reg()),
            BPF_ALU64_ARSH_K => self.arsh64(context, dst, src_imm()),
            BPF_ALU64_ARSH_X => self.arsh64(context, dst, src_reg()),
            BPF_ALU64_NEG => self.neg64(context, dst),

            // Byte swap instruction.
            BPF_ALU_END_TO_BE => self.be(context, dst, width_from_imm()?),
            BPF_ALU_END_TO_LE => self.le(context, dst, width_from_imm()?),

            // 32-bit conditional jump.
            BPF_JMP32_JEQ_K => self.jeq(context, dst, src_imm(), inst.offset()),
            BPF_JMP32_JEQ_X => self.jeq(context, dst, src_reg(), inst.offset()),
            BPF_JMP32_JGT_K => self.jgt(context, dst, src_imm(), inst.offset()),
            BPF_JMP32_JGT_X => self.jgt(context, dst, src_reg(), inst.offset()),
            BPF_JMP32_JGE_K => self.jge(context, dst, src_imm(), inst.offset()),
            BPF_JMP32_JGE_X => self.jge(context, dst, src_reg(), inst.offset()),
            BPF_JMP32_JSET_K => self.jset(context, dst, src_imm(), inst.offset()),
            BPF_JMP32_JSET_X => self.jset(context, dst, src_reg(), inst.offset()),
            BPF_JMP32_JNE_K => self.jne(context, dst, src_imm(), inst.offset()),
            BPF_JMP32_JNE_X => self.jne(context, dst, src_reg(), inst.offset()),
            BPF_JMP32_JSGT_K => self.jsgt(context, dst, src_imm(), inst.offset()),
            BPF_JMP32_JSGT_X => self.jsgt(context, dst, src_reg(), inst.offset()),
            BPF_JMP32_JSGE_K => self.jsge(context, dst, src_imm(), inst.offset()),
            BPF_JMP32_JSGE_X => self.jsge(context, dst, src_reg(), inst.offset()),
            BPF_JMP32_JLT_K => self.jlt(context, dst, src_imm(), inst.offset()),
            BPF_JMP32_JLT_X => self.jlt(context, dst, src_reg(), inst.offset()),
            BPF_JMP32_JLE_K => self.jle(context, dst, src_imm(), inst.offset()),
            BPF_JMP32_JLE_X => self.jle(context, dst, src_reg(), inst.offset()),
            BPF_JMP32_JSLT_K => self.jslt(context, dst, src_imm(), inst.offset()),
            BPF_JMP32_JSLT_X => self.jslt(context, dst, src_reg(), inst.offset()),
            BPF_JMP32_JSLE_K => self.jsle(context, dst, src_imm(), inst.offset()),
            BPF_JMP32_JSLE_X => self.jsle(context, dst, src_reg(), inst.offset()),

            // 64-bit conditional jump.
            BPF_JMP_JEQ_K => self.jeq64(context, dst, src_imm(), inst.offset()),
            BPF_JMP_JEQ_X => self.jeq64(context, dst, src_reg(), inst.offset()),
            BPF_JMP_JGT_K => self.jgt64(context, dst, src_imm(), inst.offset()),
            BPF_JMP_JGT_X => self.jgt64(context, dst, src_reg(), inst.offset()),
            BPF_JMP_JGE_K => self.jge64(context, dst, src_imm(), inst.offset()),
            BPF_JMP_JGE_X => self.jge64(context, dst, src_reg(), inst.offset()),
            BPF_JMP_JSET_K => self.jset64(context, dst, src_imm(), inst.offset()),
            BPF_JMP_JSET_X => self.jset64(context, dst, src_reg(), inst.offset()),
            BPF_JMP_JNE_K => self.jne64(context, dst, src_imm(), inst.offset()),
            BPF_JMP_JNE_X => self.jne64(context, dst, src_reg(), inst.offset()),
            BPF_JMP_JSGT_K => self.jsgt64(context, dst, src_imm(), inst.offset()),
            BPF_JMP_JSGT_X => self.jsgt64(context, dst, src_reg(), inst.offset()),
            BPF_JMP_JSGE_K => self.jsge64(context, dst, src_imm(), inst.offset()),
            BPF_JMP_JSGE_X => self.jsge64(context, dst, src_reg(), inst.offset()),
            BPF_JMP_JLT_K => self.jlt64(context, dst, src_imm(), inst.offset()),
            BPF_JMP_JLT_X => self.jlt64(context, dst, src_reg(), inst.offset()),
            BPF_JMP_JLE_K => self.jle64(context, dst, src_imm(), inst.offset()),
            BPF_JMP_JLE_X => self.jle64(context, dst, src_reg(), inst.offset()),
            BPF_JMP_JSLT_K => self.jslt64(context, dst, src_imm(), inst.offset()),
            BPF_JMP_JSLT_X => self.jslt64(context, dst, src_reg(), inst.offset()),
            BPF_JMP_JSLE_K => self.jsle64(context, dst, src_imm(), inst.offset()),
            BPF_JMP_JSLE_X => self.jsle64(context, dst, src_reg(), inst.offset()),

            // Unconditional jump
            BPF_JMP_JA => self.jump(context, inst.offset()),

            // Function call.
            BPF_JMP_CALL => match inst.src_reg() {
                0 => self.call_external(context, inst.imm() as u32),
                _ => Err(format!("unsupported call with src = {}", inst.src_reg())),
            },

            // Return.
            BPF_JMP_EXIT => self.exit(context),

            // 64-bit load.
            BPF_LDDW => self.load64(context, dst, inst.src_reg(), inst.imm() as u32),

            // Legacy packet access instructions.
            // All read the packet from R6, storing the result in R0.
            BPF_LD_B_ABS => self.load_from_packet(context, 0, 6, inst.imm(), None, DataWidth::U8),
            BPF_LD_B_IND => self.load_from_packet(
                context,
                0,
                6,
                inst.imm(),
                Some(inst.src_reg()),
                DataWidth::U8,
            ),
            BPF_LD_H_ABS => self.load_from_packet(context, 0, 6, inst.imm(), None, DataWidth::U16),
            BPF_LD_H_IND => self.load_from_packet(
                context,
                0,
                6,
                inst.imm(),
                Some(inst.src_reg()),
                DataWidth::U16,
            ),
            BPF_LD_W_ABS => self.load_from_packet(context, 0, 6, inst.imm(), None, DataWidth::U32),
            BPF_LD_W_IND => self.load_from_packet(
                context,
                0,
                6,
                inst.imm(),
                Some(inst.src_reg()),
                DataWidth::U32,
            ),
            BPF_LD_DW_ABS => self.load_from_packet(context, 0, 6, inst.imm(), None, DataWidth::U64),
            BPF_LD_DW_IND => self.load_from_packet(
                context,
                0,
                6,
                inst.imm(),
                Some(inst.src_reg()),
                DataWidth::U64,
            ),

            // Memory Load.
            BPF_LDX_B_MEM => self.load(context, dst, inst.offset(), inst.src_reg(), DataWidth::U8),
            BPF_LDX_H_MEM => self.load(context, dst, inst.offset(), inst.src_reg(), DataWidth::U16),
            BPF_LDX_W_MEM => self.load(context, dst, inst.offset(), inst.src_reg(), DataWidth::U32),
            BPF_LDX_DW_MEM => {
                self.load(context, dst, inst.offset(), inst.src_reg(), DataWidth::U64)
            }

            // Memory Store register.
            BPF_STX_B_MEM => self.store(context, dst, inst.offset(), src_reg(), DataWidth::U8),
            BPF_STX_H_MEM => self.store(context, dst, inst.offset(), src_reg(), DataWidth::U16),
            BPF_STX_W_MEM => self.store(context, dst, inst.offset(), src_reg(), DataWidth::U32),
            BPF_STX_DW_MEM => self.store(context, dst, inst.offset(), src_reg(), DataWidth::U64),

            // Memory Store constant.
            BPF_ST_B_MEM => self.store(context, dst, inst.offset(), src_imm(), DataWidth::U8),
            BPF_ST_H_MEM => self.store(context, dst, inst.offset(), src_imm(), DataWidth::U16),
            BPF_ST_W_MEM => self.store(context, dst, inst.offset(), src_imm(), DataWidth::U32),
            BPF_ST_DW_MEM => self.store(context, dst, inst.offset(), src_imm(), DataWidth::U64),

            // 32-bit atomic ops.
            BPF_STX_ATOMIC_W => {
                let operation = inst.imm() as u8;
                match operation {
                    BPF_ADD => self.atomic_add(context, false, dst, inst.offset(), inst.src_reg()),
                    BPF_ADD_FETCH => {
                        self.atomic_add(context, true, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_AND => self.atomic_and(context, false, dst, inst.offset(), inst.src_reg()),
                    BPF_AND_FETCH => {
                        self.atomic_and(context, true, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_OR => self.atomic_or(context, false, dst, inst.offset(), inst.src_reg()),
                    BPF_OR_FETCH => {
                        self.atomic_or(context, true, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_XOR => self.atomic_xor(context, false, dst, inst.offset(), inst.src_reg()),
                    BPF_XOR_FETCH => {
                        self.atomic_xor(context, true, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_XCHG => self.atomic_xchg(context, true, dst, inst.offset(), inst.src_reg()),
                    BPF_CMPXCHG => self.atomic_cmpxchg(context, dst, inst.offset(), inst.src_reg()),
                    _ => Err(format!("invalid atomic operation {:x}", operation)),
                }
            }

            // 64-bit atomic ops.
            BPF_STX_ATOMIC_DW => {
                let operation = inst.imm() as u8;
                match operation {
                    BPF_ADD => {
                        self.atomic_add64(context, false, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_ADD_FETCH => {
                        self.atomic_add64(context, true, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_AND => {
                        self.atomic_and64(context, false, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_AND_FETCH => {
                        self.atomic_and64(context, true, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_OR => self.atomic_or64(context, false, dst, inst.offset(), inst.src_reg()),
                    BPF_OR_FETCH => {
                        self.atomic_or64(context, true, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_XOR => {
                        self.atomic_xor64(context, false, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_XOR_FETCH => {
                        self.atomic_xor64(context, true, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_XCHG => {
                        self.atomic_xchg64(context, true, dst, inst.offset(), inst.src_reg())
                    }
                    BPF_CMPXCHG => {
                        self.atomic_cmpxchg64(context, dst, inst.offset(), inst.src_reg())
                    }
                    _ => Err(format!("invalid atomic operation {:x}", operation)),
                }
            }

            _ => Err(format!("invalid op code {:x}", inst.code())),
        }
    }
}
