// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use askama::Template;
use fidl_ir::Enum;
use fidlgen::rust::RustIdent as _;

use crate::context::{Context, Contextual};
use crate::ident::CompatRustIdent as _;

#[derive(Template)]
#[template(path = "enum.askama")]
pub struct EnumTemplate<'a> {
    enm: &'a Enum,
    context: &'a Context,

    ty: String,
    next_ty: String,
}

impl<'a> EnumTemplate<'a> {
    pub fn new(enm: &'a Enum, context: &'a Context) -> Self {
        Self {
            enm,
            context,

            ty: format!("{}::{}", context.rust_crate(), enm.name.decl_name().compat_camel()),
            next_ty: format!("{}::{}", context.rust_next_crate(), enm.name.decl_name().camel()),
        }
    }
}

impl Contextual for EnumTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}
