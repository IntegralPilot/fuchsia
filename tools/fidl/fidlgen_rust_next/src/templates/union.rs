// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ops::Deref;

use askama::Template;

use super::{Context, Contextual};
use fidl_ir::Union;
use fidlgen::TypeShapeExt as _;
use fidlgen::rust::RustIdent as _;

pub struct UnionTemplate<'a> {
    union_: &'a Union,
    context: &'a Context,

    is_static: bool,
    name: String,
    mod_name: String,

    de: &'static str,
    infer: &'static str,
    static_: &'static str,
    phantom: &'static str,
    decode_unknown: &'static str,
    decode_as: &'static str,
    encode_as: &'static str,
}

impl<'a> UnionTemplate<'a> {
    pub fn new(union_: &'a Union, context: &'a Context) -> Self {
        let is_static = union_.shape.is_static();

        let (de, infer, static_, phantom, decode_unknown, decode_as, encode_as) = if is_static {
            ("", "", "", "()", "decode_unknown_static", "decode_as_static", "encode_as_static")
        } else {
            (
                "<'de>",
                "<'_>",
                "<'static>",
                "&'de mut [::fidl_next::Chunk]",
                "decode_unknown",
                "decode_as",
                "encode_as",
            )
        };

        Self {
            union_,
            context,

            is_static,
            name: union_.name.decl_name().camel(),
            mod_name: union_.name.decl_name().snake(),

            de,
            infer,
            static_,
            phantom,
            decode_unknown,
            decode_as,
            encode_as,
        }
    }

    fn has_only_static_members(&self) -> bool {
        self.union_.members.iter().all(|m| m.ty.shape.is_static())
    }

    pub fn natural(self) -> NaturalUnionTemplate<'a> {
        NaturalUnionTemplate { template: self }
    }

    pub fn wire(self) -> WireUnionTemplate<'a> {
        WireUnionTemplate { template: self }
    }

    pub fn wire_optional(self) -> WireOptionalUnionTemplate<'a> {
        WireOptionalUnionTemplate { template: self }
    }
}

impl Contextual for UnionTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}

#[derive(Template)]
#[template(path = "natural/union.askama", whitespace = "preserve")]
pub struct NaturalUnionTemplate<'a> {
    template: UnionTemplate<'a>,
}

impl<'a> Deref for NaturalUnionTemplate<'a> {
    type Target = UnionTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}

#[derive(Template)]
#[template(path = "wire/union.askama", whitespace = "preserve")]
pub struct WireUnionTemplate<'a> {
    template: UnionTemplate<'a>,
}

impl<'a> Deref for WireUnionTemplate<'a> {
    type Target = UnionTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}

#[derive(Template)]
#[template(path = "wire_optional/union.askama", whitespace = "preserve")]
pub struct WireOptionalUnionTemplate<'a> {
    template: UnionTemplate<'a>,
}

impl<'a> Deref for WireOptionalUnionTemplate<'a> {
    type Target = UnionTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}
