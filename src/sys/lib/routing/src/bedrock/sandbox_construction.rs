// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::DictExt;
use crate::bedrock::aggregate_router::{AggregateRouterFn, AggregateSource};
use crate::bedrock::structured_dict::{
    ComponentEnvironment, ComponentInput, ComponentOutput, StructuredDictMap,
};
use crate::bedrock::use_dictionary_router::UseDictionaryRouter;
use crate::bedrock::with_service_renames_and_filter::WithServiceRenamesAndFilter;
use crate::component_instance::ComponentInstanceInterface;
use crate::error::{ErrorReporter, RouteVerb, RoutingError};
use crate::error_logging_router::ErrorLoggingRouter;
use crate::intermediate_router::{IntermediateRouter, RouteRequest, WeakDictionaryOrRouter};
use crate::to_request::ToRequest;
use crate::to_source::ToSource;
use async_trait::async_trait;
use capability_source::{
    AggregateCapability, AggregateInstance, AggregateMember, AnonymizedAggregateSource,
    CapabilitySource, ComponentCapability, ComponentSource, FilteredAggregateProviderSource,
    InternalCapability, InternalEventStreamCapability, VoidSource,
};
use cm_rust::offer::OfferDeclCommon;
use cm_rust::{
    CapabilityTypeName, DictionaryValue, ExposeDeclCommon, FidlIntoNative, NativeIntoFidl,
    SourceName, SourcePath, UseDeclCommon,
};
use cm_types::{IterablePath, Name, RelativePath};
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_runtime as fruntime;
use fuchsia_sync::Mutex;
use log::warn;
use moniker::{ChildName, Moniker};
use router_error::RouterError;
use runtime_capabilities::{
    Capability, CapabilityBound, Connector, Data, Dictionary, DirConnector, Routable, Router,
    RouterErrorInfo, WeakInstanceToken,
};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::sync::{Arc, LazyLock};

/// This type comes from `UseEventStreamDecl`.
pub type EventStreamFilter = Option<BTreeMap<String, DictionaryValue>>;

/// Contains all of the information needed to find and use a source of an event stream.
#[derive(Clone)]
pub struct EventStreamSourceRouter {
    /// The source router that should return a dictionary detailing specifics on the event stream
    /// such as its type and scope.
    pub router: Arc<Router<Dictionary>>,
    /// The filter that should be applied on the event stream initialized from the information
    /// returned by the router.
    pub filter: EventStreamFilter,
}
pub type EventStreamUseRouterFn<C> =
    dyn Fn(&Arc<C>, Vec<EventStreamSourceRouter>) -> Arc<Router<Connector>>;

static NAMESPACE: LazyLock<Name> = LazyLock::new(|| "namespace".parse().unwrap());
static NUMBERED_HANDLES: LazyLock<Name> = LazyLock::new(|| "numbered_handles".parse().unwrap());
static RUNNER: LazyLock<Name> = LazyLock::new(|| "runner".parse().unwrap());
static CONFIG: LazyLock<Name> = LazyLock::new(|| "config".parse().unwrap());

/// All capabilities that are available to a component's program.
#[derive(Debug, Clone)]
pub struct ProgramInput {
    // This will always have the following fields:
    // - namespace: Arc<Dictionary>
    // - runner: Option<Arc<Router<Connector>>>
    // - config: Arc<Dictionary>
    // - numbered_handles: Arc<Dictionary>
    inner: Arc<Dictionary>,
}

impl Default for ProgramInput {
    fn default() -> Self {
        Self::new(Dictionary::new(), None, Dictionary::new())
    }
}

impl From<ProgramInput> for Arc<Dictionary> {
    fn from(program_input: ProgramInput) -> Self {
        program_input.inner
    }
}

impl ProgramInput {
    pub fn new(
        namespace: Arc<Dictionary>,
        runner: Option<Arc<Router<Connector>>>,
        config: Arc<Dictionary>,
    ) -> Self {
        let inner = Dictionary::new();
        inner.insert(NAMESPACE.clone(), Capability::Dictionary(namespace));
        if let Some(runner) = runner {
            inner.insert(RUNNER.clone(), Capability::ConnectorRouter(runner));
        }
        inner.insert(NUMBERED_HANDLES.clone(), Capability::Dictionary(Dictionary::new()));
        inner.insert(CONFIG.clone(), Capability::Dictionary(config));
        ProgramInput { inner }
    }

    /// All of the capabilities that appear in a program's namespace.
    pub fn namespace(&self) -> Arc<Dictionary> {
        let cap = self.inner.get(&*NAMESPACE).unwrap();
        let Capability::Dictionary(dict) = cap else {
            unreachable!("namespace entry must be a dictionary: {cap:?}");
        };
        dict
    }

    /// All of the capabilities that appear in a program's set of numbered handles.
    pub fn numbered_handles(&self) -> Arc<Dictionary> {
        let cap = self.inner.get(&*NUMBERED_HANDLES).unwrap();
        let Capability::Dictionary(dict) = cap else {
            unreachable!("numbered_handles entry must be a dictionary: {cap:?}");
        };
        dict
    }

    /// A router for the runner that a component has used (if any).
    pub fn runner(&self) -> Option<Arc<Router<Connector>>> {
        let cap = self.inner.get(&*RUNNER);
        match cap {
            None => None,
            Some(Capability::ConnectorRouter(r)) => Some(r),
            cap => unreachable!("runner entry must be a router: {cap:?}"),
        }
    }

    fn set_runner(&self, capability: Capability) {
        let _ = self.inner.insert(RUNNER.clone(), capability);
    }

    /// All of the config capabilities that a program will use.
    pub fn config(&self) -> Arc<Dictionary> {
        let cap = self.inner.get(&*CONFIG).unwrap();
        let Capability::Dictionary(dict) = cap else {
            unreachable!("config entry must be a dictionary: {cap:?}");
        };
        dict
    }
}

