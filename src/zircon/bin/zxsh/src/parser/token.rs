// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bstr::{BStr, BString, ByteSlice};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawWordPart {
    Literal(BString),
    Var(BString),
    QuotedLiteral(BString),
    QuotedVar(BString),
    CommandSubstitution(BString),
    QuotedCommandSubstitution(BString),
    Arithmetic(BString),
    QuotedArithmetic(BString),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Word(Vec<RawWordPart>),
    Pipe,                            // |
    RedirectOut(Option<i32>),        // [fd]>
    RedirectOutClobber(Option<i32>), // [fd]>|
    RedirectAppend(Option<i32>),     // [fd]>>
    RedirectIn(Option<i32>),         // [fd]<
    RedirectDupOut(Option<i32>),     // [fd]>&
    RedirectDupIn(Option<i32>),      // [fd]<&
    LParen,                          // (
    RParen,                          // )
    Semi,                            // ;
    DoubleSemi,                      // ;;
    Newline,                         // \n
    And,                             // &&
    Or,                              // ||
    Ampersand,                       // &
    RedirectHereDoc {
        src_fd: Option<i32>,
        delimiter: Vec<RawWordPart>,
        body: BString,
        expand: bool,
    },
    RedirectHereDocPlaceholder {
        src_fd: Option<i32>,
        delimiter: Vec<RawWordPart>,
        strip_tabs: bool,
    },
}

impl Token {
    pub fn as_unquoted_bstr(&self) -> Option<&BStr> {
        match self {
            Token::Word(parts) => {
                if parts.len() == 1 {
                    match &parts[0] {
                        RawWordPart::Literal(s) => Some(s.as_bstr()),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
