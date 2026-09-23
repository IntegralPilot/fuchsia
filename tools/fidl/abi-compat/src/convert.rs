// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module implements the conversion between the IR representation and the
//! comparison representation.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use anyhow::{anyhow, bail, Context as _, Result};
use flyweights::FlyStr;

use crate::ir::Declaration;
use crate::{compare, ir, Version};

pub enum Context<'a> {
    Root {
        ir: Rc<ir::IR>,
        version: Version,
    },
    Child {
        parent: &'a Context<'a>,
        depth: usize,
        ir: &'a Rc<ir::IR>,
        identifier: Option<&'a str>,
        member_name: Option<&'a str>,
    },
}

impl<'a> Context<'a> {
    pub fn new(ir: Rc<ir::IR>, version: &Version) -> Self {
        Context::Root { ir, version: version.clone() }
    }
    pub fn nest_member(&'a self, member_name: &'a str, identifier: Option<&'a str>) -> Self {
        Context::Child {
            parent: self,
            depth: self.depth() + 1,
            ir: self.ir(),
            identifier,
            member_name: Some(member_name),
        }
    }

    pub fn nest_list(&'a self, identifier: Option<&'a str>) -> Self {
        Context::Child {
            parent: self,
            depth: self.depth() + 1,
            ir: self.ir(),
            identifier,
            member_name: None,
        }
    }

    fn depth(&self) -> usize {
        match self {
            Context::Root { .. } => 0,
            Context::Child { depth, .. } => *depth,
        }
    }

    fn ir(&self) -> &Rc<ir::IR> {
        match self {
            Context::Root { ir, .. } => ir,
            Context::Child { ir, .. } => ir,
        }
    }

    pub fn get(&self, name: &str) -> Result<&ir::Declaration> {
        self.ir().get(name)
    }

    /// Look for the supplied identifier in the identifier stack and if found return the length of the cycle to the last one.
    fn find_identifier_cycle(&self, name: &str) -> Option<usize> {
        // Skip the last one...
        let mut ctx = match self {
            Context::Root { .. } => return None,
            Context::Child { parent, .. } => parent,
        };
        for i in 0.. {
            if let Context::Child { parent, identifier, .. } = ctx {
                if identifier == &Some(name) {
                    return Some(i);
                }
                ctx = parent;
            } else {
                return None;
            }
        }
        None
    }

    fn path(&self) -> compare::Path {
        let (version, string) = self.mk_path(0);
        compare::Path::new(version, string)
    }

    fn mk_path(&self, reserve_length: usize) -> (Version, String) {
        match self {
            Context::Root { version, .. } => {
                (version.clone(), String::with_capacity(reserve_length))
            }
            Context::Child { parent, member_name, .. } => {
                // Walk up the chain calculating the space required
                let (version, mut s) = parent.mk_path(
                    reserve_length
                        + match member_name {
                            Some(name) => name.len() + 1,
                            None => 2,
                        },
                );
                // Fill up the string as we walk back down the chain.
                if let Some(name) = member_name {
                    if !s.is_empty() {
                        s.push_str(".");
                    }
                    s.push_str(name)
                } else {
                    s.push_str("[]")
                }
                (version, s)
            }
        }
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;

    fn context_for_test() -> Context<'static> {
        Context::Root { ir: ir::IR::empty_for_tests(), version: Version::new("0") }
    }

    #[test]
    fn test_find_identifier_cycle() {
        // Set up identifier stack

        fn foo(context: &Context<'_>) {
            bar(&context.nest_member("A", Some("foo")));
        }
        fn bar(context: &Context<'_>) {
            baz(&context.nest_member("B", Some("bar")));
        }
        fn baz(context: &Context<'_>) {
            foo2(&context.nest_member("C", Some("baz")));
        }
        fn foo2(context: &Context<'_>) {
            quux(&context.nest_member("D", Some("foo")));
        }
        fn quux(context: &Context<'_>) {
            let context = context.nest_member("E", Some("quux"));
            // Check that it's as expected
            assert_eq!(None, context.find_identifier_cycle("blah"));
            assert_eq!(None, context.find_identifier_cycle("quux"));
            assert_eq!(Some(0), context.find_identifier_cycle("foo"));
            assert_eq!(Some(1), context.find_identifier_cycle("baz"));
            assert_eq!(Some(2), context.find_identifier_cycle("bar"));
        }

        foo(&context_for_test())
    }
}

pub trait ConvertType {
    fn identifier(&self) -> Option<&str>;
    fn convert<'a>(&self, context: &Context<'a>) -> Result<compare::Type>;
}

