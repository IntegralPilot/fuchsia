// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;

use super::{Context, Contextual};
use fidl_ir::{
    DeclType, EndpointRole, InternalSubtype, PartialTypeConstructor, PrimSubtype, Type, TypeKind,
};
use fidlgen::{LibraryExt as _, TypeShapeExt as _};

pub struct WireTypeTemplate<'a> {
    context: &'a Context,
    ty: &'a Type,
    from_alias: Option<&'a PartialTypeConstructor>,
    lifetime: &'a str,
}

impl<'a> WireTypeTemplate<'a> {
    pub fn new(
        ty: &'a Type,
        from_alias: Option<&'a PartialTypeConstructor>,
        lifetime: &'a str,
        context: &'a Context,
    ) -> Self {
        Self { context, ty, from_alias, lifetime }
    }

    pub fn with_de(
        ty: &'a Type,
        from_alias: Option<&'a PartialTypeConstructor>,
        context: &'a Context,
    ) -> Self {
        Self::new(ty, from_alias, "'de", context)
    }

    pub fn with_static(
        ty: &'a Type,
        from_alias: Option<&'a PartialTypeConstructor>,
        context: &'a Context,
    ) -> Self {
        Self::new(ty, from_alias, "'static", context)
    }

    pub fn with_anonymous(
        ty: &'a Type,
        from_alias: Option<&'a PartialTypeConstructor>,
        context: &'a Context,
    ) -> Self {
        Self::new(ty, from_alias, "'_", context)
    }
}

impl Contextual for WireTypeTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}

impl fmt::Display for WireTypeTemplate<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.ty.kind {
            TypeKind::Array { element_type, element_count, from_alias } => {
                let wire_ty =
                    Self::new(element_type, from_alias.as_ref(), self.lifetime, self.context);
                write!(f, "[{wire_ty}; {element_count}]")?;
            }
            TypeKind::Vector { element_type, nullable, from_alias, .. } => {
                let wire_ty =
                    Self::new(element_type, from_alias.as_ref(), self.lifetime, self.context);
                if *nullable {
                    write!(f, "::fidl_next::wire::OptionalVector<{}, {wire_ty}>", self.lifetime)?;
                } else {
                    write!(f, "::fidl_next::wire::Vector<{}, {wire_ty}>", self.lifetime)?;
                }
            }
            TypeKind::String { nullable, .. } => {
                if *nullable {
                    write!(f, "::fidl_next::wire::OptionalString<{}>", self.lifetime)?;
                } else {
                    write!(f, "::fidl_next::wire::String<{}>", self.lifetime)?;
                }
            }
            TypeKind::Handle { nullable, subtype, resource_identifier, .. } => {
                if *nullable {
                    write!(
                        f,
                        "{}",
                        self.resource_bindings()
                            .handle(resource_identifier)
                            .optional_wire_path(*subtype)
                    )?;
                } else {
                    write!(
                        f,
                        "{}",
                        self.resource_bindings().handle(resource_identifier).wire_path(*subtype)
                    )?;
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
                        "{role}<{protocol_id}, {}>",
                        self.resource_bindings().endpoint(protocol_transport).optional_wire_path,
                    )?;
                } else {
                    write!(
                        f,
                        "{role}<{protocol_id}, {}>",
                        self.resource_bindings().endpoint(protocol_transport).wire_path,
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
                    write!(f, "::fidl_next::wire::fuchsia::StatusResult")?;
                } else {
                    write!(f, "{}", self.wire_prim(*subtype))?;
                }
            }
            TypeKind::Identifier { identifier, nullable, .. } => {
                let wire_id = self.wire_id(identifier);

                match self.library().get_decl_type(identifier).unwrap() {
                    DeclType::Bits | DeclType::Enum => write!(f, "{wire_id}")?,
                    DeclType::Table => write!(f, "{wire_id}<{}>", self.lifetime)?,
                    DeclType::Struct => {
                        if *nullable {
                            write!(f, "::fidl_next::wire::Box<{}, ", self.lifetime)?;
                        }

                        write!(f, "{wire_id}")?;

                        if let Some(shape) = self.library().get_type_shape(identifier)
                            && !shape.is_static()
                        {
                            write!(f, "<{}>", self.lifetime)?;
                        }

                        if *nullable {
                            write!(f, ">")?;
                        }
                    }
                    DeclType::Union => {
                        let id = if *nullable {
                            self.wire_optional_id(identifier)
                        } else {
                            self.wire_id(identifier)
                        };
                        if self.ty.shape.is_static() {
                            write!(f, "{id}")?;
                        } else {
                            write!(f, "{id}<{}>", self.lifetime)?;
                        }
                    }
                    DeclType::ExperimentalResource => unreachable!(),
                    _ => unimplemented!(),
                }
            }
            TypeKind::Internal { subtype } => match subtype {
                InternalSubtype::FrameworkError => {
                    write!(f, "::fidl_next::wire::FrameworkError")?;
                }
            },
        }

        Ok(())
    }
}
