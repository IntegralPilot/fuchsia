// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ops::Deref;

use askama::Template;

use super::{Context, Contextual};
use fidl_ir::{Struct, TypeKind};
use fidlgen::TypeShapeExt as _;
use fidlgen::rust::RustIdent as _;

pub struct StructTemplate<'a> {
    strct: &'a Struct,
    context: &'a Context,

    is_empty: bool,
    is_static: bool,
    has_padding: bool,
    name: String,

    de: &'static str,
    infer: &'static str,
    static_: &'static str,
}

impl<'a> StructTemplate<'a> {
    pub fn new(strct: &'a Struct, context: &'a Context) -> Self {
        let is_empty = strct.members.is_empty();
        let is_static = strct.shape.is_static();

        let (de, infer, static_) =
            if is_static { ("", "", "") } else { ("<'de>", "<'_>", "<'static>") };

        Self {
            strct,
            context,

            is_empty,
            is_static,
            has_padding: strct.shape.has_padding,
            name: strct.name.decl_name().camel(),

            de,
            infer,
            static_,
        }
    }

    pub fn natural(self) -> NaturalStructTemplate<'a> {
        NaturalStructTemplate { template: self }
    }

    pub fn wire(self) -> WireStructTemplate<'a> {
        WireStructTemplate { template: self }
    }

    pub fn generic(self) -> GenericStructTemplate<'a> {
        GenericStructTemplate { template: self }
    }
}

impl Contextual for StructTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}

struct ZeroPaddingRange {
    offset: u32,
    width: u32,
}

impl StructTemplate<'_> {
    fn zero_padding_ranges(&self) -> Vec<ZeroPaddingRange> {
        let mut ranges = Vec::new();
        let mut end = self.strct.shape.inline_size;
        for member in self.strct.members.iter().rev() {
            let padding = member.field_shape.padding;
            if padding != 0 {
                ranges.push(ZeroPaddingRange { offset: end - padding, width: padding });
            }
            end = member.field_shape.offset;
        }

        ranges
    }
}

#[derive(Template)]
#[template(path = "natural/struct.askama", whitespace = "preserve")]
pub struct NaturalStructTemplate<'a> {
    template: StructTemplate<'a>,
}

impl<'a> Deref for NaturalStructTemplate<'a> {
    type Target = StructTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}

#[derive(Template)]
#[template(path = "wire/struct.askama", whitespace = "preserve")]
pub struct WireStructTemplate<'a> {
    template: StructTemplate<'a>,
}

impl<'a> Deref for WireStructTemplate<'a> {
    type Target = StructTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}

#[derive(Template)]
#[template(path = "generic/struct.askama", whitespace = "preserve")]
pub struct GenericStructTemplate<'a> {
    template: StructTemplate<'a>,
}

impl<'a> Deref for GenericStructTemplate<'a> {
    type Target = StructTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}
