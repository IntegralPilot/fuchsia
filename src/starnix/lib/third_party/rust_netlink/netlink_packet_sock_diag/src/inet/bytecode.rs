// SPDX-License-Identifier: MIT

//! Functionality for parsing and serializing INET_DIAG bytecode programs.
//!
//! SOCK_DIAG_BY_FAMILY requests with NLM_F_DUMP can accept a bytecode program.
//! The program is run against all of the sockets matching the standard part of
//! the request (though some fields, like socket_id, are not examined at all).
//! If the program accepts a socket, it is returned to the caller. Acceptance is
//! signalled by the program reaching the length of the buffer exactly.
//! Rejection is signalled by the program jumping to somewhere past this.
//!
//! Each instruction is composed of the following basic structure, where `yes`
//! and `no` are how many bytes jump forward if the instruction matches or not.
//! Note that this means there are no loops and all programs trivially must
//! terminate:
//!
//! ```c
//! opcode: u8,
//! yes: u8,
//! no: u16,
//! // Followed (optionally) by parameters for the instruction.
//! ```
//!
//! Instructions are variable-length, which is unwieldy to deal with in Rust, so
//! instead we represent a program as a series of fixed-length instructions,
//! which requires mapping back and forth to byte offsets during parsing and
//! serialization.
//!
//! There is a small loss of fidelity in this Rust representation. The types
//! here encode acception and rejection explicitly, which means there's only a
//! single rejection target. It also encodes NOPs and jumps more simply,
//! forgoing the `yes` and `no` fields entirely. While this shouldn't make a
//! semantic difference, it does mean round-tripping a program might result in a
//! different representation.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::num::NonZeroUsize;

use arbitrary::Arbitrary;
use netlink_packet_utils::{DecodeError, buffer};

use crate::constants::{
    AF_INET, AF_INET6, AF_UNSPEC, INET_DIAG_BC_AUTO, INET_DIAG_BC_CGROUP_COND, INET_DIAG_BC_D_COND,
    INET_DIAG_BC_D_EQ, INET_DIAG_BC_D_GE, INET_DIAG_BC_D_LE, INET_DIAG_BC_DEV_COND,
    INET_DIAG_BC_JMP, INET_DIAG_BC_MARK_COND, INET_DIAG_BC_NOP, INET_DIAG_BC_S_COND,
    INET_DIAG_BC_S_EQ, INET_DIAG_BC_S_GE, INET_DIAG_BC_S_LE,
};

/// Types for keeping track of various `usize`s during parsing.
///
/// The two axes are "byte or instruction" and "index or offset (from current
/// instruction)". They're in a submodule to force everyone to go through the
/// interface.
mod wrappers {
    use std::num::NonZeroUsize;

    /// The absolute index of a parsed [`Instruction`](super::Instruction).
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
    pub(super) struct InstructionIndex(usize);

    impl InstructionIndex {
        pub(super) fn new(offset: usize) -> Self {
            Self(offset)
        }

        pub(super) fn get(self) -> usize {
            let Self(val) = self;
            val
        }

        pub(super) fn checked_add(self, rhs: InstructionOffset) -> Option<Self> {
            let Self(val) = self;
            val.checked_add(rhs.get().get()).map(Self)
        }
    }

    /// The relative offset between two parsed
    /// [`Instruction`s](super::Instruction).
    ///
    /// Guaranteed to be greater than 0.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
    pub(super) struct InstructionOffset(NonZeroUsize);

    impl InstructionOffset {
        pub(super) fn new(offset: usize) -> Option<Self> {
            Some(Self(NonZeroUsize::new(offset)?))
        }

        pub(super) fn new_nonzero(offset: NonZeroUsize) -> Self {
            Self(offset)
        }

        pub(super) fn get(self) -> NonZeroUsize {
            let Self(val) = self;
            val
        }
    }

    /// The absolute index of a byte in an unparsed program.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone, Hash)]
    pub(super) struct ByteIndex(usize);

    impl ByteIndex {
        pub(super) fn new(offset: usize) -> Self {
            Self(offset)
        }

        pub(super) fn get(self) -> usize {
            let Self(val) = self;
            val
        }

        pub(super) fn checked_add(self, rhs: ByteOffset) -> Option<Self> {
            let Self(val) = self;
            val.checked_add(rhs.get().get()).map(Self)
        }

        pub(super) fn checked_sub(self, rhs: ByteIndex) -> Option<ByteOffset> {
            let Self(val) = self;
            val.checked_sub(rhs.0).and_then(ByteOffset::new)
        }
    }

    /// The relative offset between two bytes in a unparsed program.
    ///
    /// Guaranteed to be greater than 0.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
    pub(super) struct ByteOffset(NonZeroUsize);

    impl ByteOffset {
        pub(super) fn new(offset: usize) -> Option<Self> {
            Some(Self(NonZeroUsize::new(offset)?))
        }

        pub(super) fn get(self) -> NonZeroUsize {
            let Self(val) = self;
            val
        }
    }
}

use wrappers::{ByteIndex, ByteOffset, InstructionIndex, InstructionOffset};

