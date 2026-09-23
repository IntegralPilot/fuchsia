// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::component_instance::{ComponentInstanceForAnalyzer, TopInstanceForAnalyzer};
use crate::route::{TargetDecl, VerifyRouteResult};
use crate::{PkgUrlMatch, match_absolute_component_urls};
use anyhow::{Context, Result, anyhow};
use capability_source::{
    CapabilitySource, CapabilityToCapabilitySource, ComponentCapability, ComponentSource,
};
use cm_config::RuntimeConfig;
use cm_rust::{
    Availability, CapabilityTypeName, ComponentDecl, ExposeDecl, OfferDecl, OfferDeclCommon,
    OfferTarget, ProgramDecl, SourceName, UseDecl, UseDeclCommon, UseRunnerDecl, UseSource,
};
use cm_types::{IterablePath, Name, Url};
use config_encoder::ConfigFields;
use fidl::prelude::*;
use fidl_fuchsia_sys2 as fsys;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use futures::FutureExt;
use moniker::{ChildName, ExtendedMoniker, Moniker};
use router_error::{Explain, RouterError};
use routing::component_instance::{ComponentInstanceInterface, ExtendedInstanceInterface};
use routing::error::{ComponentInstanceError, RoutingError};
use routing::policy::GlobalPolicyChecker;
use routing::{
    SandboxPath, debug_route_sandbox_path, debug_route_sandbox_path_with_request,
    debug_route_storage_backing_directory,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use thiserror::Error;
use zx_status;

/// Errors that may occur when building a `ComponentModelForAnalyzer` from
/// a set of component manifests.
#[derive(Clone, Debug, Deserialize, Error, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildAnalyzerModelError {
    #[error("no component declaration found for url `{0}` requested by node `{1}`")]
    ComponentDeclNotFound(String, String),

    #[error("invalid child declaration containing url `{0}` at node `{1}`")]
    InvalidChildDecl(String, String),

    #[error("no node found with path `{0}`")]
    ComponentNodeNotFound(String),

    #[error("environment `{0}` requested by child `{1}` not found at node `{2}`")]
    EnvironmentNotFound(String, String, String),

    #[error("multiple resolvers found for scheme `{0}`")]
    DuplicateResolverScheme(String),

    #[error("malformed url {0} for component instance {1}")]
    MalformedUrl(String, String),

    #[error("dynamic component with url {0} an invalid moniker")]
    DynamicComponentInvalidMoniker(String),

    #[error("dynamic component at {0} with url {1} is not part of a collection")]
    DynamicComponentWithoutCollection(String, String),
}

