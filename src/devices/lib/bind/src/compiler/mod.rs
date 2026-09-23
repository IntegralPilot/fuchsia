// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod compiler;
pub mod dependency_graph;
pub mod instruction;
pub mod symbol_table;

pub use self::compiler::{
    BindRules, BindRulesDecodeError, CompiledBindRules, CompilerError, CompositeBindRules,
    CompositeParent, SymbolicInstruction, SymbolicInstructionInfo, compile, compile_bind,
    compile_statements,
};

pub use self::symbol_table::{
    Symbol, SymbolTable, get_deprecated_key_identifier, get_deprecated_key_identifiers,
    get_deprecated_key_value,
};

pub mod test_lib;
