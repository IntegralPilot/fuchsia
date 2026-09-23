// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;

use super::{Context, Contextual};
use fidl_ir::{Constant, ConstantKind, DeclType, LiteralKind, Type, TypeKind};
use fidlgen::LibraryExt as _;
use fidlgen::rust::RustIdent as _;

pub struct ConstantTemplate<'a> {
    constant: &'a Constant,
    ty: &'a Type,
    context: &'a Context,
}

impl<'a> ConstantTemplate<'a> {
    pub fn new(constant: &'a Constant, ty: &'a Type, context: &'a Context) -> Self {
        Self { constant, ty, context }
    }
}

impl Contextual for ConstantTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}

impl fmt::Display for ConstantTemplate<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.constant.kind {
            ConstantKind::Identifier { identifier } => {
                let (comp_id, member) = identifier.split();
                let item = self.natural_id(comp_id);

                if let Some(member) = member {
                    let decl_type = self.library().get_decl_type(comp_id).unwrap();
                    let member_name = match decl_type {
                        DeclType::Bits => member.screaming_snake(),
                        DeclType::Enum => {
                            if comp_id.library() == "zx"
                                && comp_id.decl_name().non_canonical() == "ObjType"
                            {
                                member.screaming_snake()
                            } else {
                                member.camel()
                            }
                        }
                        _ => panic!("expected member to be bits or enum"),
                    };
                    write!(f, "{item}::{member_name}")?;
                } else {
                    write!(f, "{item}")?;
                }
            }
            ConstantKind::Literal { literal } => match literal.kind {
                LiteralKind::String => write!(f, "\"{}\"", literal.value.escape_default())?,
                LiteralKind::Bool => write!(f, "{}", literal.value)?,
                LiteralKind::Numeric => match &self.ty.kind {
                    TypeKind::Identifier { identifier, .. } => write!(
                        f,
                        "{}::from_bits_retain({})",
                        self.natural_id(identifier),
                        self.constant.value
                    )?,
                    TypeKind::Primitive { .. } => write!(f, "{}", literal.value)?,
                    _ => panic!("invalid constant type"),
                },
            },
            ConstantKind::BinaryOperator => {
                if let TypeKind::Identifier { identifier, .. } = &self.ty.kind {
                    write!(
                        f,
                        "{}::from_bits_retain({})",
                        self.natural_id(identifier),
                        self.constant.value
                    )?;
                } else {
                    panic!("invalid constant type");
                }
            }
        }

        Ok(())
    }
}
