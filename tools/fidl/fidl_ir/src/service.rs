// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

use crate::de::Index;

use crate::{Attributes, CompoundIdentifier, Identifier, Type};

#[derive(Clone, Debug, Deserialize)]
pub struct Service {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: CompoundIdentifier,
    pub members: Vec<ServiceMember>,
}

impl Index for Service {
    type Key = CompoundIdentifier;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServiceMember {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: Identifier,
    #[serde(rename = "type")]
    pub ty: Type,
}
