// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeSet;

use askama::Template;

use super::{Context, Contextual};
use fidl_ir::{
    CompoundIdent, CompoundIdentifier, PartialTypeConstructor, Protocol, ProtocolMethod,
    ProtocolMethodKind, ProtocolOpenness, Struct, Type, TypeKind, Union,
};
use fidlgen::TypeShapeExt as _;
use fidlgen::rust::RustIdent as _;

#[derive(Template)]
#[template(path = "protocol.askama", whitespace = "preserve")]
pub struct ProtocolTemplate<'a> {
    protocol: &'a Protocol,
    context: &'a Context,

    non_canonical_name: &'a str,
    protocol_name: String,
    module_name: String,

    client_name: String,
    local_client_handler_name: String,
    send_client_handler_name: String,

    server_name: String,
    local_server_handler_name: String,
    send_server_handler_name: String,

    transport: Option<Transport<'a>>,
}

struct Transport<'a> {
    natural_ty: &'a str,
    cfg: &'a str,
}

impl<'a> ProtocolTemplate<'a> {
    pub fn new(protocol: &'a Protocol, context: &'a Context) -> Self {
        let base_name = protocol.name.decl_name().camel();

        let name = protocol.transport().unwrap_or("Channel");
        let natural_ty = &context.resource_bindings().endpoint(name).natural_path;
        let transport = match name {
            // TODO: We should eventually replace this with feature = "fuchsia"
            "Channel" => Some(Transport { cfg: "target_os = \"fuchsia\"", natural_ty }),
            "Driver" => Some(Transport { cfg: "feature = \"driver\"", natural_ty }),
            _ => None,
        };

        Self {
            protocol,
            context,

            non_canonical_name: protocol.name.decl_name().non_canonical(),
            protocol_name: protocol.name.decl_name().camel(),
            module_name: protocol.name.decl_name().snake(),

            client_name: format!("{base_name}Client"),
            local_client_handler_name: format!("{base_name}LocalClientHandler"),
            send_client_handler_name: format!("{base_name}ClientHandler"),

            server_name: format!("{base_name}Server"),
            local_server_handler_name: format!("{base_name}LocalServerHandler"),
            send_server_handler_name: format!("{base_name}ServerHandler"),

            transport,
        }
    }

    fn get_struct(&self, identifier: &CompoundIdent) -> Option<&Struct> {
        self.library()
            .struct_declarations
            .get(identifier)
            .or_else(|| self.library().external_struct_declarations.get(identifier))
    }

    fn get_union(&self, identifier: &CompoundIdent) -> Option<&Union> {
        self.library().union_declarations.get(identifier)
    }

    fn get_request_args_struct(&self, method: &ProtocolMethod) -> Option<&Struct> {
        match method.kind {
            ProtocolMethodKind::OneWay | ProtocolMethodKind::TwoWay => {
                let args = method.maybe_request_payload.as_ref()?;
                if let TypeKind::Identifier { identifier, .. } = &args.kind {
                    return self.get_struct(identifier);
                }
            }
            ProtocolMethodKind::Event => {
                if !method.has_error {
                    let args = method.maybe_response_payload.as_ref()?;
                    if let TypeKind::Identifier { identifier, .. } = &args.kind {
                        return self.get_struct(identifier);
                    }
                }
            }
        }
        None
    }

    fn get_response_struct(&self, method: &ProtocolMethod) -> Option<&Struct> {
        if let ProtocolMethodKind::TwoWay = method.kind {
            let payload = if method.is_strict {
                method.maybe_response_payload.as_ref()
            } else {
                method.maybe_response_success_type.as_ref()
            };
            if let TypeKind::Identifier { identifier, .. } = &payload?.kind {
                let response_struct = self.get_struct(identifier)?;
                if response_struct.members.len() == 1 {
                    return Some(response_struct);
                }
            }
        }
        None
    }

    fn get_response_success_struct(&self, method: &ProtocolMethod) -> Option<&Struct> {
        if let ProtocolMethodKind::TwoWay = method.kind {
            let args = method.maybe_response_success_type.as_ref()?;
            if let TypeKind::Identifier { identifier, .. } = &args.kind {
                let response_struct = self.get_struct(identifier)?;
                if response_struct.members.len() == 1 {
                    return Some(response_struct);
                }
            }
        }
        None
    }

    fn get_response_union(&self, method: &ProtocolMethod) -> Option<&Union> {
        if let ProtocolMethodKind::TwoWay = method.kind {
            if let TypeKind::Identifier { identifier, .. } =
                &method.maybe_response_payload.as_ref()?.kind
            {
                return self.get_union(identifier);
            }
        }
        None
    }

    fn get_response_success_from_alias(
        &self,
        method: &ProtocolMethod,
    ) -> Option<&PartialTypeConstructor> {
        self.get_response_union(method)?
            .members
            .iter()
            .find(|member| member.ordinal.get() == 1)?
            .from_alias
            .as_ref()
    }

    fn get_response_error_from_alias(
        &self,
        method: &ProtocolMethod,
    ) -> Option<&PartialTypeConstructor> {
        self.get_response_union(method)?
            .members
            .iter()
            .find(|member| member.ordinal.get() == 2)?
            .from_alias
            .as_ref()
    }

    fn get_response_error_struct(&self, method: &ProtocolMethod) -> Option<&Struct> {
        if let ProtocolMethodKind::TwoWay = method.kind {
            let args = method.maybe_response_err_type.as_ref()?;
            if let TypeKind::Identifier { identifier, .. } = &args.kind {
                let response_struct = self.get_struct(identifier)?;
                if response_struct.members.len() == 1 {
                    return Some(response_struct);
                }
            }
        }
        None
    }

    fn discoverable_name(&self) -> Option<String> {
        let attr = self.protocol.attributes.attributes.get("discoverable")?;
        if let Some(name) = attr.args.get("name") {
            Some(name.value.value.clone())
        } else {
            let (library, name) = self.protocol.name.split();
            Some(format!("{}.{}", library, name.camel()))
        }
    }

    fn prelude_method_type_idents(&self) -> BTreeSet<CompoundIdentifier> {
        let mut result = BTreeSet::new();

        fn get_identifier(ty: &Type) -> Option<CompoundIdentifier> {
            if let Type { kind: TypeKind::Identifier { identifier, .. }, .. } = ty {
                Some(identifier.clone())
            } else {
                None
            }
        }

        for method in self.protocol.methods.iter() {
            // We always include the request payload in the prelude if there is one
            if let Some(request) = method.maybe_request_payload.as_deref() {
                result.extend(get_identifier(request));
            }

            if let Some(success) = method.maybe_response_success_type.as_deref() {
                // The response type is a result, so we only want to include the success and error
                // types in the prelude
                result.extend(get_identifier(success));

                if let Some(error) = method.maybe_response_err_type.as_deref() {
                    result.extend(get_identifier(error));
                }
            } else if let Some(response) = method.maybe_response_payload.as_deref() {
                // The response type is not a result, so we want to include the response payload
                // type in the prelude
                result.extend(get_identifier(response));
            }
        }

        result
    }
}

impl Contextual for ProtocolTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}
