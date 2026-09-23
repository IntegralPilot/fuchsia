// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;

use super::{Context, Contextual};
use fidl_ir::{CompoundIdent, DeclType};
use fidlgen::LibraryExt as _;
use fidlgen::rust::RustIdent as _;

enum Module {
    None,
    Natural,
    Wire,
    WireOptional,
    Generic,
}

pub struct CompoundIdentifierTemplate<'a> {
    context: &'a Context,
    id: &'a CompoundIdent,
    module: Module,
}

impl<'a> CompoundIdentifierTemplate<'a> {
    fn new(id: &'a CompoundIdent, module: Module, context: &'a Context) -> Self {
        Self { context, id, module }
    }

    pub fn non_type(id: &'a CompoundIdent, context: &'a Context) -> Self {
        Self::new(id, Module::None, context)
    }

    pub fn natural(id: &'a CompoundIdent, context: &'a Context) -> Self {
        Self::new(id, Module::Natural, context)
    }

    pub fn wire(id: &'a CompoundIdent, context: &'a Context) -> Self {
        Self::new(id, Module::Wire, context)
    }

    pub fn wire_optional(id: &'a CompoundIdent, context: &'a Context) -> Self {
        Self::new(id, Module::WireOptional, context)
    }

    pub fn generic(id: &'a CompoundIdent, context: &'a Context) -> Self {
        Self::new(id, Module::Generic, context)
    }
}

impl Contextual for CompoundIdentifierTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}

impl fmt::Display for CompoundIdentifierTemplate<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (lib, ty) = self.id.split();

        // Special case: zx::ObjType
        if lib == "zx" {
            match self.module {
                Module::None | Module::Generic => (),
                Module::Natural => {
                    // Natural type
                    match ty.non_canonical() {
                        "ObjType" => return write!(f, "::fidl_next::fuchsia::zx::ObjectType"),
                        "Rights" => return write!(f, "::fidl_next::fuchsia::zx::Rights"),
                        _ => (),
                    }
                }
                Module::Wire | Module::WireOptional => {
                    // Wire type
                    match ty.non_canonical() {
                        "ObjType" => {
                            return write!(f, "::fidl_next::wire::fuchsia::ObjectType");
                        }
                        "Rights" => return write!(f, "::fidl_next::wire::fuchsia::Rights"),
                        _ => (),
                    }
                }
            }
        } else if lib == self.library().name {
            write!(f, "crate::")?;
        } else {
            let escaped = lib.replace('.', "_");
            write!(f, "::{}_{escaped}::", self.context.crate_prefix())?;
        }

        match self.module {
            Module::None => (),
            Module::Natural => write!(f, "natural::")?,
            Module::Wire => write!(f, "wire::")?,
            Module::WireOptional => write!(f, "wire_optional::")?,
            Module::Generic => write!(f, "generic::")?,
        }

        // Type name
        let name = match self.library().get_decl_type(self.id).unwrap() {
            DeclType::Alias
            | DeclType::Bits
            | DeclType::Enum
            | DeclType::Struct
            | DeclType::Table
            | DeclType::Union
            | DeclType::Protocol => ty.camel(),
            DeclType::Const => ty.screaming_snake(),
            DeclType::ExperimentalResource | DeclType::NewType | DeclType::Service => {
                todo!()
            }
        };
        write!(f, "{name}")?;

        Ok(())
    }
}