/// A component's sandbox holds all the routing dictionaries that a component has once its been
/// resolved.
#[derive(Debug)]
pub struct ComponentSandbox {
    /// The dictionary containing all capabilities that a component's parent provided to it.
    pub component_input: ComponentInput,

    /// The dictionary containing all capabilities that a component makes available to its parent.
    pub component_output: ComponentOutput,

    /// The dictionary containing all capabilities that are available to a component's program.
    pub program_input: ProgramInput,

    /// The dictionary containing all capabilities that a component's program can provide.
    pub program_output_dict: Arc<Dictionary>,

    /// Router that returns the dictionary of framework capabilities scoped to a component. This a
    /// Router rather than the Dictionary itself to save memory.
    ///
    /// REQUIRES: This Router must never poll. This constraint exists `build_component_sandbox` is
    /// not async.
    // NOTE: This is wrapped in Mutex for interior mutability so that it is modifiable like the
    // other parts of the sandbox. If this were a Dictionary this wouldn't be necessary because
    // Dictionary already supports interior mutability, but since this is a singleton we don't need
    // a Dictionary here. The Arc around the Mutex is needed for Sync.
    framework_router: Mutex<Arc<Router<Dictionary>>>,

    /// The dictionary containing all capabilities that a component declares based on another
    /// capability. Currently this is only the storage admin protocol.
    pub capability_sourced_capabilities_dict: Arc<Dictionary>,

    /// The dictionary containing all dictionaries declared by this component.
    pub declared_dictionaries: Arc<Dictionary>,

    /// This set holds a component input dictionary for each child of a component. Each dictionary
    /// contains all capabilities the component has made available to a specific collection.
    pub child_inputs: StructuredDictMap<ComponentInput>,

    /// This set holds a component input dictionary for each collection declared by a component.
    /// Each dictionary contains all capabilities the component has made available to a specific
    /// collection.
    pub collection_inputs: StructuredDictMap<ComponentInput>,

    /// This holds the one dictionary router for each child of this component, for the child's
    /// outgoing dictionary. Invoking the router causes the child to be resolved if this hasn't yet
    /// occurred.
    pub child_outputs: Mutex<HashMap<ChildName, Arc<Router<Dictionary>>>>,
}

impl Default for ComponentSandbox {
    fn default() -> Self {
        static NULL_ROUTER: LazyLock<Arc<Router<Dictionary>>> =
            LazyLock::new(|| Router::new(NullRouter {}));
        struct NullRouter;
        #[async_trait]
        impl Routable<Dictionary> for NullRouter {
            async fn route(
                &self,
                _request: fruntime::RouteRequest,
                _target: Arc<WeakInstanceToken>,
            ) -> Result<Option<Arc<Dictionary>>, RouterError> {
                panic!("null router invoked");
            }
            async fn route_debug(
                &self,
                _request: fruntime::RouteRequest,
                _target: Arc<WeakInstanceToken>,
            ) -> Result<CapabilitySource, RouterError> {
                panic!("null router invoked");
            }
        }
        let framework_router = Mutex::new(NULL_ROUTER.clone());
        Self {
            framework_router,
            component_input: Default::default(),
            component_output: Default::default(),
            program_input: Default::default(),
            program_output_dict: Default::default(),
            capability_sourced_capabilities_dict: Default::default(),
            declared_dictionaries: Default::default(),
            child_inputs: Default::default(),
            collection_inputs: Default::default(),
            child_outputs: Mutex::new(Default::default()),
        }
    }
}

impl From<ComponentSandbox> for Arc<Dictionary> {
    fn from(sandbox: ComponentSandbox) -> Arc<Dictionary> {
        let sandbox_dictionary = Dictionary::new();
        sandbox_dictionary.insert(
            Name::new("framework").unwrap(),
            Capability::DictionaryRouter(sandbox.framework_router.lock().clone()),
        );
        sandbox_dictionary.insert(
            Name::new("component_input").unwrap(),
            Capability::Dictionary(sandbox.component_input.into()),
        );
        sandbox_dictionary.insert(
            Name::new("component_output").unwrap(),
            Capability::Dictionary(sandbox.component_output.into()),
        );
        sandbox_dictionary.insert(
            Name::new("program_input").unwrap(),
            Capability::Dictionary(sandbox.program_input.into()),
        );
        sandbox_dictionary.insert(
            Name::new("program_output").unwrap(),
            Capability::Dictionary(sandbox.program_output_dict),
        );
        sandbox_dictionary.insert(
            Name::new("capability_sourced").unwrap(),
            Capability::Dictionary(sandbox.capability_sourced_capabilities_dict),
        );
        sandbox_dictionary.insert(
            Name::new("declared_dictionaries").unwrap(),
            Capability::Dictionary(sandbox.declared_dictionaries),
        );
        sandbox_dictionary.insert(
            Name::new("child_inputs").unwrap(),
            Capability::Dictionary(sandbox.child_inputs.into()),
        );
        sandbox_dictionary.insert(
            Name::new("collection_inputs").unwrap(),
            Capability::Dictionary(sandbox.collection_inputs.into()),
        );
        sandbox_dictionary
    }
}

impl Clone for ComponentSandbox {
    fn clone(&self) -> Self {
        let Self {
            component_input,
            component_output,
            program_input,
            program_output_dict,
            framework_router,
            capability_sourced_capabilities_dict,
            declared_dictionaries,
            child_inputs,
            collection_inputs,
            child_outputs,
        } = self;
        Self {
            component_input: component_input.clone(),
            component_output: component_output.clone(),
            program_input: program_input.clone(),
            program_output_dict: program_output_dict.clone(),
            framework_router: Mutex::new(framework_router.lock().clone()),
            capability_sourced_capabilities_dict: capability_sourced_capabilities_dict.clone(),
            declared_dictionaries: declared_dictionaries.clone(),
            child_inputs: child_inputs.clone(),
            collection_inputs: collection_inputs.clone(),
            child_outputs: Mutex::new(child_outputs.lock().clone()),
        }
    }
}

