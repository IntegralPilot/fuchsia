// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module is concerned with comparing different platform versions. It
//! contains data structure to represent the platform API in ways that are
//! convenient for comparison, and the comparison algorithms.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::{Display, Write as _};

use anyhow::Result;
use flyweights::FlyStr;

use crate::ir::Openness;
use crate::{Configuration, Scope, Version};

mod types;
pub use types::*;
mod handle;
pub use handle::*;
mod path;
pub use path::*;
mod primitive;
pub use primitive::*;
mod problems;
pub use problems::*;

#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod test;
#[cfg(test)]
mod types_tests;

#[derive(PartialEq, PartialOrd, Eq, Ord, Debug, Clone, Copy)]
pub enum Optionality {
    Optional,
    Required,
}

#[derive(PartialEq, PartialOrd, Eq, Ord, Debug, Clone, Copy)]
pub enum Flexibility {
    Strict,
    Flexible,
}
impl Display for Flexibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Flexibility::Strict => f.write_str("strict"),
            Flexibility::Flexible => f.write_str("flexible"),
        }
    }
}

#[derive(PartialEq, PartialOrd, Eq, Ord, Debug, Clone, Copy)]
pub enum Transport {
    Channel,
    Driver,
}

impl Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transport::Channel => f.write_str("Channel"),
            Transport::Driver => f.write_str("Driver"),
        }
    }
}

impl From<&Option<String>> for Transport {
    fn from(value: &Option<String>) -> Self {
        match value {
            Some(transport) => transport.into(),
            None => Self::Channel,
        }
    }
}

impl From<&String> for Transport {
    fn from(value: &String) -> Self {
        match value.as_str() {
            "Channel" => Self::Channel,
            "Driver" => Self::Driver,
            _ => panic!("unknown protocol transport {value}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum MethodPayload {
    TwoWay(Type, Type),
    OneWay(Type),
    Event(Type),
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct Method {
    pub name: FlyStr,
    pub path: Path,
    pub flexibility: Flexibility,
    pub payload: MethodPayload,
}

impl Method {
    pub fn two_way(
        name: impl Into<FlyStr>,
        path: Path,
        flexibility: Flexibility,
        request: Type,
        response: Type,
    ) -> Self {
        Self {
            name: name.into(),
            path,
            flexibility,
            payload: MethodPayload::TwoWay(request, response),
        }
    }
    pub fn one_way(
        name: impl Into<FlyStr>,
        path: Path,
        flexibility: Flexibility,
        request: Type,
    ) -> Self {
        Self { name: name.into(), path, flexibility, payload: MethodPayload::OneWay(request) }
    }
    pub fn event(
        name: impl Into<FlyStr>,
        path: Path,
        flexibility: Flexibility,
        payload: Type,
    ) -> Self {
        Self { name: name.into(), path, flexibility, payload: MethodPayload::Event(payload) }
    }
    fn kind(&self) -> &'static str {
        use MethodPayload::*;
        match self.payload {
            TwoWay(_, _) => "two-way",
            OneWay(_) => "one-way",
            Event(_) => "event",
        }
    }

    fn server_initiated(&self) -> bool {
        matches!(self.payload, MethodPayload::Event(_))
    }

    fn client_initiated(&self) -> bool {
        !self.server_initiated()
    }

    fn is_one_way(&self) -> bool {
        matches!(self.payload, MethodPayload::OneWay(_))
    }

    pub fn compatible(
        client: &Self,
        server: &Self,
        config: &Configuration,
    ) -> CompatibilityProblems {
        let mut problems = CompatibilityProblems::default();
        if client.kind() != server.kind() {
            problems.error(
                [&client.path, &server.path],
                format!(
                    "Interaction kind differs between client(@{}):{} and server(@{}):{}",
                    client.path.api_level(),
                    client.kind(),
                    server.path.api_level(),
                    server.kind()
                ),
            );
            return problems;
        }
        use MethodPayload::*;
        match (&client.payload, &server.payload) {
            (TwoWay(c_request, c_response), TwoWay(s_request, s_response)) => {
                // Request
                let compat = compare_types(c_request, s_request, &config);
                if compat.is_incompatible() {
                    problems.error(
                        [&client.path, &server.path],
                        format!(
                            "Incompatible request types, client(@{}), server(@{})",
                            client.path.api_level(),
                            server.path.api_level(),
                        ),
                    );
                }
                problems.append(compat);
                // Response
                let compat = compare_types(s_response, c_response, &config);
                if compat.is_incompatible() {
                    problems.error(
                        [&client.path, &server.path],
                        format!(
                            "Incompatible response types, client(@{}), server(@{})",
                            client.path.api_level(),
                            server.path.api_level(),
                        ),
                    );
                }
                problems.append(compat);
            }
            (OneWay(c_request), OneWay(s_request)) => {
                let compat = compare_types(c_request, s_request, &config);
                if compat.is_incompatible() {
                    problems.error(
                        [&client.path, &server.path],
                        format!(
                            "Incompatible request types, client(@{}), server(@{})",
                            client.path.api_level(),
                            server.path.api_level(),
                        ),
                    );
                }
                problems.append(compat);
            }
            (Event(c_payload), Event(s_payload)) => {
                let compat = compare_types(s_payload, c_payload, &config);
                if compat.is_incompatible() {
                    problems.error(
                        [&client.path, &server.path],
                        format!(
                            "Incompatible event types, client(@{}), server(@{})",
                            client.path.api_level(),
                            server.path.api_level(),
                        ),
                    );
                }
                problems.append(compat);
            }
            _ => (), // Interaction kind mismatch handled elsewhere.
        }

        problems
    }
}

#[derive(Copy, Clone, Eq, PartialEq, PartialOrd, Ord, Debug)]
pub struct Scopes {
    pub platform: bool,
    pub external: bool,
}

impl Scopes {
    pub fn contains(&self, scope: Scope) -> bool {
        match scope {
            Scope::Platform => self.platform,
            Scope::External => self.external,
        }
    }

    fn everywhere(&self) -> bool {
        self.platform && self.external
    }
}
impl std::fmt::Display for Scopes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Scopes { platform: false, external: false } => Ok(()),
            Scopes { platform: true, external: false } => f.write_str("platform"),
            Scopes { platform: false, external: true } => f.write_str("external"),
            Scopes { platform: true, external: true } => f.write_str("platform,external"),
        }
    }
}

