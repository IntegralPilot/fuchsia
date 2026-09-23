// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::any::Any;
use fidl_ir::{
    Attributes, CompoundIdent, CompoundIdentifier, Constant, IntType, Library,
    PartialTypeConstructor, PrimSubtype, Protocol, Service, Struct, Table, Type, TypeAlias, Union,
};
use fidlgen::{Denylist, LibraryExt as _};
use std::collections::{HashMap, hash_map};

use crate::config::{Config, ResourceBindings};
use crate::templates::{
    CompoundIdentifierTemplate, ConstantTemplate, DocStringTemplate, NaturalIntTemplate,
    NaturalPrimTemplate, NaturalTypeTemplate, WireIntTemplate, WirePrimTemplate, WireTypeTemplate,
    constraint_for,
};

bitflags::bitflags! {
    #[derive(Copy, Clone)]
    struct Derives: u16 {
        const DEBUG = 1;
        const COPY = 2;
        const CLONE = 4;
        const PARTIAL_EQ = 8;
        const EQ = 16;
        const ORD = 32;
        const PARTIAL_ORD = 64;
        const HASH = 128;
        const DEFAULT = 256;
    }
}

impl core::fmt::Display for Derives {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.is_empty() {
            return Ok(());
        }

        write!(f, "#[derive(")?;

        if self.contains(Derives::DEBUG) {
            write!(f, "Debug, ")?;
        }

        if self.contains(Derives::DEFAULT) {
            write!(f, "Default, ")?;
        }

        if self.contains(Derives::COPY) {
            write!(f, "Copy, ")?;
        }

        if self.contains(Derives::CLONE) {
            write!(f, "Clone, ")?;
        }

        if self.contains(Derives::PARTIAL_EQ) {
            write!(f, "PartialEq, ")?;
        }

        if self.contains(Derives::EQ) {
            write!(f, "Eq, ")?;
        }

        if self.contains(Derives::PARTIAL_ORD) {
            write!(f, "PartialOrd, ")?;
        }

        if self.contains(Derives::ORD) {
            write!(f, "Ord, ")?;
        }

        if self.contains(Derives::HASH) {
            write!(f, "Hash, ")?;
        }

        write!(f, ")]")
    }
}

/// Value representing a cached set of [`Derives`] for a given identifier.
enum DeriveState {
    /// Marks that we are trying to populate this entry in the cache, so that we
    /// don't end up infinitely recurring if we try to populate a recursive
    /// type.
    Recursing,
    /// This cache entry is populated.
    Complete(Derives),
}

/// Cache of sets of [`Derives`] for given identifiers.
struct DeriveCache {
    cache: HashMap<CompoundIdentifier, DeriveState>,
}

/// Used to pass state down through recursive calls to [`get_derives`]
struct DeriveRecursionState {
    /// Indicates we saw some identifier twice during recursion and had to
    /// supply a fake result to prevent recursing infinitely.
    did_recursive_short_circuit: bool,
}

