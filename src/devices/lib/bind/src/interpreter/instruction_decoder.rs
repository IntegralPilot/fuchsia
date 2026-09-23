// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bytecode_constants::*;
use crate::compiler::Symbol;
use crate::interpreter::common::*;
use crate::parser::bind_library;
use num_traits::FromPrimitive;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct DecodedCondition {
    pub is_equal: bool,
    pub lhs: Symbol,
    pub rhs: Symbol,
}

// Verifies and converts instruction bytecode into a set of DecodedInstructions.
// TODO(https://fxbug.dev/42175142): Add IDs to the Label and Jump statements.
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum DecodedInstruction {
    UnconditionalAbort,
    Condition(DecodedCondition),
    Jump(Option<DecodedCondition>),
    Label,
}

#[derive(Debug, Clone)]
pub struct InstructionDecoder<'a> {
    symbol_table: &'a HashMap<u32, String>,
    inst_iter: BytecodeIter<'a>,
}

impl<'a> InstructionDecoder<'a> {
    pub fn new(
        symbol_table: &'a HashMap<u32, String>,
        instructions: &'a Vec<u8>,
    ) -> InstructionDecoder<'a> {
        InstructionDecoder { symbol_table: symbol_table, inst_iter: instructions.iter() }
    }

    pub fn decode(&mut self) -> Result<Vec<DecodedInstruction>, BytecodeError> {
        let mut decoded_instructions: Vec<DecodedInstruction> = vec![];
        while let Some(byte) = self.inst_iter.next() {
            let op_byte = FromPrimitive::from_u8(*byte).ok_or(BytecodeError::InvalidOp(*byte))?;
            let instruction = match op_byte {
                RawOp::UnconditionalJump | RawOp::JumpIfEqual | RawOp::JumpIfNotEqual => {
                    self.decode_control_flow_statement(op_byte)?
                }
                RawOp::EqualCondition | RawOp::InequalCondition => DecodedInstruction::Condition(
                    self.decode_conditional_statement(op_byte == RawOp::EqualCondition)?,
                ),
                RawOp::Abort => DecodedInstruction::UnconditionalAbort,
                RawOp::JumpLandPad => DecodedInstruction::Label,
            };
            decoded_instructions.push(instruction);
        }

        Ok(decoded_instructions)
    }

    fn decode_control_flow_statement(
        &mut self,
        op_byte: RawOp,
    ) -> Result<DecodedInstruction, BytecodeError> {
        // TODO(https://fxbug.dev/42175045): verify offset amount takes you to a jump landing pad.
        let offset_amount = next_u32(&mut self.inst_iter)?;

        let condition = match op_byte {
            RawOp::JumpIfEqual => Some(self.decode_conditional_statement(true)?),
            RawOp::JumpIfNotEqual => Some(self.decode_conditional_statement(false)?),
            RawOp::UnconditionalJump => None,
            _ => {
                return Err(BytecodeError::InvalidOp(op_byte as u8));
            }
        };

        if self.inst_iter.len() as u32 <= offset_amount {
            return Err(BytecodeError::InvalidJumpLocation);
        }

        Ok(DecodedInstruction::Jump(condition))
    }

    fn decode_conditional_statement(
        &mut self,
        is_equal: bool,
    ) -> Result<DecodedCondition, BytecodeError> {
        // Read in the LHS value first, followed by the RHS value.
        let lhs = self.decode_value()?;
        let rhs = self.decode_value()?;
        Ok(DecodedCondition { is_equal: is_equal, lhs: lhs, rhs: rhs })
    }

    fn decode_value(&mut self) -> Result<Symbol, BytecodeError> {
        let val_primitive = *next_u8(&mut self.inst_iter)?;
        let val_type = FromPrimitive::from_u8(val_primitive)
            .ok_or(BytecodeError::InvalidValueType(val_primitive))?;
        let val = next_u32(&mut self.inst_iter)?;

        match val_type {
            RawValueType::NumberValue => Ok(Symbol::NumberValue(val as u64)),
            RawValueType::BoolValue => match val {
                FALSE_VAL => Ok(Symbol::BoolValue(false)),
                TRUE_VAL => Ok(Symbol::BoolValue(true)),
                _ => Err(BytecodeError::InvalidBoolValue(val)),
            },
            RawValueType::Key => {
                Ok(Symbol::Key(self.lookup_symbol_table(val)?, bind_library::ValueType::Str))
            }
            RawValueType::StringValue => Ok(Symbol::StringValue(self.lookup_symbol_table(val)?)),
            RawValueType::EnumValue => Ok(Symbol::EnumValue(self.lookup_symbol_table(val)?)),
        }
    }

    fn lookup_symbol_table(&self, key: u32) -> Result<String, BytecodeError> {
        self.symbol_table
            .get(&key)
            .ok_or(BytecodeError::MissingEntryInSymbolTable(key))
            .map(|val| val.to_string())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::interpreter::decode_bind_rules::{DecodedRules, NODE_TYPE_HEADER_SZ};
    use crate::interpreter::test_common::*;

    fn append_section_header(bytecode: &mut Vec<u8>, magic_num: u32, sz: u32) {
        bytecode.extend_from_slice(&magic_num.to_be_bytes());
        bytecode.extend_from_slice(&sz.to_le_bytes());
    }

    fn append_node_header(bytecode: &mut Vec<u8>, node_type: RawParentType, node_id: u32, sz: u32) {
        bytecode.push(node_type as u8);
        bytecode.extend_from_slice(&node_id.to_le_bytes());
        bytecode.extend_from_slice(&sz.to_le_bytes());
    }

    #[test]
    fn test_invalid_value_type() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x01, 0x01, 0, 0, 0, 0x05, 0x10, 0, 0, 0, 0];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);

        assert_eq!(Err(BytecodeError::InvalidValueType(0x10)), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_value_string_missing_in_symbols() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x01, 0x02, 0, 0, 0, 0x05, 0x10, 0, 0, 0, 0];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);

        assert_eq!(
            Err(BytecodeError::MissingEntryInSymbolTable(0x05000000)),
            DecodedRules::new(bytecode)
        );
    }

    #[test]
    fn test_value_enum_missing_in_symbols() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x01, 0x04, 0, 0, 0, 0x05, 0x10, 0, 0, 0, 0];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);

        assert_eq!(
            Err(BytecodeError::MissingEntryInSymbolTable(0x05000000)),
            DecodedRules::new(bytecode)
        );
    }

    #[test]
    fn test_value_invalid_bool() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x01, 0x01, 0, 0, 0, 0x05, 0x03, 0, 0, 0, 0x01];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);

        assert_eq!(Err(BytecodeError::InvalidBoolValue(0x01000000)), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_invalid_outofbounds_jump_offset() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [
            0x11, 0x01, 0, 0, 0, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0,
            0, // jump 1 if 0x05000000 == 0x10
            0x30, 0x20, // abort, jump pad
            0x10, 0x02, 0, 0,
            0, // jump 2 (this is the invalid jump as there is only 2 bytes left)
            0x30, 0x20, // abort, jump pad
        ];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);

        // The last jump would put your instruction pointer past the last element.
        assert_eq!(Err(BytecodeError::InvalidJumpLocation), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_invalid_outofbounds_jump_offset_composite_primary() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        // There is a jump 4 that would go out of bounds for the primary node instructions.
        let primary_node_inst =
            [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0, 0x10, 0x04, 0, 0, 0];

        let additional_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x02, 0, 0, 0x10];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(Err(BytecodeError::InvalidJumpLocation), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_invalid_outofbounds_jump_offset_composite_additional() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        let primary_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x01, 0, 0, 0, 0x10];

        // There is a jump 4 that would go out of bounds for the additional node instructions.
        let additional_node_inst =
            [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0, 0x10, 0x04, 0, 0, 0];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(Err(BytecodeError::InvalidJumpLocation), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_invalid_value_type_composite_primary() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        // There is no value type enum for 0x05.
        let primary_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x05, 0, 0, 0, 0x10];

        let additional_node_inst = [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(Err(BytecodeError::InvalidValueType(0x05)), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_invalid_value_type_composite_additional() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        let primary_node_inst = [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        // There is no value type enum for 0x05.
        let additional_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x05, 0, 0, 0, 0x10];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(Err(BytecodeError::InvalidValueType(0x05)), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_value_key_missing_in_symbols_composite_primary() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        // There is no key type (type 0x00) with key 0x10000000.
        let primary_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x00, 0, 0, 0, 0x10];

        let additional_node_inst = [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(
            Err(BytecodeError::MissingEntryInSymbolTable(0x10000000)),
            DecodedRules::new(bytecode)
        );
    }

    #[test]
    fn test_value_key_missing_in_symbols_composite_additional() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        let primary_node_inst = [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        // There is no key type (type 0x00) with key 0x10000000.
        let additional_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x00, 0, 0, 0, 0x10];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(
            Err(BytecodeError::MissingEntryInSymbolTable(0x10000000)),
            DecodedRules::new(bytecode)
        );
    }

    #[test]
    fn test_value_string_missing_in_symbols_composite_primary() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        // There is no string literal (type 0x02) with key 0x10000000.
        let primary_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x02, 0, 0, 0, 0x10];

        let additional_node_inst = [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(
            Err(BytecodeError::MissingEntryInSymbolTable(0x10000000)),
            DecodedRules::new(bytecode)
        );
    }

    #[test]
    fn test_value_string_missing_in_symbols_composite_additional() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        let primary_node_inst = [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        // There is no string literal (type 0x02) with key 0x10000000.
        let additional_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x02, 0, 0, 0, 0x10];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(
            Err(BytecodeError::MissingEntryInSymbolTable(0x10000000)),
            DecodedRules::new(bytecode)
        );
    }

    #[test]
    fn test_value_enum_missing_in_symbols_composite_primary() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        // There is no enum value (type 0x04) with key 0x10000000.
        let primary_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x04, 0, 0, 0, 0x10];

        let additional_node_inst = [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(
            Err(BytecodeError::MissingEntryInSymbolTable(0x10000000)),
            DecodedRules::new(bytecode)
        );
    }

    #[test]
    fn test_value_enum_missing_in_symbols_composite_additional() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        let primary_node_inst = [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        // There is no enum value (type 0x04) with key 0x10000000.
        let additional_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x04, 0, 0, 0, 0x10];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(
            Err(BytecodeError::MissingEntryInSymbolTable(0x10000000)),
            DecodedRules::new(bytecode)
        );
    }

    #[test]
    fn test_value_invalid_bool_composite_primary() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        // 0x10000000 is not a valid bool value.
        let primary_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x03, 0, 0, 0, 0x10];

        let additional_node_inst = [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(Err(BytecodeError::InvalidBoolValue(0x10000000)), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_value_invalid_bool_composite_additional() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 27);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        let primary_node_inst = [0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        // 0x10000000 is not a valid bool value.
        let additional_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x03, 0, 0, 0, 0x10];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        // Add the additional node.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        assert_eq!(Err(BytecodeError::InvalidBoolValue(0x10000000)), DecodedRules::new(bytecode));
    }
}
