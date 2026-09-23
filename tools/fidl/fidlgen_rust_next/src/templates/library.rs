// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use askama::Template;

use fidl_ir::{Bits, Const, DeclType, Enum, Protocol, Service, Struct, Table, TypeAlias, Union};
use fidlgen::Denylist;

use super::{
    AliasTemplate, BitsTemplate, ConstTemplate, Context, Contextual, EnumTemplate,
    ProtocolTemplate, ServiceTemplate, StructTemplate, TableTemplate, UnionTemplate,
};

#[derive(Template)]
#[template(path = "library.askama")]
pub struct LibraryTemplate<'a> {
    context: &'a Context,
}

impl<'a> LibraryTemplate<'a> {
    pub fn new(context: &'a Context) -> Self {
        Self { context }
    }

    fn alias(&self, alias: &'a TypeAlias) -> AliasTemplate<'a> {
        AliasTemplate::new(alias, self.context)
    }

    fn bits(&self, bits: &'a Bits) -> BitsTemplate<'a> {
        BitsTemplate::new(bits, self.context)
    }

    fn cnst(&self, cnst: &'a Const) -> ConstTemplate<'a> {
        ConstTemplate::new(cnst, self.context)
    }

    fn enm(&self, enm: &'a Enum) -> EnumTemplate<'a> {
        EnumTemplate::new(enm, self.context)
    }

    fn protocol(&self, protocol: &'a Protocol) -> ProtocolTemplate<'a> {
        ProtocolTemplate::new(protocol, self.context)
    }

    fn service(&self, service: &'a Service) -> ServiceTemplate<'a> {
        ServiceTemplate::new(service, self.context)
    }

    fn strct(&self, strct: &'a Struct) -> StructTemplate<'a> {
        StructTemplate::new(strct, self.context)
    }

    fn table(&self, table: &'a Table) -> TableTemplate<'a> {
        TableTemplate::new(table, self.context)
    }

    fn union_(&self, union: &'a Union) -> UnionTemplate<'a> {
        UnionTemplate::new(union, self.context)
    }
}

impl Contextual for LibraryTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}
