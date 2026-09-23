// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ops::Deref;

use askama::Template;
use fidlgen::rust::RustIdent as _;

use super::prim::{NaturalIntTemplate, WireIntTemplate};
use super::{Context, Contextual};
use fidl_ir::Bits;

pub struct BitsTemplate<'a> {
    bits: &'a Bits,
    context: &'a Context,

    name: String,
    natural_subtype: NaturalIntTemplate,
    wire_subtype: WireIntTemplate,
}

impl Contextual for BitsTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}

impl<'a> BitsTemplate<'a> {
    pub fn new(bits: &'a Bits, context: &'a Context) -> Self {
        Self {
            bits,
            context,

            name: bits.name.decl_name().camel(),
            natural_subtype: context.natural_int(bits.subtype()),
            wire_subtype: context.wire_int(bits.subtype()),
        }
    }

    pub fn natural(self) -> NaturalBitsTemplate<'a> {
        NaturalBitsTemplate { template: self }
    }

    pub fn wire(self) -> WireBitsTemplate<'a> {
        WireBitsTemplate { template: self }
    }
}

#[derive(Template)]
#[template(path = "natural/bits.askama", whitespace = "preserve")]
pub struct NaturalBitsTemplate<'a> {
    template: BitsTemplate<'a>,
}

impl<'a> Deref for NaturalBitsTemplate<'a> {
    type Target = BitsTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}

#[derive(Template)]
#[template(path = "wire/bits.askama", whitespace = "preserve")]
pub struct WireBitsTemplate<'a> {
    template: BitsTemplate<'a>,
}

impl<'a> Deref for WireBitsTemplate<'a> {
    type Target = BitsTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}