impl ComponentSandbox {
    pub fn framework_router(&self) -> Arc<Router<Dictionary>> {
        self.framework_router.lock().clone()
    }
}

/// Once a component has been resolved and its manifest becomes known, this function produces the
/// various dicts the component needs based on the contents of its manifest.
pub fn build_component_sandbox<C: ComponentInstanceInterface + 'static>(
    component: &Arc<C>,
    child_outputs: HashMap<ChildName, Arc<Router<Dictionary>>>,
    decl: &cm_rust::ComponentDecl,
    component_input: ComponentInput,
    program_output_dict: Arc<Dictionary>,
    framework_router: Arc<Router<Dictionary>>,
    capability_sourced_capabilities_dict: Arc<Dictionary>,
    declared_dictionaries: Arc<Dictionary>,
    error_reporter: impl ErrorReporter,
    aggregate_router_fn: &AggregateRouterFn<C>,
    event_stream_use_router_fn: &EventStreamUseRouterFn<C>,
) -> ComponentSandbox {
    let sandbox = ComponentSandbox {
        framework_router: Mutex::new(framework_router),
        component_input,
        program_output_dict,
        capability_sourced_capabilities_dict,
        declared_dictionaries,
        child_outputs: Mutex::new(child_outputs),
        ..Default::default()
    };
    let mut environments = HashMap::new();

    for environment_decl in &decl.environments {
        let _ = environments.insert(
            environment_decl.name.clone(),
            build_environment(component, &sandbox, environment_decl),
        );
    }

    for child in &decl.children {
        let environment;
        if let Some(environment_name) = child.environment.as_ref() {
            environment = environments
                .get(environment_name)
                .expect(
                    "child references nonexistent environment, \
                    this should be prevented in manifest validation",
                )
                .clone();
        } else {
            environment = sandbox.component_input.environment();
        }
        let input = ComponentInput::new(environment);
        let name = Name::new(child.name.as_str()).expect("child is static so name is not long");
        let _ = sandbox.child_inputs.insert(name, input);
    }

    for collection in &decl.collections {
        let environment;
        if let Some(environment_name) = collection.environment.as_ref() {
            environment = environments
                .get(environment_name)
                .expect(
                    "collection references nonexistent environment, \
                    this should be prevented in manifest validation",
                )
                .clone();
        } else {
            environment = sandbox.component_input.environment();
        }
        let input = ComponentInput::new(environment);
        let _ = sandbox.collection_inputs.insert(collection.name.clone(), input);
    }

    let mut dictionary_use_bundles = Vec::with_capacity(decl.uses.len());
    for use_bundle in group_use_aggregates(&decl.uses).into_iter() {
        let first_use = *use_bundle.first().unwrap();
        match first_use {
            cm_rust::UseDecl::Service(_)
                if use_bundle.len() > 1
                    || matches!(first_use.source(), cm_rust::UseSource::Collection(_)) =>
            {
                let aggregate_router = new_aggregate_service_router(
                    component,
                    &sandbox,
                    &use_bundle,
                    aggregate_router_fn,
                );
                let prev = sandbox
                    .program_input
                    .namespace()
                    .insert_capability(first_use.path().unwrap(), aggregate_router);
                assert!(
                    prev.is_none(),
                    "failed to insert {}: preexisting value",
                    first_use.path().unwrap()
                );
            }
            cm_rust::UseDecl::EventStream(_) => extend_dict_with_event_stream_uses(
                component,
                &sandbox,
                use_bundle,
                error_reporter.clone(),
                event_stream_use_router_fn,
            ),
            cm_rust::UseDecl::Dictionary(_) => {
                dictionary_use_bundles.push(use_bundle);
            }
            use_ => install_use_in_sandbox(component, &sandbox, use_, error_reporter.clone()),
        }
    }

    // The runner may be specified by either use declaration or in the program section of the
    // manifest. If there's no use declaration for a runner and there is one set in the program
    // section, then let's synthesize a use decl for it and add it to the sandbox.
    if !decl.uses.iter().any(|u| matches!(u, cm_rust::UseDecl::Runner(_))) {
        if let Some(runner_name) = decl.program.as_ref().and_then(|p| p.runner.as_ref()) {
            install_use_in_sandbox(
                component,
                &sandbox,
                &cm_rust::UseDecl::Runner(cm_rust::UseRunnerDecl {
                    source: cm_rust::UseSource::Environment,
                    source_name: runner_name.clone(),
                    source_dictionary: Default::default(),
                }),
                error_reporter.clone(),
            );
        }
    }

    // Dictionary uses are special: if any capabilities are used at a path that's a prefix of a
    // dictionary use, then those capabilities are transparently added to the dictionary we
    // assemble in the program input dictionary. In order to do this correctly, we want the program
    // input dictionary to be complete (aside from used dictionaries) so that the dictionaries
    // we're merging with the used dictionaries aren't missing entries. For this reason, we wait
    // until after all other uses are processed before processing used dictionaries.
    for dictionary_use_bundle in dictionary_use_bundles {
        extend_dict_with_dictionary_use(
            component,
            &sandbox,
            dictionary_use_bundle,
            error_reporter.clone(),
        )
    }

    for offer_bundle in group_offer_aggregates(&decl.offers) {
        let first_offer = offer_bundle.first().unwrap();
        match first_offer {
            cm_rust::offer::OfferDecl::Service(_)
                if offer_bundle.len() > 1
                    || matches!(
                        first_offer.source(),
                        cm_rust::offer::OfferSource::Collection(_)
                    ) =>
            {
                let aggregate_router = new_aggregate_service_router(
                    component,
                    &sandbox,
                    &offer_bundle,
                    aggregate_router_fn,
                );
                install_router_to_target(
                    &sandbox,
                    aggregate_router.into(),
                    first_offer.target().clone().native_into_fidl(),
                    vec![first_offer.target_name().clone()].into(),
                );
            }
            _ => install_offer_in_sandbox(component, &sandbox, first_offer),
        }
    }

    for expose_bundle in group_expose_aggregates(&decl.exposes) {
        let first_expose = expose_bundle.first().unwrap();
        match first_expose {
            cm_rust::ExposeDecl::Service(_)
                if expose_bundle.len() > 1
                    || matches!(first_expose.source(), cm_rust::ExposeSource::Collection(_)) =>
            {
                let router = new_aggregate_service_router(
                    component,
                    &sandbox,
                    &expose_bundle,
                    aggregate_router_fn,
                );
                let target_name = first_expose.target_name().clone();
                let prev =
                    sandbox.component_output.capabilities().insert(target_name, router.into());
                assert!(
                    prev.is_none(),
                    "failed to insert {}: preexisting value",
                    first_expose.target_name()
                );
            }
            _ => install_expose_in_sandbox(component, &sandbox, first_expose),
        }
    }

    sandbox
}