impl ConvertType for ir::Type {
    fn identifier(&self) -> Option<&str> {
        match self {
            ir::Type::Endpoint { protocol, .. } => Some(protocol),
            ir::Type::Identifier { identifier, .. } => Some(identifier),
            _ => None,
        }
    }

    fn convert<'a>(&self, context: &Context<'a>) -> Result<compare::Type> {
        Ok(match self {
            ir::Type::Array { element_count, element_type } => {
                let element_type =
                    Box::new(element_type.convert(&context.nest_list(element_type.identifier()))?);
                compare::Type::Array(context.path(), *element_count, element_type)
            }
            ir::Type::StringArray { element_count } => {
                compare::Type::StringArray(context.path(), *element_count)
            }
            ir::Type::Vector { element_type, maybe_element_count, nullable } => {
                let element_type =
                    Box::new(element_type.convert(&context.nest_list(element_type.identifier()))?);
                compare::Type::Vector(
                    context.path(),
                    maybe_element_count.unwrap_or(0xFFFF),
                    element_type,
                    convert_nullable(nullable),
                )
            }
            ir::Type::String { maybe_element_count, nullable } => compare::Type::String(
                context.path(),
                maybe_element_count.unwrap_or(0xFFFF),
                convert_nullable(nullable),
            ),
            ir::Type::Handle { nullable, subtype, rights } => compare::Type::Handle(
                context.path(),
                convert_handle_type(subtype)?,
                convert_nullable(nullable),
                crate::compare::HandleRights::from_bits(*rights)
                    .ok_or_else(|| anyhow!("invalid handle rights bits 0x{:x}", *rights))?,
            ),
            ir::Type::Endpoint { role, protocol_transport, protocol, nullable } => {
                let decl = context.get(protocol)?;
                let ir::Declaration::Protocol(protocol_decl) = decl else {
                    panic!("endpoint had a protocol that wasn't a protocol: {decl:?}")
                };

                match role {
                    ir::EndpointRole::Client => compare::Type::ClientEnd(
                        context.path(),
                        protocol_decl.name.clone(),
                        protocol_transport.into(),
                        convert_nullable(nullable),
                        Box::new(convert_protocol(&protocol_decl, &context)?),
                    ),

                    ir::EndpointRole::Server => compare::Type::ServerEnd(
                        context.path(),
                        protocol_decl.name.clone(),
                        protocol_transport.into(),
                        convert_nullable(nullable),
                        Box::new(convert_protocol(&protocol_decl, &context)?),
                    ),
                }
            }
            ir::Type::Identifier { identifier, nullable } => {
                if let Some(cycle) = context.find_identifier_cycle(identifier) {
                    compare::Type::Cycle(context.path(), identifier.clone(), cycle + 1)
                } else {
                    let decl = context.get(identifier)?;
                    let path = context.path();
                    let t = match decl {
                        ir::Declaration::Bits(decl) => decl.convert(&context)?,
                        ir::Declaration::Enum(decl) => decl.convert(&context)?,
                        ir::Declaration::Struct(decl) => decl.convert(&context)?,
                        ir::Declaration::Table(decl) => decl.convert(&context)?,
                        ir::Declaration::Union(decl) => decl.convert(&context)?,
                        ir::Declaration::Protocol(_) => {
                            panic!("Identifiers cannot refer to protocols")
                        }
                    };

                    if *nullable {
                        compare::Type::Box(path, Box::new(t))
                    } else {
                        t
                    }
                }
            }

            ir::Type::Internal { subtype } => match subtype.as_str() {
                "framework_error" => compare::Type::FrameworkError(context.path()),
                _ => bail!("Unimplemented internal type: {subtype:?}"),
            },
            ir::Type::Primitive { subtype } => compare::Type::Primitive(
                context.path(),
                convert_primitive_subtype(subtype.as_str())?,
            ),
        })
    }
}

impl TryInto<compare::Primitive> for ir::Type {
    type Error = anyhow::Error;

    fn try_into(self) -> std::result::Result<compare::Primitive, Self::Error> {
        match self {
            ir::Type::Primitive { subtype } => convert_primitive_subtype(subtype.as_str()),
            _ => bail!("Expected primitive, got {:?}", self),
        }
    }
}

impl ConvertType for ir::BitsDeclaration {
    fn identifier(&self) -> Option<&str> {
        Some(&self.name)
    }
    fn convert<'a>(&self, context: &Context<'a>) -> Result<compare::Type> {
        let mut members = BTreeMap::new();
        for m in &self.members {
            members.insert(m.value.integer_value()?, FlyStr::new(&m.name));
        }
        let t = self.r#type.clone().try_into()?;
        Ok(compare::Type::Bits(
            context.path(),
            convert_strict(self.strict),
            t,
            members.keys().cloned().collect(),
            members,
        ))
    }
}

