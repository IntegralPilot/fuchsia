// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ops::Deref;

use askama::Template;
use fidlgen::rust::RustIdent as _;

use super::{Context, Contextual};
use fidl_ir::{Table, TypeKind};

pub struct TableTemplate<'a> {
    table: &'a Table,
    context: &'a Context,

    name: String,
}

impl<'a> TableTemplate<'a> {
    pub fn new(table: &'a Table, context: &'a Context) -> Self {
        Self { table, context, name: table.name.decl_name().camel() }
    }

    pub fn natural(self) -> NaturalTableTemplate<'a> {
        NaturalTableTemplate { template: self }
    }

    pub fn wire(self) -> WireTableTemplate<'a> {
        WireTableTemplate { template: self }
    }
}

impl Contextual for TableTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}

#[derive(Template)]
#[template(path = "natural/table.askama", whitespace = "preserve")]
pub struct NaturalTableTemplate<'a> {
    template: TableTemplate<'a>,
}

impl<'a> Deref for NaturalTableTemplate<'a> {
    type Target = TableTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}

#[derive(Template)]
#[template(path = "wire/table.askama", whitespace = "preserve")]
pub struct WireTableTemplate<'a> {
    template: TableTemplate<'a>,
}

impl<'a> Deref for WireTableTemplate<'a> {
    type Target = TableTemplate<'a>;

    fn deref(&self) -> &Self::Target {
        &self.template
    }
}