fn new_aggregate_service_router<'a, C: ComponentInstanceInterface + 'static, D>(
    component: &Arc<C>,
    sandbox: &ComponentSandbox,
    decl_bundle: &Vec<&'a D>,
    aggregate_router_fn: &AggregateRouterFn<C>,
) -> Capability
where
    AggregateMember: TryFrom<&'a D>,
    D: SourcePath + ToSource + ToRequest + ServiceDeclExt,
    &'a D: Into<CapabilityTypeName> + Into<RouteVerb>,
{
    let mut aggregate_sources = vec![];
    let source = new_aggregate_capability_source(component.moniker().clone(), decl_bundle);
    for decl in decl_bundle.iter() {
        if matches!(&source, &CapabilitySource::FilteredAggregateProvider(_))
            && decl
                .offer_service_decl()
                .map(|decl| !has_filtered_offer(&vec![decl]))
                .unwrap_or(false)
        {
            // We can ignore any offers for a filtered aggregate that have no rename or
            // filtering rules, as they will not be contributing any instances to the
            // aggregate.
            continue;
        } else if let fdecl::Ref::Collection(fdecl::CollectionRef { name }) = decl.to_source() {
            let collection_name = Name::new(name).unwrap();
            aggregate_sources.push(AggregateSource::Collection { collection_name });
        } else {
            let router_capability = new_intermediate_router(component, sandbox, *decl);
            let router: Arc<Router<DirConnector>> = router_capability
                .try_into()
                .expect("invalid type returned by new_intermediate_router");
            let source_instance = match decl.to_source() {
                fdecl::Ref::Self_(_) => AggregateInstance::Self_,
                fdecl::Ref::Parent(_) => AggregateInstance::Parent,
                fdecl::Ref::Child(child_ref) => {
                    let child_ref: cm_rust::ChildRef = child_ref.fidl_into_native();
                    AggregateInstance::Child(child_ref.into())
                }
                other_source => {
                    warn!("unsupported source found in offer aggregate: {:?}", other_source);
                    continue;
                }
            };
            aggregate_sources.push(AggregateSource::DirectoryRouter { source_instance, router })
        }
    }
    (aggregate_router_fn)(component.clone(), aggregate_sources, source).into()
}

/// Returns `true` if any of the offers set a filter or rename mapping.
fn has_filtered_offer(offer_service_decls: &Vec<&cm_rust::OfferServiceDecl>) -> bool {
    offer_service_decls.iter().any(|o| {
        o.source_instance_filter.as_ref().map(|v| !v.is_empty()).unwrap_or(false)
            || o.renamed_instances.as_ref().map(|v| !v.is_empty()).unwrap_or(false)
    })
}

fn new_aggregate_capability_source<'a, D: ServiceDeclExt + ToSource>(
    moniker: Moniker,
    decls: &Vec<&'a D>,
) -> CapabilitySource
where
    AggregateMember: TryFrom<&'a D>,
{
    let offer_service_decls =
        decls.iter().filter_map(|d| d.offer_service_decl()).collect::<Vec<_>>();
    let capability =
        AggregateCapability::Service(decls.first().unwrap().service_name().unwrap().clone());
    if has_filtered_offer(&offer_service_decls) {
        CapabilitySource::FilteredAggregateProvider(FilteredAggregateProviderSource {
            capability,
            moniker,
            offer_service_decls: offer_service_decls.into_iter().cloned().collect(),
        })
    } else {
        let members = decls.iter().filter_map(|o| AggregateMember::try_from(*o).ok()).collect();
        CapabilitySource::AnonymizedAggregate(AnonymizedAggregateSource {
            capability,
            moniker,
            members,
            instances: vec![],
        })
    }
}

/// Groups together a set of offers into sub-sets of those that have the same target and target
/// name. This is useful for identifying which offers are part of an aggregation of capabilities,
/// and which are for standalone routes.
fn group_use_aggregates<'a>(
    uses: &'a [cm_rust::UseDecl],
) -> impl Iterator<Item = Vec<&'a cm_rust::UseDecl>> + 'a {
    let mut groupings = HashMap::with_capacity(uses.len());
    let mut ungroupable_uses = Vec::new();
    for use_ in uses.iter() {
        if let Some(target_path) = use_.path() {
            groupings.entry(target_path).or_insert_with(|| Vec::with_capacity(1)).push(use_);
        } else {
            ungroupable_uses.push(use_);
        }
    }
    groupings.into_values().chain(ungroupable_uses.into_iter().map(|u| vec![u]))
}