impl DeriveCache {
    /// Get the [`Derives`] for a given identifier.
    fn get_derives(
        &mut self,
        library: &Library,
        ident: &CompoundIdent,
        recursion_state: Option<&mut DeriveRecursionState>,
    ) -> Derives {
        let (toplevel, recursion_state) = if let Some(recursion_state) = recursion_state {
            (false, recursion_state)
        } else {
            (true, &mut DeriveRecursionState { did_recursive_short_circuit: false })
        };

        let Some(decl) = library.get_local_decl(ident) else {
            let mut derives = Derives::DEBUG | Derives::PARTIAL_EQ;

            if library.get_is_resource(ident) == Some(false) {
                // Non-resource types also implement `Clone`
                derives |= Derives::CLONE;
            }

            return derives;
        };

        match self.cache.entry(ident.to_owned()) {
            hash_map::Entry::Vacant(v) => v.insert(DeriveState::Recursing),
            hash_map::Entry::Occupied(o) => match o.get() {
                DeriveState::Recursing => {
                    recursion_state.did_recursive_short_circuit = true;
                    return Derives::all();
                }
                DeriveState::Complete(x) => return *x,
            },
        };

        let ret = match decl.decl_type() {
            fidl_ir::DeclType::Alias
            | fidl_ir::DeclType::Const
            | fidl_ir::DeclType::ExperimentalResource
            | fidl_ir::DeclType::Service
            | fidl_ir::DeclType::NewType
            | fidl_ir::DeclType::Protocol => {
                panic!("Derive compiling encountered unexpected DeclType")
            }
            fidl_ir::DeclType::Bits | fidl_ir::DeclType::Enum => Derives::DEFAULT.complement(),
            fidl_ir::DeclType::Struct => {
                let Some(st) = (decl as &dyn Any).downcast_ref::<Struct>() else {
                    return Derives::empty();
                };
                let mut ret = Derives::DEFAULT.complement();

                for item in &st.members {
                    ret &= self.get_derives_for_type(library, &item.ty, recursion_state);
                }

                ret
            }
            fidl_ir::DeclType::Table => {
                let Some(table) = (decl as &dyn Any).downcast_ref::<Table>() else {
                    return Derives::empty();
                };
                let mut ret = Derives::all();

                for item in &table.members {
                    ret &= self.get_derives_for_type(library, &item.ty, recursion_state);
                }

                ret | Derives::DEFAULT
            }
            fidl_ir::DeclType::Union => {
                let Some(un) = (decl as &dyn Any).downcast_ref::<Union>() else {
                    return Derives::empty();
                };
                let mut ret = Derives::DEFAULT.complement();

                for item in &un.members {
                    ret &= self.get_derives_for_type(library, &item.ty, recursion_state);
                }

                ret
            }
        };
        if toplevel || !recursion_state.did_recursive_short_circuit {
            self.cache.insert(ident.to_owned(), DeriveState::Complete(ret));
        } else {
            let _ = self.cache.remove(ident);
        }
        ret
    }

    fn get_derives_for_type(
        &mut self,
        library: &Library,
        ty: &Type,
        recursion_state: &mut DeriveRecursionState,
    ) -> Derives {
        match &ty.kind {
            fidl_ir::TypeKind::Array { element_type, .. } => {
                self.get_derives_for_type(library, element_type, recursion_state)
            }
            fidl_ir::TypeKind::Vector { element_type, .. } => self
                .get_derives_for_type(library, element_type, recursion_state)
                .difference(Derives::COPY),
            fidl_ir::TypeKind::String { .. } => Derives::COPY.complement(),
            fidl_ir::TypeKind::Endpoint { .. } | fidl_ir::TypeKind::Handle { .. } => {
                // TODO: Old bindings support everything but clone here.
                Derives::DEBUG | Derives::PARTIAL_EQ
            }
            fidl_ir::TypeKind::Primitive {
                subtype: PrimSubtype::Float32 | PrimSubtype::Float64,
            } => (Derives::ORD | Derives::EQ | Derives::HASH).complement(),
            fidl_ir::TypeKind::Primitive { .. } => Derives::all(),
            fidl_ir::TypeKind::Identifier { identifier, nullable, .. } => {
                let Some(decl_type) = library.get_decl_type(identifier) else {
                    unreachable!("Identifier {identifier:?} matched no decl type");
                };

                match decl_type {
                    fidl_ir::DeclType::Bits | fidl_ir::DeclType::Enum => {
                        Derives::DEFAULT.complement()
                    }
                    fidl_ir::DeclType::Struct | fidl_ir::DeclType::Union => {
                        let got = self.get_derives(library, identifier, Some(recursion_state));

                        if *nullable {
                            got.difference(Derives::COPY) | Derives::DEFAULT
                        } else {
                            got
                        }
                    }
                    fidl_ir::DeclType::Table => {
                        self.get_derives(library, identifier, Some(recursion_state))
                    }
                    other => panic!("Unexpected identifier type {other:?}"),
                }
            }
            fidl_ir::TypeKind::Internal { .. } => Derives::empty(),
        }
    }
}