#[derive(Clone, Eq, PartialEq, PartialOrd, Ord, Debug)]
pub struct Discoverable {
    pub name: String,
    pub client: Scopes,
    pub server: Scopes,
}
impl Discoverable {
    fn render_for_message(&self) -> String {
        let mut message = "@discoverable(".to_owned();
        if !self.client.everywhere() {
            write!(&mut message, "client=\"{}\"", self.client).unwrap();
        }
        if !self.client.everywhere() && !self.server.everywhere() {
            message.push_str(", ");
        }
        if !self.server.everywhere() {
            write!(&mut message, "server=\"{}\"", self.server).unwrap();
        }
        message.push(')');
        message
    }
}

#[test]
fn discoverable_render_for_message() {
    assert_eq!(
        "@discoverable()",
        Discoverable {
            name: "".to_owned(),
            client: Scopes { platform: true, external: true },
            server: Scopes { platform: true, external: true }
        }
        .render_for_message()
    );
    assert_eq!(
        "@discoverable(client=\"external\")",
        Discoverable {
            name: "".to_owned(),
            client: Scopes { platform: false, external: true },
            server: Scopes { platform: true, external: true }
        }
        .render_for_message()
    );
    assert_eq!(
        "@discoverable(server=\"platform\")",
        Discoverable {
            name: "".to_owned(),
            client: Scopes { platform: true, external: true },
            server: Scopes { platform: true, external: false }
        }
        .render_for_message()
    );
    assert_eq!(
        "@discoverable(client=\"platform\", server=\"external\")",
        Discoverable {
            name: "".to_owned(),
            client: Scopes { platform: true, external: false },
            server: Scopes { platform: false, external: true }
        }
        .render_for_message()
    );
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct Protocol {
    pub name: FlyStr,
    pub path: Path,
    pub openness: Openness,
    pub discoverable: Option<Discoverable>,
    pub methods: BTreeMap<u64, Method>,
    pub added: String,
}

impl Protocol {
    fn compatible(protocols: [&Self; 2], config: &Configuration) -> CompatibilityProblems {
        let mut problems = CompatibilityProblems::default();

        // Look at discoverable to identify the pairs of client/server interactions
        let mut clients = Vec::new();
        let mut servers = Vec::new();
        for p in protocols {
            if let Some(d) = &p.discoverable {
                let scope = p.path.scope();
                if d.client.contains(scope) {
                    clients.push(p);
                }
                if d.server.contains(scope) {
                    servers.push(p);
                }
            }
        }

        for (client, server) in
            clients.into_iter().flat_map(|c| servers.iter().map(move |s| (c, s)))
        {
            problems.append(Protocol::client_server_compatible(client, server, &config));
        }

        problems
    }

    fn client_server_compatible(
        client: &Self,
        server: &Self,
        config: &Configuration,
    ) -> CompatibilityProblems {
        let mut problems = CompatibilityProblems::default();
        let client_ordinals: BTreeSet<u64> = client.methods.keys().cloned().collect();
        let server_ordinals: BTreeSet<u64> = server.methods.keys().cloned().collect();

        // Is there a contradiction between how this is declared and how it's used?
        let mut scope_contradiction = false;

        if let Some(disco) = &client.discoverable {
            if !disco.client.contains(client.path.scope()) {
                scope_contradiction = true;
                problems.add(
                    !config.fatal_contradictions,
                    [&client.path, &server.path],
                    format!(
                        "Protocol {}(@{}) used as a {} client, contradicting its {}.",
                        client.name,
                        client.path.api_level(),
                        client.path.scope(),
                        disco.render_for_message()
                    ),
                );
            }
        }

        if let Some(disco) = &server.discoverable {
            if !disco.server.contains(server.path.scope()) {
                scope_contradiction = true;
                problems.add(
                    !config.fatal_contradictions,
                    [&client.path, &server.path],
                    format!(
                        "Protocol {}(@{}) used as a {} server, contradicting its {}.",
                        server.name,
                        server.path.api_level(),
                        server.path.scope(),
                        disco.render_for_message()
                    ),
                );
            }
        }

        if scope_contradiction {
            // If we've already found a contradiction, that means that:
            // (a) this is discoverable so we'll be checking it elsewhere
            // (b) any more checks we do are kind of moot.
            return problems;
        }

        // Compare common interactions
        for ordinal in client_ordinals.intersection(&server_ordinals) {
            problems.append(Method::compatible(
                client.methods.get(ordinal).expect("Unexpectedly missing client method"),
                server.methods.get(ordinal).expect("Unexpectedly missing server method"),
                &config,
            ));
        }

        // Only on client.
        for ordinal in client_ordinals.difference(&server_ordinals) {
            let method = client.methods.get(ordinal).expect("Unexpectedly missing client method");
            if method.client_initiated() {
                let client_sets_unknown_bit = method.flexibility == Flexibility::Flexible;
                let server_allows_unknown = server.openness == Openness::Open
                    || method.is_one_way() && server.openness == Openness::Ajar;
                if client_sets_unknown_bit && server_allows_unknown {
                    // The server doesn't know about this method but: (a) the
                    // client sets the "unknown" header bit indicating that it's
                    // fine with the server not recognizing it, and (b) the
                    // server is expecting messages with the "unknown" bit set.
                } else {
                    problems.error(
                        [&method.path, &server.path],
                        format!(
                            "Server(@{}) missing method {}.{}",
                            server.path.api_level(),
                            client.name,
                            method.name
                        ),
                    );
                }
            }
        }

        // Only on server.
        for ordinal in server_ordinals.difference(&client_ordinals) {
            let method = server.methods.get(ordinal).expect("Unexpectedly missing server method");
            if method.server_initiated() {
                if method.flexibility == Flexibility::Flexible
                    && (server.openness == Openness::Open || server.openness == Openness::Ajar)
                {
                    // The server thinks the event is flexible and the client thinks the protocol is open enough.
                } else {
                    problems.error(
                        [&client.path, &method.path],
                        format!(
                            "Client(@{}) missing event {}.{}",
                            client.path.api_level(),
                            server.name,
                            method.name
                        ),
                    );
                }
            }
        }

        problems
    }
}

#[derive(Clone, Debug)]
pub struct AbiSurface {
    #[allow(unused)]
    pub version: Version,
    pub discoverable: HashMap<String, Protocol>,
    pub tear_off: HashMap<String, Protocol>,
}

pub fn compatible(
    surfaces: [&AbiSurface; 2],
    config: &Configuration,
) -> Result<CompatibilityProblems> {
    let mut problems = CompatibilityProblems::default();

    // Discoverable protocols for each ABI surface.
    let disco: [HashSet<String>; 2] = surfaces.map(|s| s.discoverable.keys().cloned().collect());

    // Discoverable protocols shared between both ABI surfaces.
    let common_disco: HashSet<String> = disco[0].intersection(&disco[1]).cloned().collect();

    for p in common_disco {
        problems.append(Protocol::compatible(surfaces.map(|s| &s.discoverable[&p]), &config));
    }

    problems.sort();

    Ok(problems)
}
