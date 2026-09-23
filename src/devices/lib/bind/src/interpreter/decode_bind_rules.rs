// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bytecode_constants::*;
use crate::interpreter::common::*;
use crate::interpreter::instruction_decoder::*;
use num_traits::FromPrimitive;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Each node section header contains a u8 node type and a uint32 section
// size.
pub const NODE_TYPE_HEADER_SZ: usize = 9;

// At minimum, the bytecode would contain the bind header, symbol table
// header and instruction header.
const MINIMUM_BYTECODE_SZ: usize = HEADER_SZ * 3;

// Each debug flag byte contains 1 byte
const DEBUG_FLAG_SZ: usize = 1;

// Parse through the bytecode and separate out the symbol table, instructions
// and an optional debug section. Verify the bytecode header and the symbol table
// header.
fn get_symbol_table_and_instruction_debug_bytecode(
    bytecode: Vec<u8>,
) -> Result<(HashMap<u32, String>, Vec<u8>, Option<Vec<u8>>), BytecodeError> {
    if bytecode.len() < MINIMUM_BYTECODE_SZ {
        return Err(BytecodeError::UnexpectedEnd);
    }

    // Remove the bytecode header and verify the bytecode version.
    let (version, bytecode) = read_and_remove_header(bytecode, BIND_MAGIC_NUM)?;
    if version != BYTECODE_VERSION {
        return Err(BytecodeError::InvalidVersion(version));
    }

    // Remove the enable_debug flag byte from the bytecode
    let (debug_flag_byte, bytecode) = read_and_remove_debug_flag(bytecode)?;

    // Remove the symbol table header and verify that the size is less than
    // the remaining bytecode.
    let (symbol_table_sz, mut symbol_table_bytecode) =
        read_and_remove_header(bytecode, SYMB_MAGIC_NUM)?;
    if symbol_table_bytecode.len() < symbol_table_sz as usize + HEADER_SZ {
        return Err(BytecodeError::IncorrectSectionSize);
    }

    // Split the instruction bytecode from the symbol table bytecode.
    let mut inst_bytecode = symbol_table_bytecode.split_off(symbol_table_sz as usize);

    // Read in the instruction section size and verify that the size is correct.
    let inst_bytecode_size = u32::from_le_bytes(get_u32_bytes(&inst_bytecode, 4)?);
    if inst_bytecode.len() < inst_bytecode_size as usize + HEADER_SZ {
        return Err(BytecodeError::IncorrectSectionSize);
    }

    // Split the debug bytecode from the instruction bytecode if the debug flag
    // byte is true.
    let debug_bytecode = if debug_flag_byte == BYTECODE_ENABLE_DEBUG {
        let debug_result = inst_bytecode.split_off(inst_bytecode_size as usize + HEADER_SZ);
        let (debug_sz, debug_result_vec) = read_and_remove_header(debug_result, DEBG_MAGIC_NUM)?;
        if debug_result_vec.len() != debug_sz as usize {
            return Err(BytecodeError::IncorrectSectionSize);
        }
        Some(debug_result_vec)
    } else {
        None
    };

    Ok((read_symbol_table(symbol_table_bytecode)?, inst_bytecode, debug_bytecode))
}