/// The size of a Linux `struct inet_diag_bc_op`.
const STRUCT_BC_OP_SIZE: usize = 4;
const DEVICE_COND_SIZE: usize = 4;
const MARK_COND_SIZE: usize = 8;
const CGROUP_COND_SIZE: usize = 8;
const TUPLE_COND_MIN_SIZE: usize = 8;
const AF_INET_ADDR_LEN: usize = 4;
const AF_INET6_ADDR_LEN: usize = 16;

/// A bytecode program used by Linux to match AF_INET sockets.
#[derive(Debug, PartialEq, Eq, Clone, Arbitrary)]
pub struct Bytecode(pub Vec<Instruction>);

#[derive(Debug, PartialEq, Eq)]
pub enum SerializationError {
    /// The target buffer provided was too small.
    BufferTooSmall,
    /// An error occurred during serialization of the instruction at index `at`.
    InvalidInstruction { at: usize, error: InvalidInstructionError },
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum InvalidInstructionError {
    IndexTooLargeForSerializedType,
    IndexOverflow,
    IndexPastEnd,
    PrefixLengthLongerThanAddress,
}

impl InvalidInstructionError {
    fn at_index(self, index: InstructionIndex) -> SerializationError {
        SerializationError::InvalidInstruction { at: index.get(), error: self }
    }
}

/// An error encountered when parsing a program from a raw byte buffer.
#[derive(Debug, PartialEq, Eq)]
pub struct ParseError {
    /// The index in the provided buffer at which the error occurred.
    pub index: usize,
    /// The specific error that occurred.
    pub code: ParseErrorCode,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseErrorCode {
    TruncatedInstruction,
    UnknownOpcode,
    InvalidJumpTarget,
    SelfReference,
    InvalidAddressFamily,
    PrefixLengthLongerThanAddress,
    IndexOverflow,
}

impl ParseErrorCode {
    fn at_index(self, index: ByteIndex) -> ParseError {
        ParseError { index: index.get(), code: self }
    }
}

impl Bytecode {
    /// Returns the length of the serialized form of this bytecode.
    ///
    /// Useful for sizing the buffer passed to [`Bytecode::serialize`].
    pub fn serialized_len(&self) -> usize {
        self.0.iter().map(Instruction::serialized_len).sum()
    }

    /// Parse a bytecode program from the provided buffer.
    pub fn parse(buf: &[u8]) -> Result<Self, ParseError> {
        let mut raw_ops = vec![];
        let mut curr_byte_index = ByteIndex::new(0);
        let buf_len = ByteIndex::new(buf.len());
        let mut instruction_index_by_byte_offset = std::collections::HashMap::new();

        // First, we build up a map of instruction index to byte offset.
        while curr_byte_index < buf_len {
            instruction_index_by_byte_offset
                .insert(curr_byte_index, InstructionIndex::new(raw_ops.len()));
            let inst = RawInstruction::parse(&buf[curr_byte_index.get()..])
                .map_err(|code| code.at_index(curr_byte_index))?;
            let inst_len = inst.serialized_len();
            raw_ops.push((curr_byte_index, inst));
            curr_byte_index = curr_byte_index
                .checked_add(inst_len)
                .ok_or_else(|| ParseErrorCode::IndexOverflow.at_index(curr_byte_index))?;
        }

        // If curr_byte_index < buf_len, we would have looped again.
        // If curr_byte_index > buf_len, we would have returned
        // TruncatedInstruction.
        assert_eq!(curr_byte_index, buf_len);

        // Now, we resolve the raw byte offsets to indexes.

        let raw_accept_offset = curr_byte_index;
        // Linux bytecode validation ensures that there is only a single valid rejection offset.
        let raw_reject_offset = raw_accept_offset
            .checked_add(ByteOffset::new(4).unwrap())
            .ok_or_else(|| ParseErrorCode::IndexOverflow.at_index(ByteIndex::new(0)))?;

        let resolve = |target_offset: ByteIndex, current_index: InstructionIndex| {
            if target_offset == raw_accept_offset {
                Ok(Action::Accept)
            } else if target_offset == raw_reject_offset {
                Ok(Action::Reject)
            } else if let Some(&target_index) = instruction_index_by_byte_offset.get(&target_offset)
            {
                // By construction we know that an instruction can't reference
                // an earlier one, so current_index will always be less than or
                // equal to target_index.
                let offset = target_index.get().checked_sub(current_index.get()).unwrap();
                let index_offset =
                    InstructionOffset::new(offset).ok_or(ParseErrorCode::SelfReference)?;
                Ok(Action::AdvanceBy(index_offset.get()))
            } else {
                Err(ParseErrorCode::InvalidJumpTarget)
            }
        };

        let resolved_ops = raw_ops
            .into_iter()
            .enumerate()
            .map(|(curr_instr_index, (curr_byte_index, raw_op))| {
                let curr_instr_index = InstructionIndex::new(curr_instr_index);

                match raw_op {
                    RawInstruction::Nop(offset) => {
                        let target = curr_byte_index.checked_add(offset).ok_or_else(|| {
                            ParseErrorCode::IndexOverflow.at_index(curr_byte_index)
                        })?;
                        let action = resolve(target, curr_instr_index)
                            .map_err(|code| code.at_index(curr_byte_index))?;
                        Ok(Instruction::Nop(action))
                    }
                    RawInstruction::Jmp(offset) => {
                        let target = curr_byte_index.checked_add(offset).ok_or_else(|| {
                            ParseErrorCode::IndexOverflow.at_index(curr_byte_index)
                        })?;
                        let action = resolve(target, curr_instr_index)
                            .map_err(|code| code.at_index(curr_byte_index))?;
                        Ok(Instruction::Jmp(action))
                    }
                    RawInstruction::Condition { yes, no, condition } => {
                        let yes_target = curr_byte_index.checked_add(yes).ok_or_else(|| {
                            ParseErrorCode::IndexOverflow.at_index(curr_byte_index)
                        })?;
                        let no_target = curr_byte_index.checked_add(no).ok_or_else(|| {
                            ParseErrorCode::IndexOverflow.at_index(curr_byte_index)
                        })?;

                        let yes = resolve(yes_target, curr_instr_index)
                            .map_err(|code| code.at_index(curr_byte_index))?;
                        let no = resolve(no_target, curr_instr_index)
                            .map_err(|code| code.at_index(curr_byte_index))?;
                        Ok(Instruction::Condition { yes, no, condition })
                    }
                }
            })
            .collect::<Result<Vec<_>, ParseError>>()?;

        Ok(Bytecode(resolved_ops))
    }

