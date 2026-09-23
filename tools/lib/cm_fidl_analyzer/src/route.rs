// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::component_model::AnalyzerModelError;
use capability_source::CapabilitySource;
use cm_rust::offer::OfferDeclCommon;
use cm_rust::{Availability, CapabilityTypeName, ExposeDeclCommon, SourceName, UseDeclCommon};
use cm_types::Name;
use fidl_fuchsia_component_runtime::RouteRequest;
use moniker::Moniker;
use routing::bedrock::request_metadata::{
    config_metadata, dictionary_metadata, directory_metadata, event_stream_metadata,
    protocol_metadata, resolver_metadata, runner_metadata, service_metadata, storage_metadata,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, PartialEq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetDecl {
    Use(cm_rust::UseDecl),
    Offer(cm_rust::offer::OfferDecl),
    Expose(cm_rust::ExposeDecl),
    ResolverFromEnvironment(String),
}

impl TargetDecl {
    pub fn source_name(&self) -> Option<Name> {
        match self {
            Self::Use(u) => Some(u.source_name().clone()),
            Self::Offer(o) => Some(o.source_name().clone()),
            Self::Expose(e) => Some(e.source_name().clone()),
            Self::ResolverFromEnvironment(name) => {
                Some(Name::new(name).expect("invalid resolver name"))
            }
        }
    }

    pub fn to_route_request(&self) -> RouteRequest {
        let (type_name, availability) = match self {
            Self::Use(u) => (CapabilityTypeName::from(u), *u.availability()),
            Self::Offer(o) => (CapabilityTypeName::from(o), *o.availability()),
            Self::Expose(e) => (CapabilityTypeName::from(e), *e.availability()),
            Self::ResolverFromEnvironment(_) => {
                (CapabilityTypeName::Resolver, Availability::Required)
            }
        };
        match type_name {
            CapabilityTypeName::Directory => directory_metadata(availability, None, None),
            CapabilityTypeName::EventStream => {
                event_stream_metadata(availability, Default::default())
            }
            CapabilityTypeName::Protocol => protocol_metadata(availability),
            CapabilityTypeName::Resolver => resolver_metadata(availability),
            CapabilityTypeName::Runner => runner_metadata(availability),
            CapabilityTypeName::Service => service_metadata(availability),
            CapabilityTypeName::Storage => storage_metadata(availability),
            CapabilityTypeName::Dictionary => dictionary_metadata(availability),
            CapabilityTypeName::Config => config_metadata(availability),
        }
    }
}

/// A summary of a specific capability route and the outcome of verification.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifyRouteResult {
    /// TODO(https://fxbug.dev/42053778): Rename to `moniker`.
    pub using_node: Moniker,
    pub target_decl: TargetDecl,
    pub capability: Option<Name>,
    pub error: Option<AnalyzerModelError>,
    pub source: Option<CapabilitySource>,
}
