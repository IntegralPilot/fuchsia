// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use askama::Template;

use super::{Context, Contextual};
use fidl_ir::{Const, TypeKind};
use fidlgen::rust::RustIdent as _;

#[derive(Template)]
#[template(path = "const.askama", whitespace = "preserve")]
pub struct ConstTemplate<'a> {
    cnst: &'a Const,
    context: &'a Context,

    name: String,
}

impl<'a> ConstTemplate<'a> {
    pub fn new(cnst: &'a Const, context: &'a Context) -> Self {
        Self { cnst, context, name: cnst.name.decl_name().screaming_snake() }
    }
}

impl Contextual for ConstTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}