/// Errors that a `ComponentModelForAnalyzer` may detect in the component graph.
#[derive(Clone, Debug, Error, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AnalyzerModelError {
    #[error("the source instance `{0}` is not executable")]
    SourceInstanceNotExecutable(Moniker),

    #[error("at component {0} the capability `{1}` is not a valid source for the capability `{2}`")]
    InvalidSourceCapability(ExtendedMoniker, String, String),

    #[error("no resolver found in environment of component `{0}` for scheme `{1}`")]
    MissingResolverForScheme(Moniker, String),

    #[error(transparent)]
    ComponentInstanceError(#[from] ComponentInstanceError),

    #[error(transparent)]
    RoutingError(#[from] RoutingError),
}

impl AnalyzerModelError {
    pub fn as_zx_status(&self) -> zx_status::Status {
        match self {
            Self::SourceInstanceNotExecutable(_) => zx_status::Status::NOT_FOUND,
            Self::InvalidSourceCapability(_, _, _) => zx_status::Status::NOT_FOUND,
            Self::MissingResolverForScheme(_, _) => zx_status::Status::NOT_FOUND,
            Self::ComponentInstanceError(err) => err.as_zx_status(),
            Self::RoutingError(err) => err.as_zx_status(),
        }
    }
}

impl From<AnalyzerModelError> for ExtendedMoniker {
    fn from(err: AnalyzerModelError) -> ExtendedMoniker {
        match err {
            AnalyzerModelError::InvalidSourceCapability(moniker, _, _) => moniker,
            AnalyzerModelError::MissingResolverForScheme(moniker, _)
            | AnalyzerModelError::SourceInstanceNotExecutable(moniker) => moniker.into(),
            AnalyzerModelError::ComponentInstanceError(err) => err.into(),
            AnalyzerModelError::RoutingError(err) => err.into(),
        }
    }
}

/// Builds a `ComponentModelForAnalyzer` from a set of component manifests.
pub struct ModelBuilderForAnalyzer {
    default_root_url: Url,
}

/// The type returned by `ModelBuilderForAnalyzer::build()`. May contain some
/// errors even if `model` is `Some`.
pub struct BuildModelResult {
    pub model: Option<Arc<ComponentModelForAnalyzer>>,
    pub errors: Vec<anyhow::Error>,
}

impl BuildModelResult {
    fn new() -> Self {
        Self { model: None, errors: Vec::new() }
    }
}

#[derive(Default)]
pub struct DynamicConfig {
    pub components: HashMap<Moniker, (AbsoluteComponentUrl, Option<Name>)>,
    pub dictionaries: DynamicDictionaryConfig,
}

pub type DynamicDictionaryConfig = HashMap<Moniker, HashMap<Name, Vec<(CapabilityTypeName, Name)>>>;

impl ModelBuilderForAnalyzer {
    pub fn new(default_root_url: Url) -> Self {
        Self { default_root_url }
    }

    fn load_dynamic_components(
        input: HashMap<Moniker, (AbsoluteComponentUrl, Option<Name>)>,
    ) -> (HashMap<Moniker, Vec<Child>>, Vec<anyhow::Error>) {
        let mut errors: Vec<anyhow::Error> = vec![];
        let mut dynamic_components: HashMap<Moniker, Vec<Child>> = HashMap::new();
        for (moniker, (url, environment)) in input.into_iter() {
            let Some((parent_moniker, child_moniker)) = moniker.split_leaf() else {
                errors.push(
                    BuildAnalyzerModelError::DynamicComponentInvalidMoniker(url.to_string()).into(),
                );
                continue;
            };
            if child_moniker.collection().is_none() {
                errors.push(
                    BuildAnalyzerModelError::DynamicComponentWithoutCollection(
                        moniker.to_string(),
                        url.to_string(),
                    )
                    .into(),
                );
                continue;
            }

            let children = dynamic_components.entry(parent_moniker).or_insert_with(|| vec![]);
            match Url::new(&url.to_string()) {
                Ok(url) => {
                    children.push(Child { child_moniker: child_moniker.into(), url, environment });
                }
                Err(_) => {
                    errors.push(
                        BuildAnalyzerModelError::MalformedUrl(url.to_string(), moniker.to_string())
                            .into(),
                    );
                }
            }
        }
        (dynamic_components, errors)
    }

    pub fn build(
        self,
        decls_by_url: HashMap<Url, (ComponentDecl, Option<ConfigFields>)>,
        runtime_config: Arc<RuntimeConfig>,
        component_id_index: Arc<component_id_index::Index>,
    ) -> BuildModelResult {
        self.build_with_dynamic_config(
            DynamicConfig::default(),
            decls_by_url,
            runtime_config,
            component_id_index,
        )
    }

    pub fn build_with_dynamic_config(
        self,
        dynamic_config: DynamicConfig,
        decls_by_url: HashMap<Url, (ComponentDecl, Option<ConfigFields>)>,
        runtime_config: Arc<RuntimeConfig>,
        component_id_index: Arc<component_id_index::Index>,
    ) -> BuildModelResult {
        let mut result = BuildModelResult::new();

        let (dynamic_components, mut dynamic_component_errors) =
            Self::load_dynamic_components(dynamic_config.components);
        result.errors.append(&mut dynamic_component_errors);

        let dynamic_dictionaries = Arc::new(dynamic_config.dictionaries);

        // Initialize the model with an empty `instances` map.
        let mut model = ComponentModelForAnalyzer {
            top_instance: TopInstanceForAnalyzer::new(
                runtime_config.namespace_capabilities.clone(),
                runtime_config.builtin_capabilities.clone(),
            ),
            instances: HashMap::new(),
            policy_checker: GlobalPolicyChecker::new(runtime_config.security_policy.clone()),
            component_id_index,
        };

        let root_url = runtime_config.root_component_url.as_ref().unwrap_or(&self.default_root_url);

        // If `root_url` matches a `ComponentDecl` in `decls_by_url`, construct the root
        // instance and then recursively add child instances to the model.
        match Self::get_decl_by_url(&decls_by_url, root_url) {
            Err(err) => {
                result.errors.push(err.context("Failed to parse root URL as fuchsia package URL"));
            }
            Ok(None) => {
                result.errors.push(anyhow!("Failed to locate root component with URL: {root_url}"));
            }
            Ok(Some((root_decl, root_config))) => {
                let root_instance = ComponentInstanceForAnalyzer::new_root(
                    root_decl.clone(),
                    root_config.clone(),
                    root_url.clone(),
                    Arc::clone(&model.top_instance),
                    Arc::clone(&runtime_config),
                    model.policy_checker.clone(),
                    Arc::clone(&model.component_id_index),
                    Arc::clone(&dynamic_dictionaries),
                );

                Self::add_descendants(
                    &root_instance,
                    &decls_by_url,
                    &dynamic_components,
                    &dynamic_dictionaries,
                    &mut model,
                    &mut result,
                );

                model.instances.insert(root_instance.moniker().clone(), root_instance);

                result.model = Some(Arc::new(model));
            }
        }

        result
    }

    // Adds all descendants of `instance` to `model`, also inserting each new instance
    // in the `children` map of its parent, including children denoted in
    // `dynamic_components`.
    fn add_descendants(
        instance: &Arc<ComponentInstanceForAnalyzer>,
        decls_by_url: &HashMap<Url, (ComponentDecl, Option<ConfigFields>)>,
        dynamic_components: &HashMap<Moniker, Vec<Child>>,
        dynamic_dictionaries: &Arc<DynamicDictionaryConfig>,
        model: &mut ComponentModelForAnalyzer,
        result: &mut BuildModelResult,
    ) {
        let mut children = vec![];
        for child_decl in instance.decl.children.iter() {
            let child_moniker = ChildName::new(child_decl.name.clone(), None);
            match Self::get_absolute_child_url(&child_decl.url, instance) {
                Ok(url) => {
                    children.push(Child {
                        child_moniker,
                        url,
                        environment: child_decl.environment.clone(),
                    });
                }
                Err(err) => {
                    result.errors.push(anyhow!(err));
                }
            }
        }
        if let Some(dynamic_children) = dynamic_components.get(instance.moniker()) {
            children.append(
                &mut dynamic_children.iter().map(|dynamic_child| dynamic_child.clone()).collect(),
            );
        }

        for child in children.iter() {
            if child.child_moniker.name().is_empty() {
                result.errors.push(anyhow!(BuildAnalyzerModelError::InvalidChildDecl(
                    child.url.to_string(),
                    instance.moniker().to_string(),
                )));
                continue;
            }

            match Self::get_decl_by_url(decls_by_url, &child.url)
                .context("Failed to parse absolute child URL")
            {
                Err(err) => {
                    result.errors.push(err);
                }
                Ok(Some((child_component_decl, child_config))) => {
                    match ComponentInstanceForAnalyzer::new_for_child(
                        child,
                        child_component_decl.clone(),
                        child_config.clone(),
                        Arc::clone(instance),
                        model.policy_checker.clone(),
                        Arc::clone(&model.component_id_index),
                        Arc::clone(&dynamic_dictionaries),
                    ) {
                        Ok(child_instance) => {
                            Self::add_descendants(
                                &child_instance,
                                decls_by_url,
                                dynamic_components,
                                dynamic_dictionaries,
                                model,
                                result,
                            );

                            instance.add_child(
                                child.child_moniker.clone(),
                                Arc::clone(&child_instance),
                            );

                            model
                                .instances
                                .insert(child_instance.moniker().clone(), child_instance);
                        }
                        Err(err) => {
                            result.errors.push(anyhow!(err));
                        }
                    }
                }
                Ok(None) => {
                    result.errors.push(anyhow!(BuildAnalyzerModelError::ComponentDeclNotFound(
                        child.url.to_string(),
                        instance.moniker().to_string(),
                    )));
                }
            }
        }
    }

    // Given a component instance and the url `child_url` of a child of that instance,
    // returns an absolute url for the child.
    fn get_absolute_child_url(
        child_url: &Url,
        instance: &Arc<ComponentInstanceForAnalyzer>,
    ) -> Result<Url, BuildAnalyzerModelError> {
        let child_url = child_url.as_str();
        let err = BuildAnalyzerModelError::MalformedUrl(
            instance.url().to_string(),
            instance.moniker().to_string(),
        );

        let url = match url::Url::parse(child_url) {
            Ok(u) => u,
            Err(url::ParseError::RelativeUrlWithoutBase) => {
                let absolute_prefix = match instance.url().is_relative() {
                    true => find_first_absolute_ancestor_url(instance).map_err(|_| err.clone())?,
                    false => instance.url().clone(),
                };
                let absolute_prefix =
                    url::Url::parse(absolute_prefix.as_str()).map_err(|_| err.clone())?;
                absolute_prefix
                    .join(child_url)
                    .expect("failed to join child URL to absolute prefix")
            }
            _ => return Err(err),
        };
        Url::new(url.as_str()).map_err(|_| err.clone())
    }

    fn get_decl_by_url<'a>(
        decls_by_url: &'a HashMap<Url, (ComponentDecl, Option<ConfigFields>)>,
        url: &Url,
    ) -> Result<Option<&'a (ComponentDecl, Option<ConfigFields>)>> {
        // Non-`fuchsia-pkg` URLs are not matched with nuance: they must precisely match an entry
        // in `decls_by_url`.
        if url.scheme().expect("all urls are absolute") != "fuchsia-pkg" {
            return Ok(decls_by_url.get(url));
        }

        let fuchsia_component_url = AbsoluteComponentUrl::parse(url.as_str())
            .context("Failed to parse component fuchsia-pkg URL as absolute package URL")?;

        // Gather both strong and weak URL matches against `fuchsia_component_url`.
        let decl_url_matches = decls_by_url
            .keys()
            .filter_map(|decl_url| {
                if decl_url.scheme().expect("all urls are absolute") != "fuchsia-pkg" {
                    None
                } else if let Ok(decl_fuchsia_pkg_url) =
                    AbsoluteComponentUrl::parse(decl_url.as_str())
                {
                    match match_absolute_component_urls(
                        &decl_fuchsia_pkg_url,
                        &fuchsia_component_url,
                    ) {
                        PkgUrlMatch::NoMatch => None,
                        pkg_url_match => Some((decl_url, pkg_url_match)),
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<(&Url, PkgUrlMatch)>>();

        // Return best match. Emit warning or error when multiple matches are found.
        if decl_url_matches.len() == 0 {
            return Ok(None);
        } else if decl_url_matches.len() == 1 {
            if decl_url_matches[0].1 == PkgUrlMatch::WeakMatch {
                log::warn!("Weak component URL match: {} matches {}", url, decl_url_matches[0].0);
            }
            return Ok(decls_by_url.get(decl_url_matches[0].0));
        } else {
            let strong_decl_url_matches = decl_url_matches
                .iter()
                .filter_map(|(url, url_match)| match url_match {
                    PkgUrlMatch::StrongMatch => Some(*url),
                    _ => None,
                })
                .collect::<Vec<&Url>>();

            if strong_decl_url_matches.len() == 0 {
                log::warn!(
                    "Multiple weak component URL matches for {}; matching to first: {}",
                    url,
                    decl_url_matches[0].0
                );
                return Ok(decls_by_url.get(decl_url_matches[0].0));
            } else {
                if strong_decl_url_matches.len() > 1 {
                    log::error!(
                        "Multiple strong package URL matches for {}; matching to first: {}",
                        url,
                        strong_decl_url_matches[0]
                    );
                }
                return Ok(decls_by_url.get(strong_decl_url_matches[0]));
            }
        }
    }
}

fn find_first_absolute_ancestor_url(
    component: &Arc<ComponentInstanceForAnalyzer>,
) -> Result<Url, ComponentInstanceError> {
    let mut parent = component.try_get_parent()?;
    loop {
        match parent {
            ExtendedInstanceInterface::Component(parent_component) => {
                if !parent_component.url().is_relative() {
                    return Ok(parent_component.url().clone());
                }
                parent = parent_component.try_get_parent()?;
            }
            ExtendedInstanceInterface::AboveRoot(_) => {
                return Err(ComponentInstanceError::NoAbsoluteUrl {
                    url: component.url().to_string(),
                    moniker: component.moniker().clone(),
                });
            }
        }
    }
}

/// `ComponentModelForAnalyzer` owns a representation of the v2 component graph and
/// supports lookup of component instances by `Moniker`.
#[derive(Debug, Default, Clone)]
pub struct ComponentModelForAnalyzer {
    top_instance: Arc<TopInstanceForAnalyzer>,
    instances: HashMap<Moniker, Arc<ComponentInstanceForAnalyzer>>,
    policy_checker: GlobalPolicyChecker,
    component_id_index: Arc<component_id_index::Index>,
}

impl ComponentModelForAnalyzer {
    /// Returns the number of component instances in the model, not counting the top instance.
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn get_root_instance(
        self: &Arc<Self>,
    ) -> Result<Arc<ComponentInstanceForAnalyzer>, ComponentInstanceError> {
        self.get_instance(&Moniker::root())
    }

    /// Returns the component instance corresponding to `id` if it is present in the model, or an
    /// `InstanceNotFound` error if not.
    pub fn get_instance(
        self: &Arc<Self>,
        moniker: &Moniker,
    ) -> Result<Arc<ComponentInstanceForAnalyzer>, ComponentInstanceError> {
        match self.instances.get(moniker) {
            Some(instance) => Ok(Arc::clone(instance)),
            None => Err(ComponentInstanceError::instance_not_found(moniker.clone())),
        }
    }

    fn does_child_reference_offer(self: &Arc<Self>, offer: &OfferDecl, child: Moniker) -> bool {
        let instance = if let Ok(i) = self.get_instance(&child.into()) {
            i
        } else {
            // We couldn't find the instance that references this offer.
            return false;
        };

        // Look for a use from parent
        for use_ in &instance.decl.uses {
            if use_.source_name() == offer.target_name() {
                match use_.source() {
                    cm_rust::UseSource::Parent => return true,
                    _ => {}
                }
            }
        }

        // Look for a next offer from parent
        for next_offer in &instance.decl.offers {
            if next_offer.source_name() == offer.target_name() {
                match next_offer.source() {
                    cm_rust::offer::OfferSource::Parent => return true,
                    _ => {}
                }
            }
        }

        return false;
    }

    /// For this offer decl, if the offer target does not reference the capability in its manifest,
    /// attempt to route it and report any errors.
    ///
    /// In other words, this will only verify offer decls that terminate the route chain.
    async fn try_check_offer_capability(
        self: &Arc<Self>,
        offer_decl: &OfferDecl,
        target: &Arc<ComponentInstanceForAnalyzer>,
    ) -> Vec<VerifyRouteResult> {
        let target_moniker = target.moniker();

        let offer_target = offer_decl.target();
        let should_check_offer = match offer_target {
            OfferTarget::Child(c) => {
                let child = ChildName::parse(&c.name).unwrap();
                let offer_target_moniker = target_moniker.child(child);

                // This offer should be checked if there is no reference to it in the child.
                !self.does_child_reference_offer(offer_decl, offer_target_moniker)
            }
            OfferTarget::Collection(_) => {
                // Offering to a collection should always cause an offer check.
                true
            }
            OfferTarget::Capability(_) => {
                // Offering to a dictionary (aggregation) should always cause an offer check.
                true
            }
        };

        if should_check_offer {
            self.check_offer_capability(offer_decl, target).await
        } else {
            // This offer decl doesn't need to be checked.
            vec![]
        }
    }

    /// Performs a debug route on the router at `sandbox_path` in the component's sandbox. Policy
    /// checks will be disabled when `skip_policy_check` is true, which is necessary when routing
    /// things from non-terminal points in the route (for example, an offer). A `VerifyRouteResult`
    /// is constructed from the route results and `target_decl`.
    ///
    /// If the route results indicate that the routed capability is a storage capability, then a
    /// second route is performed to find the source of the storage capability's backing directory,
    /// and a second `VerifyRouteResult` is created and returned along with the first.
    ///
    /// It is safe to assume that only one `VerifyRouteResult` will be returned if `target_decl` is
    /// not for a storage capability.
    async fn route_sandbox_path(
        self: &Arc<Self>,
        target: &Arc<ComponentInstanceForAnalyzer>,
        sandbox_path: impl Into<SandboxPath>,
        target_decl: TargetDecl,
        skip_policy_check: bool,
    ) -> Vec<VerifyRouteResult> {
        let mut results = vec![];
        let process_route_result = |res: Result<CapabilitySource, RoutingError>| {
            let self_clone = self.clone();
            let target_decl = target_decl.clone();
            async move {
                let source_check = match res.clone() {
                    Ok(source) => self_clone.check_use_source(&source, target).await,
                    Err(e) => Err(e.into()),
                };
                VerifyRouteResult {
                    using_node: target.moniker().clone(),
                    capability: target_decl.source_name(),
                    target_decl,
                    error: source_check.err(),
                    source: res.clone().ok(),
                }
            }
        };
        let mut request = target_decl.to_route_request();
        request.skip_policy_checks = Some(skip_policy_check);
        let res = debug_route_sandbox_path_with_request(target, sandbox_path, request)
            .now_or_never()
            .expect("future was not ready immediately");
        results.push(process_route_result(res.clone()).await);

        if let Ok(source) = res {
            if let CapabilitySource::Component(ComponentSource {
                capability: ComponentCapability::Storage(storage_decl),
                moniker,
                ..
            }) = &source
            {
                if let Ok(storage_component) = target.find_absolute(moniker).await {
                    let res = debug_route_storage_backing_directory(
                        &storage_component,
                        storage_decl.clone(),
                    )
                    .now_or_never()
                    .expect("future was not ready immediately");
                    results.push(process_route_result(res).await);
                }
            }
        }
        results
    }

    pub async fn check_offer_capability(
        self: &Arc<Self>,
        offer_decl: &OfferDecl,
        target: &Arc<ComponentInstanceForAnalyzer>,
    ) -> Vec<VerifyRouteResult> {
        let results = self
            .route_sandbox_path(target, offer_decl, TargetDecl::Offer(offer_decl.clone()), true)
            .await;
        // Ignore any valid routes to void.
        results
            .into_iter()
            .filter(|r| !matches!(r.source, Some(CapabilitySource::Void(_))))
            .collect()
    }

    /// Checks the routing for all capabilities of the specified types that are `used` by `target`.
    pub async fn check_routes_for_instance(
        self: &Arc<Self>,
        target: &Arc<ComponentInstanceForAnalyzer>,
        capability_types: &HashSet<CapabilityTypeName>,
    ) -> HashMap<CapabilityTypeName, Vec<VerifyRouteResult>> {
        let mut results = HashMap::new();
        for capability_type in capability_types.iter() {
            results.insert(capability_type.clone(), vec![]);
        }

        for use_decl in target.decl.uses.iter().filter(|&u| capability_types.contains(&u.into())) {
            let type_results = results
                .get_mut(&CapabilityTypeName::from(use_decl))
                .expect("expected results for capability type");
            for result in self.check_use_capability(use_decl, &target).await {
                type_results.push(result);
            }
        }

        for expose_decl in
            target.decl.exposes.iter().filter(|&e| capability_types.contains(&e.into()))
        {
            let type_results = results
                .get_mut(&CapabilityTypeName::from(expose_decl))
                .expect("expected results for capability type");
            if let Some(result) = self.check_use_exposed_capability(expose_decl, &target).await {
                type_results.push(result);
            }
        }

        for offer_decl in
            target.decl.offers.iter().filter(|&o| capability_types.contains(&o.into()))
        {
            let type_results = results
                .get_mut(&CapabilityTypeName::from(offer_decl))
                .expect("expected results for capability type");
            for result in self.try_check_offer_capability(offer_decl, &target).await {
                type_results.push(result);
            }
        }

        if capability_types.contains(&CapabilityTypeName::Runner) {
            if let Some(ref program) = target.decl.program {
                let type_results = results
                    .get_mut(&CapabilityTypeName::Runner)
                    .expect("expected results for capability type");
                if let Some(result) = self.check_program_runner(program, &target) {
                    type_results.push(result);
                }
            }
        }

        if capability_types.contains(&CapabilityTypeName::Resolver) {
            let type_results = results
                .get_mut(&CapabilityTypeName::Resolver)
                .expect("expected results for capability type");
            type_results.push(self.check_resolver(&target).await);
        }

        results
    }

    pub async fn check_used_path(
        path: &impl IterablePath,
        target: &Arc<ComponentInstanceForAnalyzer>,
    ) -> Result<CapabilitySource, RouterError> {
        let sandbox_path = SandboxPath::used_path(path);
        let res = debug_route_sandbox_path(target, sandbox_path)
            .now_or_never()
            .expect("future was not ready immediately");
        res.map_err(Into::into)
    }

    /// Given a `UseDecl` for a capability at an instance `target`, first routes the capability
    /// to its source and then validates the source.
    ///
    /// This returns a vector of route results because some capabilities (storage) cause
    /// multiple route verifications (route storage + backing directory) and both results
    /// are relevant.
    pub async fn check_use_capability(
        self: &Arc<Self>,
        use_decl: &UseDecl,
        target: &Arc<ComponentInstanceForAnalyzer>,
    ) -> Vec<VerifyRouteResult> {
        let results = self
            .route_sandbox_path(target, use_decl, TargetDecl::Use(use_decl.clone()), false)
            .await;
        // Ignore any valid routes to void.
        results
            .into_iter()
            .filter(|r| !matches!(r.source, Some(CapabilitySource::Void(_))))
            .collect()
    }

    /// Given a `ExposeDecl` for a capability at an instance `target`, checks whether the capability
    /// can be used from an expose declaration. If so, routes the capability to its source and then
    /// validates the source.
    pub async fn check_use_exposed_capability(
        self: &Arc<Self>,
        expose_decl: &ExposeDecl,
        target: &Arc<ComponentInstanceForAnalyzer>,
    ) -> Option<VerifyRouteResult> {
        let mut res = self
            .route_sandbox_path(target, expose_decl, TargetDecl::Expose(expose_decl.clone()), false)
            .await;
        res.pop()
    }

    /// Given a `ProgramDecl` for a component instance, checks whether the specified runner has
    /// a valid capability route.
    pub fn check_program_runner(
        self: &Arc<Self>,
        program_decl: &ProgramDecl,
        target: &Arc<ComponentInstanceForAnalyzer>,
    ) -> Option<VerifyRouteResult> {
        match program_decl.runner {
            Some(ref runner) => {
                let use_runner = UseRunnerDecl {
                    source: UseSource::Environment,
                    source_name: runner.clone(),
                    source_dictionary: Default::default(),
                };
                self.route_sandbox_path(
                    target,
                    &UseDecl::Runner(use_runner.clone()),
                    TargetDecl::Use(use_runner.into()),
                    false,
                )
                .now_or_never()
                .expect("future was not ready immediately")
                .pop()
            }
            None => None,
        }
    }

    /// Given a component instance, extracts the URL scheme for that instance and looks for a
    /// resolver for that scheme in the instance's environment, recording an error if none
    /// is found. If a resolver is found, checks that it has a valid capability route.
    pub async fn check_resolver(
        self: &Arc<Self>,
        target: &Arc<ComponentInstanceForAnalyzer>,
    ) -> VerifyRouteResult {
        let scheme = target.url().scheme().expect("all urls are absolute");
        let sandbox_path = SandboxPath::resolver(&scheme);
        let mut res = self
            .route_sandbox_path(
                target,
                sandbox_path,
                TargetDecl::ResolverFromEnvironment(scheme.clone()),
                false,
            )
            .await
            .pop()
            .expect("no route results when checking resolver");
        if let Some(AnalyzerModelError::RoutingError(RoutingError::BedrockNotPresentInDictionary {
            name,
            moniker,
        })) = &res.error
            && name.starts_with("component_input/environment/resolvers/")
            && moniker == &target.moniker().clone().into()
        {
            res.error = Some(AnalyzerModelError::MissingResolverForScheme(
                target.moniker().clone(),
                scheme,
            ));
        }
        res
    }

    // Checks properties of a capability source that are necessary to use the capability
    // and that are possible to verify statically.
    async fn check_use_source(
        &self,
        source: &CapabilitySource,
        target: &Arc<ComponentInstanceForAnalyzer>,
    ) -> Result<(), AnalyzerModelError> {
        match &source {
            CapabilitySource::Component(ComponentSource { moniker, .. }) => {
                let source_component = target.find_absolute(&moniker).await?;
                self.check_executable(&source_component)
            }
            CapabilitySource::Namespace(_) => Ok(()),
            CapabilitySource::Capability(CapabilityToCapabilitySource {
                source_capability,
                moniker: _,
            }) => self.check_capability_source(&source_capability, source.source_moniker()),
            CapabilitySource::Builtin(_) => Ok(()),
            CapabilitySource::Framework(_) => Ok(()),
            CapabilitySource::Void(_) => Ok(()),
            _ => unimplemented![],
        }
    }

    // A helper function validating a source of type `Capability`.
    // The only capability which may have a source of another capability is the `StorageAdmin`
    // protocol. We confirm that the source is a storage capability.
    fn check_capability_source(
        &self,
        source_capability: &ComponentCapability,
        source_moniker: ExtendedMoniker,
    ) -> Result<(), AnalyzerModelError> {
        match source_capability {
            ComponentCapability::Storage(_) | ComponentCapability::Directory(_) => Ok(()),
            _ => Err(AnalyzerModelError::InvalidSourceCapability(
                source_moniker,
                format!("{:?}", source_capability.source_name()),
                fsys::StorageAdminMarker::PROTOCOL_NAME.to_string(),
            )),
        }
    }

    // A helper function checking whether a component instance is executable.
    fn check_executable(
        &self,
        component: &Arc<ComponentInstanceForAnalyzer>,
    ) -> Result<(), AnalyzerModelError> {
        match component.decl.program {
            Some(_) => Ok(()),
            None => {
                Err(AnalyzerModelError::SourceInstanceNotExecutable(component.moniker().clone()))
            }
        }
    }

    pub fn collect_config_by_url(&self) -> anyhow::Result<BTreeMap<String, ConfigFields>> {
        let mut configs = BTreeMap::new();
        for instance in self.instances.values() {
            let mut fields = match instance.config_fields() {
                Some(f) => f.clone(),
                None => {
                    let Some(ref config_decl) = instance.decl.config else {
                        continue;
                    };
                    ConfigFields { fields: Vec::new(), checksum: config_decl.checksum.clone() }
                }
            };

            for use_ in instance.decl.uses.iter() {
                let cm_rust::UseDecl::Config(config) = use_ else {
                    continue;
                };
                let value = match debug_route_sandbox_path(&instance, use_)
                    .now_or_never()
                    .expect("future was not ready immediately")
                {
                    Ok(source) => source_to_value(&config.default, source)?,
                    Err(e)
                        if config.availability == Availability::Transitional
                            && e.as_zx_status() == zx_status::Status::NOT_FOUND =>
                    {
                        config.default.clone()
                    }
                    Err(e) => return Err(e.into()),
                };
                let Some(value) = value else {
                    continue;
                };

                let new_field = config_encoder::ConfigField {
                    key: config.target_name.clone().into(),
                    value,
                    mutability: Default::default(),
                };

                let mut needs_key = true;
                for field in &mut fields.fields {
                    if field.key != new_field.key {
                        continue;
                    }
                    field.value = new_field.value.clone();
                    needs_key = false;
                }
                if needs_key {
                    fields.fields.push(new_field);
                }
            }

            configs.insert(instance.url().to_string(), fields.clone());
        }
        Ok(configs)
    }
}

fn source_to_value(
    default: &Option<cm_rust::ConfigValue>,
    source: CapabilitySource,
) -> Result<Option<cm_rust::ConfigValue>, RoutingError> {
    let moniker = source.source_moniker();
    let cap = match source {
        CapabilitySource::Void(_) => {
            return Ok(default.clone());
        }
        CapabilitySource::Capability(CapabilityToCapabilitySource {
            source_capability, ..
        }) => source_capability,
        CapabilitySource::Component(ComponentSource { capability, .. }) => capability,
        o => {
            let type_name =
                o.type_name().map(|t| t.to_string()).unwrap_or_else(|| "<unknown>".to_string());
            return Err(RoutingError::unsupported_route_source(moniker, type_name));
        }
    };

    let cap = match cap {
        ComponentCapability::Config(c) => c,
        c => {
            return Err(RoutingError::unsupported_capability_type(moniker, c.type_name()));
        }
    };
    Ok(Some(cap.value))
}

#[derive(Clone, Debug)]
pub struct Child {
    pub child_moniker: ChildName,
    pub url: Url,
    pub environment: Option<Name>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use assert_matches::assert_matches;
    use cm_config::RuntimeConfig;
    use cm_rust::{ComponentDecl, RegistrationSource, ResolverRegistration, RunnerRegistration};
    use cm_rust_testing::{
        CapabilityBuilder, ChildBuilder, ComponentDeclBuilder, EnvironmentBuilder, UseBuilder,
    };
    use cm_types::{Name, RelativePath, Url};
    use config_encoder::ConfigFields;
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_component_internal as component_internal;
    use maplit::hashmap;
    use moniker::{ChildName, ExtendedMoniker, Moniker};
    use routing::bedrock::request_metadata::{resolver_metadata, runner_metadata};
    use routing::component_instance::{ComponentInstanceInterface, ExtendedInstanceInterface};
    use routing::error::ComponentInstanceError;
    use runtime_capabilities::Capability;
    use std::collections::HashMap;
    use std::sync::Arc;

    const TEST_URL_PREFIX: &str = "test:///";
    const BOOT_SCHEME: &str = "fuchsia-boot";

    fn make_test_url(component_name: &str) -> Url {
        Url::new(&format!("{}{}", TEST_URL_PREFIX, component_name)).unwrap()
    }

    fn make_decl_map(
        components: Vec<(&'static str, ComponentDecl)>,
    ) -> HashMap<Url, (ComponentDecl, Option<ConfigFields>)> {
        HashMap::from_iter(
            components.into_iter().map(|(name, decl)| (make_test_url(name), (decl, None))),
        )
    }

    // Builds a model with structure `root -- child`, retrieves each of the 2 resulting component
    // instances, and tests their public methods.
    #[fuchsia::test]
    fn build_model() -> Result<()> {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("child").build()),
            ("child", ComponentDeclBuilder::new().build()),
        ];

        let config = Arc::new(RuntimeConfig::default());
        let url = make_test_url("root");
        let build_model_result = ModelBuilderForAnalyzer::new(url).build(
            make_decl_map(components),
            config,
            Arc::new(component_id_index::Index::default()),
        );
        assert_eq!(build_model_result.errors.len(), 0);
        assert!(build_model_result.model.is_some());
        let model = build_model_result.model.unwrap();
        assert_eq!(model.len(), 2);

        let root_instance = model.get_instance(&Moniker::root()).expect("root instance");
        let child_instance =
            model.get_instance(&Moniker::parse_str("child").unwrap()).expect("child instance");

        let other_moniker = Moniker::parse_str("other").unwrap();
        let get_other_result = model.get_instance(&other_moniker);
        assert_eq!(
            get_other_result.err().unwrap().to_string(),
            ComponentInstanceError::instance_not_found(
                Moniker::parse_str(&other_moniker.to_string()).unwrap()
            )
            .to_string()
        );

        assert_eq!(root_instance.moniker(), &Moniker::root());
        assert_eq!(child_instance.moniker(), &Moniker::parse_str("child").unwrap());

        match root_instance.try_get_parent()? {
            ExtendedInstanceInterface::AboveRoot(_) => {}
            _ => panic!("root instance's parent should be `AboveRoot`"),
        }
        match child_instance.try_get_parent()? {
            ExtendedInstanceInterface::Component(component) => {
                assert_eq!(component.moniker(), root_instance.moniker());
            }
            _ => panic!("child instance's parent should be root component"),
        }

        let get_child = root_instance
            .resolve()
            .map(|locked| locked.get_child(&ChildName::try_new("child", None).unwrap()))?;
        assert!(get_child.is_some());
        assert_eq!(get_child.as_ref().unwrap().moniker(), child_instance.moniker());

        assert!(root_instance.resolve().is_ok());
        assert!(child_instance.resolve().is_ok());

        Ok(())
    }

    // Builds a model with structure `root -- child` where the child's URL is expressed in
    // the root manifest as a relative URL.
    #[fuchsia::test]
    fn build_model_with_relative_url() {
        let root_decl = ComponentDeclBuilder::new()
            .child(ChildBuilder::new().name("child").url("#child").build())
            .build();
        let child_decl = ComponentDeclBuilder::new().build();
        let root_url = make_test_url("root");
        let absolute_child_url = Url::new(&format!("{}#child", root_url)).unwrap();

        let mut decls_by_url = HashMap::new();
        decls_by_url.insert(root_url.clone(), (root_decl, None));
        decls_by_url.insert(absolute_child_url.clone(), (child_decl, None));

        let config = Arc::new(RuntimeConfig::default());
        let build_model_result = ModelBuilderForAnalyzer::new(root_url).build(
            decls_by_url,
            config,
            Arc::new(component_id_index::Index::default()),
        );
        assert_eq!(build_model_result.errors.len(), 0);
        assert!(build_model_result.model.is_some());
        let model = build_model_result.model.unwrap();
        assert_eq!(model.len(), 2);

        let child_instance =
            model.get_instance(&Moniker::parse_str("child").unwrap()).expect("child instance");

        assert_eq!(child_instance.url(), &absolute_child_url);
    }

    // Spot-checks that `route_capability` returns immediately when routing a capability from a
    // `ComponentInstanceForAnalyzer`. In addition, updates to that method should
    // be reviewed to make sure that this property holds; otherwise, `ComponentModelForAnalyzer`'s
    // sync methods may panic.
    #[fuchsia::test]
    fn route_capability_is_sync() {
        let components = vec![("root", ComponentDeclBuilder::new().build())];

        let config = Arc::new(RuntimeConfig::default());
        let url = make_test_url("root");
        let build_model_result = ModelBuilderForAnalyzer::new(url).build(
            make_decl_map(components),
            config,
            Arc::new(component_id_index::Index::default()),
        );
        assert_eq!(build_model_result.errors.len(), 0);
        assert!(build_model_result.model.is_some());
        let model = build_model_result.model.unwrap();
        assert_eq!(model.len(), 1);

        let root_instance = model.get_instance(&Moniker::root()).expect("root instance");

        // Panics if the future returned by `route_sandbox_path` was not ready immediately.
        // If no panic, discard the result.
        let _ = model
            .route_sandbox_path(
                &root_instance,
                SandboxPath::used_path(&RelativePath::new("svc/hippo").unwrap()),
                TargetDecl::Use(
                    UseBuilder::protocol()
                        .name("bar_svc")
                        .path("/svc/hippo")
                        .source(cm_rust::UseSource::Parent)
                        .build(),
                ),
                false,
            )
            .now_or_never()
            .expect("routing didn't finish immediately");
    }

    #[fuchsia::test]
    fn config_capability_overrides() {
        let package_value: cm_rust::ConfigValue = cm_rust::ConfigSingleValue::Uint8(1).into();
        let config_value: cm_rust::ConfigValue = cm_rust::ConfigSingleValue::Uint8(2).into();

        let config = Arc::new(RuntimeConfig::default());
        let cm_url = make_test_url("root");

        let decl = ComponentDeclBuilder::new()
            .capability(
                CapabilityBuilder::config().name("my_config").value(config_value.clone().into()),
            )
            .use_(
                UseBuilder::config()
                    .name("my_config")
                    .target_name("config")
                    .source(cm_rust::UseSource::Self_)
                    .config_type(cm_rust::ConfigValueType::Uint8),
            )
            .build();

        let mut decl_map = HashMap::<Url, (ComponentDecl, Option<ConfigFields>)>::new();
        decl_map.insert(
            make_test_url("root"),
            (
                decl,
                Some(ConfigFields {
                    fields: vec![config_encoder::ConfigField {
                        key: "config".into(),
                        value: package_value,
                        mutability: Default::default(),
                    }],
                    checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                }),
            ),
        );

        let build_model_result = ModelBuilderForAnalyzer::new(cm_url.clone()).build(
            decl_map,
            config,
            Arc::new(component_id_index::Index::default()),
        );
        assert_eq!(build_model_result.errors.len(), 0);
        assert!(build_model_result.model.is_some());
        let model = build_model_result.model.unwrap();
        assert_eq!(model.len(), 1);

        let config = model.collect_config_by_url().unwrap();
        let config = config.get(cm_url.as_str()).unwrap();
        assert_eq!(config.fields.len(), 1);
        assert_eq!(config.fields[0].key.as_str(), "config");
        assert_eq!(config.fields[0].value, config_value);
    }

    // This checks that a component works successfully with just config capabilities
    // and no Config Value File.
    #[fuchsia::test]
    fn config_capability_only() {
        let config_value: cm_rust::ConfigValue = cm_rust::ConfigSingleValue::Uint8(2).into();

        let config = Arc::new(RuntimeConfig::default());
        let cm_url = make_test_url("root");

        let decl = ComponentDeclBuilder::new()
            .capability(
                CapabilityBuilder::config().name("my_config").value(config_value.clone().into()),
            )
            .use_(
                UseBuilder::config()
                    .name("my_config")
                    .target_name("config")
                    .source(cm_rust::UseSource::Self_)
                    .config_type(cm_rust::ConfigValueType::Uint8),
            )
            .config(cm_rust::ConfigDecl {
                fields: Box::from([]),
                checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                value_source: cm_rust::ConfigValueSource::Capabilities(Default::default()),
            })
            .build();

        let mut decl_map = HashMap::<Url, (ComponentDecl, Option<ConfigFields>)>::new();
        decl_map.insert(make_test_url("root"), (decl, None));

        let build_model_result = ModelBuilderForAnalyzer::new(cm_url.clone()).build(
            decl_map,
            config,
            Arc::new(component_id_index::Index::default()),
        );
        assert_eq!(build_model_result.errors.len(), 0);
        assert!(build_model_result.model.is_some());
        let model = build_model_result.model.unwrap();
        assert_eq!(model.len(), 1);

        let config = model.collect_config_by_url().unwrap();
        let config = config.get(cm_url.as_str()).unwrap();
        assert_eq!(config.fields.len(), 1);
        assert_eq!(config.fields[0].key.as_str(), "config");
        assert_eq!(config.fields[0].value, config_value);
    }

    #[fuchsia::test]
    fn config_capability_optional_from_void() {
        let package_value: cm_rust::ConfigValue = cm_rust::ConfigSingleValue::Uint8(1).into();

        let config = Arc::new(RuntimeConfig::default());
        let cm_url = make_test_url("root");

        // Create and  add the root cml.
        let decl = ComponentDeclBuilder::new()
            .child(
                cm_rust_testing::ChildBuilder::new()
                    .name("child")
                    .url(&make_test_url("child").to_string()),
            )
            .offer(
                cm_rust_testing::OfferBuilder::config()
                    .source(cm_rust::offer::OfferSource::Void)
                    .name("my_config")
                    .target(cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                        name: "child".parse().unwrap(),
                        collection: None,
                    }))
                    .availability(cm_rust::Availability::Optional),
            )
            .build();

        let mut decl_map = HashMap::<Url, (ComponentDecl, Option<ConfigFields>)>::new();
        decl_map.insert(make_test_url("root"), (decl, None));

        // Create and add the child CML.
        let child_url = Url::new(make_test_url("child").to_string())
            .expect("failed to parse root component url");
        let decl = ComponentDeclBuilder::new()
            .use_(
                UseBuilder::config()
                    .name("my_config")
                    .target_name("config")
                    .source(cm_rust::UseSource::Parent)
                    .availability(cm_rust::Availability::Optional)
                    .config_type(cm_rust::ConfigValueType::Uint8),
            )
            .build();

        decl_map.insert(
            make_test_url("child"),
            (
                decl,
                Some(ConfigFields {
                    fields: vec![config_encoder::ConfigField {
                        key: "config".into(),
                        value: package_value.clone(),
                        mutability: Default::default(),
                    }],
                    checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                }),
            ),
        );

        let build_model_result = ModelBuilderForAnalyzer::new(cm_url).build(
            decl_map,
            config,
            Arc::new(component_id_index::Index::default()),
        );
        assert_eq!(build_model_result.errors.len(), 0);
        assert!(build_model_result.model.is_some());
        let model = build_model_result.model.unwrap();
        assert_eq!(model.len(), 2);

        let config = model.collect_config_by_url().unwrap();
        let config = config.get(child_url.as_str()).unwrap();
        assert_eq!(config.fields.len(), 1);
        assert_eq!(config.fields[0].key.as_str(), "config");
        assert_eq!(config.fields[0].value, package_value);
    }

    #[fuchsia::test]
    fn config_capability_routing_error() {
        let config = Arc::new(RuntimeConfig::default());
        let cm_url = make_test_url("root");

        let decl = ComponentDeclBuilder::new()
            .use_(
                UseBuilder::config()
                    .name("my_config")
                    .target_name("config")
                    .source(cm_rust::UseSource::Parent)
                    .config_type(cm_rust::ConfigValueType::Uint8),
            )
            .config(cm_rust::ConfigDecl {
                fields: Box::from([]),
                checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                value_source: cm_rust::ConfigValueSource::Capabilities(Default::default()),
            })
            .build();

        let mut decl_map = HashMap::<Url, (ComponentDecl, Option<ConfigFields>)>::new();
        decl_map.insert(make_test_url("root"), (decl, None));

        let build_model_result = ModelBuilderForAnalyzer::new(cm_url).build(
            decl_map,
            config,
            Arc::new(component_id_index::Index::default()),
        );
        assert_eq!(build_model_result.errors.len(), 0);
        assert!(build_model_result.model.is_some());
        let model = build_model_result.model.unwrap();
        assert_eq!(model.len(), 1);

        assert_matches!(model.collect_config_by_url(), Err(_));
    }

    // Builds a model with structure `root -- child` in which the child environment extends the root's.
    // Checks that the child has access to the inherited runner and resolver registrations through its
    // environment.
    #[fuchsia::test]
    async fn environment_inherits() -> Result<()> {
        let child_env_name = "child_env";
        let child_runner_registration = RunnerRegistration {
            source_name: "child_env_runner".parse().unwrap(),
            source: RegistrationSource::Self_,
            target_name: "child_env_runner".parse().unwrap(),
        };
        let child_resolver_registration = ResolverRegistration {
            resolver: "child_env_resolver".parse().unwrap(),
            source: RegistrationSource::Self_,
            scheme: "child_resolver_scheme".into(),
        };

        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("child").environment(child_env_name))
                    .capability(
                        CapabilityBuilder::resolver()
                            .name("child_env_resolver")
                            .path("/svc/child_env_resolver"),
                    )
                    .capability(
                        CapabilityBuilder::runner()
                            .name("child_env_runner")
                            .path("/svc/child_env_runner"),
                    )
                    .environment(
                        EnvironmentBuilder::new()
                            .name(child_env_name)
                            .extends(fdecl::EnvironmentExtends::Realm)
                            .resolver(child_resolver_registration.clone())
                            .runner(child_runner_registration.clone()),
                    )
                    .build(),
            ),
            ("child", ComponentDeclBuilder::new().build()),
        ];

        // Set up the RuntimeConfig to register the `fuchsia-boot` resolver as a built-in,
        // in addition to `builtin_runner`.
        let mut config = RuntimeConfig::default();
        config.builtin_boot_resolver = component_internal::BuiltinBootResolver::Boot;

        let builtin_runner_name: Name = "builtin_elf_runner".parse().unwrap();
        let builtin_runner_decl = cm_rust::CapabilityDecl::Runner(cm_rust::RunnerDecl {
            name: builtin_runner_name.clone(),
            source_path: None,
        });
        let builtin_resolver_name: Name = BOOT_SCHEME.parse().unwrap();
        let builtin_resolver_decl = cm_rust::CapabilityDecl::Resolver(cm_rust::ResolverDecl {
            name: builtin_resolver_name.clone(),
            source_path: None,
        });
        config.builtin_capabilities = vec![builtin_runner_decl, builtin_resolver_decl];

        let cm_url = make_test_url("root");
        let build_model_result = ModelBuilderForAnalyzer::new(cm_url).build(
            make_decl_map(components),
            Arc::new(config),
            Arc::new(component_id_index::Index::default()),
        );
        assert_eq!(build_model_result.errors.len(), 0);
        assert!(build_model_result.model.is_some());
        let model = build_model_result.model.unwrap();
        assert_eq!(model.len(), 2);

        let child_instance =
            model.get_instance(&Moniker::parse_str("child").unwrap()).expect("child instance");

        let environment = child_instance
            .component_sandbox()
            .await
            .expect("failed to get sandbox")
            .component_input
            .environment();
        let runner_router_capability =
            environment.runners().get(&child_runner_registration.target_name).unwrap();
        let Capability::ConnectorRouter(runner_router) = runner_router_capability else {
            panic!("unexpected capability for runner");
        };
        let request = runner_metadata(cm_rust::Availability::Required);
        let source = runner_router
            .route_debug(request, child_instance.as_weak().into())
            .await
            .expect("unexpected response");
        assert_eq!(source.source_moniker(), Moniker::root().into());

        let resolver_router_capability = environment
            .resolvers()
            .get(&Name::new(&child_resolver_registration.scheme).unwrap())
            .unwrap();
        let Capability::ConnectorRouter(resolver_router) = resolver_router_capability else {
            panic!("unexpected capability for resolver");
        };
        let request = resolver_metadata(cm_rust::Availability::Required);
        let source = resolver_router
            .route_debug(request, child_instance.as_weak().into())
            .await
            .expect("unexpected response");
        assert_eq!(source.source_moniker(), Moniker::root().into());

        let runner_router_capability =
            environment.runners().get(&child_runner_registration.target_name).unwrap();
        let Capability::ConnectorRouter(runner_router) = runner_router_capability else {
            panic!("unexpected capability for runner");
        };
        let request = runner_metadata(cm_rust::Availability::Required);
        let source = runner_router
            .route_debug(request, child_instance.as_weak().into())
            .await
            .expect("unexpected response");
        assert_eq!(source.source_moniker(), Moniker::root().into());

        let runner_router_capability = environment.runners().get(&builtin_runner_name).unwrap();
        let Capability::ConnectorRouter(runner_router) = runner_router_capability else {
            panic!("unexpected capability for runner");
        };
        let request = runner_metadata(cm_rust::Availability::Required);
        let source = runner_router
            .route_debug(request, child_instance.as_weak().into())
            .await
            .expect("unexpected response");
        assert_eq!(source.source_moniker(), ExtendedMoniker::ComponentManager);

        let scheme_name = Name::new(&*BOOT_SCHEME).unwrap();
        let resolver_router_capability = environment.resolvers().get(&scheme_name).unwrap();
        let Capability::ConnectorRouter(resolver_router) = resolver_router_capability else {
            panic!("unexpected capability for resolver");
        };
        let request = resolver_metadata(cm_rust::Availability::Required);
        let source = resolver_router
            .route_debug(request, child_instance.as_weak().into())
            .await
            .expect("unexpected response");
        assert_eq!(source.source_moniker(), ExtendedMoniker::ComponentManager);

        Ok(())
    }

    fn decl(id: &str) -> ComponentDecl {
        // Identify decls by a single child named `id`.
        ComponentDeclBuilder::new().child_default(id).build()
    }

    #[fuchsia::test]
    fn get_decl_by_url_none() {
        let beta_beta_urls = vec![
            Url::new("fuchsia-pkg://test.fuchsia.com/beta#beta.cm").unwrap(),
            Url::new("fuchsia-pkg://test.fuchsia.com/beta/0#beta.cm").unwrap(),
            Url::new("fuchsia-pkg://test.fuchsia.com/beta?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap(),
            Url::new("fuchsia-pkg://test.fuchsia.com/beta/0?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap(),
        ];
        let decls_by_url_no_beta_beta = hashmap! {
            Url::new("fuchsia-pkg://test.fuchsia.com/alpha#beta.cm").unwrap() => (decl("alpha_beta"), None),
            Url::new("fuchsia-pkg://test.fuchsia.com/beta/0#alpha.cm").unwrap() => (decl("beta_alpha"), None),
            Url::new("fuchsia-pkg://test.fuchsia.com/gamma?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap() => (decl("gamma_beta"), None),
        };

        for beta_beta_url in beta_beta_urls.iter() {
            let result =
                ModelBuilderForAnalyzer::get_decl_by_url(&decls_by_url_no_beta_beta, beta_beta_url);
            assert!(result.is_ok());
            assert_eq!(None, result.ok().unwrap());
        }
    }

    #[fuchsia::test]
    fn get_decl_by_url_fuchsia_boot() {
        let fuchsia_boot_url = Url::new("fuchsia-boot:///#meta/boot.cm").unwrap();
        let fuchsia_boot_component = decl("boot");
        let decls_by_url_with_fuchsia_boot = hashmap! {
            Url::new("fuchsia-pkg://test.fuchsia.com/alpha#beta.cm").unwrap() => (decl("alpha_beta"), None),
            Url::new("fuchsia-pkg://test.fuchsia.com/beta/0#alpha.cm").unwrap() => (decl("beta_alpha"), None),
            Url::new("fuchsia-pkg://test.fuchsia.com/gamma?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap() => (decl("gamma_beta"), None),
            fuchsia_boot_url.clone() => (fuchsia_boot_component.clone(), None),
        };

        let result = ModelBuilderForAnalyzer::get_decl_by_url(
            &decls_by_url_with_fuchsia_boot,
            &fuchsia_boot_url,
        );

        assert!(result.is_ok());
        assert_eq!(Some(&(fuchsia_boot_component, None)), result.ok().unwrap());
    }

    #[fuchsia::test]
    fn get_decl_by_url_bad_url() {
        let bad_url =
            Url::new("fuchsia-pkg:///test.fuchsia.com/alpha?hash=notahexvalue#meta/alpha.cm")
                .unwrap();
        let empty_decls_by_url = hashmap! {};

        let result = ModelBuilderForAnalyzer::get_decl_by_url(&empty_decls_by_url, &bad_url);

        assert!(result.is_err());
    }

    #[fuchsia::test]
    fn get_decl_by_url_strong() {
        let beta_beta_url = Url::new("fuchsia-pkg://test.fuchsia.com/beta/0?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap();
        let beta_beta_decl = decl("beta_beta");
        let decls_by_url_with_beta_beta = hashmap! {
            Url::new("fuchsia-pkg://test.fuchsia.com/alpha#beta.cm").unwrap() => (decl("alpha_beta"), None),
            Url::new("fuchsia-pkg://test.fuchsia.com/beta/0#alpha.cm").unwrap() => (decl("beta_alpha"), None),
            Url::new("fuchsia-pkg://test.fuchsia.com/gamma?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap() => (decl("gamma_beta"), None),
            beta_beta_url.clone() => (beta_beta_decl.clone(), None),
        };

        let result =
            ModelBuilderForAnalyzer::get_decl_by_url(&decls_by_url_with_beta_beta, &beta_beta_url);

        assert!(result.is_ok());
        assert_eq!(Some(&(beta_beta_decl, None)), result.ok().unwrap());
    }

    #[fuchsia::test]
    fn get_decl_by_url_strongest() {
        let beta_beta_strong_url = Url::new("fuchsia-pkg://test.fuchsia.com/beta/0?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap();
        let beta_beta_strong_decl = decl("beta_beta_strong");
        let beta_beta_weak_url_1 =
            Url::new("fuchsia-pkg://test.fuchsia.com/beta/0#beta.cm").unwrap();
        let beta_beta_weak_decl_1 = decl("beta_beta_weak_1");
        let beta_beta_weak_url_2 = Url::new("fuchsia-pkg://test.fuchsia.com/beta?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap();
        let beta_beta_weak_decl_2 = decl("beta_beta_weak_2");
        let beta_beta_weak_url_3 = Url::new("fuchsia-pkg://test.fuchsia.com/beta#beta.cm").unwrap();
        let beta_beta_weak_decl_3 = decl("beta_beta_weak_3");
        let decls_by_url_with_4_beta_betas = hashmap! {
            beta_beta_weak_url_1 => (beta_beta_weak_decl_1, None),
            beta_beta_weak_url_2 => (beta_beta_weak_decl_2, None),
            beta_beta_weak_url_3 => (beta_beta_weak_decl_3, None),
            beta_beta_strong_url.clone() => (beta_beta_strong_decl.clone(), None),
        };

        let result = ModelBuilderForAnalyzer::get_decl_by_url(
            &decls_by_url_with_4_beta_betas,
            &beta_beta_strong_url,
        );

        assert!(result.is_ok());
        assert_eq!(Some(&(beta_beta_strong_decl, None)), result.ok().unwrap());
    }

    #[fuchsia::test]
    fn get_decl_by_url_weak() {
        let beta_beta_strong_url = Url::new("fuchsia-pkg://test.fuchsia.com/beta/0?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap();
        let beta_beta_weak_url = Url::new("fuchsia-pkg://test.fuchsia.com/beta/0?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap();
        let beta_beta_decl = decl("beta_beta");
        let decls_by_url_with_strong_beta_beta = hashmap! {
            Url::new("fuchsia-pkg://test.fuchsia.com/alpha#beta.cm").unwrap() => (decl("alpha_beta"), None),
            Url::new("fuchsia-pkg://test.fuchsia.com/beta/0#alpha.cm").unwrap() => (decl("beta_alpha"), None),
            Url::new("fuchsia-pkg://test.fuchsia.com/gamma?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap() => (decl("gamma_beta"), None),
            beta_beta_strong_url.clone() => (beta_beta_decl.clone(), None),
        };

        let result = ModelBuilderForAnalyzer::get_decl_by_url(
            &decls_by_url_with_strong_beta_beta,
            &beta_beta_weak_url,
        );

        assert!(result.is_ok());
        assert_eq!(Some(&(beta_beta_decl, None)), result.ok().unwrap());
    }

    #[fuchsia::test]
    fn get_decl_by_url_weak_any() {
        let beta_beta_url_1 = Url::new("fuchsia-pkg://test.fuchsia.com/beta/0?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap();
        let beta_beta_decl_1 = decl("beta_beta_strong");
        let beta_beta_url_2 = Url::new("fuchsia-pkg://test.fuchsia.com/beta/0#beta.cm").unwrap();
        let beta_beta_decl_2 = decl("beta_beta_weak_1");
        let beta_beta_url_3 = Url::new("fuchsia-pkg://test.fuchsia.com/beta?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap();
        let beta_beta_decl_3 = decl("beta_beta_weak_2");
        let beta_beta_weakest_url =
            Url::new("fuchsia-pkg://test.fuchsia.com/beta#beta.cm").unwrap();
        let decls_by_url_3_weak_matches = hashmap! {
            Url::new("fuchsia-pkg://test.fuchsia.com/alpha#beta.cm").unwrap() => (decl("alpha_beta"), None),
            Url::new("fuchsia-pkg://test.fuchsia.com/beta/0#alpha.cm").unwrap() => (decl("beta_alpha"), None),
            Url::new("fuchsia-pkg://test.fuchsia.com/gamma?hash=0000000000000000000000000000000000000000000000000000000000000000#beta.cm").unwrap() => (decl("gamma_beta"), None),
            beta_beta_url_1 => (beta_beta_decl_1.clone(), None),
            beta_beta_url_2 => (beta_beta_decl_2.clone(), None),
            beta_beta_url_3 => (beta_beta_decl_3.clone(), None),
        };

        let result = ModelBuilderForAnalyzer::get_decl_by_url(
            &decls_by_url_3_weak_matches,
            &beta_beta_weakest_url,
        );

        assert!(result.is_ok());
        let actual_decl = result.ok().unwrap().unwrap();
        assert!(
            beta_beta_decl_1 == actual_decl.0
                || beta_beta_decl_2 == actual_decl.0
                || beta_beta_decl_3 == actual_decl.0
        );
    }
}