pub struct Context {
    library: Library,
    config: Config,
    derive_cache: fuchsia_sync::Mutex<DeriveCache>,
}

impl Context {
    pub fn new(library: Library, config: Config) -> Self {
        Self {
            library,
            config,
            derive_cache: fuchsia_sync::Mutex::new(DeriveCache { cache: HashMap::new() }),
        }
    }
}

pub trait Contextual {
    fn context(&self) -> &Context;

    // Helpers

    fn library(&self) -> &Library {
        &self.context().library
    }

    fn crate_prefix(&self) -> &String {
        &self.context().config.crate_prefix
    }

    fn resource_bindings(&self) -> &ResourceBindings {
        &self.context().config.resource_bindings
    }

    fn encode_trait_path(&self) -> &String {
        &self.context().config.encode_trait_path
    }

    fn decode_trait_path(&self) -> &String {
        &self.context().config.decode_trait_path
    }

    fn doc_string<'a>(&'a self, attributes: &'a Attributes) -> DocStringTemplate<'a> {
        DocStringTemplate::new(attributes)
    }

    fn emit_debug_impls(&self) -> bool {
        self.context().config.emit_debug_impls
    }

    fn non_type_id<'a>(
        &'a self,
        compound_ident: &'a CompoundIdent,
    ) -> CompoundIdentifierTemplate<'a> {
        CompoundIdentifierTemplate::non_type(compound_ident, self.context())
    }

    fn common_lib(&self) -> Option<&str> {
        self.context().config.common_lib.as_deref()
    }

    fn natural_id<'a>(
        &'a self,
        compound_ident: &'a CompoundIdent,
    ) -> CompoundIdentifierTemplate<'a> {
        CompoundIdentifierTemplate::natural(compound_ident, self.context())
    }

    fn wire_id<'a>(&'a self, compound_ident: &'a CompoundIdent) -> CompoundIdentifierTemplate<'a> {
        CompoundIdentifierTemplate::wire(compound_ident, self.context())
    }

    fn wire_optional_id<'a>(
        &'a self,
        compound_ident: &'a CompoundIdent,
    ) -> CompoundIdentifierTemplate<'a> {
        CompoundIdentifierTemplate::wire_optional(compound_ident, self.context())
    }

    fn generic_id<'a>(
        &'a self,
        compound_ident: &'a CompoundIdent,
    ) -> CompoundIdentifierTemplate<'a> {
        CompoundIdentifierTemplate::generic(compound_ident, self.context())
    }

    fn natural_int(&self, int: IntType) -> NaturalIntTemplate {
        NaturalIntTemplate(int)
    }

    fn natural_prim(&self, prim: PrimSubtype) -> NaturalPrimTemplate {
        NaturalPrimTemplate(prim)
    }

    fn natural_type<'a>(
        &'a self,
        ty: &'a Type,
        from_alias: Option<&'a PartialTypeConstructor>,
    ) -> NaturalTypeTemplate<'a> {
        NaturalTypeTemplate::new(ty, from_alias, self.context())
    }

    fn wire_int(&self, int: IntType) -> WireIntTemplate {
        WireIntTemplate(int)
    }

    fn wire_prim(&self, prim: PrimSubtype) -> WirePrimTemplate {
        WirePrimTemplate(prim)
    }

    fn wire_type<'a>(
        &'a self,
        ty: &'a Type,
        from_alias: Option<&'a PartialTypeConstructor>,
    ) -> WireTypeTemplate<'a> {
        WireTypeTemplate::with_de(ty, from_alias, self.context())
    }

    fn static_wire_type<'a>(
        &'a self,
        ty: &'a Type,
        from_alias: Option<&'a PartialTypeConstructor>,
    ) -> WireTypeTemplate<'a> {
        WireTypeTemplate::with_static(ty, from_alias, self.context())
    }

    fn anonymous_wire_type<'a>(
        &'a self,
        ty: &'a Type,
        from_alias: Option<&'a PartialTypeConstructor>,
    ) -> WireTypeTemplate<'a> {
        WireTypeTemplate::with_anonymous(ty, from_alias, self.context())
    }

    fn constant<'a>(&'a self, constant: &'a Constant, ty: &'a Type) -> ConstantTemplate<'a> {
        ConstantTemplate::new(constant, ty, self.context())
    }

    fn rust_next_denylist(&self, ident: &CompoundIdent) -> Denylist {
        self.context().library.denylist_for(ident, &["rust_next"])
    }

    fn constraint(&self, ty: &Type) -> String {
        match constraint_for(ty) {
            Some(constraint) => constraint,
            None => "()".to_string(),
        }
    }

    fn validate(&self, ty: &Type, field_name: &str) -> String {
        if let Some(constraint) = constraint_for(ty) {
            format!("::fidl_next::Constrained::validate({field_name}, {constraint})?;",)
        } else {
            String::new()
        }
    }

    fn emit_given_commonness(&self, ident: &CompoundIdent) -> bool {
        if self.context().config.is_common {
            !has_resources(self.context().library(), ident)
        } else if self.context().config.common_lib.is_some() {
            has_resources(self.context().library(), ident)
        } else {
            true
        }
    }

    fn derive_annotation(&self, ident: &CompoundIdent) -> String {
        let got =
            self.context().derive_cache.lock().get_derives(&self.context().library, ident, None);
        if self.emit_debug_impls() {
            got.to_string()
        } else {
            got.difference(Derives::DEBUG).to_string()
        }
    }
}

