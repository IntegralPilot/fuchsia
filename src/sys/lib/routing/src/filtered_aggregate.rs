// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::RoutingError;
use capability_source::CapabilitySource;
use cm_rust::NameMapping;
use futures::future::BoxFuture;
use std::fmt;

/// The return value of the routing future returned by
/// `FilteredAggregateCapabilityProvider::route_instances`, which contains information about the
/// source of the route.
#[derive(Debug)]
pub struct FilteredAggregateCapabilityRouteData {
    /// The source of the capability.
    pub capability_source: CapabilitySource,
    /// The filter to apply to service instances, as defined by
    /// [`fuchsia.component.decl/OfferService.renamed_instances`](https://fuchsia.dev/reference/fidl/fuchsia.component.decl#OfferService).
    pub instance_filter: Vec<NameMapping>,
}

/// A provider of a capability from an aggregation of zero or more offered instances of a
/// capability, with filters.
///
/// This trait type-erases the capability type, so it can be handled and hosted generically.
pub trait FilteredAggregateCapabilityProvider: Send + Sync {
    /// Return a list of futures to route every instance in the aggregate to its source. Each
    /// result is paired with the list of instances to include in the source.
    fn route_instances(
        &self,
    ) -> Vec<BoxFuture<'_, Result<FilteredAggregateCapabilityRouteData, RoutingError>>>;

    /// Trait-object compatible clone.
    fn clone_boxed(&self) -> Box<dyn FilteredAggregateCapabilityProvider>;
}

impl Clone for Box<dyn FilteredAggregateCapabilityProvider> {
    fn clone(&self) -> Self {
        self.clone_boxed()
    }
}

impl fmt::Debug for Box<dyn FilteredAggregateCapabilityProvider> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Box<dyn FilteredAggregateCapabilityProvider>").finish()
    }
}