/// Groups together a set of offers into sub-sets of those that have the same target and target
/// name. This is useful for identifying which offers are part of an aggregation of capabilities,
/// and which are for standalone routes.
fn group_offer_aggregates<'a>(
    offers: &'a [cm_rust::offer::OfferDecl],
) -> impl Iterator<Item = Vec<&'a cm_rust::offer::OfferDecl>> + 'a {
    let mut groupings = HashMap::with_capacity(offers.len());

    for offer in offers {
        groupings
            .entry((offer.target(), offer.target_name()))
            .or_insert_with(|| Vec::with_capacity(1))
            .push(offer);
    }
    groupings.into_values()
}

/// Identical to `group_offer_aggregates`, but for exposes.
fn group_expose_aggregates<'a>(
    exposes: &'a [cm_rust::ExposeDecl],
) -> impl Iterator<Item = Vec<&'a cm_rust::ExposeDecl>> + 'a {
    let mut groupings = HashMap::with_capacity(exposes.len());
    for expose in exposes {
        groupings
            .entry((expose.target(), expose.target_name()))
            .or_insert_with(|| Vec::with_capacity(1))
            .push(expose);
    }
    groupings.into_values()
}

fn build_environment<C: ComponentInstanceInterface + 'static>(
    component: &Arc<C>,
    sandbox: &ComponentSandbox,
    environment_decl: &cm_rust::EnvironmentDecl,
) -> ComponentEnvironment {
    let mut environment = ComponentEnvironment::new();
    if environment_decl.extends == fdecl::EnvironmentExtends::Realm {
        environment = sandbox.component_input.environment().shallow_copy();
    }
    environment.set_name(&environment_decl.name);
    let debug_routers_and_targets =
        environment_decl.debug_capabilities.iter().map(|registration| {
            let cm_rust::DebugRegistration::Protocol(debug_protocol_registration) = registration;
            (
                new_intermediate_router_source_name(component, sandbox, registration),
                debug_protocol_registration.target_name.clone(),
                environment.debug(),
            )
        });
    let runner_routers_and_targets = environment_decl.runners.iter().map(|registration| {
        (
            new_intermediate_router_source_name(component, sandbox, registration),
            registration.target_name.clone(),
            environment.runners(),
        )
    });
    let resolver_routers_and_targets = environment_decl.resolvers.iter().map(|registration| {
        (
            new_intermediate_router_source_name(component, sandbox, registration),
            Name::new(&registration.scheme).unwrap(),
            environment.resolvers(),
        )
    });
    for (router, target_name, target_dictionary) in debug_routers_and_targets
        .chain(runner_routers_and_targets)
        .chain(resolver_routers_and_targets)
    {
        // This might overwrite an existing value if we're shadowing something in the environment,
        // so we don't assert that there was no previous value.
        let _ = target_dictionary.insert(target_name, router);
    }
    environment
}

/// Extends the given `target_input` to contain the capabilities described in `dynamic_offers`.
pub fn extend_dict_with_offers<C: ComponentInstanceInterface + 'static>(
    component: &Arc<C>,
    sandbox: &ComponentSandbox,
    static_offers: &[cm_rust::offer::OfferDecl],
    dynamic_offers: &[cm_rust::offer::OfferDecl],
    target_input: &ComponentInput,
    aggregate_router_fn: &AggregateRouterFn<C>,
) {
    for offer_bundle in group_offer_aggregates(dynamic_offers).into_iter() {
        let first_offer = offer_bundle.first().unwrap();
        match first_offer {
            cm_rust::offer::OfferDecl::Service(_) => {
                let static_offer_bundles = group_offer_aggregates(static_offers);
                let maybe_static_offer_bundle = static_offer_bundles.into_iter().find(|bundle| {
                    bundle.first().unwrap().target_name() == first_offer.target_name()
                });
                let mut combined_offer_bundle = offer_bundle.clone();
                if let Some(mut static_offer_bundle) = maybe_static_offer_bundle {
                    // We are aggregating together dynamic and static offers, as there are static
                    // offers with the same target name as our current dynamic offers. We already
                    // populated a router for the static bundle in the target input, let's toss
                    // that and generate a new one with the expanded set of offers.
                    let _ = target_input.capabilities().remove(first_offer.target_name());
                    combined_offer_bundle.append(&mut static_offer_bundle);
                }
                if combined_offer_bundle.len() == 1
                    && !matches!(first_offer.source(), cm_rust::offer::OfferSource::Collection(_))
                {
                    let router = new_intermediate_router(component, sandbox, *first_offer);
                    let prev = target_input
                        .capabilities()
                        .insert(first_offer.target_name().clone(), router);
                    assert!(prev.is_none(), "failed to insert capability into target dict");
                } else {
                    let aggregate_router = new_aggregate_service_router(
                        component,
                        sandbox,
                        &combined_offer_bundle,
                        aggregate_router_fn,
                    );
                    let prev = target_input
                        .capabilities()
                        .insert(first_offer.target_name().clone(), aggregate_router.into());
                    assert!(prev.is_none(), "failed to insert capability into target dict");
                }
            }
            offer => {
                let router = new_intermediate_router(component, sandbox, *offer);
                let prev =
                    target_input.capabilities().insert(first_offer.target_name().clone(), router);
                assert!(prev.is_none(), "failed to insert capability into target dict");
            }
        }
    }
}

