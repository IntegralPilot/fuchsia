// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ops::Deref;

use askama::Template;

use super::natural_type::NaturalTypeTemplate;
use super::wire_type::WireTypeTemplate;
use super::{Context, Contextual};
use fidl_ir::TypeAlias;
use fidlgen::TypeShapeExt as _;
use fidlgen::rust::RustIdent as _;

pub struct AliasTemplate<'a> {
    alias: &'a TypeAlias,
    context: &'a Context,

    name: String,
    is_static: bool,
    natural_ty: NaturalTypeTemplate<'a>,
    wire_ty: WireTypeTemplate<'a>,
}

impl Contextual for AliasTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}

impl<'a> AliasTemplate<'a> {
    pub fn new(alias: &'a TypeAlias, context: &'a Context) -> Self {
        Self {
            alias,
            context,

            name: alias.name.decl_name().camel(),
            is_static: alias.ty.shape.is_static(),
            natural_ty: context.natural_type(&alias.ty, alias.from_alias.as_ref()),
            wire_ty: context.wire_type(&alias.ty, alias.from_alias.as_ref()),
        }
    }

    pub fn natural(self) -> NaturalAliasTemplate<'a> {
        NaturalAliasTemplate { template: self }
    }

    pub fn wire(self) -> WireAliasTemplate<'a> {
        WireAliasTemplate { template: self }
    }
}

#[derive(Template)]
#[template(path = "natural/alias.askama", whitespace = "preserve")]
pub struct NaturalAliasTemplate<'a> {
    template: AliasTemplate<'a>,
}

impl<'a> Deref for NaturalAliasTemplate<'a> {
    type Target = AliasTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}

#[derive(Template)]
#[template(path = "wire/alias.askama", whitespace = "preserve")]
pub struct WireAliasTemplate<'a> {
    template: AliasTemplate<'a>,
}

impl<'a> Deref for WireAliasTemplate<'a> {
    type Target = AliasTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}
