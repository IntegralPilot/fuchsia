// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::compiler::symbol_table::{Symbol, get_deprecated_key_identifiers};
use crate::interpreter::common::BytecodeError;
use crate::interpreter::decode_bind_rules::{DecodedCompositeBindRules, DecodedRules, Parent};
use crate::interpreter::instruction_decoder::{DecodedCondition, DecodedInstruction};
use crate::parser::common::ParentType;

fn dump_node(
    node: &Parent,
    node_type: ParentType,
    decoded_rules: &DecodedCompositeBindRules,
) -> String {
    let node_name = decoded_rules
        .symbol_table
        .get(&node.name_id)
        .map_or_else(|| "N/A".to_string(), |name| name.clone());
    let mut node_dump = match node_type {
        ParentType::Primary => format!("Node (primary): {}", node_name),
        ParentType::Additional => format!("Node: {}", node_name),
        ParentType::Optional => format!("Node (optional): {}", node_name),
    };
    node_dump.push_str(&dump_instructions(node.decoded_instructions.clone()));
    node_dump.push_str("\n");
    node_dump
}

pub fn dump_bind_rules(bytecode: Vec<u8>) -> Result<String, BytecodeError> {
    match DecodedRules::new(bytecode)? {
        DecodedRules::Normal(decoded_rules) => {
            Ok(dump_instructions(decoded_rules.decoded_instructions))
        }
        DecodedRules::Composite(rules) => {
            let mut node_dump = dump_node(&rules.primary_parent, ParentType::Primary, &rules);

            if !rules.additional_parents.is_empty() {
                let additional_nodes_dump = rules
                    .additional_parents
                    .iter()
                    .map(|node| dump_node(node, ParentType::Additional, &rules))
                    .collect::<Vec<String>>()
                    .join("\n");
                node_dump.push_str(&additional_nodes_dump);
            }

            if !rules.optional_parents.is_empty() {
                let optional_nodes_dump = rules
                    .optional_parents
                    .iter()
                    .map(|node| dump_node(node, ParentType::Optional, &rules))
                    .collect::<Vec<String>>()
                    .join("\n");
                node_dump.push_str(&optional_nodes_dump);
            }
            Ok(node_dump)
        }
    }
}

fn dump_condition(cond: DecodedCondition) -> String {
    let op = if cond.is_equal { "==" } else { "!=" };
    let lhs_dump = match cond.lhs {
        Symbol::NumberValue(value) => {
            let deprecated_keys = get_deprecated_key_identifiers();
            match deprecated_keys.get(&(value as u32)) {
                Some(value) => value.clone(),
                None => value.to_string(),
            }
        }
        _ => cond.lhs.to_string(),
    };
    format!("{} {} {}", lhs_dump, op, cond.rhs)
}

