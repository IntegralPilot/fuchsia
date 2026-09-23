// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `parsed_board_dump` is the foundational library defining the Abstract Syntax Tree (AST),
//! normalization, comparison, and formatting tools for Fuchsia hardware board topologies.
//!
//! It serves as the standard, canonical intermediate representation used across
//! Board DML compilers, validators, Inspect dumpers, Devicetree parsers, and host verification tools.

pub mod ast;
pub mod compare;
pub mod format;
pub mod parse;

pub use ast::*;
pub use compare::*;
pub use format::*;
pub use parse::*;
