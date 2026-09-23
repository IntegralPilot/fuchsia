// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::parser::ast::{ASTBuilder, CommandTag, ResolvedWordPart, WordPartTag};

#[test]
fn test_ast_builder_resolved_word() {
    let mut builder = ASTBuilder::new();
    let parts = vec![
        ResolvedWordPart::Literal(bstr::BString::from("echo")),
        ResolvedWordPart::Var(bstr::BString::from("VAR")),
    ];
    let word_slice = builder.add_resolved_word(&parts);

    let slice = builder.get_slice(word_slice);
    assert_eq!(slice[0].tag, WordPartTag::LITERAL);
    assert_eq!(slice[0].text.as_bstr(&builder), "echo");
    assert_eq!(slice[1].tag, WordPartTag::VAR);
    assert_eq!(slice[1].text.as_bstr(&builder), "VAR");
}

#[test]
fn test_ast_builder_commands() {
    let mut builder = ASTBuilder::new();

    let parts1 = vec![ResolvedWordPart::Literal(bstr::BString::from("ls"))];
    let w1_slice = builder.add_resolved_word(&parts1);

    let parts2 = vec![ResolvedWordPart::Literal(bstr::BString::from("-l"))];
    let w2_slice = builder.add_resolved_word(&parts2);

    let arg_refs = vec![w1_slice, w2_slice];
    let cmd1_ptr = builder.add_simple_command(&arg_refs);

    let parts3 = vec![ResolvedWordPart::Literal(bstr::BString::from("grep"))];
    let w3_slice = builder.add_resolved_word(&parts3);
    let arg_refs2 = vec![w3_slice];
    let cmd2_ptr = builder.add_simple_command(&arg_refs2);

    let pipeline_ptr = builder.add_binary_command(CommandTag::PIPELINE, cmd1_ptr, cmd2_ptr);

    let pipeline = builder.get_ref(pipeline_ptr);
    assert_eq!(pipeline.tag, CommandTag::PIPELINE);

    let left = pipeline.left.as_ref(&builder);
    assert_eq!(left.tag, CommandTag::SIMPLE);
    let left_args = left.simple_args.as_slice(&builder);
    assert_eq!(left_args.len(), 2);
    assert_eq!(left_args[0].as_slice(&builder)[0].text.as_bstr(&builder), "ls");
    assert_eq!(left_args[1].as_slice(&builder)[0].text.as_bstr(&builder), "-l");

    let right = pipeline.right.as_ref(&builder);
    assert_eq!(right.tag, CommandTag::SIMPLE);
    let right_args = right.simple_args.as_slice(&builder);
    assert_eq!(right_args.len(), 1);
    assert_eq!(right_args[0].as_slice(&builder)[0].text.as_bstr(&builder), "grep");
}
