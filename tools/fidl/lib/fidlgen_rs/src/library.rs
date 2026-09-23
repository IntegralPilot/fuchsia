// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_ir::*;

use crate::{Decl, Denylist};

pub trait LibraryExt {
    fn get_local_decl(&self, ident: &CompoundIdent) -> Option<&dyn Decl>;
    fn get_decl_type(&self, ident: &CompoundIdent) -> Option<DeclType>;
    fn get_is_resource(&self, ident: &CompoundIdent) -> Option<bool>;
    fn get_type_shape(&self, ident: &CompoundIdent) -> Option<&TypeShape>;

    fn denylist_for(&self, ident: &CompoundIdent, bindings: &[&str]) -> Denylist;
}

impl LibraryExt for Library {
    fn get_local_decl(&self, ident: &CompoundIdent) -> Option<&dyn Decl> {
        match self.declarations.get(ident)? {
            DeclType::Alias => Some(self.alias_declarations.get(ident)?),
            DeclType::Bits => Some(self.bits_declarations.get(ident)?),
            DeclType::Const => Some(self.const_declarations.get(ident)?),
            DeclType::Enum => Some(self.enum_declarations.get(ident)?),
            DeclType::Protocol => Some(self.protocol_declarations.get(ident)?),
            DeclType::Service => Some(self.service_declarations.get(ident)?),
            DeclType::Struct => Some(self.struct_declarations.get(ident)?),
            DeclType::Table => Some(self.table_declarations.get(ident)?),
            DeclType::Union => Some(self.union_declarations.get(ident)?),
            DeclType::NewType | DeclType::ExperimentalResource => None,
        }
    }

    fn get_decl_type(&self, ident: &CompoundIdent) -> Option<DeclType> {
        let library = ident.library();
        if library == self.name {
            Some(self.get_local_decl(ident)?.decl_type())
        } else {
            Some(self.library_dependencies.get(library)?.declarations.get(ident)?.kind)
        }
    }

    fn get_is_resource(&self, ident: &CompoundIdent) -> Option<bool> {
        let library = ident.library();
        if library == self.name {
            self.get_local_decl(ident)?.is_resource()
        } else {
            Some(self.library_dependencies.get(library)?.declarations.get(ident)?.is_resouce)
        }
    }

    fn get_type_shape(&self, ident: &CompoundIdent) -> Option<&TypeShape> {
        let library = ident.library();
        if library == self.name {
            self.get_local_decl(ident)?.type_shape()
        } else {
            self.library_dependencies.get(library)?.declarations.get(ident)?.shape.as_ref()
        }
    }

    fn denylist_for(&self, ident: &CompoundIdent, bindings: &[&str]) -> Denylist {
        Denylist::for_ident(self, ident, bindings)
    }
}