    /// Serialize the bytecode into the provided buffer.
    pub fn serialize(self, buf: &mut [u8]) -> Result<(), SerializationError> {
        let Self(instructions) = self;

        let mut total_len = ByteIndex::new(0);
        let byte_indices_by_instruction_index: Vec<_> =
            instructions
                .iter()
                .enumerate()
                .map(|(i, inst)| {
                    let res = total_len;
                    match total_len.checked_add(ByteOffset::new(inst.serialized_len()).unwrap()) {
                        Some(new_len) => {
                            total_len = new_len;
                            Ok(res)
                        }
                        None => Err(InvalidInstructionError::IndexOverflow
                            .at_index(InstructionIndex::new(i))),
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;

        if total_len.get() > buf.len() {
            return Err(SerializationError::BufferTooSmall);
        }

        instructions.into_iter().enumerate().try_for_each(|(curr_inst_index, inst)| {
            let curr_inst_index = InstructionIndex::new(curr_inst_index);
            let curr_byte_index = byte_indices_by_instruction_index[curr_inst_index.get()];

            inst.try_into_raw(
                &byte_indices_by_instruction_index,
                curr_inst_index,
                curr_byte_index,
                total_len,
            )
            .and_then(|raw| raw.serialize(&mut buf[curr_byte_index.get()..]))
            .map_err(|e| e.at_index(curr_inst_index))
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Arbitrary)]
pub enum Instruction {
    Nop(Action),
    Jmp(Action),
    Condition { yes: Action, no: Action, condition: Condition },
}

impl Instruction {
    fn serialized_len(&self) -> usize {
        STRUCT_BC_OP_SIZE
            + match self {
                Self::Nop(_) | Self::Jmp(_) => 0,
                Self::Condition { condition, .. } => condition.serialized_len(),
            }
    }

    fn try_into_raw(
        self,
        byte_indices_by_instruction_index: &[ByteIndex],
        instruction_index: InstructionIndex,
        byte_index: ByteIndex,
        total_len: ByteIndex,
    ) -> Result<RawInstruction, InvalidInstructionError> {
        // Calculate relative offsets
        let calculate_rel = |action| {
            let target = match action {
                Action::Accept => total_len,
                // Linux checks that all targets are multiples of 4.
                Action::Reject => total_len
                    .checked_add(ByteOffset::new(4).unwrap())
                    .ok_or(InvalidInstructionError::IndexOverflow)?,
                Action::AdvanceBy(dist) => {
                    let target_index = instruction_index
                        .checked_add(InstructionOffset::new_nonzero(dist))
                        .ok_or(InvalidInstructionError::IndexOverflow)?;
                    *byte_indices_by_instruction_index
                        .get(target_index.get())
                        .ok_or(InvalidInstructionError::IndexPastEnd)?
                }
            };

            // This is safe because the elements of offsets are strictly
            // increasing, so indexing into my_index+dist (and we know dist
            // can't be zero because of its type) must give a larger value.
            Ok(target.checked_sub(byte_index).unwrap())
        };

        match self {
            Instruction::Nop(action) => Ok(RawInstruction::Nop(calculate_rel(action)?)),
            Instruction::Jmp(action) => Ok(RawInstruction::Jmp(calculate_rel(action)?)),
            Instruction::Condition { yes, no, condition } => Ok(RawInstruction::Condition {
                yes: calculate_rel(yes)?,
                no: calculate_rel(no)?,
                condition,
            }),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Arbitrary)]
pub enum Action {
    Accept,
    Reject,
    AdvanceBy(NonZeroUsize),
}

enum RawInstruction {
    Nop(ByteOffset),
    Jmp(ByteOffset),
    Condition { yes: ByteOffset, no: ByteOffset, condition: Condition },
}

buffer!(RawInstructionBuffer(STRUCT_BC_OP_SIZE) {
    code: (u8, 0),
    yes: (u8, 1),
    no: (u16, 2..4),
    payload: (slice, STRUCT_BC_OP_SIZE..),
});

impl RawInstruction {
    fn serialized_len(&self) -> ByteOffset {
        ByteOffset::new(
            STRUCT_BC_OP_SIZE
                + match self {
                    Self::Nop(_) => 0,
                    Self::Jmp(_) => 0,
                    Self::Condition { condition, .. } => condition.serialized_len(),
                },
        )
        .unwrap()
    }

    fn parse(buf: &[u8]) -> Result<RawInstruction, ParseErrorCode> {
        let buf =
            RawInstructionBuffer::new(buf).map_err(|_| ParseErrorCode::TruncatedInstruction)?;

        let code = buf.code();
        let yes = ByteOffset::new(buf.yes().into());
        let no = ByteOffset::new(buf.no().into());

        // Handle these separately because they don't follow the same pattern
        // for how to handle yes and no as the other instructions.
        if code == INET_DIAG_BC_NOP {
            return Ok(RawInstruction::Nop(yes.ok_or(ParseErrorCode::SelfReference)?));
        } else if code == INET_DIAG_BC_JMP {
            return Ok(RawInstruction::Jmp(no.ok_or(ParseErrorCode::SelfReference)?));
        }

        fn port_cond<F>(buf: &[u8], f: F) -> Result<Condition, ParseErrorCode>
        where
            F: FnOnce(u16) -> Condition,
        {
            match PortConditionBuffer::new(buf) {
                Ok(buf) => Ok(f(buf.port())),
                Err(_) => Err(ParseErrorCode::TruncatedInstruction),
            }
        }

        // Put the condition at the beginning of buf.
        let payload = buf.payload();
        let condition = match code {
            // Handled above.
            INET_DIAG_BC_NOP => unreachable!(),
            INET_DIAG_BC_JMP => unreachable!(),

            INET_DIAG_BC_S_COND => TupleCondition::parse(payload).map(Condition::SrcTuple),
            INET_DIAG_BC_D_COND => TupleCondition::parse(payload).map(Condition::DstTuple),
            INET_DIAG_BC_DEV_COND => match DeviceConditionBuffer::new(payload) {
                Ok(buf) => Ok(Condition::Device(buf.ifindex())),
                Err(_) => Err(ParseErrorCode::TruncatedInstruction),
            },
            INET_DIAG_BC_MARK_COND => match MarkConditionBuffer::new(payload) {
                Ok(buf) => Ok(Condition::Mark { mark: buf.mark(), mask: buf.mask() }),
                Err(_) => Err(ParseErrorCode::TruncatedInstruction),
            },
            INET_DIAG_BC_S_EQ => port_cond(payload, Condition::SrcPortEq),
            INET_DIAG_BC_D_EQ => port_cond(payload, Condition::DstPortEq),
            INET_DIAG_BC_S_GE => port_cond(payload, Condition::SrcPortGe),
            INET_DIAG_BC_D_GE => port_cond(payload, Condition::DstPortGe),
            INET_DIAG_BC_S_LE => port_cond(payload, Condition::SrcPortLe),
            INET_DIAG_BC_D_LE => port_cond(payload, Condition::DstPortLe),
            INET_DIAG_BC_AUTO => Ok(Condition::AutoPort),
            INET_DIAG_BC_CGROUP_COND => match CgroupConditionBuffer::new(payload) {
                Ok(buf) => Ok(Condition::Cgroup(buf.cgroup_id())),
                Err(_) => Err(ParseErrorCode::TruncatedInstruction),
            },
            _ => Err(ParseErrorCode::UnknownOpcode),
        }?;

        let yes = yes.ok_or(ParseErrorCode::SelfReference)?;
        let no = no.ok_or(ParseErrorCode::SelfReference)?;

        let inst = RawInstruction::Condition { yes, no, condition };

        Ok(inst)
    }

    fn serialize(&self, buf: &mut [u8]) -> Result<(), InvalidInstructionError> {
        // NOTE: buffer length was already checked in Bytecode::serialize, so we
        // don't need to do that again in this function.

        let (code, yes, no, condition) = match self {
            Self::Nop(offset) => (INET_DIAG_BC_NOP, offset.get().get(), 0, None),
            // Linux requires that the yes field always points at the next
            // instruction, even though JMP doesn't use it.
            Self::Jmp(offset) => (INET_DIAG_BC_JMP, STRUCT_BC_OP_SIZE, offset.get().get(), None),
            Self::Condition { yes, no, condition } => {
                (condition.code(), yes.get().get(), no.get().get(), Some(condition))
            }
        };

        let mut buf = RawInstructionBuffer::new(buf).unwrap();
        buf.set_code(code);
        buf.set_yes(
            yes.try_into().map_err(|_| InvalidInstructionError::IndexTooLargeForSerializedType)?,
        );
        buf.set_no(
            u16::try_from(no)
                .map_err(|_| InvalidInstructionError::IndexTooLargeForSerializedType)?,
        );

        let buf = buf.payload_mut();
        if let Some(condition) = condition {
            match condition {
                Condition::AutoPort => {}
                Condition::SrcPortGe(port)
                | Condition::SrcPortLe(port)
                | Condition::DstPortGe(port)
                | Condition::DstPortLe(port)
                | Condition::SrcPortEq(port)
                | Condition::DstPortEq(port) => {
                    let mut buf = PortConditionBuffer::new(buf).unwrap();
                    buf.set_port(*port);
                }
                Condition::SrcTuple(c) | Condition::DstTuple(c) => c.serialize(buf)?,
                Condition::Device(ifindex) => {
                    let mut buf = DeviceConditionBuffer::new(buf).unwrap();
                    buf.set_ifindex(*ifindex);
                }
                Condition::Mark { mark, mask } => {
                    let mut buf = MarkConditionBuffer::new(buf).unwrap();
                    buf.set_mark(*mark);
                    buf.set_mask(*mask);
                }
                Condition::Cgroup(cgroup_id) => {
                    let mut buf = CgroupConditionBuffer::new(buf).unwrap();
                    buf.set_cgroup_id(*cgroup_id);
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Arbitrary)]
pub enum Condition {
    SrcPortGe(u16),
    SrcPortLe(u16),
    DstPortGe(u16),
    DstPortLe(u16),
    SrcPortEq(u16),
    DstPortEq(u16),
    AutoPort,
    SrcTuple(TupleCondition),
    DstTuple(TupleCondition),
    Device(u32),
    Mark { mark: u32, mask: u32 },
    Cgroup(u64),
}

// Linux uses a struct inet_diag_bc_op for the condition payload, but just the
// `no` field.
buffer!(PortConditionBuffer(STRUCT_BC_OP_SIZE) {
    padding: (slice, 0..1),
    port: (u16, 2..4),
});

buffer!(DeviceConditionBuffer(DEVICE_COND_SIZE) {
    ifindex: (u32, 0..4),
});

buffer!(MarkConditionBuffer(MARK_COND_SIZE) {
    mark: (u32, 0..4),
    mask: (u32, 4..8),
});

buffer!(CgroupConditionBuffer(CGROUP_COND_SIZE) {
    cgroup_id: (u64, 0..8),
});

impl Condition {
    fn serialized_len(&self) -> usize {
        match self {
            Condition::AutoPort => 0,
            // Linux puts the port in the no field of a struct inet_diag_bc_op.
            Condition::SrcPortGe(_)
            | Condition::SrcPortLe(_)
            | Condition::DstPortGe(_)
            | Condition::DstPortLe(_)
            | Condition::SrcPortEq(_)
            | Condition::DstPortEq(_) => STRUCT_BC_OP_SIZE,
            Condition::SrcTuple(c) | Condition::DstTuple(c) => c.serialized_len(),
            Condition::Device(_) => DEVICE_COND_SIZE,
            Condition::Mark { .. } => MARK_COND_SIZE,
            Condition::Cgroup(_) => CGROUP_COND_SIZE,
        }
    }

    fn code(&self) -> u8 {
        match self {
            Self::SrcPortGe(_) => INET_DIAG_BC_S_GE,
            Self::SrcPortLe(_) => INET_DIAG_BC_S_LE,
            Self::DstPortGe(_) => INET_DIAG_BC_D_GE,
            Self::DstPortLe(_) => INET_DIAG_BC_D_LE,
            Self::AutoPort => INET_DIAG_BC_AUTO,
            Self::SrcTuple(_) => INET_DIAG_BC_S_COND,
            Self::DstTuple(_) => INET_DIAG_BC_D_COND,
            Self::Device(_) => INET_DIAG_BC_DEV_COND,
            Self::Mark { .. } => INET_DIAG_BC_MARK_COND,
            Self::SrcPortEq(_) => INET_DIAG_BC_S_EQ,
            Self::DstPortEq(_) => INET_DIAG_BC_D_EQ,
            Self::Cgroup(_) => INET_DIAG_BC_CGROUP_COND,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Arbitrary)]
pub struct TupleCondition {
    pub prefix_len: u8,
    pub addr: Option<IpAddr>,
    pub port: Option<u16>,
}

buffer!(TupleConditionBuffer(TUPLE_COND_MIN_SIZE) {
    family: (u8, 0),
    prefix_len: (u8, 1),
    port: (i32, 4..8),
    payload: (slice, TUPLE_COND_MIN_SIZE..),
});

buffer!(Ipv4AddrBuffer(AF_INET_ADDR_LEN) {
    addr: (u32, 0..AF_INET_ADDR_LEN),
});

buffer!(Ipv6AddrBuffer(AF_INET6_ADDR_LEN) {
    addr: (u128, 0..AF_INET6_ADDR_LEN),
});

impl TupleCondition {
    fn serialized_len(&self) -> usize {
        TUPLE_COND_MIN_SIZE
            + match self.addr {
                Some(IpAddr::V4(_)) => AF_INET_ADDR_LEN,
                Some(IpAddr::V6(_)) => AF_INET6_ADDR_LEN,
                None => 0,
            }
    }

    fn parse(buf: &[u8]) -> Result<Self, ParseErrorCode> {
        let buf =
            TupleConditionBuffer::new(buf).map_err(|_| ParseErrorCode::TruncatedInstruction)?;
        let family = buf.family();
        let prefix_len = buf.prefix_len();
        let port = buf.port();
        let port = if port == -1 { None } else { Some(port as u16) };

        let payload = buf.payload();
        let addr = match family {
            AF_INET => match Ipv4AddrBuffer::new(payload) {
                Ok(buf) => Ok(Some(IpAddr::V4(Ipv4Addr::from(buf.addr())))),
                Err(_) => Err(ParseErrorCode::TruncatedInstruction),
            },
            AF_INET6 => match Ipv6AddrBuffer::new(payload) {
                Ok(buf) => Ok(Some(IpAddr::V6(Ipv6Addr::from(buf.addr())))),
                Err(_) => Err(ParseErrorCode::TruncatedInstruction),
            },
            AF_UNSPEC => Ok(None),
            _ => Err(ParseErrorCode::InvalidAddressFamily),
        }?;

        let max_prefix_len = addr
            .map(|a| match a {
                IpAddr::V4(_) => AF_INET_ADDR_LEN,
                IpAddr::V6(_) => AF_INET6_ADDR_LEN,
            })
            .unwrap_or(0)
            * 8;

        if usize::from(prefix_len) > max_prefix_len {
            return Err(ParseErrorCode::PrefixLengthLongerThanAddress);
        }

        Ok(TupleCondition { prefix_len, port, addr })
    }

    fn serialize(&self, buf: &mut [u8]) -> Result<(), InvalidInstructionError> {
        let mut buf = TupleConditionBuffer::new(buf).unwrap();

        let max_prefix_len = match self.addr {
            Some(IpAddr::V4(addr)) => {
                buf.set_family(AF_INET);
                Ipv4AddrBuffer::new(buf.payload_mut()).unwrap().set_addr(addr.into());

                32
            }
            Some(IpAddr::V6(addr)) => {
                buf.set_family(AF_INET6);
                Ipv6AddrBuffer::new(buf.payload_mut()).unwrap().set_addr(addr.into());

                128
            }
            None => {
                buf.set_family(AF_UNSPEC);

                0
            }
        };

        if self.prefix_len > max_prefix_len {
            return Err(InvalidInstructionError::PrefixLengthLongerThanAddress);
        }

        buf.set_prefix_len(self.prefix_len);
        buf.set_port(self.port.map(i32::from).unwrap_or(-1));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_roundtrip() {
        let conditions = vec![
            Condition::SrcPortGe(100),
            Condition::SrcPortLe(200),
            Condition::DstPortGe(300),
            Condition::DstPortLe(400),
            Condition::SrcPortEq(500),
            Condition::DstPortEq(600),
            Condition::AutoPort,
            Condition::SrcTuple(TupleCondition {
                prefix_len: 24,
                port: Some(8080),
                addr: Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))),
            }),
            Condition::SrcTuple(TupleCondition {
                prefix_len: 128,
                port: Some(8081),
                addr: Some(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))),
            }),
            Condition::DstTuple(TupleCondition {
                prefix_len: 24,
                port: Some(9090),
                addr: Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))),
            }),
            Condition::DstTuple(TupleCondition {
                prefix_len: 64,
                port: None,
                addr: Some(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))),
            }),
            Condition::Device(1),
            Condition::Mark { mark: 0x1234, mask: 0xFFFF },
            Condition::Cgroup(123456789),
        ];

        for condition in conditions {
            let bc = Bytecode(vec![
                Instruction::Condition {
                    yes: Action::AdvanceBy(NonZeroUsize::new(1).unwrap()),
                    no: Action::Accept,
                    condition: condition.clone(),
                },
                Instruction::Nop(Action::Accept),
            ]);

            let mut buf = vec![0u8; bc.serialized_len()];
            bc.clone().serialize(&mut buf).unwrap();
            let parsed = Bytecode::parse(&buf)
                .unwrap_or_else(|e| panic!("parse failed for {:?}: {:?}", condition, e));
            assert_eq!(parsed, bc, "roundtrip failed for {:?}", condition);
        }
    }

    #[test]
    fn accept_reject_mapping() {
        let bc = Bytecode(vec![Instruction::Jmp(Action::Accept), Instruction::Jmp(Action::Reject)]);

        let mut buf = vec![0u8; bc.serialized_len()];
        bc.clone().serialize(&mut buf).unwrap();

        assert_eq!(
            buf,
            [
                INET_DIAG_BC_JMP,
                4, // yes
                8u16.to_ne_bytes()[0],
                8u16.to_ne_bytes()[1], // no
                INET_DIAG_BC_JMP,
                4, // yes
                8u16.to_ne_bytes()[0],
                8u16.to_ne_bytes()[1], // no
            ]
        );

        let parsed = Bytecode::parse(&buf).unwrap();
        assert_eq!(parsed, bc);
    }

    #[test]
    fn buffer_too_small() {
        let bc = Bytecode(vec![Instruction::Nop(Action::AdvanceBy(NonZeroUsize::new(1).unwrap()))]);
        let mut buf = vec![0u8; 3]; // Nop is 4 bytes
        assert_eq!(bc.serialize(&mut buf), Err(SerializationError::BufferTooSmall));
    }

    #[test]
    fn index_too_large_yes() {
        const COUNT: usize = 64;

        let mut ops = vec![Instruction::Condition {
            yes: Action::AdvanceBy(NonZeroUsize::new(COUNT + 1).unwrap()),
            no: Action::Accept,
            condition: Condition::AutoPort,
        }];
        // Each NOP is 4 bytes, so 64 NOPs is 256 bytes.
        ops.extend(
            (0..COUNT).map(|_| Instruction::Nop(Action::AdvanceBy(NonZeroUsize::new(1).unwrap()))),
        );
        ops.push(Instruction::Nop(Action::AdvanceBy(NonZeroUsize::new(1).unwrap()))); // Target

        let bc = Bytecode(ops);
        let mut buf = vec![0u8; bc.serialized_len()];
        assert_eq!(
            bc.serialize(&mut buf),
            Err(SerializationError::InvalidInstruction {
                at: 0,
                error: InvalidInstructionError::IndexTooLargeForSerializedType
            })
        );
    }

    #[test]
    fn index_too_large_no() {
        const COUNT: usize = 16384;

        let mut ops =
            vec![Instruction::Jmp(Action::AdvanceBy(NonZeroUsize::new(COUNT + 1).unwrap()))];
        ops.extend(
            (0..COUNT).map(|_| Instruction::Nop(Action::AdvanceBy(NonZeroUsize::new(1).unwrap()))),
        );
        ops.push(Instruction::Nop(Action::AdvanceBy(NonZeroUsize::new(1).unwrap()))); // Target

        let bc = Bytecode(ops);
        let mut buf = vec![0u8; bc.serialized_len()];
        assert_eq!(
            bc.serialize(&mut buf),
            Err(SerializationError::InvalidInstruction {
                at: 0,
                error: InvalidInstructionError::IndexTooLargeForSerializedType
            })
        );
    }

    #[test]
    fn index_overflow() {
        let ops = vec![
            Instruction::Nop(Action::AdvanceBy(NonZeroUsize::new(1).unwrap())),
            Instruction::Jmp(Action::AdvanceBy(NonZeroUsize::MAX)),
        ];
        let bc = Bytecode(ops);
        let mut buf = vec![0u8; bc.serialized_len()];
        assert_eq!(
            bc.serialize(&mut buf),
            Err(SerializationError::InvalidInstruction {
                at: 1,
                error: InvalidInstructionError::IndexOverflow
            })
        );
    }

    #[test]
    fn advance_by_mapping() {
        let bc = Bytecode(vec![
            Instruction::Jmp(Action::AdvanceBy(NonZeroUsize::new(2).unwrap())),
            Instruction::Nop(Action::AdvanceBy(NonZeroUsize::new(1).unwrap())),
            Instruction::Nop(Action::Accept),
        ]);

        let mut buf = vec![0u8; bc.serialized_len()];
        bc.clone().serialize(&mut buf).unwrap();

        let parsed = Bytecode::parse(&buf).unwrap();
        assert_eq!(parsed, bc);
    }

    #[test]
    fn parse_errors() {
        // Invalid bytecode!
        let buf = vec![255, 4, 0, 0];
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::UnknownOpcode })
        );

