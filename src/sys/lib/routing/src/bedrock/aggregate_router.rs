// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use capability_source::{AggregateInstance, CapabilitySource};
use cm_types::Name;
use runtime_capabilities::{DirConnector, Router};
use std::sync::Arc;

/// Functions of this signature are used during sandbox to construct new aggregate routers. These
/// aggregate routers synthesize together one capability from multiple sources.
pub type AggregateRouterFn<C> =
    dyn Fn(Arc<C>, Vec<AggregateSource>, CapabilitySource) -> Arc<Router<DirConnector>>;

/// An `AggregateSource` describes the source of one (or more) service capabilities whose instances
/// will be added to an aggregated service.
#[derive(Debug, Clone)]
pub enum AggregateSource {
    /// A router to a single service capability provider, whose published instances will be part of
    /// an aggregate.
    DirectoryRouter {
        /// Where the router comes from, be it a parent, child, etc.
        source_instance: AggregateInstance,
        /// The router that will back this source to the aggregate.
        router: Arc<Router<DirConnector>>,
    },
    /// A collection whose dynamically created components may contribute to an aggregate.
    Collection { collection_name: Name },
}
