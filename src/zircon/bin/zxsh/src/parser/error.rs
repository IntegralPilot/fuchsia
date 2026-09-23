// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompleteReason {
    Quote,
    Brace,
    Paren,
    Arithmetic,
    LineContinuation,
    Pipeline,
    LogicalOperator,
    Heredoc,
    Keyword,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Incomplete(IncompleteReason),
    Syntax(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Incomplete(reason) => write!(f, "Incomplete input ({:?})", reason),
            ParseError::Syntax(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for ParseError {}