        // Invalid target jump (jumping into the middle of an instruction).
        let mut buf = vec![];
        buf.push(INET_DIAG_BC_NOP);
        buf.push(4); // yes
        buf.extend_from_slice(&4u16.to_ne_bytes()); // no

        buf.push(INET_DIAG_BC_MARK_COND);
        buf.push(4); // yes.
        buf.extend_from_slice(&6u16.to_ne_bytes()); // no. Middle of the next instruction. Invalid!
        buf.extend_from_slice(&0u32.to_ne_bytes());
        buf.extend_from_slice(&0u32.to_ne_bytes());

        buf.push(INET_DIAG_BC_JMP);
        buf.push(4);
        buf.extend_from_slice(&4u16.to_ne_bytes());
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 4, code: ParseErrorCode::InvalidJumpTarget })
        );

        // Truncated instruction body (SrcPortGe missing bytes)
        // S_GE: (4 bytes) + 4 bytes payload.
        let mut buf = Vec::new();
        buf.push(INET_DIAG_BC_S_GE);
        buf.push(4); // yes.
        buf.extend_from_slice(&4u16.to_ne_bytes());
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::TruncatedInstruction })
        );

        // Invalid self-reference (yes=0).
        let mut buf = Vec::new();
        buf.push(INET_DIAG_BC_AUTO);
        buf.push(0); // yes. Invalid!
        buf.extend_from_slice(&4u16.to_ne_bytes()); // no
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::SelfReference })
        );

        // Invalid self-reference (no=0).
        let mut buf = Vec::new();
        buf.push(INET_DIAG_BC_AUTO);
        buf.push(4); // yes
        buf.extend_from_slice(&0u16.to_ne_bytes()); // no. Invalid!
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::SelfReference })
        );

        // Invalid address family.
        let mut buf = Vec::new();
        buf.push(INET_DIAG_BC_S_COND);
        buf.push(12); // yes
        buf.extend_from_slice(&4u16.to_ne_bytes()); // no
        buf.push(255); // Invalid family
        buf.push(0); // prefix len
        buf.push(0); // pad
        buf.push(0); // pad
        buf.extend_from_slice(&(-1i32).to_ne_bytes()); // port (none)
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::InvalidAddressFamily })
        );

        // Prefix length longer than address.
        let mut buf = Vec::new();
        buf.push(INET_DIAG_BC_S_COND);
        buf.push(16); // yes
        buf.extend_from_slice(&4u16.to_ne_bytes()); // no
        buf.push(AF_INET);
        buf.push(33); // prefix len
        buf.push(0); // pad
        buf.push(0); // pad
        buf.extend_from_slice(&(-1i32).to_ne_bytes()); // port (none)
        buf.extend_from_slice(&[0, 0, 0, 0]); // address
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::PrefixLengthLongerThanAddress })
        );
    }

    #[test]
    fn truncated_payloads() {
        let mut buf = Vec::new();
        buf.push(INET_DIAG_BC_DEV_COND);
        buf.push(8); // yes
        buf.extend_from_slice(&4u16.to_ne_bytes()); // no
        buf.extend_from_slice(&[0; 3]); // 3 bytes instead of 4
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::TruncatedInstruction })
        );

        let mut buf = Vec::new();
        buf.push(INET_DIAG_BC_MARK_COND);
        buf.push(12); // yes
        buf.extend_from_slice(&4u16.to_ne_bytes()); // no
        buf.extend_from_slice(&[0; 7]); // 7 bytes instead of 8
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::TruncatedInstruction })
        );

        let mut buf = Vec::new();
        buf.push(INET_DIAG_BC_CGROUP_COND);
        buf.push(12); // yes
        buf.extend_from_slice(&4u16.to_ne_bytes()); // no
        buf.extend_from_slice(&[0; 7]); // 7 bytes instead of 8
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::TruncatedInstruction })
        );

        let mut buf = Vec::new();
        buf.push(INET_DIAG_BC_S_COND);
        buf.push(12); // yes
        buf.extend_from_slice(&4u16.to_ne_bytes()); // no
        buf.extend_from_slice(&[0; 7]); // 7 bytes instead of 8 (min header size)
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::TruncatedInstruction })
        );

        let mut buf = Vec::new();
        buf.push(INET_DIAG_BC_S_COND);
        buf.push(16); // yes
        buf.extend_from_slice(&4u16.to_ne_bytes()); // no
        buf.push(AF_INET);
        buf.push(0); // prefix len
        buf.push(0);
        buf.push(0); // pad
        buf.extend_from_slice(&(-1i32).to_ne_bytes()); // port (none)
        buf.extend_from_slice(&[0; 3]); // 3 bytes instead of 4 for IPv4
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::TruncatedInstruction })
        );

        let mut buf = Vec::new();
        buf.push(INET_DIAG_BC_S_COND);
        buf.push(28); // yes
        buf.extend_from_slice(&4u16.to_ne_bytes()); // no
        buf.push(AF_INET6);
        buf.push(0); // prefix len
        buf.push(0);
        buf.push(0); // pad
        buf.extend_from_slice(&(-1i32).to_ne_bytes()); // port (none)
        buf.extend_from_slice(&[0; 15]); // 15 bytes instead of 16 for IPv6
        assert_eq!(
            Bytecode::parse(&buf),
            Err(ParseError { index: 0, code: ParseErrorCode::TruncatedInstruction })
        );
    }

    #[test]
    fn index_past_end() {
        let bc = Bytecode(vec![Instruction::Nop(Action::AdvanceBy(NonZeroUsize::new(2).unwrap()))]);
        let mut buf = vec![0u8; bc.serialized_len()];
        assert_eq!(
            bc.serialize(&mut buf),
            Err(SerializationError::InvalidInstruction {
                at: 0,
                error: InvalidInstructionError::IndexPastEnd
            })
        );
    }

    #[test]
    fn prefix_length_longer_than_address_v4() {
        let bc = Bytecode(vec![Instruction::Condition {
            yes: Action::Accept,
            no: Action::Accept,
            condition: Condition::SrcTuple(TupleCondition {
                prefix_len: 33,
                port: None,
                addr: Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
            }),
        }]);
        let mut buf = vec![0u8; bc.serialized_len()];
        assert_eq!(
            bc.serialize(&mut buf),
            Err(SerializationError::InvalidInstruction {
                at: 0,
                error: InvalidInstructionError::PrefixLengthLongerThanAddress
            })
        );
    }

    #[test]
    fn prefix_length_longer_than_address_v6() {
        let bc = Bytecode(vec![Instruction::Condition {
            yes: Action::Accept,
            no: Action::Accept,
            condition: Condition::SrcTuple(TupleCondition {
                prefix_len: 129,
                port: None,
                addr: Some(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))),
            }),
        }]);
        let mut buf = vec![0u8; bc.serialized_len()];
        assert_eq!(
            bc.serialize(&mut buf),
            Err(SerializationError::InvalidInstruction {
                at: 0,
                error: InvalidInstructionError::PrefixLengthLongerThanAddress
            })
        );
    }

    #[test]
    fn prefix_length_longer_than_address_unspec() {
        let bc = Bytecode(vec![Instruction::Condition {
            yes: Action::Accept,
            no: Action::Accept,
            condition: Condition::SrcTuple(TupleCondition {
                prefix_len: 1,
                port: None,
                addr: None,
            }),
        }]);
        let mut buf = vec![0u8; bc.serialized_len()];
        assert_eq!(
            bc.serialize(&mut buf),
            Err(SerializationError::InvalidInstruction {
                at: 0,
                error: InvalidInstructionError::PrefixLengthLongerThanAddress
            })
        );
    }
}
