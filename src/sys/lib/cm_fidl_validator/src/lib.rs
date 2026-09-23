// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub(crate) mod util;

pub mod error;

pub use crate::util::check_url;

use crate::error::*;
use crate::util::*;
use cm_graph::DependencyNode;
use cm_types::IterablePath;
use directed_graph::DirectedGraph;
use fidl_fuchsia_component_decl as fdecl;
use itertools::Itertools;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

trait HasAvailability {
    fn availability(&self) -> fdecl::Availability;
}

impl HasAvailability for fdecl::ExposeService {
    fn availability(&self) -> fdecl::Availability {
        return self.availability.unwrap_or(fdecl::Availability::Required);
    }
}

impl HasAvailability for fdecl::OfferService {
    fn availability(&self) -> fdecl::Availability {
        return self.availability.unwrap_or(fdecl::Availability::Required);
    }
}

/// Validates Configuration Value Spec.
///
/// For now, this simply verifies that all semantically required fields are present.
pub fn validate_value_spec(spec: &fdecl::ConfigValueSpec) -> Result<(), ErrorList> {
    let mut errors = vec![];
    if let Some(value) = &spec.value {
        match value {
            fdecl::ConfigValue::Single(s) => match s {
                fdecl::ConfigSingleValue::Bool(_)
                | fdecl::ConfigSingleValue::Uint8(_)
                | fdecl::ConfigSingleValue::Uint16(_)
                | fdecl::ConfigSingleValue::Uint32(_)
                | fdecl::ConfigSingleValue::Uint64(_)
                | fdecl::ConfigSingleValue::Int8(_)
                | fdecl::ConfigSingleValue::Int16(_)
                | fdecl::ConfigSingleValue::Int32(_)
                | fdecl::ConfigSingleValue::Int64(_)
                | fdecl::ConfigSingleValue::String(_) => {}
                fdecl::ConfigSingleValueUnknown!() => {
                    errors.push(Error::invalid_field(DeclType::ConfigValueSpec, "value"));
                }
            },
            fdecl::ConfigValue::Vector(l) => match l {
                fdecl::ConfigVectorValue::BoolVector(_)
                | fdecl::ConfigVectorValue::Uint8Vector(_)
                | fdecl::ConfigVectorValue::Uint16Vector(_)
                | fdecl::ConfigVectorValue::Uint32Vector(_)
                | fdecl::ConfigVectorValue::Uint64Vector(_)
                | fdecl::ConfigVectorValue::Int8Vector(_)
                | fdecl::ConfigVectorValue::Int16Vector(_)
                | fdecl::ConfigVectorValue::Int32Vector(_)
                | fdecl::ConfigVectorValue::Int64Vector(_)
                | fdecl::ConfigVectorValue::StringVector(_) => {}
                fdecl::ConfigVectorValueUnknown!() => {
                    errors.push(Error::invalid_field(DeclType::ConfigValueSpec, "value"));
                }
            },
            fdecl::ConfigValueUnknown!() => {
                errors.push(Error::invalid_field(DeclType::ConfigValueSpec, "value"));
            }
        }
    } else {
        errors.push(Error::missing_field(DeclType::ConfigValueSpec, "value"));
    }

    if errors.is_empty() { Ok(()) } else { Err(ErrorList::new(errors)) }
}

/// Validates Configuration Values Data.
///
/// The Value Data may ultimately originate from a CVF file, or be directly constructed by the
/// caller. Either way, Value Data should always be validated before it's used. For now, this
/// simply verifies that all semantically required fields are present.
///
/// This method does not validate value data against a configuration schema.
pub fn validate_values_data(data: &fdecl::ConfigValuesData) -> Result<(), ErrorList> {
    let mut errors = vec![];
    if let Some(values) = &data.values {
        for spec in values {
            if let Err(mut e) = validate_value_spec(spec) {
                errors.append(&mut e.errs);
            }
        }
    } else {
        errors.push(Error::missing_field(DeclType::ConfigValuesData, "values"));
    }

    if let Some(checksum) = &data.checksum {
        match checksum {
            fdecl::ConfigChecksum::Sha256(_) => {}
            fdecl::ConfigChecksumUnknown!() => {
                errors.push(Error::invalid_field(DeclType::ConfigValuesData, "checksum"));
            }
        }
    } else {
        errors.push(Error::missing_field(DeclType::ConfigValuesData, "checksum"));
    }

    if errors.is_empty() { Ok(()) } else { Err(ErrorList::new(errors)) }
}

// `fdecl::Ref` is not hashable, so define this equivalent type for use in maps
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RefKey<'a> {
    Parent,
    Self_,
    Child(&'a str),
    Collection(&'a str),
    Framework,
    Capability,
    Debug,
}

/// Validates a Component.
///
/// The Component may ultimately originate from a CM file, or be directly constructed by the
/// caller. Either way, a Component should always be validated before it's used. Examples
/// of what is validated (which may evolve in the future):
///
/// - That all semantically required fields are present
/// - That a child_name referenced in a source actually exists in the list of children
/// - That there are no duplicate target paths.
/// - That only weak-dependency capabilities may be offered back to the
///   component that exposed them.
///
/// `dependencies` is an output parameter that captures the capability dependency graph derived
/// from `decl`. Callers may use this to traverse the capability graph in topological order.
///
/// All checks are local to this Component.
pub fn validate<'a>(
    decl: &'a fdecl::Component,
    dependencies: &'a mut DirectedGraph<DependencyNode>,
) -> Result<(), ErrorList> {
    let ctx = ValidationContext::new(dependencies);
    ctx.validate(decl, &[]).map_err(|errs| ErrorList::new(errs))
}

/// Validates a list of namespace or builtin Capabilities.
fn validate_capabilities(
    capabilities: &[fdecl::Capability],
    as_builtin: bool,
) -> Result<(), ErrorList> {
    let mut deps = DirectedGraph::new();
    let mut ctx = ValidationContext::new(&mut deps);

    ctx.load_dictionary_names(capabilities.iter().filter_map(|capability| match capability {
        fdecl::Capability::Dictionary(dictionary_decl) => Some(dictionary_decl),
        _ => None,
    }));

    ctx.validate_capability_decls(capabilities, as_builtin);
    if ctx.errors.is_empty() { Ok(()) } else { Err(ErrorList::new(ctx.errors)) }
}

// Validate builtin capabilities.
pub fn validate_builtin_capabilities(
    capabilities: &Vec<fdecl::Capability>,
) -> Result<(), ErrorList> {
    validate_capabilities(capabilities, true)
}

// Validate namespace capabilities.
pub fn validate_namespace_capabilities(
    capabilities: &Vec<fdecl::Capability>,
) -> Result<(), ErrorList> {
    validate_capabilities(capabilities, false)
}

/// An interface to call into either `check_dynamic_name()` or `check_name()`, depending on the context
/// of the caller.
type CheckChildNameFn = fn(Option<&String>, DeclType, &str, &mut Vec<Error>) -> bool;

pub fn validate_dynamic_child(child: &fdecl::Child) -> Result<(), ErrorList> {
    let mut errors = vec![];

    if let Err(mut error_list) = validate_child(child, check_dynamic_name) {
        errors.append(&mut error_list.errs);
    }

    if child.environment.is_some() {
        errors.push(Error::DynamicChildWithEnvironment);
    }

    if errors.is_empty() { Ok(()) } else { Err(ErrorList { errs: errors }) }
}

/// Validates an independent Child. Performs the same validation on it as `validate`. A
/// `check_name_fn` is passed into specify the function used to validate the child name.
fn validate_child(
    child: &fdecl::Child,
    check_child_name: CheckChildNameFn,
) -> Result<(), ErrorList> {
    let mut errors = vec![];
    check_child_name(child.name.as_ref(), DeclType::Child, "name", &mut errors);
    check_url(child.url.as_ref(), DeclType::Child, "url", &mut errors);
    if child.startup.is_none() {
        errors.push(Error::missing_field(DeclType::Child, "startup"));
    }
    // Allow `on_terminate` to be unset since the default is almost always desired.
    if child.environment.is_some() {
        check_name(child.environment.as_ref(), DeclType::Child, "environment", &mut errors);
    }
    if errors.is_empty() { Ok(()) } else { Err(ErrorList { errs: errors }) }
}

/// Validates a collection of dynamic offers. Dynamic offers differ from static
/// offers, in that
///
/// 1. a dynamic offer's `target` field must be omitted;
/// 2. a dynamic offer's `source` _may_ be a dynamic child;
/// 3. since this crate isn't really designed to handle dynamic children, we
///    disable the checks that ensure that the source/target exist, and that the
///    offers don't introduce any cycles.
///
/// `dependencies` is an output parameter that captures new dependencies from `new_dynamic_offers`.
/// Any existing dependency edges are preserved. The input may be non empty and is normally the
/// output of the previous [`validate`] or [`validate_dynamic_offers`].
pub fn validate_dynamic_offers<'a>(
    dynamic_children: Vec<(&'a str, &'a str)>,
    dependencies: &mut DirectedGraph<DependencyNode>,
    new_dynamic_offers: &'a [fdecl::Offer],
    decl: &'a fdecl::Component,
) -> Result<(), ErrorList> {
    let mut ctx = ValidationContext::new(dependencies);
    ctx.dynamic_children = dynamic_children;
    ctx.validate(decl, new_dynamic_offers).map_err(|errs| ErrorList::new(errs))
}

fn check_offer_name(
    prop: Option<&String>,
    decl: DeclType,
    keyword: &str,
    offer_type: OfferType,
    errors: &mut Vec<Error>,
) -> bool {
    if offer_type == OfferType::Dynamic {
        check_dynamic_name(prop, decl, keyword, errors)
    } else {
        check_name(prop, decl, keyword, errors)
    }
}

struct ValidationContext<'a> {
    all_children: HashMap<&'a str, &'a fdecl::Child>,
    all_collections: HashSet<&'a str>,
    all_capability_ids: HashSet<&'a str>,
    all_storages: HashMap<&'a str, Option<&'a fdecl::Ref>>,
    all_services: HashSet<&'a str>,
    all_protocols: HashSet<&'a str>,
    all_directories: HashSet<&'a str>,
    all_runners: HashSet<&'a str>,
    all_resolvers: HashSet<&'a str>,
    all_dictionaries: HashMap<&'a str, &'a fdecl::Dictionary>,

    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    all_configs: HashSet<&'a str>,

    all_environment_names: HashSet<&'a str>,
    dynamic_children: Vec<(&'a str, &'a str)>,
    strong_dependencies: &'a mut DirectedGraph<DependencyNode>,
    target_ids: IdMap<'a>,
    errors: Vec<Error>,
}

/// [Container] provides a capability type agnostic trait to check for the existence of a
/// capability definition of a particular type. This is useful for writing common validation
/// functions.
trait Container {
    fn contains(&self, key: &str) -> bool;
}

impl<'a> Container for HashSet<&'a str> {
    fn contains(&self, key: &str) -> bool {
        self.contains(key)
    }
}

impl<'a, T> Container for HashMap<&'a str, T> {
    fn contains(&self, key: &str) -> bool {
        self.contains_key(key)
    }
}

impl<'a> ValidationContext<'a> {
    fn new(strong_dependencies: &'a mut DirectedGraph<DependencyNode>) -> Self {
        Self {
            strong_dependencies,
            all_children: Default::default(),
            all_collections: Default::default(),
            all_capability_ids: Default::default(),
            all_storages: Default::default(),
            all_services: Default::default(),
            all_protocols: Default::default(),
            all_directories: Default::default(),
            all_runners: Default::default(),
            all_resolvers: Default::default(),
            all_dictionaries: Default::default(),

            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            all_configs: Default::default(),

            all_environment_names: Default::default(),
            dynamic_children: Default::default(),
            target_ids: Default::default(),
            errors: Default::default(),
        }
    }

    fn validate(
        mut self,
        decl: &'a fdecl::Component,
        new_dynamic_offers: &'a [fdecl::Offer],
    ) -> Result<(), Vec<Error>> {
        // Collect all environment names first, so that references to them can be checked.
        if let Some(envs) = &decl.environments {
            self.collect_environment_names(&envs);
        }

        // Validate "children" and build the set of all children.
        if let Some(children) = decl.children.as_ref() {
            for child in children {
                self.validate_child_decl(&child);
            }
        }

        // Validate "collections" and build the set of all collections.
        if let Some(collections) = decl.collections.as_ref() {
            for collection in collections {
                self.validate_collection_decl(&collection);
            }
        }

        // Validate "capabilities" and build the set of all capabilities.
        if let Some(capabilities) = decl.capabilities.as_ref() {
            self.load_dictionary_names(capabilities.iter().filter_map(
                |capability| match capability {
                    fdecl::Capability::Dictionary(dictionary_decl) => Some(dictionary_decl),
                    _ => None,
                },
            ));
            self.validate_capability_decls(capabilities, false);
        }

        // Validate "uses".
        let mut use_runner_name = None;
        let mut use_runner_source = None;
        if let Some(uses) = decl.uses.as_ref() {
            (use_runner_name, use_runner_source) = self.validate_use_decls(uses);
        }

        // Validate "program".
        if let Some(program) = decl.program.as_ref() {
            self.validate_program(program, use_runner_name, use_runner_source);
        }

        // Validate "exposes".
        if let Some(exposes) = decl.exposes.as_ref() {
            let mut expose_to_parent_ids = HashMap::new();
            let mut expose_to_framework_ids = HashMap::new();
            for expose in exposes.iter() {
                self.validate_expose_decl(
                    &expose,
                    &mut expose_to_parent_ids,
                    &mut expose_to_framework_ids,
                );
            }
            self.validate_expose_group(&exposes);
        }

        // Validate "offers".
        if let Some(offers) = decl.offers.as_ref() {
            for offer in offers.iter() {
                self.validate_offer_decl(&offer, OfferType::Static);
            }
            self.validate_offer_group(&offers, OfferType::Static);
        }

        for dynamic_offer in new_dynamic_offers {
            self.validate_offer_decl(dynamic_offer, OfferType::Dynamic);
            cm_graph::add_dependencies_from_offer(
                &mut self.strong_dependencies,
                dynamic_offer,
                &self.dynamic_children,
            );
        }
        self.validate_offer_group(new_dynamic_offers, OfferType::Dynamic);

        // Validate "environments" after all other declarations are processed.
        if let Some(environment) = decl.environments.as_ref() {
            for environment in environment {
                self.validate_environment_decl(&environment);
            }
        }

        // Validate "config"
        self.validate_config(decl.config.as_ref(), decl.uses.as_ref());

        // Check that there are no strong cyclical dependencies
        cm_graph::generate_dependency_graph(
            &mut self.strong_dependencies,
            &decl,
            &self.dynamic_children,
        );
        if let Err(e) = self.strong_dependencies.topological_sort() {
            self.errors.push(Error::dependency_cycle(e.format_cycle()));
        }

        if self.errors.is_empty() { Ok(()) } else { Err(self.errors) }
    }

    /// Collects all the environment names, watching for duplicates.
    fn collect_environment_names(&mut self, envs: &'a [fdecl::Environment]) {
        for env in envs {
            if let Some(name) = env.name.as_ref() {
                if !self.all_environment_names.insert(name) {
                    self.errors.push(Error::duplicate_field(DeclType::Environment, "name", name));
                }
            }
        }
    }

    // Validates a config schema. Checks that each field's layout matches the expected constraints
    // and properties.
    fn validate_config(
        &mut self,
        config: Option<&fdecl::ConfigSchema>,
        uses: Option<&Vec<fdecl::Use>>,
    ) {
        use std::collections::BTreeMap;

        // Get all of the `use` configs that are optional without a default.
        let optional_use_keys: BTreeMap<String, fdecl::ConfigType> =
            uses.map_or(BTreeMap::new(), |u| {
                u.iter()
                    .map(|u| {
                        let fdecl::Use::Config(config) = u else {
                            return None;
                        };
                        if config.availability == Some(fdecl::Availability::Required)
                            || config.availability == None
                        {
                            return None;
                        }
                        if let Some(_) = config.default.as_ref() {
                            return None;
                        }
                        let Some(key) = config.target_name.clone() else {
                            return None;
                        };
                        let Some(value) = config.type_.clone() else {
                            return None;
                        };
                        Some((key, value))
                    })
                    .flatten()
                    .collect()
            });

        // Validate default values in use configs.
        for u in uses.iter().flat_map(|x| x.iter()) {
            let fdecl::Use::Config(config) = u else { continue };
            let Some(default) = config.default.as_ref() else { continue };
            validate_value_spec(&fdecl::ConfigValueSpec {
                value: Some(default.clone()),
                ..Default::default()
            })
            .map_err(|mut e| self.errors.append(&mut e.errs))
            .ok();
        }

        let Some(config) = config else {
            if !optional_use_keys.is_empty() {
                self.errors.push(Error::missing_field(DeclType::ConfigField, "config"))
            }
            return;
        };

        if let Some(fields) = &config.fields {
            for field in fields {
                if field.key.is_none() {
                    self.errors.push(Error::missing_field(DeclType::ConfigField, "key"));
                }
                if let Some(type_) = &field.type_ {
                    self.validate_config_type(type_, true);
                } else {
                    self.errors.push(Error::missing_field(DeclType::ConfigField, "value_type"));
                }
            }
        } else {
            self.errors.push(Error::missing_field(DeclType::ConfigSchema, "fields"));
        }

        if let Some(checksum) = &config.checksum {
            match checksum {
                fdecl::ConfigChecksum::Sha256(_) => {}
                fdecl::ConfigChecksumUnknown!() => {
                    self.errors.push(Error::invalid_field(DeclType::ConfigSchema, "checksum"));
                }
            }
        } else {
            self.errors.push(Error::missing_field(DeclType::ConfigSchema, "checksum"));
        }

        'outer: for (key, value) in optional_use_keys.iter() {
            for field in config.fields.iter().flatten() {
                if field.key.as_ref() == Some(key) {
                    if field.type_.as_ref() != Some(value) {
                        self.errors.push(Error::invalid_field(DeclType::ConfigField, key.clone()));
                    }
                    continue 'outer;
                }
            }
            self.errors.push(Error::missing_field(DeclType::ConfigField, key.clone()));
        }

