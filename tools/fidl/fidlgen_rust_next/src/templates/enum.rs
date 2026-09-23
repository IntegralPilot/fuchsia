// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ops::Deref;

use askama::Template;
use fidlgen::rust::RustIdent as _;

use super::prim::{NaturalIntTemplate, WireIntTemplate};
use super::{Context, Contextual};
use fidl_ir::{Enum, IntType};

pub struct EnumTemplate<'a> {
    enm: &'a Enum,
    context: &'a Context,

    name: String,
    natural_int: NaturalIntTemplate,
    wire_int: WireIntTemplate,
    unknown_ordinal_value: i128,
}

impl<'a> EnumTemplate<'a> {
    pub fn new(enm: &'a Enum, context: &'a Context) -> Self {
        let unknown_ordinal_value = enm
            .members
            .iter()
            .map(|m| m.value.value.parse::<i128>().unwrap() + 1)
            .max()
            .unwrap_or(0);

        Self {
            enm,
            context,

            name: enm.name.decl_name().camel(),
            natural_int: context.natural_int(enm.ty),
            wire_int: context.wire_int(enm.ty),
            unknown_ordinal_value,
        }
    }

    pub fn natural(self) -> NaturalEnumTemplate<'a> {
        NaturalEnumTemplate { template: self }
    }

    pub fn wire(self) -> WireEnumTemplate<'a> {
        WireEnumTemplate { template: self }
    }
}

impl Contextual for EnumTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}

#[derive(Template)]
#[template(path = "natural/enum.askama", whitespace = "preserve")]
pub struct NaturalEnumTemplate<'a> {
    template: EnumTemplate<'a>,
}

impl<'a> Deref for NaturalEnumTemplate<'a> {
    type Target = EnumTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}

#[derive(Template)]
#[template(path = "wire/enum.askama", whitespace = "preserve")]
pub struct WireEnumTemplate<'a> {
    template: EnumTemplate<'a>,
}

impl<'a> Deref for WireEnumTemplate<'a> {
    type Target = EnumTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}
