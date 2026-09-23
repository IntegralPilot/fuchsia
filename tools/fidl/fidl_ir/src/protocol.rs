// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

use crate::de::Index;

use crate::{Attributes, CompoundIdentifier, Identifier, Type};

#[derive(Clone, Debug, Deserialize)]
pub struct Protocol {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: CompoundIdentifier,
    pub composed_protocols: Vec<ComposedProtocol>,
    pub methods: Vec<ProtocolMethod>,
    pub openness: ProtocolOpenness,
}

impl Protocol {
    pub fn transport(&self) -> Option<&str> {
        self.attributes.get_value("transport")
    }
}

impl Index for Protocol {
    type Key = CompoundIdentifier;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolOpenness {
    Open,
    Ajar,
    Closed,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProtocolMethod {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub has_request: bool,
    pub has_response: bool,
    pub is_composed: bool,
    pub has_error: bool,
    pub kind: ProtocolMethodKind,
    pub maybe_request_payload: Option<Box<Type>>,
    pub maybe_response_payload: Option<Box<Type>>,
    pub maybe_response_success_type: Option<Box<Type>>,
    pub maybe_response_err_type: Option<Box<Type>>,
    pub name: Identifier,
    pub ordinal: u64,
    #[serde(rename = "strict")]
    pub is_strict: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolMethodKind {
    OneWay,
    TwoWay,
    Event,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ComposedProtocol {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: CompoundIdentifier,
}
