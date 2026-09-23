// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeSet;

use askama::Template;

use super::{Context, Contextual};
use fidl_ir::{CompoundIdent, Service, ServiceMember, TypeKind};
use fidlgen::rust::RustIdent as _;

#[derive(Template)]
#[template(path = "service.askama", whitespace = "preserve")]
pub struct ServiceTemplate<'a> {
    service: &'a Service,
    context: &'a Context,

    non_canonical_name: &'a str,
    service_name: String,
    connector_name: String,
    handler_name: String,
}

impl<'a> ServiceTemplate<'a> {
    pub fn new(service: &'a Service, context: &'a Context) -> Self {
        let base_name = service.name.decl_name().camel();
        let connector_name = format!("{base_name}Connector");
        let handler_name = format!("{base_name}Handler");

        Self {
            service,
            context,

            non_canonical_name: service.name.decl_name().non_canonical(),
            service_name: base_name,
            connector_name: connector_name,
            handler_name: handler_name,
        }
    }

    fn service_name(&self) -> String {
        let (library, name) = self.service.name.split();
        format!("{}.{}", library, name.camel())
    }

    fn member_protocol<'m>(&self, member: &'m ServiceMember) -> &'m CompoundIdent {
        let TypeKind::Endpoint { protocol, .. } = &member.ty.kind else {
            panic!("service member type must be an endpoint");
        };

        protocol
    }

    fn member_transport(&self, member: &ServiceMember) -> &str {
        let TypeKind::Endpoint { protocol_transport, .. } = &member.ty.kind else {
            panic!("service member type must be an endpoint");
        };

        &self.resource_bindings().endpoint(protocol_transport).natural_path
    }

    fn member_transports(&self) -> BTreeSet<&str> {
        self.service.members.iter().map(|member| self.member_transport(member)).collect()
    }
}

impl Contextual for ServiceTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}
