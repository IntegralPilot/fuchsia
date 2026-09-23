// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::parser::ast::{Command, CommandTag, WordPart, WordPartTag};
use crate::relative;
use bstr::{BString, ByteVec};

/// Formats an AST `Command` node into a readable byte string (`BString`).
///
/// Used for command execution tracing (`set -x` / verbose mode) and error debugging.
/// Recursively formats pipelines (`|`), subshells (`(...)`), background jobs (`&`), and
/// individual command word parts (literals, variables, command substitutions, arithmetic).
pub fn command_to_bstring(command: &Command, buffer: &relative::Buffer) -> BString {
    let mut formatted = BString::default();
    match command.tag {
        CommandTag::SIMPLE => {
            let arguments = command.simple_args.as_slice(buffer);
            for (i, argument) in arguments.iter().enumerate() {
                if i > 0 {
                    formatted.push_byte(b' ');
                }
                formatted
                    .extend_from_slice(&argument_to_bstring(argument.as_slice(buffer), buffer));
            }
        }
        CommandTag::PIPELINE => {
            formatted.extend_from_slice(&command_to_bstring(command.left.as_ref(buffer), buffer));
            formatted.push_str(" | ");
            formatted.extend_from_slice(&command_to_bstring(command.right.as_ref(buffer), buffer));
        }
        CommandTag::REDIRECT => {
            let inner = command.left.as_ref(buffer);
            formatted.extend_from_slice(&command_to_bstring(inner, buffer));
            if inner.tag == CommandTag::SIMPLE {
                formatted.push_str(" <redirection>");
            }
        }
        CommandTag::SUBSHELL => {
            formatted.push_str("( ");
            formatted.extend_from_slice(&command_to_bstring(command.left.as_ref(buffer), buffer));
            formatted.push_str(" )");
        }
        CommandTag::BACKGROUND => {
            formatted.extend_from_slice(&command_to_bstring(command.left.as_ref(buffer), buffer));
        }
        CommandTag::IF => {
            formatted.push_str("if ");
            formatted.extend_from_slice(&command_to_bstring(command.cond.as_ref(buffer), buffer));
            formatted.push_str("; then ");
            formatted
                .extend_from_slice(&command_to_bstring(command.then_branch.as_ref(buffer), buffer));
            if !command.else_branch.is_null() {
                formatted.push_str("; else ");
                formatted.extend_from_slice(&command_to_bstring(
                    command.else_branch.as_ref(buffer),
                    buffer,
                ));
            }
            formatted.push_str("; fi");
        }
        CommandTag::WHILE => {
            formatted.push_str("while ");
            formatted.extend_from_slice(&command_to_bstring(command.cond.as_ref(buffer), buffer));
            formatted.push_str("; do ");
            formatted
                .extend_from_slice(&command_to_bstring(command.then_branch.as_ref(buffer), buffer));
            formatted.push_str("; done");
        }
        CommandTag::UNTIL => {
            formatted.push_str("until ");
            formatted.extend_from_slice(&command_to_bstring(command.cond.as_ref(buffer), buffer));
            formatted.push_str("; do ");
            formatted
                .extend_from_slice(&command_to_bstring(command.then_branch.as_ref(buffer), buffer));
            formatted.push_str("; done");
        }
        CommandTag::FOR => {
            formatted.push_str("for ");
            formatted.extend_from_slice(command.for_var.as_bstr(buffer));
            formatted.push_str(" in");
            for item in command.for_items.as_slice(buffer) {
                formatted.push_byte(b' ');
                formatted.extend_from_slice(&argument_to_bstring(item.as_slice(buffer), buffer));
            }
            formatted.push_str("; do ");
            formatted
                .extend_from_slice(&command_to_bstring(command.then_branch.as_ref(buffer), buffer));
            formatted.push_str("; done");
        }
        CommandTag::CASE => {
            formatted.push_str("case ");
            formatted.extend_from_slice(&argument_to_bstring(
                command.case_word.as_slice(buffer),
                buffer,
            ));
            formatted.push_str(" in ");
            for item in command.case_items.as_slice(buffer) {
                let pats = item.patterns.as_slice(buffer);
                for (i, pat) in pats.iter().enumerate() {
                    if i > 0 {
                        formatted.push_str(" | ");
                    }
                    formatted.extend_from_slice(&argument_to_bstring(pat.as_slice(buffer), buffer));
                }
                formatted.push_str(") ");
                formatted.extend_from_slice(&command_to_bstring(item.body.as_ref(buffer), buffer));
                formatted.push_str(";; ");
            }
            formatted.push_str("esac");
        }
        CommandTag::FUNCTION_DEF => {
            formatted.extend_from_slice(command.name.as_bstr(buffer));
            formatted.push_str("() { ... }");
        }
        CommandTag::LOGICAL_AND => {
            formatted.extend_from_slice(&command_to_bstring(command.left.as_ref(buffer), buffer));
            formatted.push_str(" && ");
            formatted.extend_from_slice(&command_to_bstring(command.right.as_ref(buffer), buffer));
        }
        CommandTag::LOGICAL_OR => {
            formatted.extend_from_slice(&command_to_bstring(command.left.as_ref(buffer), buffer));
            formatted.push_str(" || ");
            formatted.extend_from_slice(&command_to_bstring(command.right.as_ref(buffer), buffer));
        }
        CommandTag::SEQUENCE => {
            let cmds = command.sequence.as_slice(buffer);
            for (i, cmd_ptr) in cmds.iter().enumerate() {
                if i > 0 {
                    formatted.push_str("; ");
                }
                formatted.extend_from_slice(&command_to_bstring(cmd_ptr.as_ref(buffer), buffer));
            }
        }
        _ => unreachable!("invalid CommandTag: {}", command.tag.0),
    }
    formatted
}

fn argument_to_bstring(argument: &[WordPart], buffer: &relative::Buffer) -> BString {
    let mut formatted = BString::default();
    for word_part in argument {
        match word_part.tag {
            WordPartTag::LITERAL | WordPartTag::QUOTED_LITERAL => {
                formatted.extend_from_slice(word_part.text.as_bstr(buffer));
            }
            WordPartTag::VAR | WordPartTag::QUOTED_VAR => {
                formatted.push_byte(b'$');
                formatted.extend_from_slice(word_part.text.as_bstr(buffer));
            }
            WordPartTag::COMMAND_SUBSTITUTION | WordPartTag::QUOTED_COMMAND_SUBSTITUTION => {
                formatted.push_str("$(");
                formatted.extend_from_slice(&command_to_bstring(
                    word_part.command.as_ref(buffer),
                    buffer,
                ));
                formatted.push_byte(b')');
            }
            WordPartTag::ARITHMETIC | WordPartTag::QUOTED_ARITHMETIC => {
                formatted.push_str("$((");
                formatted.extend_from_slice(word_part.text.as_bstr(buffer));
                formatted.push_str("))");
            }
            _ => unreachable!("invalid WordPartTag: {}", word_part.tag.0),
        }
    }
    formatted
}
