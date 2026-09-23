// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::rights::Rights;
use crate::subdir::SubDir;
use cm_rust::{CapabilityTypeName, EventScope, NativeIntoFidl};
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_io as fio;
use moniker::Moniker;

/// Returns a `RouteRequest` specifying a Protocol build type.
pub fn protocol_metadata(availability: cm_types::Availability) -> RouteRequest {
    RouteRequest {
        build_type_name: Some(CapabilityTypeName::Protocol.to_string()),
        availability: Some(availability.native_into_fidl()),
        ..Default::default()
    }
}

/// Returns a `RouteRequest` specifying a Dictionary build type.
pub fn dictionary_metadata(availability: cm_types::Availability) -> RouteRequest {
    RouteRequest {
        build_type_name: Some(CapabilityTypeName::Dictionary.to_string()),
        availability: Some(availability.native_into_fidl()),
        ..Default::default()
    }
}

/// Returns a `RouteRequest` specifying a Directory build type.
pub fn directory_metadata(
    availability: cm_types::Availability,
    rights: Option<Rights>,
    subdir: Option<SubDir>,
) -> RouteRequest {
    RouteRequest {
        build_type_name: Some(CapabilityTypeName::Directory.to_string()),
        availability: Some(availability.native_into_fidl()),
        sub_directory_path: subdir.map(|sd| sd.as_ref().clone().native_into_fidl()),
        directory_rights: rights.map(Into::into),
        inherit_rights: Some(rights.is_none()),
        ..Default::default()
    }
}

/// Returns a `RouteRequest` specifying a Config build type.
pub fn config_metadata(availability: cm_types::Availability) -> RouteRequest {
    RouteRequest {
        build_type_name: Some(CapabilityTypeName::Config.to_string()),
        availability: Some(availability.native_into_fidl()),
        ..Default::default()
    }
}

/// Returns a `RouteRequest` specifying a Runner build type.
pub fn runner_metadata(availability: cm_types::Availability) -> RouteRequest {
    RouteRequest {
        build_type_name: Some(CapabilityTypeName::Runner.to_string()),
        availability: Some(availability.native_into_fidl()),
        ..Default::default()
    }
}

/// Returns a `RouteRequest` specifying a Resolver build type.
pub fn resolver_metadata(availability: cm_types::Availability) -> RouteRequest {
    RouteRequest {
        build_type_name: Some(CapabilityTypeName::Resolver.to_string()),
        availability: Some(availability.native_into_fidl()),
        ..Default::default()
    }
}

/// Returns a `RouteRequest` specifying a Service build type.
pub fn service_metadata(availability: cm_types::Availability) -> RouteRequest {
    RouteRequest {
        build_type_name: Some(CapabilityTypeName::Service.to_string()),
        availability: Some(availability.native_into_fidl()),
        // Service capabilities are implemented as DirConnectors. When the Router<DirConnector>
        // that connects to a component's outgoing directory wants to assemble a DirConnector, it
        // pulls the set of rights that are allowed for that DirConnector from the route metadata.
        // This gives us two choices: maintain a different Router<DirConnector> exclusively for
        // connecting service capabilities to an outgoing directory that hard-codes R_STAR_DIR, or
        // set R_STAR_DIR in the routing metadata and let the existing Router<DirConnector> use
        // that information.
        //
        // It's less code duplication to do the latter, so we set the necessary bits to carry
        // rights information in the routing metadata for service capability routing.
        directory_rights: Some(fio::PERM_READABLE),
        inherit_rights: Some(true),
        ..Default::default()
    }
}

/// Returns a `RouteRequest` specifying an EventStream build type.
pub fn event_stream_metadata(
    availability: cm_types::Availability,
    route_metadata: Option<(Moniker, Box<[EventScope]>)>,
) -> RouteRequest {
    let (scope_moniker, scope) = match route_metadata {
        Some((moniker, scope)) => (Some(moniker), Some(scope)),
        None => (None, None),
    };
    RouteRequest {
        build_type_name: Some(CapabilityTypeName::EventStream.to_string()),
        availability: Some(availability.native_into_fidl()),
        event_stream_scope_moniker: scope_moniker.map(NativeIntoFidl::native_into_fidl),
        event_stream_scope: scope.map(NativeIntoFidl::native_into_fidl),
        ..Default::default()
    }
}

/// Returns a `RouteRequest` specifying a Storage build type.
pub fn storage_metadata(availability: cm_types::Availability) -> RouteRequest {
    RouteRequest {
        build_type_name: Some(CapabilityTypeName::Storage.to_string()),
        availability: Some(availability.native_into_fidl()),
        directory_rights: Some(fio::PERM_READABLE | fio::PERM_WRITABLE),
        inherit_rights: Some(false),
        ..Default::default()
    }
}