// Remove the instructions in the first node and return it along with the
// remaining bytecode.
fn split_off_parent(
    mut bytecode: Vec<u8>,
    expect_primary: bool,
    symbol_table: &HashMap<u32, String>,
) -> Result<(bool, Parent, Vec<u8>), BytecodeError> {
    // Verify the node type and retrieve the node section size.
    let (is_optional, node_id, node_inst_sz) =
        verify_and_read_node_header(&bytecode, expect_primary)?;
    if bytecode.len() < NODE_TYPE_HEADER_SZ + node_inst_sz as usize {
        return Err(BytecodeError::IncorrectParentSectionSize);
    }

    if !symbol_table.contains_key(&node_id) {
        return Err(BytecodeError::MissingParentIdInSymbolTable);
    }

    let mut node_instructions = bytecode.split_off(NODE_TYPE_HEADER_SZ);
    let remaining_bytecode = node_instructions.split_off(node_inst_sz as usize);
    let decoded_instructions =
        InstructionDecoder::new(symbol_table, &node_instructions).decode()?;
    Ok((
        is_optional,
        Parent {
            name_id: node_id,
            instructions: node_instructions,
            decoded_instructions: decoded_instructions,
        },
        remaining_bytecode,
    ))
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum DecodedRules {
    Normal(DecodedBindRules),
    Composite(DecodedCompositeBindRules),
}

impl DecodedRules {
    pub fn new(bytecode: Vec<u8>) -> Result<Self, BytecodeError> {
        let (symbol_table, inst_bytecode, debug_bytecode) =
            get_symbol_table_and_instruction_debug_bytecode(bytecode)?;
        let parsed_magic_num = u32::from_be_bytes(get_u32_bytes(&inst_bytecode, 0)?);
        if parsed_magic_num == COMPOSITE_MAGIC_NUM {
            return Ok(DecodedRules::Composite(DecodedCompositeBindRules::new(
                symbol_table,
                inst_bytecode,
                decode_debug_bytecode(debug_bytecode)?,
            )?));
        }
        Ok(DecodedRules::Normal(DecodedBindRules::new(
            symbol_table,
            inst_bytecode,
            decode_debug_bytecode(debug_bytecode)?,
        )?))
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct DecodedDebugInfo {
    pub symbol_table: HashMap<u32, String>,
}

impl DecodedDebugInfo {
    pub fn new(debug_bytecode: Vec<u8>) -> Result<Self, BytecodeError> {
        // Verify the debug symbol table header and size.
        let (debug_sz, debug_sym_bytecode) =
            read_and_remove_header(debug_bytecode, DBSY_MAGIC_NUM)?;
        if debug_sym_bytecode.len() != debug_sz as usize {
            return Err(BytecodeError::IncorrectSectionSize);
        }
        // Read in the debug symbol table.
        let symbol_table = read_symbol_table(debug_sym_bytecode)?;

        Ok(DecodedDebugInfo { symbol_table: symbol_table })
    }
}

// Decode the debug bytecode if it's not empty.
pub fn decode_debug_bytecode(
    debug_bytecode: Option<Vec<u8>>,
) -> Result<Option<DecodedDebugInfo>, BytecodeError> {
    if debug_bytecode.is_none() {
        return Ok(None);
    }
    let bytecode = debug_bytecode.unwrap();
    if bytecode.is_empty() {
        return Ok(None);
    }
    return Ok(Some(DecodedDebugInfo::new(bytecode)?));
}

// This struct decodes and unwraps the given bytecode into a symbol table
// and list of instructions. It contains an optional debug section.
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct DecodedBindRules {
    pub symbol_table: HashMap<u32, String>,
    pub instructions: Vec<u8>,
    pub decoded_instructions: Vec<DecodedInstruction>,
    pub debug_info: Option<DecodedDebugInfo>,
}

impl DecodedBindRules {
    pub fn new(
        symbol_table: HashMap<u32, String>,
        inst_bytecode: Vec<u8>,
        debug_info: Option<DecodedDebugInfo>,
    ) -> Result<Self, BytecodeError> {
        // Remove the INST header and check if the section size is correct.
        let (inst_sz, inst_bytecode) =
            read_and_remove_header(inst_bytecode, INSTRUCTION_MAGIC_NUM)?;

        if inst_bytecode.len() != inst_sz as usize {
            return Err(BytecodeError::IncorrectSectionSize);
        }

        let decoded_instructions =
            InstructionDecoder::new(&symbol_table, &inst_bytecode).decode()?;

        Ok(DecodedBindRules {
            symbol_table: symbol_table,
            instructions: inst_bytecode,
            decoded_instructions: decoded_instructions,
            debug_info: debug_info,
        })
    }

    pub fn from_bytecode(bytecode: Vec<u8>) -> Result<Self, BytecodeError> {
        let (symbol_table, inst_bytecode, debug_bytecode) =
            get_symbol_table_and_instruction_debug_bytecode(bytecode)?;
        DecodedBindRules::new(symbol_table, inst_bytecode, decode_debug_bytecode(debug_bytecode)?)
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct Parent {
    // Symbol table ID for the node name.
    pub name_id: u32,
    pub instructions: Vec<u8>,
    pub decoded_instructions: Vec<DecodedInstruction>,
}

// This struct decodes and unwraps the given bytecode into a symbol table
// and list of instructions.
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct DecodedCompositeBindRules {
    pub symbol_table: HashMap<u32, String>,
    pub device_name_id: u32,
    pub primary_parent: Parent,
    pub additional_parents: Vec<Parent>,
    pub optional_parents: Vec<Parent>,
    pub debug_info: Option<DecodedDebugInfo>,
}

impl DecodedCompositeBindRules {
    pub fn new(
        symbol_table: HashMap<u32, String>,
        composite_inst_bytecode: Vec<u8>,
        debug_info: Option<DecodedDebugInfo>,
    ) -> Result<Self, BytecodeError> {
        // Separate the instruction bytecode out of the symbol table bytecode and verify
        // the magic number and length. Remove the composite instruction header.
        let (composite_inst_sz, mut composite_inst_bytecode) =
            read_and_remove_header(composite_inst_bytecode, COMPOSITE_MAGIC_NUM)?;
        if composite_inst_bytecode.len() != composite_inst_sz as usize {
            return Err(BytecodeError::IncorrectSectionSize);
        }

        // Retrieve the device name ID and check if it's in the symbol table.
        let device_name_id = u32::from_le_bytes(get_u32_bytes(&composite_inst_bytecode, 0)?);
        if !symbol_table.contains_key(&device_name_id) {
            return Err(BytecodeError::MissingDeviceNameInSymbolTable);
        }

        // Split off the device name ID.
        let node_bytecode = composite_inst_bytecode.split_off(4);

        // Extract the primary node instructions.
        let (_, primary_parent, mut node_bytecode) =
            split_off_parent(node_bytecode, true, &symbol_table)?;

        // Extract additional and optional nodes from the remaining bytecode until there's none left.
        let mut additional_parents: Vec<Parent> = vec![];
        let mut optional_parents: Vec<Parent> = vec![];
        while !node_bytecode.is_empty() {
            let (is_optional, parent, remaining) =
                split_off_parent(node_bytecode, false, &symbol_table)?;
            node_bytecode = remaining;
            if is_optional {
                optional_parents.push(parent);
            } else {
                additional_parents.push(parent);
            }
        }

        Ok(DecodedCompositeBindRules {
            symbol_table: symbol_table,
            device_name_id: device_name_id,
            primary_parent: primary_parent,
            additional_parents: additional_parents,
            optional_parents: optional_parents,
            debug_info: debug_info,
        })
    }

    pub fn from_bytecode(bytecode: Vec<u8>) -> Result<Self, BytecodeError> {
        let (symbol_table, inst_bytecode, debug_bytecode) =
            get_symbol_table_and_instruction_debug_bytecode(bytecode)?;
        DecodedCompositeBindRules::new(
            symbol_table,
            inst_bytecode,
            decode_debug_bytecode(debug_bytecode)?,
        )
    }
}

fn get_u32_bytes(bytecode: &Vec<u8>, idx: usize) -> Result<[u8; 4], BytecodeError> {
    if idx + 4 > bytecode.len() {
        return Err(BytecodeError::UnexpectedEnd);
    }

    let mut bytes: [u8; 4] = [0; 4];
    bytes.copy_from_slice(&bytecode[idx..(4 + idx)]);
    Ok(bytes)
}

// Verify the header magic number. Return the value after the magic number and the following bytes.
fn read_and_remove_header(
    mut bytecode: Vec<u8>,
    magic_num: u32,
) -> Result<(u32, Vec<u8>), BytecodeError> {
    let parsed_magic_num = u32::from_be_bytes(get_u32_bytes(&bytecode, 0)?);
    if parsed_magic_num != magic_num {
        return Err(BytecodeError::InvalidHeader(magic_num, parsed_magic_num));
    }

    let val = u32::from_le_bytes(get_u32_bytes(&bytecode, 4)?);
    Ok((val, bytecode.split_off(HEADER_SZ)))
}

// Verify the enable_debug flag byte.
fn read_and_remove_debug_flag(mut bytecode: Vec<u8>) -> Result<(u8, Vec<u8>), BytecodeError> {
    let debug_flag_byte = bytecode[0];
    if debug_flag_byte != BYTECODE_DISABLE_DEBUG && debug_flag_byte != BYTECODE_ENABLE_DEBUG {
        return Err(BytecodeError::InvalidDebugFlag(debug_flag_byte));
    }
    Ok((debug_flag_byte, bytecode.split_off(DEBUG_FLAG_SZ)))
}

// Verify the node type and return the node ID and the number of bytes in the node instructions.
fn verify_and_read_node_header(
    bytecode: &Vec<u8>,
    expect_primary: bool,
) -> Result<(bool, u32, u32), BytecodeError> {
    if bytecode.len() < NODE_TYPE_HEADER_SZ {
        return Err(BytecodeError::UnexpectedEnd);
    }

    let is_optional = match FromPrimitive::from_u8(bytecode[0]) {
        Some(RawParentType::Primary) => {
            if !expect_primary {
                return Err(BytecodeError::MultiplePrimaryParents);
            }

            false
        }
        Some(RawParentType::Additional) => {
            if expect_primary {
                return Err(BytecodeError::InvalidPrimaryParent);
            }

            false
        }
        Some(RawParentType::Optional) => {
            if expect_primary {
                return Err(BytecodeError::InvalidPrimaryParent);
            }

            true
        }
        None => {
            return Err(BytecodeError::InvalidParentType(bytecode[0]));
        }
    };

    let node_id = u32::from_le_bytes(get_u32_bytes(bytecode, 1)?);
    let inst_sz = u32::from_le_bytes(get_u32_bytes(bytecode, 5)?);
    Ok((is_optional, node_id, inst_sz))
}

fn read_string(iter: &mut BytecodeIter) -> Result<String, BytecodeError> {
    let mut str_bytes = vec![];

    // Read in the bytes for the string until a zero terminator is reached.
    // If the number of bytes exceed the maximum string length, return an error.
    loop {
        let byte = *next_u8(iter)?;
        if byte == 0 {
            break;
        }

        str_bytes.push(byte);
        if str_bytes.len() > MAX_STRING_LENGTH {
            return Err(BytecodeError::InvalidStringLength);
        }
    }

    if str_bytes.is_empty() {
        return Err(BytecodeError::EmptyString);
    }

    String::from_utf8(str_bytes).map_err(|_| BytecodeError::Utf8ConversionFailure)
}

fn read_symbol_table(bytecode: Vec<u8>) -> Result<HashMap<u32, String>, BytecodeError> {
    let mut iter = bytecode.iter();

    let mut symbol_table = HashMap::new();
    let mut expected_key = SYMB_TBL_START_KEY;
    while let Some(key) = try_next_u32(&mut iter)? {
        if key != expected_key {
            return Err(BytecodeError::InvalidSymbolTableKey(key));
        }

        // Read the string and increase the byte count by the string length and the
        // zero terminator.
        let str_val = read_string(&mut iter)?;
        symbol_table.insert(key, str_val);
        expected_key += 1;
    }

    Ok(symbol_table)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::compiler::Symbol;
    use crate::interpreter::test_common::*;
    use crate::parser::bind_library;

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
    fn test_invalid_header() {
        // Test invalid magic number.
        let mut bytecode: Vec<u8> = vec![0x41, 0x49, 0x4E, 0x44, 0x02, 0, 0, 0];
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);
        assert_eq!(
            Err(BytecodeError::InvalidHeader(BIND_MAGIC_NUM, 0x41494E44)),
            DecodedRules::new(bytecode)
        );

        // Test invalid version.
        let mut bytecode: Vec<u8> = vec![0x42, 0x49, 0x4E, 0x44, 0x03, 0, 0, 0];
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);
        assert_eq!(Err(BytecodeError::InvalidVersion(3)), DecodedRules::new(bytecode));

        // Test invalid symbol table header.
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, 0xAAAAAAAA, 0);
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);
        assert_eq!(
            Err(BytecodeError::InvalidHeader(SYMB_MAGIC_NUM, 0xAAAAAAAA)),
            DecodedRules::new(bytecode)
        );

        // Test invalid instruction header.
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);
        append_section_header(&mut bytecode, 0xAAAAAAAA, 0);
        assert_eq!(
            Err(BytecodeError::InvalidHeader(INSTRUCTION_MAGIC_NUM, 0xAAAAAAAA)),
            DecodedRules::new(bytecode)
        );

        // Test invalid debug information section header.
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_ENABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);
        append_section_header(&mut bytecode, 0x44454241, 0);
        assert_eq!(
            Err(BytecodeError::InvalidHeader(DEBG_MAGIC_NUM, 0x44454241)),
            DecodedRules::new(bytecode)
        );

        // Test invalid debug symbol section header.
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_ENABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);
        append_section_header(&mut bytecode, DEBG_MAGIC_NUM, 8);
        append_section_header(&mut bytecode, 0x44454247, 0);
        assert_eq!(
            Err(BytecodeError::InvalidHeader(DBSY_MAGIC_NUM, 0x44454247)),
            DecodedRules::new(bytecode)
        );
    }

    #[test]
    fn test_long_string() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);

        let mut long_str: [u8; 275] = [0x41; 275];
        long_str[274] = 0;

        let symbol_section_sz = long_str.len() + 4;
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, symbol_section_sz as u32);

        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&long_str);

        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);
        assert_eq!(Err(BytecodeError::InvalidStringLength), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_unexpected_end() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        bytecode.extend_from_slice(&SYMB_MAGIC_NUM.to_be_bytes());
        assert_eq!(Err(BytecodeError::UnexpectedEnd), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_string_with_no_zero_terminator() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 14);

        let invalid_str: [u8; 10] = [0x41; 10];
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&invalid_str);

        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);

        assert_eq!(Err(BytecodeError::UnexpectedEnd), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_duplicate_key() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 14);

        let str_1: [u8; 3] = [0x41, 0x42, 0];
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&str_1);

        let str_2: [u8; 3] = [0x42, 0x43, 0];
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&str_2);

        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);
        assert_eq!(Err(BytecodeError::InvalidSymbolTableKey(1)), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_invalid_key() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 15);

        let str_1: [u8; 4] = [0x41, 0x45, 0x60, 0];
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&str_1);

        let str_2: [u8; 3] = [0x42, 0x43, 0];
        bytecode.extend_from_slice(&[5, 0, 0, 0]);
        bytecode.extend_from_slice(&str_2);

        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);
        assert_eq!(Err(BytecodeError::InvalidSymbolTableKey(5)), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_cutoff_symbol_table_key() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 9);

        let str_1: [u8; 3] = [0x41, 0x42, 0];
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&str_1);

        bytecode.extend_from_slice(&[2, 0]);
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);

        assert_eq!(Err(BytecodeError::UnexpectedEnd), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_incorrect_inst_size() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);
        bytecode.push(0x30);
        assert_eq!(Err(BytecodeError::IncorrectSectionSize), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_minimum_size_bytecode() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);
        assert_eq!(
            DecodedRules::Normal(DecodedBindRules {
                symbol_table: HashMap::new(),
                instructions: vec![],
                decoded_instructions: vec![],
                debug_info: None,
            }),
            DecodedRules::new(bytecode).unwrap()
        );
    }

    #[test]
    fn test_incorrect_size_symbol_table() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, u32::MAX);
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 0);
        assert_eq!(Err(BytecodeError::IncorrectSectionSize), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_instructions() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x30, 0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0, 0x10];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);
        let bind_rules = DecodedBindRules::from_bytecode(bytecode).unwrap();
        assert_eq!(instructions.to_vec(), bind_rules.instructions);
    }

    #[test]
    fn test_enable_debug_flag_empty_debug_info_section() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_ENABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x30];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);
        append_section_header(&mut bytecode, DEBG_MAGIC_NUM, 0);

        let rules = DecodedBindRules {
            symbol_table: HashMap::new(),
            instructions: vec![0x30],
            decoded_instructions: vec![DecodedInstruction::UnconditionalAbort],
            debug_info: None,
        };

        assert_eq!(DecodedRules::Normal(rules), DecodedRules::new(bytecode).unwrap());
    }

    #[test]
    fn test_enable_debug_flag_empty_debug_symb_section() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_ENABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x30];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);
        append_section_header(&mut bytecode, DEBG_MAGIC_NUM, 8);
        append_section_header(&mut bytecode, DBSY_MAGIC_NUM, 0);

        let rules = DecodedBindRules {
            symbol_table: HashMap::new(),
            instructions: vec![0x30],
            decoded_instructions: vec![DecodedInstruction::UnconditionalAbort],
            debug_info: Some(DecodedDebugInfo { symbol_table: HashMap::new() }),
        };

        assert_eq!(DecodedRules::Normal(rules), DecodedRules::new(bytecode).unwrap());
    }

    #[test]
    fn test_enable_debug_flag() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_ENABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x30];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);

        append_section_header(&mut bytecode, DEBG_MAGIC_NUM, 0x22);
        append_section_header(&mut bytecode, DBSY_MAGIC_NUM, 0x1A);

        let str_1: [u8; 22] = [
            // "fuchsia.BIND_PROTOCOL"
            0x66, 0x75, 0x63, 0x68, 0x73, 0x69, 0x61, 0x2e, 0x42, 0x49, 0x4e, 0x44, 0x5f, 0x50,
            0x52, 0x4f, 0x54, 0x4f, 0x43, 0x4f, 0x4c, 0,
        ];
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&str_1);

        let mut expected_symbol_table: HashMap<u32, String> = HashMap::new();
        expected_symbol_table.insert(1, "fuchsia.BIND_PROTOCOL".to_string());

        let rules = DecodedBindRules {
            symbol_table: HashMap::new(),
            instructions: vec![0x30],
            decoded_instructions: vec![DecodedInstruction::UnconditionalAbort],
            debug_info: Some(DecodedDebugInfo { symbol_table: expected_symbol_table }),
        };

        assert_eq!(DecodedRules::Normal(rules), DecodedRules::new(bytecode).unwrap());
    }

    #[test]
    fn test_missing_debug_flag() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x01, 0x01, 0x05, 0, 0, 0, 0x01, 0x16, 0, 0, 0];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);

        assert_eq!(Err(BytecodeError::InvalidDebugFlag(0x53)), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_invalid_debug_flag() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(0x03);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x01, 0x01, 0x05, 0, 0, 0, 0x01, 0x16, 0, 0, 0];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);

        assert_eq!(Err(BytecodeError::InvalidDebugFlag(0x03)), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_value_key_missing_in_symbols() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x01, 0x00, 0, 0, 0, 0x05, 0x10, 0, 0, 0, 0];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);

        assert_eq!(
            Err(BytecodeError::MissingEntryInSymbolTable(0x05000000)),
            DecodedRules::new(bytecode)
        );
    }

    #[test]
    fn test_valid_bytecode() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 18);

        let str_1: [u8; 5] = [0x57, 0x52, 0x45, 0x4E, 0]; // "WREN"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&str_1);

        let str_2: [u8; 5] = [0x44, 0x55, 0x43, 0x4B, 0]; // "DUCK"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&str_2);

        let instructions = [
            0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0, 0x10, // 0x05000000 == 0x10000010
            0x11, 0x01, 0, 0, 0, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0,
            0, // jmp 1 if 0x05000000 == 0x10
            0x30, 0x20, // abort, jump pad
            0x11, 0x01, 0, 0, 0, 0x00, 0x01, 0, 0, 0, 0x02, 0x02, 0, 0,
            0, // jmp 1 if key("WREN") == "DUCK"
            0x30, 0x20, // abort, jump pad
            0x10, 0x02, 0, 0, 0, 0x30, 0x30, 0x20, // jump 2, 2 aborts, jump pad
        ];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);

        let mut expected_symbol_table: HashMap<u32, String> = HashMap::new();
        expected_symbol_table.insert(1, "WREN".to_string());
        expected_symbol_table.insert(2, "DUCK".to_string());

        let expected_decoded_inst = vec![
            DecodedInstruction::Condition(DecodedCondition {
                is_equal: true,
                lhs: Symbol::NumberValue(0x05000000),
                rhs: Symbol::NumberValue(0x10000010),
            }),
            DecodedInstruction::Jump(Some(DecodedCondition {
                is_equal: true,
                lhs: Symbol::NumberValue(0x05000000),
                rhs: Symbol::NumberValue(0x10),
            })),
            DecodedInstruction::UnconditionalAbort,
            DecodedInstruction::Label,
            DecodedInstruction::Jump(Some(DecodedCondition {
                is_equal: true,
                lhs: Symbol::Key("WREN".to_string(), bind_library::ValueType::Str),
                rhs: Symbol::StringValue("DUCK".to_string()),
            })),
            DecodedInstruction::UnconditionalAbort,
            DecodedInstruction::Label,
            DecodedInstruction::Jump(None),
            DecodedInstruction::UnconditionalAbort,
            DecodedInstruction::UnconditionalAbort,
            DecodedInstruction::Label,
        ];

        let rules = DecodedBindRules {
            symbol_table: expected_symbol_table,
            instructions: instructions.to_vec(),
            decoded_instructions: expected_decoded_inst,
            debug_info: None,
        };
        assert_eq!(DecodedRules::Normal(rules), DecodedRules::new(bytecode).unwrap());
    }

    #[test]
    fn test_enable_debug_composite() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_ENABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 38);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        let node_name_2: [u8; 7] = [0x50, 0x4C, 0x4F, 0x56, 0x45, 0x52, 0]; // "PLOVER"
        bytecode.extend_from_slice(&[4, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_2);

        let primary_node_inst = [0x30, 0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];
        let additional_node_inst_1 = [0x02, 0x01, 0, 0, 0, 0x02, 0x01, 0, 0, 0, 0x10];
        let additional_node_inst_2 = [0x30, 0x30];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 3)
                + primary_node_inst.len()
                + additional_node_inst_1.len()
                + additional_node_inst_2.len()) as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add the node instructions.
        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst_1.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst_1);
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            4,
            additional_node_inst_2.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst_2);

        let mut expected_symbol_table: HashMap<u32, String> = HashMap::new();
        expected_symbol_table.insert(1, "IBIS".to_string());
        expected_symbol_table.insert(2, "RAIL".to_string());
        expected_symbol_table.insert(3, "COOT".to_string());
        expected_symbol_table.insert(4, "PLOVER".to_string());

        append_section_header(&mut bytecode, DEBG_MAGIC_NUM, 0x22);
        append_section_header(&mut bytecode, DBSY_MAGIC_NUM, 0x1A);

        let str_1: [u8; 22] = [
            // "fuchsia.BIND_PROTOCOL"
            0x66, 0x75, 0x63, 0x68, 0x73, 0x69, 0x61, 0x2e, 0x42, 0x49, 0x4e, 0x44, 0x5f, 0x50,
            0x52, 0x4f, 0x54, 0x4f, 0x43, 0x4f, 0x4c, 0,
        ];
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&str_1);

        let mut debug_symbol_table: HashMap<u32, String> = HashMap::new();
        debug_symbol_table.insert(1, "fuchsia.BIND_PROTOCOL".to_string());

        let rules = DecodedCompositeBindRules {
            symbol_table: expected_symbol_table,
            device_name_id: 1,
            primary_parent: Parent {
                name_id: 2,
                instructions: primary_node_inst.to_vec(),
                decoded_instructions: vec![
                    DecodedInstruction::UnconditionalAbort,
                    DecodedInstruction::Condition(DecodedCondition {
                        is_equal: true,
                        lhs: Symbol::NumberValue(0x5000000),
                        rhs: Symbol::NumberValue(0x200010),
                    }),
                ],
            },
            additional_parents: vec![
                Parent {
                    name_id: 3,
                    instructions: additional_node_inst_1.to_vec(),
                    decoded_instructions: vec![DecodedInstruction::Condition(DecodedCondition {
                        is_equal: false,
                        lhs: Symbol::NumberValue(0x2000000),
                        rhs: Symbol::NumberValue(0x10000000),
                    })],
                },
                Parent {
                    name_id: 4,
                    instructions: additional_node_inst_2.to_vec(),
                    decoded_instructions: vec![
                        DecodedInstruction::UnconditionalAbort,
                        DecodedInstruction::UnconditionalAbort,
                    ],
                },
            ],
            optional_parents: vec![],
            debug_info: Some(DecodedDebugInfo { symbol_table: debug_symbol_table }),
        };
        assert_eq!(DecodedRules::Composite(rules), DecodedRules::new(bytecode).unwrap());
    }

    #[test]
    fn test_enable_debug_composite_with_optional() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_ENABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 38);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        let node_name_2: [u8; 7] = [0x50, 0x4C, 0x4F, 0x56, 0x45, 0x52, 0]; // "PLOVER"
        bytecode.extend_from_slice(&[4, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_2);

        let primary_node_inst = [0x30, 0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];
        let additional_node_inst_1 = [0x02, 0x01, 0, 0, 0, 0x02, 0x01, 0, 0, 0, 0x10];
        let optional_node_inst_1 = [0x30, 0x30];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 3)
                + primary_node_inst.len()
                + additional_node_inst_1.len()
                + optional_node_inst_1.len()) as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add the node instructions.
        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst_1.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst_1);
        append_node_header(
            &mut bytecode,
            RawParentType::Optional,
            4,
            optional_node_inst_1.len() as u32,
        );
        bytecode.extend_from_slice(&optional_node_inst_1);

        let mut expected_symbol_table: HashMap<u32, String> = HashMap::new();
        expected_symbol_table.insert(1, "IBIS".to_string());
        expected_symbol_table.insert(2, "RAIL".to_string());
        expected_symbol_table.insert(3, "COOT".to_string());
        expected_symbol_table.insert(4, "PLOVER".to_string());

        append_section_header(&mut bytecode, DEBG_MAGIC_NUM, 0x22);
        append_section_header(&mut bytecode, DBSY_MAGIC_NUM, 0x1A);

        let str_1: [u8; 22] = [
            // "fuchsia.BIND_PROTOCOL"
            0x66, 0x75, 0x63, 0x68, 0x73, 0x69, 0x61, 0x2e, 0x42, 0x49, 0x4e, 0x44, 0x5f, 0x50,
            0x52, 0x4f, 0x54, 0x4f, 0x43, 0x4f, 0x4c, 0,
        ];
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&str_1);

        let mut debug_symbol_table: HashMap<u32, String> = HashMap::new();
        debug_symbol_table.insert(1, "fuchsia.BIND_PROTOCOL".to_string());

        let rules = DecodedCompositeBindRules {
            symbol_table: expected_symbol_table,
            device_name_id: 1,
            primary_parent: Parent {
                name_id: 2,
                instructions: primary_node_inst.to_vec(),
                decoded_instructions: vec![
                    DecodedInstruction::UnconditionalAbort,
                    DecodedInstruction::Condition(DecodedCondition {
                        is_equal: true,
                        lhs: Symbol::NumberValue(0x5000000),
                        rhs: Symbol::NumberValue(0x200010),
                    }),
                ],
            },
            additional_parents: vec![Parent {
                name_id: 3,
                instructions: additional_node_inst_1.to_vec(),
                decoded_instructions: vec![DecodedInstruction::Condition(DecodedCondition {
                    is_equal: false,
                    lhs: Symbol::NumberValue(0x2000000),
                    rhs: Symbol::NumberValue(0x10000000),
                })],
            }],
            optional_parents: vec![Parent {
                name_id: 4,
                instructions: optional_node_inst_1.to_vec(),
                decoded_instructions: vec![
                    DecodedInstruction::UnconditionalAbort,
                    DecodedInstruction::UnconditionalAbort,
                ],
            }],
            debug_info: Some(DecodedDebugInfo { symbol_table: debug_symbol_table }),
        };
        assert_eq!(DecodedRules::Composite(rules), DecodedRules::new(bytecode).unwrap());
    }

    #[test]
    fn test_valid_composite_bind() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 38);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        let node_name_2: [u8; 7] = [0x50, 0x4C, 0x4F, 0x56, 0x45, 0x52, 0]; // "PLOVER"
        bytecode.extend_from_slice(&[4, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_2);

        let primary_node_inst = [0x30, 0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];
        let additional_node_inst_1 = [0x02, 0x01, 0, 0, 0, 0x02, 0x01, 0, 0, 0, 0x10];
        let additional_node_inst_2 = [0x30, 0x30];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 3)
                + primary_node_inst.len()
                + additional_node_inst_1.len()
                + additional_node_inst_2.len()) as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add the node instructions.
        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst_1.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst_1);
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            4,
            additional_node_inst_2.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst_2);

        let mut expected_symbol_table: HashMap<u32, String> = HashMap::new();
        expected_symbol_table.insert(1, "IBIS".to_string());
        expected_symbol_table.insert(2, "RAIL".to_string());
        expected_symbol_table.insert(3, "COOT".to_string());
        expected_symbol_table.insert(4, "PLOVER".to_string());

        let rules = DecodedCompositeBindRules {
            symbol_table: expected_symbol_table,
            device_name_id: 1,
            primary_parent: Parent {
                name_id: 2,
                instructions: primary_node_inst.to_vec(),
                decoded_instructions: vec![
                    DecodedInstruction::UnconditionalAbort,
                    DecodedInstruction::Condition(DecodedCondition {
                        is_equal: true,
                        lhs: Symbol::NumberValue(0x5000000),
                        rhs: Symbol::NumberValue(0x200010),
                    }),
                ],
            },
            additional_parents: vec![
                Parent {
                    name_id: 3,
                    instructions: additional_node_inst_1.to_vec(),
                    decoded_instructions: vec![DecodedInstruction::Condition(DecodedCondition {
                        is_equal: false,
                        lhs: Symbol::NumberValue(0x2000000),
                        rhs: Symbol::NumberValue(0x10000000),
                    })],
                },
                Parent {
                    name_id: 4,
                    instructions: additional_node_inst_2.to_vec(),
                    decoded_instructions: vec![
                        DecodedInstruction::UnconditionalAbort,
                        DecodedInstruction::UnconditionalAbort,
                    ],
                },
            ],
            optional_parents: vec![],
            debug_info: None,
        };
        assert_eq!(DecodedRules::Composite(rules), DecodedRules::new(bytecode).unwrap());
    }

    #[test]
    fn test_valid_composite_bind_with_optional() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 38);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let node_name_1: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "COOT"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_1);

        let node_name_2: [u8; 7] = [0x50, 0x4C, 0x4F, 0x56, 0x45, 0x52, 0]; // "PLOVER"
        bytecode.extend_from_slice(&[4, 0, 0, 0]);
        bytecode.extend_from_slice(&node_name_2);

        let primary_node_inst = [0x30, 0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];
        let additional_node_inst_1 = [0x02, 0x01, 0, 0, 0, 0x02, 0x01, 0, 0, 0, 0x10];
        let optional_node_inst_1 = [0x30, 0x30];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 3)
                + primary_node_inst.len()
                + additional_node_inst_1.len()
                + optional_node_inst_1.len()) as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add the node instructions.
        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst_1.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst_1);
        append_node_header(
            &mut bytecode,
            RawParentType::Optional,
            4,
            optional_node_inst_1.len() as u32,
        );
        bytecode.extend_from_slice(&optional_node_inst_1);

        let mut expected_symbol_table: HashMap<u32, String> = HashMap::new();
        expected_symbol_table.insert(1, "IBIS".to_string());
        expected_symbol_table.insert(2, "RAIL".to_string());
        expected_symbol_table.insert(3, "COOT".to_string());
        expected_symbol_table.insert(4, "PLOVER".to_string());

        let rules = DecodedCompositeBindRules {
            symbol_table: expected_symbol_table,
            device_name_id: 1,
            primary_parent: Parent {
                name_id: 2,
                instructions: primary_node_inst.to_vec(),
                decoded_instructions: vec![
                    DecodedInstruction::UnconditionalAbort,
                    DecodedInstruction::Condition(DecodedCondition {
                        is_equal: true,
                        lhs: Symbol::NumberValue(0x5000000),
                        rhs: Symbol::NumberValue(0x200010),
                    }),
                ],
            },
            additional_parents: vec![Parent {
                name_id: 3,
                instructions: additional_node_inst_1.to_vec(),
                decoded_instructions: vec![DecodedInstruction::Condition(DecodedCondition {
                    is_equal: false,
                    lhs: Symbol::NumberValue(0x2000000),
                    rhs: Symbol::NumberValue(0x10000000),
                })],
            }],
            optional_parents: vec![Parent {
                name_id: 4,
                instructions: optional_node_inst_1.to_vec(),
                decoded_instructions: vec![
                    DecodedInstruction::UnconditionalAbort,
                    DecodedInstruction::UnconditionalAbort,
                ],
            }],
            debug_info: None,
        };
        assert_eq!(DecodedRules::Composite(rules), DecodedRules::new(bytecode).unwrap());
    }

    #[test]
    fn test_primary_node_only() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 18);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let primary_node_inst = [0x30, 0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        let composite_insts_sz =
            COMPOSITE_NAME_ID_BYTES + (NODE_TYPE_HEADER_SZ + primary_node_inst.len()) as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add the node instructions.
        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        let mut expected_symbol_table: HashMap<u32, String> = HashMap::new();
        expected_symbol_table.insert(1, "IBIS".to_string());
        expected_symbol_table.insert(2, "COOT".to_string());

        assert_eq!(
            DecodedRules::Composite(DecodedCompositeBindRules {
                symbol_table: expected_symbol_table,
                device_name_id: 1,
                primary_parent: Parent {
                    name_id: 2,
                    instructions: primary_node_inst.to_vec(),
                    decoded_instructions: vec![
                        DecodedInstruction::UnconditionalAbort,
                        DecodedInstruction::Condition(DecodedCondition {
                            is_equal: true,
                            lhs: Symbol::NumberValue(0x5000000),
                            rhs: Symbol::NumberValue(0x200010),
                        }),
                    ],
                },
                additional_parents: vec![],
                optional_parents: vec![],
                debug_info: None,
            }),
            DecodedRules::new(bytecode).unwrap()
        );
    }

    #[test]
    fn test_missing_device_name() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 9);

        let primary_node_name: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "RAIL"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let primary_node_inst = [0x30];
        let composite_insts_sz =
            COMPOSITE_NAME_ID_BYTES + (NODE_TYPE_HEADER_SZ + primary_node_inst.len()) as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);
        bytecode.extend_from_slice(&[2, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            1,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        assert_eq!(Err(BytecodeError::MissingDeviceNameInSymbolTable), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_missing_node_name() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 9);

        let primary_node_name: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "RAIL"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let primary_node_inst = [0x30];
        let composite_insts_sz =
            COMPOSITE_NAME_ID_BYTES + (NODE_TYPE_HEADER_SZ + primary_node_inst.len()) as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        assert_eq!(Err(BytecodeError::MissingParentIdInSymbolTable), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_missing_primary_node() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 29);

        let device_name: [u8; 5] = [0x4C, 0x4F, 0x4F, 0x4E, 0]; // "LOON"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 6] = [0x47, 0x52, 0x45, 0x42, 0x45, 0]; // "GREBE"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let additional_node_name: [u8; 6] = [0x53, 0x43, 0x41, 0x55, 0x50, 0]; // "SCAUP"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&additional_node_name);

        let additional_node_inst_1 = [0x02, 0x01, 0, 0, 0, 0x02, 0x02, 0, 0, 0x10];
        let additional_node_inst_2 = [0x30];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2)
                + additional_node_inst_1.len()
                + additional_node_inst_2.len()) as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add the node instructions.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            2,
            additional_node_inst_1.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst_1);
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst_2.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst_2);

        assert_eq!(Err(BytecodeError::InvalidPrimaryParent), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_primary_node_incorrect_order() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 29);

        let device_name: [u8; 5] = [0x4C, 0x4F, 0x4F, 0x4E, 0]; // "LOON"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 6] = [0x47, 0x52, 0x45, 0x42, 0x45, 0]; // "GREBE"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let additional_node_name: [u8; 6] = [0x53, 0x43, 0x41, 0x55, 0x50, 0]; // "SCAUP"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&additional_node_name);

        let primary_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x02, 0, 0, 0x10];
        let additional_node_inst = [0x30];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add the node instructions.
        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);
        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        assert_eq!(Err(BytecodeError::InvalidPrimaryParent), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_multiple_primary_nodes() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 29);

        let device_name: [u8; 5] = [0x4C, 0x4F, 0x4F, 0x4E, 0]; // "LOON"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 6] = [0x47, 0x52, 0x45, 0x42, 0x45, 0]; // "GREBE"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let primary_node_name_2: [u8; 6] = [0x53, 0x43, 0x41, 0x55, 0x50, 0]; // "SCAUP"
        bytecode.extend_from_slice(&[3, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name_2);

        let primary_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x01, 0, 0, 0, 0x10];
        let primary_node_inst_2 = [0x30];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + primary_node_inst_2.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add the node instructions.
        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);
        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            3,
            primary_node_inst_2.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst_2);

        assert_eq!(Err(BytecodeError::MultiplePrimaryParents), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_invalid_node_type() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 18);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let primary_node_inst = [0x30];

        let composite_insts_sz =
            COMPOSITE_NAME_ID_BYTES + (NODE_TYPE_HEADER_SZ + primary_node_inst.len()) as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add the node instructions with an invalid node type.
        bytecode.push(0x53);
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&(primary_node_inst.len() as u32).to_le_bytes());
        bytecode.extend_from_slice(&primary_node_inst);

        assert_eq!(Err(BytecodeError::InvalidParentType(0x53)), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_incorrect_node_section_sz_overlap() {
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

        let primary_node_inst = [0x30];
        let additional_node_inst = [0x02, 0x01, 0, 0, 0, 0x02, 0x02, 0, 0, 0x10];

        let composite_insts_sz = COMPOSITE_NAME_ID_BYTES
            + ((NODE_TYPE_HEADER_SZ * 2) + primary_node_inst.len() + additional_node_inst.len())
                as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add the primary node instructions with a size that overlaps to the next
        // node.
        append_node_header(&mut bytecode, RawParentType::Primary, 2, 7);
        bytecode.extend_from_slice(&primary_node_inst);

        append_node_header(
            &mut bytecode,
            RawParentType::Additional,
            3,
            additional_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&additional_node_inst);

        // Instructions for the primary node end, the next byte is the node type for the next node.
        assert_eq!(
            Err(BytecodeError::InvalidOp(RawParentType::Additional as u8)),
            DecodedRules::new(bytecode)
        );
    }

    #[test]
    fn test_incorrect_node_section_sz_undersize() {
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

        let primary_node_inst = [0x30];
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

        // Add the additional node with a size that's too small.
        append_node_header(&mut bytecode, RawParentType::Additional, 3, 1);
        bytecode.extend_from_slice(&additional_node_inst);

        // Reach end when trying to read in value type after the inequality operator (0x02).
        assert_eq!(Err(BytecodeError::UnexpectedEnd), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_incorrect_node_section_sz_oversize() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 18);

        let device_name: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&device_name);

        let primary_node_name: [u8; 5] = [0x43, 0x4F, 0x4F, 0x54, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let primary_node_inst = [0x30, 0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        let composite_insts_sz =
            COMPOSITE_NAME_ID_BYTES + (NODE_TYPE_HEADER_SZ + primary_node_inst.len()) as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add primary node instructions with a size that exceed the entire instruction size.
        append_node_header(&mut bytecode, RawParentType::Primary, 2, 50);
        bytecode.extend_from_slice(&primary_node_inst);

        assert_eq!(Err(BytecodeError::IncorrectParentSectionSize), DecodedRules::new(bytecode));
    }

    #[test]
    fn test_composite_header_in_noncomposite_bytecode() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 18);

        let str_1: [u8; 5] = [0x49, 0x42, 0x49, 0x53, 0]; // "IBIS"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&str_1);

        let primary_node_name: [u8; 5] = [0x52, 0x41, 0x49, 0x4C, 0]; // "RAIL"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&primary_node_name);

        let primary_node_inst = [0x30, 0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0x20, 0];

        let composite_insts_sz =
            COMPOSITE_NAME_ID_BYTES + (NODE_TYPE_HEADER_SZ + primary_node_inst.len()) as u32;
        append_section_header(&mut bytecode, COMPOSITE_MAGIC_NUM, composite_insts_sz);

        // Add device name ID.
        bytecode.extend_from_slice(&[1, 0, 0, 0]);

        // Add the node instructions.
        append_node_header(
            &mut bytecode,
            RawParentType::Primary,
            2,
            primary_node_inst.len() as u32,
        );
        bytecode.extend_from_slice(&primary_node_inst);

        assert_eq!(
            Err(BytecodeError::InvalidHeader(INSTRUCTION_MAGIC_NUM, COMPOSITE_MAGIC_NUM)),
            DecodedBindRules::from_bytecode(bytecode)
        );
    }

    #[test]
    fn test_inst_header_in_composite_bytecode() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 0);

        let instructions = [0x30, 0x01, 0x01, 0, 0, 0, 0x05, 0x10, 0, 0, 0x10];
        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, instructions.len() as u32);
        bytecode.extend_from_slice(&instructions);

        assert_eq!(
            Err(BytecodeError::InvalidHeader(COMPOSITE_MAGIC_NUM, INSTRUCTION_MAGIC_NUM)),
            DecodedCompositeBindRules::from_bytecode(bytecode)
        );
    }

    #[test]
    fn test_composite_with_compiler() {
        use crate::compiler::{
            CompiledBindRules, CompositeBindRules, CompositeParent, Symbol, SymbolicInstruction,
            SymbolicInstructionInfo,
        };
        use crate::parser::bind_library::ValueType;

        let primary_node_inst = vec![SymbolicInstructionInfo {
            location: None,
            instruction: SymbolicInstruction::UnconditionalAbort,
        }];

        let additional_node_inst = vec![
            SymbolicInstructionInfo {
                location: None,
                instruction: SymbolicInstruction::AbortIfEqual {
                    lhs: Symbol::Key("bobolink".to_string(), ValueType::Bool),
                    rhs: Symbol::BoolValue(false),
                },
            },
            SymbolicInstructionInfo {
                location: None,
                instruction: SymbolicInstruction::AbortIfNotEqual {
                    lhs: Symbol::Key("grackle".to_string(), ValueType::Number),
                    rhs: Symbol::NumberValue(1),
                },
            },
        ];

        let bytecode = CompiledBindRules::CompositeBind(CompositeBindRules {
            device_name: "blackbird".to_string(),
            symbol_table: HashMap::new(),
            primary_parent: CompositeParent {
                name: "meadowlark".to_string(),
                instructions: primary_node_inst,
            },
            additional_parents: vec![CompositeParent {
                name: "cowbird".to_string(),
                instructions: additional_node_inst,
            }],
            optional_parents: vec![],
            enable_debug: false,
        })
        .encode_to_bytecode()
        .unwrap();

        let mut expected_symbol_table: HashMap<u32, String> = HashMap::new();
        expected_symbol_table.insert(1, "blackbird".to_string());
        expected_symbol_table.insert(2, "meadowlark".to_string());
        expected_symbol_table.insert(3, "cowbird".to_string());
        expected_symbol_table.insert(4, "bobolink".to_string());
        expected_symbol_table.insert(5, "grackle".to_string());

        let primary_node_inst = [0x30];
        let additional_node_inst = [
            0x02, 0x0, 0x04, 0x0, 0x0, 0x0, 0x03, 0x0, 0x0, 0x0, 0x0, // bobolink != false
            0x01, 0x0, 0x05, 0x0, 0x0, 0x0, 0x01, 0x1, 0x0, 0x0, 0x0, // grackle == 1
        ];

        assert_eq!(
            DecodedRules::Composite(DecodedCompositeBindRules {
                symbol_table: expected_symbol_table,
                device_name_id: 1,
                primary_parent: Parent {
                    name_id: 2,
                    instructions: primary_node_inst.to_vec(),
                    decoded_instructions: vec![DecodedInstruction::UnconditionalAbort],
                },
                additional_parents: vec![Parent {
                    name_id: 3,
                    instructions: additional_node_inst.to_vec(),
                    decoded_instructions: vec![
                        DecodedInstruction::Condition(DecodedCondition {
                            is_equal: false,
                            lhs: Symbol::Key("bobolink".to_string(), ValueType::Str),
                            rhs: Symbol::BoolValue(false)
                        }),
                        DecodedInstruction::Condition(DecodedCondition {
                            is_equal: true,
                            lhs: Symbol::Key("grackle".to_string(), ValueType::Str),
                            rhs: Symbol::NumberValue(1)
                        })
                    ]
                }],
                optional_parents: vec![],
                debug_info: None,
            }),
            DecodedRules::new(bytecode).unwrap()
        );
    }

    #[test]
    fn test_composite_optional_with_compiler() {
        use crate::compiler::{
            CompiledBindRules, CompositeBindRules, CompositeParent, Symbol, SymbolicInstruction,
            SymbolicInstructionInfo,
        };
        use crate::parser::bind_library::ValueType;

        let primary_node_inst = vec![SymbolicInstructionInfo {
            location: None,
            instruction: SymbolicInstruction::UnconditionalAbort,
        }];

        let additional_node_inst = vec![
            SymbolicInstructionInfo {
                location: None,
                instruction: SymbolicInstruction::AbortIfEqual {
                    lhs: Symbol::Key("bobolink".to_string(), ValueType::Bool),
                    rhs: Symbol::BoolValue(false),
                },
            },
            SymbolicInstructionInfo {
                location: None,
                instruction: SymbolicInstruction::AbortIfNotEqual {
                    lhs: Symbol::Key("grackle".to_string(), ValueType::Number),
                    rhs: Symbol::NumberValue(1),
                },
            },
        ];

        let optional_node_inst = vec![SymbolicInstructionInfo {
            location: None,
            instruction: SymbolicInstruction::AbortIfEqual {
                lhs: Symbol::Key("mockingbird".to_string(), ValueType::Bool),
                rhs: Symbol::BoolValue(false),
            },
        }];

        let bytecode = CompiledBindRules::CompositeBind(CompositeBindRules {
            device_name: "blackbird".to_string(),
            symbol_table: HashMap::new(),
            primary_parent: CompositeParent {
                name: "meadowlark".to_string(),
                instructions: primary_node_inst,
            },
            additional_parents: vec![CompositeParent {
                name: "cowbird".to_string(),
                instructions: additional_node_inst,
            }],
            optional_parents: vec![CompositeParent {
                name: "cowbird_optional".to_string(),
                instructions: optional_node_inst,
            }],
            enable_debug: false,
        })
        .encode_to_bytecode()
        .unwrap();

        let mut expected_symbol_table: HashMap<u32, String> = HashMap::new();
        expected_symbol_table.insert(1, "blackbird".to_string());
        expected_symbol_table.insert(2, "meadowlark".to_string());
        expected_symbol_table.insert(3, "cowbird".to_string());
        expected_symbol_table.insert(4, "bobolink".to_string());
        expected_symbol_table.insert(5, "grackle".to_string());
        expected_symbol_table.insert(6, "cowbird_optional".to_string());
        expected_symbol_table.insert(7, "mockingbird".to_string());

        let primary_node_inst = [0x30];
        let additional_node_inst = [
            0x02, 0x0, 0x04, 0x0, 0x0, 0x0, 0x03, 0x0, 0x0, 0x0, 0x0, // bobolink != false
            0x01, 0x0, 0x05, 0x0, 0x0, 0x0, 0x01, 0x1, 0x0, 0x0, 0x0, // grackle == 1
        ];

        let optional_node_inst = [
            0x02, 0x0, 0x07, 0x0, 0x0, 0x0, 0x03, 0x0, 0x0, 0x0, 0x0, // mockingbird != false
        ];

        assert_eq!(
            DecodedRules::Composite(DecodedCompositeBindRules {
                symbol_table: expected_symbol_table,
                device_name_id: 1,
                primary_parent: Parent {
                    name_id: 2,
                    instructions: primary_node_inst.to_vec(),
                    decoded_instructions: vec![DecodedInstruction::UnconditionalAbort],
                },
                additional_parents: vec![Parent {
                    name_id: 3,
                    instructions: additional_node_inst.to_vec(),
                    decoded_instructions: vec![
                        DecodedInstruction::Condition(DecodedCondition {
                            is_equal: false,
                            lhs: Symbol::Key("bobolink".to_string(), ValueType::Str),
                            rhs: Symbol::BoolValue(false)
                        }),
                        DecodedInstruction::Condition(DecodedCondition {
                            is_equal: true,
                            lhs: Symbol::Key("grackle".to_string(), ValueType::Str),
                            rhs: Symbol::NumberValue(1)
                        })
                    ]
                }],
                optional_parents: vec![Parent {
                    name_id: 6,
                    instructions: optional_node_inst.to_vec(),
                    decoded_instructions: vec![DecodedInstruction::Condition(DecodedCondition {
                        is_equal: false,
                        lhs: Symbol::Key("mockingbird".to_string(), ValueType::Str),
                        rhs: Symbol::BoolValue(false)
                    })],
                }],
                debug_info: None,
            }),
            DecodedRules::new(bytecode).unwrap()
        );
    }
}
