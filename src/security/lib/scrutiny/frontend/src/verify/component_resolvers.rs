// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use capability_source::CapabilitySource;
use cm_fidl_analyzer::component_instance::ComponentInstanceForAnalyzer;
use cm_fidl_analyzer::{BreadthFirstModelWalker, ComponentInstanceVisitor, ComponentModelWalker};
use cm_rust::UseDecl;
use futures::FutureExt;
use moniker::ExtendedMoniker;
use routing::bedrock::request_metadata::resolver_metadata;
use routing::component_instance::ComponentInstanceInterface;
use runtime_capabilities::Capability;
use scrutiny_collection::model::DataModel;
use scrutiny_collection::v2_component_model::V2ComponentModel;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// ComponentResolversController
///
/// A DataController which returns a list of absolute monikers of all
/// components that, in their environment, contain a resolver with the
///  given moniker for a scheme with access to a protocol.
#[derive(Default)]
pub struct ComponentResolversController {}

/// The expected query format.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct ComponentResolverRequest {
    /// `resolver` URI scheme of interest
    pub scheme: String,
    /// Absolute moniker of the `resolver`
    pub moniker: String,
    /// Filter the results to components resolved with a `resolver` with access to a protocol
    pub protocol: String,
}

/// The response schema.
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct ComponentResolverResponse {
    /// Files accessed to perform this query, for depfile generation.
    pub deps: HashSet<PathBuf>,
    /// Component monikers that matched the query.
    pub monikers: Vec<String>,
}

/// Walks the tree for the absolute monikers of all components that,
/// in their environment, contain a resolver with the given moniker
/// for a scheme with access to a protocol.  `monikers` contains the
/// components which match the `request` parameters.
struct ComponentResolversVisitor {
    request: ComponentResolverRequest,
    monikers: Vec<String>,
}

impl ComponentResolversVisitor {
    fn new(request: ComponentResolverRequest) -> Self {
        let monikers = Vec::new();
        Self { request, monikers }
    }

    fn get_monikers(&self) -> Vec<String> {
        self.monikers.clone()
    }

    fn check_instance(&mut self, instance: &Arc<ComponentInstanceForAnalyzer>) -> Result<()> {
        let scheme_name = cm_types::Name::new(&self.request.scheme).expect("invalid scheme");
        if let Some(Capability::ConnectorRouter(resolver_router)) = instance
            .component_sandbox()
            .now_or_never()
            .expect("now or never did not return a result")
            .map_err(|e| anyhow!("failed to get sandbox of component: {e:?}"))?
            .component_input
            .environment()
            .resolvers()
            .get(&scheme_name)
        {
            let request = resolver_metadata(cm_types::Availability::Required);

            let source: CapabilitySource = match resolver_router
                .route_debug(request, instance.as_weak().into())
                .now_or_never()
                .expect("now or never did not return a result")
            {
                Ok(source) => source,
                Err(e) => {
                    eprintln!(
                        "Ignoring invalid resolver configuration for {}: {:#}",
                        instance.moniker(),
                        anyhow!(e).context("failed to route to a resolver")
                    );
                    return Ok(());
                }
            };
            let resolver_source_moniker = match source.source_moniker() {
                ExtendedMoniker::ComponentInstance(moniker) => moniker,
                ExtendedMoniker::ComponentManager => {
                    return Err(anyhow!(
                        "The plugin is unable to verify resolvers declared above the root."
                    ));
                }
            };
            let resolver_source = instance
                .find_absolute(&resolver_source_moniker)
                .now_or_never()
                .expect("now or never did not return a result")
                .expect("failed to walk to other component instance");
            let moniker = moniker::Moniker::parse_str(&self.request.moniker)?;

            if resolver_source.moniker() == &moniker {
                for use_decl in &resolver_source.decl_for_testing().uses {
                    if let UseDecl::Protocol(name) = use_decl {
                        if name.source_name == self.request.protocol {
                            self.monikers.push(instance.moniker().to_string());
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

impl ComponentInstanceVisitor for ComponentResolversVisitor {
    fn visit_instance(&mut self, instance: &Arc<ComponentInstanceForAnalyzer>) -> Result<()> {
        self.check_instance(instance)
            .with_context(|| format!("while visiting {}", instance.moniker()))
    }
}

impl ComponentResolversController {
    pub fn get_monikers(
        model: Arc<DataModel>,
        request: ComponentResolverRequest,
    ) -> Result<ComponentResolverResponse> {
        let tree_data = model
            .get::<V2ComponentModel>()
            .context("Failed to get V2ComponentModel from ComponentResolversController model")?;
        let deps = tree_data.deps.clone();

        let model = &tree_data.component_model;

        let mut walker = BreadthFirstModelWalker::new();
        let mut visitor = ComponentResolversVisitor::new(request);

        walker.walk(&model, &mut visitor).context(
            "Failed to walk V2ComponentModel with BreadthFirstWalker and ComponentResolversVisitor",
        )?;

        Ok(ComponentResolverResponse { monikers: visitor.get_monikers(), deps })
    }
}
