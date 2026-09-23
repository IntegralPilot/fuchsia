// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;

use super::{Context, Contextual};
use fidl_ir::{EndpointRole, InternalSubtype, PartialTypeConstructor, PrimSubtype, Type, TypeKind};

pub struct NaturalTypeTemplate<'a> {
    context: &'a Context,
    ty: &'a Type,
    from_alias: Option<&'a PartialTypeConstructor>,
}

impl<'a> NaturalTypeTemplate<'a> {
    pub fn new(
        ty: &'a Type,
        from_alias: Option<&'a PartialTypeConstructor>,
        context: &'a Context,
    ) -> Self {
        Self { context, ty, from_alias }
    }
}

impl Contextual for NaturalTypeTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}

impl fmt::Display for NaturalTypeTemplate<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.ty.kind {
            TypeKind::Array { element_type, element_count, from_alias } => {
                let natural_ty = Self::new(element_type, from_alias.as_ref(), self.context);
                write!(f, "[{natural_ty}; {element_count}]")?;
            }
            TypeKind::Vector { element_type, nullable, from_alias, .. } => {
                let natural_ty = Self::new(element_type, from_alias.as_ref(), self.context);
                if *nullable {
                    write!(f, "::core::option::Option<::std::vec::Vec<{natural_ty}>>")?;
                } else {
                    write!(f, "::std::vec::Vec<{natural_ty}>")?;
                }
            }
            TypeKind::String { nullable, .. } => {
                if *nullable {
                    write!(f, "::core::option::Option<::std::string::String>")?;
                } else {
                    write!(f, "::std::string::String")?;
                }
            }
            TypeKind::Handle { nullable, subtype, resource_identifier, .. } => {
                let handle_ty =
                    self.resource_bindings().handle(resource_identifier).natural_path(*subtype);
                if *nullable {
                    write!(f, "::core::option::Option<{handle_ty}>")?;
                } else {
                    write!(f, "{handle_ty}")?;
                }
            }
            TypeKind::Endpoint { nullable, role, protocol, protocol_transport } => {
                let role = match role {
                    EndpointRole::Client => "::fidl_next::ClientEnd",
                    EndpointRole::Server => "::fidl_next::ServerEnd",
                };
                let protocol_id = self.non_type_id(protocol);
                if *nullable {
                    write!(
                        f,
                        "::core::option::Option<{role}<{protocol_id}, {}>>",
                        self.resource_bindings().endpoint(protocol_transport).natural_path,
                    )?;
                } else {
                    write!(
                        f,
                        "{role}<{protocol_id}, {}>",
                        self.resource_bindings().endpoint(protocol_transport).natural_path,
                    )?;
                }
            }
            TypeKind::Primitive { subtype } => {
                if matches!(subtype, PrimSubtype::Int32)
                    && self.from_alias.is_some_and(|from_alias| {
                        from_alias.name.library() == "zx"
                            && from_alias.name.decl_name().non_canonical() == "Status"
                    })
                {
                    write!(f, "::core::result::Result<(), ::fidl_next::fuchsia::zx::Status>")?;
                } else {
                    write!(f, "{}", self.natural_prim(*subtype))?;
                }
            }
            TypeKind::Identifier { identifier, nullable, .. } => {
                let natural_id = self.natural_id(identifier);
                if *nullable {
                    write!(f, "::core::option::Option<::std::boxed::Box<{natural_id}>>")?;
                } else {
                    write!(f, "{natural_id}")?;
                }
            }
            TypeKind::Internal { subtype } => match subtype {
                InternalSubtype::FrameworkError => {
                    write!(f, "::fidl_next::FrameworkError")?;
                }
            },
        }

        Ok(())
    }
}