fn extend_dict_with_event_stream_uses<C: ComponentInstanceInterface + 'static>(
    component: &Arc<C>,
    sandbox: &ComponentSandbox,
    uses: Vec<&cm_rust::UseDecl>,
    error_reporter: impl ErrorReporter,
    event_stream_use_router_fn: &EventStreamUseRouterFn<C>,
) {
    let routers = uses
        .iter()
        .map(|use_| {
            let router = new_intermediate_router(component, sandbox, *use_);
            let router = ErrorLoggingRouter::new(
                router,
                *use_,
                error_reporter.clone(),
                component.as_weak().into(),
            );
            let filter = match use_ {
                cm_rust::UseDecl::EventStream(u) => u.filter.clone(),
                _ => panic!("found non-event-stream use"),
            };
            EventStreamSourceRouter {
                router: router.try_into().expect("unexpected router type"),
                filter,
            }
        })
        .collect::<Vec<_>>();

    let router = event_stream_use_router_fn(component, routers);
    let target_path = match uses.first().unwrap() {
        cm_rust::UseDecl::EventStream(u) => u.target_path.clone(),
        _ => panic!("found non-event-stream use"),
    };
    let prev = sandbox
        .program_input
        .namespace()
        .insert_capability(&target_path, Capability::ConnectorRouter(router));
    assert!(prev.is_none(), "failed to insert {target_path}: preexisting value");
}

use std::borrow::Borrow;

fn new_intermediate_router_inner(
    moniker: Moniker,
    type_name: CapabilityTypeName,
    sandbox: &ComponentSandbox,
    request: RouteRequest,
    default_token: Arc<WeakInstanceToken>,
    verb: RouteVerb,
    ref_: fdecl::Ref,
    source_path: RelativePath,
) -> Capability {
    let source: WeakDictionaryOrRouter = match &ref_ {
        fdecl::Ref::Parent(_) => Arc::downgrade(&sandbox.component_input.capabilities()).into(),
        fdecl::Ref::Self_(_) => {
            let fruntime_dictionary_router_name =
                Name::new(fidl_fuchsia_component_runtime::DictionaryRouterMarker::PROTOCOL_NAME)
                    .unwrap();
            let fsandbox_dictionary_router_name =
                Name::new(fidl_fuchsia_component_sandbox::DictionaryRouterMarker::PROTOCOL_NAME)
                    .unwrap();
            if type_name == CapabilityTypeName::Dictionary {
                if !source_path.split().contains(&&fruntime_dictionary_router_name.borrow())
                    && !source_path.split().contains(&&fsandbox_dictionary_router_name.borrow())
                {
                    Arc::downgrade(&sandbox.program_output_dict).into()
                } else {
                    Arc::downgrade(&sandbox.program_output_dict).into()
                }
            } else {
                Arc::downgrade(&sandbox.program_output_dict).into()
            }
        }
        fdecl::Ref::Child(child) => {
            let child_ref: cm_rust::ChildRef = child.clone().fidl_into_native();
            let child_name = moniker::ChildName::from(child_ref);
            let guard = sandbox.child_outputs.lock();
            let router = guard.get(&child_name).expect("reference to non-existent child");
            Arc::downgrade(&router).into()
        }
        fdecl::Ref::Collection(_) => unimplemented!(),
        fdecl::Ref::Framework(_) => Arc::downgrade(&*sandbox.framework_router.lock()).into(),
        fdecl::Ref::Capability(_) => {
            Arc::downgrade(&sandbox.capability_sourced_capabilities_dict).into()
        }
        fdecl::Ref::Debug(_) => {
            Arc::downgrade(&sandbox.component_input.environment().debug()).into()
        }
        fdecl::Ref::VoidType(_) => {
            let source_name = source_path.basename().expect("invalid source capability path");
            let type_name = request.build_type_name;
            return UnavailableRouter::new_from_type_name(source_name.into(), type_name, moniker);
        }
        fdecl::Ref::Environment(_) => {
            let type_name = request.build_type_name;
            match type_name {
                CapabilityTypeName::Runner => {
                    Arc::downgrade(&sandbox.component_input.environment().runners()).into()
                }
                CapabilityTypeName::Resolver => {
                    Arc::downgrade(&sandbox.component_input.environment().resolvers()).into()
                }
                _ => unreachable!("other capability types may not have an environment source"),
            }
        }
        _ => unreachable!("unexpected ref type"),
    };
    IntermediateRouter::new(source, source_path, request, default_token, moniker, verb, ref_)
}

fn install_use_in_sandbox<C: ComponentInstanceInterface + 'static>(
    component: &Arc<C>,
    sandbox: &ComponentSandbox,
    use_: &cm_rust::UseDecl,
    error_reporter: impl ErrorReporter,
) {
    let router = new_intermediate_router(component, sandbox, use_);
    let router = ErrorLoggingRouter::new(router, use_, error_reporter, component.as_weak().into());
    match use_ {
        cm_rust::UseDecl::Protocol(cm_rust::UseProtocolDecl {
            numbered_handle: Some(numbered_handle),
            ..
        }) => {
            let numbered_handle = Name::from(*numbered_handle);
            let prev = sandbox
                .program_input
                .numbered_handles()
                .insert_capability(&numbered_handle, router);
            assert!(prev.is_none(), "failed to insert {numbered_handle}: preexisting value");
        }
        cm_rust::UseDecl::Runner(_) => {
            assert!(
                sandbox.program_input.runner().is_none(),
                "component can't use multiple runners"
            );
            sandbox.program_input.set_runner(router.try_into().expect("invalid type for runner"));
        }
        cm_rust::UseDecl::Config(use_config) => {
            let prev =
                sandbox.program_input.config().insert_capability(&use_config.target_name, router);
            assert!(
                prev.is_none(),
                "failed to insert {}: preexisting value",
                use_config.target_name
            );
        }
        _ => {
            let prev =
                sandbox.program_input.namespace().insert_capability(use_.path().unwrap(), router);
            assert!(prev.is_none(), "failed to insert {}: preexisting value", use_.path().unwrap());
        }
    }
}

