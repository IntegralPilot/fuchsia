// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::memio::EbpfPtr;
use crate::visitor::{BpfVisitor, ProgramCounter, Register, Source};
use crate::{
    BPF_STACK_SIZE, BpfValue, DataWidth, EbpfInstruction, EbpfProgramContext, FromBpfValue,
    GENERAL_REGISTER_COUNT, HelperSet, Packet,
};
use byteorder::{BigEndian, ByteOrder, LittleEndian};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use zerocopy::IntoBytes;

pub fn execute<C: EbpfProgramContext>(
    code: &[EbpfInstruction],
    helpers: &HelperSet<C>,
    run_context: &mut C::RunContext<'_>,
    arguments: &[BpfValue],
) -> u64 {
    assert!(arguments.len() < 5);
    let mut context = ComputationContext {
        code,
        helpers,
        registers: Default::default(),
        stack: [MaybeUninit::uninit(); BPF_STACK_SIZE / std::mem::size_of::<BpfValue>()],
        pc: 0,
        result: None,
    };
    for (i, v) in arguments.iter().enumerate() {
        // Arguments are in registers r1 to r5.
        context.set_reg((i as u8) + 1, *v);
    }

    // R10 points at the stack.
    context.registers[10] =
        BpfValue::from((context.stack.as_mut_ptr() as u64) + (BPF_STACK_SIZE as u64));

    loop {
        if let Some(result) = context.result {
            return result;
        }
        context.visit(run_context, code[context.pc]).expect("verifier should have found an issue");
        context.next();
    }
}

impl BpfValue {
    #[inline(always)]
    pub fn add(&self, offset: u64) -> Self {
        Self::from(self.as_u64().overflowing_add(offset).0)
    }
}

/// The state of the computation as known by the interpreter at a given point in time.
struct ComputationContext<'a, C: EbpfProgramContext> {
    /// The program being executed.
    code: &'a [EbpfInstruction],
    /// Helpers.
    helpers: &'a HelperSet<C>,
    /// Registers.
    registers: [BpfValue; GENERAL_REGISTER_COUNT as usize + 1],
    /// The state of the stack.
    stack: [MaybeUninit<BpfValue>; BPF_STACK_SIZE / std::mem::size_of::<BpfValue>()],
    /// The program counter.
    pc: ProgramCounter,
    /// The result, set to Some(value) when the program terminates.
    result: Option<u64>,
}

