// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::intermediate_router::RouteRequest;
use cm_rust::offer::{OfferDecl, OfferDeclCommon};
use cm_rust::{
    CapabilityTypeName, DebugRegistration, ExposeDecl, ExposeDeclCommon, ResolverRegistration,
    RunnerRegistration, UseDecl, UseDeclCommon,
};
use cm_types::RelativePath;
use fidl_fuchsia_io as fio;
use moniker::Moniker;

pub trait ToRequest {
    fn to_request(&self, moniker: &Moniker) -> RouteRequest;
}

impl ToRequest for OfferDecl {
    fn to_request(&self, moniker: &Moniker) -> RouteRequest {
        let mut request = RouteRequest {
            build_type_name: CapabilityTypeName::from(self),
            availability: Some(self.availability().clone()),
            ..Default::default()
        };
        match self {
            cm_rust::OfferDecl::Service(_) => {
                request.directory_rights = Some(fio::PERM_READABLE);
                request.inherit_rights = Some(true);
            }
            cm_rust::OfferDecl::Directory(o) => {
                request.directory_rights = o.rights.map(|rights| {
                    fio::Flags::from_bits(rights.bits())
                        .expect("operations and flags are bit compatible")
                });
                request.sub_directory_path = Some(o.subdir.clone());
                request.inherit_rights = Some(true);
            }
            cm_rust::OfferDecl::Storage(_) => {
                request.directory_rights = Some(fio::PERM_READABLE | fio::PERM_WRITABLE);
                request.sub_directory_path = Some(RelativePath::dot());
                request.inherit_rights = Some(false);
            }
            cm_rust::OfferDecl::EventStream(o) => {
                if let Some(scope) = &o.scope {
                    request.event_stream_scope_moniker = Some(moniker.clone());
                    request.event_stream_scope = Some(scope.clone());
                }
            }
            cm_rust::OfferDecl::Config(_)
            | cm_rust::OfferDecl::Runner(_)
            | cm_rust::OfferDecl::Resolver(_)
            | cm_rust::OfferDecl::Dictionary(_)
            | cm_rust::OfferDecl::Protocol(_) => (),
        }
        request
    }
}

impl ToRequest for UseDecl {
    fn to_request(&self, moniker: &Moniker) -> RouteRequest {
        let mut request = RouteRequest {
            build_type_name: CapabilityTypeName::from(self),
            availability: Some(self.availability().clone()),
            ..Default::default()
        };
        match self {
            cm_rust::UseDecl::Service(_) => {
                request.directory_rights = Some(fio::PERM_READABLE);
                request.inherit_rights = Some(false);
            }
            cm_rust::UseDecl::Directory(u) => {
                request.directory_rights = Some(
                    fio::Flags::from_bits(u.rights.bits())
                        .expect("operations and flags are bit compatible"),
                );
                request.sub_directory_path = Some(u.subdir.clone());
                request.inherit_rights = Some(false);
            }
            cm_rust::UseDecl::Storage(_) => {
                request.directory_rights = Some(fio::PERM_READABLE | fio::PERM_WRITABLE);
                request.sub_directory_path = Some(RelativePath::dot());
                request.inherit_rights = Some(false);
            }
            cm_rust::UseDecl::EventStream(u) => {
                if let Some(scope) = &u.scope {
                    request.event_stream_scope = Some(scope.clone());
                    request.event_stream_scope_moniker = Some(moniker.clone());
                }
            }
            cm_rust::UseDecl::Config(_)
            | cm_rust::UseDecl::Runner(_)
            | cm_rust::UseDecl::Dictionary(_)
            | cm_rust::UseDecl::Protocol(_) => (),
        }
        request
    }
}

impl ToRequest for ExposeDecl {
    fn to_request(&self, _moniker: &Moniker) -> RouteRequest {
        let mut request = RouteRequest {
            build_type_name: CapabilityTypeName::from(self),
            availability: Some(self.availability().clone()),
            ..Default::default()
        };
        match self {
            cm_rust::ExposeDecl::Service(_) => {
                request.directory_rights = Some(fio::PERM_READABLE);
                request.inherit_rights = Some(true);
            }
            cm_rust::ExposeDecl::Directory(o) => {
                request.directory_rights = o.rights.map(|rights| {
                    fio::Flags::from_bits(rights.bits())
                        .expect("operations and flags are bit compatible")
                });
                request.sub_directory_path = Some(o.subdir.clone());
                request.inherit_rights = Some(true);
            }
            cm_rust::ExposeDecl::Config(_)
            | cm_rust::ExposeDecl::Runner(_)
            | cm_rust::ExposeDecl::Resolver(_)
            | cm_rust::ExposeDecl::Dictionary(_)
            | cm_rust::ExposeDecl::Protocol(_) => (),
        }
        request
    }
}

impl ToRequest for DebugRegistration {
    fn to_request(&self, _moniker: &Moniker) -> RouteRequest {
        RouteRequest {
            build_type_name: CapabilityTypeName::Protocol,
            availability: Some(cm_rust::Availability::Required),
            ..Default::default()
        }
    }
}

impl ToRequest for RunnerRegistration {
    fn to_request(&self, _moniker: &Moniker) -> RouteRequest {
        RouteRequest {
            build_type_name: CapabilityTypeName::Runner,
            availability: Some(cm_rust::Availability::Required),
            ..Default::default()
        }
    }
}

impl ToRequest for ResolverRegistration {
    fn to_request(&self, _moniker: &Moniker) -> RouteRequest {
        RouteRequest {
            build_type_name: CapabilityTypeName::Resolver,
            availability: Some(cm_rust::Availability::Required),
            ..Default::default()
        }
    }
}