        match config.value_source {
            None => self.errors.push(Error::missing_field(DeclType::ConfigSchema, "value_source")),
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            Some(fdecl::ConfigValueSource::Capabilities(_)) => {
                if !optional_use_keys.is_empty() {
                    self.errors
                        .push(Error::invalid_field(DeclType::ConfigValueSource, "ValueSource"))
                }
            }
            Some(fdecl::ConfigValueSource::__SourceBreaking { .. }) => {
                self.errors.push(Error::invalid_field(DeclType::ConfigValueSource, "ValueSource"))
            }
            _ => (),
        };
    }

    fn validate_config_type(&mut self, type_: &fdecl::ConfigType, accept_vectors: bool) {
        match &type_.layout {
            fdecl::ConfigTypeLayout::Bool
            | fdecl::ConfigTypeLayout::Uint8
            | fdecl::ConfigTypeLayout::Uint16
            | fdecl::ConfigTypeLayout::Uint32
            | fdecl::ConfigTypeLayout::Uint64
            | fdecl::ConfigTypeLayout::Int8
            | fdecl::ConfigTypeLayout::Int16
            | fdecl::ConfigTypeLayout::Int32
            | fdecl::ConfigTypeLayout::Int64 => {
                // These layouts have no parameters or constraints
                if let Some(parameters) = &type_.parameters {
                    if !parameters.is_empty() {
                        self.errors
                            .push(Error::extraneous_field(DeclType::ConfigType, "parameters"));
                    }
                } else {
                    self.errors.push(Error::missing_field(DeclType::ConfigType, "parameters"));
                }

                if !type_.constraints.is_empty() {
                    self.errors.push(Error::extraneous_field(DeclType::ConfigType, "constraints"));
                }
            }
            fdecl::ConfigTypeLayout::String => {
                // String has exactly one constraint and no parameter
                if let Some(parameters) = &type_.parameters {
                    if !parameters.is_empty() {
                        self.errors
                            .push(Error::extraneous_field(DeclType::ConfigType, "parameters"));
                    }
                } else {
                    self.errors.push(Error::missing_field(DeclType::ConfigType, "parameters"));
                }

                if type_.constraints.is_empty() {
                    self.errors.push(Error::missing_field(DeclType::ConfigType, "constraints"));
                } else if type_.constraints.len() > 1 {
                    self.errors.push(Error::extraneous_field(DeclType::ConfigType, "constraints"));
                } else if let fdecl::LayoutConstraint::MaxSize(_) = &type_.constraints[0] {
                } else {
                    self.errors.push(Error::invalid_field(DeclType::ConfigType, "constraints"));
                }
            }
            fdecl::ConfigTypeLayout::Vector => {
                if accept_vectors {
                    // Vector has exactly one constraint and one parameter
                    if let Some(parameters) = &type_.parameters {
                        if parameters.is_empty() {
                            self.errors
                                .push(Error::missing_field(DeclType::ConfigType, "parameters"));
                        } else if parameters.len() > 1 {
                            self.errors
                                .push(Error::extraneous_field(DeclType::ConfigType, "parameters"));
                        } else if let fdecl::LayoutParameter::NestedType(nested_type) =
                            &parameters[0]
                        {
                            self.validate_config_type(nested_type, false);
                        } else {
                            self.errors
                                .push(Error::invalid_field(DeclType::ConfigType, "parameters"));
                        }
                    } else {
                        self.errors.push(Error::missing_field(DeclType::ConfigType, "parameters"))
                    }

                    if type_.constraints.is_empty() {
                        self.errors.push(Error::missing_field(DeclType::ConfigType, "constraints"));
                    } else if type_.constraints.len() > 1 {
                        self.errors
                            .push(Error::extraneous_field(DeclType::ConfigType, "constraints"));
                    } else if let fdecl::LayoutConstraint::MaxSize(_) = &type_.constraints[0] {
                    } else {
                        self.errors.push(Error::invalid_field(DeclType::ConfigType, "constraints"));
                    }
                } else {
                    self.errors.push(Error::nested_vector());
                }
            }
            _ => self.errors.push(Error::invalid_field(DeclType::ConfigType, "layout")),
        }
    }

    fn validate_capability_decls(
        &mut self,
        capabilities: &'a [fdecl::Capability],
        as_builtin: bool,
    ) {
        for capability in capabilities {
            self.validate_capability_decl(capability, as_builtin);
        }
    }

    /// Validates an individual capability declaration as either a built-in capability or (if
    /// `as_builtin = false`) as a component or namespace capability.
    // Storage capabilities are not currently allowed as built-ins, but there's no deep reason for this.
    // Update this method to allow built-in storage capabilities as needed.
    fn validate_capability_decl(&mut self, capability: &'a fdecl::Capability, as_builtin: bool) {
        match capability {
            fdecl::Capability::Service(service) => self.validate_service_decl(&service, as_builtin),
            fdecl::Capability::Protocol(protocol) => {
                self.validate_protocol_decl(&protocol, as_builtin)
            }
            fdecl::Capability::Directory(directory) => {
                self.validate_directory_decl(&directory, as_builtin)
            }
            fdecl::Capability::Storage(storage) => {
                if as_builtin {
                    self.errors.push(Error::CapabilityCannotBeBuiltin(DeclType::Storage))
                } else {
                    self.validate_storage_decl(&storage)
                }
            }
            fdecl::Capability::Runner(runner) => self.validate_runner_decl(&runner, as_builtin),
            fdecl::Capability::Resolver(resolver) => {
                self.validate_resolver_decl(&resolver, as_builtin)
            }
            fdecl::Capability::EventStream(event) => {
                if as_builtin {
                    self.validate_event_stream_decl(&event)
                } else {
                    self.errors.push(Error::CapabilityMustBeBuiltin(DeclType::EventStream))
                }
            }
            fdecl::Capability::Dictionary(dictionary) => {
                self.validate_dictionary_decl(&dictionary);
            }
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            fdecl::Capability::Config(config) => {
                self.validate_configuration_decl(&config);
            }
            fdecl::CapabilityUnknown!() => self.errors.push(Error::UnknownCapability),
        }
    }

    /// Returns the `source_name` and `source` of the runner in `uses`, if present.
    fn validate_use_decls(
        &mut self,
        uses: &'a [fdecl::Use],
    ) -> (Option<&'a String>, Option<&'a fdecl::Ref>) {
        // Validate individual fields.
        for use_ in uses.iter() {
            self.validate_use_decl(&use_);
        }
        self.validate_use_paths(&uses);

        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        {
            let mut use_runner_name = None;
            let mut use_runner_source = None;
            for use_ in uses.iter() {
                if let fdecl::Use::Runner(use_runner) = use_ {
                    if use_runner_name.is_some() {
                        self.errors.push(Error::MultipleRunnersUsed);
                    }

                    use_runner_name = use_runner.source_name.as_ref();
                    use_runner_source = use_runner.source.as_ref();
                }
            }
            return (use_runner_name, use_runner_source);
        }
        #[cfg(fuchsia_api_level_less_than = "HEAD")]
        return (None, None);
    }

    fn validate_use_decl(&mut self, use_: &'a fdecl::Use) {
        match use_ {
            fdecl::Use::Service(u) => {
                let decl = DeclType::UseService;
                self.validate_use_fields(
                    decl,
                    Self::service_checker,
                    u.source.as_ref(),
                    u.source_name.as_ref(),
                    u.source_dictionary.as_ref(),
                    u.target_path.as_ref(),
                    u.dependency_type.as_ref(),
                    u.availability.as_ref(),
                );
                if u.dependency_type.is_none() {
                    self.errors.push(Error::missing_field(decl, "dependency_type"));
                }
            }
            fdecl::Use::Protocol(u) => {
                let decl = DeclType::UseProtocol;
                self.validate_use_fields(
                    decl,
                    Self::protocol_checker,
                    u.source.as_ref(),
                    u.source_name.as_ref(),
                    u.source_dictionary.as_ref(),
                    u.target_path.as_ref(),
                    u.dependency_type.as_ref(),
                    u.availability.as_ref(),
                );
                #[cfg(not(fuchsia_api_level_at_least = "29"))]
                let has_numbered_handle = false;
                #[cfg(fuchsia_api_level_at_least = "29")]
                let has_numbered_handle = u.numbered_handle.is_some();
                if u.target_path.is_none() && !has_numbered_handle {
                    self.errors.push(Error::missing_field(decl, "target_path"));
                }
                if has_numbered_handle && u.target_path.is_some() {
                    self.errors.push(Error::extraneous_field(decl, "numbered_handle"));
                }
                if u.dependency_type.is_none() {
                    self.errors.push(Error::missing_field(decl, "dependency_type"));
                }
            }
            fdecl::Use::Directory(u) => {
                let decl = DeclType::UseDirectory;
                self.validate_use_fields(
                    decl,
                    Self::directory_checker,
                    u.source.as_ref(),
                    u.source_name.as_ref(),
                    u.source_dictionary.as_ref(),
                    u.target_path.as_ref(),
                    u.dependency_type.as_ref(),
                    u.availability.as_ref(),
                );
                if u.dependency_type.is_none() {
                    self.errors.push(Error::missing_field(decl, "dependency_type"));
                }
                if u.rights.is_none() {
                    self.errors.push(Error::missing_field(DeclType::UseDirectory, "rights"));
                }
                if let Some(subdir) = u.subdir.as_ref() {
                    check_relative_path(
                        Some(subdir),
                        DeclType::UseDirectory,
                        "subdir",
                        &mut self.errors,
                    );
                }
            }
            fdecl::Use::Storage(u) => {
                const SOURCE: Option<fdecl::Ref> = Some(fdecl::Ref::Parent(fdecl::ParentRef {}));
                const DEPENDENCY_TYPE: Option<fdecl::DependencyType> =
                    Some(fdecl::DependencyType::Strong);
                self.validate_use_fields(
                    DeclType::UseStorage,
                    Self::storage_checker,
                    SOURCE.as_ref(),
                    u.source_name.as_ref(),
                    None,
                    u.target_path.as_ref(),
                    DEPENDENCY_TYPE.as_ref(),
                    u.availability.as_ref(),
                );
            }
            fdecl::Use::EventStream(u) => {
                const DEPENDENCY_TYPE: Option<fdecl::DependencyType> =
                    Some(fdecl::DependencyType::Strong);
                let decl = DeclType::UseEventStream;
                self.validate_use_fields(
                    decl,
                    Self::event_stream_checker,
                    u.source.as_ref(),
                    u.source_name.as_ref(),
                    None,
                    u.target_path.as_ref(),
                    DEPENDENCY_TYPE.as_ref(),
                    u.availability.as_ref(),
                );
                // Additional validation.
                match u.source {
                    Some(fdecl::Ref::Child(_)) | Some(fdecl::Ref::Parent(_)) => {
                        // Allowed.
                    }
                    Some(fdecl::Ref::Framework(_))
                    | Some(fdecl::Ref::Self_(_))
                    | Some(fdecl::Ref::Debug(_)) => {
                        // Allowed in general but not for event streams, add an error.
                        self.errors.push(Error::invalid_field(decl, "source"));
                    }
                    Some(fdecl::Ref::Collection(_)) | Some(fdecl::RefUnknown!()) | None => {
                        // Already handled by validate_use_fields.
                    }
                }
                if let Some(scope) = &u.scope {
                    for reference in scope {
                        if !matches!(reference, fdecl::Ref::Child(_) | fdecl::Ref::Collection(_)) {
                            self.errors.push(Error::invalid_field(decl, "scope"));
                        }
                    }
                }
            }
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            fdecl::Use::Runner(u) => {
                const DEPENDENCY_TYPE: Option<fdecl::DependencyType> =
                    Some(fdecl::DependencyType::Strong);
                const AVAILABILITY: Option<fdecl::Availability> =
                    Some(fdecl::Availability::Required);
                let decl = DeclType::UseRunner;
                self.validate_use_fields(
                    decl,
                    Self::runner_checker,
                    u.source.as_ref(),
                    u.source_name.as_ref(),
                    u.source_dictionary.as_ref(),
                    None,
                    DEPENDENCY_TYPE.as_ref(),
                    AVAILABILITY.as_ref(),
                );
            }
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            fdecl::Use::Config(u) => {
                const DEPENDENCY_TYPE: Option<fdecl::DependencyType> =
                    Some(fdecl::DependencyType::Strong);
                let decl = DeclType::UseConfiguration;
                self.validate_use_fields(
                    decl,
                    Self::config_checker,
                    u.source.as_ref(),
                    u.source_name.as_ref(),
                    None,
                    None,
                    DEPENDENCY_TYPE.as_ref(),
                    u.availability.as_ref(),
                );
            }
            #[cfg(fuchsia_api_level_at_least = "29")]
            fdecl::Use::Dictionary(u) => {
                let decl = DeclType::UseDictionary;
                self.validate_use_fields(
                    decl,
                    Self::dictionary_checker,
                    u.source.as_ref(),
                    u.source_name.as_ref(),
                    u.source_dictionary.as_ref(),
                    u.target_path.as_ref(),
                    u.dependency_type.as_ref(),
                    u.availability.as_ref(),
                );
                if u.dependency_type.is_none() {
                    self.errors.push(Error::missing_field(decl, "dependency_type"));
                }
            }
            fdecl::UseUnknown!() => {
                self.errors.push(Error::invalid_field(DeclType::Component, "use"));
            }
        }
    }

    /// Validates the "program" declaration. This does not check runner-specific properties
    /// since those are checked by the runner.
    fn validate_program(
        &mut self,
        program: &fdecl::Program,
        use_runner_name: Option<&String>,
        _use_runner_source: Option<&fdecl::Ref>,
    ) {
        match &program.runner {
            Some(_) =>
            {
                #[cfg(fuchsia_api_level_at_least = "HEAD")]
                if use_runner_name.is_some() {
                    if use_runner_name != program.runner.as_ref()
                        || _use_runner_source
                            != Some(&fdecl::Ref::Environment(fdecl::EnvironmentRef))
                    {
                        self.errors.push(Error::ConflictingRunners);
                    }
                }
            }
            None => {
                if use_runner_name.is_none() {
                    self.errors.push(Error::MissingRunner);
                }
            }
        }

        if program.info.is_none() {
            self.errors.push(Error::missing_field(DeclType::Program, "info"));
        }
    }

    /// Validates that paths-based capabilities (service, directory, protocol, etc.)
    /// are different, are not prefixes of each other, and do not collide "/pkg".
    fn validate_use_paths(&mut self, uses: &[fdecl::Use]) {
        fn use_to_path<'a>(use_: &'a fdecl::Use) -> Option<&'a Path> {
            match use_ {
                #[cfg(fuchsia_api_level_at_least = "29")]
                fdecl::Use::Dictionary(fdecl::UseDictionary {
                    target_path: Some(path), ..
                }) => Some(Path::new(path)),
                fdecl::Use::Directory(fdecl::UseDirectory { target_path: Some(path), .. }) => {
                    Some(Path::new(path))
                }
                fdecl::Use::Protocol(fdecl::UseProtocol { target_path: Some(path), .. }) => {
                    Some(Path::new(path))
                }
                fdecl::Use::Service(fdecl::UseService { target_path: Some(path), .. }) => {
                    Some(Path::new(path))
                }
                fdecl::Use::Storage(fdecl::UseStorage { target_path: Some(path), .. }) => {
                    Some(Path::new(path))
                }
                _ => None,
            }
        }
        enum NodeType {
            Service,
            Directory,
            // This variant is never constructed if we're at an API version before "use dictionary"
            // was added.
            #[allow(unused)]
            Dictionary,
        }
        fn use_to_type(use_: &fdecl::Use) -> Option<NodeType> {
            match use_ {
                fdecl::Use::Config(_) => None,
                #[cfg(fuchsia_api_level_at_least = "29")]
                fdecl::Use::Dictionary(_) => Some(NodeType::Dictionary),
                fdecl::Use::Directory(_) => Some(NodeType::Directory),
                fdecl::Use::EventStream(_) => Some(NodeType::Service),
                fdecl::Use::Protocol(_) => Some(NodeType::Service),
                #[cfg(fuchsia_api_level_at_least = "HEAD")]
                fdecl::Use::Runner(_) => None,
                fdecl::Use::Service(_) => Some(NodeType::Directory),
                fdecl::Use::Storage(_) => Some(NodeType::Directory),
                _ => None,
            }
        }
        let mut paths_and_uses =
            uses.iter().filter_map(|use_| use_to_path(use_).map(|x| (x, use_))).collect::<Vec<_>>();
        let mut duplicate_fields = false;
        while let Some((path, use_)) = paths_and_uses.pop() {
            if paths_and_uses.iter().any(|(path_from_iter, use_from_iter)| {
                path == *path_from_iter
                    && !(DeclType::from(*use_from_iter) == DeclType::UseDictionary
                        && DeclType::from(use_) == DeclType::UseDictionary)
            }) {
                self.errors.push(Error::duplicate_field(
                    use_.into(),
                    "target_path",
                    path.to_str().unwrap(),
                ));
                duplicate_fields = true;
            }
        }
        if duplicate_fields {
            // When paths are identical instead of merely prefixes of each other, then the above is
            // a more ergonomic and easy to understand error. The logic below will also fire for
            return;
        }
        for (use_a, use_b) in uses.iter().tuple_combinations() {
            let Some(path_a) = use_to_path(use_a) else {
                continue;
            };
            let Some(path_b) = use_to_path(use_b) else {
                continue;
            };
            let Some(type_a) = use_to_type(use_a) else {
                continue;
            };
            let Some(type_b) = use_to_type(use_b) else {
                continue;
            };
            let mut conflicts = false;
            match (type_a, type_b) {
                (NodeType::Service, NodeType::Service)
                | (NodeType::Directory, NodeType::Service)
                | (NodeType::Service, NodeType::Directory)
                | (NodeType::Directory, NodeType::Directory) => {
                    if path_a.starts_with(path_b) || path_b.starts_with(path_a) {
                        conflicts = true;
                    }
                }
                (NodeType::Dictionary, NodeType::Service)
                | (NodeType::Dictionary, NodeType::Directory) => {
                    if path_a.starts_with(path_b) {
                        conflicts = true;
                    }
                }
                (NodeType::Service, NodeType::Dictionary)
                | (NodeType::Directory, NodeType::Dictionary) => {
                    if path_b.starts_with(path_a) {
                        conflicts = true;
                    }
                }
                (NodeType::Dictionary, NodeType::Dictionary) => {
                    // All combinations of two dictionaries are valid.
                }
            }
            if conflicts {
                self.errors.push(Error::invalid_path_overlap(
                    use_a.into(),
                    path_a.to_str().unwrap(),
                    use_b.into(),
                    path_b.to_str().unwrap(),
                ));
            }
        }
        for use_ in uses {
            let Some(used_path) = use_to_path(use_) else {
                continue;
            };
            if used_path.starts_with(Path::new("/pkg")) {
                self.errors.push(Error::pkg_path_overlap(use_.into(), used_path.to_str().unwrap()));
            }
        }
    }

    fn validate_use_fields(
        &mut self,
        decl: DeclType,
        // This takes a callback that returns a [Container], instead of the &[Container] directly,
        // to avoid a borrow checker error that would occur from a simultaneous borrow on
        // &mut self.
        capability_checker: impl Fn(&Self) -> &dyn Container,
        source: Option<&'a fdecl::Ref>,
        source_name: Option<&'a String>,
        source_dictionary: Option<&'a String>,
        target_path: Option<&'a String>,
        dependency_type: Option<&fdecl::DependencyType>,
        availability: Option<&'a fdecl::Availability>,
    ) {
        self.validate_use_source(decl, source, source_dictionary);

        check_name(source_name, decl, "source_name", &mut self.errors);
        if source_dictionary.is_some() {
            check_relative_path(source_dictionary, decl, "source_dictionary", &mut self.errors);
        }
        match decl {
            DeclType::UseRunner | DeclType::UseConfiguration => {}
            #[cfg(fuchsia_api_level_at_least = "29")]
            DeclType::UseProtocol => {
                if target_path.is_some() {
                    check_path(target_path, decl, "target_path", &mut self.errors);
                } else {
                    // Expect this use to contain numbered_handle, verified by the caller.
                }
            }
            _ => {
                check_path(target_path, decl, "target_path", &mut self.errors);
            }
        }
        check_use_availability(decl, availability, &mut self.errors);

        // Only allow `weak` dependency with `use from child`.
        let is_use_from_child = match source {
            Some(fdecl::Ref::Child(_)) => true,
            _ => false,
        };
        match (is_use_from_child, dependency_type) {
            (false, Some(fdecl::DependencyType::Weak)) => {
                self.errors.push(Error::invalid_field(decl, "dependency_type"));
            }
            _ => {}
        }

        self.validate_route_from_self(
            decl,
            source,
            source_name,
            source_dictionary,
            capability_checker,
        );
    }

    fn validate_use_source(
        &mut self,
        decl: DeclType,
        source: Option<&'a fdecl::Ref>,
        source_dictionary: Option<&'a String>,
    ) {
        match (source, source_dictionary) {
            // These sources support source_dictionary.
            (Some(fdecl::Ref::Parent(_)), _) => {}
            (Some(fdecl::Ref::Self_(_)), _) => {}
            (Some(fdecl::Ref::Child(child)), _) => {
                self.validate_child_ref(decl, "source", &child, OfferType::Static);
                return;
            }
            // These sources don't.
            (Some(fdecl::Ref::Framework(_)), None) => {}
            (Some(fdecl::Ref::Debug(_)), None) => {}
            (Some(fdecl::Ref::Capability(c)), None) => {
                self.validate_source_capability(&c, decl, "source");
                return;
            }
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            (Some(fdecl::Ref::Environment(_)), None) => {}
            (Some(fdecl::Ref::Collection(collection)), None) if decl == DeclType::UseService => {
                self.validate_collection_ref(decl, "source", &collection);
                return;
            }
            // `source` is required.
            (None, _) => self.errors.push(Error::missing_field(decl, "source")),
            // Any combination that was not recognized above must be invalid.
            (_, _) => self.errors.push(Error::invalid_field(decl, "source")),
        }
    }

    fn validate_child_decl(&mut self, child: &'a fdecl::Child) {
        if let Err(mut e) = validate_child(child, check_name) {
            self.errors.append(&mut e.errs);
        }
        if let Some(name) = child.name.as_ref() {
            let name: &str = name;
            if self.all_children.insert(name, child).is_some() {
                self.errors.push(Error::duplicate_field(DeclType::Child, "name", name));
            }
        }
        if let Some(environment) = child.environment.as_ref() {
            if !self.all_environment_names.contains(environment.as_str()) {
                self.errors.push(Error::invalid_environment(
                    DeclType::Child,
                    "environment",
                    environment,
                ));
            }
        }
    }

    fn validate_collection_decl(&mut self, collection: &'a fdecl::Collection) {
        let name = collection.name.as_ref();
        if check_name(name, DeclType::Collection, "name", &mut self.errors) {
            let name: &str = name.unwrap();
            if !self.all_collections.insert(name) {
                self.errors.push(Error::duplicate_field(DeclType::Collection, "name", name));
            }
        }
        if collection.durability.is_none() {
            self.errors.push(Error::missing_field(DeclType::Collection, "durability"));
        }
        if let Some(environment) = collection.environment.as_ref() {
            if !self.all_environment_names.contains(environment.as_str()) {
                self.errors.push(Error::invalid_environment(
                    DeclType::Collection,
                    "environment",
                    environment,
                ));
            }
        }
        // Allow `allowed_offers` & `allow_long_names` to be unset/unvalidated, for backwards compatibility.
    }

    fn validate_environment_decl(&mut self, environment: &'a fdecl::Environment) {
        let name = environment.name.as_ref();
        check_name(name, DeclType::Environment, "name", &mut self.errors);
        if environment.extends.is_none() {
            self.errors.push(Error::missing_field(DeclType::Environment, "extends"));
        }
        if let Some(runners) = environment.runners.as_ref() {
            let mut registered_runners = HashSet::new();
            for runner in runners {
                self.validate_runner_registration(runner, &mut registered_runners);
            }
        }
        if let Some(resolvers) = environment.resolvers.as_ref() {
            let mut registered_schemes = HashSet::new();
            for resolver in resolvers {
                self.validate_resolver_registration(resolver, &mut registered_schemes);
            }
        }

        match environment.extends.as_ref() {
            Some(fdecl::EnvironmentExtends::None) => {
                if environment.stop_timeout_ms.is_none() {
                    self.errors
                        .push(Error::missing_field(DeclType::Environment, "stop_timeout_ms"));
                }
            }
            None | Some(fdecl::EnvironmentExtends::Realm) => {}
        }

        if let Some(debugs) = environment.debug_capabilities.as_ref() {
            for debug in debugs {
                self.validate_environment_debug_registration(debug);
            }
        }
    }

    fn validate_runner_registration(
        &mut self,
        runner_registration: &'a fdecl::RunnerRegistration,
        runner_names: &mut HashSet<&'a str>,
    ) {
        check_name(
            runner_registration.source_name.as_ref(),
            DeclType::RunnerRegistration,
            "source_name",
            &mut self.errors,
        );
        self.validate_registration_source(
            runner_registration.source.as_ref(),
            DeclType::RunnerRegistration,
        );
        // If the source is `self`, ensure we have a corresponding Runner.
        if let (Some(fdecl::Ref::Self_(_)), Some(name)) =
            (&runner_registration.source, runner_registration.source_name.as_ref())
        {
            if !self.all_runners.contains(name as &str) {
                self.errors.push(Error::invalid_runner(
                    DeclType::RunnerRegistration,
                    "source_name",
                    name,
                ));
            }
        }

        check_name(
            runner_registration.target_name.as_ref(),
            DeclType::RunnerRegistration,
            "target_name",
            &mut self.errors,
        );
        if let Some(name) = runner_registration.target_name.as_ref() {
            if !runner_names.insert(name.as_str()) {
                self.errors.push(Error::duplicate_field(
                    DeclType::RunnerRegistration,
                    "target_name",
                    name,
                ));
            }
        }
    }

    fn validate_resolver_registration(
        &mut self,
        resolver_registration: &'a fdecl::ResolverRegistration,
        schemes: &mut HashSet<&'a str>,
    ) {
        check_name(
            resolver_registration.resolver.as_ref(),
            DeclType::ResolverRegistration,
            "resolver",
            &mut self.errors,
        );
        self.validate_registration_source(
            resolver_registration.source.as_ref(),
            DeclType::ResolverRegistration,
        );
        check_url_scheme(
            resolver_registration.scheme.as_ref(),
            DeclType::ResolverRegistration,
            "scheme",
            &mut self.errors,
        );
        if let Some(scheme) = resolver_registration.scheme.as_ref() {
            if !schemes.insert(scheme.as_str()) {
                self.errors.push(Error::duplicate_field(
                    DeclType::ResolverRegistration,
                    "scheme",
                    scheme,
                ));
            }
        }
    }

    fn validate_registration_source(&mut self, source: Option<&'a fdecl::Ref>, ty: DeclType) {
        match source {
            Some(fdecl::Ref::Parent(_)) => {}
            Some(fdecl::Ref::Self_(_)) => {}
            Some(fdecl::Ref::Child(child_ref)) => {
                // Make sure the child is valid.
                self.validate_child_ref(ty, "source", &child_ref, OfferType::Static);
            }
            Some(_) => {
                self.errors.push(Error::invalid_field(ty, "source"));
            }
            None => {
                self.errors.push(Error::missing_field(ty, "source"));
            }
        }
    }

    fn validate_service_decl(&mut self, service: &'a fdecl::Service, as_builtin: bool) {
        if check_name(service.name.as_ref(), DeclType::Service, "name", &mut self.errors) {
            let name = service.name.as_ref().unwrap();
            if !self.all_capability_ids.insert(name) {
                self.errors.push(Error::duplicate_field(DeclType::Service, "name", name.as_str()));
            }
            self.all_services.insert(name);
        }
        match as_builtin {
            true => {
                if let Some(path) = service.source_path.as_ref() {
                    self.errors.push(Error::extraneous_source_path(DeclType::Service, path))
                }
            }
            false => {
                check_path(
                    service.source_path.as_ref(),
                    DeclType::Service,
                    "source_path",
                    &mut self.errors,
                );
            }
        }
    }

    fn validate_protocol_decl(&mut self, protocol: &'a fdecl::Protocol, as_builtin: bool) {
        if check_name(protocol.name.as_ref(), DeclType::Protocol, "name", &mut self.errors) {
            let name = protocol.name.as_ref().unwrap();
            if !self.all_capability_ids.insert(name) {
                self.errors.push(Error::duplicate_field(DeclType::Protocol, "name", name.as_str()));
            }
            self.all_protocols.insert(name);
        }
        match as_builtin {
            true => {
                if let Some(path) = protocol.source_path.as_ref() {
                    self.errors.push(Error::extraneous_source_path(DeclType::Protocol, path))
                }
            }
            false => {
                check_path(
                    protocol.source_path.as_ref(),
                    DeclType::Protocol,
                    "source_path",
                    &mut self.errors,
                );
            }
        }

        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        match protocol.delivery {
            Some(delivery) => match cm_types::DeliveryType::try_from(delivery) {
                Ok(_) => {}
                Err(_) => self.errors.push(Error::invalid_field(DeclType::Protocol, "delivery")),
            },
            None => {}
        }
    }

    fn validate_directory_decl(&mut self, directory: &'a fdecl::Directory, as_builtin: bool) {
        if check_name(directory.name.as_ref(), DeclType::Directory, "name", &mut self.errors) {
            let name = directory.name.as_ref().unwrap();
            if !self.all_capability_ids.insert(name) {
                self.errors.push(Error::duplicate_field(
                    DeclType::Directory,
                    "name",
                    name.as_str(),
                ));
            }
            self.all_directories.insert(name);
        }
        match as_builtin {
            true => {
                if let Some(path) = directory.source_path.as_ref() {
                    self.errors.push(Error::extraneous_source_path(DeclType::Directory, path))
                }
            }
            false => {
                check_path(
                    directory.source_path.as_ref(),
                    DeclType::Directory,
                    "source_path",
                    &mut self.errors,
                );
            }
        }
        if directory.rights.is_none() {
            self.errors.push(Error::missing_field(DeclType::Directory, "rights"));
        }
    }

    fn validate_storage_decl(&mut self, storage: &'a fdecl::Storage) {
        match storage.source.as_ref() {
            Some(fdecl::Ref::Parent(_)) => {}
            Some(fdecl::Ref::Self_(_)) => {}
            Some(fdecl::Ref::Child(child)) => {
                let _ =
                    self.validate_child_ref(DeclType::Storage, "source", &child, OfferType::Static);
            }
            Some(_) => {
                self.errors.push(Error::invalid_field(DeclType::Storage, "source"));
            }
            None => {
                self.errors.push(Error::missing_field(DeclType::Storage, "source"));
            }
        };
        if check_name(storage.name.as_ref(), DeclType::Storage, "name", &mut self.errors) {
            let name = storage.name.as_ref().unwrap();
            if !self.all_capability_ids.insert(name) {
                self.errors.push(Error::duplicate_field(DeclType::Storage, "name", name.as_str()));
            }
            self.all_storages.insert(name, storage.source.as_ref());
        }
        if storage.storage_id.is_none() {
            self.errors.push(Error::missing_field(DeclType::Storage, "storage_id"));
        }
        check_name(
            storage.backing_dir.as_ref(),
            DeclType::Storage,
            "backing_dir",
            &mut self.errors,
        );
    }

    fn validate_runner_decl(&mut self, runner: &'a fdecl::Runner, as_builtin: bool) {
        if check_name(runner.name.as_ref(), DeclType::Runner, "name", &mut self.errors) {
            let name = runner.name.as_ref().unwrap();
            if !self.all_capability_ids.insert(name) {
                self.errors.push(Error::duplicate_field(DeclType::Runner, "name", name.as_str()));
            }
            self.all_runners.insert(name);
        }
        match as_builtin {
            true => {
                if let Some(path) = runner.source_path.as_ref() {
                    self.errors.push(Error::extraneous_source_path(DeclType::Runner, path))
                }
            }
            false => {
                check_path(
                    runner.source_path.as_ref(),
                    DeclType::Runner,
                    "source_path",
                    &mut self.errors,
                );
            }
        }
    }

    fn validate_resolver_decl(&mut self, resolver: &'a fdecl::Resolver, as_builtin: bool) {
        if check_name(resolver.name.as_ref(), DeclType::Resolver, "name", &mut self.errors) {
            let name = resolver.name.as_ref().unwrap();
            if !self.all_capability_ids.insert(name) {
                self.errors.push(Error::duplicate_field(DeclType::Resolver, "name", name.as_str()));
            }
            self.all_resolvers.insert(name);
        }
        match as_builtin {
            true => {
                if let Some(path) = resolver.source_path.as_ref() {
                    self.errors.push(Error::extraneous_source_path(DeclType::Resolver, path))
                }
            }
            false => {
                check_path(
                    resolver.source_path.as_ref(),
                    DeclType::Resolver,
                    "source_path",
                    &mut self.errors,
                );
            }
        }
    }

    // Dictionaries can reference other dictionaries in the same manifest, so before processing any
    // dictionary declarations this function should be called to do a first pass to pre-populate
    // the dictionary map.
    fn load_dictionary_names(&mut self, dictionaries: impl Iterator<Item = &'a fdecl::Dictionary>) {
        for dictionary in dictionaries {
            let decl = DeclType::Dictionary;
            if check_name(dictionary.name.as_ref(), decl, "name", &mut self.errors) {
                let name = dictionary.name.as_ref().unwrap();
                if !self.all_capability_ids.insert(name) {
                    self.errors.push(Error::duplicate_field(decl, "name", name.as_str()));
                }
                self.all_dictionaries.insert(name, &dictionary);
            }
        }
    }

    fn validate_dictionary_decl(&mut self, dictionary: &'a fdecl::Dictionary) {
        let decl = DeclType::Dictionary;
        if let Some(path) = dictionary.source_path.as_ref() {
            if dictionary.source.is_some() {
                self.errors.push(Error::extraneous_field(decl, "source"));
            }
            check_path(Some(path), DeclType::Dictionary, "source_path", &mut self.errors);
        }
    }

    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    fn validate_configuration_decl(&mut self, config: &'a fdecl::Configuration) {
        let decl = DeclType::Configuration;
        if check_name(config.name.as_ref(), decl, "name", &mut self.errors) {
            let name = config.name.as_ref().unwrap();
            if !self.all_capability_ids.insert(name) {
                self.errors.push(Error::duplicate_field(decl, "name", name.as_str()));
            }
            self.all_configs.insert(name);
        }
    }

    fn validate_environment_debug_registration(&mut self, debug: &'a fdecl::DebugRegistration) {
        match debug {
            fdecl::DebugRegistration::Protocol(o) => {
                let decl = DeclType::DebugProtocolRegistration;
                self.validate_environment_debug_fields(
                    decl,
                    o.source.as_ref(),
                    o.source_name.as_ref(),
                    o.target_name.as_ref(),
                );

                if let (Some(fdecl::Ref::Self_(_)), Some(name)) =
                    (&o.source, o.source_name.as_ref())
                {
                    if !self.all_protocols.contains(&name as &str) {
                        self.errors.push(Error::invalid_field(decl, "source"));
                    }
                }
            }
            _ => {
                self.errors.push(Error::invalid_field(DeclType::Environment, "debug"));
            }
        }
    }

    fn validate_environment_debug_fields(
        &mut self,
        decl: DeclType,
        source: Option<&fdecl::Ref>,
        source_name: Option<&String>,
        target_name: Option<&'a String>,
    ) {
        // We don't support "source" from "capability" for now.
        match source {
            Some(fdecl::Ref::Parent(_)) => {}
            Some(fdecl::Ref::Self_(_)) => {}
            Some(fdecl::Ref::Framework(_)) => {}
            Some(fdecl::Ref::Child(child)) => {
                let _ = self.validate_child_ref(decl, "source", &child, OfferType::Static);
            }
            Some(_) => self.errors.push(Error::invalid_field(decl, "source")),
            None => self.errors.push(Error::missing_field(decl, "source")),
        }
        check_name(source_name, decl, "source_name", &mut self.errors);
        check_name(target_name, decl, "target_name", &mut self.errors);
    }

    fn validate_event_stream_decl(&mut self, event: &'a fdecl::EventStream) {
        if check_name(event.name.as_ref(), DeclType::EventStream, "name", &mut self.errors) {
            let name = event.name.as_ref().unwrap();
            if !self.all_capability_ids.insert(name) {
                self.errors.push(Error::duplicate_field(
                    DeclType::EventStream,
                    "name",
                    name.as_str(),
                ));
            }
        }
    }

    fn validate_source_collection(
        &mut self,
        collection: &fdecl::CollectionRef,
        decl_type: DeclType,
    ) -> bool {
        let num_errors = self.errors.len();
        if check_name(Some(&collection.name), decl_type, "source.collection.name", &mut self.errors)
            && !self.all_collections.contains(&collection.name as &str)
        {
            self.errors.push(Error::invalid_collection(
                decl_type,
                "source",
                &collection.name as &str,
            ));
        }
        num_errors == self.errors.len()
    }

    fn validate_filtered_service_fields(
        &mut self,
        decl_type: DeclType,
        source_instance_filter: Option<&Vec<String>>,
        renamed_instances: Option<&Vec<fdecl::NameMapping>>,
    ) {
        if let Some(source_instance_filter) = source_instance_filter {
            if source_instance_filter.is_empty() {
                // if the  source_instance_filter is empty the offered service will have 0 instances,
                // which means the offer shouldn't have been created at all.
                self.errors.push(Error::invalid_field(decl_type, "source_instance_filter"));
            }
            for name in source_instance_filter {
                check_name(Some(name), decl_type, "source_instance_filter", &mut self.errors);
            }
        }
        if let Some(renamed_instances) = renamed_instances {
            // Multiple sources shouldn't map to the same target name
            let mut seen_target_names = HashSet::<String>::new();
            for mapping in renamed_instances {
                check_name(
                    Some(&mapping.source_name),
                    decl_type,
                    "renamed_instances.source_name",
                    &mut self.errors,
                );
                check_name(
                    Some(&mapping.target_name),
                    decl_type,
                    "renamed_instances.target_name",
                    &mut self.errors,
                );
                if !seen_target_names.insert(mapping.target_name.clone()) {
                    self.errors.push(Error::invalid_field(decl_type, "renamed_instances"));
                    break;
                }
            }
        }
    }

    fn validate_source_capability(
        &mut self,
        capability: &fdecl::CapabilityRef,
        decl_type: DeclType,
        field: &str,
    ) -> bool {
        let num_errors = self.errors.len();
        if check_name(Some(&capability.name), decl_type, "source.capability.name", &mut self.errors)
            && !self.all_capability_ids.contains(capability.name.as_str())
        {
            self.errors.push(Error::invalid_capability(decl_type, field, &capability.name));
        }
        num_errors == self.errors.len()
    }

    /// Return a key that can be used in `HashMap` to group aggregate declarations.
    ///
    /// Returns `None` if the input resembles an invalid declaration.
    fn make_group_key(
        target_name: Option<&'a String>,
        target: Option<&'a fdecl::Ref>,
    ) -> Option<(&'a str, RefKey<'a>)> {
        if target_name.is_none() {
            return None;
        }
        let target_name = target_name.unwrap().as_str();
        if target.is_none() {
            return None;
        }
        let target = match target.unwrap() {
            fdecl::Ref::Parent(_) => RefKey::Parent,
            fdecl::Ref::Self_(_) => RefKey::Self_,
            fdecl::Ref::Child(r) => RefKey::Child(r.name.as_str()),
            fdecl::Ref::Collection(r) => RefKey::Collection(r.name.as_str()),
            fdecl::Ref::Framework(_) => RefKey::Framework,
            fdecl::Ref::Capability(_) => RefKey::Capability,
            fdecl::Ref::Debug(_) => RefKey::Debug,
            fdecl::RefUnknown!() => {
                return None;
            }
        };
        Some((target_name, target))
    }

    fn validate_aggregation_has_same_availability(
        &mut self,
        route_group: &Vec<impl HasAvailability>,
    ) {
        // Use `BtreeSet` for stable ordering of items in error message.
        let availability_of_sources: BTreeSet<_> =
            route_group.iter().map(|r| r.availability()).collect();

        // All sources that feed into an aggregation operation should have the same availability.
        if availability_of_sources.len() > 1 {
            self.errors.push(Error::different_availability_in_aggregation(
                availability_of_sources.into_iter().collect(),
            ));
        }
    }

    // Checks a group of expose decls to confirm that any duplicate exposes are
    // valid aggregate expose declarations.
    fn validate_expose_group(&mut self, exposes: &'a [fdecl::Expose]) {
        let mut expose_groups: HashMap<_, Vec<fdecl::ExposeService>> = HashMap::new();
        let service_exposes = exposes
            .iter()
            .filter_map(|o| if let fdecl::Expose::Service(s) = o { Some(s) } else { None });
        for expose in service_exposes {
            let key = Self::make_group_key(expose.target_name.as_ref(), expose.target.as_ref());
            if let Some(key) = key {
                expose_groups.entry(key).or_insert_with(|| vec![]).push(expose.clone());
            }
        }
        for expose_group in expose_groups.into_values() {
            if expose_group.len() == 1 {
                // If there are not multiple exposes for a (target_name, target) pair then there are
                // no aggregation conditions to check.
                continue;
            }

            self.validate_aggregation_has_same_availability(&expose_group);
        }
    }

    fn validate_expose_decl(
        &mut self,
        expose: &'a fdecl::Expose,
        expose_to_parent_ids: &mut HashMap<&'a str, AllowableIds>,
        expose_to_framework_ids: &mut HashMap<&'a str, AllowableIds>,
    ) {
        match expose {
            fdecl::Expose::Service(e) => {
                let decl = DeclType::ExposeService;
                self.validate_expose_fields(
                    decl,
                    AllowableIds::Many,
                    CollectionSource::Allow,
                    Self::service_checker,
                    e.source.as_ref(),
                    e.source_name.as_ref(),
                    e.source_dictionary.as_ref(),
                    e.target.as_ref(),
                    e.target_name.as_ref(),
                    e.availability.as_ref(),
                    expose_to_parent_ids,
                    expose_to_framework_ids,
                );
            }
            fdecl::Expose::Protocol(e) => {
                let decl = DeclType::ExposeProtocol;
                self.validate_expose_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::protocol_checker,
                    e.source.as_ref(),
                    e.source_name.as_ref(),
                    e.source_dictionary.as_ref(),
                    e.target.as_ref(),
                    e.target_name.as_ref(),
                    e.availability.as_ref(),
                    expose_to_parent_ids,
                    expose_to_framework_ids,
                );
            }
            fdecl::Expose::Directory(e) => {
                let decl = DeclType::ExposeDirectory;
                self.validate_expose_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::directory_checker,
                    e.source.as_ref(),
                    e.source_name.as_ref(),
                    e.source_dictionary.as_ref(),
                    e.target.as_ref(),
                    e.target_name.as_ref(),
                    e.availability.as_ref(),
                    expose_to_parent_ids,
                    expose_to_framework_ids,
                );

                // Subdir makes sense when routing, but when exposing to framework the subdirectory
                // can be exposed directly.
                match e.target.as_ref() {
                    Some(fdecl::Ref::Framework(_)) => {
                        if e.subdir.is_some() {
                            self.errors.push(Error::invalid_field(decl, "subdir"));
                        }
                    }
                    _ => {}
                }

                if let Some(subdir) = e.subdir.as_ref() {
                    check_relative_path(Some(subdir), decl, "subdir", &mut self.errors);
                }
            }
            fdecl::Expose::Runner(e) => {
                let decl = DeclType::ExposeRunner;
                self.validate_expose_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::runner_checker,
                    e.source.as_ref(),
                    e.source_name.as_ref(),
                    e.source_dictionary.as_ref(),
                    e.target.as_ref(),
                    e.target_name.as_ref(),
                    Some(&fdecl::Availability::Required),
                    expose_to_parent_ids,
                    expose_to_framework_ids,
                );
            }
            fdecl::Expose::Resolver(e) => {
                let decl = DeclType::ExposeResolver;
                self.validate_expose_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::resolver_checker,
                    e.source.as_ref(),
                    e.source_name.as_ref(),
                    e.source_dictionary.as_ref(),
                    e.target.as_ref(),
                    e.target_name.as_ref(),
                    Some(&fdecl::Availability::Required),
                    expose_to_parent_ids,
                    expose_to_framework_ids,
                );
            }
            fdecl::Expose::Dictionary(e) => {
                let decl = DeclType::ExposeDictionary;
                self.validate_expose_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::dictionary_checker,
                    e.source.as_ref(),
                    e.source_name.as_ref(),
                    e.source_dictionary.as_ref(),
                    e.target.as_ref(),
                    e.target_name.as_ref(),
                    e.availability.as_ref(),
                    expose_to_parent_ids,
                    expose_to_framework_ids,
                );
            }
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            fdecl::Expose::Config(e) => {
                let decl = DeclType::ExposeConfig;
                self.validate_expose_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::config_checker,
                    e.source.as_ref(),
                    e.source_name.as_ref(),
                    None,
                    e.target.as_ref(),
                    e.target_name.as_ref(),
                    e.availability.as_ref(),
                    expose_to_parent_ids,
                    expose_to_framework_ids,
                );
            }
            _ => {
                self.errors.push(Error::invalid_field(DeclType::Component, "expose"));
            }
        }
    }

    fn validate_expose_fields(
        &mut self,
        decl: DeclType,
        allowable_ids: AllowableIds,
        collection_source: CollectionSource,
        // This takes a callback that returns a [Container], instead of the &[Container] directly,
        // to avoid a borrow checker error that would occur from a simultaneous borrow on
        // &mut self.
        capability_checker: impl Fn(&Self) -> &dyn Container,
        source: Option<&fdecl::Ref>,
        source_name: Option<&String>,
        source_dictionary: Option<&String>,
        target: Option<&fdecl::Ref>,
        target_name: Option<&'a String>,
        availability: Option<&fdecl::Availability>,
        expose_to_parent_ids: &mut HashMap<&'a str, AllowableIds>,
        expose_to_framework_ids: &mut HashMap<&'a str, AllowableIds>,
    ) {
        self.validate_expose_source(decl, collection_source, source, source_dictionary);
        check_route_availability(decl, availability, source, source_name, &mut self.errors);
        match target {
            Some(r) => match r {
                fdecl::Ref::Parent(_) => {}
                fdecl::Ref::Framework(_) => {}
                _ => {
                    self.errors.push(Error::invalid_field(decl, "target"));
                }
            },
            None => {
                self.errors.push(Error::missing_field(decl, "target"));
            }
        }
        check_name(source_name, decl, "source_name", &mut self.errors);
        if source_dictionary.is_some() {
            check_relative_path(source_dictionary, decl, "source_dictionary", &mut self.errors);
        }
        if check_name(target_name, decl, "target_name", &mut self.errors) {
            let maybe_ids_set = match target {
                Some(fdecl::Ref::Parent(_)) => Some(expose_to_parent_ids),
                Some(fdecl::Ref::Framework(_)) => Some(expose_to_framework_ids),
                _ => None,
            };
            if let Some(ids_set) = maybe_ids_set {
                let target_name = target_name.unwrap();
                if let Some(prev_state) = ids_set.insert(target_name, allowable_ids) {
                    if prev_state == AllowableIds::One || prev_state != allowable_ids {
                        self.errors.push(Error::duplicate_field(decl, "target_name", target_name));
                    }
                }
            }
        }

        self.validate_route_from_self(
            decl,
            source,
            source_name,
            source_dictionary,
            capability_checker,
        );
    }

    fn validate_expose_source(
        &mut self,
        decl: DeclType,
        collection_source: CollectionSource,
        source: Option<&fdecl::Ref>,
        source_dictionary: Option<&String>,
    ) {
        match (source, source_dictionary) {
            // These sources support source_dictionary.
            (Some(fdecl::Ref::Self_(_)), _) => {}
            (Some(fdecl::Ref::Child(child)), _) => {
                let _ = self.validate_child_ref(decl, "source", &child, OfferType::Static);
            }
            // These sources don't.
            (Some(fdecl::Ref::VoidType(_)), None) => {}
            (Some(fdecl::Ref::Framework(_)), None) => {}
            (Some(fdecl::Ref::Capability(c)), None) => {
                self.validate_source_capability(c, decl, "source");
            }
            (Some(fdecl::Ref::Collection(c)), None)
                if collection_source == CollectionSource::Allow =>
            {
                self.validate_source_collection(c, decl);
            }
            // `source` is required.
            (None, _) => {
                self.errors.push(Error::missing_field(decl, "source"));
            }
            // Any combination that was not recognized above must be invalid.
            (_, _) => {
                self.errors.push(Error::invalid_field(decl, "source"));
            }
        }
    }

    // Checks a group of offer decls to confirm that any duplicate offers are
    // valid aggregate offer declarations.
    fn validate_offer_group(&mut self, offers: &'a [fdecl::Offer], offer_type: OfferType) {
        let mut offer_groups: HashMap<_, Vec<fdecl::OfferService>> = HashMap::new();
        let service_offers = offers
            .iter()
            .filter_map(|o| if let fdecl::Offer::Service(s) = o { Some(s) } else { None });
        for offer in service_offers {
            let key = Self::make_group_key(offer.target_name.as_ref(), offer.target.as_ref());
            if let Some(key) = key {
                offer_groups.entry(key).or_insert_with(|| vec![]).push(offer.clone());
            }
        }
        for offer_group in offer_groups.into_values() {
            if offer_group.len() == 1 {
                // If there are not multiple offers for a (target_name, target) pair then there are
                // no aggregation conditions to check.
                continue;
            }

            self.validate_aggregation_has_same_availability(&offer_group);

            let mut source_instance_filter_entries: HashSet<String> = HashSet::new();
            let mut service_source_names: HashSet<String> = HashSet::new();
            for o in offer_group {
                // Currently only service capabilities can be aggregated
                match (o.source_instance_filter, offer_type) {
                    (Some(source_instance_filter), _) => {
                        for instance_name in source_instance_filter {
                            if !source_instance_filter_entries.insert(instance_name.clone()) {
                                // If the source instance in the filter has been seen before this
                                // means there is a conflicting aggregate service offer.
                                self.errors.push(Error::invalid_aggregate_offer(format!(
                                    "Conflicting source_instance_filter in aggregate service \
                                    offer, instance_name '{}' seen in filter lists multiple times",
                                    instance_name,
                                )));
                            }
                        }
                    }
                    (None, OfferType::Static) => {}
                    (None, OfferType::Dynamic) => {
                        // Dynamic offers must include a filter.
                        self.errors.push(Error::invalid_aggregate_offer(
                            "source_instance_filter must be set for dynamic aggregate service \
                            offers",
                        ));
                    }
                }
                service_source_names.insert(
                    o.source_name
                        .expect("Offer Service declarations must always contain source_name"),
                );
            }

            if service_source_names.len() > 1 {
                self.errors.push(Error::invalid_aggregate_offer(format!(
                    "All aggregate service offers must have the same source_name, saw {}. Use \
                    renamed_instances to rename instance names to avoid conflict.",
                    service_source_names.into_iter().sorted().collect::<Vec<String>>().join(", ")
                )));
            }
        }
    }

    fn validate_offer_decl(&mut self, offer: &'a fdecl::Offer, offer_type: OfferType) {
        match offer {
            fdecl::Offer::Service(o) => {
                let decl = DeclType::OfferService;
                self.validate_offer_fields(
                    decl,
                    AllowableIds::Many,
                    CollectionSource::Allow,
                    Self::service_checker,
                    o.source.as_ref(),
                    o.source_name.as_ref(),
                    o.source_dictionary.as_ref(),
                    o.target.as_ref(),
                    o.target_name.as_ref(),
                    #[cfg(fuchsia_api_level_at_least = "HEAD")]
                    Some(o.dependency_type.as_ref().unwrap_or(&fdecl::DependencyType::Strong)),
                    #[cfg(fuchsia_api_level_less_than = "HEAD")]
                    Some(&fdecl::DependencyType::Strong),
                    o.availability.as_ref(),
                    offer_type,
                );
                self.validate_filtered_service_fields(
                    decl,
                    o.source_instance_filter.as_ref(),
                    o.renamed_instances.as_ref(),
                );
            }
            fdecl::Offer::Protocol(o) => {
                let decl = DeclType::OfferProtocol;
                self.validate_offer_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::protocol_checker,
                    o.source.as_ref(),
                    o.source_name.as_ref(),
                    o.source_dictionary.as_ref(),
                    o.target.as_ref(),
                    o.target_name.as_ref(),
                    o.dependency_type.as_ref(),
                    o.availability.as_ref(),
                    offer_type,
                );
            }
            fdecl::Offer::Directory(o) => {
                let decl = DeclType::OfferDirectory;
                self.validate_offer_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::directory_checker,
                    o.source.as_ref(),
                    o.source_name.as_ref(),
                    o.source_dictionary.as_ref(),
                    o.target.as_ref(),
                    o.target_name.as_ref(),
                    o.dependency_type.as_ref(),
                    o.availability.as_ref(),
                    offer_type,
                );
                if let Some(subdir) = o.subdir.as_ref() {
                    check_relative_path(
                        Some(subdir),
                        DeclType::OfferDirectory,
                        "subdir",
                        &mut self.errors,
                    );
                }
            }
            fdecl::Offer::Storage(o) => {
                let decl = DeclType::OfferStorage;
                self.validate_storage_offer_fields(
                    decl,
                    Self::storage_checker,
                    o.source.as_ref(),
                    o.source_name.as_ref(),
                    o.target.as_ref(),
                    o.target_name.as_ref(),
                    o.availability.as_ref(),
                    offer_type,
                );
            }
            fdecl::Offer::Runner(o) => {
                let decl = DeclType::OfferRunner;
                self.validate_offer_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::runner_checker,
                    o.source.as_ref(),
                    o.source_name.as_ref(),
                    o.source_dictionary.as_ref(),
                    o.target.as_ref(),
                    o.target_name.as_ref(),
                    Some(&fdecl::DependencyType::Strong),
                    Some(&fdecl::Availability::Required),
                    offer_type,
                );
            }
            fdecl::Offer::Resolver(o) => {
                let decl = DeclType::OfferResolver;
                self.validate_offer_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::resolver_checker,
                    o.source.as_ref(),
                    o.source_name.as_ref(),
                    o.source_dictionary.as_ref(),
                    o.target.as_ref(),
                    o.target_name.as_ref(),
                    Some(&fdecl::DependencyType::Strong),
                    Some(&fdecl::Availability::Required),
                    offer_type,
                );
            }
            fdecl::Offer::EventStream(e) => {
                self.validate_event_stream_offer_fields(e, offer_type);
            }
            fdecl::Offer::Dictionary(o) => {
                let decl = DeclType::OfferDictionary;
                self.validate_offer_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::dictionary_checker,
                    o.source.as_ref(),
                    o.source_name.as_ref(),
                    o.source_dictionary.as_ref(),
                    o.target.as_ref(),
                    o.target_name.as_ref(),
                    o.dependency_type.as_ref(),
                    o.availability.as_ref(),
                    offer_type,
                );
            }
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            fdecl::Offer::Config(o) => {
                let decl = DeclType::OfferConfig;
                self.validate_offer_fields(
                    decl,
                    AllowableIds::One,
                    CollectionSource::Deny,
                    Self::config_checker,
                    o.source.as_ref(),
                    o.source_name.as_ref(),
                    None,
                    o.target.as_ref(),
                    o.target_name.as_ref(),
                    Some(&fdecl::DependencyType::Strong),
                    o.availability.as_ref(),
                    offer_type,
                );
            }
            fdecl::OfferUnknown!() => {
                self.errors.push(Error::invalid_field(DeclType::Component, "offer"));
            }
        }
    }

    fn validate_offer_fields(
        &mut self,
        decl: DeclType,
        allowable_names: AllowableIds,
        collection_source: CollectionSource,
        capability_checker: impl Fn(&Self) -> &dyn Container,
        source: Option<&'a fdecl::Ref>,
        source_name: Option<&'a String>,
        source_dictionary: Option<&'a String>,
        target: Option<&'a fdecl::Ref>,
        target_name: Option<&'a String>,
        dependency_type: Option<&'a fdecl::DependencyType>,
        availability: Option<&'a fdecl::Availability>,
        offer_type: OfferType,
    ) {
        self.validate_offer_source(decl, collection_source, source, source_dictionary, offer_type);
        check_route_availability(decl, availability, source, source_name, &mut self.errors);
        check_offer_name(source_name, decl, "source_name", offer_type, &mut self.errors);
        if source_dictionary.is_some() {
            check_relative_path(source_dictionary, decl, "source_dictionary", &mut self.errors);
        }
        self.validate_offer_target(decl, allowable_names, target, target_name, offer_type);
        check_offer_name(target_name, decl, "target_name", offer_type, &mut self.errors);

        if dependency_type.is_none() {
            self.errors.push(Error::missing_field(decl, "dependency_type"));
        }

        self.validate_route_from_self(
            decl,
            source,
            source_name,
            source_dictionary,
            capability_checker,
        );
    }

    fn validate_offer_source(
        &mut self,
        decl: DeclType,
        collection_source: CollectionSource,
        source: Option<&'a fdecl::Ref>,
        source_dictionary: Option<&'a String>,
        offer_type: OfferType,
    ) {
        match (source, source_dictionary) {
            // These sources support source_dictionary.
            (Some(fdecl::Ref::Parent(_)), _) => {}
            (Some(fdecl::Ref::Self_(_)), _) => {}
            (Some(fdecl::Ref::Child(child)), _) => {
                self.validate_child_ref(decl, "source", &child, offer_type);
            }
            // These sources don't.
            (Some(fdecl::Ref::VoidType(_)), None) => {}
            (Some(fdecl::Ref::Framework(_)), None) => {}
            (Some(fdecl::Ref::Capability(c)), None) => {
                self.validate_source_capability(c, decl, "source");
            }
            (Some(fdecl::Ref::Collection(c)), None)
                if collection_source == CollectionSource::Allow =>
            {
                self.validate_source_collection(c, decl);
            }
            // `source` is required.
            (None, _) => self.errors.push(Error::missing_field(decl, "source")),
            // Any combination that was not recognized above must be invalid.
            (_, _) => self.errors.push(Error::invalid_field(decl, "source")),
        }
    }

    fn validate_storage_offer_fields(
        &mut self,
        decl: DeclType,
        // This takes a callback that returns a [Container], instead of the &[Container] directly,
        // to avoid a borrow checker error that would occur from a simultaneous borrow on
        // &mut self.
        capability_checker: impl Fn(&Self) -> &dyn Container,
        source: Option<&'a fdecl::Ref>,
        source_name: Option<&'a String>,
        target: Option<&'a fdecl::Ref>,
        target_name: Option<&'a String>,
        availability: Option<&fdecl::Availability>,
        offer_type: OfferType,
    ) {
        match source {
            Some(fdecl::Ref::Parent(_) | fdecl::Ref::VoidType(_) | fdecl::Ref::Self_(_)) => {}
            Some(_) => {
                self.errors.push(Error::invalid_field(decl, "source"));
            }
            None => {
                self.errors.push(Error::missing_field(decl, "source"));
            }
        }
        check_route_availability(decl, availability, source, source_name, &mut self.errors);
        check_offer_name(source_name, decl, "source_name", offer_type, &mut self.errors);
        self.validate_offer_target(decl, AllowableIds::One, target, target_name, offer_type);
        check_offer_name(target_name, decl, "target_name", offer_type, &mut self.errors);

        if let (Some(fdecl::Ref::Self_(_)), Some(name)) = (source, source_name) {
            if !(capability_checker)(self).contains(name) {
                self.errors.push(Error::invalid_capability(decl, "source", name));
            }
        }
    }

    fn validate_event_stream_offer_fields(
        &mut self,
        event_stream: &'a fdecl::OfferEventStream,
        offer_type: OfferType,
    ) {
        let decl = DeclType::OfferEventStream;
        check_name(event_stream.source_name.as_ref(), decl, "source_name", &mut self.errors);
        if event_stream.target == Some(fdecl::Ref::Framework(fdecl::FrameworkRef {})) {
            // Expose to framework from framework is never valid.
            self.errors.push(Error::invalid_field(DeclType::OfferEventStream, "target"));
        }
        if let Some(scope) = &event_stream.scope {
            if scope.is_empty() {
                self.errors.push(Error::invalid_field(decl, "scope"));
            }
            for value in scope {
                match value {
                    fdecl::Ref::Child(child) => {
                        self.validate_child_ref(
                            DeclType::OfferEventStream,
                            "scope",
                            &child,
                            offer_type,
                        );
                    }
                    fdecl::Ref::Collection(collection) => {
                        self.validate_collection_ref(
                            DeclType::OfferEventStream,
                            "scope",
                            &collection,
                        );
                    }
                    _ => {
                        self.errors.push(Error::invalid_field(DeclType::OfferEventStream, "scope"));
                    }
                }
            }
        }
        // Only parent, framework, child, and void are valid.
        match event_stream.source {
            Some(
                fdecl::Ref::Parent(_)
                | fdecl::Ref::Framework(_)
                | fdecl::Ref::Child(_)
                | fdecl::Ref::VoidType(_),
            ) => {}
            Some(_) => {
                self.errors.push(Error::invalid_field(decl, "source"));
            }
            None => {
                self.errors.push(Error::missing_field(decl, "source"));
            }
        };

        check_route_availability(
            decl,
            event_stream.availability.as_ref(),
            event_stream.source.as_ref(),
            event_stream.source_name.as_ref(),
            &mut self.errors,
        );

        self.validate_offer_target(
            decl,
            AllowableIds::One,
            event_stream.target.as_ref(),
            event_stream.target_name.as_ref(),
            offer_type,
        );
        check_name(event_stream.target_name.as_ref(), decl, "target_name", &mut self.errors);
    }

    /// Check a `ChildRef` contains a valid child that exists.
    fn validate_child_ref(
        &mut self,
        decl: DeclType,
        field_name: &str,
        child: &fdecl::ChildRef,
        offer_type: OfferType,
    ) -> bool {
        if offer_type == OfferType::Dynamic && child.collection.is_some() {
            return self.validate_dynamic_child_ref(decl, field_name, child);
        }
        // Ensure the name is valid, and the reference refers to a static child.
        //
        // We attempt to list all errors if possible.
        let mut valid = true;
        if !check_name(
            Some(&child.name),
            decl,
            &format!("{}.child.name", field_name),
            &mut self.errors,
        ) {
            valid = false;
        }
        if child.collection.is_some() {
            self.errors
                .push(Error::extraneous_field(decl, format!("{}.child.collection", field_name)));
            valid = false;
        }
        if !valid {
            return false;
        }

        // Ensure the child exists.
        let name: &str = &child.name;
        if !self.all_children.contains_key(name) {
            self.errors.push(Error::invalid_child(decl, field_name, name));
            return false;
        }

        true
    }

    /// Check a `ChildRef` contains a valid dynamic child.
    ///
    /// The manifest we're validating doesn't contain dynamic children so we can't check if the dynamic
    /// child actually exists, but we can confirm things like the name is valid.
    fn validate_dynamic_child_ref(
        &mut self,
        decl: DeclType,
        field_name: &str,
        child: &fdecl::ChildRef,
    ) -> bool {
        // Ensure the name is valid.
        //
        // We attempt to list all errors if possible.
        let mut valid = true;
        if !check_dynamic_name(
            Some(&child.name),
            decl,
            &format!("{}.child.name", field_name),
            &mut self.errors,
        ) {
            valid = false;
        }
        if !check_name(
            child.collection.as_ref(),
            decl,
            &format!("{}.child.collection", field_name),
            &mut self.errors,
        ) {
            valid = false;
        }
        valid
    }

    /// Check a `CollectionRef` is valid and refers to an existing collection.
    fn validate_collection_ref(
        &mut self,
        decl: DeclType,
        field_name: &str,
        collection: &fdecl::CollectionRef,
    ) -> bool {
        // Ensure the name is valid.
        if !check_name(
            Some(&collection.name),
            decl,
            &format!("{}.collection.name", field_name),
            &mut self.errors,
        ) {
            return false;
        }

        // Ensure the collection exists.
        if !self.all_collections.contains(&collection.name as &str) {
            self.errors.push(Error::invalid_collection(decl, field_name, &collection.name as &str));
            return false;
        }

        true
    }

    fn validate_offer_target(
        &mut self,
        decl: DeclType,
        allowable_names: AllowableIds,
        target: Option<&'a fdecl::Ref>,
        target_name: Option<&'a String>,
        offer_type: OfferType,
    ) {
        match target {
            Some(fdecl::Ref::Child(c)) => {
                self.validate_target_child(decl, allowable_names, c, target_name, offer_type);
            }
            Some(fdecl::Ref::Collection(c)) => {
                self.validate_target_collection(decl, allowable_names, c, target_name);
            }
            Some(fdecl::Ref::Capability(c)) => {
                // Only offers to dictionary capabilities are valid.
                if let Some(d) = self.all_dictionaries.get(&c.name.as_str()) {
                    if d.source_path.is_some() {
                        // If `source_path` is present that means this is an offer into a
                        // dynamic dictionary, which is not allowed.
                        self.errors.push(Error::invalid_field(decl, "target"));
                    }
                } else {
                    self.errors.push(Error::invalid_field(decl, "target"));
                }
            }
            Some(_) => {
                self.errors.push(Error::invalid_field(decl, "target"));
            }
            None => {
                self.errors.push(Error::missing_field(decl, "target"));
            }
        }
    }

    fn validate_target_child(
        &mut self,
        decl: DeclType,
        allowable_names: AllowableIds,
        child: &'a fdecl::ChildRef,
        target_name: Option<&'a String>,
        offer_type: OfferType,
    ) {
        if !self.validate_child_ref(decl, "target", child, offer_type) {
            return;
        }
        if let Some(target_name) = target_name {
            let names_for_target = self
                .target_ids
                .entry(TargetId::Component(
                    &child.name,
                    child.collection.as_ref().map(|s| s.as_str()),
                ))
                .or_insert(HashMap::new());
            if let Some(prev_state) = names_for_target.insert(target_name, allowable_names) {
                if prev_state == AllowableIds::One || prev_state != allowable_names {
                    self.errors.push(Error::duplicate_field(
                        decl,
                        "target_name",
                        target_name as &str,
                    ));
                }
            }
            if let Some(collection) = child.collection.as_ref() {
                if let Some(names_for_target) =
                    self.target_ids.get(&TargetId::Collection(&collection))
                {
                    if names_for_target.contains_key(&target_name.as_str()) {
                        // This dynamic offer conflicts with a static offer to the same collection.
                        self.errors.push(Error::duplicate_field(
                            decl,
                            "target_name",
                            target_name as &str,
                        ));
                    }
                }
            }
        }
    }

    fn validate_target_collection(
        &mut self,
        decl: DeclType,
        allowable_names: AllowableIds,
        collection: &'a fdecl::CollectionRef,
        target_name: Option<&'a String>,
    ) {
        if !self.validate_collection_ref(decl, "target", &collection) {
            return;
        }
        if let Some(target_name) = target_name {
            let names_for_target = self
                .target_ids
                .entry(TargetId::Collection(&collection.name))
                .or_insert(HashMap::new());
            if let Some(prev_state) = names_for_target.insert(target_name, allowable_names) {
                if prev_state == AllowableIds::One || prev_state != allowable_names {
                    self.errors.push(Error::duplicate_field(
                        decl,
                        "target_name",
                        target_name as &str,
                    ));
                }
            }
        }
    }

    fn validate_route_from_self(
        &mut self,
        decl: DeclType,
        source: Option<&fdecl::Ref>,
        source_name: Option<&String>,
        source_dictionary: Option<&String>,
        capability_checker: impl Fn(&Self) -> &dyn Container,
    ) {
        let (Some(fdecl::Ref::Self_(_)), Some(name)) = (source, source_name) else {
            return;
        };
        match source_dictionary {
            Some(source_dictionary) => {
                if let Ok(path) = cm_types::RelativePath::new(source_dictionary) {
                    if let Some(first_segment) = path.iter_segments().next().map(|s| s.as_str()) {
                        if !self.all_dictionaries.contains_key(first_segment) {
                            self.errors.push(Error::invalid_capability(
                                decl,
                                "source",
                                first_segment,
                            ));
                        }
                    }
                }
            }
            None => {
                if !(capability_checker)(self).contains(name) {
                    self.errors.push(Error::invalid_capability(decl, "source", name));
                }
            }
        }
    }

    // The following functions can be used to convert a type-specific collection of capabilities
    // into [Container].
    fn service_checker(&self) -> &dyn Container {
        &self.all_services
    }
    fn protocol_checker(&self) -> &dyn Container {
        &self.all_protocols
    }
    fn directory_checker(&self) -> &dyn Container {
        &self.all_directories
    }
    fn runner_checker(&self) -> &dyn Container {
        &self.all_runners
    }
    fn resolver_checker(&self) -> &dyn Container {
        &self.all_resolvers
    }

    fn dictionary_checker(&self) -> &dyn Container {
        &self.all_dictionaries
    }

    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    fn config_checker(&self) -> &dyn Container {
        &self.all_configs
    }
    fn storage_checker(&self) -> &dyn Container {
        &self.all_storages
    }
    fn event_stream_checker(&self) -> &dyn Container {
        // Components can't define their own event streams. If someone tries to route an event
        // stream from Self it should generate some other error. So just return `true` to bypass
        // the logic.
        struct AlwaysTrueContainer {}
        impl Container for AlwaysTrueContainer {
            fn contains(&self, _key: &str) -> bool {
                true
            }
        }
        static CONTAINER: AlwaysTrueContainer = AlwaysTrueContainer {};
        &CONTAINER
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_types::MAX_LONG_NAME_LENGTH;
    use test_case::test_case;
    use {
        fidl_fuchsia_component_decl as fdecl, fidl_fuchsia_data as fdata, fidl_fuchsia_io as fio,
    };

    macro_rules! test_validate {
        (
            $(
                $test_name:ident => {
                    input = $input:expr,
                    result = $result:expr,
                },
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    validate_test($input, $result);
                }
            )+
        }
    }

    macro_rules! test_validate_any_result {
        (
            $(
                $test_name:ident => {
                    input = $input:expr,
                    results = $results:expr,
                },
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    validate_test_any_result($input, $results);
                }
            )+
        }
    }

    macro_rules! test_validate_values_data {
        (
            $(
                $test_name:ident => {
                    input = $input:expr,
                    result = $result:expr,
                },
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    validate_values_data_test($input, $result);
                }
            )+
        }
    }

    macro_rules! test_validate_capabilities {
        (
            $(
                $test_name:ident => {
                    input = $input:expr,
                    as_builtin = $as_builtin:expr,
                    result = $result:expr,
                },
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    validate_capabilities_test($input, $as_builtin, $result);
                }
            )+
        }
    }

    macro_rules! test_dependency {
        (
            $(
                ($test_name:ident) => {
                    ty = $ty:expr,
                    offer_decl = $offer_decl:expr,
                },
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    let mut decl = new_component_decl();
                    let dependencies = vec![
                        ("a", "b"),
                        ("b", "a"),
                    ];
                    let offers = dependencies.into_iter().map(|(from,to)| {
                        let mut offer_decl = $offer_decl;
                        offer_decl.source = Some(fdecl::Ref::Child(
                           fdecl::ChildRef { name: from.to_string(), collection: None },
                        ));
                        offer_decl.target = Some(fdecl::Ref::Child(
                           fdecl::ChildRef { name: to.to_string(), collection: None },
                        ));
                        $ty(offer_decl)
                    }).collect();
                    let children = ["a", "b"].iter().map(|name| {
                        fdecl::Child {
                            name: Some(name.to_string()),
                            url: Some(format!("fuchsia-pkg://fuchsia.com/pkg#meta/{}.cm", name)),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            environment: None,
                            ..Default::default()
                        }
                    }).collect();
                    decl.offers = Some(offers);
                    decl.children = Some(children);
                    let result = Err(ErrorList::new(vec![
                        Error::dependency_cycle("{{child a -> child b -> child a}}")
                    ]));
                    validate_test(decl, result);
                }
            )+
        }
    }

    macro_rules! test_weak_dependency {
        (
            $(
                ($test_name:ident) => {
                    ty = $ty:expr,
                    offer_decl = $offer_decl:expr,
                },
            )+
        ) => {
            $(
                #[test_case(fdecl::DependencyType::Weak)]
                fn $test_name(weak_dep: fdecl::DependencyType) {
                    let mut decl = new_component_decl();
                    let offers = vec![
                        {
                            let mut offer_decl = $offer_decl;
                            offer_decl.source = Some(fdecl::Ref::Child(
                               fdecl::ChildRef { name: "a".to_string(), collection: None },
                            ));
                            offer_decl.target = Some(fdecl::Ref::Child(
                               fdecl::ChildRef { name: "b".to_string(), collection: None },
                            ));
                            offer_decl.dependency_type = Some(fdecl::DependencyType::Strong);
                            $ty(offer_decl)
                        },
                        {
                            let mut offer_decl = $offer_decl;
                            offer_decl.source = Some(fdecl::Ref::Child(
                               fdecl::ChildRef { name: "b".to_string(), collection: None },
                            ));
                            offer_decl.target = Some(fdecl::Ref::Child(
                               fdecl::ChildRef { name: "a".to_string(), collection: None },
                            ));
                            offer_decl.dependency_type = Some(weak_dep);
                            $ty(offer_decl)
                        },
                    ];
                    let children = ["a", "b"].iter().map(|name| {
                        fdecl::Child {
                            name: Some(name.to_string()),
                            url: Some(format!("fuchsia-pkg://fuchsia.com/pkg#meta/{}.cm", name)),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            environment: None,
                            ..Default::default()
                        }
                    }).collect();
                    decl.offers = Some(offers);
                    decl.children = Some(children);
                    let result = Ok(());
                    validate_test(decl, result);
                }
            )+
        }
    }

    #[track_caller]
    fn validate_test(input: fdecl::Component, expected_res: Result<(), ErrorList>) {
        let res = validate(&input, &mut DirectedGraph::new());
        assert_eq!(res, expected_res);
    }

    #[track_caller]
    fn validate_test_any_result(input: fdecl::Component, expected_res: Vec<Result<(), ErrorList>>) {
        let res = format!("{:?}", validate(&input, &mut DirectedGraph::new()));
        let expected_res_debug = format!("{:?}", expected_res);

        let matched_exp =
            expected_res.into_iter().find(|expected| res == format!("{:?}", expected));

        assert!(
            matched_exp.is_some(),
            "assertion failed: Expected one of:\n{:?}\nActual:\n{:?}",
            expected_res_debug,
            res
        );
    }

    #[track_caller]
    fn validate_values_data_test(
        input: fdecl::ConfigValuesData,
        expected_res: Result<(), ErrorList>,
    ) {
        let res = validate_values_data(&input);
        assert_eq!(res, expected_res);
    }

    #[track_caller]
    fn validate_capabilities_test(
        input: Vec<fdecl::Capability>,
        as_builtin: bool,
        expected_res: Result<(), ErrorList>,
    ) {
        let res = validate_capabilities(&input, as_builtin);
        assert_eq!(res, expected_res);
    }

    fn new_component_decl() -> fdecl::Component {
        fdecl::Component {
            program: None,
            uses: None,
            exposes: None,
            offers: None,
            facets: None,
            capabilities: None,
            children: None,
            collections: None,
            environments: None,
            ..Default::default()
        }
    }

    test_validate_any_result! {
        test_validate_use_disallows_nested_dirs => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("abc".to_string()),
                        target_path: Some("/foo/bar".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("abc".to_string()),
                        target_path: Some("/foo/bar/baz".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseDirectory, "/foo/bar/baz", DeclType::UseDirectory, "/foo/bar"),
                ])),
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseDirectory, "/foo/bar", DeclType::UseDirectory, "/foo/bar/baz"),
                ])),
            ],
        },
        test_validate_use_disallows_nested_dirs_storage => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Storage(fdecl::UseStorage {
                        source_name: Some("abc".to_string()),
                        target_path: Some("/foo/bar".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Use::Storage(fdecl::UseStorage {
                        source_name: Some("abc".to_string()),
                        target_path: Some("/foo/bar/baz".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseStorage, "/foo/bar/baz", DeclType::UseStorage, "/foo/bar"),
                ])),
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseStorage, "/foo/bar", DeclType::UseStorage, "/foo/bar/baz"),
                ])),
            ],
        },
        test_validate_use_disallows_nested_dirs_directory_and_storage => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("abc".to_string()),
                        target_path: Some("/foo/bar".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Use::Storage(fdecl::UseStorage {
                        source_name: Some("abc".to_string()),
                        target_path: Some("/foo/bar/baz".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseStorage, "/foo/bar/baz", DeclType::UseDirectory, "/foo/bar"),
                ])),
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseDirectory, "/foo/bar", DeclType::UseStorage, "/foo/bar/baz"),
                ])),
            ],
        },
        test_validate_use_disallows_common_prefixes_protocol => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("abc".to_string()),
                        target_path: Some("/foo/bar".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("crow".to_string()),
                        target_path: Some("/foo/bar/fuchsia.2".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseProtocol, "/foo/bar/fuchsia.2", DeclType::UseDirectory, "/foo/bar"),
                ])),
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseDirectory, "/foo/bar", DeclType::UseProtocol, "/foo/bar/fuchsia.2"),
                ])),
            ],
        },
        test_validate_use_disallows_common_prefixes_service => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("abc".to_string()),
                        target_path: Some("/foo/bar".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Use::Service(fdecl::UseService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("space".to_string()),
                        target_path: Some("/foo/bar/baz/fuchsia.logger.Log".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseService, "/foo/bar/baz/fuchsia.logger.Log", DeclType::UseDirectory, "/foo/bar"),
                ])),
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseDirectory, "/foo/bar", DeclType::UseService, "/foo/bar/baz/fuchsia.logger.Log"),
                ])),
            ],
        },
        test_validate_use_disallows_pkg => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("abc".to_string()),
                        target_path: Some("/pkg".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::pkg_path_overlap(DeclType::UseDirectory, "/pkg"),
                ])),
            ],
        },
        test_validate_use_disallows_pkg_overlap => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("abc".to_string()),
                        target_path: Some("/pkg/foo".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::pkg_path_overlap(DeclType::UseDirectory, "/pkg/foo"),
                ])),
            ],
        },
        test_validate_use_optional_config_correct => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Config(fdecl::UseConfiguration {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("abc".to_string()),
                        target_name: Some("foo".to_string()),
                        availability: Some(fdecl::Availability::Optional),
                        type_: Some(fdecl::ConfigType {
                            layout: fdecl::ConfigTypeLayout::Bool,
                            parameters: Some(Vec::new()),
                            constraints: Vec::new(),
                        }),
                        ..Default::default()
                    }),
                ]);
                decl.config = Some(fdecl::ConfigSchema {
                    fields: Some(vec![fdecl::ConfigField {
                        key: Some("foo".into()),
                        type_: Some(fdecl::ConfigType {
                            layout: fdecl::ConfigTypeLayout::Bool,
                            parameters: Some(Vec::new()),
                            constraints: Vec::new(),
                        }),
                        mutability: None,
                        ..Default::default()}]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0;32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("path".into())),
                    ..Default::default()
                     });
                decl
            },
            results = vec![Ok(())],
        },
        test_validate_use_optional_config_no_config_schema => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Config(fdecl::UseConfiguration {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("abc".to_string()),
                        target_name: Some("foo".to_string()),
                        availability: Some(fdecl::Availability::Optional),
                        type_: Some(fdecl::ConfigType {
                            layout: fdecl::ConfigTypeLayout::Bool,
                            parameters: None,
                            constraints: Vec::new(),
                        }),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::missing_field(DeclType::ConfigField, "config"),
                ])),
            ],
        },
        test_validate_use_optional_config_no_config_field => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Config(fdecl::UseConfiguration {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("abc".to_string()),
                        target_name: Some("foo".to_string()),
                        availability: Some(fdecl::Availability::Optional),
                        type_: Some(fdecl::ConfigType {
                            layout: fdecl::ConfigTypeLayout::Bool,
                            parameters: None,
                            constraints: Vec::new(),
                        }),
                        ..Default::default()
                    }),
                ]);
                decl.config = Some(fdecl::ConfigSchema {
                    fields: Some(vec![]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0;32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("path".into())),
                    ..Default::default()
                     });
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::missing_field(DeclType::ConfigField, "foo"),
                ])),
            ],
        },
        test_validate_use_optional_config_bad_type => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Config(fdecl::UseConfiguration {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("abc".to_string()),
                        target_name: Some("foo".to_string()),
                        availability: Some(fdecl::Availability::Optional),
                        type_: Some(fdecl::ConfigType {
                            layout: fdecl::ConfigTypeLayout::Bool,
                            parameters: None,
                            constraints: Vec::new(),
                        }),
                        ..Default::default()
                    }),
                ]);
                decl.config = Some(fdecl::ConfigSchema {
                    fields: Some(vec![fdecl::ConfigField {
                        key: Some("foo".into()),
                        type_: Some(fdecl::ConfigType {
                            layout: fdecl::ConfigTypeLayout::Int16,
                            parameters: Some(Vec::new()),
                            constraints: Vec::new(),
                        }),
                        mutability: None,
                        ..Default::default()}]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0;32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("path".into())),
                    ..Default::default()
                     });
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::invalid_field(DeclType::ConfigField, "foo"),
                ])),
            ],
        },
    }

    #[cfg(fuchsia_api_level_at_least = "29")]
    test_validate_any_result! {
        test_validate_use_dictionary => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Dictionary(fdecl::UseDictionary {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("toolbox".to_string()),
                        target_path: Some("/svc".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Ok(()),
            ],
        },
        test_validate_use_dictionary_invalid_name => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Dictionary(fdecl::UseDictionary {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("toolbox@".to_string()),
                        target_path: Some("/svc".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![Error::invalid_field(DeclType::UseDictionary, "source_name")])),
            ],
        },
        test_validate_use_dictionary_invalid_path => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Dictionary(fdecl::UseDictionary {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("toolbox".to_string()),
                        target_path: Some("/svc@".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![Error::field_invalid_segment(DeclType::UseDictionary, "target_path")])),
            ],
        },
        test_validate_use_dictionary_disallows_pkg_overlap => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Dictionary(fdecl::UseDictionary {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("toolbox".to_string()),
                        target_path: Some("/pkg/toolbox".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::pkg_path_overlap(DeclType::UseDictionary, "/pkg/toolbox"),
                ])),
            ],
        },
        test_validate_use_dictionary_path_overlap_directory => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Dictionary(fdecl::UseDictionary {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("toolbox".to_string()),
                        target_path: Some("/foo/bar".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("data".to_string()),
                        target_path: Some("/foo".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseDirectory,
                        "/foo",
                        DeclType::UseDictionary,
                        "/foo/bar",
                    ),
                ])),
                Err(ErrorList::new(vec![
                    Error::invalid_path_overlap(
                        DeclType::UseDictionary,
                        "/foo/bar",
                        DeclType::UseDirectory,
                        "/foo",
                    ),
                ])),
            ],
        },
        test_validate_use_dictionary_path_overlap_protocol => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Dictionary(fdecl::UseDictionary {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("toolbox".to_string()),
                        target_path: Some("/svc/toolbox".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.examples.Echo".to_string()),
                        target_path: Some("/svc/fuchsia.examples.Echo".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Ok(()),
            ],
        },
        test_validate_use_dictionary_path_overlap_protocol_2 => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Dictionary(fdecl::UseDictionary {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("toolbox".to_string()),
                        target_path: Some("/svc".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.examples.Echo".to_string()),
                        target_path: Some("/svc/fuchsia.examples.Echo".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Ok(()),
            ],
        },
        test_validate_use_dictionary_path_identical_dictionary => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Dictionary(fdecl::UseDictionary {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("toolbox-1".to_string()),
                        target_path: Some("/svc".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Use::Dictionary(fdecl::UseDictionary {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("toolbox-2".to_string()),
                        target_path: Some("/svc".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.examples.Echo".to_string()),
                        target_path: Some("/svc/fuchsia.examples.Echo".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            results = vec![
                Ok(()),
            ],
        },
        test_validate_use_two_dictionaires_and_a_protocol_at_the_same_path => {
            input = {
                fdecl::Component {
                    program: Some(fdecl::Program {
                        runner: Some("realm_builder".to_string()),
                        info: Some(fdata::Dictionary {
                            entries: Some(vec![ ]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    uses: Some(vec![
                        fdecl::Use::Dictionary(fdecl::UseDictionary {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("my_dictionary".to_string()),
                            target_path: Some("/svc".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            source_dictionary: None,
                            ..Default::default()
                        }),
                        fdecl::Use::Dictionary(fdecl::UseDictionary {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                            source_name: Some("my_dictionary2".to_string()),
                            target_path: Some("/svc".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            source_dictionary: None,
                            ..Default::default()
                        }),
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                            source_name: Some("fuchsia.echo2".to_string()),
                            target_path: Some("/svc/fuchsia.echo2".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            source_dictionary: None,
                            numbered_handle: None,
                            ..Default::default()
                        }),
                    ]),
                    ..Default::default()
                }
            },
            results = vec![
                Ok(()),
            ],
        },
    }

    test_validate_values_data! {
        test_values_data_ok => {
            input = fdecl::ConfigValuesData {
                values: Some(vec![
                    fdecl::ConfigValueSpec {
                        value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Bool(true))),
                        ..Default::default()
                    }
                ]),
                checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                ..Default::default()
            },
            result = Ok(()),
        },
        test_values_data_no_checksum => {
            input = fdecl::ConfigValuesData {
                values: Some(vec![]),
                checksum: None,
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::ConfigValuesData, "checksum")
            ])),
        },
        test_values_data_unknown_checksum => {
            input = fdecl::ConfigValuesData {
                values: Some(vec![]),
                checksum: Some(fdecl::ConfigChecksum::unknown_variant_for_testing()),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::ConfigValuesData, "checksum")
            ])),
        },
        test_values_data_no_values => {
            input = fdecl::ConfigValuesData {
                values: None,
                checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::ConfigValuesData, "values")
            ])),
        },
        test_values_data_no_inner_value => {
            input = fdecl::ConfigValuesData {
                values: Some(vec![
                    fdecl::ConfigValueSpec::default()
                ]),
                checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::ConfigValueSpec, "value")
            ])),
        },
        test_values_data_unknown_inner_value => {
            input = fdecl::ConfigValuesData {
                values: Some(vec![
                    fdecl::ConfigValueSpec {
                        value: Some(fdecl::ConfigValue::unknown_variant_for_testing()),
                        ..Default::default()
                    }
                ]),
                checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::ConfigValueSpec, "value")
            ])),
        },
        test_values_data_unknown_single_value => {
            input = fdecl::ConfigValuesData {
                values: Some(vec![
                    fdecl::ConfigValueSpec {
                        value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::unknown_variant_for_testing())),
                        ..Default::default()
                    }
                ]),
                checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::ConfigValueSpec, "value")
            ])),
        },
        test_values_data_unknown_list_value => {
            input = fdecl::ConfigValuesData {
                values: Some(vec![
                    fdecl::ConfigValueSpec {
                        value: Some(fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::unknown_variant_for_testing())),
                        ..Default::default()
                    }
                ]),
                checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::ConfigValueSpec, "value")
            ])),
        },
    }

    test_validate! {
        // uses
        test_validate_uses_empty => {
            input = {
                let mut decl = new_component_decl();
                decl.program = Some(fdecl::Program {
                    runner: Some("elf".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
                decl.uses = Some(vec![
                    fdecl::Use::Service(fdecl::UseService {
                        source: None,
                        source_name: None,
                        target_path: None,
                        dependency_type: None,
                        ..Default::default()
                    }),
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        dependency_type: None,
                        source: None,
                        source_name: None,
                        target_path: None,
                        ..Default::default()
                    }),
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        dependency_type: None,
                        source: None,
                        source_name: None,
                        target_path: None,
                        rights: None,
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Use::Storage(fdecl::UseStorage {
                        source_name: None,
                        target_path: None,
                        ..Default::default()
                    }),
                    fdecl::Use::EventStream(fdecl::UseEventStream {
                        source_name: None,
                        source: None,
                        target_path: None,
                        ..Default::default()
                    }),
                    fdecl::Use::Runner(fdecl::UseRunner {
                        source_name: None,
                        source: None,
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::UseService, "source"),
                Error::missing_field(DeclType::UseService, "source_name"),
                Error::missing_field(DeclType::UseService, "target_path"),
                Error::missing_field(DeclType::UseService, "dependency_type"),
                Error::missing_field(DeclType::UseProtocol, "source"),
                Error::missing_field(DeclType::UseProtocol, "source_name"),
                Error::missing_field(DeclType::UseProtocol, "target_path"),
                Error::missing_field(DeclType::UseProtocol, "dependency_type"),
                Error::missing_field(DeclType::UseDirectory, "source"),
                Error::missing_field(DeclType::UseDirectory, "source_name"),
                Error::missing_field(DeclType::UseDirectory, "target_path"),
                Error::missing_field(DeclType::UseDirectory, "dependency_type"),
                Error::missing_field(DeclType::UseDirectory, "rights"),
                Error::missing_field(DeclType::UseStorage, "source_name"),
                Error::missing_field(DeclType::UseStorage, "target_path"),
                Error::missing_field(DeclType::UseEventStream, "source"),
                Error::missing_field(DeclType::UseEventStream, "source_name"),
                Error::missing_field(DeclType::UseEventStream, "target_path"),
                Error::missing_field(DeclType::UseRunner, "source"),
                Error::missing_field(DeclType::UseRunner, "source_name"),
            ])),
        },
        test_validate_missing_program_info => {
            input = {
                let mut decl = new_component_decl();
                decl.program = Some(fdecl::Program {
                    runner: Some("runner".to_string()),
                    info: None,
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::Program, "info")
            ])),
        },
        test_validate_uses_invalid_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Service(fdecl::UseService {
                        source: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                            name: "^bad".to_string(),
                        })),
                        source_name: Some("foo/".to_string()),
                        target_path: Some("a/foo".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("foo/".to_string()),
                        target_path: Some("b/foo".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("foo/".to_string()),
                        target_path: Some("c".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: Some("/foo".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Use::Storage(fdecl::UseStorage {
                        source_name: Some("foo/".to_string()),
                        target_path: Some("d".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Use::EventStream(fdecl::UseEventStream {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("foo/".to_string()),
                        target_path: Some("e".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Use::Runner(fdecl::UseRunner {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("foo/".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::UseService, "source.capability.name"),
                Error::invalid_field(DeclType::UseService, "source_name"),
                Error::invalid_field(DeclType::UseService, "target_path"),
                Error::invalid_field(DeclType::UseProtocol, "source.child.name"),
                Error::invalid_field(DeclType::UseProtocol, "source_name"),
                Error::invalid_field(DeclType::UseProtocol, "target_path"),
                Error::invalid_field(DeclType::UseDirectory, "source.child.name"),
                Error::invalid_field(DeclType::UseDirectory, "source_name"),
                Error::invalid_field(DeclType::UseDirectory, "target_path"),
                Error::invalid_field(DeclType::UseDirectory, "subdir"),
                Error::invalid_field(DeclType::UseStorage, "source_name"),
                Error::invalid_field(DeclType::UseStorage, "target_path"),
                Error::invalid_field(DeclType::UseEventStream, "source.child.name"),
                Error::invalid_field(DeclType::UseEventStream, "source_name"),
                Error::invalid_field(DeclType::UseEventStream, "target_path"),
                Error::invalid_field(DeclType::UseRunner, "source.child.name"),
                Error::invalid_field(DeclType::UseRunner, "source_name"),
            ])),
        },
        test_validate_uses_missing_source => {
            input = {
                fdecl::Component {
                    uses: Some(vec![
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                                name: "this-storage-doesnt-exist".to_string(),
                            })),
                            source_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            target_path: Some("/svc/fuchsia.sys2.StorageAdmin".to_string()),
                            ..Default::default()
                        })
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_capability(DeclType::UseProtocol, "source", "this-storage-doesnt-exist"),
            ])),
        },
        test_validate_uses_invalid_child => {
            input = {
                fdecl::Component {
                    uses: Some(vec![
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "no-such-child".to_string(), collection: None})),
                            source_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            target_path: Some("/svc/fuchsia.sys2.StorageAdmin".to_string()),
                            ..Default::default()
                        }),
                        fdecl::Use::Service(fdecl::UseService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "no-such-child".to_string(), collection: None})),
                            source_name: Some("service_name".to_string()),
                            target_path: Some("/svc/service_name".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }),
                        fdecl::Use::Directory(fdecl::UseDirectory {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "no-such-child".to_string(), collection: None})),
                            source_name: Some("DirectoryName".to_string()),
                            target_path: Some("/data/DirectoryName".to_string()),
                            rights: Some(fio::Operations::CONNECT),
                            subdir: None,
                            ..Default::default()
                        }),
                        fdecl::Use::Runner(fdecl::UseRunner {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "no-such-child".to_string(), collection: None})),
                            source_name: Some("RunnerName".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_child(DeclType::UseProtocol, "source", "no-such-child"),
                Error::invalid_child(DeclType::UseService, "source", "no-such-child"),
                Error::invalid_child(DeclType::UseDirectory, "source", "no-such-child"),
                Error::invalid_child(DeclType::UseRunner, "source", "no-such-child"),
            ])),
        },
        test_validate_uses_invalid_capability_from_self => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Service(fdecl::UseService {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("fuchsia.some.library.SomeService".into()),
                        target_path: Some("/svc/foo".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("fuchsia.some.library.SomeProtocol".into()),
                        target_path: Some("/svc/bar".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("dir".into()),
                        target_path: Some("/assets".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        rights: Some(fio::Operations::CONNECT),
                        ..Default::default()
                    }),
                    fdecl::Use::Runner(fdecl::UseRunner {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_elf".into()),
                        ..Default::default()
                    }),
                    fdecl::Use::Config(fdecl::UseConfiguration {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_config".into()),
                        target_name: Some("config".into()),
                        type_: Some(fdecl::ConfigType {
                            layout: fdecl::ConfigTypeLayout::Bool,
                            parameters: Some(Vec::new()),
                            constraints: Vec::new(),
                        }),
                        ..Default::default()
                    }),
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("fuchsia.some.library.SomeProtocol".into()),
                        source_dictionary: Some("dict/inner".into()),
                        target_path: Some("/svc/baz".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_capability(
                    DeclType::UseService,
                    "source",
                    "fuchsia.some.library.SomeService"),
                Error::invalid_capability(
                    DeclType::UseProtocol,
                    "source",
                    "fuchsia.some.library.SomeProtocol"),
                Error::invalid_capability(DeclType::UseDirectory, "source", "dir"),
                Error::invalid_capability(DeclType::UseRunner, "source", "source_elf"),
                Error::invalid_capability(DeclType::UseConfiguration, "source", "source_config"),
                Error::invalid_capability(DeclType::UseProtocol, "source", "dict"),
            ])),
        },
        test_validate_use_numbered_handle => {
            input = {
                fdecl::Component {
                    uses: Some(vec![
                        // Valid
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                            source_name: Some("fuchsia.logger.LogSink".into()),
                            numbered_handle: Some(0xab),
                            ..Default::default()
                        }),
                        // Invalid
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                            source_name: Some("fuchsia.logger.LogSink".into()),
                            target_path: Some("/svc/fuchsia.logger.LogSink".into()),
                            numbered_handle: Some(0xab),
                            ..Default::default()
                        }),
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::extraneous_field(DeclType::UseProtocol, "numbered_handle"),
            ])),
        },
        test_validate_use_from_child_offer_to_child_strong_cycle => {
            input = {
                fdecl::Component {
                    capabilities: Some(vec![
                        fdecl::Capability::Service(fdecl::Service {
                            name: Some("a".to_string()),
                            source_path: Some("/a".to_string()),
                            ..Default::default()
                        })]),
                    uses: Some(vec![
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "child".to_string(), collection: None})),
                            source_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            target_path: Some("/svc/fuchsia.sys2.StorageAdmin".to_string()),
                            ..Default::default()
                        }),
                        fdecl::Use::Service(fdecl::UseService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "child".to_string(), collection: None})),
                            source_name: Some("service_name".to_string()),
                            target_path: Some("/svc/service_name".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }),
                        fdecl::Use::Directory(fdecl::UseDirectory {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "child".to_string(), collection: None})),
                            source_name: Some("DirectoryName".to_string()),
                            target_path: Some("/data/DirectoryName".to_string()),
                            rights: Some(fio::Operations::CONNECT),
                            subdir: None,
                            ..Default::default()
                        }),
                    ]),
                    offers: Some(vec![
                        fdecl::Offer::Service(fdecl::OfferService {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            source_name: Some("a".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                            target_name: Some("a".to_string()),
                            ..Default::default()
                        })
                    ]),
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        }
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{self -> child child -> self}}"),
            ])),
        },
        test_validate_use_from_child_storage_no_cycle => {
            input = {
                fdecl::Component {
                    capabilities: Some(vec![
                        fdecl::Capability::Storage(fdecl::Storage {
                            name: Some("cdata".to_string()),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child2".to_string(), collection: None } )),
                            backing_dir: Some("minfs".to_string()),
                            storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                            ..Default::default()
                        }),
                        fdecl::Capability::Storage(fdecl::Storage {
                            name: Some("pdata".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                            backing_dir: Some("minfs".to_string()),
                            storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                            ..Default::default()
                        }),
                    ]),
                    uses: Some(vec![
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "child1".to_string(), collection: None})),
                            source_name: Some("a".to_string()),
                            target_path: Some("/svc/a".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    offers: Some(vec![
                        fdecl::Offer::Storage(fdecl::OfferStorage {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            source_name: Some("cdata".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None })),
                            target_name: Some("cdata".to_string()),
                            ..Default::default()
                        }),
                        fdecl::Offer::Storage(fdecl::OfferStorage {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            source_name: Some("pdata".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None })),
                            target_name: Some("pdata".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child1".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        },
                        fdecl::Child {
                            name: Some("child2".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo2".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        }
                    ]),
                    ..new_component_decl()
                }
            },
            result = Ok(()),
        },
        test_validate_use_from_child_storage_cycle => {
            input = {
                fdecl::Component {
                    capabilities: Some(vec![
                        fdecl::Capability::Storage(fdecl::Storage {
                            name: Some("data".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            backing_dir: Some("minfs".to_string()),
                            storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                            ..Default::default()
                        }),
                    ]),
                    uses: Some(vec![
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "child".to_string(), collection: None})),
                            source_name: Some("a".to_string()),
                            target_path: Some("/svc/a".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    offers: Some(vec![
                        fdecl::Offer::Storage(fdecl::OfferStorage {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            source_name: Some("data".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                            target_name: Some("data".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        },
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{self -> capability data -> child child -> self}}"),
            ])),
        },
        test_validate_storage_strong_cycle_between_children => {
            input = {
                fdecl::Component {
                    capabilities: Some(vec![
                        fdecl::Capability::Storage(fdecl::Storage {
                            name: Some("data".to_string()),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None } )),
                            backing_dir: Some("minfs".to_string()),
                            storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                            ..Default::default()
                        })
                    ]),
                    offers: Some(vec![
                        fdecl::Offer::Storage(fdecl::OfferStorage {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            source_name: Some("data".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child2".to_string(), collection: None })),
                            target_name: Some("data".to_string()),
                            ..Default::default()
                        }),
                        fdecl::Offer::Service(fdecl::OfferService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child2".to_string(), collection: None })),
                            source_name: Some("a".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None })),
                            target_name: Some("a".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child1".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        },
                        fdecl::Child {
                            name: Some("child2".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo2".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        }
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{child child1 -> capability data -> child child2 -> child child1}}"),
            ])),
        },
        test_validate_strong_cycle_between_children_through_environment_debug => {
            input = {
                fdecl::Component {
                    environments: Some(vec![
                        fdecl::Environment {
                            name: Some("env".to_string()),
                            extends: Some(fdecl::EnvironmentExtends::Realm),
                            debug_capabilities: Some(vec![
                                fdecl::DebugRegistration::Protocol(fdecl::DebugProtocolRegistration {
                                    source: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None })),
                                    source_name: Some("fuchsia.foo.Bar".to_string()),
                                    target_name: Some("fuchsia.foo.Bar".to_string()),
                                    ..Default::default()
                                }),
                            ]),
                            ..Default::default()
                        },
                    ]),
                    offers: Some(vec![
                        fdecl::Offer::Service(fdecl::OfferService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child2".to_string(), collection: None })),
                            source_name: Some("a".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None })),
                            target_name: Some("a".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child1".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        },
                        fdecl::Child {
                            name: Some("child2".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo2".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            environment: Some("env".to_string()),
                            on_terminate: None,
                            ..Default::default()
                        }
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{child child1 -> environment env -> child child2 -> child child1}}"),
            ])),
        },
        test_validate_strong_cycle_between_children_through_environment_runner => {
            input = {
                fdecl::Component {
                    environments: Some(vec![
                        fdecl::Environment {
                            name: Some("env".to_string()),
                            extends: Some(fdecl::EnvironmentExtends::Realm),
                            runners: Some(vec![
                                fdecl::RunnerRegistration {
                                    source: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None })),
                                    source_name: Some("coff".to_string()),
                                    target_name: Some("coff".to_string()),
                                    ..Default::default()
                                }
                            ]),
                            ..Default::default()
                        },
                    ]),
                    offers: Some(vec![
                        fdecl::Offer::Service(fdecl::OfferService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child2".to_string(), collection: None })),
                            source_name: Some("a".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None })),
                            target_name: Some("a".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child1".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        },
                        fdecl::Child {
                            name: Some("child2".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo2".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            environment: Some("env".to_string()),
                            on_terminate: None,
                            ..Default::default()
                        }
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{child child1 -> environment env -> child child2 -> child child1}}"),
            ])),
        },
        test_validate_strong_cycle_between_children_through_environment_resolver => {
            input = {
                fdecl::Component {
                    environments: Some(vec![
                        fdecl::Environment {
                            name: Some("env".to_string()),
                            extends: Some(fdecl::EnvironmentExtends::Realm),
                            resolvers: Some(vec![
                                fdecl::ResolverRegistration {
                                    resolver: Some("gopher".to_string()),
                                    source: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None })),
                                    scheme: Some("gopher".to_string()),
                                    ..Default::default()
                                }
                            ]),
                            ..Default::default()
                        },
                    ]),
                    offers: Some(vec![
                        fdecl::Offer::Service(fdecl::OfferService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child2".to_string(), collection: None })),
                            source_name: Some("a".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None })),
                            target_name: Some("a".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child1".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        },
                        fdecl::Child {
                            name: Some("child2".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo2".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            environment: Some("env".to_string()),
                            on_terminate: None,
                            ..Default::default()
                        }
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{child child1 -> environment env -> child child2 -> child child1}}"),
            ])),
        },
        test_validate_strong_cycle_between_self_and_two_children => {
            input = {
                fdecl::Component {
                    capabilities: Some(vec![
                        fdecl::Capability::Protocol(fdecl::Protocol {
                            name: Some("fuchsia.foo.Bar".to_string()),
                            source_path: Some("/svc/fuchsia.foo.Bar".to_string()),
                            ..Default::default()
                        })
                    ]),
                    offers: Some(vec![
                        fdecl::Offer::Protocol(fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            source_name: Some("fuchsia.foo.Bar".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None })),
                            target_name: Some("fuchsia.foo.Bar".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }),
                        fdecl::Offer::Protocol(fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child1".to_string(), collection: None })),
                            source_name: Some("fuchsia.bar.Baz".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child2".to_string(), collection: None })),
                            target_name: Some("fuchsia.bar.Baz".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }),
                    ]),
                    uses: Some(vec![
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "child2".to_string(), collection: None})),
                            source_name: Some("fuchsia.baz.Foo".to_string()),
                            target_path: Some("/svc/fuchsia.baz.Foo".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }),
                    ]),
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child1".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        },
                        fdecl::Child {
                            name: Some("child2".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo2".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        }
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{self -> child child1 -> child child2 -> self}}"),
            ])),
        },
        test_validate_strong_cycle_with_self_storage => {
            input = {
                fdecl::Component {
                    capabilities: Some(vec![
                        fdecl::Capability::Storage(fdecl::Storage {
                            name: Some("data".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            backing_dir: Some("minfs".to_string()),
                            storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                            ..Default::default()
                        }),
                        fdecl::Capability::Directory(fdecl::Directory {
                            name: Some("minfs".to_string()),
                            source_path: Some("/minfs".to_string()),
                            rights: Some(fio::RW_STAR_DIR),
                            ..Default::default()
                        }),
                    ]),
                    offers: Some(vec![
                        fdecl::Offer::Storage(fdecl::OfferStorage {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            source_name: Some("data".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                            target_name: Some("data".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    uses: Some(vec![
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "child".to_string(), collection: None})),
                            source_name: Some("fuchsia.foo.Bar".to_string()),
                            target_path: Some("/svc/fuchsia.foo.Bar".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            ..Default::default()
                        },
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{self -> capability data -> child child -> self}}"),
            ])),
        },
        test_validate_strong_cycle_with_self_storage_admin_protocol => {
            input = {
                fdecl::Component {
                    capabilities: Some(vec![
                        fdecl::Capability::Storage(fdecl::Storage {
                            name: Some("data".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            backing_dir: Some("minfs".to_string()),
                            storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                            ..Default::default()
                        }),
                        fdecl::Capability::Directory(fdecl::Directory {
                            name: Some("minfs".to_string()),
                            source_path: Some("/minfs".to_string()),
                            rights: Some(fio::RW_STAR_DIR),
                            ..Default::default()
                        }),
                    ]),
                    offers: Some(vec![
                        fdecl::Offer::Protocol(fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Capability(fdecl::CapabilityRef { name: "data".to_string() })),
                            source_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                            target_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }),
                    ]),
                    uses: Some(vec![
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "child".to_string(), collection: None})),
                            source_name: Some("fuchsia.foo.Bar".to_string()),
                            target_path: Some("/svc/fuchsia.foo.Bar".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            ..Default::default()
                        },
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{self -> capability data -> child child -> self}}"),
            ])),
        },
        test_validate_strong_cycle_with_dictionary => {
            input = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                        source_name: Some("dict".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "a".into(),
                            collection: None,
                        })),
                        target_name: Some("dict".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "b".into(),
                            collection: None,
                        })),
                        source_name: Some("1".into()),
                        target: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                            name: "dict".into(),
                        })),
                        target_name: Some("1".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "a".into(),
                            collection: None,
                        })),
                        source_name: Some("2".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "b".into(),
                            collection: None,
                        })),
                        target_name: Some("2".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("a".into()),
                        url: Some("fuchsia-pkg://child".into()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("b".into()),
                        url: Some("fuchsia-pkg://child".into()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Dictionary(fdecl::Dictionary {
                        name: Some("dict".into()),
                        ..Default::default()
                    }),
                ]),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{child a -> child b -> capability dict -> child a}}"),
            ])),
        },
        test_validate_strong_cycle_with_dictionary_indirect => {
            input = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                        source_name: Some("3".into()),
                        source_dictionary: Some("dict".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "a".into(),
                            collection: None,
                        })),
                        target_name: Some("3".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "b".into(),
                            collection: None,
                        })),
                        source_name: Some("1".into()),
                        target: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                            name: "dict".into(),
                        })),
                        target_name: Some("1".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "a".into(),
                            collection: None,
                        })),
                        source_name: Some("2".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "b".into(),
                            collection: None,
                        })),
                        target_name: Some("2".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("a".into()),
                        url: Some("fuchsia-pkg://child".into()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("b".into()),
                        url: Some("fuchsia-pkg://child".into()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Dictionary(fdecl::Dictionary {
                        name: Some("dict".into()),
                        ..Default::default()
                    }),
                ]),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{child a -> child b -> capability dict -> child a}}"),
            ])),
        },
        test_validate_use_dependency_cycle_with_dictionary => {
            input = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                        source_name: Some("dict".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "a".into(),
                            collection: None,
                        })),
                        target_name: Some("dict".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                        source_name: Some("1".into()),
                        target: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                            name: "dict".into(),
                        })),
                        target_name: Some("1".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("a".into()),
                        url: Some("fuchsia-pkg://child".into()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                uses: Some(vec![
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "a".to_string(), collection: None})),
                            source_name: Some("2".to_string()),
                            target_path: Some("/svc/foo".into()),
                            ..Default::default()
                        }),
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Dictionary(fdecl::Dictionary {
                        name: Some("dict".into()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("1".to_string()),
                        source_path: Some("/path".to_string()),
                        ..Default::default()
                    }),
                ]),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{self -> capability dict -> child a -> self}}"),
            ])),
        },
        test_validate_use_from_child_offer_to_child_weak_cycle => {
            input = {
                fdecl::Component {
                    capabilities: Some(vec![
                        fdecl::Capability::Service(fdecl::Service {
                            name: Some("a".to_string()),
                            source_path: Some("/a".to_string()),
                            ..Default::default()
                        })]),
                    uses: Some(vec![
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Weak),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "child".to_string(), collection: None})),
                            source_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            target_path: Some("/svc/fuchsia.sys2.StorageAdmin".to_string()),
                            ..Default::default()
                        }),
                        fdecl::Use::Service(fdecl::UseService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "child".to_string(), collection: None})),
                            source_name: Some("service_name".to_string()),
                            target_path: Some("/svc/service_name".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Weak),
                            ..Default::default()
                        }),
                        fdecl::Use::Directory(fdecl::UseDirectory {
                            dependency_type: Some(fdecl::DependencyType::Weak),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{ name: "child".to_string(), collection: None})),
                            source_name: Some("DirectoryName".to_string()),
                            target_path: Some("/data/DirectoryName".to_string()),
                            rights: Some(fio::Operations::CONNECT),
                            subdir: None,
                            ..Default::default()
                        }),
                    ]),
                    offers: Some(vec![
                        fdecl::Offer::Service(fdecl::OfferService {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            source_name: Some("a".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                            target_name: Some("a".to_string()),
                            ..Default::default()
                        })
                    ]),
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child".to_string()),
                            url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            ..Default::default()
                        }
                    ]),
                    ..new_component_decl()
                }
            },
            result = Ok(()),
        },
        test_validate_expose_from_self_to_framework_and_parent => {
            input = {
                fdecl::Component {
                    capabilities: Some(vec![
                        fdecl::Capability::Protocol(fdecl::Protocol {
                            name: Some("a".to_string()),
                            source_path: Some("/a".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    exposes: Some(vec![
                        fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            source_name: Some("a".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("a".to_string()),
                            ..Default::default()
                        }),
                        fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            source_name: Some("a".to_string()),
                            target: Some(fdecl::Ref::Framework(fdecl::FrameworkRef {})),
                            target_name: Some("a".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    ..new_component_decl()
                }
            },
            result = Ok(()),
        },
        test_validate_use_from_not_child_weak => {
            input = {
                fdecl::Component {
                    uses: Some(vec![
                        fdecl::Use::Protocol(fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Weak),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                            source_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            target_path: Some("/svc/fuchsia.sys2.StorageAdmin".to_string()),
                            ..Default::default()
                        }),
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::UseProtocol, "dependency_type"),
            ])),
        },
        test_validate_event_stream_offer_valid_decls => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("stopped".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        target_name: Some("stopped".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("started".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        target_name: Some("started".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![fdecl::Child{
                    name: Some("test".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger#meta/logger.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                },
                fdecl::Child{
                    name: Some("test2".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/fake_component#meta/fake_component.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }
                ]);
                decl
            },
            result = Ok(()),
        },
        test_validate_event_stream_offer_to_framework_invalid => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("stopped".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Framework(fdecl::FrameworkRef{})),
                        target_name: Some("stopped".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("started".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        target_name: Some("started".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![fdecl::Child{
                    name: Some("test".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger#meta/logger.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                },
                fdecl::Child{
                    name: Some("test2".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/fake_component#meta/fake_component.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::InvalidField(DeclField { decl: DeclType::OfferEventStream, field: "target".to_string() }),
                Error::InvalidField(DeclField { decl: DeclType::OfferEventStream, field: "target".to_string() }),
            ])),
        },
        test_validate_event_stream_offer_to_scope_zero_length_invalid => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("started".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        scope: Some(vec![]),
                        target_name: Some("started".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![fdecl::Child{
                    name: Some("test".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger#meta/logger.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                },
                fdecl::Child{
                    name: Some("test2".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/fake_component#meta/fake_component.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::InvalidField(DeclField { decl: DeclType::OfferEventStream, field: "scope".to_string() }),
            ])),
        },
        test_validate_event_stream_offer_to_scope_framework_invalid => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("started".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        scope: Some(vec![fdecl::Ref::Framework(fdecl::FrameworkRef{})]),
                        target_name: Some("started".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![fdecl::Child{
                    name: Some("test".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger#meta/logger.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                },
                fdecl::Child{
                    name: Some("test2".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/fake_component#meta/fake_component.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::InvalidField(DeclField { decl: DeclType::OfferEventStream, field: "scope".to_string() }),
            ])),
        },
        test_validate_event_stream_offer_to_scope_valid => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("started".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        scope: Some(vec![fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})]),
                        target_name: Some("started".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![fdecl::Child{
                    name: Some("test".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger#meta/logger.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                },
                fdecl::Child{
                    name: Some("test2".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/fake_component#meta/fake_component.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }
                ]);
                decl
            },
            result = Ok(()),
        },
        test_validate_event_stream_offer_to_scope_with_capability_requested => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("capability_requested".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        scope: Some(vec![fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})]),
                        target_name: Some("started".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![fdecl::Child{
                    name: Some("test".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger#meta/logger.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                },
                fdecl::Child{
                    name: Some("test2".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/fake_component#meta/fake_component.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }
                ]);
                decl
            },
            result = Ok(()),
        },
        test_validate_event_stream_offer_with_no_source_name_invalid => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: None,
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        scope: Some(vec![fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})]),
                        target_name: Some("started".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![fdecl::Child{
                    name: Some("test".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger#meta/logger.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                },
                fdecl::Child{
                    name: Some("test2".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/fake_component#meta/fake_component.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::MissingField(DeclField { decl: DeclType::OfferEventStream, field: "source_name".to_string() }),
            ])),
        },
        test_validate_event_stream_offer_invalid_source => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("stopped".to_string()),
                        source: Some(fdecl::Ref::Framework(fdecl::FrameworkRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        target_name: Some("stopped".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("started".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        target_name: Some("started".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("capability_requested".to_string()),
                        source: Some(fdecl::Ref::Debug(fdecl::DebugRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        target_name: Some("capability_requested".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![fdecl::Child{
                    name: Some("test".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger#meta/logger.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                },
                fdecl::Child{
                    name: Some("test2".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/fake_component#meta/fake_component.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::InvalidField(DeclField { decl: DeclType::OfferEventStream, field: "source".to_string() }),
            ])),
        },

        test_validate_event_stream_offer_missing_source => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("stopped".to_string()),
                        source: Some(fdecl::Ref::Framework(fdecl::FrameworkRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        target_name: Some("stopped".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("started".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        target_name: Some("started".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("capability_requested".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "test2".to_string(), collection: None})),
                        target_name: Some("capability_requested".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![fdecl::Child{
                    name: Some("test".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger#meta/logger.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                },
                fdecl::Child{
                    name: Some("test2".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/fake_component#meta/fake_component.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::MissingField(DeclField { decl: DeclType::OfferEventStream, field: "source".to_string() }),
            ])),
        },
        test_validate_event_stream_must_have_target_path => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::EventStream(fdecl::UseEventStream {
                        source_name: Some("bar".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::MissingField(DeclField { decl: DeclType::UseEventStream, field: "target_path".to_string() })
            ])),
        },
        test_validate_event_stream_must_have_source_names => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::EventStream(fdecl::UseEventStream {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target_path: Some("/svc/something".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::MissingField(DeclField { decl: DeclType::UseEventStream, field: "source_name".to_string() })
            ])),
        },
        test_validate_event_stream_scope_must_be_child_or_collection => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::EventStream(fdecl::UseEventStream {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target_path: Some("/svc/something".to_string()),
                        source_name: Some("some_source".to_string()),
                        scope: Some(vec![fdecl::Ref::Framework(fdecl::FrameworkRef{})]),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::InvalidField(DeclField { decl: DeclType::UseEventStream, field: "scope".to_string() })
            ])),
        },
        test_validate_event_stream_source_must_be_parent_or_child => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::EventStream(fdecl::UseEventStream {
                        source: Some(fdecl::Ref::Debug(fdecl::DebugRef{})),
                        target_path: Some("/svc/something".to_string()),
                        source_name: Some("some_source".to_string()),
                        scope: Some(vec![]),
                        ..Default::default()
                    }),
                    fdecl::Use::EventStream(fdecl::UseEventStream {
                        source: Some(fdecl::Ref::Framework(fdecl::FrameworkRef{})),
                        target_path: Some("/svc/something_else".to_string()),
                        source_name: Some("some_source".to_string()),
                        scope: Some(vec![]),
                        ..Default::default()
                    }),
                    fdecl::Use::EventStream(fdecl::UseEventStream {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        target_path: Some("/svc/yet_something_else".to_string()),
                        source_name: Some("some_source".to_string()),
                        scope: Some(vec![]),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::InvalidField(DeclField { decl: DeclType::UseEventStream, field: "source".to_string() }),
                Error::InvalidField(DeclField { decl: DeclType::UseEventStream, field: "source".to_string() }),
                Error::InvalidField(DeclField { decl: DeclType::UseEventStream, field: "source".to_string() })
            ])),
        },
        test_validate_no_runner => {
            input = {
                let mut decl = new_component_decl();
                decl.program = Some(fdecl::Program {
                    runner: None,
                    info: Some(fdata::Dictionary {
                        entries: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::MissingRunner,
            ])),
        },
        test_validate_uses_runner => {
            input = {
                let mut decl = new_component_decl();
                decl.program = Some(fdecl::Program {
                    runner: None,
                    info: Some(fdata::Dictionary {
                        entries: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
                decl.uses = Some(vec![
                    fdecl::Use::Runner(fdecl::UseRunner {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("runner".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Ok(()),
        },
        test_validate_program_and_uses_runner_match => {
            input = {
                let mut decl = new_component_decl();
                decl.program = Some(fdecl::Program {
                    runner: Some("runner".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
                decl.uses = Some(vec![
                    fdecl::Use::Runner(fdecl::UseRunner {
                        source: Some(fdecl::Ref::Environment(fdecl::EnvironmentRef {})),
                        source_name: Some("runner".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Ok(()),
        },
        test_validate_runner_names_conflict => {
            input = {
                let mut decl = new_component_decl();
                decl.program = Some(fdecl::Program {
                    runner: Some("runner".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
                decl.uses = Some(vec![
                    fdecl::Use::Runner(fdecl::UseRunner {
                        source: Some(fdecl::Ref::Environment(fdecl::EnvironmentRef {})),
                        source_name: Some("other.runner".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::ConflictingRunners,
            ])),
        },
        test_validate_uses_runner_not_environement => {
            input = {
                let mut decl = new_component_decl();
                decl.program = Some(fdecl::Program {
                    runner: Some("runner".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
                decl.uses = Some(vec![
                    fdecl::Use::Runner(fdecl::UseRunner {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("runner".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::ConflictingRunners,
            ])),
        },
        test_validate_uses_long_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.program = Some(fdecl::Program {
                    runner: Some("elf".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
                decl.uses = Some(vec![
                    fdecl::Use::Service(fdecl::UseService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some(format!("{}", "a".repeat(256))),
                        target_path: Some("/a".repeat(2048)),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some(format!("{}", "a".repeat(256))),
                        target_path: Some("/b".repeat(2048)),
                        ..Default::default()
                    }),
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some(format!("{}", "a".repeat(256))),
                        target_path: Some("/c".repeat(2048)),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Use::Storage(fdecl::UseStorage {
                        source_name: Some("cache".to_string()),
                        target_path: Some("/d".repeat(2048)),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::field_too_long(DeclType::UseService, "source_name"),
                Error::field_too_long(DeclType::UseService, "target_path"),
                Error::field_too_long(DeclType::UseProtocol, "source_name"),
                Error::field_too_long(DeclType::UseProtocol, "target_path"),
                Error::field_too_long(DeclType::UseDirectory, "source_name"),
                Error::field_too_long(DeclType::UseDirectory, "target_path"),
                Error::field_too_long(DeclType::UseStorage, "target_path"),
            ])),
        },
        test_validate_conflicting_paths => {
            input = {
                let mut decl = new_component_decl();
                decl.uses = Some(vec![
                    fdecl::Use::Service(fdecl::UseService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("foo".to_string()),
                        target_path: Some("/bar".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("space".to_string()),
                        target_path: Some("/bar".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Use::Directory(fdecl::UseDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("crow".to_string()),
                        target_path: Some("/bar".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::duplicate_field(DeclType::UseDirectory, "target_path", "/bar"),
                Error::duplicate_field(DeclType::UseProtocol, "target_path", "/bar"),
            ])),
        },
        // exposes
        test_validate_exposes_empty => {
            input = {
                let mut decl = new_component_decl();
                decl.exposes = Some(vec![
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: None,
                        source_name: None,
                        target_name: None,
                        target: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                        source: None,
                        source_name: None,
                        target_name: None,
                        target: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: None,
                        source_name: None,
                        target_name: None,
                        target: None,
                        rights: None,
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Runner(fdecl::ExposeRunner {
                        source: None,
                        source_name: None,
                        target: None,
                        target_name: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Resolver(fdecl::ExposeResolver {
                        source: None,
                        source_name: None,
                        target: None,
                        target_name: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Dictionary(fdecl::ExposeDictionary {
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::ExposeService, "source"),
                Error::missing_field(DeclType::ExposeService, "target"),
                Error::missing_field(DeclType::ExposeService, "source_name"),
                Error::missing_field(DeclType::ExposeService, "target_name"),
                Error::missing_field(DeclType::ExposeProtocol, "source"),
                Error::missing_field(DeclType::ExposeProtocol, "target"),
                Error::missing_field(DeclType::ExposeProtocol, "source_name"),
                Error::missing_field(DeclType::ExposeProtocol, "target_name"),
                Error::missing_field(DeclType::ExposeDirectory, "source"),
                Error::missing_field(DeclType::ExposeDirectory, "target"),
                Error::missing_field(DeclType::ExposeDirectory, "source_name"),
                Error::missing_field(DeclType::ExposeDirectory, "target_name"),
                Error::missing_field(DeclType::ExposeRunner, "source"),
                Error::missing_field(DeclType::ExposeRunner, "target"),
                Error::missing_field(DeclType::ExposeRunner, "source_name"),
                Error::missing_field(DeclType::ExposeRunner, "target_name"),
                Error::missing_field(DeclType::ExposeResolver, "source"),
                Error::missing_field(DeclType::ExposeResolver, "target"),
                Error::missing_field(DeclType::ExposeResolver, "source_name"),
                Error::missing_field(DeclType::ExposeResolver, "target_name"),
                Error::missing_field(DeclType::ExposeDictionary, "source"),
                Error::missing_field(DeclType::ExposeDictionary, "target"),
                Error::missing_field(DeclType::ExposeDictionary, "source_name"),
                Error::missing_field(DeclType::ExposeDictionary, "target_name"),
            ])),
        },
        test_validate_exposes_extraneous => {
            input = {
                let mut decl = new_component_decl();
                decl.exposes = Some(vec![
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("logger".to_string()),
                        target_name: Some("logger".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("legacy_logger".to_string()),
                        target_name: Some("legacy_logger".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("data".to_string()),
                        target_name: Some("data".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Runner(fdecl::ExposeRunner {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("elf".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("elf".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Resolver(fdecl::ExposeResolver {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("pkg".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("pkg".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Dictionary(fdecl::ExposeDictionary {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("dict".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("dict".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::extraneous_field(DeclType::ExposeService, "source.child.collection"),
                Error::extraneous_field(DeclType::ExposeProtocol, "source.child.collection"),
                Error::extraneous_field(DeclType::ExposeDirectory, "source.child.collection"),
                Error::extraneous_field(DeclType::ExposeRunner, "source.child.collection"),
                Error::extraneous_field(DeclType::ExposeResolver, "source.child.collection"),
                Error::extraneous_field(DeclType::ExposeDictionary, "source.child.collection"),
            ])),
        },
        test_validate_exposes_invalid_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.exposes = Some(vec![
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("foo/".to_string()),
                        target_name: Some("/".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("foo/".to_string()),
                        target_name: Some("/".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("foo/".to_string()),
                        target_name: Some("/".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: Some("/foo".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Runner(fdecl::ExposeRunner {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("/path".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("elf!".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Resolver(fdecl::ExposeResolver {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("/path".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("pkg!".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Dictionary(fdecl::ExposeDictionary {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("/path".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("pkg!".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::ExposeService, "source.child.name"),
                Error::invalid_field(DeclType::ExposeService, "source_name"),
                Error::invalid_field(DeclType::ExposeService, "target_name"),
                Error::invalid_field(DeclType::ExposeProtocol, "source.child.name"),
                Error::invalid_field(DeclType::ExposeProtocol, "source_name"),
                Error::invalid_field(DeclType::ExposeProtocol, "target_name"),
                Error::invalid_field(DeclType::ExposeDirectory, "source.child.name"),
                Error::invalid_field(DeclType::ExposeDirectory, "source_name"),
                Error::invalid_field(DeclType::ExposeDirectory, "target_name"),
                Error::invalid_field(DeclType::ExposeDirectory, "subdir"),
                Error::invalid_field(DeclType::ExposeRunner, "source.child.name"),
                Error::invalid_field(DeclType::ExposeRunner, "source_name"),
                Error::invalid_field(DeclType::ExposeRunner, "target_name"),
                Error::invalid_field(DeclType::ExposeResolver, "source.child.name"),
                Error::invalid_field(DeclType::ExposeResolver, "source_name"),
                Error::invalid_field(DeclType::ExposeResolver, "target_name"),
                Error::invalid_field(DeclType::ExposeDictionary, "source.child.name"),
                Error::invalid_field(DeclType::ExposeDictionary, "source_name"),
                Error::invalid_field(DeclType::ExposeDictionary, "target_name"),
            ])),
        },
        test_validate_exposes_invalid_source_target => {
            input = {
                let mut decl = new_component_decl();
                decl.children = Some(vec![fdecl::Child{
                    name: Some("logger".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger#meta/logger.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }]);
                decl.exposes = Some(vec![
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: None,
                        source_name: Some("a".to_string()),
                        target_name: Some("b".to_string()),
                        target: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("c".to_string()),
                        target_name: Some("d".to_string()),
                        target: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {name: "z".to_string()})),
                        source_name: Some("e".to_string()),
                        target_name: Some("f".to_string()),
                        target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {name: "z".to_string()})),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("g".to_string()),
                        target_name: Some("h".to_string()),
                        target: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Runner(fdecl::ExposeRunner {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("i".to_string()),
                        target: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                        target_name: Some("j".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Resolver(fdecl::ExposeResolver {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("k".to_string()),
                        target: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                        target_name: Some("l".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Dictionary(fdecl::ExposeDictionary {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        source_name: Some("m".to_string()),
                        target_name: Some("n".to_string()),
                        target: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::ExposeService, "source"),
                Error::missing_field(DeclType::ExposeService, "target"),
                Error::invalid_field(DeclType::ExposeProtocol, "source"),
                Error::invalid_field(DeclType::ExposeProtocol, "target"),
                Error::invalid_field(DeclType::ExposeDirectory, "source"),
                Error::invalid_field(DeclType::ExposeDirectory, "target"),
                Error::invalid_field(DeclType::ExposeDirectory, "source"),
                Error::invalid_field(DeclType::ExposeDirectory, "target"),
                Error::invalid_field(DeclType::ExposeRunner, "source"),
                Error::invalid_field(DeclType::ExposeRunner, "target"),
                Error::invalid_field(DeclType::ExposeResolver, "source"),
                Error::invalid_field(DeclType::ExposeResolver, "target"),
                Error::invalid_field(DeclType::ExposeDictionary, "target"),
            ])),
        },
        test_validate_exposes_invalid_source_collection => {
            input = {
                let mut decl = new_component_decl();
                decl.collections = Some(vec![fdecl::Collection{
                    name: Some("col".to_string()),
                    durability: Some(fdecl::Durability::Transient),
                    allowed_offers: None,
                    allow_long_names: None,
                    ..Default::default()
                }]);
                decl.exposes = Some(vec![
                    fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "col".to_string() })),
                        source_name: Some("a".to_string()),
                        target_name: Some("a".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {name: "col".to_string()})),
                        source_name: Some("b".to_string()),
                        target_name: Some("b".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Runner(fdecl::ExposeRunner {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {name: "col".to_string()})),
                        source_name: Some("c".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("c".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Resolver(fdecl::ExposeResolver {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {name: "col".to_string()})),
                        source_name: Some("d".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("d".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Dictionary(fdecl::ExposeDictionary {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {name: "col".to_string()})),
                        source_name: Some("e".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("e".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::ExposeProtocol, "source"),
                Error::invalid_field(DeclType::ExposeDirectory, "source"),
                Error::invalid_field(DeclType::ExposeRunner, "source"),
                Error::invalid_field(DeclType::ExposeResolver, "source"),
                Error::invalid_field(DeclType::ExposeDictionary, "source"),
            ])),
        },
        test_validate_exposes_sources_collection => {
            input = {
                let mut decl = new_component_decl();
                decl.collections = Some(vec![
                    fdecl::Collection {
                        name: Some("col".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        allowed_offers: Some(fdecl::AllowedOffers::StaticOnly),
                        allow_long_names: None,
                        ..Default::default()
                    }
                ]);
                decl.exposes = Some(vec![
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "col".to_string() })),
                        source_name: Some("a".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("a".to_string()),
                        ..Default::default()
                    })
                ]);
                decl
            },
            result = Ok(()),
        },
        test_validate_exposes_long_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.exposes = Some(vec![
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "b".repeat(256),
                            collection: None,
                        })),
                        source_name: Some(format!("{}", "a".repeat(1025))),
                        target_name: Some(format!("{}", "b".repeat(1025))),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "b".repeat(256),
                            collection: None,
                        })),
                        source_name: Some(format!("{}", "a".repeat(256))),
                        target_name: Some(format!("{}", "b".repeat(256))),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "b".repeat(256),
                            collection: None,
                        })),
                        source_name: Some(format!("{}", "a".repeat(256))),
                        target_name: Some(format!("{}", "b".repeat(256))),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Runner(fdecl::ExposeRunner {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "b".repeat(256),
                            collection: None,
                        })),
                        source_name: Some("a".repeat(256)),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("b".repeat(256)),
                        ..Default::default()
                    }),
                    fdecl::Expose::Resolver(fdecl::ExposeResolver {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "b".repeat(256),
                            collection: None,
                        })),
                        source_name: Some("a".repeat(256)),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("b".repeat(256)),
                        ..Default::default()
                    }),
                    fdecl::Expose::Dictionary(fdecl::ExposeDictionary {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "b".repeat(256),
                            collection: None,
                        })),
                        source_name: Some("a".repeat(256)),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("b".repeat(256)),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::field_too_long(DeclType::ExposeService, "source.child.name"),
                Error::field_too_long(DeclType::ExposeService, "source_name"),
                Error::field_too_long(DeclType::ExposeService, "target_name"),
                Error::field_too_long(DeclType::ExposeProtocol, "source.child.name"),
                Error::field_too_long(DeclType::ExposeProtocol, "source_name"),
                Error::field_too_long(DeclType::ExposeProtocol, "target_name"),
                Error::field_too_long(DeclType::ExposeDirectory, "source.child.name"),
                Error::field_too_long(DeclType::ExposeDirectory, "source_name"),
                Error::field_too_long(DeclType::ExposeDirectory, "target_name"),
                Error::field_too_long(DeclType::ExposeRunner, "source.child.name"),
                Error::field_too_long(DeclType::ExposeRunner, "source_name"),
                Error::field_too_long(DeclType::ExposeRunner, "target_name"),
                Error::field_too_long(DeclType::ExposeResolver, "source.child.name"),
                Error::field_too_long(DeclType::ExposeResolver, "source_name"),
                Error::field_too_long(DeclType::ExposeResolver, "target_name"),
                Error::field_too_long(DeclType::ExposeDictionary, "source.child.name"),
                Error::field_too_long(DeclType::ExposeDictionary, "source_name"),
                Error::field_too_long(DeclType::ExposeDictionary, "target_name"),
            ])),
        },
        test_validate_exposes_invalid_child => {
            input = {
                let mut decl = new_component_decl();
                decl.exposes = Some(vec![
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        })),
                        source_name: Some("fuchsia.logger.LegacyLog".to_string()),
                        target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        })),
                        source_name: Some("data".to_string()),
                        target_name: Some("data".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Runner(fdecl::ExposeRunner {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        })),
                        source_name: Some("elf".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("elf".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Resolver(fdecl::ExposeResolver {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        })),
                        source_name: Some("pkg".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("pkg".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Dictionary(fdecl::ExposeDictionary {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        })),
                        source_name: Some("dict".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("dict".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_child(DeclType::ExposeService, "source", "netstack"),
                Error::invalid_child(DeclType::ExposeProtocol, "source", "netstack"),
                Error::invalid_child(DeclType::ExposeDirectory, "source", "netstack"),
                Error::invalid_child(DeclType::ExposeRunner, "source", "netstack"),
                Error::invalid_child(DeclType::ExposeResolver, "source", "netstack"),
                Error::invalid_child(DeclType::ExposeDictionary, "source", "netstack"),
            ])),
        },
        test_validate_exposes_invalid_source_capability => {
            input = {
                fdecl::Component {
                    exposes: Some(vec![
                        fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                                name: "this-storage-doesnt-exist".to_string(),
                            })),
                            source_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            target_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            ..Default::default()
                        }),
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_capability(DeclType::ExposeProtocol, "source", "this-storage-doesnt-exist"),
            ])),
        },
        test_validate_exposes_duplicate_target => {
            input = {
                let mut decl = new_component_decl();
                decl.exposes = Some(vec![
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll".into(),
                        })),
                        source_name: Some("netstack".to_string()),
                        target_name: Some("fuchsia.net.Stack".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll2".into(),
                        })),
                        source_name: Some("netstack2".to_string()),
                        target_name: Some("fuchsia.net.Stack".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("fonts".to_string()),
                        target_name: Some("fuchsia.fonts.Provider".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("fonts2".to_string()),
                        target_name: Some("fuchsia.fonts.Provider".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        ..Default::default()
                    }),
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("assets".to_string()),
                        target_name: Some("stuff".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        rights: None,
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("assets2".to_string()),
                        target_name: Some("stuff".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        rights: None,
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Runner(fdecl::ExposeRunner {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_elf".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("elf".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Runner(fdecl::ExposeRunner {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_elf".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("elf".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Resolver(fdecl::ExposeResolver {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_pkg".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("pkg".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Resolver(fdecl::ExposeResolver {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_pkg".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("pkg".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Dictionary(fdecl::ExposeDictionary {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_dict".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("dict".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Dictionary(fdecl::ExposeDictionary {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_dict".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("dict".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl.collections = Some(vec![
                    fdecl::Collection {
                        name: Some("coll".into()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                    fdecl::Collection {
                        name: Some("coll2".into()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                ]);
                decl.capabilities = Some(vec![
                    fdecl::Capability::Service(fdecl::Service {
                        name: Some("netstack".to_string()),
                        source_path: Some("/path".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Service(fdecl::Service {
                        name: Some("netstack2".to_string()),
                        source_path: Some("/path".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("fonts".to_string()),
                        source_path: Some("/path".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("fonts2".to_string()),
                        source_path: Some("/path".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: Some("assets".to_string()),
                        source_path: Some("/path".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        ..Default::default()
                    }),
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: Some("assets2".to_string()),
                        source_path: Some("/path".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        ..Default::default()
                    }),
                    fdecl::Capability::Runner(fdecl::Runner {
                        name: Some("source_elf".to_string()),
                        source_path: Some("/path".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Resolver(fdecl::Resolver {
                        name: Some("source_pkg".to_string()),
                        source_path: Some("/path".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Dictionary(fdecl::Dictionary {
                        name: Some("source_dict".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                // Duplicate services are allowed.
                Error::duplicate_field(DeclType::ExposeProtocol, "target_name",
                                    "fuchsia.fonts.Provider"),
                Error::duplicate_field(DeclType::ExposeDirectory, "target_name",
                                    "stuff"),
                Error::duplicate_field(DeclType::ExposeRunner, "target_name",
                                    "elf"),
                Error::duplicate_field(DeclType::ExposeResolver, "target_name", "pkg"),
                Error::duplicate_field(DeclType::ExposeDictionary, "target_name", "dict"),
            ])),
        },
        test_validate_exposes_invalid_capability_from_self => {
            input = {
                let mut decl = new_component_decl();
                decl.exposes = Some(vec![
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("fuchsia.some.library.SomeProtocol".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("foo".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("fuchsia.some.library.SomeProtocol".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("bar".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("dir".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("assets".to_string()),
                        rights: None,
                        subdir: None,
                        ..Default::default()
                    }),
                    fdecl::Expose::Runner(fdecl::ExposeRunner {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_elf".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("elf".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Resolver(fdecl::ExposeResolver {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_pkg".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("pkg".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Dictionary(fdecl::ExposeDictionary {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_dict".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("dict".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Config(fdecl::ExposeConfiguration {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_config".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("config".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("fuchsia.some.library.SomeProtocol".to_string()),
                        source_dictionary: Some("dict/inner".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("baz".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_capability(
                    DeclType::ExposeService,
                    "source",
                    "fuchsia.some.library.SomeProtocol"),
                Error::invalid_capability(
                    DeclType::ExposeProtocol,
                    "source",
                    "fuchsia.some.library.SomeProtocol"),
                Error::invalid_capability(DeclType::ExposeDirectory, "source", "dir"),
                Error::invalid_capability(DeclType::ExposeRunner, "source", "source_elf"),
                Error::invalid_capability(DeclType::ExposeResolver, "source", "source_pkg"),
                Error::invalid_capability(DeclType::ExposeDictionary, "source", "source_dict"),
                Error::invalid_capability(DeclType::ExposeConfig, "source", "source_config"),
                Error::invalid_capability(DeclType::ExposeProtocol, "source", "dict"),
            ])),
        },

        test_validate_exposes_availability_service => {
            input = {
                let mut decl = generate_expose_different_source_and_availability_decl(
                    |source, availability, target_name|
                        fdecl::Expose::Service(fdecl::ExposeService {
                            source: Some(source),
                            source_name: Some("fuchsia.examples.Echo".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some(target_name.to_string()),
                            availability: Some(availability),
                            ..Default::default()
                        })
                );
                decl.capabilities = Some(vec![
                    fdecl::Capability::Service(fdecl::Service {
                        name: Some("fuchsia.examples.Echo".to_string()),
                        source_path: Some("/svc/fuchsia.examples.Echo".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = {
                Err(ErrorList::new(vec![
                    Error::availability_must_be_optional(
                        DeclType::ExposeService,
                        "availability",
                        Some(&"fuchsia.examples.Echo".to_string()),
                    ),
                    Error::availability_must_be_optional(
                        DeclType::ExposeService,
                        "availability",
                        Some(&"fuchsia.examples.Echo".to_string()),
                    ),
                ]))
            },
        },
        test_validate_exposes_availability_protocol => {
            input = {
                let mut decl = generate_expose_different_source_and_availability_decl(
                    |source, availability, target_name|
                        fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                            source: Some(source),
                            source_name: Some("fuchsia.examples.Echo".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some(target_name.to_string()),
                            availability: Some(availability),
                            ..Default::default()
                        })
                );
                decl.capabilities = Some(vec![
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("fuchsia.examples.Echo".to_string()),
                        source_path: Some("/svc/fuchsia.examples.Echo".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = {
                Err(ErrorList::new(vec![
                    Error::availability_must_be_optional(
                        DeclType::ExposeProtocol,
                        "availability",
                        Some(&"fuchsia.examples.Echo".to_string()),
                    ),
                    Error::availability_must_be_optional(
                        DeclType::ExposeProtocol,
                        "availability",
                        Some(&"fuchsia.examples.Echo".to_string()),
                    ),
                ]))
            },
        },
        test_validate_exposes_availability_directory => {
            input = {
                let mut decl = generate_expose_different_source_and_availability_decl(
                    |source, availability, target_name|
                        fdecl::Expose::Directory(fdecl::ExposeDirectory {
                            source: Some(source),
                            source_name: Some("fuchsia.examples.Echo".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some(target_name.to_string()),
                            availability: Some(availability),
                            ..Default::default()
                        })
                );
                decl.capabilities = Some(vec![
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: Some("fuchsia.examples.Echo".to_string()),
                        source_path: Some("/svc/fuchsia.examples.Echo".to_string()),
                        rights: Some(fio::Operations::READ_BYTES),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = {
                Err(ErrorList::new(vec![
                    Error::availability_must_be_optional(
                        DeclType::ExposeDirectory,
                        "availability",
                        Some(&"fuchsia.examples.Echo".to_string()),
                    ),
                    Error::availability_must_be_optional(
                        DeclType::ExposeDirectory,
                        "availability",
                        Some(&"fuchsia.examples.Echo".to_string()),
                    ),
                ]))
            },
        },

        // offers
        test_validate_offers_empty => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: None,
                        source_name: None,
                        target: None,
                        target_name: None,
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: None,
                        source_name: None,
                        target: None,
                        target_name: None,
                        dependency_type: None,
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: None,
                        source_name: None,
                        target: None,
                        target_name: None,
                        rights: None,
                        subdir: None,
                        dependency_type: None,
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source_name: None,
                        source: None,
                        target: None,
                        target_name: None,
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: None,
                        source_name: None,
                        target: None,
                        target_name: None,
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        ..Default::default()
                    }),
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        ..Default::default()
                    }),
                ]);
                decl
            },
            // TODO(dgonyeo): we need to handle the availability being unset until we've soft
            // migrated all manifests
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::OfferService, "source"),
                Error::missing_field(DeclType::OfferService, "source_name"),
                Error::missing_field(DeclType::OfferService, "target"),
                Error::missing_field(DeclType::OfferService, "target_name"),
                //Error::missing_field(DeclType::OfferService, "availability"),
                Error::missing_field(DeclType::OfferProtocol, "source"),
                Error::missing_field(DeclType::OfferProtocol, "source_name"),
                Error::missing_field(DeclType::OfferProtocol, "target"),
                Error::missing_field(DeclType::OfferProtocol, "target_name"),
                Error::missing_field(DeclType::OfferProtocol, "dependency_type"),
                //Error::missing_field(DeclType::OfferProtocol, "availability"),
                Error::missing_field(DeclType::OfferDirectory, "source"),
                Error::missing_field(DeclType::OfferDirectory, "source_name"),
                Error::missing_field(DeclType::OfferDirectory, "target"),
                Error::missing_field(DeclType::OfferDirectory, "target_name"),
                Error::missing_field(DeclType::OfferDirectory, "dependency_type"),
                //Error::missing_field(DeclType::OfferDirectory, "availability"),
                Error::missing_field(DeclType::OfferStorage, "source"),
                Error::missing_field(DeclType::OfferStorage, "source_name"),
                Error::missing_field(DeclType::OfferStorage, "target"),
                Error::missing_field(DeclType::OfferStorage, "target_name"),
                //Error::missing_field(DeclType::OfferStorage, "availability"),
                Error::missing_field(DeclType::OfferRunner, "source"),
                Error::missing_field(DeclType::OfferRunner, "source_name"),
                Error::missing_field(DeclType::OfferRunner, "target"),
                Error::missing_field(DeclType::OfferRunner, "target_name"),
                //Error::missing_field(DeclType::OfferRunner, "availability"),
                Error::missing_field(DeclType::OfferResolver, "source"),
                Error::missing_field(DeclType::OfferResolver, "source_name"),
                Error::missing_field(DeclType::OfferResolver, "target"),
                Error::missing_field(DeclType::OfferResolver, "target_name"),
                Error::missing_field(DeclType::OfferDictionary, "source"),
                Error::missing_field(DeclType::OfferDictionary, "source_name"),
                Error::missing_field(DeclType::OfferDictionary, "target"),
                Error::missing_field(DeclType::OfferDictionary, "target_name"),
                Error::missing_field(DeclType::OfferDictionary, "dependency_type"),
            ])),
        },
        test_validate_offers_long_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "a".repeat(256),
                            collection: None,
                        })),
                        source_name: Some(format!("{}", "a".repeat(256))),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "b".repeat(256),
                            collection: None,
                        }
                        )),
                        target_name: Some(format!("{}", "b".repeat(256))),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("a".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef {
                            name: "b".repeat(256),
                        }
                        )),
                        target_name: Some(format!("{}", "b".repeat(256))),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "a".repeat(256),
                            collection: None,
                        })),
                        source_name: Some(format!("{}", "a".repeat(256))),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "b".repeat(256),
                            collection: None,
                        }
                        )),
                        target_name: Some(format!("{}", "b".repeat(256))),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("a".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef {
                            name: "b".repeat(256),
                        }
                        )),
                        target_name: Some(format!("{}", "b".repeat(256))),
                        dependency_type: Some(fdecl::DependencyType::Weak),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "a".repeat(256),
                            collection: None,
                        })),
                        source_name: Some(format!("{}", "a".repeat(256))),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "b".repeat(256),
                            collection: None,
                        }
                        )),
                        target_name: Some(format!("{}", "b".repeat(256))),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("a".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef {
                            name: "b".repeat(256),
                        }
                        )),
                        target_name: Some(format!("{}", "b".repeat(256))),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        dependency_type: Some(fdecl::DependencyType::Weak),
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source_name: Some("data".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "b".repeat(256),
                                collection: None,
                            }
                        )),
                        target_name: Some("data".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source_name: Some("data".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target: Some(fdecl::Ref::Collection(
                            fdecl::CollectionRef { name: "b".repeat(256) }
                        )),
                        target_name: Some("data".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "a".repeat(256),
                            collection: None,
                        })),
                        source_name: Some("b".repeat(256)),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef {
                            name: "c".repeat(256),
                        }
                        )),
                        target_name: Some("d".repeat(256)),
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "a".repeat(256),
                            collection: None,
                        })),
                        source_name: Some("b".repeat(256)),
                        target: Some(fdecl::Ref::Collection(
                            fdecl::CollectionRef {
                                name: "c".repeat(256),
                            }
                        )),
                        target_name: Some("d".repeat(256)),
                        ..Default::default()
                    }),
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "a".repeat(256),
                            collection: None,
                        })),
                        source_name: Some("b".repeat(256)),
                        target: Some(fdecl::Ref::Collection(
                            fdecl::CollectionRef {
                                name: "c".repeat(256),
                            }
                        )),
                        target_name: Some("d".repeat(256)),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::field_too_long(DeclType::OfferService, "source.child.name"),
                Error::field_too_long(DeclType::OfferService, "source_name"),
                Error::field_too_long(DeclType::OfferService, "target.child.name"),
                Error::field_too_long(DeclType::OfferService, "target_name"),
                Error::field_too_long(DeclType::OfferService, "target.collection.name"),
                Error::field_too_long(DeclType::OfferService, "target_name"),
                Error::field_too_long(DeclType::OfferProtocol, "source.child.name"),
                Error::field_too_long(DeclType::OfferProtocol, "source_name"),
                Error::field_too_long(DeclType::OfferProtocol, "target.child.name"),
                Error::field_too_long(DeclType::OfferProtocol, "target_name"),
                Error::field_too_long(DeclType::OfferProtocol, "target.collection.name"),
                Error::field_too_long(DeclType::OfferProtocol, "target_name"),
                Error::field_too_long(DeclType::OfferDirectory, "source.child.name"),
                Error::field_too_long(DeclType::OfferDirectory, "source_name"),
                Error::field_too_long(DeclType::OfferDirectory, "target.child.name"),
                Error::field_too_long(DeclType::OfferDirectory, "target_name"),
                Error::field_too_long(DeclType::OfferDirectory, "target.collection.name"),
                Error::field_too_long(DeclType::OfferDirectory, "target_name"),
                Error::field_too_long(DeclType::OfferStorage, "target.child.name"),
                Error::field_too_long(DeclType::OfferStorage, "target.collection.name"),
                Error::field_too_long(DeclType::OfferRunner, "source.child.name"),
                Error::field_too_long(DeclType::OfferRunner, "source_name"),
                Error::field_too_long(DeclType::OfferRunner, "target.collection.name"),
                Error::field_too_long(DeclType::OfferRunner, "target_name"),
                Error::field_too_long(DeclType::OfferResolver, "source.child.name"),
                Error::field_too_long(DeclType::OfferResolver, "source_name"),
                Error::field_too_long(DeclType::OfferResolver, "target.collection.name"),
                Error::field_too_long(DeclType::OfferResolver, "target_name"),
                Error::field_too_long(DeclType::OfferDictionary, "source.child.name"),
                Error::field_too_long(DeclType::OfferDictionary, "source_name"),
                Error::field_too_long(DeclType::OfferDictionary, "target.collection.name"),
                Error::field_too_long(DeclType::OfferDictionary, "target_name"),
            ])),
        },
        test_validate_offers_extraneous => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: Some("modular".to_string()),
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: Some("modular".to_string()),
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("assets".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: Some("modular".to_string()),
                            }
                        )),
                        target_name: Some("assets".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        dependency_type: Some(fdecl::DependencyType::Weak),
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source_name: Some("data".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{ })),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: Some("modular".to_string()),
                            }
                        )),
                        target_name: Some("data".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("elf".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: Some("modular".to_string()),
                            }
                        )),
                        target_name: Some("elf".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("pkg".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: Some("modular".to_string()),
                            }
                        )),
                        target_name: Some("pkg".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: Some("modular".to_string()),
                        })),
                        source_name: Some("dict".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: Some("modular".to_string()),
                            }
                        )),
                        target_name: Some("dict".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]);
                decl.capabilities = Some(vec![
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("fuchsia.logger.Log".to_string()),
                        source_path: Some("/svc/logger".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: Some("assets".to_string()),
                        source_path: Some("/data/assets".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::extraneous_field(DeclType::OfferService, "source.child.collection"),
                Error::extraneous_field(DeclType::OfferService, "target.child.collection"),
                Error::extraneous_field(DeclType::OfferProtocol, "source.child.collection"),
                Error::extraneous_field(DeclType::OfferProtocol, "target.child.collection"),
                Error::extraneous_field(DeclType::OfferDirectory, "source.child.collection"),
                Error::extraneous_field(DeclType::OfferDirectory, "target.child.collection"),
                Error::extraneous_field(DeclType::OfferStorage, "target.child.collection"),
                Error::extraneous_field(DeclType::OfferRunner, "source.child.collection"),
                Error::extraneous_field(DeclType::OfferRunner, "target.child.collection"),
                Error::extraneous_field(DeclType::OfferResolver, "source.child.collection"),
                Error::extraneous_field(DeclType::OfferResolver, "target.child.collection"),
                Error::extraneous_field(DeclType::OfferDictionary, "source.child.collection"),
                Error::extraneous_field(DeclType::OfferDictionary, "target.child.collection"),
            ])),
        },
        test_validate_offers_invalid_filtered_service_fields => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        source_instance_filter: Some(vec![]),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log2".to_string()),
                        source_instance_filter: Some(vec!["^badname".to_string()]),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log1".to_string()),
                        renamed_instances: Some(vec![fdecl::NameMapping{source_name: "a".to_string(), target_name: "b".to_string()}, fdecl::NameMapping{source_name: "c".to_string(), target_name: "b".to_string()}]),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log3".to_string()),
                        renamed_instances: Some(vec![
                            fdecl::NameMapping {
                                source_name: "^badname".to_string(),
                                target_name: "^badname".to_string(),
                            }
                        ]),
                        ..Default::default()
                    })
                ]);
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        environment: None,
                        ..Default::default()
                    },
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::OfferService, "source_instance_filter"),
                Error::invalid_field(DeclType::OfferService, "source_instance_filter"),
                Error::invalid_field(DeclType::OfferService, "renamed_instances"),
                Error::invalid_field(DeclType::OfferService, "renamed_instances.source_name"),
                Error::invalid_field(DeclType::OfferService, "renamed_instances.target_name"),
            ])),
        },
        test_validate_offers_invalid_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("foo/".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "%bad".to_string(),
                            collection: None,
                        })),
                        target_name: Some("/".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("foo/".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "%bad".to_string(),
                            collection: None,
                        })),
                        target_name: Some("/".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("foo/".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "%bad".to_string(),
                            collection: None,
                        })),
                        target_name: Some("/".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: Some("/foo".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("/path".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "%bad".to_string(),
                            collection: None,
                        })),
                        target_name: Some("elf!".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("/path".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "%bad".to_string(),
                            collection: None,
                        })),
                        target_name: Some("pkg!".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "^bad".to_string(),
                            collection: None,
                        })),
                        source_name: Some("/path".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "%bad".to_string(),
                            collection: None,
                        })),
                        target_name: Some("pkg!".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::OfferService, "source.child.name"),
                Error::invalid_field(DeclType::OfferService, "source_name"),
                Error::invalid_field(DeclType::OfferService, "target.child.name"),
                Error::invalid_field(DeclType::OfferService, "target_name"),
                Error::invalid_field(DeclType::OfferProtocol, "source.child.name"),
                Error::invalid_field(DeclType::OfferProtocol, "source_name"),
                Error::invalid_field(DeclType::OfferProtocol, "target.child.name"),
                Error::invalid_field(DeclType::OfferProtocol, "target_name"),
                Error::invalid_field(DeclType::OfferDirectory, "source.child.name"),
                Error::invalid_field(DeclType::OfferDirectory, "source_name"),
                Error::invalid_field(DeclType::OfferDirectory, "target.child.name"),
                Error::invalid_field(DeclType::OfferDirectory, "target_name"),
                Error::invalid_field(DeclType::OfferDirectory, "subdir"),
                Error::invalid_field(DeclType::OfferRunner, "source.child.name"),
                Error::invalid_field(DeclType::OfferRunner, "source_name"),
                Error::invalid_field(DeclType::OfferRunner, "target.child.name"),
                Error::invalid_field(DeclType::OfferRunner, "target_name"),
                Error::invalid_field(DeclType::OfferResolver, "source.child.name"),
                Error::invalid_field(DeclType::OfferResolver, "source_name"),
                Error::invalid_field(DeclType::OfferResolver, "target.child.name"),
                Error::invalid_field(DeclType::OfferResolver, "target_name"),
                Error::invalid_field(DeclType::OfferDictionary, "source.child.name"),
                Error::invalid_field(DeclType::OfferDictionary, "source_name"),
                Error::invalid_field(DeclType::OfferDictionary, "target.child.name"),
                Error::invalid_field(DeclType::OfferDictionary, "target_name"),
            ])),
        },
        test_validate_offers_target_equals_source => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        source_name: Some("logger".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("logger".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        source_name: Some("legacy_logger".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("weak_legacy_logger".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Weak),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        source_name: Some("legacy_logger".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("strong_legacy_logger".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        source_name: Some("assets".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("assets".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        source_name: Some("web".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("web".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        source_name: Some("pkg".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("pkg".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        source_name: Some("dict".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("dict".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![fdecl::Child{
                    name: Some("logger".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger#meta/logger.cm".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{child logger -> child logger}}"),
            ])),
        },
        test_validate_offers_storage_target_equals_source => {
            input = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source_name: Some("data".to_string()),
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef { })),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("data".to_string()),
                        ..Default::default()
                    })
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Storage(fdecl::Storage {
                        name: Some("data".to_string()),
                        backing_dir: Some("minfs".to_string()),
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        subdir: None,
                        storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        environment: None,
                        ..Default::default()
                    },
                ]),
                ..new_component_decl()
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{child logger -> capability data -> child logger}}"),
            ])),
        },
        test_validate_offers_invalid_child => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        source_name: Some("fuchsia.logger.LegacyLog".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        source_name: Some("assets".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string() }
                        )),
                        target_name: Some("assets".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        dependency_type: Some(fdecl::DependencyType::Weak),
                        ..Default::default()
                    }),
                ]);
                decl.capabilities = Some(vec![
                    fdecl::Capability::Storage(fdecl::Storage {
                        name: Some("memfs".to_string()),
                        backing_dir: Some("memfs".to_string()),
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".to_string(),
                            collection: None,
                        })),
                        subdir: None,
                        storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("netstack".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/netstack/stable#meta/netstack.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        environment: None,
                        ..Default::default()
                    },
                ]);
                decl.collections = Some(vec![
                    fdecl::Collection {
                        name: Some("modular".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        environment: None,
                        allowed_offers: Some(fdecl::AllowedOffers::StaticAndDynamic),
                        allow_long_names: None,
                        ..Default::default()
                    },
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_child(DeclType::Storage, "source", "logger"),
                Error::invalid_child(DeclType::OfferService, "source", "logger"),
                Error::invalid_child(DeclType::OfferProtocol, "source", "logger"),
                Error::invalid_child(DeclType::OfferDirectory, "source", "logger"),
            ])),
        },
        test_validate_offers_invalid_source_capability => {
            input = {
                fdecl::Component {
                    offers: Some(vec![
                        fdecl::Offer::Protocol(fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                                name: "this-storage-doesnt-exist".to_string(),
                            })),
                            source_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            }
                            )),
                            target_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }),
                    ]),
                    ..new_component_decl()
                }
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_capability(DeclType::OfferProtocol, "source", "this-storage-doesnt-exist"),
                Error::invalid_child(DeclType::OfferProtocol, "target", "netstack"),
            ])),
        },
        test_validate_offers_target => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "modular".into()
                        })),
                        source_name: Some("logger".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "modular".into()
                        })),
                        source_name: Some("logger".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("fuchsia.logger.LegacyLog".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("fuchsia.logger.LegacyLog".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("assets".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string() }
                        )),
                        target_name: Some("assets".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("assets".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string() }
                        )),
                        target_name: Some("assets".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        dependency_type: Some(fdecl::DependencyType::Weak),
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("data".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string() }
                        )),
                        target_name: Some("data".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("data".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string() }
                        )),
                        target_name: Some("data".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("elf".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string() }
                        )),
                        target_name: Some("duplicated".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("elf".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string() }
                        )),
                        target_name: Some("duplicated".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("pkg".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string() }
                        )),
                        target_name: Some("duplicated".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("started".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "netstack".to_string(), collection: None})),
                        target_name: Some("started".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::EventStream(fdecl::OfferEventStream {
                        source_name: Some("started".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef{name: "netstack".to_string(), collection: None})),
                        target_name: Some("started".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("a".to_string()),
                        target: Some(fdecl::Ref::Collection(
                            fdecl::CollectionRef { name: "modular".to_string() }
                        )),
                        target_name: Some("dict".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("b".to_string()),
                        target: Some(fdecl::Ref::Collection(
                            fdecl::CollectionRef { name: "modular".to_string() }
                        )),
                        target_name: Some("dict".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]);
                decl.children = Some(vec![
                    fdecl::Child{
                        name: Some("netstack".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/netstack/stable#meta/netstack.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Eager),
                        on_terminate: None,
                        environment: None,
                        ..Default::default()
                    },
                ]);
                decl.collections = Some(vec![
                    fdecl::Collection{
                        name: Some("modular".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        environment: None,
                        allowed_offers: Some(fdecl::AllowedOffers::StaticOnly),
                        allow_long_names: None,
                        ..Default::default()
                    },
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                // Duplicate services are allowed, for aggregation.
                Error::duplicate_field(DeclType::OfferProtocol, "target_name", "fuchsia.logger.LegacyLog"),
                Error::duplicate_field(DeclType::OfferDirectory, "target_name", "assets"),
                Error::duplicate_field(DeclType::OfferStorage, "target_name", "data"),
                Error::duplicate_field(DeclType::OfferRunner, "target_name", "duplicated"),
                Error::duplicate_field(DeclType::OfferResolver, "target_name", "duplicated"),
                Error::duplicate_field(DeclType::OfferEventStream, "target_name", "started"),
                Error::duplicate_field(DeclType::OfferDictionary, "target_name", "dict"),
            ])),
        },
        test_validate_offers_target_invalid => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("logger".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("logger".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string(), }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("legacy_logger".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Weak),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("legacy_logger".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string(), }
                        )),
                        target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("assets".to_string()),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef {
                            name: "netstack".to_string(),
                            collection: None,
                        }
                        )),
                        target_name: Some("data".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("assets".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string(), }
                        )),
                        target_name: Some("data".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        dependency_type: Some(fdecl::DependencyType::Weak),
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source_name: Some("data".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("data".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source_name: Some("data".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target: Some(fdecl::Ref::Collection(
                            fdecl::CollectionRef { name: "modular".to_string(), }
                        )),
                        target_name: Some("data".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("elf".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("elf".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("elf".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string(), }
                        )),
                        target_name: Some("elf".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("pkg".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("pkg".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("pkg".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string(), }
                        )),
                        target_name: Some("pkg".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("pkg".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("pkg".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("pkg".to_string()),
                        target: Some(fdecl::Ref::Collection(
                        fdecl::CollectionRef { name: "modular".to_string(), }
                        )),
                        target_name: Some("pkg".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_child(DeclType::OfferService, "target", "netstack"),
                Error::invalid_collection(DeclType::OfferService, "target", "modular"),
                Error::invalid_child(DeclType::OfferProtocol, "target", "netstack"),
                Error::invalid_collection(DeclType::OfferProtocol, "target", "modular"),
                Error::invalid_child(DeclType::OfferDirectory, "target", "netstack"),
                Error::invalid_collection(DeclType::OfferDirectory, "target", "modular"),
                Error::invalid_child(DeclType::OfferStorage, "target", "netstack"),
                Error::invalid_collection(DeclType::OfferStorage, "target", "modular"),
                Error::invalid_child(DeclType::OfferRunner, "target", "netstack"),
                Error::invalid_collection(DeclType::OfferRunner, "target", "modular"),
                Error::invalid_child(DeclType::OfferResolver, "target", "netstack"),
                Error::invalid_collection(DeclType::OfferResolver, "target", "modular"),
                Error::invalid_child(DeclType::OfferDictionary, "target", "netstack"),
                Error::invalid_collection(DeclType::OfferDictionary, "target", "modular"),
            ])),
        },
        test_validate_offers_target_dictionary => {
            input = fdecl::Component {
                offers: Some(vec![
                    // Offer to static dictionary is ok
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("p".to_string()),
                        target: Some(fdecl::Ref::Capability(
                            fdecl::CapabilityRef {
                                name: "dict".into(),
                            },
                        )),
                        target_name: Some("p".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    // Offer to dynamic dictionary is forbidden
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        source_name: Some("p".to_string()),
                        target: Some(fdecl::Ref::Capability(
                            fdecl::CapabilityRef {
                                name: "dynamic".into(),
                            },
                        )),
                        target_name: Some("p".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Dictionary(fdecl::Dictionary {
                        name: Some("dict".into()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Dictionary(fdecl::Dictionary {
                        name: Some("dynamic".into()),
                        source_path: Some("/out/dir".into()),
                        ..Default::default()
                    }),
                ]),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::OfferProtocol, "target"),
            ])),
        },
        test_validate_offers_invalid_source_collection => {
            input = {
                let mut decl = new_component_decl();
                decl.collections = Some(vec![
                    fdecl::Collection {
                        name: Some("col".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        allowed_offers: Some(fdecl::AllowedOffers::StaticOnly),
                        allow_long_names: None,
                        ..Default::default()
                    }
                ]);
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("child".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        ..Default::default()
                    }
                ]);
                decl.offers = Some(vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "col".to_string() })),
                        source_name: Some("a".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                        target_name: Some("a".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "col".to_string() })),
                        source_name: Some("b".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                        target_name: Some("b".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        subdir: None,
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "col".to_string() })),
                        source_name: Some("c".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                        target_name: Some("c".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "col".to_string() })),
                        source_name: Some("d".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                        target_name: Some("d".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "col".to_string() })),
                        source_name: Some("e".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                        target_name: Some("e".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "col".to_string() })),
                        source_name: Some("f".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                        target_name: Some("f".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::OfferProtocol, "source"),
                Error::invalid_field(DeclType::OfferDirectory, "source"),
                Error::invalid_field(DeclType::OfferStorage, "source"),
                Error::invalid_field(DeclType::OfferRunner, "source"),
                Error::invalid_field(DeclType::OfferResolver, "source"),
                Error::invalid_field(DeclType::OfferDictionary, "source"),
            ])),
        },
        test_validate_offers_source_collection => {
            input = {
                let mut decl = new_component_decl();
                decl.collections = Some(vec![
                    fdecl::Collection {
                        name: Some("col".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        allowed_offers: Some(fdecl::AllowedOffers::StaticOnly),
                        allow_long_names: None,
                        ..Default::default()
                    }
                ]);
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("child".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        ..Default::default()
                    }
                ]);
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "col".to_string() })),
                        source_name: Some("a".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                        target_name: Some("a".to_string()),
                        ..Default::default()
                    })
                ]);
                decl
            },
            result = Ok(()),
        },
        test_validate_offers_invalid_capability_from_self => {
            input = {
                let mut decl = new_component_decl();
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("child".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/foo".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    }
                ]);
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("fuchsia.some.library.SomeProtocol".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child".into(),
                            collection: None
                        })),
                        target_name: Some("foo".into()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("fuchsia.some.library.SomeProtocol".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child".into(),
                            collection: None
                        })),
                        target_name: Some("bar".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("dir".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child".into(),
                            collection: None
                        })),
                        target_name: Some("assets".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_elf".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child".into(),
                            collection: None
                        })),
                        target_name: Some("elf".into()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_pkg".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child".into(),
                            collection: None
                        })),
                        target_name: Some("pkg".into()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_dict".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child".into(),
                            collection: None
                        })),
                        target_name: Some("dict".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_storage".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child".into(),
                            collection: None
                        })),
                        target_name: Some("storage".into()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Config(fdecl::OfferConfiguration {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("source_config".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child".into(),
                            collection: None
                        })),
                        target_name: Some("config".into()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        source_name: Some("fuchsia.some.library.SomeProtocol".into()),
                        source_dictionary: Some("dict/inner".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child".into(),
                            collection: None
                        })),
                        target_name: Some("baz".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_capability(
                    DeclType::OfferService,
                    "source",
                    "fuchsia.some.library.SomeProtocol"),
                Error::invalid_capability(
                    DeclType::OfferProtocol,
                    "source",
                    "fuchsia.some.library.SomeProtocol"),
                Error::invalid_capability(DeclType::OfferDirectory, "source", "dir"),
                Error::invalid_capability(DeclType::OfferRunner, "source", "source_elf"),
                Error::invalid_capability(DeclType::OfferResolver, "source", "source_pkg"),
                Error::invalid_capability(DeclType::OfferDictionary, "source", "source_dict"),
                Error::invalid_capability(DeclType::OfferStorage, "source", "source_storage"),
                Error::invalid_capability(DeclType::OfferConfig, "source", "source_config"),
                Error::invalid_capability(DeclType::OfferProtocol, "source", "dict"),
            ])),
        },
        test_validate_offers_long_dependency_cycle => {
            input = {
                let mut decl = new_component_decl();
                let dependencies = vec![
                    ("d", "b"),
                    ("a", "b"),
                    ("b", "c"),
                    ("b", "d"),
                    ("c", "a"),
                ];
                let offers = dependencies.into_iter().map(|(from,to)|
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(
                        fdecl::ChildRef { name: from.to_string(), collection: None },
                        )),
                        source_name: Some(format!("thing_{}", from)),
                        target: Some(fdecl::Ref::Child(
                        fdecl::ChildRef { name: to.to_string(), collection: None },
                        )),
                        target_name: Some(format!("thing_{}", from)),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    })).collect();
                let children = ["a", "b", "c", "d"].iter().map(|name| {
                    fdecl::Child {
                        name: Some(name.to_string()),
                        url: Some(format!("fuchsia-pkg://fuchsia.com/pkg#meta/{}.cm", name)),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        environment: None,
                        ..Default::default()
                    }
                }).collect();
                decl.offers = Some(offers);
                decl.children = Some(children);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle("{{child a -> child b -> child c -> child a}, {child b -> child d -> child b}}")
            ])),
        },
        test_validate_offers_not_required_invalid_source_service => {
            input = {
                let mut decl = generate_offer_different_source_and_availability_decl(
                    |source, availability, target_name|
                        fdecl::Offer::Service(fdecl::OfferService {
                            source: Some(source),
                            source_name: Some("fuchsia.examples.Echo".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "sink".to_string(),
                                collection: None,
                            })),
                            target_name: Some(target_name.into()),
                            availability: Some(availability),
                            ..Default::default()
                        })
                );
                decl.capabilities = Some(vec![
                    fdecl::Capability::Service(fdecl::Service {
                        name: Some("fuchsia.examples.Echo".to_string()),
                        source_path: Some("/svc/fuchsia.examples.Echo".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = {
                Err(ErrorList::new(vec![
                    Error::availability_must_be_optional(
                        DeclType::OfferService,
                        "availability",
                        Some(&"fuchsia.examples.Echo".to_string()),
                    ),
                    Error::availability_must_be_optional(
                        DeclType::OfferService,
                        "availability",
                        Some(&"fuchsia.examples.Echo".to_string()),
                    ),
                ]))
            },
        },
        test_validate_offers_not_required_invalid_source_protocol => {
            input = {
                let mut decl = generate_offer_different_source_and_availability_decl(
                    |source, availability, target_name|
                        fdecl::Offer::Protocol(fdecl::OfferProtocol {
                            source: Some(source),
                            source_name: Some("fuchsia.examples.Echo".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "sink".to_string(),
                                collection: None,
                            })),
                            target_name: Some(target_name.into()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(availability),
                            ..Default::default()
                        })
                );
                decl.capabilities = Some(vec![
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("fuchsia.examples.Echo".to_string()),
                        source_path: Some("/svc/fuchsia.examples.Echo".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = {
                Err(ErrorList::new(vec![
                    Error::availability_must_be_optional(
                        DeclType::OfferProtocol,
                        "availability",
                        Some(&"fuchsia.examples.Echo".to_string()),
                    ),
                    Error::availability_must_be_optional(
                        DeclType::OfferProtocol,
                        "availability",
                        Some(&"fuchsia.examples.Echo".to_string()),
                    ),
                ]))
            },
        },
        test_validate_offers_not_required_invalid_source_directory => {
            input = {
                let mut decl = generate_offer_different_source_and_availability_decl(
                    |source, availability, target_name|
                        fdecl::Offer::Directory(fdecl::OfferDirectory {
                            source: Some(source),
                            source_name: Some("assets".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "sink".to_string(),
                                collection: None,
                            })),
                            target_name: Some(target_name.into()),
                            rights: Some(fio::Operations::CONNECT),
                            subdir: None,
                            dependency_type: Some(fdecl::DependencyType::Weak),
                            availability: Some(availability),
                            ..Default::default()
                        })
                );
                decl.capabilities = Some(vec![
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: Some("assets".to_string()),
                        source_path: Some("/assets".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = {
                Err(ErrorList::new(vec![
                    Error::availability_must_be_optional(
                        DeclType::OfferDirectory,
                        "availability",
                        Some(&"assets".to_string()),
                    ),
                    Error::availability_must_be_optional(
                        DeclType::OfferDirectory,
                        "availability",
                        Some(&"assets".to_string()),
                    ),
                ]))
            },
        },
        test_validate_offers_not_required_invalid_source_storage => {
            input = {
                let mut decl = new_component_decl();
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("sink".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/sink#meta/sink.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        environment: None,
                        ..Default::default()
                    },
                ]);
                decl.capabilities = Some(vec![
                    fdecl::Capability::Storage(fdecl::Storage {
                        name: Some("data".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        backing_dir: Some("minfs".to_string()),
                        subdir: None,
                        storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                        ..Default::default()
                    }),
                ]);
                let new_offer = |source: fdecl::Ref, availability: fdecl::Availability,
                                        target_name: &str|
                {
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source: Some(source),
                        source_name: Some("data".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "sink".to_string(),
                            collection: None,
                        })),
                        target_name: Some(target_name.into()),
                        availability: Some(availability),
                        ..Default::default()
                    })
                };
                decl.offers = Some(vec![
                    // These offers are fine, offers with a source of parent or void can be
                    // optional.
                    new_offer(
                        fdecl::Ref::Parent(fdecl::ParentRef {}),
                        fdecl::Availability::Required,
                        "data0",
                    ),
                    new_offer(
                        fdecl::Ref::Parent(fdecl::ParentRef {}),
                        fdecl::Availability::Optional,
                        "data1",
                    ),
                    new_offer(
                        fdecl::Ref::Parent(fdecl::ParentRef {}),
                        fdecl::Availability::SameAsTarget,
                        "data2",
                    ),
                    new_offer(
                        fdecl::Ref::VoidType(fdecl::VoidRef {}),
                        fdecl::Availability::Optional,
                        "data3",
                    ),
                    // These offers are not fine, offers with a source other than parent or void
                    // must be required.
                    new_offer(
                        fdecl::Ref::Self_(fdecl::SelfRef {}),
                        fdecl::Availability::Optional,
                        "data4",
                    ),
                    new_offer(
                        fdecl::Ref::Self_(fdecl::SelfRef {}),
                        fdecl::Availability::SameAsTarget,
                        "data5",
                    ),
                    // These offers are also not fine, offers with a source of void must be optional
                    new_offer(
                        fdecl::Ref::VoidType(fdecl::VoidRef {}),
                        fdecl::Availability::Required,
                        "data6",
                    ),
                    new_offer(
                        fdecl::Ref::VoidType(fdecl::VoidRef {}),
                        fdecl::Availability::SameAsTarget,
                        "data7",
                    ),
                ]);
                decl
            },
            result = {
                Err(ErrorList::new(vec![
                    Error::availability_must_be_optional(
                        DeclType::OfferStorage,
                        "availability",
                        Some(&"data".to_string()),
                    ),
                    Error::availability_must_be_optional(
                        DeclType::OfferStorage,
                        "availability",
                        Some(&"data".to_string()),
                    ),
                ]))
            },
        },

        test_validate_offers_valid_service_aggregation => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll_a".to_string()
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "child_c".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        source_instance_filter: None,
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll_b".to_string()
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "child_c".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        source_instance_filter: Some(vec!["a_different_default".to_string()]),
                        ..Default::default()
                    })
                ]);
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("child_c".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]);
                decl.collections = Some(vec![
                    fdecl::Collection {
                        name: Some("coll_a".into()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                    fdecl::Collection {
                        name: Some("coll_b".into()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                ]);
                decl
            },
            result = Ok(()),
        },

        // dictionaries
        test_validate_source_dictionary => {
            input = fdecl::Component {
                program: Some(fdecl::Program {
                    runner: Some("elf".into()),
                    info: Some(fdata::Dictionary {
                        entries: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                uses: Some(vec![
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_dictionary: Some("bad//".into()),
                        source_name: Some("foo".into()),
                        target_path: Some("/svc/foo".into()),
                        ..Default::default()
                    }),
                ]),
                exposes: Some(vec![
                    fdecl::Expose::Directory(fdecl::ExposeDirectory {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "missing".into(),
                            collection: None,
                        })),
                        source_dictionary: Some("in/dict".into()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("foo".into()),
                        target_name: Some("bar".into()),
                        ..Default::default()
                    }),
                ]),
                offers: Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_dictionary: Some("bad//".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child".into(),
                            collection: None,
                        })),
                        source_name: Some("foo".into()),
                        target_name: Some("bar".into()),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("child".into()),
                        url: Some("fuchsia-pkg://child".into()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::UseProtocol, "source_dictionary"),
                Error::invalid_child(DeclType::ExposeDirectory, "source", "missing"),
                Error::invalid_field(DeclType::OfferService, "source_dictionary"),
            ])),
        },
        test_validate_dictionary_too_long => {
            input = fdecl::Component {
                program: Some(fdecl::Program {
                    runner: Some("elf".into()),
                    info: Some(fdata::Dictionary {
                        entries: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                uses: Some(vec![
                    fdecl::Use::Protocol(fdecl::UseProtocol {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent( fdecl::ParentRef {} )),
                        source_dictionary: Some("a".repeat(4096)),
                        source_name: Some("foo".into()),
                        target_path: Some("/svc/foo".into()),
                        ..Default::default()
                    }),
                ]),
                ..Default::default()
            },
            result = Err(ErrorList::new(vec![
                Error::field_too_long(DeclType::UseProtocol, "source_dictionary"),
            ])),
        },

        // environments
        test_validate_environment_empty => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: None,
                    extends: None,
                    runners: None,
                    resolvers: None,
                    stop_timeout_ms: None,
                    debug_capabilities: None,
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::Environment, "name"),
                Error::missing_field(DeclType::Environment, "extends"),
            ])),
        },

        test_validate_environment_no_stop_timeout => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: Some("env".to_string()),
                    extends: Some(fdecl::EnvironmentExtends::None),
                    runners: None,
                    resolvers: None,
                    stop_timeout_ms: None,
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![Error::missing_field(DeclType::Environment, "stop_timeout_ms")])),
        },

        test_validate_environment_extends_stop_timeout => {
            input = {  let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: Some("env".to_string()),
                    extends: Some(fdecl::EnvironmentExtends::Realm),
                    runners: None,
                    resolvers: None,
                    stop_timeout_ms: None,
                    ..Default::default()
                }]);
                decl
            },
            result = Ok(()),
        },
        test_validate_environment_long_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: Some("a".repeat(256)),
                    extends: Some(fdecl::EnvironmentExtends::None),
                    runners: Some(vec![
                        fdecl::RunnerRegistration {
                            source_name: Some("a".repeat(256)),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                            target_name: Some("a".repeat(256)),
                            ..Default::default()
                        },
                    ]),
                    resolvers: Some(vec![
                        fdecl::ResolverRegistration {
                            resolver: Some("a".repeat(256)),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                            scheme: Some("a".repeat(256)),
                            ..Default::default()
                        },
                    ]),
                    stop_timeout_ms: Some(1234),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::field_too_long(DeclType::Environment, "name"),
                Error::field_too_long(DeclType::RunnerRegistration, "source_name"),
                Error::field_too_long(DeclType::RunnerRegistration, "target_name"),
                Error::field_too_long(DeclType::ResolverRegistration, "resolver"),
                Error::field_too_long(DeclType::ResolverRegistration, "scheme"),
            ])),
        },
        test_validate_environment_empty_runner_resolver_fields => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: Some("a".to_string()),
                    extends: Some(fdecl::EnvironmentExtends::None),
                    runners: Some(vec![
                        fdecl::RunnerRegistration {
                            source_name: None,
                            source: None,
                            target_name: None,
                            ..Default::default()
                        },
                    ]),
                    resolvers: Some(vec![
                        fdecl::ResolverRegistration {
                            resolver: None,
                            source: None,
                            scheme: None,
                            ..Default::default()
                        },
                    ]),
                    stop_timeout_ms: Some(1234),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::RunnerRegistration, "source_name"),
                Error::missing_field(DeclType::RunnerRegistration, "source"),
                Error::missing_field(DeclType::RunnerRegistration, "target_name"),
                Error::missing_field(DeclType::ResolverRegistration, "resolver"),
                Error::missing_field(DeclType::ResolverRegistration, "source"),
                Error::missing_field(DeclType::ResolverRegistration, "scheme"),
            ])),
        },
        test_validate_environment_invalid_fields => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: Some("a".to_string()),
                    extends: Some(fdecl::EnvironmentExtends::None),
                    runners: Some(vec![
                        fdecl::RunnerRegistration {
                            source_name: Some("^a".to_string()),
                            source: Some(fdecl::Ref::Framework(fdecl::FrameworkRef{})),
                            target_name: Some("%a".to_string()),
                            ..Default::default()
                        },
                    ]),
                    resolvers: Some(vec![
                        fdecl::ResolverRegistration {
                            resolver: Some("^a".to_string()),
                            source: Some(fdecl::Ref::Framework(fdecl::FrameworkRef{})),
                            scheme: Some("9scheme".to_string()),
                            ..Default::default()
                        },
                    ]),
                    stop_timeout_ms: Some(1234),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::RunnerRegistration, "source_name"),
                Error::invalid_field(DeclType::RunnerRegistration, "source"),
                Error::invalid_field(DeclType::RunnerRegistration, "target_name"),
                Error::invalid_field(DeclType::ResolverRegistration, "resolver"),
                Error::invalid_field(DeclType::ResolverRegistration, "source"),
                Error::invalid_field(DeclType::ResolverRegistration, "scheme"),
            ])),
        },
        test_validate_environment_missing_runner => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: Some("a".to_string()),
                    extends: Some(fdecl::EnvironmentExtends::None),
                    runners: Some(vec![
                        fdecl::RunnerRegistration {
                            source_name: Some("dart".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                            target_name: Some("dart".to_string()),
                            ..Default::default()
                        },
                    ]),
                    resolvers: None,
                    stop_timeout_ms: Some(1234),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_runner(DeclType::RunnerRegistration, "source_name", "dart"),
            ])),
        },
        test_validate_environment_duplicate_registrations => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: Some("a".to_string()),
                    extends: Some(fdecl::EnvironmentExtends::None),
                    runners: Some(vec![
                        fdecl::RunnerRegistration {
                            source_name: Some("dart".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                            target_name: Some("dart".to_string()),
                            ..Default::default()
                        },
                        fdecl::RunnerRegistration {
                            source_name: Some("other-dart".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                            target_name: Some("dart".to_string()),
                            ..Default::default()
                        },
                    ]),
                    resolvers: Some(vec![
                        fdecl::ResolverRegistration {
                            resolver: Some("pkg_resolver".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                            scheme: Some("fuchsia-pkg".to_string()),
                            ..Default::default()
                        },
                        fdecl::ResolverRegistration {
                            resolver: Some("base_resolver".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                            scheme: Some("fuchsia-pkg".to_string()),
                            ..Default::default()
                        },
                    ]),
                    stop_timeout_ms: Some(1234),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::duplicate_field(DeclType::RunnerRegistration, "target_name", "dart"),
                Error::duplicate_field(DeclType::ResolverRegistration, "scheme", "fuchsia-pkg"),
            ])),
        },
        test_validate_environment_from_missing_child => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: Some("a".to_string()),
                    extends: Some(fdecl::EnvironmentExtends::None),
                    runners: Some(vec![
                        fdecl::RunnerRegistration {
                            source_name: Some("elf".to_string()),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{
                                name: "missing".to_string(),
                                collection: None,
                            })),
                            target_name: Some("elf".to_string()),
                            ..Default::default()
                        },
                    ]),
                    resolvers: Some(vec![
                        fdecl::ResolverRegistration {
                            resolver: Some("pkg_resolver".to_string()),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{
                                name: "missing".to_string(),
                                collection: None,
                            })),
                            scheme: Some("fuchsia-pkg".to_string()),
                            ..Default::default()
                        },
                    ]),
                    stop_timeout_ms: Some(1234),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_child(DeclType::RunnerRegistration, "source", "missing"),
                Error::invalid_child(DeclType::ResolverRegistration, "source", "missing"),
            ])),
        },
        test_validate_environment_runner_child_cycle => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: Some("env".to_string()),
                    extends: Some(fdecl::EnvironmentExtends::None),
                    runners: Some(vec![
                        fdecl::RunnerRegistration {
                            source_name: Some("elf".to_string()),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{
                                name: "child".to_string(),
                                collection: None,
                            })),
                            target_name: Some("elf".to_string()),
                            ..Default::default()
                        },
                    ]),
                    resolvers: None,
                    stop_timeout_ms: Some(1234),
                    ..Default::default()
                }]);
                decl.children = Some(vec![fdecl::Child {
                    name: Some("child".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    url: Some("fuchsia-pkg://child".to_string()),
                    environment: Some("env".to_string()),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle(
                    "{{child child -> environment env -> child child}}"
                ),
            ])),
        },
        test_validate_environment_resolver_child_cycle => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: Some("env".to_string()),
                    extends: Some(fdecl::EnvironmentExtends::None),
                    runners: None,
                    resolvers: Some(vec![
                        fdecl::ResolverRegistration {
                            resolver: Some("pkg_resolver".to_string()),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{
                                name: "child".to_string(),
                                collection: None,
                            })),
                            scheme: Some("fuchsia-pkg".to_string()),
                            ..Default::default()
                        },
                    ]),
                    stop_timeout_ms: Some(1234),
                    ..Default::default()
                }]);
                decl.children = Some(vec![fdecl::Child {
                    name: Some("child".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    url: Some("fuchsia-pkg://child".to_string()),
                    environment: Some("env".to_string()),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle(
                    "{{child child -> environment env -> child child}}"
                ),
            ])),
        },
        test_validate_environment_resolver_multiple_children_cycle => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![fdecl::Environment {
                    name: Some("env".to_string()),
                    extends: Some(fdecl::EnvironmentExtends::None),
                    runners: None,
                    resolvers: Some(vec![
                        fdecl::ResolverRegistration {
                            resolver: Some("pkg_resolver".to_string()),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef{
                                name: "a".to_string(),
                                collection: None,
                            })),
                            scheme: Some("fuchsia-pkg".to_string()),
                            ..Default::default()
                        },
                    ]),
                    stop_timeout_ms: Some(1234),
                    ..Default::default()
                }]);
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("a".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        url: Some("fuchsia-pkg://child-a".to_string()),
                        environment: None,
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("b".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        url: Some("fuchsia-pkg://child-b".to_string()),
                        environment: Some("env".to_string()),
                        ..Default::default()
                    },
                ]);
                decl.offers = Some(vec![fdecl::Offer::Service(fdecl::OfferService {
                    source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "b".to_string(),
                        collection: None,
                    })),
                    source_name: Some("thing".to_string()),
                    target: Some(fdecl::Ref::Child(
                    fdecl::ChildRef {
                        name: "a".to_string(),
                        collection: None,
                    }
                    )),
                    target_name: Some("thing".to_string()),
                    ..Default::default()
                })]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::dependency_cycle(
                    "{{child a -> environment env -> child b -> child a}}"
                ),
            ])),
        },
        test_validate_environment_debug_empty => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![
                    fdecl::Environment {
                        name: Some("a".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::None),
                        stop_timeout_ms: Some(2),
                        debug_capabilities:Some(vec![
                            fdecl::DebugRegistration::Protocol(fdecl::DebugProtocolRegistration {
                                source: None,
                                source_name: None,
                                target_name: None,
                                ..Default::default()
                            }),
                    ]),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::DebugProtocolRegistration, "source"),
                Error::missing_field(DeclType::DebugProtocolRegistration, "source_name"),
                Error::missing_field(DeclType::DebugProtocolRegistration, "target_name"),
            ])),
        },
        test_validate_environment_debug_log_identifier => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![
                    fdecl::Environment {
                        name: Some("a".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::None),
                        stop_timeout_ms: Some(2),
                        debug_capabilities:Some(vec![
                            fdecl::DebugRegistration::Protocol(fdecl::DebugProtocolRegistration {
                                source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                    name: "a".repeat(256),
                                    collection: None,
                                })),
                                source_name: Some(format!("{}", "a".repeat(256))),
                                target_name: Some(format!("{}", "b".repeat(256))),
                                ..Default::default()
                            }),
                            fdecl::DebugRegistration::Protocol(fdecl::DebugProtocolRegistration {
                                source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                                source_name: Some("a".to_string()),
                                target_name: Some(format!("{}", "b".repeat(256))),
                                ..Default::default()
                            }),
                    ]),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::field_too_long(DeclType::DebugProtocolRegistration, "source.child.name"),
                Error::field_too_long(DeclType::DebugProtocolRegistration, "source_name"),
                Error::field_too_long(DeclType::DebugProtocolRegistration, "target_name"),
                Error::field_too_long(DeclType::DebugProtocolRegistration, "target_name"),
            ])),
        },
        test_validate_environment_debug_log_extraneous => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![
                    fdecl::Environment {
                        name: Some("a".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::None),
                        stop_timeout_ms: Some(2),
                        debug_capabilities:Some(vec![
                            fdecl::DebugRegistration::Protocol(fdecl::DebugProtocolRegistration {
                                source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                    name: "logger".to_string(),
                                    collection: Some("modular".to_string()),
                                })),
                                source_name: Some("fuchsia.logger.Log".to_string()),
                                target_name: Some("fuchsia.logger.Log".to_string()),
                                ..Default::default()
                            }),
                    ]),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::extraneous_field(DeclType::DebugProtocolRegistration, "source.child.collection"),
            ])),
        },
        test_validate_environment_debug_log_invalid_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![
                    fdecl::Environment {
                        name: Some("a".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::None),
                        stop_timeout_ms: Some(2),
                        debug_capabilities:Some(vec![
                            fdecl::DebugRegistration::Protocol(fdecl::DebugProtocolRegistration {
                                source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                    name: "^bad".to_string(),
                                    collection: None,
                                })),
                                source_name: Some("foo/".to_string()),
                                target_name: Some("/".to_string()),
                                ..Default::default()
                            }),
                    ]),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::DebugProtocolRegistration, "source.child.name"),
                Error::invalid_field(DeclType::DebugProtocolRegistration, "source_name"),
                Error::invalid_field(DeclType::DebugProtocolRegistration, "target_name"),
            ])),
        },
        test_validate_environment_debug_log_invalid_child => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![
                    fdecl::Environment {
                        name: Some("a".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::None),
                        stop_timeout_ms: Some(2),
                        debug_capabilities:Some(vec![
                            fdecl::DebugRegistration::Protocol(fdecl::DebugProtocolRegistration {
                                source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                    name: "logger".to_string(),
                                    collection: None,
                                })),
                                source_name: Some("fuchsia.logger.LegacyLog".to_string()),
                                target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                                ..Default::default()
                            }),
                    ]),
                    ..Default::default()
                }]);
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("netstack".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/netstack/stable#meta/netstack.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        environment: None,
                        ..Default::default()
                    },
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_child(DeclType::DebugProtocolRegistration, "source", "logger"),

            ])),
        },
        test_validate_environment_debug_source_capability => {
            input = {
                let mut decl = new_component_decl();
                decl.environments = Some(vec![
                    fdecl::Environment {
                        name: Some("a".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::None),
                        stop_timeout_ms: Some(2),
                        debug_capabilities:Some(vec![
                            fdecl::DebugRegistration::Protocol(fdecl::DebugProtocolRegistration {
                                source: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                                    name: "storage".to_string(),
                                })),
                                source_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                                target_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                                ..Default::default()
                            }),
                    ]),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::DebugProtocolRegistration, "source"),
            ])),
        },

        // children
        test_validate_children_empty => {
            input = {
                let mut decl = new_component_decl();
                decl.children = Some(vec![fdecl::Child{
                    name: None,
                    url: None,
                    startup: None,
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::Child, "name"),
                Error::missing_field(DeclType::Child, "url"),
                Error::missing_field(DeclType::Child, "startup"),
                // `on_terminate` is allowed to be None
            ])),
        },
        test_validate_children_invalid_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.children = Some(vec![fdecl::Child{
                    name: Some("^bad".to_string()),
                    url: Some("scheme://invalid-port:99999999/path#frag".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::Child, "name"),
                Error::invalid_url(DeclType::Child, "url", "\"scheme://invalid-port:99999999/path#frag\": Malformed URL: InvalidPort."),
            ])),
        },
        test_validate_children_long_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.children = Some(vec![fdecl::Child{
                    name: Some("a".repeat(1025)),
                    url: Some(format!("fuchsia-pkg://{}", "a".repeat(4083))),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: Some("a".repeat(1025)),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::field_too_long(DeclType::Child, "name"),
                Error::field_too_long(DeclType::Child, "url"),
                Error::field_too_long(DeclType::Child, "environment"),
                Error::invalid_environment(DeclType::Child, "environment", "a".repeat(1025)),
            ])),
        },
        test_validate_child_references_unknown_env => {
            input = {
                let mut decl = new_component_decl();
                decl.children = Some(vec![fdecl::Child{
                    name: Some("foo".to_string()),
                    url: Some("fuchsia-pkg://foo".to_string()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    on_terminate: None,
                    environment: Some("test_env".to_string()),
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_environment(DeclType::Child, "environment", "test_env"),
            ])),
        },

        // collections
        test_validate_collections_empty => {
            input = {
                let mut decl = new_component_decl();
                decl.collections = Some(vec![fdecl::Collection{
                    name: None,
                    durability: None,
                    environment: None,
                    allowed_offers: None,
                    allow_long_names: None,
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::Collection, "name"),
                Error::missing_field(DeclType::Collection, "durability"),
            ])),
        },
        test_validate_collections_invalid_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.collections = Some(vec![fdecl::Collection{
                    name: Some("^bad".to_string()),
                    durability: Some(fdecl::Durability::Transient),
                    environment: None,
                    allowed_offers: Some(fdecl::AllowedOffers::StaticOnly),
                    allow_long_names: None,
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::Collection, "name"),
            ])),
        },
        test_validate_collections_long_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.collections = Some(vec![fdecl::Collection{
                    name: Some("a".repeat(1025)),
                    durability: Some(fdecl::Durability::Transient),
                    environment: None,
                    allowed_offers: Some(fdecl::AllowedOffers::StaticOnly),
                    allow_long_names: None,
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::field_too_long(DeclType::Collection, "name"),
            ])),
        },
        test_validate_collection_references_unknown_env => {
            input = {
                let mut decl = new_component_decl();
                decl.collections = Some(vec![fdecl::Collection {
                    name: Some("foo".to_string()),
                    durability: Some(fdecl::Durability::Transient),
                    environment: Some("test_env".to_string()),
                    allowed_offers: Some(fdecl::AllowedOffers::StaticOnly),
                    allow_long_names: None,
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_environment(DeclType::Collection, "environment", "test_env"),
            ])),
        },

        // capabilities
        test_validate_capabilities_empty => {
            input = {
                let mut decl = new_component_decl();
                decl.capabilities = Some(vec![
                    fdecl::Capability::Service(fdecl::Service {
                        name: None,
                        source_path: None,
                        ..Default::default()
                    }),
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: None,
                        source_path: None,
                        ..Default::default()
                    }),
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: None,
                        source_path: None,
                        rights: None,
                        ..Default::default()
                    }),
                    fdecl::Capability::Storage(fdecl::Storage {
                        name: None,
                        source: None,
                        backing_dir: None,
                        subdir: None,
                        storage_id: None,
                        ..Default::default()
                    }),
                    fdecl::Capability::Runner(fdecl::Runner {
                        name: None,
                        source_path: None,
                        ..Default::default()
                    }),
                    fdecl::Capability::Resolver(fdecl::Resolver {
                        name: None,
                        source_path: None,
                        ..Default::default()
                    }),
                    fdecl::Capability::Dictionary(fdecl::Dictionary {
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::Dictionary, "name"),
                Error::missing_field(DeclType::Service, "name"),
                Error::missing_field(DeclType::Service, "source_path"),
                Error::missing_field(DeclType::Protocol, "name"),
                Error::missing_field(DeclType::Protocol, "source_path"),
                Error::missing_field(DeclType::Directory, "name"),
                Error::missing_field(DeclType::Directory, "source_path"),
                Error::missing_field(DeclType::Directory, "rights"),
                Error::missing_field(DeclType::Storage, "source"),
                Error::missing_field(DeclType::Storage, "name"),
                Error::missing_field(DeclType::Storage, "storage_id"),
                Error::missing_field(DeclType::Storage, "backing_dir"),
                Error::missing_field(DeclType::Runner, "name"),
                Error::missing_field(DeclType::Runner, "source_path"),
                Error::missing_field(DeclType::Resolver, "name"),
                Error::missing_field(DeclType::Resolver, "source_path"),
            ])),
        },
        test_validate_capabilities_invalid_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.capabilities = Some(vec![
                    fdecl::Capability::Service(fdecl::Service {
                        name: Some("^bad".to_string()),
                        source_path: Some("&bad".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("^bad".to_string()),
                        source_path: Some("&bad".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: Some("^bad".to_string()),
                        source_path: Some("&bad".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        ..Default::default()
                    }),
                    fdecl::Capability::Storage(fdecl::Storage {
                        name: Some("^bad".to_string()),
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "/bad".to_string()
                        })),
                        backing_dir: Some("&bad".to_string()),
                        subdir: None,
                        storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                        ..Default::default()
                    }),
                    fdecl::Capability::Runner(fdecl::Runner {
                        name: Some("^bad".to_string()),
                        source_path: Some("&bad".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Resolver(fdecl::Resolver {
                        name: Some("^bad".to_string()),
                        source_path: Some("&bad".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Dictionary(fdecl::Dictionary {
                        name: Some("^bad".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::Dictionary, "name"),
                Error::invalid_field(DeclType::Service, "name"),
                Error::invalid_field(DeclType::Service, "source_path"),
                Error::invalid_field(DeclType::Protocol, "name"),
                Error::invalid_field(DeclType::Protocol, "source_path"),
                Error::invalid_field(DeclType::Directory, "name"),
                Error::invalid_field(DeclType::Directory, "source_path"),
                Error::invalid_field(DeclType::Storage, "source"),
                Error::invalid_field(DeclType::Storage, "name"),
                Error::invalid_field(DeclType::Storage, "backing_dir"),
                Error::invalid_field(DeclType::Runner, "name"),
                Error::invalid_field(DeclType::Runner, "source_path"),
                Error::invalid_field(DeclType::Resolver, "name"),
                Error::invalid_field(DeclType::Resolver, "source_path"),
            ])),
        },
        test_validate_capabilities_invalid_child => {
            input = {
                let mut decl = new_component_decl();
                decl.capabilities = Some(vec![
                    fdecl::Capability::Storage(fdecl::Storage {
                        name: Some("foo".to_string()),
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "invalid".to_string(),
                        })),
                        backing_dir: Some("foo".to_string()),
                        subdir: None,
                        storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::Storage, "source"),
            ])),
        },
        test_validate_capabilities_long_identifiers => {
            input = {
                let mut decl = new_component_decl();
                decl.capabilities = Some(vec![
                    fdecl::Capability::Service(fdecl::Service {
                        name: Some("a".repeat(256)),
                        source_path: Some("/c".repeat(2048)),
                        ..Default::default()
                    }),
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("a".repeat(256)),
                        source_path: Some("/c".repeat(2048)),
                        ..Default::default()
                    }),
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: Some("a".repeat(256)),
                        source_path: Some("/c".repeat(2048)),
                        rights: Some(fio::Operations::CONNECT),
                        ..Default::default()
                    }),
                    fdecl::Capability::Storage(fdecl::Storage {
                        name: Some("a".repeat(256)),
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "b".repeat(256),
                            collection: None,
                        })),
                        backing_dir: Some(format!("{}", "c".repeat(256))),
                        subdir: None,
                        storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                        ..Default::default()
                    }),
                    fdecl::Capability::Runner(fdecl::Runner {
                        name: Some("a".repeat(256)),
                        source_path: Some("/c".repeat(2048)),
                        ..Default::default()
                    }),
                    fdecl::Capability::Resolver(fdecl::Resolver {
                        name: Some("a".repeat(256)),
                        source_path: Some("/c".repeat(2048)),
                        ..Default::default()
                    }),
                    fdecl::Capability::Dictionary(fdecl::Dictionary {
                        name: Some("a".repeat(256)),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::field_too_long(DeclType::Dictionary, "name"),
                Error::field_too_long(DeclType::Service, "name"),
                Error::field_too_long(DeclType::Service, "source_path"),
                Error::field_too_long(DeclType::Protocol, "name"),
                Error::field_too_long(DeclType::Protocol, "source_path"),
                Error::field_too_long(DeclType::Directory, "name"),
                Error::field_too_long(DeclType::Directory, "source_path"),
                Error::field_too_long(DeclType::Storage, "source.child.name"),
                Error::field_too_long(DeclType::Storage, "name"),
                Error::field_too_long(DeclType::Storage, "backing_dir"),
                Error::field_too_long(DeclType::Runner, "name"),
                Error::field_too_long(DeclType::Runner, "source_path"),
                Error::field_too_long(DeclType::Resolver, "name"),
                Error::field_too_long(DeclType::Resolver, "source_path"),
            ])),
        },
        test_validate_capabilities_duplicate_name => {
            input = {
                let mut decl = new_component_decl();
                decl.capabilities = Some(vec![
                    fdecl::Capability::Service(fdecl::Service {
                        name: Some("service".to_string()),
                        source_path: Some("/service".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Service(fdecl::Service {
                        name: Some("service".to_string()),
                        source_path: Some("/service".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("protocol".to_string()),
                        source_path: Some("/protocol".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("protocol".to_string()),
                        source_path: Some("/protocol".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: Some("directory".to_string()),
                        source_path: Some("/directory".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        ..Default::default()
                    }),
                    fdecl::Capability::Directory(fdecl::Directory {
                        name: Some("directory".to_string()),
                        source_path: Some("/directory".to_string()),
                        rights: Some(fio::Operations::CONNECT),
                        ..Default::default()
                    }),
                    fdecl::Capability::Storage(fdecl::Storage {
                        name: Some("storage".to_string()),
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        backing_dir: Some("directory".to_string()),
                        subdir: None,
                        storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                        ..Default::default()
                    }),
                    fdecl::Capability::Storage(fdecl::Storage {
                        name: Some("storage".to_string()),
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef{})),
                        backing_dir: Some("directory".to_string()),
                        subdir: None,
                        storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                        ..Default::default()
                    }),
                    fdecl::Capability::Runner(fdecl::Runner {
                        name: Some("runner".to_string()),
                        source_path: Some("/runner".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Runner(fdecl::Runner {
                        name: Some("runner".to_string()),
                        source_path: Some("/runner".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Resolver(fdecl::Resolver {
                        name: Some("resolver".to_string()),
                        source_path: Some("/resolver".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Resolver(fdecl::Resolver {
                        name: Some("resolver".to_string()),
                        source_path: Some("/resolver".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Dictionary(fdecl::Dictionary {
                        name: Some("dictionary".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Dictionary(fdecl::Dictionary {
                        name: Some("dictionary".to_string()),
                        ..Default::default()
                    }),
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::duplicate_field(DeclType::Dictionary, "name", "dictionary"),
                Error::duplicate_field(DeclType::Service, "name", "service"),
                Error::duplicate_field(DeclType::Protocol, "name", "protocol"),
                Error::duplicate_field(DeclType::Directory, "name", "directory"),
                Error::duplicate_field(DeclType::Storage, "name", "storage"),
                Error::duplicate_field(DeclType::Runner, "name", "runner"),
                Error::duplicate_field(DeclType::Resolver, "name", "resolver"),
            ])),
        },
        test_validate_invalid_service_aggregation_conflicting_filter => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll_a".to_string()
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "child_c".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log1".to_string()),
                        source_instance_filter: Some(vec!["default".to_string()]),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll_b".to_string()
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "child_c".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log1".to_string()),
                        source_instance_filter: Some(vec!["default".to_string()]),
                        ..Default::default()
                    }),
                ]);
                decl.collections = Some(vec![
                    fdecl::Collection {
                        name: Some("coll_a".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                    fdecl::Collection {
                        name: Some("coll_b".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                ]);
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("child_c".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_aggregate_offer("Conflicting source_instance_filter in aggregate \
                   service offer, instance_name 'default' seen in filter lists multiple times"),
            ])),
        },

        test_validate_invalid_service_aggregation_conflicting_source_name => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll_a".into()
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "child_c".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log2".to_string()),
                        source_instance_filter: Some(vec!["default2".to_string()]),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll_b".into()
                        })),
                        source_name: Some("fuchsia.logger.LogAlt".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "child_c".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log2".to_string()),
                        source_instance_filter: Some(vec!["default".to_string()]),
                        ..Default::default()
                    })
                ]);
                decl.collections = Some(vec![
                    fdecl::Collection {
                        name: Some("coll_a".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                    fdecl::Collection {
                        name: Some("coll_b".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                ]);
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("child_c".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        environment: None,
                        ..Default::default()
                    },
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_aggregate_offer("All aggregate service offers must have the same source_name, saw fuchsia.logger.Log, fuchsia.logger.LogAlt. Use renamed_instances to rename instance names to avoid conflict."),
            ])),
        },

        test_validate_resolvers_missing_from_offer => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![fdecl::Offer::Resolver(fdecl::OfferResolver {
                    source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                    source_name: Some("a".to_string()),
                    target: Some(fdecl::Ref::Child(fdecl::ChildRef { name: "child".to_string(), collection: None })),
                    target_name: Some("a".to_string()),
                    ..Default::default()
                })]);
                decl.children = Some(vec![fdecl::Child {
                    name: Some("child".to_string()),
                    url: Some("test:///child".to_string()),
                    startup: Some(fdecl::StartupMode::Eager),
                    on_terminate: None,
                    environment: None,
                    ..Default::default()
                }]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_capability(DeclType::OfferResolver, "source", "a"),
            ])),
        },
        test_validate_resolvers_missing_from_expose => {
            input = {
                let mut decl = new_component_decl();
                decl.exposes = Some(vec![fdecl::Expose::Resolver(fdecl::ExposeResolver {
                    source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                    source_name: Some("a".to_string()),
                    target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                    target_name: Some("a".to_string()),
                    ..Default::default()
                })]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::invalid_capability(DeclType::ExposeResolver, "source", "a"),
            ])),
        },

        test_validate_config_missing_config => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: None,
                    checksum: None,
                    value_source: None,
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::ConfigSchema, "fields"),
                Error::missing_field(DeclType::ConfigSchema, "checksum"),
                Error::missing_field(DeclType::ConfigSchema, "value_source"),
            ])),
        },

        test_validate_config_missing_config_field => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: None,
                            type_: None,
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::ConfigField, "key"),
                Error::missing_field(DeclType::ConfigField, "value_type"),
            ])),
        },

        test_validate_config_bool => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Bool,
                                parameters: Some(vec![]),
                                constraints: vec![]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Ok(()),
        },

        test_validate_config_bool_extra_constraint => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Bool,
                                parameters: Some(vec![]),
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(10)]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::extraneous_field(DeclType::ConfigType, "constraints")
            ])),
        },

        test_validate_config_bool_missing_parameters => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Bool,
                                parameters: None,
                                constraints: vec![]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::ConfigType, "parameters")
            ])),
        },

        test_validate_config_string => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::String,
                                parameters: Some(vec![]),
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(10)]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Ok(()),
        },

        test_validate_config_string_missing_parameter => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::String,
                                parameters: None,
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(10)]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::ConfigType, "parameters")
            ])),
        },

        test_validate_config_string_missing_constraint => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::String,
                                parameters: Some(vec![]),
                                constraints: vec![]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::ConfigType, "constraints")
            ])),
        },

        test_validate_config_string_extra_constraint => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::String,
                                parameters: Some(vec![]),
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(10), fdecl::LayoutConstraint::MaxSize(10)]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::extraneous_field(DeclType::ConfigType, "constraints")
            ])),
        },

        test_validate_config_vector_bool => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Vector,
                                parameters: Some(vec![fdecl::LayoutParameter::NestedType(fdecl::ConfigType {
                                    layout: fdecl::ConfigTypeLayout::Bool,
                                    parameters: Some(vec![]),
                                    constraints: vec![],
                                })]),
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(10)]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Ok(()),
        },

        test_validate_config_vector_extra_parameter => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Vector,
                                parameters: Some(vec![fdecl::LayoutParameter::NestedType(fdecl::ConfigType {
                                    layout: fdecl::ConfigTypeLayout::Bool,
                                    parameters: Some(vec![]),
                                    constraints: vec![],
                                }), fdecl::LayoutParameter::NestedType(fdecl::ConfigType {
                                    layout: fdecl::ConfigTypeLayout::Uint8,
                                    parameters: Some(vec![]),
                                    constraints: vec![],
                                })]),
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(10)]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::extraneous_field(DeclType::ConfigType, "parameters")
            ])),
        },

        test_validate_config_vector_missing_parameter => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Vector,
                                parameters: Some(vec![]),
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(10)]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::ConfigType, "parameters")
            ])),
        },

        test_validate_config_vector_string => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Vector,
                                parameters: Some(vec![fdecl::LayoutParameter::NestedType(fdecl::ConfigType {
                                    layout: fdecl::ConfigTypeLayout::String,
                                    parameters: Some(vec![]),
                                    constraints: vec![fdecl::LayoutConstraint::MaxSize(10)]
                                })]),
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(10)]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Ok(()),
        },

        test_validate_config_vector_vector => {
            input = {
                let mut decl = new_component_decl();
                decl.config = Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Vector,
                                parameters: Some(vec![fdecl::LayoutParameter::NestedType(fdecl::ConfigType {
                                    layout: fdecl::ConfigTypeLayout::Vector,
                                    parameters: Some(vec![fdecl::LayoutParameter::NestedType(fdecl::ConfigType {
                                        layout: fdecl::ConfigTypeLayout::String,
                                        parameters: Some(vec![]),
                                        constraints: vec![fdecl::LayoutConstraint::MaxSize(10)]
                                    })]),
                                    constraints: vec![fdecl::LayoutConstraint::MaxSize(10)]
                                })]),
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(10)]
                            }),
                            ..Default::default()
                        }
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("config/test.cvf".to_string())),
                    ..Default::default()
                });
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::nested_vector()
            ])),
        },

        test_validate_exposes_invalid_aggregation_different_availability => {
            input = {
                let mut decl = new_component_decl();
                decl.exposes = Some(vec![
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll_a".into()
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Expose::Service(fdecl::ExposeService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll_b".into()
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        availability: Some(fdecl::Availability::Optional),
                        ..Default::default()
                    })
                ]);
                decl.collections = Some(vec![
                    fdecl::Collection {
                        name: Some("coll_a".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                    fdecl::Collection {
                        name: Some("coll_b".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::DifferentAvailabilityInAggregation(AvailabilityList(vec![
                    fdecl::Availability::Required,
                    fdecl::Availability::Optional,
                ]))
            ])),
        },

        test_validate_offers_invalid_aggregation_different_availability => {
            input = {
                let mut decl = new_component_decl();
                decl.offers = Some(vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll_a".into()
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "child_c".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        source_instance_filter: Some(vec!["default".to_string()]),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll_b".into()
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "child_c".to_string(),
                                collection: None,
                            }
                        )),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        source_instance_filter: Some(vec!["a_different_default".to_string()]),
                        availability: Some(fdecl::Availability::Optional),
                        ..Default::default()
                    })
                ]);
                decl.collections = Some(vec![
                    fdecl::Collection {
                        name: Some("coll_a".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                    fdecl::Collection {
                        name: Some("coll_b".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                ]);
                decl.children = Some(vec![
                    fdecl::Child {
                        name: Some("child_c".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        on_terminate: None,
                        environment: None,
                        ..Default::default()
                    },
                ]);
                decl
            },
            result = Err(ErrorList::new(vec![
                Error::DifferentAvailabilityInAggregation(AvailabilityList(vec![
                    fdecl::Availability::Required,
                    fdecl::Availability::Optional,
                ]))
            ])),
        },
    }

    test_validate_capabilities! {
        test_validate_capabilities_individually_ok => {
            input = vec![
                fdecl::Capability::Protocol(fdecl::Protocol {
                    name: Some("foo_svc".into()),
                    source_path: Some("/svc/foo".into()),
                    ..Default::default()
                }),
                fdecl::Capability::Directory(fdecl::Directory {
                    name: Some("foo_dir".into()),
                    source_path: Some("/foo".into()),
                    rights: Some(fio::Operations::CONNECT),
                    ..Default::default()
                }),
            ],
            as_builtin = false,
            result = Ok(()),
        },
        test_validate_capabilities_individually_err => {
            input = vec![
                fdecl::Capability::Protocol(fdecl::Protocol {
                    name: None,
                    source_path: None,
                    ..Default::default()
                }),
                fdecl::Capability::Directory(fdecl::Directory {
                    name: None,
                    source_path: None,
                    rights: None,
                    ..Default::default()
                }),
            ],
            as_builtin = false,
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::Protocol, "name"),
                Error::missing_field(DeclType::Protocol, "source_path"),
                Error::missing_field(DeclType::Directory, "name"),
                Error::missing_field(DeclType::Directory, "source_path"),
                Error::missing_field(DeclType::Directory, "rights"),
            ])),
        },
        test_validate_builtin_capabilities_individually_ok => {
            input = vec![
                fdecl::Capability::Protocol(fdecl::Protocol {
                    name: Some("foo_protocol".into()),
                    source_path: None,
                    ..Default::default()
                }),
                fdecl::Capability::Directory(fdecl::Directory {
                    name: Some("foo_dir".into()),
                    source_path: None,
                    rights: Some(fio::Operations::CONNECT),
                    ..Default::default()
                }),
                fdecl::Capability::Service(fdecl::Service {
                    name: Some("foo_svc".into()),
                    source_path: None,
                    ..Default::default()
                }),
                fdecl::Capability::Runner(fdecl::Runner {
                    name: Some("foo_runner".into()),
                    source_path: None,
                    ..Default::default()
                }),
                fdecl::Capability::Resolver(fdecl::Resolver {
                    name: Some("foo_resolver".into()),
                    source_path: None,
                    ..Default::default()
                }),
            ],
            as_builtin = true,
            result = Ok(()),
        },
        test_validate_builtin_capabilities_individually_err => {
            input = vec![
                fdecl::Capability::Protocol(fdecl::Protocol {
                    name: None,
                    source_path: Some("/svc/foo".into()),
                    ..Default::default()
                }),
                fdecl::Capability::Directory(fdecl::Directory {
                    name: None,
                    source_path: Some("/foo".into()),
                    rights: None,
                    ..Default::default()
                }),
                fdecl::Capability::Service(fdecl::Service {
                    name: None,
                    source_path: Some("/svc/foo".into()),
                    ..Default::default()
                }),
                fdecl::Capability::Runner(fdecl::Runner {
                    name: None,
                    source_path:  Some("/foo".into()),
                    ..Default::default()
                }),
                fdecl::Capability::Resolver(fdecl::Resolver {
                    name: None,
                    source_path:  Some("/foo".into()),
                    ..Default::default()
                }),
                fdecl::Capability::Storage(fdecl::Storage {
                    name: None,
                    ..Default::default()
                }),
            ],
            as_builtin = true,
            result = Err(ErrorList::new(vec![
                Error::missing_field(DeclType::Protocol, "name"),
                Error::extraneous_source_path(DeclType::Protocol, "/svc/foo"),
                Error::missing_field(DeclType::Directory, "name"),
                Error::extraneous_source_path(DeclType::Directory, "/foo"),
                Error::missing_field(DeclType::Directory, "rights"),
                Error::missing_field(DeclType::Service, "name"),
                Error::extraneous_source_path(DeclType::Service, "/svc/foo"),
                Error::missing_field(DeclType::Runner, "name"),
                Error::extraneous_source_path(DeclType::Runner, "/foo"),
                Error::missing_field(DeclType::Resolver, "name"),
                Error::extraneous_source_path(DeclType::Resolver, "/foo"),
                Error::CapabilityCannotBeBuiltin(DeclType::Storage),
            ])),
        },
        test_validate_delivery_type_ok => {
            input = vec![
                fdecl::Capability::Protocol(fdecl::Protocol {
                    name: Some("foo_svc1".into()),
                    source_path: Some("/svc/foo1".into()),
                    ..Default::default()
                }),
                fdecl::Capability::Protocol(fdecl::Protocol {
                    name: Some("foo_svc2".into()),
                    source_path: Some("/svc/foo2".into()),
                    delivery: Some(fdecl::DeliveryType::Immediate),
                    ..Default::default()
                }),
                fdecl::Capability::Protocol(fdecl::Protocol {
                    name: Some("foo_svc3".into()),
                    source_path: Some("/svc/foo3".into()),
                    delivery: Some(fdecl::DeliveryType::OnReadable),
                    ..Default::default()
                }),
            ],
            as_builtin = false,
            result = Ok(()),
        },
        test_validate_delivery_type_err => {
            input = vec![
                fdecl::Capability::Protocol(fdecl::Protocol {
                    name: Some("foo_svc".into()),
                    source_path: Some("/svc/foo".into()),
                    delivery: Some(fdecl::DeliveryType::unknown()),
                    ..Default::default()
                }),
            ],
            as_builtin = false,
            result = Err(ErrorList::new(vec![
                Error::invalid_field(DeclType::Protocol, "delivery"),
            ])),
        },
    }

    /// Passes different source and availability options to `new_expose` to
    /// generate a component declaration.
    fn generate_expose_different_source_and_availability_decl(
        new_expose: impl Fn(fdecl::Ref, fdecl::Availability, &str) -> fdecl::Expose,
    ) -> fdecl::Component {
        let mut decl = new_component_decl();
        let child = "child";
        decl.children = Some(vec![fdecl::Child {
            name: Some(child.to_string()),
            url: Some("fuchsia-pkg://fuchsia.com/source#meta/source.cm".to_string()),
            startup: Some(fdecl::StartupMode::Lazy),
            ..Default::default()
        }]);
        decl.exposes = Some(vec![
            // Optional expose from self is okay.
            new_expose(
                fdecl::Ref::Self_(fdecl::SelfRef {}),
                fdecl::Availability::Optional,
                "fuchsia.examples.Echo1",
            ),
            // Optional expose from child is okay.
            new_expose(
                fdecl::Ref::Child(fdecl::ChildRef { name: child.to_string(), collection: None }),
                fdecl::Availability::Optional,
                "fuchsia.examples.Echo2",
            ),
            // Optional expose from void is okay.
            new_expose(
                fdecl::Ref::VoidType(fdecl::VoidRef {}),
                fdecl::Availability::Optional,
                "fuchsia.examples.Echo3",
            ),
            // Optional expose from framework is okay.
            new_expose(
                fdecl::Ref::Framework(fdecl::FrameworkRef {}),
                fdecl::Availability::Optional,
                "fuchsia.examples.Echo4",
            ),
            // Transitional expose from self is okay.
            new_expose(
                fdecl::Ref::Self_(fdecl::SelfRef {}),
                fdecl::Availability::Transitional,
                "fuchsia.examples.Echo5",
            ),
            // Transitional expose from child is okay.
            new_expose(
                fdecl::Ref::Child(fdecl::ChildRef { name: child.to_string(), collection: None }),
                fdecl::Availability::Transitional,
                "fuchsia.examples.Echo6",
            ),
            // Transitional expose from void is okay.
            new_expose(
                fdecl::Ref::VoidType(fdecl::VoidRef {}),
                fdecl::Availability::Transitional,
                "fuchsia.examples.Echo7",
            ),
            // Transitional expose from framework is okay.
            new_expose(
                fdecl::Ref::Framework(fdecl::FrameworkRef {}),
                fdecl::Availability::Transitional,
                "fuchsia.examples.Echo8",
            ),
            // Same-as-target expose from self is okay.
            new_expose(
                fdecl::Ref::Self_(fdecl::SelfRef {}),
                fdecl::Availability::SameAsTarget,
                "fuchsia.examples.Echo9",
            ),
            // Same-as-target expose from child is okay.
            new_expose(
                fdecl::Ref::Child(fdecl::ChildRef { name: child.to_string(), collection: None }),
                fdecl::Availability::SameAsTarget,
                "fuchsia.examples.Echo10",
            ),
            // Same-as-target expose from framework is okay.
            new_expose(
                fdecl::Ref::Framework(fdecl::FrameworkRef {}),
                fdecl::Availability::SameAsTarget,
                "fuchsia.examples.Echo11",
            ),
            // Required expose from void is an error.
            new_expose(
                fdecl::Ref::VoidType(fdecl::VoidRef {}),
                fdecl::Availability::Required,
                "fuchsia.examples.Echo12",
            ),
            // Same-as-target expose from void is an error.
            new_expose(
                fdecl::Ref::VoidType(fdecl::VoidRef {}),
                fdecl::Availability::SameAsTarget,
                "fuchsia.examples.Echo13",
            ),
        ]);
        decl
    }

    /// Passes different source and availability options to `new_offer` to
    /// generate a component declaration.
    fn generate_offer_different_source_and_availability_decl(
        new_offer: impl Fn(fdecl::Ref, fdecl::Availability, &str) -> fdecl::Offer,
    ) -> fdecl::Component {
        let mut decl = new_component_decl();
        decl.children = Some(vec![
            fdecl::Child {
                name: Some("source".to_string()),
                url: Some("fuchsia-pkg://fuchsia.com/source#meta/source.cm".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                on_terminate: None,
                environment: None,
                ..Default::default()
            },
            fdecl::Child {
                name: Some("sink".to_string()),
                url: Some("fuchsia-pkg://fuchsia.com/sink#meta/sink.cm".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                on_terminate: None,
                environment: None,
                ..Default::default()
            },
        ]);
        decl.offers = Some(vec![
            // These offers are fine, offers with a source of parent or void can be
            // optional.
            new_offer(
                fdecl::Ref::Parent(fdecl::ParentRef {}),
                fdecl::Availability::Required,
                "fuchsia.examples.Echo0",
            ),
            new_offer(
                fdecl::Ref::Parent(fdecl::ParentRef {}),
                fdecl::Availability::Optional,
                "fuchsia.examples.Echo1",
            ),
            new_offer(
                fdecl::Ref::Parent(fdecl::ParentRef {}),
                fdecl::Availability::SameAsTarget,
                "fuchsia.examples.Echo2",
            ),
            new_offer(
                fdecl::Ref::VoidType(fdecl::VoidRef {}),
                fdecl::Availability::Optional,
                "fuchsia.examples.Echo3",
            ),
            // These offers are fine, offers with a source other than parent or void
            // can also be optional.
            new_offer(
                fdecl::Ref::Self_(fdecl::SelfRef {}),
                fdecl::Availability::Optional,
                "fuchsia.examples.Echo4",
            ),
            new_offer(
                fdecl::Ref::Self_(fdecl::SelfRef {}),
                fdecl::Availability::SameAsTarget,
                "fuchsia.examples.Echo5",
            ),
            new_offer(
                fdecl::Ref::Framework(fdecl::FrameworkRef {}),
                fdecl::Availability::Optional,
                "fuchsia.examples.Echo6",
            ),
            new_offer(
                fdecl::Ref::Framework(fdecl::FrameworkRef {}),
                fdecl::Availability::SameAsTarget,
                "fuchsia.examples.Echo7",
            ),
            new_offer(
                fdecl::Ref::Child(fdecl::ChildRef { name: "source".to_string(), collection: None }),
                fdecl::Availability::Optional,
                "fuchsia.examples.Echo8",
            ),
            new_offer(
                fdecl::Ref::Child(fdecl::ChildRef { name: "source".to_string(), collection: None }),
                fdecl::Availability::SameAsTarget,
                "fuchsia.examples.Echo9",
            ),
            // These offers are also not fine, offers with a source of void must be optional
            new_offer(
                fdecl::Ref::VoidType(fdecl::VoidRef {}),
                fdecl::Availability::Required,
                "fuchsia.examples.Echo10",
            ),
            new_offer(
                fdecl::Ref::VoidType(fdecl::VoidRef {}),
                fdecl::Availability::SameAsTarget,
                "fuchsia.examples.Echo11",
            ),
        ]);
        decl
    }

    #[test]
    fn test_validate_dynamic_offers_empty() {
        assert_eq!(
            validate_dynamic_offers(
                vec![],
                &mut DirectedGraph::new(),
                &vec![],
                &fdecl::Component::default()
            ),
            Ok(())
        );
    }

    #[test]
    fn test_validate_dynamic_offers_okay() {
        assert_eq!(
            validate_dynamic_offers(
                vec![],
                &mut DirectedGraph::new(),
                &vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef)),
                        source_name: Some("thing".to_string()),
                        target_name: Some("thing".repeat(26)),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "foo".to_string(),
                            collection: Some("foo".to_string()),
                        })),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("thang".repeat(26)),
                        target_name: Some("thang".repeat(26)),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "foo".to_string(),
                            collection: Some("foo".to_string()),
                        })),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("thung1".repeat(26)),
                        target_name: Some("thung1".repeat(26)),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "foo".to_string(),
                            collection: Some("foo".to_string()),
                        })),
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("thung2".repeat(26)),
                        target_name: Some("thung2".repeat(26)),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "foo".to_string(),
                            collection: Some("foo".to_string()),
                        })),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("thung3".repeat(26)),
                        target_name: Some("thung3".repeat(26)),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "foo".to_string(),
                            collection: Some("foo".to_string()),
                        })),
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("thung4".repeat(26)),
                        target_name: Some("thung4".repeat(26)),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "foo".to_string(),
                            collection: Some("foo".to_string()),
                        })),
                        ..Default::default()
                    }),
                ],
                &fdecl::Component {
                    capabilities: Some(vec![fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("thing".to_string()),
                        source_path: Some("/svc/foo".into()),
                        ..Default::default()
                    }),]),
                    ..Default::default()
                }
            ),
            Ok(())
        );
    }

    #[test]
    fn test_validate_dynamic_offers_valid_service_aggregation() {
        assert_eq!(
            validate_dynamic_offers(
                vec![],
                &mut DirectedGraph::new(),
                &vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child_a".to_string(),
                            collection: None
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child_c".to_string(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        source_instance_filter: Some(vec!["default".to_string()]),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child_b".to_string(),
                            collection: None
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child_c".to_string(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        source_instance_filter: Some(vec!["a_different_default".to_string()]),
                        ..Default::default()
                    })
                ],
                &fdecl::Component {
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child_a".to_string()),
                            url: Some(
                                "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                                    .to_string()
                            ),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            environment: None,
                            ..Default::default()
                        },
                        fdecl::Child {
                            name: Some("child_b".to_string()),
                            url: Some(
                                "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                                    .to_string()
                            ),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            environment: None,
                            ..Default::default()
                        },
                        fdecl::Child {
                            name: Some("child_c".to_string()),
                            url: Some(
                                "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                                    .to_string()
                            ),
                            startup: Some(fdecl::StartupMode::Lazy),
                            on_terminate: None,
                            environment: None,
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                }
            ),
            Ok(())
        );
    }

    #[test]
    fn test_validate_dynamic_service_aggregation_missing_filter() {
        assert_eq!(
            validate_dynamic_offers(
                vec![],
                &mut DirectedGraph::new(),
                &vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child_a".to_string(),
                            collection: None
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child_c".to_string(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        source_instance_filter: Some(vec!["default".to_string()]),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child_b".to_string(),
                            collection: None
                        })),
                        source_name: Some("fuchsia.logger.Log".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "child_c".to_string(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.Log".to_string()),
                        source_instance_filter: None,
                        ..Default::default()
                    }),
                ],
                &fdecl::Component {
                    children: Some(vec![
                        fdecl::Child {
                            name: Some("child_a".to_string()),
                            url: Some(
                                "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                                    .to_string()
                            ),
                            startup: Some(fdecl::StartupMode::Lazy),
                            ..Default::default()
                        },
                        fdecl::Child {
                            name: Some("child_b".to_string()),
                            url: Some(
                                "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                                    .to_string()
                            ),
                            startup: Some(fdecl::StartupMode::Lazy),
                            ..Default::default()
                        },
                        fdecl::Child {
                            name: Some("child_c".to_string()),
                            url: Some(
                                "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                                    .to_string()
                            ),
                            startup: Some(fdecl::StartupMode::Lazy),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                },
            ),
            Err(ErrorList::new(vec![Error::invalid_aggregate_offer(
                "source_instance_filter must be set for dynamic aggregate service offers"
            ),]))
        );
    }

    #[test]
    fn test_validate_dynamic_offers_omit_target() {
        assert_eq!(
            validate_dynamic_offers(
                vec![],
                &mut DirectedGraph::new(),
                &vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef)),
                        source_name: Some("thing".to_string()),
                        target_name: Some("thing".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Service(fdecl::OfferService {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("thang".to_string()),
                        target_name: Some("thang".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Directory(fdecl::OfferDirectory {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("thung1".to_string()),
                        target_name: Some("thung1".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Storage(fdecl::OfferStorage {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("thung2".to_string()),
                        target_name: Some("thung2".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Runner(fdecl::OfferRunner {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("thung3".to_string()),
                        target_name: Some("thung3".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Offer::Resolver(fdecl::OfferResolver {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("thung4".to_string()),
                        target_name: Some("thung4".to_string()),
                        ..Default::default()
                    }),
                ],
                &fdecl::Component {
                    capabilities: Some(vec![fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("thing".to_string()),
                        source_path: Some("/svc/foo".into()),
                        ..Default::default()
                    }),]),
                    ..Default::default()
                }
            ),
            Err(ErrorList::new(vec![
                Error::missing_field(DeclType::OfferProtocol, "target"),
                Error::missing_field(DeclType::OfferService, "target"),
                Error::missing_field(DeclType::OfferDirectory, "target"),
                Error::missing_field(DeclType::OfferStorage, "target"),
                Error::missing_field(DeclType::OfferRunner, "target"),
                Error::missing_field(DeclType::OfferResolver, "target"),
            ]))
        );
    }

    #[test]
    fn test_validate_dynamic_offers_collection_collision() {
        assert_eq!(
            validate_dynamic_offers(
                vec![],
                &mut DirectedGraph::new(),
                &vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                    dependency_type: Some(fdecl::DependencyType::Strong),
                    source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                    source_name: Some("thing".to_string()),
                    target_name: Some("thing".to_string()),
                    target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "child".to_string(),
                        collection: Some("coll".to_string()),
                    })),
                    ..Default::default()
                }),],
                &fdecl::Component {
                    offers: Some(vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                        source_name: Some("thing".to_string()),
                        target_name: Some("thing".to_string()),
                        target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll".into()
                        })),
                        ..Default::default()
                    }),]),
                    collections: Some(vec![fdecl::Collection {
                        name: Some("coll".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },]),
                    ..Default::default()
                }
            ),
            Err(ErrorList::new(vec![Error::duplicate_field(
                DeclType::OfferProtocol,
                "target_name",
                "thing"
            ),]))
        );
    }

    #[test]
    fn test_validate_dynamic_offers_cycle_collection_to_static_child() {
        assert_eq!(
            validate_dynamic_offers(
                vec![("dyn", "coll")],
                &mut DirectedGraph::new(),
                &vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                    source_name: Some("bar".to_string()),
                    target_name: Some("bar".to_string()),
                    source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "static_child".into(),
                        collection: None,
                    })),
                    target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "dyn".to_string(),
                        collection: Some("coll".to_string()),
                    })),
                    dependency_type: Some(fdecl::DependencyType::Strong),
                    ..Default::default()
                }),],
                &fdecl::Component {
                    offers: Some(vec![fdecl::Offer::Service(fdecl::OfferService {
                        source_name: Some("foo".to_string()),
                        target_name: Some("foo".to_string()),
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll".into(),
                        })),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "static_child".into(),
                            collection: None,
                        })),
                        ..Default::default()
                    })]),
                    children: Some(vec![fdecl::Child {
                        name: Some("static_child".into()),
                        url: Some("url#child.cm".into()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    }]),
                    collections: Some(vec![fdecl::Collection {
                        name: Some("coll".into()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }
            ),
            Err(ErrorList::new(vec![Error::dependency_cycle(
                "{{child coll:dyn -> collection coll -> child static_child -> child coll:dyn}}"
            )]))
        );
    }

    #[test]
    fn test_validate_dynamic_offers_cycle_collection_to_dynamic_child() {
        assert_eq!(
            validate_dynamic_offers(
                vec![("dyn", "coll1"), ("dyn", "coll2")],
                &mut DirectedGraph::new(),
                &vec![
                    fdecl::Offer::Service(fdecl::OfferService {
                        source_name: Some("foo".to_string()),
                        target_name: Some("foo".to_string()),
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll2".into(),
                        })),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "dyn".into(),
                            collection: Some("coll1".into()),
                        })),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source_name: Some("bar".to_string()),
                        target_name: Some("bar".to_string()),
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "dyn".into(),
                            collection: Some("coll1".into()),
                        })),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "dyn".to_string(),
                            collection: Some("coll2".to_string()),
                        })),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                ],
                &fdecl::Component {
                    collections: Some(vec![
                        fdecl::Collection {
                            name: Some("coll1".into()),
                            durability: Some(fdecl::Durability::Transient),
                            ..Default::default()
                        },
                        fdecl::Collection {
                            name: Some("coll2".into()),
                            durability: Some(fdecl::Durability::Transient),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                }
            ),
            Err(ErrorList::new(vec![Error::dependency_cycle(
                "{{child coll1:dyn -> child coll2:dyn -> collection coll2 -> child coll1:dyn}}"
            )]))
        );
    }

    #[test]
    fn test_validate_dynamic_offers_cycle_collection_to_dynamic_child_between_new_and_existing() {
        assert_eq!(
            validate_dynamic_offers(
                vec![("dyn", "coll1"), ("dyn", "coll2")],
                &mut DirectedGraph::from([(
                    DependencyNode::Child("dyn".into(), Some("coll1".into())),
                    DependencyNode::Child("dyn".into(), Some("coll2".into())),
                )]),
                &vec![fdecl::Offer::Service(fdecl::OfferService {
                    source_name: Some("foo".to_string()),
                    target_name: Some("foo".to_string()),
                    source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                        name: "coll2".into(),
                    })),
                    target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "dyn".into(),
                        collection: Some("coll1".into()),
                    })),
                    ..Default::default()
                }),],
                &fdecl::Component {
                    collections: Some(vec![
                        fdecl::Collection {
                            name: Some("coll1".into()),
                            durability: Some(fdecl::Durability::Transient),
                            ..Default::default()
                        },
                        fdecl::Collection {
                            name: Some("coll2".into()),
                            durability: Some(fdecl::Durability::Transient),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                }
            ),
            Err(ErrorList::new(vec![Error::dependency_cycle(
                "{{child coll1:dyn -> child coll2:dyn -> collection coll2 -> child coll1:dyn}}"
            )]))
        );
    }

    #[test]
    fn test_validate_dynamic_offers_cycle_collection_to_collection() {
        assert_eq!(
            validate_dynamic_offers(
                vec![("dyn", "coll1"), ("dyn", "coll2")],
                &mut DirectedGraph::new(),
                &vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                    source_name: Some("bar".to_string()),
                    target_name: Some("bar".to_string()),
                    source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "dyn".into(),
                        collection: Some("coll2".parse().unwrap()),
                    })),
                    target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "dyn".into(),
                        collection: Some("coll1".parse().unwrap()),
                    })),
                    dependency_type: Some(fdecl::DependencyType::Strong),
                    ..Default::default()
                }),],
                &fdecl::Component {
                    offers: Some(vec![fdecl::Offer::Service(fdecl::OfferService {
                        source_name: Some("foo".to_string()),
                        target_name: Some("foo".to_string()),
                        source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll1".into(),
                        })),
                        target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll2".into(),
                        })),
                        ..Default::default()
                    })]),
                    collections: Some(vec![
                        fdecl::Collection {
                            name: Some("coll1".into()),
                            durability: Some(fdecl::Durability::Transient),
                            ..Default::default()
                        },
                        fdecl::Collection {
                            name: Some("coll2".into()),
                            durability: Some(fdecl::Durability::Transient),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                }
            ),
            Err(ErrorList::new(vec![Error::dependency_cycle(
                "{{child coll1:dyn -> collection coll1 -> child coll2:dyn -> child coll1:dyn}}",
            )]))
        );
    }

    #[test]
    fn test_validate_dynamic_child() {
        assert_eq!(
            Ok(()),
            validate_dynamic_child(&fdecl::Child {
                name: Some("a".repeat(MAX_LONG_NAME_LENGTH).to_string()),
                url: Some("test:///child".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                on_terminate: None,
                environment: None,
                ..Default::default()
            })
        );
    }

    #[test]
    fn test_validate_dynamic_child_environment_is_invalid() {
        assert_eq!(
            validate_dynamic_child(&fdecl::Child {
                name: Some("a".repeat(MAX_LONG_NAME_LENGTH).to_string()),
                url: Some("test:///child".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                on_terminate: None,
                environment: Some("env".to_string()),
                ..Default::default()
            }),
            Err(ErrorList::new(vec![Error::DynamicChildWithEnvironment]))
        );
    }

    #[test]
    fn test_validate_dynamic_offers_missing_stuff() {
        assert_eq!(
            validate_dynamic_offers(
                vec![],
                &mut DirectedGraph::new(),
                &vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol::default()),
                    fdecl::Offer::Service(fdecl::OfferService::default()),
                    fdecl::Offer::Directory(fdecl::OfferDirectory::default()),
                    fdecl::Offer::Storage(fdecl::OfferStorage::default()),
                    fdecl::Offer::Runner(fdecl::OfferRunner::default()),
                    fdecl::Offer::Resolver(fdecl::OfferResolver::default()),
                ],
                &fdecl::Component::default()
            ),
            Err(ErrorList::new(vec![
                Error::missing_field(DeclType::OfferProtocol, "source"),
                Error::missing_field(DeclType::OfferProtocol, "source_name"),
                Error::missing_field(DeclType::OfferProtocol, "target"),
                Error::missing_field(DeclType::OfferProtocol, "target_name"),
                Error::missing_field(DeclType::OfferProtocol, "dependency_type"),
                Error::missing_field(DeclType::OfferService, "source"),
                Error::missing_field(DeclType::OfferService, "source_name"),
                Error::missing_field(DeclType::OfferService, "target"),
                Error::missing_field(DeclType::OfferService, "target_name"),
                Error::missing_field(DeclType::OfferDirectory, "source"),
                Error::missing_field(DeclType::OfferDirectory, "source_name"),
                Error::missing_field(DeclType::OfferDirectory, "target"),
                Error::missing_field(DeclType::OfferDirectory, "target_name"),
                Error::missing_field(DeclType::OfferDirectory, "dependency_type"),
                Error::missing_field(DeclType::OfferStorage, "source"),
                Error::missing_field(DeclType::OfferStorage, "source_name"),
                Error::missing_field(DeclType::OfferStorage, "target"),
                Error::missing_field(DeclType::OfferStorage, "target_name"),
                Error::missing_field(DeclType::OfferRunner, "source"),
                Error::missing_field(DeclType::OfferRunner, "source_name"),
                Error::missing_field(DeclType::OfferRunner, "target"),
                Error::missing_field(DeclType::OfferRunner, "target_name"),
                Error::missing_field(DeclType::OfferResolver, "source"),
                Error::missing_field(DeclType::OfferResolver, "source_name"),
                Error::missing_field(DeclType::OfferResolver, "target"),
                Error::missing_field(DeclType::OfferResolver, "target_name"),
            ]))
        );
    }

    test_dependency! {
        (test_validate_offers_protocol_dependency_cycle) => {
            ty = fdecl::Offer::Protocol,
            offer_decl = fdecl::OfferProtocol {
                source: None,  // Filled by macro
                target: None,  // Filled by macro
                source_name: Some(format!("thing")),
                target_name: Some(format!("thing")),
                dependency_type: Some(fdecl::DependencyType::Strong),
                ..Default::default()
            },
        },
        (test_validate_offers_directory_dependency_cycle) => {
            ty = fdecl::Offer::Directory,
            offer_decl = fdecl::OfferDirectory {
                source: None,  // Filled by macro
                target: None,  // Filled by macro
                source_name: Some(format!("thing")),
                target_name: Some(format!("thing")),
                rights: Some(fio::Operations::CONNECT),
                subdir: None,
                dependency_type: Some(fdecl::DependencyType::Strong),
                ..Default::default()
            },
        },
        (test_validate_offers_service_dependency_cycle) => {
            ty = fdecl::Offer::Service,
            offer_decl = fdecl::OfferService {
                source: None,  // Filled by macro
                target: None,  // Filled by macro
                source_name: Some(format!("thing")),
                target_name: Some(format!("thing")),
                ..Default::default()
            },
        },
        (test_validate_offers_runner_dependency_cycle) => {
            ty = fdecl::Offer::Runner,
            offer_decl = fdecl::OfferRunner {
                source: None,  // Filled by macro
                target: None,  // Filled by macro
                source_name: Some(format!("thing")),
                target_name: Some(format!("thing")),
                ..Default::default()
            },
        },
        (test_validate_offers_resolver_dependency_cycle) => {
            ty = fdecl::Offer::Resolver,
            offer_decl = fdecl::OfferResolver {
                source: None,  // Filled by macro
                target: None,  // Filled by macro
                source_name: Some(format!("thing")),
                target_name: Some(format!("thing")),
                ..Default::default()
            },
        },
    }
    test_weak_dependency! {
        (test_validate_offers_protocol_weak_dependency_cycle) => {
            ty = fdecl::Offer::Protocol,
            offer_decl = fdecl::OfferProtocol {
                source: None,  // Filled by macro
                target: None,  // Filled by macro
                source_name: Some(format!("thing")),
                target_name: Some(format!("thing")),
                dependency_type: None, // Filled by macro
                ..Default::default()
            },
        },
        (test_validate_offers_directory_weak_dependency_cycle) => {
            ty = fdecl::Offer::Directory,
            offer_decl = fdecl::OfferDirectory {
                source: None,  // Filled by macro
                target: None,  // Filled by macro
                source_name: Some(format!("thing")),
                target_name: Some(format!("thing")),
                rights: Some(fio::Operations::CONNECT),
                subdir: None,
                dependency_type: None,  // Filled by macro
                ..Default::default()
            },
        },
        (test_validate_offers_service_weak_dependency_cycle) => {
            ty = fdecl::Offer::Service,
            offer_decl = fdecl::OfferService {
                source: None,  // Filled by macro
                target: None,  // Filled by macro
                source_name: Some(format!("thing")),
                target_name: Some(format!("thing")),
                dependency_type: None, // Filled by macro
                ..Default::default()
            },
        },
    }
}