// TODO(https://fxbug.dev/42175142): Print the label IDs in the jump and label statements.
fn dump_instructions(instructions: Vec<DecodedInstruction>) -> String {
    let mut bind_rules_dump = String::new();
    for inst in instructions {
        let inst_dump = match inst {
            DecodedInstruction::UnconditionalAbort => "  Abort".to_string(),
            DecodedInstruction::Condition(cond) => format!("  {}", dump_condition(cond)),
            DecodedInstruction::Jump(cond) => match cond {
                Some(condition) => format!("  Jump if {} to ??", dump_condition(condition)),
                None => "  Jump to ??".to_string(),
            },
            DecodedInstruction::Label => "  Label ??".to_string(),
        };

        bind_rules_dump.push_str("\n");
        bind_rules_dump.push_str(&inst_dump);
    }

    bind_rules_dump
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::bytecode_constants::*;

    const BIND_HEADER: [u8; 8] = [0x42, 0x49, 0x4E, 0x44, 0x02, 0, 0, 0];

    fn append_section_header(bytecode: &mut Vec<u8>, magic_num: u32, sz: u32) {
        bytecode.extend_from_slice(&magic_num.to_be_bytes());
        bytecode.extend_from_slice(&sz.to_le_bytes());
    }

    #[test]
    fn test_bytecode_print() {
        let mut bytecode: Vec<u8> = BIND_HEADER.to_vec();
        bytecode.push(BYTECODE_DISABLE_DEBUG);
        append_section_header(&mut bytecode, SYMB_MAGIC_NUM, 18);

        let str_1: [u8; 5] = [0x57, 0x52, 0x45, 0x4E, 0]; // "WREN"
        bytecode.extend_from_slice(&[1, 0, 0, 0]);
        bytecode.extend_from_slice(&str_1);

        let str_2: [u8; 5] = [0x44, 0x55, 0x43, 0x4B, 0]; // "DUCK"
        bytecode.extend_from_slice(&[2, 0, 0, 0]);
        bytecode.extend_from_slice(&str_2);

        append_section_header(&mut bytecode, INSTRUCTION_MAGIC_NUM, 28);

        let instructions = [
            0x01, 0x01, 0, 0, 0, 0x05, 0x01, 0x10, 0, 0, 0x10, // 0x05000000 == 0x10000010
            0x11, 0x01, 0, 0, 0, 0x00, 0x01, 0, 0, 0, 0x02, // jmp 1 if key("WREN") == "DUCK"
            0x02, 0, 0, 0,    // jmp 1 if key("WREN") == "DUCK"
            0x30, // abort
            0x20, // jump pad
        ];
        bytecode.extend_from_slice(&instructions);

        let expected_dump =
            "\n  83886080 == 268435472\n  Jump if Key(WREN) == \"DUCK\" to ??\n  Abort\n  Label ??";
        assert_eq!(expected_dump.to_string(), dump_bind_rules(bytecode).unwrap());
    }

    #[test]
    fn test_composite_bytecode_print() {
        let bytecode = vec![
            0x42, 0x49, 0x4e, 0x44, 0x02, 0x00, 0x00, 0x00, // BIND header
            0x00, // debug_flag byte
            0x53, 0x59, 0x4e, 0x42, 0x28, 0x00, 0x00, 0x00, // SYMB header
            0x01, 0x00, 0x00, 0x00, // "wallcreeper" ID
            0x77, 0x61, 0x6c, 0x6c, 0x63, 0x72, 0x65, 0x65, // "wallcree"
            0x70, 0x65, 0x72, 0x00, // "per"
            0x02, 0x00, 0x00, 0x00, // "wagtail" ID
            0x77, 0x61, 0x67, 0x74, 0x61, 0x69, 0x6c, 0x00, // "wagtail"
            0x03, 0x00, 0x00, 0x00, // "redpoll" ID
            0x72, 0x65, 0x64, 0x70, 0x6f, 0x6c, 0x6c, 0x00, // "redpoll"
            0x43, 0x4f, 0x4d, 0x50, 0x2d, 0x00, 0x00, 0x00, // COMP header
            0x01, 0x00, 0x00, 0x00, // Device name ID
            0x50, 0x02, 0x00, 0x00, 0x00, 0x0b, 0x00, 0x00, 0x00, // Primary node header
            0x01, 0x01, 0x01, 0x00, 0x00, 0x00, // BIND_PROTOCOL ==
            0x01, 0x01, 0x00, 0x00, 0x00, // 1
            0x51, 0x03, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, // Node header
            0x02, 0x01, 0x01, 0x00, 0x00, 0x00, // BIND_PROTOCOL !=
            0x01, 0x02, 0x00, 0x00, 0x00, // 2
            0x30, // abort
        ];

        let expected_dump = "Node (primary): wagtail\n  \
            fuchsia.BIND_PROTOCOL == 1\n\
            Node: redpoll\n  \
            fuchsia.BIND_PROTOCOL != 2\n  \
            Abort\n";
        assert_eq!(expected_dump.to_string(), dump_bind_rules(bytecode).unwrap());
    }
}