fn extend_dict_with_dictionary_use<C: ComponentInstanceInterface + 'static>(
    component: &Arc<C>,
    sandbox: &ComponentSandbox,
    use_bundle: Vec<&cm_rust::UseDecl>,
    error_reporter: impl ErrorReporter,
) {
    let path = use_bundle[0].path().unwrap();

    let original_dictionary = match sandbox.program_input.namespace().remove_capability(path) {
        Some(Capability::Dictionary(dictionary)) => dictionary,
        _ => Dictionary::new(),
    };

    let mut dictionary_routers = vec![];
    for use_ in use_bundle.iter() {
        install_use_in_sandbox(component, sandbox, use_, error_reporter.clone());
        let dictionary_router = match sandbox.program_input.namespace().remove_capability(path) {
            Some(Capability::DictionaryRouter(router)) => router,
            other_value => panic!("unexpected dictionary get result: {other_value:?}"),
        };
        dictionary_routers.push(dictionary_router);
    }

    let router = UseDictionaryRouter::new(
        path.clone(),
        component.moniker().clone(),
        original_dictionary,
        dictionary_routers,
        CapabilitySource::Component(ComponentSource {
            capability: ComponentCapability::Use_((*use_bundle.first().unwrap()).clone()),
            moniker: component.moniker().clone(),
        }),
    );
    // This value will be `Some` if we're shadowing something else. This is fine in this case
    // because we've already merged any preexisting value with what we're inserting.
    let _ = sandbox
        .program_input
        .namespace()
        .insert_capability(path, Capability::DictionaryRouter(router));
}

pub(crate) trait ServiceDeclExt {
    fn offer_service_decl(&self) -> Option<&cm_rust::OfferServiceDecl>;
    fn service_name(&self) -> Option<&Name>;
}

impl ServiceDeclExt for cm_rust::UseDecl {
    fn offer_service_decl(&self) -> Option<&cm_rust::OfferServiceDecl> {
        None
    }
    fn service_name(&self) -> Option<&Name> {
        match self {
            cm_rust::UseDecl::Service(s) => Some(&s.source_name),
            _ => None,
        }
    }
}

impl ServiceDeclExt for cm_rust::ExposeDecl {
    fn offer_service_decl(&self) -> Option<&cm_rust::OfferServiceDecl> {
        None
    }
    fn service_name(&self) -> Option<&Name> {
        match self {
            cm_rust::ExposeDecl::Service(s) => Some(&s.target_name),
            _ => None,
        }
    }
}

impl ServiceDeclExt for cm_rust::OfferDecl {
    fn offer_service_decl(&self) -> Option<&cm_rust::OfferServiceDecl> {
        match self {
            cm_rust::OfferDecl::Service(s) => Some(&s),
            _ => None,
        }
    }
    fn service_name(&self) -> Option<&Name> {
        match self {
            cm_rust::OfferDecl::Service(s) => Some(&s.target_name),
            _ => None,
        }
    }
}

fn install_offer_in_sandbox<C: ComponentInstanceInterface + 'static>(
    component: &Arc<C>,
    sandbox: &ComponentSandbox,
    offer: &cm_rust::offer::OfferDecl,
) {
    let intermediate_router = new_intermediate_router(component, sandbox, offer);
    install_router_to_target(
        sandbox,
        intermediate_router,
        offer.target().clone().native_into_fidl(),
        vec![offer.target_name().clone()].into(),
    );
}

fn install_router_to_target(
    sandbox: &ComponentSandbox,
    router: Capability,
    target: fdecl::Ref,
    target_path: RelativePath,
) {
    let target_dictionary = match target {
        fdecl::Ref::Parent(_) => sandbox.component_output.capabilities(),
        fdecl::Ref::Self_(_) => {
            unimplemented!("use is handled elsewhere");
        }
        fdecl::Ref::Child(child) => {
            let child_name =
                Name::new(child.name.as_str()).expect("child is static so name is not long");
            sandbox.child_inputs.get(&child_name).expect("invalid child ref").capabilities()
        }
        fdecl::Ref::Collection(collection) => {
            let collection_name = Name::new(collection.name.as_str()).unwrap();
            sandbox
                .collection_inputs
                .get(&collection_name)
                .expect("invalid collection ref")
                .capabilities()
        }
        fdecl::Ref::Framework(_) => sandbox.component_output.framework(),
        fdecl::Ref::Capability(capability) => {
            let capability_name = Name::new(capability.name.as_str()).unwrap();
            sandbox
                .declared_dictionaries
                .get(&capability_name)
                .expect("capability target doesn't exist")
                .try_into()
                .expect("unexpected capability type")
        }
        fdecl::Ref::Debug(_) => {
            unimplemented!("debug registrations are handled elsewhere");
        }
        fdecl::Ref::VoidType(_) => {
            unimplemented!("it's not possible to route to void, only from");
        }
        fdecl::Ref::Environment(_) => {
            unimplemented!("environment registrations are handled elsewhere");
        }
        _ => unreachable!("unexpected ref type"),
    };
    let prev = target_dictionary.insert_capability(&target_path, router);
    assert!(prev.is_none(), "failed to insert {target_path}: preexisting value");
}

fn new_intermediate_router_source_name<'a, C: ComponentInstanceInterface + 'static, D>(
    component: &Arc<C>,
    sandbox: &ComponentSandbox,
    decl: &'a D,
) -> Capability
where
    D: SourceName + ToSource + ToRequest,
    &'a D: Into<CapabilityTypeName> + Into<RouteVerb>,
{
    let source = decl.to_source();
    let source_path = RelativePath::from(vec![decl.source_name().clone()]);
    new_intermediate_router_inner(
        component.moniker().clone(),
        decl.into(),
        sandbox,
        decl.to_request(component.moniker()),
        component.as_weak().into(),
        decl.into(),
        source,
        source_path,
    )
}