impl Contextual for Context {
    fn context(&self) -> &Context {
        self
    }
}

fn has_resources(library: &Library, ident: &CompoundIdent) -> bool {
    let Some(decl) = library.get_local_decl(ident) else {
        return true;
    };

    if let Some(is_resource) = decl.is_resource() {
        is_resource
    } else if (decl as &dyn Any).is::<Service>() {
        true
    } else if let Some(protocol) = (decl as &dyn Any).downcast_ref::<Protocol>() {
        for method in &protocol.methods {
            if let Some(ty) = &method.maybe_request_payload
                && type_has_resources(library, ty)
            {
                return true;
            }

            if let Some(ty) = &method.maybe_response_payload
                && type_has_resources(library, ty)
            {
                return true;
            }

            if let Some(ty) = &method.maybe_response_err_type
                && type_has_resources(library, ty)
            {
                return true;
            }

            if let Some(ty) = &method.maybe_response_success_type
                && type_has_resources(library, ty)
            {
                return true;
            }
        }

        false
    } else if let Some(alias) = (decl as &dyn Any).downcast_ref::<TypeAlias>() {
        type_has_resources(library, &alias.ty)
    } else {
        unreachable!("Did not recognize Decl type");
    }
}

fn type_has_resources(library: &Library, ty: &Type) -> bool {
    match &ty.kind {
        fidl_ir::TypeKind::Array { element_type, .. }
        | fidl_ir::TypeKind::Vector { element_type, .. } => {
            type_has_resources(library, element_type)
        }
        fidl_ir::TypeKind::String { .. } => false,
        fidl_ir::TypeKind::Handle { .. } => true,
        fidl_ir::TypeKind::Endpoint { .. } => true,
        fidl_ir::TypeKind::Primitive { .. } => false,
        fidl_ir::TypeKind::Identifier { identifier, .. } => has_resources(library, identifier),
        fidl_ir::TypeKind::Internal { .. } => false,
    }
}