impl ConvertType for ir::EnumDeclaration {
    fn identifier(&self) -> Option<&str> {
        Some(&self.name)
    }
    fn convert<'a>(&self, context: &Context<'a>) -> Result<compare::Type> {
        let mut members = BTreeMap::new();
        for m in &self.members {
            members.insert(m.value.integer_value()?, FlyStr::new(&m.name));
        }
        let t = convert_primitive_subtype(self.r#type.as_str())?;
        Ok(compare::Type::Enum(
            context.path(),
            convert_strict(self.strict),
            t,
            members.keys().cloned().collect(),
            members,
        ))
    }
}

impl ConvertType for ir::TableDeclaration {
    fn identifier(&self) -> Option<&str> {
        Some(&self.name)
    }
    fn convert<'a>(&self, context: &Context<'a>) -> Result<compare::Type> {
        let mut members = BTreeMap::new();
        for m in &self.members {
            let t = &m.r#type;
            members.insert(m.ordinal, t.convert(&context.nest_member(&m.name, t.identifier()))?);
        }
        Ok(compare::Type::Table(context.path(), members))
    }
}

impl ConvertType for ir::StructDeclaration {
    fn identifier(&self) -> Option<&str> {
        Some(&self.name)
    }
    fn convert<'a>(&self, context: &Context<'a>) -> Result<compare::Type> {
        let members = self
            .members
            .iter()
            .map(|m| m.r#type.convert(&context.nest_member(&m.name, m.r#type.identifier())))
            .collect::<Result<_>>()?;
        Ok(compare::Type::Struct(context.path(), members))
    }
}

impl ConvertType for ir::UnionDeclaration {
    fn identifier(&self) -> Option<&str> {
        Some(&self.name)
    }
    fn convert<'a>(&self, context: &Context<'a>) -> Result<compare::Type> {
        let mut members = BTreeMap::new();
        for m in &self.members {
            let t = &m.r#type;
            members.insert(m.ordinal, t.convert(&context.nest_member(&m.name, t.identifier()))?);
        }
        Ok(compare::Type::Union(context.path(), convert_strict(self.strict), members))
    }
}

impl ConvertType for ir::Declaration {
    fn identifier(&self) -> Option<&str> {
        match self {
            ir::Declaration::Bits(decl) => decl.identifier(),
            ir::Declaration::Enum(decl) => decl.identifier(),
            ir::Declaration::Protocol(decl) => Some(&decl.name),
            ir::Declaration::Struct(decl) => decl.identifier(),
            ir::Declaration::Table(decl) => decl.identifier(),
            ir::Declaration::Union(decl) => decl.identifier(),
        }
    }

    fn convert<'a>(&self, context: &Context<'a>) -> Result<compare::Type> {
        match self {
            ir::Declaration::Bits(decl) => decl.convert(&context),
            ir::Declaration::Enum(decl) => decl.convert(&context),
            ir::Declaration::Struct(decl) => decl.convert(&context),
            ir::Declaration::Table(decl) => decl.convert(&context),
            ir::Declaration::Union(decl) => decl.convert(&context),
            ir::Declaration::Protocol(decl) => panic!(
                "Protocols are not type declarations and thus can't be converted to types: {:?}",
                decl
            ),
        }
    }
}

fn convert_nullable(nullable: &bool) -> compare::Optionality {
    use compare::Optionality::*;
    if *nullable {
        Optional
    } else {
        Required
    }
}

fn convert_strict(strict: bool) -> compare::Flexibility {
    use compare::Flexibility::*;
    if strict {
        Strict
    } else {
        Flexible
    }
}

fn convert_primitive_subtype(subtype: &str) -> Result<compare::Primitive> {
    use compare::Primitive::*;
    Ok(match subtype {
        "bool" => Bool,
        "int8" => Int8,
        "uint8" => Uint8,
        "int16" => Int16,
        "uint16" => Uint16,
        "int32" => Int32,
        "uint32" => Uint32,
        "int64" => Int64,
        "uint64" => Uint64,
        "float32" => Float32,
        "float64" => Float64,
        _ => bail!("Unsupported primitive subtype: {}", subtype),
    })
}

fn convert_handle_type(subtype: impl AsRef<str>) -> Result<Option<compare::HandleType>> {
    use compare::HandleType::*;
    let subtype = subtype.as_ref();
    Ok(match subtype {
        "" => None,
        // TODO: actually convert
        _ => Some(Channel),
    })
}