fn new_intermediate_router<'a, C: ComponentInstanceInterface + 'static, D>(
    component: &Arc<C>,
    sandbox: &ComponentSandbox,
    decl: &'a D,
) -> Capability
where
    D: SourcePath + ToSource + ToRequest + ServiceDeclExt,
    &'a D: Into<CapabilityTypeName> + Into<RouteVerb>,
{
    let source = decl.to_source();
    let source_path = match &source {
        fdecl::Ref::Capability(fdecl::CapabilityRef { name })
            if decl.source_path().basename
                == &Name::new("fuchsia.component.StorageAdmin").unwrap()
                || decl.source_path().basename
                    == &Name::new("fuchsia.sys2.StorageAdmin").unwrap() =>
        {
            let mut path: RelativePath =
                decl.source_path().iter_segments().collect::<Vec<_>>().into();
            path = path.parent().unwrap_or(path);
            let not_too_long = path.push(Name::new(name).unwrap());
            assert!(not_too_long);
            path
        }
        _ => decl.source_path().iter_segments().collect::<Vec<_>>().into(),
    };
    let router = new_intermediate_router_inner(
        component.moniker().clone(),
        decl.into(),
        sandbox,
        decl.to_request(component.moniker()),
        component.as_weak().into(),
        decl.into(),
        source,
        source_path,
    );

    // Service capabilities are special, and we need to handle the filtering/renaming that they do
    // when offered.
    if let Some(offer_service_decl) = decl.offer_service_decl() {
        let Capability::DirConnectorRouter(r) = router else {
            panic!("wrong type returned for service capability");
        };
        r.with_service_renames_and_filter(cm_rust::OfferDecl::Service(Box::new(
            offer_service_decl.clone(),
        )))
    } else {
        router
    }
}

fn install_expose_in_sandbox<C: ComponentInstanceInterface + 'static>(
    component: &Arc<C>,
    sandbox: &ComponentSandbox,
    expose: &cm_rust::ExposeDecl,
) {
    let router = new_intermediate_router(component, sandbox, expose);
    install_router_to_target(
        sandbox,
        router,
        expose.target().clone().native_into_fidl(),
        vec![expose.target_name().clone()].into(),
    );
}

struct UnavailableRouter {
    capability: InternalCapability,
    moniker: Moniker,
}

impl UnavailableRouter {
    fn new<T: CapabilityBound>(capability: InternalCapability, moniker: Moniker) -> Arc<Router<T>> {
        Router::<T>::new(Self { capability, moniker })
    }

    fn new_from_type_name(
        name: Name,
        type_name: CapabilityTypeName,
        moniker: Moniker,
    ) -> Capability {
        match type_name {
            CapabilityTypeName::Service => {
                Self::new::<DirConnector>(InternalCapability::Service(name), moniker).into()
            }
            CapabilityTypeName::Protocol => {
                Self::new::<Connector>(InternalCapability::Protocol(name), moniker).into()
            }
            CapabilityTypeName::Directory => {
                Self::new::<DirConnector>(InternalCapability::Directory(name), moniker).into()
            }
            CapabilityTypeName::Storage => {
                Self::new::<DirConnector>(InternalCapability::Storage(name), moniker).into()
            }
            CapabilityTypeName::Runner => {
                Self::new::<Connector>(InternalCapability::Runner(name), moniker).into()
            }
            CapabilityTypeName::Resolver => {
                Self::new::<Connector>(InternalCapability::Resolver(name), moniker).into()
            }
            CapabilityTypeName::EventStream => Self::new::<Dictionary>(
                InternalCapability::EventStream(InternalEventStreamCapability {
                    name,
                    scope_moniker: None,
                    scope: None,
                }),
                moniker,
            )
            .into(),
            CapabilityTypeName::Dictionary => {
                Self::new::<Dictionary>(InternalCapability::Dictionary(name), moniker).into()
            }
            CapabilityTypeName::Config => {
                Self::new::<Data>(InternalCapability::Config(name), moniker).into()
            }
        }
    }
}

#[async_trait]
impl<T: CapabilityBound> Routable<T> for UnavailableRouter {
    async fn route(
        &self,
        request: fruntime::RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<T>>, RouterError> {
        let availability = request
            .availability
            .ok_or_else(|| RoutingError::RouteRequestMissingField {
                moniker: self.moniker.clone().into(),
                missing_field: "availability".to_string(),
            })?
            .fidl_into_native();
        match availability {
            cm_rust::Availability::Required => {
                Err(RoutingError::SourceCapabilityIsVoid { moniker: self.moniker.clone().into() }
                    .into())
            }
            cm_rust::Availability::Optional
            | cm_rust::Availability::Transitional
            | cm_rust::Availability::SameAsTarget => Ok(None),
        }
    }

    async fn route_debug(
        &self,
        request: fruntime::RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        match request.availability {
            Some(fdecl::Availability::Required) => {
                Err(RoutingError::SourceCapabilityIsVoid { moniker: self.moniker.clone().into() }
                    .into())
            }
            Some(fdecl::Availability::Optional)
            | Some(fdecl::Availability::Transitional)
            | Some(fdecl::Availability::SameAsTarget)
            | None => Ok(CapabilitySource::Void(VoidSource {
                capability: self.capability.clone(),
                moniker: self.moniker.clone(),
            })),
        }
    }

    fn error_info(&self) -> Option<RouterErrorInfo> {
        Some(RouterErrorInfo {
            capability_type: self.capability.type_name(),
            name: self.capability.source_name().clone(),
            availability: cm_rust::Availability::Optional,
        })
    }
}