impl<C: EbpfProgramContext> ComputationContext<'_, C> {
    #[inline(always)]
    fn reg(&mut self, index: Register) -> BpfValue {
        self.registers[index as usize]
    }

    #[inline(always)]
    fn set_reg(&mut self, index: Register, value: BpfValue) {
        self.registers[index as usize] = value;
    }

    #[inline(always)]
    fn next(&mut self) {
        self.advance_pc(1)
    }

    /// Adds `offset` to the program counter in `ComputationContext`.
    #[inline(always)]
    fn advance_pc(&mut self, offset: i16) {
        let mut pc = self.pc as i64;
        pc += offset as i64;
        self.pc = pc as usize;
    }

    #[inline(always)]
    fn store_memory(
        &mut self,
        addr: BpfValue,
        value: BpfValue,
        instruction_offset: u64,
        width: DataWidth,
    ) {
        let addr = addr.add(instruction_offset);
        match width {
            DataWidth::U8 => {
                // SAFETY: Verifier ensures that the store operation is safe.
                unsafe { EbpfPtr::new(addr.as_ptr::<u8>()).store_relaxed(value.as_u8()) }
            }
            DataWidth::U16 => {
                // SAFETY: Verifier ensures that the store operation is safe.
                unsafe { EbpfPtr::new(addr.as_ptr::<u16>()).store_relaxed(value.as_u16()) }
            }
            DataWidth::U32 => {
                // SAFETY: Verifier ensures that the store operation is safe.
                unsafe { EbpfPtr::new(addr.as_ptr::<u32>()).store_relaxed(value.as_u32()) }
            }
            DataWidth::U64 => {
                // SAFETY: Verifier ensures that the store operation is safe.
                unsafe { EbpfPtr::new(addr.as_ptr::<u64>()).store_relaxed(value.as_u64()) }
            }
        }
    }

    #[inline(always)]
    fn load_memory(&self, addr: BpfValue, instruction_offset: u64, width: DataWidth) -> BpfValue {
        let addr = addr.add(instruction_offset);
        match width {
            DataWidth::U8 => {
                // SAFETY: Verifier ensures that the load is safe.
                BpfValue::from(unsafe { EbpfPtr::new(addr.as_ptr::<u8>()).load_relaxed() })
            }
            DataWidth::U16 => {
                // SAFETY: Verifier ensures that the load is safe.
                BpfValue::from(unsafe { EbpfPtr::new(addr.as_ptr::<u16>()).load_relaxed() })
            }
            DataWidth::U32 => {
                // SAFETY: Verifier ensures that the load is safe.
                BpfValue::from(unsafe { EbpfPtr::new(addr.as_ptr::<u32>()).load_relaxed() })
            }
            DataWidth::U64 => {
                // SAFETY: Verifier ensures that the load is safe.
                BpfValue::from(unsafe { EbpfPtr::new(addr.as_ptr::<u64>()).load_relaxed() })
            }
        }
    }

    #[inline(always)]
    fn compute_source(&mut self, src: Source) -> BpfValue {
        match src {
            Source::Reg(reg) => self.reg(reg),
            Source::Value(v) => v.into(),
        }
    }

    #[inline(always)]
    fn alu(
        &mut self,
        dst: Register,
        src: Source,
        op: impl Fn(u64, u64) -> u64,
    ) -> Result<(), String> {
        let op1 = self.reg(dst).as_u64();
        let op2 = self.compute_source(src).as_u64();
        let result = op(op1, op2);
        self.set_reg(dst, result.into());
        Ok(())
    }

    #[inline(always)]
    fn atomic_operation(
        &mut self,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
        op: impl Fn(&mut Self, &AtomicU32, u32) -> u32,
    ) -> Result<(), String> {
        let addr = self.reg(dst).add(offset as u64);
        // TODO How to statically check alignment?
        if addr.as_usize() % std::mem::size_of::<AtomicU32>() != 0 {
            return Err(format!("misaligned access"));
        }
        // SAFETY
        //
        // The address has been verified by the verifier that ensured the memory is valid for
        // reading and writing.
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let atomic = unsafe { &*addr.as_ptr::<AtomicU32>() };
        let value = self.reg(src).as_u32();
        let old_value = op(self, atomic, value);
        if fetch {
            self.set_reg(src, old_value.into());
        }
        Ok(())
    }

    #[inline(always)]
    fn atomic_operation64(
        &mut self,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
        op: impl Fn(&mut Self, &AtomicU64, u64) -> u64,
    ) -> Result<(), String> {
        let addr = self.reg(dst).add(offset as u64);
        // TODO How to statically check alignment?
        if addr.as_usize() % std::mem::size_of::<AtomicU64>() != 0 {
            return Err(format!("misaligned access"));
        }
        // SAFETY
        //
        // The address has been verified by the verifier that ensured the memory is valid for
        // reading and writing.
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let atomic = unsafe { &*addr.as_ptr::<AtomicU64>() };
        let value = self.reg(src).as_u64();
        let old_value = op(self, atomic, value);
        if fetch {
            self.set_reg(src, old_value.into());
        }
        Ok(())
    }

    #[inline(always)]
    fn endianness<BO: ByteOrder>(&mut self, dst: Register, width: DataWidth) -> Result<(), String> {
        let value = self.reg(dst);
        let new_value = match width {
            DataWidth::U16 => BO::read_u16((value.as_u64() as u16).as_bytes()) as u64,
            DataWidth::U32 => BO::read_u32((value.as_u64() as u32).as_bytes()) as u64,
            DataWidth::U64 => BO::read_u64(value.as_u64().as_bytes()),
            _ => {
                panic!("Unexpected bit width for endianness operation");
            }
        };
        self.set_reg(dst, new_value.into());
        Ok(())
    }

    #[inline(always)]
    fn conditional_jump(
        &mut self,
        dst: Register,
        src: Source,
        offset: i16,
        op: impl Fn(u64, u64) -> bool,
    ) -> Result<(), String> {
        let op1 = self.reg(dst).as_u64();
        let op2 = self.compute_source(src.clone()).as_u64();
        if op(op1, op2) {
            self.advance_pc(offset);
        }
        Ok(())
    }
}