/// Convert an Option<ir::Type> to either the appropriate compare type, or an empty struct.
fn maybe_convert_type<'a>(
    maybe_type: &Option<ir::Type>,
    context: Context<'a>,
) -> Result<compare::Type> {
    match maybe_type {
        Some(t) => Ok(t.convert(&context)?),
        None => Ok(compare::Type::Struct(context.path(), vec![])),
    }
}

fn maybe_type_identifier(maybe_type: &Option<ir::Type>) -> Option<&str> {
    if let Some(t) = maybe_type {
        t.identifier()
    } else {
        None
    }
}

fn convert_method<'a>(
    method: &ir::ProtocolMethod,
    context: &Context<'a>,
) -> Result<compare::Method> {
    let context = context.nest_member(&method.name, None);
    let flexibility = convert_strict(method.strict);
    let path = context.path();
    Ok(match (method.has_request, method.has_response) {
        (true, true) => compare::Method::two_way(
            &method.name,
            path,
            flexibility,
            maybe_convert_type(
                &method.maybe_request_payload,
                context
                    .nest_member("REQUEST", maybe_type_identifier(&method.maybe_request_payload)),
            )?,
            maybe_convert_type(
                &method.maybe_response_payload,
                context
                    .nest_member("RESPONSE", maybe_type_identifier(&method.maybe_response_payload)),
            )?,
        ),
        (true, false) => compare::Method::one_way(
            &method.name,
            path,
            flexibility,
            maybe_convert_type(
                &method.maybe_request_payload,
                context
                    .nest_member("REQUEST", maybe_type_identifier(&method.maybe_request_payload)),
            )?,
        ),
        (false, true) => compare::Method::event(
            &method.name,
            path,
            flexibility,
            maybe_convert_type(
                &method.maybe_response_payload,
                context
                    .nest_member("PAYLOAD", maybe_type_identifier(&method.maybe_response_payload)),
            )?,
        ),
        (false, false) => panic!("Invalid IR"),
    })
}

pub fn convert_protocol<'a>(
    p: &ir::ProtocolDeclaration,
    context: &Context<'a>,
) -> Result<compare::Protocol> {
    let mut methods = BTreeMap::new();

    for pm in &p.methods {
        methods.insert(
            pm.ordinal,
            convert_method(pm, &context)
                .with_context(|| format!("Method {}.{}", p.name, pm.name))?,
        );
    }

    let added = ir::get_attribute(&p.maybe_attributes, "available")
        .expect("All protocols should have \"available\" attributes by this point")
        .get_argument("added")
        .expect(
            "All protocol available attributes should have an \"added\" argument by this point.",
        )
        .value()
        .to_string();

    let discoverable = ir::get_attribute(&p.maybe_attributes, "discoverable").map(|discoverable| {
        let attr_or = |name: &str, default: &str| {
            discoverable
                .get_argument(name)
                .map(|c| c.value().to_string())
                .unwrap_or_else(|| default.to_string())
        };
        let scopes = |name: &str| {
            let locs: Vec<String> = attr_or(name, "platform,external")
                .split(",")
                .into_iter()
                .map(|l| l.trim().into())
                .collect();
            compare::Scopes {
                platform: locs.contains(&"platform".to_string()),
                external: locs.contains(&"external".to_string()),
            }
        };
        compare::Discoverable {
            name: attr_or("name", &p.name),
            client: scopes("client"),
            server: scopes("server"),
        }
    });

    Ok(compare::Protocol {
        name: FlyStr::new(&p.name),
        path: context.path(),
        openness: p.openness,
        methods,
        discoverable,
        added,
    })
}

pub fn convert_abi_surface(ir: Rc<ir::IR>) -> Result<compare::AbiSurface> {
    let levels = ir.available.get("fuchsia").context("missing API level for 'fuchsia'")?;
    let version = Version::from_api_levels(levels.as_slice());
    let context = Context::new(ir.clone(), &version);
    let mut abi_surface =
        compare::AbiSurface { version, discoverable: HashMap::new(), tear_off: HashMap::new() };

    for decl in ir.declarations.values() {
        if let Declaration::Protocol(decl) = decl {
            let protocol =
                convert_protocol(decl, &context.nest_member(&decl.name, Some(&decl.name)))?;

            if let Some(discoverable) = protocol.discoverable.clone() {
                abi_surface.discoverable.insert(discoverable.name, protocol);
            } else {
                abi_surface.tear_off.insert(decl.name.clone(), protocol);
            };
        }
    }

    Ok(abi_surface)
}