impl<C: EbpfProgramContext> BpfVisitor for ComputationContext<'_, C> {
    type Context<'a> = C::RunContext<'a>;

    #[inline(always)]
    fn add<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| alu32(x, y, |x, y| x.overflowing_add(y).0))
    }
    #[inline(always)]
    fn add64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| x.overflowing_add(y).0)
    }
    #[inline(always)]
    fn and<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| alu32(x, y, |x, y| x & y))
    }
    #[inline(always)]
    fn and64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| x & y)
    }
    #[inline(always)]
    fn arsh<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| {
            alu32(x, y, |x, y| {
                let x = x as i32;
                x.overflowing_shr(y).0 as u32
            })
        })
    }
    #[inline(always)]
    fn arsh64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| {
            let x = x as i64;
            // ebpf mask shift operation to the number of bytes, as does `overflowing_sh{rl}`
            // rust operation. So, it is valid to just cast it to `u32`.
            let y = y as u32;
            x.overflowing_shr(y).0 as u64
        })
    }
    #[inline(always)]
    fn div<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| alu32(x, y, |x, y| if y == 0 { 0 } else { x / y }))
    }
    #[inline(always)]
    fn div64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| if y == 0 { 0 } else { x / y })
    }
    #[inline(always)]
    fn lsh<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| alu32(x, y, |x, y| x.overflowing_shl(y).0))
    }
    #[inline(always)]
    fn lsh64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| {
            // ebpf mask shift operation to the number of bytes, as does `overflowing_sh{rl}`
            // rust operation. So, it is valid to just cast it to `u32`.
            let y = y as u32;
            x.overflowing_shl(y).0
        })
    }
    #[inline(always)]
    fn r#mod<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| alu32(x, y, |x, y| if y == 0 { x } else { x % y }))
    }
    #[inline(always)]
    fn mod64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| if y == 0 { x } else { x % y })
    }
    #[inline(always)]
    fn mov<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| alu32(x, y, |_x, y| y))
    }
    #[inline(always)]
    fn mov64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |_x, y| y)
    }
    #[inline(always)]
    fn mul<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| alu32(x, y, |x, y| x.overflowing_mul(y).0))
    }
    #[inline(always)]
    fn mul64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| x.overflowing_mul(y).0)
    }
    #[inline(always)]
    fn or<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| alu32(x, y, |x, y| x | y))
    }
    #[inline(always)]
    fn or64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| x | y)
    }
    #[inline(always)]
    fn rsh<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| alu32(x, y, |x, y| x.overflowing_shr(y).0))
    }
    #[inline(always)]
    fn rsh64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| {
            // ebpf mask shift operation to the number of bytes, as does `overflowing_sh{rl}`
            // rust operation. So, it is valid to just cast it to `u32`.
            let y = y as u32;
            x.overflowing_shr(y).0
        })
    }
    #[inline(always)]
    fn sub<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| alu32(x, y, |x, y| x.overflowing_sub(y).0))
    }
    #[inline(always)]
    fn sub64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| x.overflowing_sub(y).0)
    }
    #[inline(always)]
    fn xor<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| alu32(x, y, |x, y| x ^ y))
    }
    #[inline(always)]
    fn xor64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
    ) -> Result<(), String> {
        self.alu(dst, src, |x, y| x ^ y)
    }

    #[inline(always)]
    fn neg<'a>(&mut self, _context: &mut Self::Context<'a>, dst: Register) -> Result<(), String> {
        self.alu(dst, Source::Value(0), |x, y| {
            alu32(x, y, |x, _y| (x as i32).overflowing_neg().0 as u32)
        })
    }
    #[inline(always)]
    fn neg64<'a>(&mut self, _context: &mut Self::Context<'a>, dst: Register) -> Result<(), String> {
        self.alu(dst, Source::Value(0), |x, _y| (x as i64).overflowing_neg().0 as u64)
    }

    #[inline(always)]
    fn be<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        width: DataWidth,
    ) -> Result<(), String> {
        self.endianness::<BigEndian>(dst, width)
    }

    #[inline(always)]
    fn le<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        width: DataWidth,
    ) -> Result<(), String> {
        self.endianness::<LittleEndian>(dst, width)
    }

    #[inline(always)]
    fn call_external<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        index: u32,
    ) -> Result<(), String> {
        let helper = &self.helpers.get_by_index(index).unwrap();
        let result =
            helper.0(context, self.reg(1), self.reg(2), self.reg(3), self.reg(4), self.reg(5));
        self.set_reg(0, result);
        Ok(())
    }

    #[inline(always)]
    fn exit<'a>(&mut self, _context: &mut Self::Context<'a>) -> Result<(), String> {
        self.result = Some(self.reg(0).as_u64());
        Ok(())
    }

    #[inline(always)]
    fn jump<'a>(&mut self, _context: &mut Self::Context<'a>, offset: i16) -> Result<(), String> {
        self.advance_pc(offset);
        Ok(())
    }

    #[inline(always)]
    fn jeq<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| comp32(x, y, |x, y| x == y))
    }
    #[inline(always)]
    fn jeq64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| x == y)
    }
    #[inline(always)]
    fn jne<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| comp32(x, y, |x, y| x != y))
    }
    #[inline(always)]
    fn jne64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| x != y)
    }
    #[inline(always)]
    fn jge<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| comp32(x, y, |x, y| x >= y))
    }
    #[inline(always)]
    fn jge64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| x >= y)
    }
    #[inline(always)]
    fn jgt<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| comp32(x, y, |x, y| x > y))
    }
    #[inline(always)]
    fn jgt64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| x > y)
    }
    #[inline(always)]
    fn jle<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| comp32(x, y, |x, y| x <= y))
    }
    #[inline(always)]
    fn jle64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| x <= y)
    }
    #[inline(always)]
    fn jlt<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| comp32(x, y, |x, y| x < y))
    }
    #[inline(always)]
    fn jlt64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| x < y)
    }
    #[inline(always)]
    fn jsge<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| scomp32(x, y, |x, y| x >= y))
    }
    #[inline(always)]
    fn jsge64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| scomp64(x, y, |x, y| x >= y))
    }
    #[inline(always)]
    fn jsgt<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| scomp32(x, y, |x, y| x > y))
    }
    #[inline(always)]
    fn jsgt64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| scomp64(x, y, |x, y| x > y))
    }
    #[inline(always)]
    fn jsle<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| scomp32(x, y, |x, y| x <= y))
    }
    #[inline(always)]
    fn jsle64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| scomp64(x, y, |x, y| x <= y))
    }
    #[inline(always)]
    fn jslt<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| scomp32(x, y, |x, y| x < y))
    }
    #[inline(always)]
    fn jslt64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| scomp64(x, y, |x, y| x < y))
    }
    #[inline(always)]
    fn jset<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| comp32(x, y, |x, y| x & y != 0))
    }
    #[inline(always)]
    fn jset64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        src: Source,
        offset: i16,
    ) -> Result<(), String> {
        self.conditional_jump(dst, src, offset, |x, y| x & y != 0)
    }

    #[inline(always)]
    fn atomic_add<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation(fetch, dst, offset, src, |_, a, v| a.fetch_add(v, Ordering::SeqCst))
    }

    #[inline(always)]
    fn atomic_add64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation64(fetch, dst, offset, src, |_, a, v| a.fetch_add(v, Ordering::SeqCst))
    }

    #[inline(always)]
    fn atomic_and<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation(fetch, dst, offset, src, |_, a, v| a.fetch_and(v, Ordering::SeqCst))
    }

    #[inline(always)]
    fn atomic_and64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation64(fetch, dst, offset, src, |_, a, v| a.fetch_and(v, Ordering::SeqCst))
    }

    #[inline(always)]
    fn atomic_or<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation(fetch, dst, offset, src, |_, a, v| a.fetch_or(v, Ordering::SeqCst))
    }

    #[inline(always)]
    fn atomic_or64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation64(fetch, dst, offset, src, |_, a, v| a.fetch_or(v, Ordering::SeqCst))
    }

    #[inline(always)]
    fn atomic_xor<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation(fetch, dst, offset, src, |_, a, v| a.fetch_xor(v, Ordering::SeqCst))
    }

    #[inline(always)]
    fn atomic_xor64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation64(fetch, dst, offset, src, |_, a, v| a.fetch_xor(v, Ordering::SeqCst))
    }

    #[inline(always)]
    fn atomic_xchg<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation(fetch, dst, offset, src, |_, a, v| a.swap(v, Ordering::SeqCst))
    }

    #[inline(always)]
    fn atomic_xchg64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        fetch: bool,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation64(fetch, dst, offset, src, |_, a, v| a.swap(v, Ordering::SeqCst))
    }

    #[inline(always)]
    fn atomic_cmpxchg<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation(false, dst, offset, src, |this, a, v| {
            let r0 = this.reg(0).as_u32();
            let r0 = match a.compare_exchange(r0, v, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(v) | Err(v) => v,
            };
            this.set_reg(0, r0.into());
            0
        })
    }

    #[inline(always)]
    fn atomic_cmpxchg64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        offset: i16,
        src: Register,
    ) -> Result<(), String> {
        self.atomic_operation64(false, dst, offset, src, |this, a, v| {
            let r0 = this.reg(0).as_u64();
            let r0 = match a.compare_exchange(r0, v, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(v) | Err(v) => v,
            };
            this.set_reg(0, r0.into());
            0
        })
    }

    #[inline(always)]
    fn load<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        offset: i16,
        src: Register,
        width: DataWidth,
    ) -> Result<(), String> {
        let addr = self.reg(src);
        let loaded = self.load_memory(addr, offset as u64, width);
        self.set_reg(dst, loaded);
        Ok(())
    }

    #[inline(always)]
    fn load64<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        _src: u8,
        lower: u32,
    ) -> Result<(), String> {
        let value = (lower as u64) | (((self.code[self.pc + 1].imm() as u32) as u64) << 32);
        self.set_reg(dst, value.into());
        self.advance_pc(1);
        Ok(())
    }

    #[inline(always)]
    fn load_from_packet<'a>(
        &mut self,
        context: &mut Self::Context<'a>,
        dst_reg: Register,
        src_reg: Register,
        offset: i32,
        register_offset: Option<Register>,
        width: DataWidth,
    ) -> Result<(), String> {
        let Some(offset) =
            register_offset.map(|r| self.reg(r).as_i32()).unwrap_or(0).checked_add(offset as i32)
        else {
            // Offset overflowed. Exit.
            self.result = Some(0);
            return Ok(());
        };
        let src_reg = self.reg(src_reg);
        // SAFETY: The verifier checks that the `src_reg` points at packet.
        let packet = unsafe { C::Packet::from_bpf_value(context, src_reg) };
        if let Some(value) = packet.load(offset, width) {
            self.set_reg(dst_reg, value.into());
        } else {
            self.result = Some(0);
        }
        Ok(())
    }

    #[inline(always)]
    fn store<'a>(
        &mut self,
        _context: &mut Self::Context<'a>,
        dst: Register,
        offset: i16,
        src: Source,
        width: DataWidth,
    ) -> Result<(), String> {
        let src = self.compute_source(src);
        let dst = self.reg(dst);
        self.store_memory(dst, src, offset as u64, width);
        Ok(())
    }
}

#[inline(always)]
fn alu32(x: u64, y: u64, op: impl FnOnce(u32, u32) -> u32) -> u64 {
    op(x as u32, y as u32) as u64
}

#[inline(always)]
fn comp32(x: u64, y: u64, op: impl FnOnce(u32, u32) -> bool) -> bool {
    op(x as u32, y as u32)
}

#[inline(always)]
fn scomp64(x: u64, y: u64, op: impl FnOnce(i64, i64) -> bool) -> bool {
    op(x as i64, y as i64)
}

#[inline(always)]
fn scomp32(x: u64, y: u64, op: impl FnOnce(i32, i32) -> bool) -> bool {
    op(x as i32, y as i32)
}
