// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::features::{Feature, FeatureSet};
use crate::types::capability::{CapabilityFromRef, ContextCapability};
use crate::types::capability_id::CapabilityId;
use crate::types::child::ContextChild;
use crate::types::collection::ContextCollection;
use crate::types::document::DocumentContext;
use crate::types::environment::{ContextEnvironment, EnvironmentExtends, RegistrationRef};
use crate::types::expose::{ContextExpose, ExposeFromRef, ExposeToRef};
use crate::types::offer::{
    ContextOffer, OfferFromRef, OfferToAllCapability, OfferToRef, TargetAvailability,
    offer_to_all_would_duplicate_context,
};
use crate::types::program::ContextProgram;
use crate::types::right::Rights;
use crate::types::r#use::{ContextUse, UseFromRef};
use crate::{
    AnyRef, Availability, ConfigKey, ConfigType, ConfigValueType, ContextCapabilityClause,
    ContextSpanned, DependencyType, DictionaryRef, Error, EventScope, FromClauseContext, OneOrMany,
    RootDictionaryRef, SourceAvailability,
};
use cm_types::{BorrowedName, IterablePath, Name};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::hash::Hash;

use std::sync::Arc;

#[derive(Default, Clone)]
pub struct CapabilityRequirements<'a> {
    pub must_offer: &'a [OfferToAllCapability<'a>],
    pub must_use: &'a [MustUseRequirement<'a>],
}

#[derive(PartialEq)]
pub enum MustUseRequirement<'a> {
    Protocol(&'a str),
}

impl<'a> MustUseRequirement<'a> {
    fn name(&self) -> &str {
        match self {
            MustUseRequirement::Protocol(name) => name,
        }
    }
}

macro_rules! val {
    ($field:expr) => {
        $field.as_ref().map(|s| &s.value)
    };
}

pub(crate) fn validate_cml(
    document: &DocumentContext,
    features: &FeatureSet,
    capability_requirements: &CapabilityRequirements<'_>,
) -> Result<(), Error> {
    let mut ctx = ValidationContext::new(&document, features, capability_requirements);
    ctx.validate()
}

fn offer_can_have_dependency(offer: &ContextOffer) -> bool {
    offer.directory.is_some()
        || offer.protocol.is_some()
        || offer.service.is_some()
        || offer.dictionary.is_some()
}

fn offer_dependency(offer: &ContextOffer) -> DependencyType {
    match offer.dependency.clone() {
        Some(cs_dep) => cs_dep.value,
        None => DependencyType::Strong,
    }
}

type ConflictInfo<'a> = (CapabilityId<'a>, Arc<std::path::Path>);

struct ValidationContext<'a> {
    document: &'a DocumentContext,
    features: &'a FeatureSet,
    capability_requirements: &'a CapabilityRequirements<'a>,
    all_children: HashMap<Name, &'a ContextSpanned<ContextChild>>,
    all_collections: HashSet<Name>,
    all_resolvers: HashSet<Name>,
    all_runners: HashSet<Name>,
    all_storages: HashMap<Name, &'a CapabilityFromRef>,
    all_capability_names: HashSet<Name>,
    all_dictionaries: HashMap<Name, &'a ContextCapability>,
    all_directories: HashSet<Name>,
    all_protocols: HashSet<Name>,
    all_configs: HashSet<Name>,
    all_services: HashSet<Name>,
}

impl<'a> ValidationContext<'a> {
    fn new(
        document: &'a DocumentContext,
        features: &'a FeatureSet,
        capability_requirements: &'a CapabilityRequirements<'a>,
    ) -> Self {
        ValidationContext {
            document,
            features,
            capability_requirements,
            all_children: HashMap::new(),
            all_collections: HashSet::new(),
            all_resolvers: HashSet::new(),
            all_runners: HashSet::new(),
            all_storages: HashMap::new(),
            all_capability_names: HashSet::new(),
            all_dictionaries: HashMap::new(),
            all_directories: HashSet::new(),
            all_protocols: HashSet::new(),
            all_configs: HashSet::new(),
            all_services: HashSet::new(),
        }
    }

    fn populate_and_check_names(&mut self) -> Result<(), Error> {
        let child_names = self.document.all_children_names();
        let col_names = self.document.all_collection_names();
        let storage_names = self.document.all_storage_names();
        let runner_names = self.document.all_runner_names();
        let resolver_names = self.document.all_resolver_names();
        let env_names = self.document.all_environment_names();
        let dict_names = self.document.all_dictionary_names();

        ensure_no_duplicate_names(
            child_names
                .iter()
                .map(|n| (*n, "children"))
                .chain(col_names.iter().map(|n| (*n, "collections")))
                .chain(storage_names.iter().map(|n| (*n, "storage")))
                .chain(runner_names.iter().map(|n| (*n, "runner")))
                .chain(resolver_names.iter().map(|n| (*n, "resolver")))
                .chain(env_names.iter().map(|n| (*n, "environment")))
                .chain(
                    dict_names
                        .iter()
                        .map(|n: &&cm_types::BoundedBorrowedName<255>| (*n, "dictionary")),
                ),
        )?;

        if let Some(children) = &self.document.children {
            self.all_children = children.iter().map(|c| (c.value.name.value.clone(), c)).collect();
        }

        self.all_collections = col_names.into_iter().map(|n| n.to_owned()).collect();
        self.all_capability_names = self.document.all_capability_names();
        self.all_storages = self.document.all_storage_with_sources().into_iter().collect();

        self.all_dictionaries = self.document.all_dictionaries().into_iter().collect();
        self.all_configs =
            self.document.all_config_names().into_iter().map(|n| n.to_owned()).collect();
        self.all_services = self
            .document
            .capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .filter(|c| c.value.service.is_some())
                    .flat_map(|c| c.value.names())
                    .map(|n| n.value)
                    .collect()
            })
            .unwrap_or_default();

        self.all_directories = self
            .document
            .capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .filter(|c| c.value.directory.is_some())
                    .flat_map(|c| c.value.names())
                    .map(|n| n.value)
                    .collect()
            })
            .unwrap_or_default();

        self.all_runners = self
            .document
            .capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .filter(|c| c.value.runner.is_some())
                    .flat_map(|c| c.value.names())
                    .map(|n| n.value)
                    .collect()
            })
            .unwrap_or_default();

        self.all_resolvers = self
            .document
            .capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .filter(|c| c.value.resolver.is_some())
                    .flat_map(|c| c.value.names())
                    .map(|n| n.value)
                    .collect()
            })
            .unwrap_or_default();

        self.all_protocols = self
            .document
            .capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .filter(|c| c.value.protocol.is_some())
                    .flat_map(|c| c.value.names())
                    .map(|n| n.value)
                    .collect()
            })
            .unwrap_or_default();

        Ok(())
    }

    fn validate(&mut self) -> Result<(), Error> {
        self.populate_and_check_names()?;

        if let Some(children) = self.document.children.as_ref() {
            for child in children {
                self.validate_child(&child)?;
            }
        }

        if let Some(collections) = self.document.collections.as_ref() {
            for collection in collections {
                self.validate_collection(&collection)?;
            }
        }

        if let Some(capabilities) = self.document.capabilities.as_ref() {
            let mut used_ids = HashMap::new();
            for capability in capabilities.iter() {
                self.validate_capability(&capability, &mut used_ids)?;
            }
        }

        if let Some(uses) = self.document.r#use.as_ref() {
            let mut used_ids = HashMap::new();
            for use_ in uses.iter() {
                self.validate_use(&use_, &mut used_ids)?;
            }
        }

        let uses_runner = self
            .document
            .r#use
            .as_ref()
            .map_or(false, |uses| uses.iter().any(|u| u.value.runner.is_some()));

        if uses_runner {
            self.validate_runner_not_specified(self.document.program.as_ref())?;
        } else {
            self.validate_runner_specified(self.document.program.as_ref())?;
        }

        if let Some(exposes) = self.document.expose.as_ref() {
            let mut used_ids = HashMap::new();
            let mut exposed_to_framework_ids = HashMap::new();
            for expose in exposes.iter() {
                self.validate_expose(&expose, &mut used_ids, &mut exposed_to_framework_ids)?;
            }
        }

        if let Some(offers) = self.document.offer.as_ref() {
            let mut problem_protocols: Vec<ConflictInfo<'a>> = Vec::new();
            let mut problem_dictionaries: Vec<ConflictInfo<'a>> = Vec::new();
            let mut offered_ids: HashSet<CapabilityId<'a>> = HashSet::new();

            offers
                .iter()
                .filter(|o_span| matches!(o_span.value.to.value, OneOrMany::One(OfferToRef::All)))
                .try_for_each(|offer_wrapper| -> Result<(), Error> {
                    let offer = &offer_wrapper.value;
                    let mut process_cap = |field_is_some: bool,
                                           conflict_list: &mut Vec<ConflictInfo<'a>>|
                     -> Result<(), Error> {
                        if field_is_some {
                            for (cap_id, cap_origin) in
                                CapabilityId::from_context_offer_expose(offer_wrapper)?
                            {
                                if !offered_ids.insert(cap_id.clone()) {
                                    conflict_list.push((cap_id, cap_origin));
                                }
                            }
                        }
                        Ok(())
                    };

                    process_cap(offer.protocol.is_some(), &mut problem_protocols)?;
                    process_cap(offer.dictionary.is_some(), &mut problem_dictionaries)?;
                    Ok(())
                })?;

            if !problem_protocols.is_empty() {
                return Err(Error::validate_contexts(
                    format!(
                        r#"{} {:?} offered to "all" multiple times"#,
                        "Protocol(s)",
                        problem_protocols.iter().map(|(p, _o)| format!("{p}")).collect::<Vec<_>>()
                    ),
                    problem_protocols.iter().map(|(_p, o)| o.clone()).collect::<Vec<_>>(),
                ));
            }

            if !problem_dictionaries.is_empty() {
                return Err(Error::validate_contexts(
                    format!(
                        r#"{} {:?} offered to "all" multiple times"#,
                        "Dictionary(s)",
                        problem_dictionaries
                            .iter()
                            .map(|(p, _)| format!("{p}"))
                            .collect::<Vec<_>>()
                    ),
                    problem_dictionaries.iter().map(|(_p, o)| o.clone()).collect::<Vec<_>>(),
                ));
            }

            let offered_to_all = offers
                .iter()
                .filter(|o| matches!(o.value.to.value, OneOrMany::One(OfferToRef::All)))
                .filter(|o| o.value.protocol.is_some() || o.value.dictionary.is_some())
                .collect::<Vec<&ContextSpanned<ContextOffer>>>();

            let mut offered_ids = HashMap::new();
            for offer in offers.iter() {
                self.validate_offer(&offer, &mut offered_ids, &offered_to_all)?;
            }
        }

        if let Some(environments) = self.document.environments.as_ref() {
            for environment in environments {
                self.validate_environment(&environment)?;
            }
        }

        self.validate_required_offer_decls()?;

        self.validate_required_use_decls()?;

        self.validate_facets()?;

        self.validate_config()?;

        Ok(())
    }

    fn validate_child(
        &mut self,
        child_wrapper: &'a ContextSpanned<ContextChild>,
    ) -> Result<(), Error> {
        let child = &child_wrapper.value;

        if let Some(resource) = child.url.value.resource() {
            if resource.ends_with(".cml") {
                return Err(Error::validate_context(
                    format!(
                        "child URL ends in .cml instead of .cm, \
which is almost certainly a mistake: {}",
                        child.url.value
                    ),
                    Some(child.url.origin.clone()),
                ));
            }
        }

        Ok(())
    }

    fn validate_capability(
        &mut self,
        capability_wrapper: &'a ContextSpanned<ContextCapability>,
        used_ids: &mut HashMap<String, Arc<std::path::Path>>,
    ) -> Result<(), Error> {
        let capability = &capability_wrapper.value;

        if let Some(cs_directory) = &capability.directory {
            if capability.path.is_none() {
                return Err(Error::validate_context(
                    "\"path\" should be present with \"directory\"",
                    Some(cs_directory.origin.clone()),
                ));
            }

            if capability.rights.is_none() {
                return Err(Error::validate_context(
                    "\"rights\" should be present with \"directory\"",
                    Some(cs_directory.origin.clone()),
                ));
            }
        }

        if let Some(cs_storage) = capability.storage.as_ref() {
            if capability.from.is_none() {
                return Err(Error::validate_context(
                    "\"from\" should be present with \"storage\"",
                    Some(cs_storage.origin.clone()),
                ));
            }
            if let Some(cs_path) = &capability.path {
                return Err(Error::validate_context(
                    "\"path\" cannot be present with \"storage\", use \"backing_dir\"",
                    Some(cs_path.origin.clone()),
                ));
            }
            if capability.backing_dir.is_none() {
                return Err(Error::validate_context(
                    "\"backing_dir\" should be present with \"storage\"",
                    Some(cs_storage.origin.clone()),
                ));
            }
            if capability.storage_id.is_none() {
                return Err(Error::validate_context(
                    "\"storage_id\" should be present with \"storage\"",
                    Some(cs_storage.origin.clone()),
                ));
            }
        }

        if let Some(cs_runner) = &capability.runner {
            if let Some(cs_from) = &capability.from {
                return Err(Error::validate_context(
                    "\"from\" should not be present with \"runner\"",
                    Some(cs_from.origin.clone()),
                ));
            }

            if capability.path.is_none() {
                return Err(Error::validate_context(
                    "\"path\" should be present with \"runner\"",
                    Some(cs_runner.origin.clone()),
                ));
            }
        }

        if let Some(cs_resolver) = &capability.resolver {
            if let Some(cs_from) = &capability.from {
                return Err(Error::validate_context(
                    "\"from\" should not be present with \"resolver\"",
                    Some(cs_from.origin.clone()),
                ));
            }

            if capability.path.is_none() {
                return Err(Error::validate_context(
                    "\"path\" should be present with \"resolver\"",
                    Some(cs_resolver.origin.clone()),
                ));
            }
        }

        if capability.dictionary.as_ref().is_some() && capability.path.is_some() {
            self.features.check(Feature::DynamicDictionaries)?;
        }
        if capability.delivery.is_some() {
            self.features.check(Feature::DeliveryType)?;
        }
        if let Some(from) = capability.from.as_ref() {
            self.validate_component_child_ref(
                "\"capabilities\" source",
                &AnyRef::from(&from.value),
                Some(&capability_wrapper.origin),
            )?;
        }

        // Disallow multiple capability ids of the same name.
        let capability_ids = CapabilityId::from_context_capability(capability_wrapper)?;
        for (capability_id, cap_origin) in capability_ids {
            if let Some(conflict_origin) =
                used_ids.insert(capability_id.to_string(), cap_origin.clone())
            {
                return Err(Error::validate_contexts(
                    format!("\"{}\" is a duplicate \"capability\" name", capability_id,),
                    vec![cap_origin, conflict_origin],
                ));
            }
        }

        Ok(())
    }

    fn validate_use(
        &mut self,
        use_wrapper: &'a ContextSpanned<ContextUse>,
        used_ids: &mut HashMap<String, (CapabilityId<'a>, Arc<std::path::Path>)>,
    ) -> Result<(), Error> {
        use_wrapper.capability_type(Some(use_wrapper.origin.clone()))?;
        let use_ = &use_wrapper.value;

        for checker in [
            self.service_from_self_checker(use_),
            self.protocol_from_self_checker(use_),
            self.directory_from_self_checker(use_),
            self.config_from_self_checker(use_),
        ] {
            checker.validate("used")?;
        }

        if val!(&use_.from) == Some(&UseFromRef::Debug) && val!(&use_.protocol).is_none() {
            return Err(Error::validate_context(
                "only \"protocol\" supports source from \"debug\"",
                use_.from.as_ref().map(|s| s.origin.clone()),
            ));
        }

        if use_.event_stream.is_some() {
            if let Some(avail) = &use_.availability {
                return Err(Error::validate_context(
                    "\"availability\" cannot be used with \"event_stream\"",
                    Some(avail.origin.clone()),
                ));
            }
            if val!(&use_.from) == Some(&UseFromRef::Self_) {
                return Err(Error::validate_context(
                    "\"from: self\" cannot be used with \"event_stream\"",
                    use_.from.as_ref().map(|s| s.origin.clone()),
                ));
            }
        } else {
            // event_stream is NONE.
            if let Some(filter) = &use_.filter {
                return Err(Error::validate_context(
                    "\"filter\" can only be used with \"event_stream\"",
                    Some(filter.origin.clone()),
                ));
            }
        }

        if use_.storage.is_some() {
            if let Some(from) = &use_.from {
                return Err(Error::validate_context(
                    "\"from\" cannot be used with \"storage\"",
                    Some(from.origin.clone()),
                ));
            }
        }

        if use_.runner.is_some() {
            if let Some(avail) = &use_.availability {
                return Err(Error::validate_context(
                    "\"availability\" cannot be used with \"runner\"",
                    Some(avail.origin.clone()),
                ));
            }
            if val!(&use_.from) == Some(&UseFromRef::Self_) {
                return Err(Error::validate_context(
                    "\"from: self\" cannot be used with \"runner\"",
                    use_.from.as_ref().map(|s| s.origin.clone()),
                ));
            }
        }

        if let Some(avail) = &use_.availability {
            if avail.value == Availability::SameAsTarget {
                return Err(Error::validate_context(
                    "\"availability: same_as_target\" cannot be used with use declarations",
                    Some(avail.origin.clone()),
                ));
            }
        }

        if use_.dictionary.is_some() {
            self.features.check(Feature::UseDictionaries)?;
        }
        if let Some(ContextSpanned { value: UseFromRef::Dictionary(_), origin: _ }) =
            use_.from.as_ref()
        {
            if let Some(storage) = &use_.storage {
                return Err(Error::validate_context(
                    "Dictionaries do not support \"storage\" capabilities",
                    Some(storage.origin.clone()),
                ));
            }
            if let Some(event_stream) = &use_.event_stream {
                return Err(Error::validate_context(
                    "Dictionaries do not support \"event_stream\" capabilities",
                    Some(event_stream.origin.clone()),
                ));
            }
        }

        if let Some(config) = &use_.config {
            if use_.key.is_none() {
                return Err(Error::validate_context(
                    format!("Config '{}' missing field 'key'", config.value),
                    Some(config.origin.clone()),
                ));
            }
            let _ = use_config_to_value_type_context(use_)?;

            let availability = val!(&use_.availability).cloned().unwrap_or(Availability::Required);
            if availability == Availability::Required {
                if let Some(default) = &use_.config_default {
                    return Err(Error::validate_context(
                        format!("Config '{}' is required and has a default value", config.value),
                        Some(default.origin.clone()),
                    ));
                }
            }
        }

        if let Some(handle) = &use_.numbered_handle {
            if use_.protocol.is_some() {
                if let Some(path) = &use_.path {
                    return Err(Error::validate_context(
                        "`path` and `numbered_handle` are incompatible",
                        Some(path.origin.clone()),
                    ));
                }
            } else {
                return Err(Error::validate_context(
                    "`numbered_handle` is only supported for `use protocol`",
                    Some(handle.origin.clone()),
                ));
            }
        }

        let capability_ids_with_origins = CapabilityId::from_context_use(use_wrapper)?;
        for (capability_id, origin) in capability_ids_with_origins {
            if let Some((conflicting_id, conflicting_origin)) =
                used_ids.insert(capability_id.to_string(), (capability_id.clone(), origin.clone()))
            {
                if !matches!(
                    (&capability_id, &conflicting_id),
                    (CapabilityId::UsedDictionary(_), CapabilityId::UsedDictionary(_))
                ) {
                    return Err(Error::validate_contexts(
                        format!(
                            "\"{}\" is a duplicate \"use\" target {}",
                            capability_id,
                            capability_id.type_str()
                        ),
                        vec![origin, conflicting_origin],
                    ));
                }
            }

            let dir = capability_id.get_dir_path();

            // Capability paths must not conflict with `/pkg`, or namespace generation might fail
            let pkg_path = cm_types::NamespacePath::new("/pkg").unwrap();
            if let Some(ref dir) = dir {
                if dir.has_prefix(&pkg_path) {
                    return Err(Error::validate_context(
                        format!(
                            "{} \"{}\" conflicts with the protected path \"/pkg\", please use this capability with a different path",
                            capability_id.type_str(),
                            capability_id,
                        ),
                        Some(origin),
                    ));
                }
            }

            // Validate that paths-based capabilities (service, directory, protocol)
            // are not prefixes of each other.
            for (_, (used_id, origin)) in used_ids.iter() {
                if capability_id == *used_id {
                    continue;
                }
                let Some(ref path_b) = capability_id.get_target_path() else {
                    continue;
                };
                let Some(path_a) = used_id.get_target_path() else {
                    continue;
                };
                #[derive(Debug, Clone, Copy)]
                enum NodeType {
                    Service,
                    Directory,
                    // This variant is never constructed if we're at an API version before "use
                    // dictionary" was added.
                    #[allow(unused)]
                    Dictionary,
                }
                fn capability_id_to_type(id: &CapabilityId<'_>) -> Option<NodeType> {
                    match id {
                        CapabilityId::UsedConfiguration(_) => None,
                        #[cfg(fuchsia_api_level_at_least = "30")]
                        CapabilityId::UsedDictionary(_) => Some(NodeType::Dictionary),
                        CapabilityId::UsedDirectory(_) => Some(NodeType::Directory),
                        CapabilityId::UsedEventStream(_) => Some(NodeType::Service),
                        CapabilityId::UsedProtocol(_) => Some(NodeType::Service),
                        #[cfg(fuchsia_api_level_at_least = "HEAD")]
                        CapabilityId::UsedRunner(_) => None,
                        CapabilityId::UsedService(_) => Some(NodeType::Directory),
                        CapabilityId::UsedStorage(_) => Some(NodeType::Directory),
                        _ => None,
                    }
                }
                let Some(type_a) = capability_id_to_type(&used_id) else {
                    continue;
                };
                let Some(type_b) = capability_id_to_type(&capability_id) else {
                    continue;
                };
                let mut conflicts = false;
                match (type_a, type_b) {
                    (NodeType::Service, NodeType::Service)
                    | (NodeType::Directory, NodeType::Service)
                    | (NodeType::Service, NodeType::Directory)
                    | (NodeType::Directory, NodeType::Directory) => {
                        if path_a.has_prefix(&path_b) || path_b.has_prefix(&path_a) {
                            conflicts = true;
                        }
                    }
                    (NodeType::Dictionary, NodeType::Service)
                    | (NodeType::Dictionary, NodeType::Directory) => {
                        if path_a.has_prefix(&path_b) {
                            conflicts = true;
                        }
                    }
                    (NodeType::Service, NodeType::Dictionary)
                    | (NodeType::Directory, NodeType::Dictionary) => {
                        if path_b.has_prefix(&path_a) {
                            conflicts = true;
                        }
                    }
                    (NodeType::Dictionary, NodeType::Dictionary) => {
                        // All combinations of two dictionaries are valid.
                    }
                }
                if conflicts {
                    return Err(Error::validate_contexts(
                        format!(
                            "{} \"{}\" is a prefix of \"use\" target {} \"{}\"",
                            used_id.type_str(),
                            used_id,
                            capability_id.type_str(),
                            capability_id,
                        ),
                        vec![origin.clone()],
                    ));
                }
            }
        }

        if let Some(dir) = &use_.directory {
            match &use_.rights {
                Some(rights) => {
                    self.validate_directory_rights(&rights.value, Some(&rights.origin))?
                }
                None => {
                    return Err(Error::validate_contexts(
                        "This use statement requires a `rights` field. Refer to: https://fuchsia.dev/go/components/directory#consumer.",
                        vec![dir.origin.clone()],
                    ));
                }
            };
        }

        match (&use_.from, &use_.dependency) {
            (Some(ContextSpanned { value: UseFromRef::Named(name), origin }), _)
                if use_.service.is_some() =>
            {
                self.validate_component_child_or_collection_ref(
                    "\"use\" source",
                    &AnyRef::Named(&name.clone()),
                    Some(origin),
                )?;
            }
            (Some(ContextSpanned { value: UseFromRef::Named(name), origin }), _) => {
                self.validate_component_child_or_capability_ref(
                    "\"use\" source",
                    &AnyRef::Named(&name.clone()),
                    Some(origin),
                )?;
            }
            (
                Some(ContextSpanned {
                    value:
                        UseFromRef::Dictionary(DictionaryRef {
                            path: _,
                            root: RootDictionaryRef::Named(name),
                        }),
                    origin,
                }),
                _,
            ) => {
                self.validate_component_child_or_capability_ref(
                    "\"use\" source",
                    &AnyRef::Named(&name.clone()),
                    Some(origin),
                )?;
            }
            (_, Some(ContextSpanned { value: DependencyType::Weak, origin })) => {
                return Err(Error::validate_context(
                    format!("Only `use` from children can have dependency: \"weak\""),
                    Some(origin.clone()),
                ));
            }
            _ => {}
        }

        Ok(())
    }

    fn validate_expose(
        &self,
        expose_wrapper: &'a ContextSpanned<ContextExpose>,
        used_ids: &mut HashMap<String, Arc<std::path::Path>>,
        exposed_to_framework_ids: &mut HashMap<String, Arc<std::path::Path>>,
    ) -> Result<(), Error> {
        let expose = &expose_wrapper.value;

        expose.capability_type(Some(expose_wrapper.origin.clone()))?;

        for checker in [
            self.service_from_self_checker(expose),
            self.protocol_from_self_checker(expose),
            self.directory_from_self_checker(expose),
            self.runner_from_self_checker(expose),
            self.resolver_from_self_checker(expose),
            self.dictionary_from_self_checker(expose),
            self.config_from_self_checker(expose),
        ] {
            checker.validate("exposed")?;
        }

        // Ensure directory rights are valid.
        if let Some(_) = expose.directory.as_ref() {
            if expose.from.value.iter().any(|r| *r == ExposeFromRef::Self_)
                || expose.rights.is_some()
            {
                if let Some(rights) = expose.rights.as_ref() {
                    self.validate_directory_rights(&rights.value, Some(&rights.origin))?;
                }
            }

            // Exposing a subdirectory makes sense for routing but when exposing to framework,
            // the subdir should be exposed directly.
            if let Some(e2) = &expose.to {
                if e2.value == ExposeToRef::Framework
                    && let Some(expose_subdir) = &expose.subdir
                {
                    return Err(Error::validate_context(
                        "`subdir` is not supported for expose to framework. Directly expose the subdirectory instead.",
                        Some(expose_subdir.origin.clone()),
                    ));
                }
            }
        }

        if let Some(event_stream) = &expose.event_stream {
            if event_stream.value.iter().len() > 1 && expose.r#as.is_some() {
                return Err(Error::validate_context(
                    format!("as cannot be used with multiple event streams"),
                    Some(expose.r#as.clone().unwrap().origin),
                ));
            }
            if let Some(e2) = &expose.to
                && e2.value == ExposeToRef::Framework
            {
                return Err(Error::validate_context(
                    format!("cannot expose an event_stream to framework"),
                    Some(event_stream.origin.clone()),
                ));
            }
            for from in expose.from.value.iter() {
                if from == &ExposeFromRef::Self_ {
                    return Err(Error::validate_context(
                        format!("Cannot expose event_streams from self"),
                        Some(event_stream.origin.clone()),
                    ));
                }
            }
            if let Some(scopes) = &expose.scope {
                for scope in &scopes.value {
                    match scope {
                        EventScope::Named(name) => {
                            if !self.all_children.contains_key::<BorrowedName>(name)
                                && !self.all_collections.contains::<BorrowedName>(name)
                            {
                                return Err(Error::validate_context(
                                    format!(
                                        "event_stream scope {} did not match a component or collection in this .cml file.",
                                        name.as_str()
                                    ),
                                    Some(scopes.origin.clone()),
                                ));
                            }
                        }
                    }
                }
            }
        }

        for ref_ in expose.from.value.iter() {
            if let ExposeFromRef::Dictionary(d) = ref_ {
                if expose.event_stream.is_some() {
                    return Err(Error::validate_context(
                        "Dictionaries do not support \"event_stream\" capabilities",
                        Some(expose.event_stream.clone().unwrap().origin),
                    ));
                }
                match &d.root {
                    RootDictionaryRef::Self_ | RootDictionaryRef::Named(_) => {}
                    RootDictionaryRef::Parent => {
                        return Err(Error::validate_context(
                            "`expose` dictionary path must begin with `self` or `#<child-name>`",
                            Some(expose.from.origin.clone()),
                        ));
                    }
                }
            }
        }

        // Ensure we haven't already exposed an entity of the same name.
        let target_cap_ids_with_origin = CapabilityId::from_context_offer_expose(expose_wrapper)?;
        for (capability_id, cap_origin) in target_cap_ids_with_origin {
            let mut ids = &mut *used_ids;
            if let Some(e2) = &expose.to
                && e2.value == ExposeToRef::Framework
            {
                ids = &mut *exposed_to_framework_ids;
            }
            if let Some(conflict_origin) = ids.insert(capability_id.to_string(), cap_origin.clone())
            {
                if let CapabilityId::Service(_) = capability_id {
                    // Services may have duplicates (aggregation).
                } else {
                    let expose_print = match &expose.to {
                        Some(expose_to) => &expose_to.value,
                        None => &ExposeToRef::Parent,
                    };
                    return Err(Error::validate_contexts(
                        format!(
                            "\"{}\" is a duplicate \"expose\" target capability for \"{}\"",
                            capability_id, expose_print
                        ),
                        vec![cap_origin, conflict_origin],
                    ));
                }
            }
        }

        // Validate `from` (done last because this validation depends on the capability type, which
        // must be validated first)
        self.validate_from_clause(
            "expose",
            expose,
            &expose.source_availability.as_ref().map(|s| s.value.clone()),
            &expose.availability.as_ref().map(|s| s.value.clone()),
            expose.from.origin.clone(),
        )?;

        Ok(())
    }

    fn validate_required_offer_decls(&self) -> Result<(), Error> {
        let children = self.document.children.as_ref().map(|c| c.as_slice()).unwrap_or(&[]);
        let collections = self.document.collections.as_ref().map(|c| c.as_slice()).unwrap_or(&[]);
        let offers = self.document.offer.as_ref().map(|o| o.as_slice()).unwrap_or(&[]);

        for required_offer in self.capability_requirements.must_offer {
            for child in children {
                if !offers.iter().any(|offer| {
                    Self::has_required_offer(&offer.value, &child.value.name.value, required_offer)
                }) {
                    let capability_type = required_offer.offer_type();
                    return Err(Error::validate_context(
                        format!(
                            r#"{capability_type} "{}" is not offered to child component "{}" but it is a required offer"#,
                            required_offer.name(),
                            child.value.name.value
                        ),
                        Some(child.origin.clone()),
                    ));
                }
            }

            for collection in collections {
                if !offers.iter().any(|offer| {
                    Self::has_required_offer(
                        &offer.value,
                        &collection.value.name.value,
                        required_offer,
                    )
                }) {
                    let capability_type = required_offer.offer_type();
                    return Err(Error::validate_context(
                        format!(
                            r#"{capability_type} "{}" is not offered to collection "{}" but it is a required offer"#,
                            required_offer.name(),
                            collection.value.name.value
                        ),
                        Some(collection.origin.clone()),
                    ));
                }
            }
        }
        Ok(())
    }

    fn has_required_offer(
        offer: &ContextOffer,
        target_name: &BorrowedName,
        required_offer: &OfferToAllCapability<'_>,
    ) -> bool {
        let names_this_collection = offer.to.value.iter().any(|target| match target {
            OfferToRef::Named(name) => name.as_str() == target_name.as_str(),
            OfferToRef::All => true,
            OfferToRef::OwnDictionary(_) => false,
        });

        let capability_names = match required_offer {
            OfferToAllCapability::Dictionary(_) => offer.dictionary.as_ref(),
            OfferToAllCapability::Protocol(_) => offer.protocol.as_ref(),
        };

        let names_this_capability = match capability_names {
            Some(spanned_names) => match &spanned_names.value {
                OneOrMany::Many(names) => {
                    names.iter().any(|cap_name| cap_name.as_str() == required_offer.name())
                }
                OneOrMany::One(name) => {
                    let cap_name = offer.r#as.as_ref().map(|s| &s.value).unwrap_or(name);

                    cap_name.as_str() == required_offer.name()
                }
            },
            None => false,
        };

        names_this_collection && names_this_capability
    }

    fn validate_required_use_decls(&self) -> Result<(), Error> {
        let use_decls = self.document.r#use.as_ref().map(|u| u.as_slice()).unwrap_or(&[]);

        for required_usage in self.capability_requirements.must_use {
            if !use_decls.iter().any(|usage| match usage.value.protocol().as_ref() {
                None => false,
                Some(protocol) => protocol
                    .value
                    .iter()
                    .any(|protocol_name| protocol_name.as_str() == required_usage.name()),
            }) {
                return Err(Error::validate(format!(
                    r#"Protocol "{}" is not used by a component but is required by all"#,
                    required_usage.name(),
                )));
            }
        }
        Ok(())
    }

    fn validate_offer(
        &mut self,
        offer_wrapper: &'a ContextSpanned<ContextOffer>,
        used_ids: &mut HashMap<Name, HashMap<String, Arc<std::path::Path>>>,
        protocols_offered_to_all: &[&'a ContextSpanned<ContextOffer>],
    ) -> Result<(), Error> {
        let offer = &offer_wrapper.value;
        offer.capability_type(Some(offer_wrapper.origin.clone()))?;

        for checker in [
            self.service_from_self_checker(offer),
            self.protocol_from_self_checker(offer),
            self.directory_from_self_checker(offer),
            self.storage_from_self_checker(offer),
            self.runner_from_self_checker(offer),
            self.resolver_from_self_checker(offer),
            self.dictionary_from_self_checker(offer),
            self.config_from_self_checker(offer),
        ] {
            checker.validate("offered")?;
        }

        let from_wrapper = &offer.from;
        let from_one_or_many = &from_wrapper.value;
        let from_self = self.from_self(from_one_or_many);

        if let Some(stream_span) = offer.event_stream.as_ref() {
            if stream_span.value.iter().len() > 1 {
                if let Some(as_span) = offer.r#as.as_ref() {
                    return Err(Error::validate_context(
                        "as cannot be used with multiple events",
                        Some(as_span.origin.clone()),
                    ));
                }
            }

            if from_self {
                return Err(Error::validate_context(
                    "cannot offer an event_stream from self",
                    Some(from_wrapper.origin.clone()),
                ));
            }
        }

        if offer.directory.as_ref().is_some() {
            if from_self || offer.rights.is_some() {
                if let Some(rights_span) = offer.rights.as_ref() {
                    self.validate_directory_rights(&rights_span.value, Some(&rights_span.origin))?;
                }
            }
        }

        if let Some(storages) = offer.storage.as_ref() {
            for storage in &storages.value {
                if offer.from.value.iter().any(|r| r.is_named()) {
                    return Err(Error::validate_contexts(
                        format!(
                            "Storage \"{}\" is offered from a child, but storage capabilities cannot be exposed",
                            storage
                        ),
                        vec![storages.origin.clone()],
                    ));
                }
            }
        }

        for from_ref in offer.from.value.iter() {
            if let OfferFromRef::Dictionary(_) = &from_ref {
                if let Some(storage_span) = offer.storage.as_ref() {
                    return Err(Error::validate_context(
                        "Dictionaries do not support \"storage\" capabilities",
                        Some(storage_span.origin.clone()),
                    ));
                }
                if let Some(stream_span) = offer.event_stream.as_ref() {
                    return Err(Error::validate_context(
                        "Dictionaries do not support \"event_stream\" capabilities",
                        Some(stream_span.origin.clone()),
                    ));
                }
            }
        }

        if !offer_can_have_dependency(offer) {
            if let Some(dep_span) = offer.dependency.as_ref() {
                return Err(Error::validate_context(
                    "Dependency can only be provided for protocol, directory, and service capabilities",
                    Some(dep_span.origin.clone()),
                ));
            }
        }

        let target_cap_ids_with_origin = CapabilityId::from_context_offer_expose(offer_wrapper)?;

        let to_wrapper = &offer.to;

        let to_field_origin = &to_wrapper.origin;
        let to_targets = &to_wrapper.value;

        for target_ref in to_targets.iter() {
            let to_target = match target_ref {
                OfferToRef::All => continue,
                OfferToRef::Named(to_target) => {
                    // Verify that only a legal set of offers-to-all are made, including that any
                    // offer to all duplicated as an offer to a specific component are exactly the same
                    for offer_to_all in protocols_offered_to_all {
                        offer_to_all_would_duplicate_context(
                            offer_to_all,
                            offer_wrapper,
                            &to_target,
                        )?;
                    }

                    let target_availability_is_unknown = offer
                        .target_availability
                        .as_ref()
                        .map_or(false, |a| a.value == TargetAvailability::Unknown);

                    // Check that any referenced child actually exists.
                    if self.all_children.contains_key::<BorrowedName>(to_target.as_ref())
                        || self.all_collections.contains::<BorrowedName>(to_target.as_ref())
                        || target_availability_is_unknown
                    {
                        // Allowed.
                    } else {
                        if let OneOrMany::One(from) = &offer.from.value {
                            return Err(Error::validate_context(
                                format!(
                                    "\"{target_ref}\" is an \"offer\" target from \"{from}\" but \"{target_ref}\" does \
                            not appear in \"children\" or \"collections\"",
                                ),
                                Some(to_field_origin.clone()),
                            ));
                        } else {
                            return Err(Error::validate_context(
                                format!(
                                    "\"{target_ref}\" is an \"offer\" target but \"{target_ref}\" does not appear in \
                            \"children\" or \"collections\"",
                                ),
                                Some(to_field_origin.clone()),
                            ));
                        }
                    }

                    // Ensure we are not offering a capability back to its source.
                    if let Some(storage) = offer.storage.as_ref() {
                        for storage in &storage.value {
                            // Storage can only have a single `from` clause and this has been
                            // verified.
                            if let OneOrMany::One(OfferFromRef::Self_) = &offer.from.value {
                                if let Some(CapabilityFromRef::Named(source)) =
                                    self.all_storages.get::<BorrowedName>(storage.as_ref())
                                {
                                    if *to_target == *source {
                                        return Err(Error::validate_context(
                                            format!(
                                                "Storage offer target \"{}\" is same as source",
                                                target_ref
                                            ),
                                            Some(to_field_origin.clone()),
                                        ));
                                    }
                                }
                            }
                        }
                    } else {
                        for reference in offer.from.value.iter() {
                            // Weak offers from a child to itself are acceptable.
                            if offer_dependency(offer) == DependencyType::Weak {
                                continue;
                            }
                            match reference {
                                OfferFromRef::Named(name) if name == to_target => {
                                    return Err(Error::validate_context(
                                        format!(
                                            "Offer target \"{}\" is same as source",
                                            target_ref
                                        ),
                                        Some(to_field_origin.clone()),
                                    ));
                                }
                                _ => {}
                            }
                        }
                    }
                    to_target
                }
                OfferToRef::OwnDictionary(to_target) => {
                    let r2 = CapabilityId::from_context_offer_expose(offer_wrapper)?;
                    for (id, cap_origin) in r2 {
                        match &id {
                            CapabilityId::Protocol(_)
                            | CapabilityId::Dictionary(_)
                            | CapabilityId::Directory(_)
                            | CapabilityId::Runner(_)
                            | CapabilityId::Resolver(_)
                            | CapabilityId::Service(_)
                            | CapabilityId::Configuration(_) => {}
                            CapabilityId::Storage(_) | CapabilityId::EventStream(_) => {
                                let type_name = id.type_str();
                                return Err(Error::validate_context(
                                    format!(
                                        "\"offer\" to dictionary \"{target_ref}\" for \"{type_name}\" but \
                                    dictionaries do not support this type yet."
                                    ),
                                    Some(cap_origin),
                                ));
                            }
                            CapabilityId::UsedService(_)
                            | CapabilityId::UsedProtocol(_)
                            | CapabilityId::UsedProtocolNumberedHandle(_)
                            | CapabilityId::UsedDirectory(_)
                            | CapabilityId::UsedStorage(_)
                            | CapabilityId::UsedEventStream(_)
                            | CapabilityId::UsedRunner(_)
                            | CapabilityId::UsedConfiguration(_)
                            | CapabilityId::UsedDictionary(_) => {
                                unreachable!("this is not a use")
                            }
                        }
                    }
                    // Check that any referenced child actually exists.
                    let target_availability_is_unknown = offer
                        .target_availability
                        .as_ref()
                        .map_or(false, |a| a.value == TargetAvailability::Unknown);

                    // Check that any referenced dictionary actually exists.
                    if let Some(d) = self.all_dictionaries.get::<BorrowedName>(to_target.as_ref()) {
                        // If it exists, verify it's not dynamic
                        if d.path.is_some() {
                            return Err(Error::validate_context(
                                format!(
                                    "\"offer\" has dictionary target \"{target_ref}\" but \"{to_target}\" \
                                sets \"path\". Therefore, it is a dynamic dictionary that \
                                does not allow offers into it."
                                ),
                                Some(to_field_origin.clone()),
                            ));
                        }
                    } else if !target_availability_is_unknown {
                        // If it doesn't exist, ONLY error if availability is NOT unknown
                        return Err(Error::validate_context(
                            format!(
                                "\"offer\" has dictionary target \"{target_ref}\" but \"{to_target}\" \
                                is not a dictionary capability defined by this component"
                            ),
                            Some(to_field_origin.clone()),
                        ));
                    }
                    to_target
                }
            };

            // Ensure that a target is not offered more than once.
            let ids_for_entity = used_ids.entry(to_target.clone()).or_insert(HashMap::new());
            for (target_cap_id, target_origin) in &target_cap_ids_with_origin {
                let key = target_cap_id.to_string();
                if let Some(existing_origin) = ids_for_entity.get(&key) {
                    if !matches!(target_cap_id, CapabilityId::Service(_)) {
                        return Err(Error::validate_contexts(
                            format!(
                                "\"{}\" is a duplicate \"offer\" target capability for \"{}\"",
                                target_cap_id, target_ref
                            ),
                            vec![target_origin.clone(), existing_origin.clone()],
                        ));
                    }
                }

                ids_for_entity.insert(key, target_origin.clone());
            }
        }

        self.validate_from_clause(
            "offer",
            offer,
            &offer.source_availability.as_ref().map(|s| s.value.clone()),
            &offer.availability.as_ref().map(|s| s.value.clone()),
            offer.from.origin.clone(),
        )?;

        Ok(())
    }

    fn validate_collection(
        &mut self,
        collection: &'a ContextSpanned<ContextCollection>,
    ) -> Result<(), Error> {
        if collection.value.allow_long_names.is_some() {
            self.features.check(Feature::AllowLongNames)?;
        }
        Ok(())
    }

    fn validate_environment(
        &mut self,
        environment_wrapper: &'a ContextSpanned<ContextEnvironment>,
    ) -> Result<(), Error> {
        let environment = &environment_wrapper.value;

        if let Some(extends_span) = &environment.extends {
            if extends_span.value == EnvironmentExtends::None {
                if environment.stop_timeout_ms.is_none() {
                    return Err(Error::validate_context(
                        "'__stop_timeout_ms' must be provided if the environment extends 'none'",
                        Some(extends_span.origin.clone()),
                    ));
                }
            }
        }

        if let Some(runners) = &environment.runners {
            let mut used_names = HashMap::new();
            for registration_span in runners {
                let reg = &registration_span.value;

                let (target_name, name_origin) = if let Some(as_span) = &reg.r#as {
                    (&as_span.value, &as_span.origin)
                } else {
                    (&reg.runner.value, &reg.runner.origin)
                };

                if let Some((prev_runner, prev_origin)) =
                    used_names.insert(target_name, (&reg.runner.value, name_origin))
                {
                    return Err(Error::validate_contexts(
                        format!(
                            "Duplicate runners registered under name \"{}\": \"{}\" and \"{}\".",
                            target_name, reg.runner.value, prev_runner
                        ),
                        vec![prev_origin.clone(), name_origin.clone()],
                    ));
                }

                // Ensure runner exists if source is 'self'
                let runner_ref: Name = reg.runner.value.clone();
                if reg.from.value == RegistrationRef::Self_
                    && !self.all_runners.contains(&runner_ref)
                {
                    return Err(Error::validate_context(
                        format!(
                            "Runner \"{}\" is not defined in the root \"runners\" section",
                            reg.runner.value
                        ),
                        Some(reg.runner.origin.clone()),
                    ));
                }

                self.validate_component_child_ref(
                    &format!("\"{}\" runner source", reg.runner.value),
                    &AnyRef::from(&reg.from.value),
                    Some(&reg.from.origin),
                )?;
            }
        }

        if let Some(resolvers) = &environment.resolvers {
            let mut used_schemes = HashMap::new();
            for registration_span in resolvers {
                let reg = &registration_span.value;

                if let Some((prev_resolver, prev_origin)) = used_schemes
                    .insert(&reg.scheme.value, (&reg.resolver.value, &reg.scheme.origin))
                {
                    return Err(Error::validate_contexts(
                        format!(
                            "scheme \"{}\" for resolver \"{}\" is already registered to \"{}\".",
                            reg.scheme.value, reg.resolver.value, prev_resolver
                        ),
                        vec![prev_origin.clone(), reg.scheme.origin.clone()],
                    ));
                }

                self.validate_component_child_ref(
                    &format!("\"{}\" resolver source", reg.resolver.value),
                    &AnyRef::from(&reg.from.value),
                    Some(&reg.from.origin),
                )?;
            }
        }

        if let Some(debug_capabilities) = &environment.debug {
            for debug_span in debug_capabilities {
                let debug = &debug_span.value;
                self.protocol_from_self_checker(debug).validate("registered as debug")?;
                self.validate_from_clause("debug", debug, &None, &None, debug.from.origin.clone())?;
            }
        }

        Ok(())
    }

    fn get_test_facet(&self) -> Option<&ContextSpanned<serde_json::Value>> {
        match &self.document.facets {
            Some(m) => m.get(TEST_FACET_KEY),
            None => None,
        }
    }

    fn validate_facets(&self) -> Result<(), Error> {
        let test_facet_spanned = self.get_test_facet();
        let enable_allow_non_hermetic_packages =
            self.features.has(&Feature::EnableAllowNonHermeticPackagesFeature);

        if let Some(spanned) = test_facet_spanned {
            let test_facet_origin = &spanned.origin;
            let test_facet_map = match &spanned.value {
                serde_json::Value::Object(m) => m,
                _ => {
                    return Err(Error::validate_context(
                        format!("'{}' is not an object", TEST_FACET_KEY),
                        Some(test_facet_origin.clone()),
                    ));
                }
            };

            let restrict_test_type = self.features.has(&Feature::RestrictTestTypeInFacet);

            if restrict_test_type {
                if test_facet_map.contains_key(TEST_TYPE_FACET_KEY) {
                    return Err(Error::validate_context(
                        format!(
                            "'{}' is not allowed in facets. Refer to: \
                            https://fuchsia.dev/fuchsia-src/development/testing/components/test_runner_framework#non-hermetic_tests",
                            TEST_TYPE_FACET_KEY
                        ),
                        Some(test_facet_origin.clone()),
                    ));
                }
            }
        }

        if enable_allow_non_hermetic_packages {
            let allow_non_hermetic_packages = self.features.has(&Feature::AllowNonHermeticPackages);

            let has_deprecated_facet = test_facet_spanned
                .and_then(|s| s.value.as_object())
                .map_or(false, |m| m.contains_key(TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY));

            if allow_non_hermetic_packages && !has_deprecated_facet {
                return Err(Error::validate(format!(
                    "Remove restricted_feature '{}' as manifest does not contain facet '{}'",
                    Feature::AllowNonHermeticPackages,
                    TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY
                )));
            }

            if has_deprecated_facet && !allow_non_hermetic_packages {
                let origin = test_facet_spanned.map(|s| s.origin.clone());

                return Err(Error::validate_context(
                    format!(
                        "restricted_feature '{}' should be present with facet '{}'",
                        Feature::AllowNonHermeticPackages,
                        TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY
                    ),
                    origin,
                ));
            }
        }

        Ok(())
    }

    fn validate_config(&self) -> Result<(), Error> {
        let fields = self.document.config.as_ref();

        let optional_use_keys: BTreeMap<ConfigKey, ContextSpanned<ConfigValueType>> = self
            .document
            .r#use
            .iter()
            .flatten()
            .filter_map(|u_spanned| {
                let u = &u_spanned.value;
                if u.config.is_none() {
                    return None;
                }

                let avail = u.availability.as_ref().map(|s| &s.value);
                if avail == Some(&Availability::Required) || avail.is_none() {
                    return None;
                }

                if u.config_default.is_some() {
                    return None;
                }

                let key =
                    ConfigKey(u.key.as_ref().expect("key should be set").value.clone().into());
                let value_type =
                    use_config_to_value_type_context(u).expect("config type should be valid");

                Some((key, ContextSpanned { value: value_type, origin: u_spanned.origin.clone() }))
            })
            .collect();

        let Some(fields_map) = fields else {
            if !optional_use_keys.is_empty() {
                return Err(Error::validate(
                    "Optionally using a config capability without a default requires a matching 'config' section.",
                ));
            }
            return Ok(());
        };

        if fields_map.is_empty() {
            return Err(Error::validate("'config' section is empty"));
        }

        for (key, use_spanned) in optional_use_keys {
            match fields_map.get(&key) {
                None => {
                    return Err(Error::validate_context(
                        format!("'config' section must contain key for optional use '{}'", key),
                        Some(use_spanned.origin),
                    ));
                }
                Some(config_spanned) => {
                    if config_spanned.value != use_spanned.value {
                        return Err(Error::validate_contexts(
                            format!("Use and config block differ on type for key '{}'", key),
                            vec![use_spanned.origin, config_spanned.origin.clone()],
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    fn validate_runner_specified(
        &self,
        program: Option<&ContextSpanned<ContextProgram>>,
    ) -> Result<(), Error> {
        if let Some(p) = program {
            if p.value.runner.is_none() {
                return Err(Error::validate_context(
                    "Component has a `program` block defined, but doesn't specify a `runner`. \
                    Components need to use a runner to actually execute code.",
                    Some(p.origin.clone()),
                ));
            }
        }
        Ok(())
    }

    fn validate_runner_not_specified(
        &self,
        program: Option<&ContextSpanned<ContextProgram>>,
    ) -> Result<(), Error> {
        if let Some(p) = program {
            if p.value.runner.is_some() {
                return Err(Error::validate_context(
                    "Component has conflicting runners in `program` block and `use` block.",
                    Some(p.origin.clone()),
                ));
            }
        }
        Ok(())
    }

    /// Validates that directory rights for all route types are valid, i.e that it does not
    /// contain duplicate rights.
    fn validate_directory_rights(
        &self,
        rights_clause: &Rights,
        origin: Option<&Arc<std::path::Path>>,
    ) -> Result<(), Error> {
        let mut rights = HashSet::new();
        for right_token in rights_clause.0.iter() {
            for right in right_token.expand() {
                if !rights.insert(right) {
                    return Err(Error::validate_context(
                        format!("\"{}\" is duplicated in the rights clause.", right_token),
                        origin.cloned(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn from_self(&self, from_one_or_many: &OneOrMany<OfferFromRef>) -> bool {
        for from_ref in from_one_or_many.iter() {
            match from_ref {
                OfferFromRef::Self_ => return true,
                _ => {}
            }
        }
        false
    }

    /// Validates that the from clause:
    ///
    /// - is applicable to the capability type,
    /// - does not contain duplicates,
    /// - references names that exist.
    /// - has availability "optional" if the source is "void"
    ///
    /// `verb` is used in any error messages and is expected to be "offer", "expose", etc.
    fn validate_from_clause<T>(
        &self,
        verb: &str,
        cap: &T,
        source_availability: &Option<SourceAvailability>,
        availability: &Option<Availability>,
        origin: Arc<std::path::Path>,
    ) -> Result<(), Error>
    where
        T: ContextCapabilityClause + FromClauseContext,
    {
        let from = cap.from_();
        if cap.service().is_none() && from.value.is_many() {
            return Err(Error::validate_context(
                format!(
                    "\"{}\" capabilities cannot have multiple \"from\" clauses",
                    cap.capability_type(None).unwrap()
                ),
                Some(origin),
            ));
        }

        let from_val = &from.value;

        if from_val.is_many() {
            ensure_no_duplicate_values(from_val.iter())?;
        }

        let reference_description = format!("\"{}\" source", verb);
        for from_clause in from_val {
            // If this is a protocol, it could reference either a child or a storage capability
            // (for the storage admin protocol).
            let ref_validity_res = if cap.protocol().is_some() {
                self.validate_component_child_or_capability_ref(
                    &reference_description,
                    &from_clause,
                    Some(&origin),
                )
            } else if cap.service().is_some() {
                // Services can also be sourced from collections.
                self.validate_component_child_or_collection_ref(
                    &reference_description,
                    &from_clause,
                    Some(&origin),
                )
            } else {
                self.validate_component_child_ref(
                    &reference_description,
                    &from_clause,
                    Some(&origin),
                )
            };

            match ref_validity_res {
                Ok(()) if *from_clause == AnyRef::Void => {
                    // The source is valid and void
                    if availability != &Some(Availability::Optional) {
                        return Err(Error::validate_context(
                            format!(
                                "capabilities with a source of \"void\" must have an availability of \"optional\", capabilities: \"{}\", from: \"{}\"",
                                cap.names()
                                    .iter()
                                    .map(|n| n.value.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                cap.from_().value,
                            ),
                            Some(origin),
                        ));
                    }
                }
                Ok(()) => {
                    // The source is valid and not void.
                }
                Err(_) if source_availability == &Some(SourceAvailability::Unknown) => {
                    // The source is invalid, and will be rewritten to void
                    if availability != &Some(Availability::Optional) && availability != &None {
                        return Err(Error::validate_context(
                            format!(
                                "capabilities with an intentionally missing source must have an availability that is either unset or \"optional\", capabilities: \"{}\", from: \"{}\"",
                                cap.names()
                                    .iter()
                                    .map(|n| n.value.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                cap.from_().value,
                            ),
                            Some(origin),
                        ));
                    }
                }
                Err(e) => {
                    // The source is invalid, but we're expecting it to be valid.
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Validates that the given component exists.
    ///
    /// - `reference_description` is a human-readable description of the reference used in error
    ///   message, such as `"offer" source`.
    /// - `component_ref` is a reference to a component. If the reference is a named child, we
    ///   ensure that the child component exists.
    fn validate_component_child_ref(
        &self,
        reference_description: &str,
        component_ref: &AnyRef<'_>,
        origin: Option<&Arc<std::path::Path>>,
    ) -> Result<(), Error> {
        match component_ref {
            AnyRef::Named(name) => {
                // Ensure we have a child defined by that name.
                if !self.all_children.contains(*name) {
                    return Err(Error::validate_context(
                        format!(
                            "{} \"{}\" does not appear in \"children\"",
                            reference_description, component_ref
                        ),
                        origin.cloned(),
                    ));
                }
                Ok(())
            }
            // We don't attempt to validate other reference types.
            _ => Ok(()),
        }
    }

    /// Validates that the given component/collection exists.
    ///
    /// - `reference_description` is a human-readable description of the reference used in error
    ///   message, such as `"offer" source`.
    /// - `component_ref` is a reference to a component/collection. If the reference is a named
    ///   child or collection, we ensure that the child component/collection exists.
    fn validate_component_child_or_collection_ref(
        &self,
        reference_description: &str,
        component_ref: &AnyRef<'_>,
        origin: Option<&Arc<std::path::Path>>,
    ) -> Result<(), Error> {
        match component_ref {
            AnyRef::Named(name) => {
                // Ensure we have a child or collection defined by that name.
                if !self.all_children.contains(*name) && !self.all_collections.contains(*name) {
                    return Err(Error::validate_context(
                        format!(
                            "{} \"{}\" does not appear in \"children\" or \"collections\"",
                            reference_description, component_ref
                        ),
                        origin.cloned(),
                    ));
                }
                Ok(())
            }
            // We don't attempt to validate other reference types.
            _ => Ok(()),
        }
    }

    /// Validates that the given capability exists.
    ///
    /// - `reference_description` is a human-readable description of the reference used in error
    ///   message, such as `"offer" source`.
    /// - `capability_ref` is a reference to a capability. If the reference is a named capability,
    ///   we ensure that the capability exists.
    fn validate_component_capability_ref(
        &self,
        reference_description: &str,
        capability_ref: &AnyRef<'_>,
        origin: Option<&Arc<std::path::Path>>,
    ) -> Result<(), Error> {
        match capability_ref {
            AnyRef::Named(name) => {
                if !self.all_capability_names.contains(*name) {
                    return Err(Error::validate_context(
                        format!(
                            "{} \"{}\" does not appear in \"capabilities\"",
                            reference_description, capability_ref
                        ),
                        origin.cloned(),
                    ));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Validates that the given child component, collection, or capability exists.
    ///
    /// - `reference_description` is a human-readable description of the reference used in error
    ///   message, such as `"offer" source`.
    /// - `ref_` is a reference to a child component or capability. If the reference contains a
    ///   name, we ensure that a child component or a capability with the name exists.
    fn validate_component_child_or_capability_ref(
        &self,
        reference_description: &str,
        ref_: &AnyRef<'_>,
        origin: Option<&Arc<std::path::Path>>,
    ) -> Result<(), Error> {
        if self.validate_component_child_ref(reference_description, ref_, origin).is_err()
            && self.validate_component_capability_ref(reference_description, ref_, origin).is_err()
        {
            return Err(Error::validate_context(
                format!(
                    "{} \"{}\" does not appear in \"children\" or \"capabilities\"",
                    reference_description, ref_
                ),
                origin.cloned(),
            ));
        }
        Ok(())
    }

    fn protocol_from_self_checker<'b>(
        &'b self,
        input: &'b (impl ContextCapabilityClause + FromClauseContext),
    ) -> RouteFromSelfCheckerV2<'b> {
        RouteFromSelfCheckerV2 {
            capability_name: input.protocol().map(|spanned| {
                spanned.map(|one_or_many| match one_or_many {
                    OneOrMany::One(name) => OneOrMany::One(AnyRef::from(name)),
                    OneOrMany::Many(names) => {
                        OneOrMany::Many(names.iter().cloned().map(AnyRef::from).collect())
                    }
                })
            }),
            from: input.from_(),
            container: &self.all_protocols,
            all_dictionaries: &self.all_dictionaries,
            typename: "protocol",
        }
    }

    fn resolver_from_self_checker<'b>(
        &'b self,
        input: &'b (impl ContextCapabilityClause + FromClauseContext),
    ) -> RouteFromSelfCheckerV2<'b> {
        RouteFromSelfCheckerV2 {
            capability_name: input.resolver().map(|spanned| {
                spanned.map(|one_or_many| match one_or_many {
                    OneOrMany::One(name) => OneOrMany::One(AnyRef::from(name)),
                    OneOrMany::Many(names) => {
                        OneOrMany::Many(names.iter().cloned().map(AnyRef::from).collect())
                    }
                })
            }),
            from: input.from_(),
            container: &self.all_resolvers,
            all_dictionaries: &self.all_dictionaries,
            typename: "resolver",
        }
    }

    fn runner_from_self_checker<'b>(
        &'b self,
        input: &'b (impl ContextCapabilityClause + FromClauseContext),
    ) -> RouteFromSelfCheckerV2<'b> {
        RouteFromSelfCheckerV2 {
            capability_name: input.runner().map(|spanned| {
                spanned.map(|one_or_many| match one_or_many {
                    OneOrMany::One(name) => OneOrMany::One(AnyRef::from(name)),
                    OneOrMany::Many(names) => {
                        OneOrMany::Many(names.iter().cloned().map(AnyRef::from).collect())
                    }
                })
            }),
            from: input.from_(),
            container: &self.all_runners,
            all_dictionaries: &self.all_dictionaries,
            typename: "runner",
        }
    }

    fn service_from_self_checker<'b>(
        &'b self,
        input: &'b (impl ContextCapabilityClause + FromClauseContext),
    ) -> RouteFromSelfCheckerV2<'b> {
        RouteFromSelfCheckerV2 {
            capability_name: input.service().map(|spanned| {
                spanned.map(|one_or_many| match one_or_many {
                    OneOrMany::One(name) => OneOrMany::One(AnyRef::from(name)),
                    OneOrMany::Many(names) => {
                        OneOrMany::Many(names.iter().cloned().map(AnyRef::from).collect())
                    }
                })
            }),
            from: input.from_(),
            container: &self.all_services,
            all_dictionaries: &self.all_dictionaries,
            typename: "service",
        }
    }

    fn config_from_self_checker<'b>(
        &'b self,
        input: &'b (impl ContextCapabilityClause + FromClauseContext),
    ) -> RouteFromSelfCheckerV2<'b> {
        RouteFromSelfCheckerV2 {
            capability_name: input.config().map(|spanned| {
                spanned.map(|one_or_many| match one_or_many {
                    OneOrMany::One(name) => OneOrMany::One(AnyRef::from(name)),
                    OneOrMany::Many(names) => {
                        OneOrMany::Many(names.iter().cloned().map(AnyRef::from).collect())
                    }
                })
            }),
            from: input.from_(),
            container: &self.all_configs,
            all_dictionaries: &self.all_dictionaries,
            typename: "config",
        }
    }

    fn dictionary_from_self_checker<'b>(
        &'b self,
        input: &'b (impl ContextCapabilityClause + FromClauseContext),
    ) -> RouteFromSelfCheckerV2<'b> {
        RouteFromSelfCheckerV2 {
            capability_name: input.dictionary().map(|spanned| {
                spanned.map(|one_or_many| match one_or_many {
                    OneOrMany::One(name) => OneOrMany::One(AnyRef::from(name)),
                    OneOrMany::Many(names) => {
                        OneOrMany::Many(names.iter().cloned().map(AnyRef::from).collect())
                    }
                })
            }),
            from: input.from_(),
            container: &self.all_dictionaries,
            all_dictionaries: &self.all_dictionaries,
            typename: "dictionary",
        }
    }

    fn directory_from_self_checker<'b>(
        &'b self,
        input: &'b (impl ContextCapabilityClause + FromClauseContext),
    ) -> RouteFromSelfCheckerV2<'b> {
        RouteFromSelfCheckerV2 {
            capability_name: input.directory().map(|spanned| {
                spanned.map(|one_or_many| match one_or_many {
                    OneOrMany::One(name) => OneOrMany::One(AnyRef::from(name)),
                    OneOrMany::Many(names) => {
                        OneOrMany::Many(names.iter().cloned().map(AnyRef::from).collect())
                    }
                })
            }),
            from: input.from_(),
            container: &self.all_directories,
            all_dictionaries: &self.all_dictionaries,
            typename: "directory",
        }
    }

    fn storage_from_self_checker<'b>(
        &'b self,
        input: &'b (impl ContextCapabilityClause + FromClauseContext),
    ) -> RouteFromSelfCheckerV2<'b> {
        RouteFromSelfCheckerV2 {
            capability_name: input.storage().map(|spanned| {
                spanned.map(|one_or_many| match one_or_many {
                    OneOrMany::One(name) => OneOrMany::One(AnyRef::from(name)),
                    OneOrMany::Many(names) => {
                        OneOrMany::Many(names.iter().cloned().map(AnyRef::from).collect())
                    }
                })
            }),
            from: input.from_(),
            container: &self.all_storages,
            all_dictionaries: &self.all_dictionaries,
            typename: "storage",
        }
    }
}

// Facet key for fuchsia.test
const TEST_FACET_KEY: &'static str = "fuchsia.test";

// Facet key for deprecated-allowed-packages.
const TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY: &'static str = "deprecated-allowed-packages";

// Facet key for type.
const TEST_TYPE_FACET_KEY: &'static str = "type";

/// Helper type that assists with validating declarations of `{use, offer, expose} from self`.
struct RouteFromSelfCheckerV2<'a> {
    capability_name: Option<ContextSpanned<OneOrMany<AnyRef<'a>>>>,

    from: ContextSpanned<OneOrMany<AnyRef<'a>>>,

    container: &'a dyn Container,

    all_dictionaries: &'a HashMap<Name, &'a ContextCapability>,

    typename: &'static str,
}

impl<'a> RouteFromSelfCheckerV2<'a> {
    fn validate(self, operand: &'static str) -> Result<(), Error> {
        let Self { capability_name, from, container, all_dictionaries, typename } = self;

        let Some(capability_span) = capability_name else {
            return Ok(());
        };

        for capability in capability_span.value.iter() {
            let AnyRef::Named(name_ref) = capability else {
                continue;
            };

            let capability_name = name_ref.as_str();

            for from_ref in from.value.iter() {
                match from_ref {
                    AnyRef::Self_ if !container.contains(name_ref) => {
                        return Err(Error::validate_context(
                            format!(
                                "{typename} \"{capability_name}\" is {operand} from self, so it \
                                must be declared as a \"{typename}\" in \"capabilities\"",
                            ),
                            Some(capability_span.origin.clone()),
                        ));
                    }

                    AnyRef::Dictionary(DictionaryRef { root: RootDictionaryRef::Self_, path }) => {
                        let first_segment = path.iter_segments().next().unwrap();
                        if !all_dictionaries.contains_key(first_segment) {
                            return Err(Error::validate_context(
                                format!(
                                    "{typename} \"{capability_name}\" is {operand} from \"self/{path}\", so \
                                    \"{first_segment}\" must be declared as a \"dictionary\" in \"capabilities\"",
                                ),
                                Some(from.origin.clone()),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

/// [Container] provides a capability type agnostic trait to check for the existence of a
/// capability definition of a particular type. This is useful for writing common validation
/// functions.
trait Container {
    fn contains(&self, key: &BorrowedName) -> bool;
}

impl<'a> Container for HashSet<&'a BorrowedName> {
    fn contains(&self, key: &BorrowedName) -> bool {
        self.contains(key)
    }
}

impl Container for HashSet<Name> {
    fn contains(&self, key: &BorrowedName) -> bool {
        self.contains(key)
    }
}

impl<'a, T> Container for HashMap<&'a BorrowedName, T> {
    fn contains(&self, key: &BorrowedName) -> bool {
        self.contains_key(key)
    }
}

impl<T> Container for HashMap<Name, T> {
    fn contains(&self, key: &BorrowedName) -> bool {
        self.contains_key(key)
    }
}

// Construct the config type information out of a `use` for a configuration capability.
// This will return validation errors if the `use` is missing fields.
pub fn use_config_to_value_type_context(u: &ContextUse) -> Result<ConfigValueType, Error> {
    let config = u.config.clone().expect("Only call use_config_to_value_type on a Config");

    let Some(config_type) = u.config_type.as_ref() else {
        return Err(Error::validate_context(
            format!("Config '{}' is missing field 'type'", config.value),
            Some(config.origin),
        ));
    };

    let config_type = match config_type.value {
        ConfigType::Bool => ConfigValueType::Bool { mutability: None },
        ConfigType::Uint8 => ConfigValueType::Uint8 { mutability: None },
        ConfigType::Uint16 => ConfigValueType::Uint16 { mutability: None },
        ConfigType::Uint32 => ConfigValueType::Uint32 { mutability: None },
        ConfigType::Uint64 => ConfigValueType::Uint64 { mutability: None },
        ConfigType::Int8 => ConfigValueType::Int8 { mutability: None },
        ConfigType::Int16 => ConfigValueType::Int16 { mutability: None },
        ConfigType::Int32 => ConfigValueType::Int32 { mutability: None },
        ConfigType::Int64 => ConfigValueType::Int64 { mutability: None },
        ConfigType::String => {
            let Some(ref max_size) = u.config_max_size else {
                return Err(Error::validate_context(
                    format!(
                        "Config '{}' is type String but is missing field 'max_size'",
                        config.value
                    ),
                    Some(config.origin),
                ));
            };
            ConfigValueType::String { max_size: max_size.value.into(), mutability: None }
        }
        ConfigType::Vector => {
            let Some(ref element) = u.config_element_type else {
                return Err(Error::validate_context(
                    format!(
                        "Config '{}' is type Vector but is missing field 'element'",
                        config.value
                    ),
                    Some(config.origin),
                ));
            };
            let Some(ref max_count) = u.config_max_count else {
                return Err(Error::validate_context(
                    format!(
                        "Config '{}' is type Vector but is missing field 'max_count'",
                        config.value
                    ),
                    Some(config.origin),
                ));
            };
            ConfigValueType::Vector {
                max_count: max_count.value.into(),
                element: element.value.clone(),
                mutability: None,
            }
        }
    };
    Ok(config_type)
}

/// Given an iterator with `(key, name)` tuples, ensure that `key` doesn't
/// appear twice. `name` is used in generated error messages.
fn ensure_no_duplicate_names<'a, I>(values: I) -> Result<(), Error>
where
    I: Iterator<Item = (&'a BorrowedName, &'a str)>,
{
    let mut seen_keys = HashMap::new();
    for (key, name) in values {
        if let Some(preexisting_name) = seen_keys.insert(key, name) {
            return Err(Error::validate(format!(
                "identifier \"{}\" is defined twice, once in \"{}\" and once in \"{}\"",
                key, name, preexisting_name
            )));
        }
    }
    Ok(())
}

/// Returns an error if the iterator contains duplicate values.
fn ensure_no_duplicate_values<'a, I, V>(values: I) -> Result<(), Error>
where
    I: IntoIterator<Item = &'a V>,
    V: 'a + Hash + Eq + fmt::Display,
{
    let mut seen = HashSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(Error::validate(format!("Found duplicate value \"{}\" in array.", value)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Location;
    use crate::types::offer::{
        offer_to_all_and_component_diff_capabilities_message,
        offer_to_all_and_component_diff_sources_message,
    };
    use assert_matches::assert_matches;
    use serde_json::json;
    use std::path::Path;

    macro_rules! test_validate_cml_with_context {
        (
            $(
                $test_name:ident($input:expr, $($pattern:tt)+),
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    let input = format!("{}", $input);
                    let result = validate_for_test_context("test.cml", &input.as_bytes());
                    assert_matches!(result, $($pattern)+);
                }
            )+
        }
    }

    macro_rules! test_validate_cml_with_feature_context {
        (
            $features:expr,
            {
                $(
                    $test_name:ident($input:expr, $($pattern:tt)+),
                )+
            }
        ) => {
            $(
                #[test]
                fn $test_name() {
                    let input = format!("{}", $input);
                    let features = $features;
                    let result = validate_with_features_for_test("test.cml", &input.as_bytes(), &features, &vec![], &vec![], &vec![]);
                    assert_matches!(result, $($pattern)+);
                }
            )+
        }
    }

    fn validate_for_test_context(filename: &str, input: &[u8]) -> Result<(), Error> {
        validate_with_features_for_test(filename, input, &FeatureSet::empty(), &[], &[], &[])
    }

    fn validate_with_features_for_test(
        filename: &str,
        input: &[u8],
        features: &FeatureSet,
        required_offers: &[String],
        required_uses: &[String],
        required_dictionary_offers: &[String],
    ) -> Result<(), Error> {
        let input = format!("{}", std::str::from_utf8(input).unwrap().to_string());
        let file = Path::new(filename);
        let document = crate::load_cml_with_context(&input, file)?;
        validate_cml(
            &document,
            &features,
            &CapabilityRequirements {
                must_offer: &required_offers
                    .iter()
                    .map(|value| OfferToAllCapability::Protocol(value))
                    .chain(
                        required_dictionary_offers
                            .iter()
                            .map(|value| OfferToAllCapability::Dictionary(value)),
                    )
                    .collect::<Vec<_>>(),
                must_use: &required_uses
                    .iter()
                    .map(|value| MustUseRequirement::Protocol(value))
                    .collect::<Vec<_>>(),
            },
        )
    }

    fn unused_component_err_message(missing: &str) -> String {
        format!(r#"Protocol "{}" is not used by a component but is required by all"#, missing)
    }

    #[test]
    fn must_use_protocol() {
        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &[],
            &vec!["fuchsia.logger.LogSink".into()],
            &[],
        );

        assert_matches!(result,
            Err(Error::Validate { err, .. }) => {
                assert_eq!(err, unused_component_err_message("fuchsia.logger.LogSink"));
            }
        );

        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
            ],

            use: [
                {
                    protocol: [ "fuchsia.component.Binder" ],
                    from: "framework",
                }
            ],
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &[],
            &vec!["fuchsia.component.Binder".into()],
            &[],
        );
        assert_matches!(result, Ok(_));
    }

    #[test]
    fn required_offer_to_all() {
        let input = r##"{
           children: [
               {
                   name: "logger",
                   url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
               },
               {
                   name: "something",
                   url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
               },
           ],
           collections: [
               {
                   name: "coll",
                   durability: "transient",
               },
           ],
           offer: [
               {
                   protocol: "fuchsia.logger.LogSink",
                   from: "parent",
                   to: "all"
               },
               {
                   protocol: "fuchsia.inspect.InspectSink",
                   from: "parent",
                   to: "all"
               },
               {
                   protocol: "fuchsia.process.Launcher",
                   from: "parent",
                   to: "#something",
               },
           ]
       }"##;
        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into(), "fuchsia.inspect.InspectSink".into()],
            &Vec::new(),
            &[],
        );
        assert_matches!(result, Ok(_));
    }

    #[test]
    fn required_offer_to_all_manually() {
        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            collections: [
                {
                    name: "coll",
                    durability: "transient",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "#something",
                    to: "#logger"
                },
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "#something"
                },
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "#coll",
                },
            ]
        }"##;
        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &[],
            &[],
        );
        assert_matches!(result, Ok(_));

        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
                {
                    name: "something_v2",
                    url: "fuchsia-pkg://fuchsia.com/something_v2#meta/something_v2.cm",
                },
            ],
            collections: [
                {
                    name: "coll",
                    durability: "transient",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: ["#logger", "#something", "#something_v2", "#coll"],
                },
            ]
        }"##;
        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &[],
            &[],
        );
        assert_matches!(result, Ok(_));
    }

    #[test]
    fn offer_to_all_mixed_with_array_syntax() {
        let input = r##"{
                "children": [
                    {
                        "name": "something",
                        "url": "fuchsia-pkg://fuchsia.com/something/stable#meta/something.cm",
                    },
                ],
                "offer": [
                    {
                        "protocol": ["fuchsia.logger.LogSink", "fuchsia.inspect.InspectSink",],
                        "from": "parent",
                        "to": "#something",
                    },
                    {
                        "protocol": "fuchsia.logger.LogSink",
                        "from": "parent",
                        "to": "all",
                    },
                ],
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &Vec::new(),
            &[],
        );

        assert_matches!(result, Ok(_));

        let input = r##"{

                "children": [
                    {
                        "name": "something",
                        "url": "fuchsia-pkg://fuchsia.com/something/stable#meta/something.cm",
                    },
                ],
                "offer": [
                    {
                        "protocol": ["fuchsia.logger.LogSink", "fuchsia.inspect.InspectSink",],
                        "from": "parent",
                        "to": "all",
                    },
                    {
                        "protocol": "fuchsia.logger.LogSink",
                        "from": "parent",
                        "to": "#something",
                    },
                ],
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &Vec::new(),
            &[],
        );

        assert_matches!(result, Ok(_));
    }

    #[test]
    fn offer_to_all_and_manual() {
        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "all"
                },
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "#something"
                },
            ]
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &Vec::new(),
            &[],
        );

        // exact duplication is allowed
        assert_matches!(result, Ok(_));

        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "all"
                },
                {
                    protocol: "fuchsia.logger.FakLog",
                    from: "parent",
                    as: "fuchsia.logger.LogSink",
                    to: "#something"
                },
            ]
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &Vec::new(),
            &[],
        );

        // aliased duplications are forbidden
        assert_matches!(result,
            Err(Error::ValidateContexts { err, .. }) => {
                assert_eq!(
                    err,
                    offer_to_all_and_component_diff_capabilities_message([OfferToAllCapability::Protocol("fuchsia.logger.LogSink")].into_iter(), "something"),
                );
            }
        );

        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "all"
                },
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "framework",
                    to: "#something"
                },
            ]
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &Vec::new(),
            &[],
        );

        // offering the same protocol without an alias from different sources is forbidden
        assert_matches!(result,
            Err(Error::ValidateContexts { err, .. }) => {
                assert_eq!(
                    err,
                    offer_to_all_and_component_diff_sources_message([OfferToAllCapability::Protocol("fuchsia.logger.LogSink")].into_iter(), "something"),
                );
            }
        );
    }

    #[test]
    fn offer_to_all_and_manual_for_dictionary() {
        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    dictionary: "diagnostics",
                    from: "parent",
                    to: "all"
                },
                {
                    dictionary: "diagnostics",
                    from: "parent",
                    to: "#something"
                },
            ]
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec![],
            &Vec::new(),
            &["diagnostics".into()],
        );

        // exact duplication is allowed
        assert_matches!(result, Ok(_));

        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    dictionary: "diagnostics",
                    from: "parent",
                    to: "all"
                },
                {
                    dictionary: "FakDictionary",
                    from: "parent",
                    as: "diagnostics",
                    to: "#something"
                },
            ]
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec![],
            &Vec::new(),
            &["diagnostics".into()],
        );

        // aliased duplications are forbidden
        assert_matches!(result,
            Err(Error::ValidateContexts { err, .. }) => {
                assert_eq!(
                    err,
                    offer_to_all_and_component_diff_capabilities_message([OfferToAllCapability::Dictionary("diagnostics")].into_iter(), "something"),
                );
            }
        );

        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    dictionary: "diagnostics",
                    from: "parent",
                    to: "all"
                },
                {
                    dictionary: "diagnostics",
                    from: "framework",
                    to: "#something"
                },
            ]
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec![],
            &Vec::new(),
            &["diagnostics".into()],
        );

        // offering the same dictionary without an alias from different sources is forbidden
        assert_matches!(result,
            Err(Error::ValidateContexts { err, .. }) => {
                assert_eq!(
                    err,
                    offer_to_all_and_component_diff_sources_message([OfferToAllCapability::Dictionary("diagnostics")].into_iter(), "something"),
                );
            }
        );
    }

    fn offer_to_all_diff_sources_message(protocols: &[&str]) -> String {
        format!(r#"Protocol(s) {:?} offered to "all" multiple times"#, protocols)
    }

    #[test]
    fn offer_to_all_from_diff_sources() {
        let input = r##"{
            "children": [
                {
                    "name": "logger",
                    "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                },
                {
                    "name": "something",
                    "url": "fuchsia-pkg://fuchsia.com/something#meta/something.cm"
                }
            ],
            "offer": [
                {
                    "protocol": "fuchsia.logger.LogSink",
                    "from": "parent",
                    "to": "all"
                },
                {
                    "protocol": "fuchsia.logger.LogSink",
                    "from": "framework",
                    "to": "all"
                }
            ]
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &Vec::new(),
            &[],
        );

        assert_matches!(result,
            Err(Error::ValidateContexts { err, .. }) => {
                assert_eq!(
                    err,
                    offer_to_all_diff_sources_message(&["fuchsia.logger.LogSink"]),
                );
            }
        );
    }

    #[test]
    fn offer_to_all_with_aliases() {
        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "all"
                },
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "framework",
                    to: "all",
                    as: "OtherLogSink",
                },
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "framework",
                    to: "#something",
                    as: "OtherOtherLogSink",
                },
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "#something",
                    as: "fuchsia.logger.LogSink",
                },
            ]
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &["fuchsia.logger.LogSink".into()],
            &[],
            &[],
        );

        assert_matches!(result, Ok(_));
    }

    #[test]
    fn offer_to_all_with_aliases_no_span() {
        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "all"
                },
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "framework",
                    to: "all",
                    as: "OtherLogSink",
                },
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "framework",
                    to: "#something",
                    as: "OtherOtherLogSink",
                },
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "#something",
                    as: "fuchsia.logger.LogSink",
                },
            ]
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &["fuchsia.logger.LogSink".into()],
            &[],
            &[],
        );

        assert_matches!(result, Ok(_));
    }

    #[test]
    fn required_dict_offers_accept_aliases() {
        let input = r##"{
            capabilities: [
                {
                    dictionary: "test-diagnostics",
                }
            ],
            children: [
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    dictionary: "test-diagnostics",
                    from: "self",
                    to: "#something",
                    as: "diagnostics",
                }
            ]
        }"##;

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &[],
            &[],
            &["diagnostics".into()],
        );

        assert_matches!(result, Ok(_));
    }

    fn fail_to_make_required_offer(
        protocol: &str,
        child_or_collection: &str,
        component: &str,
    ) -> String {
        format!(
            r#"Protocol "{}" is not offered to {} "{}" but it is a required offer"#,
            protocol, child_or_collection, component
        )
    }

    fn fail_to_make_required_offer_dictionary(
        dictionary: &str,
        child_or_collection: &str,
        component: &str,
    ) -> String {
        format!(
            r#"Dictionary "{}" is not offered to {} "{}" but it is a required offer"#,
            dictionary, child_or_collection, component
        )
    }

    #[test]
    fn fail_to_offer_to_all_when_required() {
        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "#logger"
                },
                {
                    protocol: "fuchsia.logger.LegacyLog",
                    from: "parent",
                    to: "#something"
                },
            ]
        }"##;
        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &[],
            &[],
        );

        assert_matches!(result,
            Err(Error::ValidateContext { err, origin }) => {
                assert_eq!(
                    err,
                    fail_to_make_required_offer(
                        "fuchsia.logger.LogSink",
                        "child component",
                        "something",
                    ),
                );
                assert!(origin.is_some(), "Expected there to be a origin in error message");
            }
        );

        let result_context = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &[],
            &[],
        );

        assert_matches!(result_context,
            Err(Error::ValidateContext { err, origin }) => {
                assert_eq!(
                    err,
                    fail_to_make_required_offer(
                        "fuchsia.logger.LogSink",
                        "child component",
                        "something",
                    ),
                );
                assert!(origin.is_some(), "Expected there to be an origin in error message");
            }
        );

        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
            ],
            collections: [
                {
                    name: "coll",
                    durability: "transient",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "#logger"
                },
            ]
        }"##;
        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &[],
            &[],
        );

        assert_matches!(result,
            Err(Error::ValidateContext { err, origin }) => {
                assert_eq!(
                    err,
                    fail_to_make_required_offer("fuchsia.logger.LogSink", "collection", "coll"),
                );
                assert!(origin.is_some(), "Expected there to be a origin in error message");
            }
        );

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &[],
            &[],
        );

        assert_matches!(result,
            Err(Error::ValidateContext { err, origin }) => {
                assert_eq!(
                    err,
                    fail_to_make_required_offer("fuchsia.logger.LogSink", "collection", "coll"),
                );
                assert!(origin.is_some(), "Expected there to be an origin in error message");
            }
        );
    }

    #[test]
    fn fail_to_offer_dictionary_to_all_when_required() {
        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "all"
                },
                {
                    dictionary: "diagnostics",
                    from: "parent",
                    to: "#logger"
                },
                {
                    protocol: "fuchsia.logger.LegacyLog",
                    from: "parent",
                    to: "#something"
                },
            ]
        }"##;
        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec![],
            &[],
            &["diagnostics".to_string()],
        );

        assert_matches!(result,
            Err(Error::ValidateContext { err, origin }) => {
                assert_eq!(
                    err,
                    fail_to_make_required_offer_dictionary(
                        "diagnostics",
                        "child component",
                        "something",
                    ),
                );
                assert!(origin.is_some(), "Expected there to be a origin in error message");
            }
        );

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec![],
            &[],
            &["diagnostics".to_string()],
        );

        assert_matches!(result,
            Err(Error::ValidateContext { err, origin }) => {
                assert_eq!(
                    err,
                    fail_to_make_required_offer_dictionary(
                        "diagnostics",
                        "child component",
                        "something",
                    ),
                );
                assert!(origin.is_some(), "Expected there to be an origin in error message");
            }
        );

        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
            ],
            collections: [
                {
                    name: "coll",
                    durability: "transient",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "all"
                },
                {
                    protocol: "diagnostics",
                    from: "parent",
                    to: "all"
                },
                {
                    dictionary: "diagnostics",
                    from: "parent",
                    to: "#logger"
                },
            ]
        }"##;
        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &[],
            &["diagnostics".to_string()],
        );
        assert_matches!(result,
            Err(Error::ValidateContext { err, origin }) => {
                assert_eq!(
                    err,
                    fail_to_make_required_offer_dictionary("diagnostics", "collection", "coll"),
                );
                assert!(origin.is_some(), "Expected there to be a origin in error message");
            }
        );

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &vec!["fuchsia.logger.LogSink".into()],
            &[],
            &["diagnostics".to_string()],
        );
        assert_matches!(result,
            Err(Error::ValidateContext { err, origin }) => {
                assert_eq!(
                    err,
                    fail_to_make_required_offer_dictionary("diagnostics", "collection", "coll"),
                );
                assert!(origin.is_some(), "Expected there to be an origin in error message");
            }
        );
    }

    #[test]
    fn fail_to_offer_dictionary_to_all_when_required_even_if_protocol_called_diagnostics_offered() {
        let input = r##"{
            children: [
                {
                    name: "logger",
                    url: "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
                {
                    name: "something",
                    url: "fuchsia-pkg://fuchsia.com/something#meta/something.cm",
                },
            ],
            offer: [
                {
                    protocol: "fuchsia.logger.LogSink",
                    from: "parent",
                    to: "all"
                },
                {
                    protocol: "diagnostics",
                    from: "parent",
                    to: "all"
                },
                {
                    protocol: "fuchsia.logger.LegacyLog",
                    from: "parent",
                    to: "#something"
                },
            ]
        }"##;
        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &[],
            &[],
            &["diagnostics".to_string()],
        );

        assert_matches!(result,
            Err(Error::ValidateContext { err, origin }) => {
                assert_eq!(
                    err,
                    fail_to_make_required_offer_dictionary(
                        "diagnostics",
                        "child component",
                        "logger",
                    ),
                );
                assert!(origin.is_some(), "Expected there to be a origin in error message");
            }
        );

        let result = validate_with_features_for_test(
            "test.cml",
            input.as_bytes(),
            &FeatureSet::empty(),
            &[],
            &[],
            &["diagnostics".to_string()],
        );

        assert_matches!(result,
            Err(Error::ValidateContext { err, origin }) => {
                assert_eq!(
                    err,
                    fail_to_make_required_offer_dictionary(
                        "diagnostics",
                        "child component",
                        "logger",
                    ),
                );
                assert!(origin.is_some(), "Expected there to be an origin in error message");
            }
        );
    }

    #[test]
    fn test_validate_invalid_json_fails() {
        let result = validate_for_test_context("test.cml", b"{");
        let expected_err = r#" --> 1:2
  |
1 | {
  |  ^---
  |
  = expected identifier or string"#;
        assert_matches!(result, Err(Error::Parse { err, .. }) if &err == expected_err);
    }

    #[test]
    fn test_cml_json5() {
        let input = r##"{
            "expose": [
                // Here are some services to expose.
                { "protocol": "fuchsia.logger.Log", "from": "#logger", },
                { "directory": "blobfs", "from": "#logger", "rights": ["rw*"]},
            ],
            "children": [
                {
                    name: 'logger',
                    'url': 'fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm',
                },
            ],
        }"##;

        let result = validate_for_test_context("test.cml", input.as_bytes());
        assert_matches!(result, Ok(()));
    }

    #[test]
    fn test_cml_error_location() {
        let input = r##"{
    "use": [
        {
            "protocol": "foo",
            "from": "bad",
        },
    ],
}"##;
        let result = validate_for_test_context("test.cml", input.as_bytes());
        assert_matches!(
            result,
            Err(Error::Parse { err, location: Some(l), filename: Some(f) })
                if &err == "invalid value: string \"bad\", expected \"parent\", \"framework\", \"debug\", \"self\", \"#<capability-name>\", \"#<child-name>\", \"#<collection-name>\", dictionary path, or none" &&
                l == Location { line: 5, column: 21 } &&
                f.ends_with("test.cml")
        );
    }

    test_validate_cml_with_context! {
        test_cml_empty_json(
            json!({}),
            Ok(())
        ),

        test_cml_children_url_ends_in_cml(
            r##"{
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cml"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "child URL ends in .cml instead of .cm, which is almost certainly a mistake: fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cml"),

        test_cml_allow_long_names_without_feature(
            json!({
                "collections": [
                    {
                        "name": "foo",
                        "durability": "transient",
                        "allow_long_names": true
                    },
                ],
            }),
            Err(Error::RestrictedFeature(s)) if s == "allow_long_names"
        ),

        test_cml_directory_missing_path(
            r##"{
                "capabilities": [
                    {
                        "directory": "dir",
                        "rights": ["connect"]
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, ..}) if &err == "\"path\" should be present with \"directory\""
        ),
        test_cml_directory_missing_rights(
            r##"{
                "capabilities": [
                    {
                        "directory": "dir",
                        "path": "/dir"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"rights\" should be present with \"directory\""),

        test_cml_storage_missing_from(
                r##"{
                "capabilities": [
                    {
                        "storage": "data-storage",
                        "backing_dir": "minfs",
                        "storage_id": "static_instance_id_or_moniker"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"from\" should be present with \"storage\""),


        test_cml_storage_path(
            r##"{
                    "capabilities": [ {
                        "storage": "minfs",
                        "from": "self",
                        "path": "/minfs",
                        "storage_id": "static_instance_id_or_moniker"
                    } ]
                }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"path\" cannot be present with \"storage\", use \"backing_dir\""),

        test_cml_storage_missing_path_or_backing_dir(
            r##"{
                    "capabilities": [ {
                        "storage": "minfs",
                        "from": "self",
                        "storage_id": "static_instance_id_or_moniker"
                    } ]
                }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"backing_dir\" should be present with \"storage\""
        ),

        test_cml_storage_missing_storage_id(
            r##"{
                    "capabilities": [ {
                        "storage": "minfs",
                        "from": "self",
                        "backing_dir": "storage"
                    } ]
                }"##,
            Err(Error::ValidateContext{ err, .. }) if &err == "\"storage_id\" should be present with \"storage\""),

        test_cml_capabilities_extraneous_resolver_from(
            r##"{
                "capabilities": [
                    {
                        "resolver": "pkg_resolver",
                        "path": "/svc/fuchsia.component.resolution.Resolver",
                        "from": "self"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"from\" should not be present with \"resolver\""),

        test_cml_resolver_missing_path(
            r##"{
                "capabilities": [
                    {
                        "resolver": "pkg_resolver"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"path\" should be present with \"resolver\""),

        test_cml_runner_missing_path(
            r##"{
                "capabilities": [
                    {
                        "runner": "runrun"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"path\" should be present with \"runner\""
        ),

        test_cml_runner_extraneous_from(
            r##"{
                "capabilities": [
                    {
                        "runner": "a",
                        "path": "/example",
                        "from": "self"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"from\" should not be present with \"runner\""
        ),

        test_cml_service_multi_invalid_path(
            r##"{
                "capabilities": [
                    {
                        "service": ["a", "b", "c"],
                        "path": "/minfs"
                    }
                ]
            }"##,
            Err(Error::ValidateContext{ err, .. }) if &err == "\"path\" can only be specified when one `service` is supplied."
        ),

        test_cml_protocol_multi_invalid_path(
            r##"{
                "capabilities": [
                    {
                        "protocol": ["a", "b", "c"],
                        "path": "/minfs"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"path\" can only be specified when one `protocol` is supplied."),

        test_cml_use_bad_duplicate_target_names(
            r##"{
                "use": [
                  { "protocol": "fuchsia.component.Realm" },
                  { "protocol": "fuchsia.component.Realm" }
                ]
            }"##,
            Err(Error::ValidateContexts { err, .. }) if &err == "\"/svc/fuchsia.component.Realm\" is a duplicate \"use\" target protocol"),

        test_cml_use_disallows_nested_dirs_directory(
            r##"{
                "use": [
                    { "directory": "foobar", "path": "/foo/bar", "rights": [ "r*" ] },
                    { "directory": "foobarbaz", "path": "/foo/bar/baz", "rights": [ "r*" ] }
                ]
            }"##,
            Err(Error::ValidateContexts { err, .. }) if &err == "directory \"/foo/bar\" is a prefix of \"use\" target directory \"/foo/bar/baz\""
        ),
        test_cml_use_disallows_nested_dirs_storage(
            r##"{
                "use": [
                    { "storage": "foobar", "path": "/foo/bar" },
                    { "storage": "foobarbaz", "path": "/foo/bar/baz" }
                ]
            }"##,
            Err(Error::ValidateContexts { err, .. }) if &err == "storage \"/foo/bar\" is a prefix of \"use\" target storage \"/foo/bar/baz\""
        ),
        test_cml_use_disallows_nested_dirs_directory_and_storage(
            r##"{
                "use": [
                    { "directory": "foobar", "path": "/foo/bar", "rights": [ "r*" ] },
                    { "storage": "foobarbaz", "path": "/foo/bar/baz" }
                ]
            }"##,
            Err(Error::ValidateContexts { err, .. }) if &err == "directory \"/foo/bar\" is a prefix of \"use\" target storage \"/foo/bar/baz\""
        ),
         test_cml_use_disallows_common_prefixes_service(
             r##"{
                 "use": [
                     { "directory": "foobar", "path": "/foo/bar", "rights": [ "r*" ] },
                     { "protocol": "fuchsia", "path": "/foo/bar/fuchsia" }
                 ]
             }"##,
             Err(Error::ValidateContexts { err, .. }) if &err == "directory \"/foo/bar\" is a prefix of \"use\" target protocol \"/foo/bar/fuchsia\""
        ),
        test_cml_use_disallows_common_prefixes_protocol(
            r##"{
                "use": [
                    { "directory": "foobar", "path": "/foo/bar", "rights": [ "r*" ] },
                    { "protocol": "fuchsia", "path": "/foo/bar/fuchsia.2" }
                ]
            }"##,
            Err(Error::ValidateContexts { err, .. }) if &err == "directory \"/foo/bar\" is a prefix of \"use\" target protocol \"/foo/bar/fuchsia.2\""
        ),


        test_cml_use_invalid_from_with_service(
            json!({
                "use": [ { "service": "foo", "from": "debug" } ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "only \"protocol\" supports source from \"debug\""
        ),

        test_cml_use_runner_debug_ref(
            r##"{
                "use": [
                    {
                        "runner": "elf",
                        "from": "debug"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "only \"protocol\" supports source from \"debug\""
        ),

        test_cml_availability_not_supported_for_event_streams(
            r##"{
                "use": [
                    {
                        "event_stream": ["destroyed"],
                        "from": "parent",
                        "availability": "optional"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"availability\" cannot be used with \"event_stream\""
        ),

        test_cml_use_disallows_filter_on_non_events(
            json!({
                "use": [
                    { "directory": "foobar", "path": "/foo/bar", "rights": [ "r*" ], "filter": {"path": "/diagnostics"} },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"filter\" can only be used with \"event_stream\""
        ),

        test_cml_use_from_with_storage(
            json!({
                "use": [ { "storage": "cache", "from": "parent" } ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"from\" cannot be used with \"storage\""
        ),

        test_cml_availability_not_supported_for_runner(
            r##"{
                "use": [
                    {
                        "runner": "destroyed",
                        "from": "parent",
                        "availability": "optional"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"availability\" cannot be used with \"runner\""
        ),

        test_cml_use_event_stream_self_ref(
            r##"{
                "use": [
                    {
                        "event_stream": ["started"],
                        "path": "/svc/my_stream",
                        "from": "self"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"from: self\" cannot be used with \"event_stream\""
        ),

        test_cml_use_runner_self_ref(
            r##"{
                "use": [
                    {
                        "runner": "elf",
                        "from": "self"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"from: self\" cannot be used with \"runner\""
        ),

        test_cml_use_invalid_availability(
            r##"{
                "use": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "availability": "same_as_target"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "\"availability: same_as_target\" cannot be used with use declarations"
        ),

        test_cml_use_config_bad_string(
            r##"{
                "use": [
                    {
                        "config": "fuchsia.config.MyConfig",
                        "key": "my_config",
                        "type": "string"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. })
            if &err == "Config 'fuchsia.config.MyConfig' is type String but is missing field 'max_size'"
        ),

        test_config_required_with_default(
            r##"{"use": [
                {
                    "config": "fuchsia.config.MyConfig",
                    "key": "my_config",
                    "type": "bool",
                    "default": "true"
                }
            ]}"##,
            Err(Error::ValidateContext {err, ..})
            if &err == "Config 'fuchsia.config.MyConfig' is required and has a default value"
        ),

        test_cml_use_numbered_handle_and_path(
            json!({
                "use": [
                    {
                        "protocol": "foo",
                        "path": "/svc/foo",
                        "numbered_handle": 0xab
                    }
                ]
        }),
            Err(Error::ValidateContext { err, .. }) if &err == "`path` and `numbered_handle` are incompatible"
        ),

        test_cml_use_numbered_handle_not_protocol(
            json!({
                "use": [
                    {
                        "runner": "foo",
                        "numbered_handle": 0xab
                    }
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "`numbered_handle` is only supported for `use protocol`"
        ),

        test_cml_expose_invalid_subdir_to_framework(
            r##"{
                "capabilities": [
                    {
                        "directory": "foo",
                        "rights": ["r*"],
                        "path": "/foo"
                    }
                ],
                "expose": [
                    {
                        "directory": "foo",
                        "from": "self",
                        "to": "framework",
                        "subdir": "blob"
                    }
                ],
                "children": [
                    {
                        "name": "child",
                        "url": "fuchsia-pkg://fuchsia.com/pkg#comp.cm"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "`subdir` is not supported for expose to framework. Directly expose the subdirectory instead."
        ),
        test_cml_expose_event_stream_multiple_as(
            r##"{
                "expose": [
                    {
                        "event_stream": ["started", "stopped"],
                        "from" : "framework",
                        "as": "something"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "as cannot be used with multiple event streams"
        ),

        test_cml_expose_event_stream_to_framework(
            r##"{
                "expose": [
                    {
                        "event_stream": ["started", "stopped"],
                        "from" : "self",
                        "to": "framework"
                    }
                ]
            }"##,
            Err(Error::ValidateContext { err, .. }) if &err == "cannot expose an event_stream to framework"
        ),

        test_cml_expose_event_stream_from_self(
            json!({
                "expose": [
                    { "event_stream": ["started", "stopped"], "from" : "self" },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "Cannot expose event_streams from self"
        ),

        test_cml_offer_event_stream_from_self(
            json!({
                "offer": [
                    { "event_stream": ["started", "stopped"], "from" : "self", "to": "#self" },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "cannot offer an event_stream from self"
        ),

        test_cml_rights_alias_star_expansion_collision(
            r##"{
                "use": [
                  {
                    "directory": "mydir",
                    "path": "/mydir",
                    "rights": ["w*", "x*"]
                  }
                ]
            }"##,
            Err(Error::ValidateContext { err, ..  }) if &err == "\"x*\" is duplicated in the rights clause."
        ),

        test_cml_rights_alias_star_expansion_with_longform_collision(
            r##"{
                "use": [
                  {
                    "directory": "mydir",
                    "path": "/mydir",
                    "rights": ["r*", "read_bytes"]
                  }
                ]
            }"##,
            Err(Error::ValidateContext { err, ..}) if &err == "\"read_bytes\" is duplicated in the rights clause."
        ),

        test_cml_rights_use_invalid(
            json!({
                "use": [
                  { "directory": "mydir", "path": "/mydir" },
                ]
            }),
            Err(Error::ValidateContexts { err, .. }) if &err == "This use statement requires a `rights` field. Refer to: https://fuchsia.dev/go/components/directory#consumer."
        ),
        test_cml_use_missing_props(
            json!({
                "use": [ { "path": "/svc/fuchsia.logger.Log" } ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "`use` declaration is missing a capability keyword, one of: \"service\", \"protocol\", \"directory\", \"storage\", \"event_stream\", \"runner\", \"config\", \"dictionary\""
        ),


        test_cml_use_two_types_bad(
            r##"{"use": [
                {
                    "protocol": "fuchsia.protocol.MyProtocol",
                    "service": "fuchsia.service.MyService"
                }
            ]
        }"##,
            Err(Error::ValidateContext {err, ..})
            if &err == "use declaration has multiple capability types defined: [\"service\", \"protocol\"]"
            ),
    test_cml_expose_two_types_bad(
        r##"{"expose": [
            {
                "protocol": "fuchsia.protocol.MyProtocol",
                "service": "fuchsia.service.MyService",
                "from" : "self"
            }
        ]
    }"##,
        Err(Error::ValidateContext {err, ..})
        if &err == "expose declaration has multiple capability types defined: [\"service\", \"protocol\"]"
        ),

        test_cml_expose_from_self(
            json!({
                "expose": [
                    {
                        "protocol": "foo_protocol",
                        "from": "self",
                    },
                    {
                        "protocol": [ "bar_protocol", "baz_protocol" ],
                        "from": "self",
                    },
                    {
                        "directory": "foo_directory",
                        "from": "self",
                    },
                    {
                        "runner": "foo_runner",
                        "from": "self",
                    },
                    {
                        "resolver": "foo_resolver",
                        "from": "self",
                    },
                ],
                "capabilities": [
                    {
                        "protocol": "foo_protocol",
                    },
                    {
                        "protocol": "bar_protocol",
                    },
                    {
                        "protocol": "baz_protocol",
                    },
                    {
                        "directory": "foo_directory",
                        "path": "/dir",
                        "rights": [ "r*" ],
                    },
                    {
                        "runner": "foo_runner",
                        "path": "/svc/runner",
                    },
                    {
                        "resolver": "foo_resolver",
                        "path": "/svc/resolver",
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_expose_protocol_from_self_missing(
            json!({
                "expose": [
                    {
                        "protocol": "pkg_protocol",
                        "from": "self",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "protocol \"pkg_protocol\" is exposed from self, so it must be declared as a \"protocol\" in \"capabilities\""
        ),
        test_cml_expose_protocol_from_self_missing_multiple(
            json!({
                "expose": [
                    {
                        "protocol": [ "foo_protocol", "bar_protocol" ],
                        "from": "self",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "protocol \"foo_protocol\" is exposed from self, so it must be declared as a \"protocol\" in \"capabilities\""
        ),
        test_cml_expose_directory_from_self_missing(
            json!({
                "expose": [
                    {
                        "directory": "pkg_directory",
                        "from": "self",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "directory \"pkg_directory\" is exposed from self, so it must be declared as a \"directory\" in \"capabilities\""
        ),
        test_cml_expose_service_from_self_missing(
            json!({
                "expose": [
                    {
                        "service": "pkg_service",
                        "from": "self",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "service \"pkg_service\" is exposed from self, so it must be declared as a \"service\" in \"capabilities\""
        ),
        test_cml_expose_runner_from_self_missing(
            json!({
                "expose": [
                    {
                        "runner": "dart",
                        "from": "self",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "runner \"dart\" is exposed from self, so it must be declared as a \"runner\" in \"capabilities\""
        ),
        test_cml_expose_resolver_from_self_missing(
            json!({
                "expose": [
                    {
                        "resolver": "pkg_resolver",
                        "from": "self",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "resolver \"pkg_resolver\" is exposed from self, so it must be declared as a \"resolver\" in \"capabilities\""
        ),
        test_cml_expose_from_self_missing_dictionary(
            json!({
                "expose": [
                    {
                        "protocol": "foo_protocol",
                        "from": "self/dict/inner",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "protocol \"foo_protocol\" is exposed from \"self/dict/inner\", so \"dict\" must be declared as a \"dictionary\" in \"capabilities\""
        ),

                test_cml_expose_from_dictionary_parent(
            json!({
                "expose": [
                    {
                        "protocol": "pkg_protocol",
                        "from": "parent/a",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "`expose` dictionary path must begin with `self` or `#<child-name>`"
        ),
        test_cml_expose_protocol_from_collection_invalid(
            json!({
                "collections": [ {
                    "name": "coll",
                    "durability": "transient",
                } ],
                "expose": [
                    { "protocol": "fuchsia.logger.Log", "from": "#coll" },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"expose\" source \"#coll\" does not appear in \"children\" or \"capabilities\""
        ),
        test_cml_expose_directory_from_collection_invalid(
            json!({
                "collections": [ {
                    "name": "coll",
                    "durability": "transient",
                } ],
                "expose": [
                    { "directory": "temp", "from": "#coll" },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"expose\" source \"#coll\" does not appear in \"children\""
        ),
        test_cml_expose_runner_from_collection_invalid(
            json!({
                "collections": [ {
                    "name": "coll",
                    "durability": "transient",
                } ],
                "expose": [
                    { "runner": "elf", "from": "#coll" },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"expose\" source \"#coll\" does not appear in \"children\""
        ),
        test_cml_expose_resolver_from_collection_invalid(
            json!({
                "collections": [ {
                    "name": "coll",
                    "durability": "transient",
                } ],
                "expose": [
                    { "resolver": "base", "from": "#coll" },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"expose\" source \"#coll\" does not appear in \"children\""
        ),

        test_cml_offer_from_self(
            json!({
                "offer": [
                    {
                        "protocol": "foo_protocol",
                        "from": "self",
                        "to": [ "#modular" ],
                    },
                    {
                        "protocol": [ "bar_protocol", "baz_protocol" ],
                        "from": "self",
                        "to": [ "#modular" ],
                    },
                    {
                        "directory": "foo_directory",
                        "from": "self",
                        "to": [ "#modular" ],
                    },
                    {
                        "runner": "foo_runner",
                        "from": "self",
                        "to": [ "#modular" ],
                    },
                    {
                        "resolver": "foo_resolver",
                        "from": "self",
                        "to": [ "#modular" ],
                    },
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    },
                ],
                "capabilities": [
                    {
                        "protocol": "foo_protocol",
                    },
                    {
                        "protocol": "bar_protocol",
                    },
                    {
                        "protocol": "baz_protocol",
                    },
                    {
                        "directory": "foo_directory",
                        "path": "/dir",
                        "rights": [ "r*" ],
                    },
                    {
                        "runner": "foo_runner",
                        "path": "/svc/fuchsia.sys2.ComponentRunner",
                    },
                    {
                        "resolver": "foo_resolver",
                        "path": "/svc/fuchsia.component.resolution.Resolver",
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_offer_service_from_self_missing(
            json!({
                "offer": [
                    {
                        "service": "pkg_service",
                        "from": "self",
                        "to": [ "#modular" ],
                    },
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "service \"pkg_service\" is offered from self, so it must be declared as a \"service\" in \"capabilities\""
        ),
        test_cml_offer_protocol_from_self_missing(
            json!({
                "offer": [
                    {
                        "protocol": "pkg_protocol",
                        "from": "self",
                        "to": [ "#modular" ],
                    },
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "protocol \"pkg_protocol\" is offered from self, so it must be declared as a \"protocol\" in \"capabilities\""
        ),
        test_cml_offer_protocol_from_self_missing_multiple(
            json!({
                "offer": [
                    {
                        "protocol": [ "foo_protocol", "bar_protocol" ],
                        "from": "self",
                        "to": [ "#modular" ],
                    },
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "protocol \"foo_protocol\" is offered from self, so it must be declared as a \"protocol\" in \"capabilities\""
        ),
        test_cml_offer_directory_from_self_missing(
            json!({
                "offer": [
                    {
                        "directory": "pkg_directory",
                        "from": "self",
                        "to": [ "#modular" ],
                    },
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "directory \"pkg_directory\" is offered from self, so it must be declared as a \"directory\" in \"capabilities\""
        ),
        test_cml_offer_runner_from_self_missing(
            json!({
                "offer": [
                    {
                        "runner": "dart",
                        "from": "self",
                        "to": [ "#modular" ],
                    },
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "runner \"dart\" is offered from self, so it must be declared as a \"runner\" in \"capabilities\""
        ),
        test_cml_offer_resolver_from_self_missing(
            json!({
                "offer": [
                    {
                        "resolver": "pkg_resolver",
                        "from": "self",
                        "to": [ "#modular" ],
                    },
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "resolver \"pkg_resolver\" is offered from self, so it must be declared as a \"resolver\" in \"capabilities\""
        ),
        test_cml_offer_storage_from_self_missing(
            json!({
                    "offer": [
                        {
                            "storage": "cache",
                            "from": "self",
                            "to": [ "#echo_server" ],
                        },
                    ],
                    "children": [
                        {
                            "name": "echo_server",
                            "url": "fuchsia-pkg://fuchsia.com/echo_server#meta/echo_server.cm",
                        },
                    ],
                }),
            Err(Error::ValidateContext { err, .. }) if &err == "storage \"cache\" is offered from self, so it must be declared as a \"storage\" in \"capabilities\""
        ),
        test_cml_offer_from_self_missing_dictionary(
            json!({
                "offer": [
                    {
                        "protocol": "foo_protocol",
                        "from": "self/dict/inner",
                        "to": [ "#modular" ],
                    },
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "protocol \"foo_protocol\" is offered from \"self/dict/inner\", so \"dict\" must be declared as a \"dictionary\" in \"capabilities\""
        ),

        test_cml_storage_offer_from_child(
            r##"{
                    "offer": [
                        {
                            "storage": "cache",
                            "from": "#storage_provider",
                            "to": [ "#echo_server" ]
                        }
                    ],
                    "children": [
                        {
                            "name": "echo_server",
                            "url": "fuchsia-pkg://fuchsia.com/echo_server#meta/echo_server.cm"
                        },
                        {
                            "name": "storage_provider",
                            "url": "fuchsia-pkg://fuchsia.com/storage_provider#meta/storage_provider.cm"
                        }
                    ]
                }"##,
            Err(Error::ValidateContexts { err, .. }) if &err == "Storage \"cache\" is offered from a child, but storage capabilities cannot be exposed"
        ),

        test_cml_offer_storage_from_collection_invalid(
            r##"{
                "collections": [ {
                    "name": "coll",
                    "durability": "transient"
                } ],
                "children": [ {
                    "name": "echo_server",
                    "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm"
                } ],
                "offer": [
                    { "storage": "cache", "from": "#coll", "to": [ "#echo_server" ] }
                ]
            }"##,
            Err(Error::ValidateContexts { err, .. }) if &err == "Storage \"cache\" is offered from a child, but storage capabilities cannot be exposed"
        ),

        test_cml_children_bad_environment(
            json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                        "environment": "parent",
                    }
                ]
            }),
            Err(Error::Parse { err, .. }) if err.starts_with("invalid value: string \"parent\", expected \"#<environment-name>\"")
        ),
        test_cml_children_environment(
            json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                        "environment": "#foo_env",
                    }
                ],
                "environments": [
                    {
                        "name": "foo_env",
                    }
                ]
            }),
            Ok(())
        ),
        test_cml_collections_bad_environment(
            json!({
                "collections": [
                    {
                        "name": "tests",
                        "durability": "transient",
                        "environment": "parent",
                    }
                ]
            }),
            Err(Error::Parse { err, .. }) if err.starts_with("invalid value: string \"parent\", expected \"#<environment-name>\"")
        ),
        test_cml_collections_environment(
            json!({
                "collections": [
                    {
                        "name": "tests",
                        "durability": "transient",
                        "environment": "#foo_env",
                    }
                ],
                "environments": [
                    {
                        "name": "foo_env",
                    }
                ]
            }),
            Ok(())
        ),

        test_cml_environment_timeout(
            json!({
                "environments": [
                    {
                        "name": "foo_env",
                        "__stop_timeout_ms": 10000,
                    }
                ]
            }),
            Ok(())
        ),
        test_cml_environment_bad_timeout(
            json!({
                "environments": [
                    {
                        "name": "foo_env",
                        "__stop_timeout_ms": -3,
                    }
                ]
            }),
            Err(Error::Parse { err, .. }) if err.starts_with("invalid value: integer `-3`, expected an unsigned 32-bit integer")
        ),

        test_cml_environment_debug(
            json!({
                "capabilities": [
                    {
                        "protocol": "fuchsia.logger.Log2",
                    },
                ],
                "environments": [
                    {
                        "name": "foo_env",
                        "extends": "realm",
                        "debug": [
                            {
                                "protocol": "fuchsia.module.Module",
                                "from": "#modular",
                            },
                            {
                                "protocol": "fuchsia.logger.OtherLog",
                                "from": "parent",
                            },
                            {
                                "protocol": "fuchsia.logger.Log2",
                                "from": "self",
                            },
                        ]
                    }
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    },
                ],
            }),
           Ok(())
        ),

        test_cml_environment_debug_missing_capability(
            json!({
                "environments": [
                    {
                        "name": "foo_env",
                        "extends": "realm",
                        "debug": [
                            {
                                "protocol": "fuchsia.module.Module",
                                "from": "#modular",
                            },
                            {
                                "protocol": "fuchsia.logger.OtherLog",
                                "from": "parent",
                            },
                            {
                                "protocol": "fuchsia.logger.Log2",
                                "from": "self",
                            },
                        ]
                    }
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "protocol \"fuchsia.logger.Log2\" is registered as debug from self, so it must be declared as a \"protocol\" in \"capabilities\""
        ),

        test_cml_environment_invalid_from_child(
            json!({
                "capabilities": [
                    {
                        "protocol": "fuchsia.logger.Log2",
                    },
                ],
                "environments": [
                    {
                        "name": "foo_env",
                        "extends": "realm",
                        "debug": [
                            {
                                "protocol": "fuchsia.module.Module",
                                "from": "#missing",
                            },
                            {
                                "protocol": "fuchsia.logger.OtherLog",
                                "from": "parent",
                            },
                            {
                                "protocol": "fuchsia.logger.Log2",
                                "from": "self",
                            },
                        ]
                    }
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"debug\" source \"#missing\" does not appear in \"children\" or \"capabilities\""
        ),

        test_cml_facets(
            json!({
                "facets": {
                    "metadata": {
                        "title": "foo",
                        "authors": [ "me", "you" ],
                        "year": 2018
                    }
                }
            }),
            Ok(())
        ),
        test_cml_facets_wrong_type(
            json!({
                "facets": 55
            }),
            Err(Error::Parse { err, .. }) if err.starts_with("invalid type: integer `55`, expected a map")
        ),

        test_cml_use_from_self(
            json!({
                "use": [
                    {
                        "protocol": [ "bar_protocol", "baz_protocol" ],
                        "from": "self",
                    },
                    {
                        "directory": "foo_directory",
                        "from": "self",
                        "path": "/dir",
                        "rights": [ "r*" ],
                    },
                    {
                        "service": "foo_service",
                        "from": "self",
                    },
                    {
                        "config": "foo_config",
                        "type": "bool",
                        "key": "k",
                        "from": "self",
                    },
                ],
                "capabilities": [
                    {
                        "protocol": "bar_protocol",
                    },
                    {
                        "protocol": "baz_protocol",
                    },
                    {
                        "directory": "foo_directory",
                        "path": "/dir",
                        "rights": [ "r*" ],
                    },
                    {
                        "service": "foo_service",
                    },
                    {
                        "config": "foo_config",
                        "type": "bool",
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_use_protocol_from_self_missing(
            json!({
                "use": [
                    {
                        "protocol": "foo_protocol",
                        "from": "self",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "protocol \"foo_protocol\" is used from self, so it must be declared as a \"protocol\" in \"capabilities\""
        ),

        test_cml_use_directory_from_self_missing(
            json!({
                "use": [
                    {
                        "directory": "foo_directory",
                        "from": "self",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "directory \"foo_directory\" is used from self, so it must be declared as a \"directory\" in \"capabilities\""
        ),
        test_cml_use_service_from_self_missing(
            json!({
                "use": [
                    {
                        "service": "foo_service",
                        "from": "self",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "service \"foo_service\" is used from self, so it must be declared as a \"service\" in \"capabilities\""
        ),
        test_cml_use_config_from_self_missing(
            json!({
                "use": [
                    {
                        "config": "foo_config",
                        "from": "self",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "config \"foo_config\" is used from self, so it must be declared as a \"config\" in \"capabilities\""
        ),
        test_cml_use_from_self_missing_dictionary(
            json!({
                "use": [
                    {
                        "protocol": "foo_protocol",
                        "from": "self/dict/inner",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "protocol \"foo_protocol\" is used from \"self/dict/inner\", so \"dict\" must be declared as a \"dictionary\" in \"capabilities\""
        ),
        test_cml_use_event_stream_duplicate(
            json!({
                "use": [
                    { "event_stream": ["started", "started"], "from" : "parent" },
                ]
            }),
            Err(Error::Parse { err, .. }) if err.starts_with("invalid value: array with duplicate element, expected a name or nonempty array of names, with unique elements")
        ),
        test_cml_use_event_stream_overlapping_path(
            json!({
                "use": [
                    { "directory": "foobarbaz", "path": "/foo/bar/baz", "rights": [ "r*" ] },
                    {
                        "event_stream": ["started"],
                        "path": "/foo/bar/baz/er",
                        "from": "parent",
                    },
                ],
            }),
            Err(Error::ValidateContexts { err, .. }) if &err == "directory \"/foo/bar/baz\" is a prefix of \"use\" target event_stream \"/foo/bar/baz/er\""
        ),
        test_cml_use_event_stream_invalid_path(
            json!({
                "use": [
                    {
                        "event_stream": ["started"],
                        "path": "my_stream",
                        "from": "parent",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if err.starts_with("invalid value: string \"my_stream\", expected a path with leading `/` and non-empty segments, where each segment is no more than fuchsia.io/MAX_NAME_LENGTH bytes in length, cannot be . or .., and cannot contain embedded NULs")
        ),

        test_cml_offer_bad_subdir(
            json!({
                "offer": [
                    {
                        "directory": "index",
                        "subdir": "/",
                        "from": "parent",
                        "to": [ "#modular" ],
                    },
                ],
                "children": [
                    {
                        "name": "modular",
                        "url": "fuchsia-pkg://fuchsia.com/modular#meta/modular.cm"
                    }
                ]
            }),
            Err(Error::Parse { err, .. }) if err.starts_with("invalid value: string \"/\", expected a path with no leading `/` and non-empty segments")
        ),

        test_cml_program(
            json!(
                {
                    "program": {
                        "runner": "elf",
                        "binary": "bin/app",
                    },
                }
            ),
            Ok(())
        ),

        test_cml_program_use_runner(
            json!(
                {
                    "program": {
                        "binary": "bin/app",
                    },
                    "use": [
                        { "runner": "elf", "from": "parent" }
                    ]
                }
            ),
            Ok(())
        ),

        test_cml_program_use_runner_conflict(
            json!(
                {
                    "program": {
                        "runner": "elf",
                        "binary": "bin/app",
                    },
                    "use": [
                        { "runner": "elf", "from": "parent" }
                    ]
                }
            ),
            Err(Error::ValidateContext { err, .. }) if &err ==
                "Component has conflicting runners in `program` block and `use` block."
        ),

        test_cml_program_no_runner(
            json!({"program": { "binary": "bin/app" }}),
            Err(Error::ValidateContext { err, .. }) if &err ==
                "Component has a `program` block defined, but doesn't specify a `runner`. \
                Components need to use a runner to actually execute code."
        ),

    }

    test_validate_cml_with_context! {
        // include
        test_cml_empty_include(
            json!(
                {
                    "include": [],
                }
            ),
            Ok(())
        ),
        test_cml_some_include(
            json!(
                {
                    "include": [ "some.cml" ],
                }
            ),
            Ok(())
        ),
        test_cml_couple_of_include(
            json!(
                {
                    "include": [ "some1.cml", "some2.cml" ],
                }
            ),
            Ok(())
        ),

        // use
        test_cml_use(
            json!({
                "use": [
                    { "protocol": "CoolFonts", "path": "/svc/MyFonts" },
                    { "protocol": "CoolFonts2", "path": "/svc/MyFonts2", "from": "parent/dict" },
                    { "protocol": "fuchsia.test.hub.HubReport", "from": "framework" },
                    { "protocol": "fuchsia.sys2.StorageAdmin", "from": "#data-storage" },
                    { "protocol": ["fuchsia.ui.scenic.Scenic", "fuchsia.logger.LogSink"] },
                    {
                        "directory": "assets",
                        "path": "/data/assets",
                        "rights": ["rw*"],
                    },
                    {
                        "directory": "config",
                        "from": "parent",
                        "path": "/data/config",
                        "rights": ["rx*"],
                        "subdir": "fonts/all",
                    },
                    { "storage": "data", "path": "/example" },
                    { "storage": "cache", "path": "/tmp" },
                    {
                        "event_stream": ["started", "stopped", "running"],
                        "scope":["#test"],
                        "path":"/svc/testpath",
                        "from":"parent",
                    },
                    { "runner": "usain", "from": "parent" },
                ],
                "capabilities": [
                    {
                        "storage": "data-storage",
                        "from": "parent",
                        "backing_dir": "minfs",
                        "storage_id": "static_instance_id_or_moniker",
                    }
                ]
            }),
            Ok(())
        ),

        test_cml_offer_event_stream_capability_requested_not_from_framework(
            json!({
                "offer": [
                    {
                        "event_stream": ["capability_requested", "stopped"],
                        "from" : "parent",
                        "to": "#something"
                    },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"#something\" is an \"offer\" target from \"parent\" but \"#something\" does not appear in \"children\" or \"collections\""
        ),
        test_cml_offer_event_stream_capability_requested_with_filter(
            json!({
                "offer": [
                    {
                        "event_stream": "capability_requested",
                        "from" : "framework",
                        "to": "#something",
                    },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"#something\" is an \"offer\" target from \"framework\" but \"#something\" does not appear in \"children\" or \"collections\""
        ),
        test_cml_offer_event_stream_multiple_as(
            json!({
                "offer": [
                    {
                        "event_stream": ["started", "stopped"],
                        "from" : "framework",
                        "to": "#self",
                        "as": "something"
                    },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "as cannot be used with multiple events"
        ),
        test_cml_offer_event_stream_from_anything_else(
            json!({
                "offer": [
                    {
                        "event_stream": ["started", "stopped"],
                        "from" : "framework",
                        "to": "#self"
                    },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"#self\" is an \"offer\" target from \"framework\" but \"#self\" does not appear in \"children\" or \"collections\""
        ),
        test_cml_expose_event_stream_scope_invalid_component(
            json!({
                "expose": [
                    {
                        "event_stream": ["started", "stopped"],
                        "from" : "framework",
                        "scope":["#invalid_component"]
                    },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "event_stream scope invalid_component did not match a component or collection in this .cml file."
        ),
        test_cml_use_invalid_from(
            json!({
                "use": [
                  { "protocol": "CoolFonts", "from": "bad" }
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"bad\", expected \"parent\", \"framework\", \"debug\", \"self\", \"#<capability-name>\", \"#<child-name>\", \"#<collection-name>\", dictionary path, or none"
        ),
        test_cml_use_invalid_from_dictionary(
            json!({
                "use": [
                  { "protocol": "CoolFonts", "from": "bad/dict" }
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"bad/dict\", expected \"parent\", \"framework\", \"debug\", \"self\", \"#<capability-name>\", \"#<child-name>\", \"#<collection-name>\", dictionary path, or none"
        ),
        test_cml_use_from_missing_capability(
            json!({
                "use": [
                  { "protocol": "fuchsia.sys2.Admin", "from": "#mystorage" }
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"use\" source \"#mystorage\" does not appear in \"children\" or \"capabilities\""
        ),
        test_cml_use_bad_path(
            json!({
                "use": [
                    {
                        "protocol": ["CoolFonts", "FunkyFonts"],
                        "path": "/MyFonts"
                    }
                ]
            }),
            Err(Error::ValidateContexts { err, .. }) if &err == "\"path\" can only be specified when one `protocol` is supplied."
        ),
        test_cml_use_empty_protocols(
            json!({
                "use": [
                    {
                        "protocol": [],
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 0, expected a name or nonempty array of names, with unique elements"
        ),
        test_cml_use_bad_subdir(
            json!({
                "use": [
                  {
                    "directory": "config",
                    "path": "/config",
                    "from": "parent",
                    "rights": [ "r*" ],
                    "subdir": "/",
                  },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"/\", expected a path with no leading `/` and non-empty segments"
        ),
        test_cml_use_resolver_fails(
            json!({
                "use": [
                    {
                        "resolver": "pkg_resolver",
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if err.starts_with("unknown field `resolver`, expected one of")
        ),
        test_cml_use_disallows_pkg_conflicts_for_directories(
            json!({
                "use": [
                    { "directory": "dir", "path": "/pkg/dir", "rights": [ "r*" ] },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "directory \"/pkg/dir\" conflicts with the protected path \"/pkg\", please use this capability with a different path"
        ),
        test_cml_use_disallows_pkg_conflicts_for_protocols(
            json!({
                "use": [
                    { "protocol": "prot", "path": "/pkg/protocol" },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "protocol \"/pkg/protocol\" conflicts with the protected path \"/pkg\", please use this capability with a different path"
        ),
        test_cml_use_disallows_pkg_conflicts_for_storage(
            json!({
                "use": [
                    { "storage": "store", "path": "/pkg/storage" },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "storage \"/pkg/storage\" conflicts with the protected path \"/pkg\", please use this capability with a different path"
        ),
        test_cml_use_from_parent_weak(
            json!({
                "use": [
                    {
                        "protocol": "fuchsia.parent.Protocol",
                        "from": "parent",
                        "dependency": "weak",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "Only `use` from children can have dependency: \"weak\""
        ),
        test_cml_use_from_child_weak(
            json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    },
                ],
                "use": [
                    {
                        "protocol": "fuchsia.child.Protocol",
                        "from": "#logger",
                        "dependency": "weak",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_use_from_child_dictionary_weak(
            json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    },
                ],
                "use": [
                    {
                        "protocol": "fuchsia.child.Protocol",
                        "from": "#logger/dictionary",
                        "dependency": "weak",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_use_numbered_handle_from_same_protocol_ok(
            json!({
                "use": [
                    {
                        "protocol": "foo",
                    },
                    {
                        "protocol": "foo",
                        "numbered_handle": 0xab,
                    },
                    {
                        "protocol": "foo",
                        "numbered_handle": 0xac,
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_use_numbered_handle_not_a_number(
            json!({
                "use": [
                    {
                        "protocol": "foo",
                        "numbered_handle": "0xab",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "error parsing number"
        ),
        test_cml_use_numbered_handle_out_of_range(
            json!({
                "use": [
                    {
                        "protocol": "foo",
                        "numbered_handle": 256,
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: integer `256`, expected a uint8 from zircon/processargs.h"
        ),

        // expose
        test_cml_expose(
            json!({
                "expose": [
                    {
                        "protocol": "A",
                        "from": "self",
                    },
                    {
                        "protocol": ["B", "C"],
                        "from": "self",
                    },
                    {
                        "protocol": "D",
                        "from": "#mystorage",
                    },
                    {
                        "directory": "blobfs",
                        "from": "self",
                        "rights": ["r*"],
                        "subdir": "blob",
                    },
                    { "directory": "data", "from": "framework" },
                    { "runner": "elf", "from": "#logger",  },
                    { "resolver": "pkg_resolver", "from": "#logger" },
                ],
                "capabilities": [
                    { "protocol": ["A", "B", "C"] },
                    {
                        "directory": "blobfs",
                        "path": "/blobfs",
                        "rights": ["rw*"],
                    },
                    {
                        "storage": "mystorage",
                        "from": "self",
                        "backing_dir": "blobfs",
                        "storage_id": "static_instance_id_or_moniker",
                    }
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_expose_all_valid_chars(
            json!({
                "expose": [
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#abcdefghijklmnopqrstuvwxyz0123456789_-.",
                    },
                ],
                "children": [
                    {
                        "name": "abcdefghijklmnopqrstuvwxyz0123456789_-.",
                        "url": "https://www.google.com/gmail"
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_expose_missing_props(
            json!({
                "expose": [ {} ]
            }),
            Err(Error::Parse { err, .. }) if &err == "missing field `from`"
        ),
        test_cml_expose_missing_from(
            json!({
                "expose": [
                    { "protocol": "fuchsia.logger.Log", "from": "#missing" },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"expose\" source \"#missing\" does not appear in \"children\" or \"capabilities\""
        ),
        test_cml_expose_duplicate_target_names(
            json!({
                "capabilities": [
                    { "protocol": "logger" },
                ],
                "expose": [
                    { "protocol": "logger", "from": "self", "as": "thing" },
                    { "directory": "thing", "from": "#child" , "rights": ["rx*"] },
                ],
                "children": [
                    {
                        "name": "child",
                        "url": "fuchsia-pkg://fuchsia.com/pkg#comp.cm",
                    },
                ],
            }),
            Err(Error::ValidateContexts { err, .. }) if &err == "\"thing\" is a duplicate \"expose\" target capability for \"parent\""
        ),
        test_cml_expose_invalid_multiple_from(
            json!({
                    "capabilities": [
                        { "protocol": "fuchsia.logger.Log" },
                    ],
                    "expose": [
                        {
                            "protocol": "fuchsia.logger.Log",
                            "from": [ "self", "#logger" ],
                        },
                    ],
                    "children": [
                        {
                            "name": "logger",
                            "url": "fuchsia-pkg://fuchsia.com/logger#meta/logger.cm",
                        },
                    ]
                }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"protocol\" capabilities cannot have multiple \"from\" clauses"
        ),
        test_cml_expose_from_missing_named_source(
            json!({
                    "expose": [
                        {
                            "protocol": "fuchsia.logger.Log",
                            "from": "#does-not-exist",
                        },
                    ],
                }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"expose\" source \"#does-not-exist\" does not appear in \"children\" or \"capabilities\""
        ),
        test_cml_expose_bad_from(
            json!({
                "expose": [ {
                    "protocol": "fuchsia.logger.Log", "from": "parent"
                } ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"parent\", expected one or an array of \"framework\", \"self\", \"#<child-name>\", or a dictionary path"
        ),
        // if "as" is specified, only 1 array item is allowed.
        test_cml_expose_bad_as(
            json!({
                "expose": [
                    {
                        "protocol": ["A", "B"],
                        "from": "#echo_server",
                        "as": "thing"
                    },
                ],
                "children": [
                    {
                        "name": "echo_server",
                        "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm"
                    }
                ]
            }),
            Err(Error::ValidateContexts { err, .. }) if &err == "\"as\" can only be specified when one `protocol` is supplied."
        ),
        test_cml_expose_empty_protocols(
            json!({
                "expose": [
                    {
                        "protocol": [],
                        "from": "#child",
                        "as": "thing"
                    },
                ],
                "children": [
                    {
                        "name": "child",
                        "url": "fuchsia-pkg://fuchsia.com/pkg#comp.cm",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 0, expected a name or nonempty array of names, with unique elements"
        ),
        test_cml_expose_bad_subdir(
            json!({
                "expose": [
                    {
                        "directory": "blobfs",
                        "from": "self",
                        "rights": ["r*"],
                        "subdir": "/",
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"/\", expected a path with no leading `/` and non-empty segments"
        ),
        test_cml_expose_from_dictionary_invalid(
            json!({
                "expose": [
                    {
                        "protocol": "pkg_protocol",
                        "from": "bad/a",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"bad/a\", expected one or an array of \"framework\", \"self\", \"#<child-name>\", or a dictionary path"
        ),
        // offer
        test_cml_offer(
            json!({
                "offer": [
                    {
                        "protocol": "fuchsia.fonts.LegacyProvider",
                        "from": "parent",
                        "to": [ "#echo_server" ],
                        "dependency": "weak"
                    },
                    {
                        "protocol": "fuchsia.sys2.StorageAdmin",
                        "from": "#data",
                        "to": [ "#echo_server" ]
                    },
                    {
                        "protocol": [
                            "fuchsia.settings.Accessibility",
                            "fuchsia.ui.scenic.Scenic"
                        ],
                        "from": "parent",
                        "to": [ "#echo_server" ],
                        "dependency": "strong"
                    },
                    {
                        "directory": "assets",
                        "from": "self",
                        "to": [ "#echo_server" ],
                        "rights": ["r*"]
                    },
                    {
                        "directory": "index",
                        "subdir": "files",
                        "from": "parent",
                        "to": [ "#modular" ],
                        "dependency": "weak"
                    },
                    {
                        "directory": "config",
                        "from": "framework",
                        "to": [ "#modular" ],
                        "as": "config",
                        "dependency": "strong"
                    },
                    {
                        "storage": "data",
                        "from": "self",
                        "to": [ "#modular", "#logger" ]
                    },
                    {
                        "runner": "elf",
                        "from": "parent",
                        "to": [ "#modular", "#logger" ]
                    },
                    {
                        "resolver": "pkg_resolver",
                        "from": "parent",
                        "to": [ "#modular" ],
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    },
                    {
                        "name": "echo_server",
                        "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm"
                    },
                ],
                "collections": [
                    {
                        "name": "modular",
                        "durability": "transient",
                    },
                ],
                "capabilities": [
                    {
                        "directory": "assets",
                        "path": "/data/assets",
                        "rights": [ "rw*" ],
                    },
                    {
                        "storage": "data",
                        "from": "parent",
                        "backing_dir": "minfs",
                        "storage_id": "static_instance_id_or_moniker",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_offer_all_valid_chars(
            json!({
                "offer": [
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#abcdefghijklmnopqrstuvwxyz0123456789_-from",
                        "to": [ "#abcdefghijklmnopqrstuvwxyz0123456789_-to" ],
                    },
                ],
                "children": [
                    {
                        "name": "abcdefghijklmnopqrstuvwxyz0123456789_-from",
                        "url": "https://www.google.com/gmail"
                    },
                    {
                        "name": "abcdefghijklmnopqrstuvwxyz0123456789_-to",
                        "url": "https://www.google.com/gmail"
                    },
                ],
                "capabilities": [
                    {
                        "storage": "abcdefghijklmnopqrstuvwxyz0123456789_-storage",
                        "from": "#abcdefghijklmnopqrstuvwxyz0123456789_-from",
                        "backing_dir": "example",
                        "storage_id": "static_instance_id_or_moniker",
                    }
                ]
            }),
            Ok(())
        ),
        test_cml_offer_singleton_to (
            json!({
                "offer": [
                    {
                        "protocol": "fuchsia.fonts.LegacyProvider",
                        "from": "parent",
                        "to": "#echo_server",
                        "dependency": "weak"
                    },
                ],
                "children": [
                    {
                        "name": "echo_server",
                        "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm"
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_offer_missing_props(
            json!({
                "offer": [ {} ]
            }),
            Err(Error::Parse { err, .. }) if &err == "missing field `from`"
        ),
        test_cml_offer_missing_from(
            json!({
                    "offer": [
                        {
                            "protocol": "fuchsia.logger.Log",
                            "from": "#missing",
                            "to": [ "#echo_server" ],
                        },
                    ],
                    "children": [
                        {
                            "name": "echo_server",
                            "url": "fuchsia-pkg://fuchsia.com/echo_server#meta/echo_server.cm",
                        },
                    ],
                }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"offer\" source \"#missing\" does not appear in \"children\" or \"capabilities\""
        ),
        test_cml_offer_bad_from(
            json!({
                    "offer": [ {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#invalid@",
                        "to": [ "#echo_server" ],
                    } ]
                }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"#invalid@\", expected one or an array of \"parent\", \"framework\", \"self\", \"#<child-name>\", \"#<collection-name>\", or a dictionary path"
        ),
        test_cml_offer_invalid_multiple_from(
            json!({
                    "offer": [
                        {
                            "protocol": "fuchsia.logger.Log",
                            "from": [ "parent", "#logger" ],
                            "to": [ "#echo_server" ],
                        },
                    ],
                    "children": [
                        {
                            "name": "logger",
                            "url": "fuchsia-pkg://fuchsia.com/logger#meta/logger.cm",
                        },
                        {
                            "name": "echo_server",
                            "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm",
                        },
                    ]
                }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"protocol\" capabilities cannot have multiple \"from\" clauses"
        ),
        test_cml_offer_from_missing_named_source(
            json!({
                    "offer": [
                        {
                            "protocol": "fuchsia.logger.Log",
                            "from": "#does-not-exist",
                            "to": ["#echo_server" ],
                        },
                    ],
                    "children": [
                        {
                            "name": "echo_server",
                            "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm",
                        },
                    ]
                }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"offer\" source \"#does-not-exist\" does not appear in \"children\" or \"capabilities\""
        ),
        test_cml_offer_protocol_from_collection_invalid(
            json!({
                "collections": [ {
                    "name": "coll",
                    "durability": "transient",
                } ],
                "children": [ {
                    "name": "echo_server",
                    "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm",
                } ],
                "offer": [
                    { "protocol": "fuchsia.logger.Log", "from": "#coll", "to": [ "#echo_server" ] },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"offer\" source \"#coll\" does not appear in \"children\" or \"capabilities\""
        ),
        test_cml_offer_directory_from_collection_invalid(
            json!({
                "collections": [ {
                    "name": "coll",
                    "durability": "transient",
                } ],
                "children": [ {
                    "name": "echo_server",
                    "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm",
                } ],
                "offer": [
                    { "directory": "temp", "from": "#coll", "to": [ "#echo_server" ] },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"offer\" source \"#coll\" does not appear in \"children\""
        ),
        test_cml_offer_runner_from_collection_invalid(
            json!({
                "collections": [ {
                    "name": "coll",
                    "durability": "transient",
                } ],
                "children": [ {
                    "name": "echo_server",
                    "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm",
                } ],
                "offer": [
                    { "runner": "elf", "from": "#coll", "to": [ "#echo_server" ] },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"offer\" source \"#coll\" does not appear in \"children\""
        ),
        test_cml_offer_resolver_from_collection_invalid(
            json!({
                "collections": [ {
                    "name": "coll",
                    "durability": "transient",
                } ],
                "children": [ {
                    "name": "echo_server",
                    "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm",
                } ],
                "offer": [
                    { "resolver": "base", "from": "#coll", "to": [ "#echo_server" ] },
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"offer\" source \"#coll\" does not appear in \"children\""
        ),
        test_cml_offer_from_dictionary_invalid(
            json!({
                "offer": [
                    {
                        "protocol": "pkg_protocol",
                        "from": "bad/a",
                        "to": "#child",
                    },
                ],
                "children": [
                    {
                        "name": "child",
                        "url": "fuchsia-pkg://child",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"bad/a\", expected one or an array of \"parent\", \"framework\", \"self\", \"#<child-name>\", \"#<collection-name>\", or a dictionary path"
        ),
        test_cml_offer_to_non_dictionary(
            json!({
                "offer": [
                    {
                        "protocol": "p",
                        "from": "parent",
                        "to": "self/dict",
                    },
                ],
                "capabilities": [
                    {
                        "protocol": "dict",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"offer\" has dictionary target \
            \"self/dict\" but \"dict\" is not a dictionary capability defined by \
            this component"
        ),

        test_cml_offer_empty_targets(
            json!({
                "offer": [
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#child",
                        "to": []
                    },
                ],
                "children": [
                    {
                        "name": "child",
                        "url": "fuchsia-pkg://fuchsia.com/pkg#comp.cm",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 0, expected one or an array of \"#<child-name>\", \"#<collection-name>\", or \"self/<dictionary>\", with unique elements"
        ),
        test_cml_offer_duplicate_targets(
            json!({
                "offer": [ {
                    "protocol": "fuchsia.logger.Log",
                    "from": "#logger",
                    "to": ["#a", "#a"]
                } ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: array with duplicate element, expected one or an array of \"#<child-name>\", \"#<collection-name>\", or \"self/<dictionary>\", with unique elements"
        ),
        test_cml_offer_target_missing_props(
            json!({
                "offer": [ {
                    "protocol": "fuchsia.logger.Log",
                    "from": "#logger",
                    "as": "fuchsia.logger.SysLog",
                } ]
            }),
            Err(Error::Parse { err, .. }) if &err == "missing field `to`"
        ),
        test_cml_offer_target_missing_to(
            json!({
                "offer": [ {
                    "protocol": "fuchsia.logger.Log",
                    "from": "#logger",
                    "to": [ "#missing" ],
                } ],
                "children": [ {
                    "name": "logger",
                    "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                } ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"#missing\" is an \"offer\" target from \"#logger\" but \"#missing\" does not appear in \"children\" or \"collections\""
        ),
        test_cml_offer_target_bad_to(
            json!({
                "offer": [ {
                    "protocol": "fuchsia.logger.Log",
                    "from": "#logger",
                    "to": [ "self" ],
                    "as": "fuchsia.logger.SysLog",
                } ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"self\", expected \"#<child-name>\", \"#<collection-name>\", or \"self/<dictionary>\""
        ),
        test_cml_offer_empty_protocols(
            json!({
                "offer": [
                    {
                        "protocol": [],
                        "from": "parent",
                        "to": [ "#echo_server" ],
                        "as": "thing"
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 0, expected a name or nonempty array of names, with unique elements"
        ),
        test_cml_offer_target_equals_from(
            json!({
                "children": [
                    {
                        "name": "child",
                        "url": "fuchsia-pkg://fuchsia.com/child#meta/child.cm",
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.example.Protocol",
                        "from": "#child",
                        "to": [ "#child" ],
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "Offer target \"#child\" is same as source"
        ),
        test_cml_offer_target_equals_from_weak(
            json!({
                "children": [
                    {
                        "name": "child",
                        "url": "fuchsia-pkg://fuchsia.com/child#meta/child.cm",
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.example.Protocol",
                        "from": "#child",
                        "to": [ "#child" ],
                        "dependency": "weak",
                    },
                    {
                        "directory": "data",
                        "from": "#child",
                        "to": [ "#child" ],
                        "dependency": "weak",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_storage_offer_target_equals_from(
            json!({
                "offer": [ {
                    "storage": "minfs",
                    "from": "self",
                    "to": [ "#logger" ],
                } ],
                "children": [ {
                    "name": "logger",
                    "url": "fuchsia-pkg://fuchsia.com/logger#meta/logger.cm",
                } ],
                "capabilities": [ {
                    "storage": "minfs",
                    "from": "#logger",
                    "backing_dir": "minfs-dir",
                    "storage_id": "static_instance_id_or_moniker",
                } ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "Storage offer target \"#logger\" is same as source"
        ),
        test_cml_offer_duplicate_target_names(
            json!({
                "offer": [
                    {
                        "protocol": "logger",
                        "from": "parent",
                        "to": [ "#echo_server" ],
                        "as": "thing"
                    },
                    {
                        "protocol": "logger",
                        "from": "parent",
                        "to": [ "#scenic" ],
                    },
                    {
                        "directory": "thing",
                        "from": "parent",
                        "to": [ "#echo_server" ],
                    }
                ],
                "children": [
                    {
                        "name": "scenic",
                        "url": "fuchsia-pkg://fuchsia.com/scenic/stable#meta/scenic.cm"
                    },
                    {
                        "name": "echo_server",
                        "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm"
                    },
                ],
            }),
            Err(Error::ValidateContexts { err, .. }) if &err == "\"thing\" is a duplicate \"offer\" target capability for \"#echo_server\""
        ),
        test_cml_offer_duplicate_storage_names(
            json!({
                "offer": [
                    {
                        "storage": "cache",
                        "from": "parent",
                        "to": [ "#echo_server" ]
                    },
                    {
                        "storage": "cache",
                        "from": "self",
                        "to": [ "#echo_server" ]
                    }
                ],
                "capabilities": [ {
                    "storage": "cache",
                    "from": "self",
                    "backing_dir": "minfs",
                    "storage_id": "static_instance_id_or_moniker",
                } ],
                "children": [ {
                    "name": "echo_server",
                    "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm"
                } ]
            }),
            Err(Error::ValidateContexts { err, .. }) if &err == "\"cache\" is a duplicate \"offer\" target capability for \"#echo_server\""
        ),
        // if "as" is specified, only 1 array item is allowed.
        test_cml_offer_bad_as(
            json!({
                "offer": [
                    {
                        "protocol": ["A", "B"],
                        "from": "parent",
                        "to": [ "#echo_server" ],
                        "as": "thing"
                    },
                ],
                "children": [
                    {
                        "name": "echo_server",
                        "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm"
                    }
                ]
            }),
            Err(Error::ValidateContexts { err, .. }) if &err == "\"as\" can only be specified when one `protocol` is supplied."
        ),
        test_cml_offer_dependency_on_wrong_type(
            json!({
                    "offer": [ {
                        "resolver": "fuchsia.logger.Log",
                        "from": "parent",
                        "to": [ "#echo_server" ],
                        "dependency": "strong",
                    } ],
                    "children": [ {
                        "name": "echo_server",
                        "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo_server.cm",
                    } ],
                }),
            Err(Error::ValidateContext { err, .. }) if err.starts_with("Dependency can only be provided for")
        ),

        // children
        test_cml_children(
            json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                        "on_terminate": "reboot",
                    },
                    {
                        "name": "gmail",
                        "url": "https://www.google.com/gmail",
                        "startup": "eager",
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_children_missing_props(
            json!({
                "children": [ {} ]
            }),
            Err(Error::Parse { err, .. }) if &err == "missing field `name`"
        ),
        test_cml_children_duplicate_names(
            json!({
                "children": [
                     {
                         "name": "logger",
                         "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                     },
                     {
                         "name": "logger",
                         "url": "fuchsia-pkg://fuchsia.com/logger/beta#meta/logger.cm"
                     }
                 ]
             }),
             Err(Error::Validate { err, .. }) if &err == "identifier \"logger\" is defined twice, once in \"children\" and once in \"children\""
         ),
          test_cml_children_bad_startup(
            json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                        "startup": "zzz",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "unknown variant `zzz`, expected `lazy` or `eager`"
        ),
        test_cml_children_bad_on_terminate(
            json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                        "on_terminate": "zzz",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "unknown variant `zzz`, expected `none` or `reboot`"
        ),
        // collections
        test_cml_collections(
            json!({
                "collections": [
                    {
                        "name": "test_single_run_coll",
                        "durability": "single_run"
                    },
                    {
                        "name": "test_transient_coll",
                        "durability": "transient"
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_collections_missing_props(
            json!({
                "collections": [ {} ]
            }),
            Err(Error::Parse { err, .. }) if &err == "missing field `name`"
        ),
        test_cml_collections_duplicate_names(
           json!({
               "collections": [
                    {
                        "name": "duplicate",
                        "durability": "single_run"
                    },
                    {
                        "name": "duplicate",
                        "durability": "transient"
                    }
                ]
            }),
            Err(Error::Validate { err, .. }) if &err == "identifier \"duplicate\" is defined twice, once in \"collections\" and once in \"collections\""
        ),
        test_cml_collections_bad_durability(
            json!({
                "collections": [
                    {
                        "name": "modular",
                        "durability": "zzz",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "unknown variant `zzz`, expected `transient` or `single_run`"
        ),

        // capabilities
        test_cml_protocol(
            json!({
                "capabilities": [
                    {
                        "protocol": "a",
                        "path": "/minfs",
                    },
                    {
                        "protocol": "b",
                        "path": "/data",
                    },
                    {
                        "protocol": "c",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_protocol_multi(
            json!({
                "capabilities": [
                    {
                        "protocol": ["a", "b", "c"],
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_protocol_all_valid_chars(
            json!({
                "capabilities": [
                    {
                        "protocol": "abcdefghijklmnopqrstuvwxyz0123456789_-service",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_directory(
            json!({
                "capabilities": [
                    {
                        "directory": "a",
                        "path": "/minfs",
                        "rights": ["connect"],
                    },
                    {
                        "directory": "b",
                        "path": "/data",
                        "rights": ["connect"],
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_directory_all_valid_chars(
            json!({
                "capabilities": [
                    {
                        "directory": "abcdefghijklmnopqrstuvwxyz0123456789_-service",
                        "path": "/data",
                        "rights": ["connect"],
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_storage(
            json!({
                "capabilities": [
                    {
                        "storage": "a",
                        "from": "#minfs",
                        "backing_dir": "minfs",
                        "storage_id": "static_instance_id",
                    },
                    {
                        "storage": "b",
                        "from": "parent",
                        "backing_dir": "data",
                        "storage_id": "static_instance_id_or_moniker",
                    },
                    {
                        "storage": "c",
                        "from": "self",
                        "backing_dir": "storage",
                        "storage_id": "static_instance_id_or_moniker",
                    },
                ],
                "children": [
                    {
                        "name": "minfs",
                        "url": "fuchsia-pkg://fuchsia.com/minfs/stable#meta/minfs.cm",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_storage_all_valid_chars(
            json!({
                "capabilities": [
                    {
                        "storage": "abcdefghijklmnopqrstuvwxyz0123456789_-storage",
                        "from": "#abcdefghijklmnopqrstuvwxyz0123456789_-from",
                        "backing_dir": "example",
                        "storage_id": "static_instance_id_or_moniker",
                    },
                ],
                "children": [
                    {
                        "name": "abcdefghijklmnopqrstuvwxyz0123456789_-from",
                        "url": "https://www.google.com/gmail",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_storage_invalid_from(
            json!({
                    "capabilities": [ {
                        "storage": "minfs",
                        "from": "#missing",
                        "backing_dir": "minfs",
                        "storage_id": "static_instance_id_or_moniker",
                    } ]
                }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"capabilities\" source \"#missing\" does not appear in \"children\""
        ),
        test_cml_runner(
            json!({
                "capabilities": [
                    {
                        "runner": "a",
                        "path": "/minfs",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_runner_all_valid_chars(
            json!({
                "children": [
                    {
                        "name": "abcdefghijklmnopqrstuvwxyz0123456789_-from",
                        "url": "https://www.google.com/gmail"
                    },
                ],
                "capabilities": [
                    {
                        "runner": "abcdefghijklmnopqrstuvwxyz0123456789_-runner",
                        "path": "/example",
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_capabilities_duplicates(
            json!({
                "capabilities": [
                    {
                        "runner": "pkg_resolver",
                        "path": "/svc/fuchsia.component.resolution.Resolver",
                    },
                    {
                        "resolver": "pkg_resolver",
                        "path": "/svc/my-resolver",
                    },
                ]
            }),
            Err(Error::Validate { err, .. }) if &err == "identifier \"pkg_resolver\" is defined twice, once in \"resolver\" and once in \"runner\""
        ),

        // environments
        test_cml_environments(
            json!({
                "environments": [
                    {
                        "name": "my_env_a",
                    },
                    {
                        "name": "my_env_b",
                        "extends": "realm",
                    },
                    {
                        "name": "my_env_c",
                        "extends": "none",
                        "__stop_timeout_ms": 8000,
                    },
                ],
            }),
            Ok(())
        ),

        test_invalid_cml_environment_no_stop_timeout(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "none",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err ==
                "'__stop_timeout_ms' must be provided if the environment extends 'none'"
        ),

        test_cml_environment_invalid_extends(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "some_made_up_string",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "unknown variant `some_made_up_string`, expected `realm` or `none`"
        ),
        test_cml_environment_missing_props(
            json!({
                "environments": [ {} ]
            }),
            Err(Error::Parse { err, .. }) if &err == "missing field `name`"
        ),

        test_cml_environment_with_runners(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "realm",
                        "runners": [
                            {
                                "runner": "dart",
                                "from": "parent",
                            }
                        ]
                    }
                ],
            }),
            Ok(())
        ),
        test_cml_environment_with_runners_alias(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "realm",
                        "runners": [
                            {
                                "runner": "dart",
                                "from": "parent",
                                "as": "my-dart",
                            }
                        ]
                    }
                ],
            }),
            Ok(())
        ),
        test_cml_environment_with_runners_missing(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "realm",
                        "runners": [
                            {
                                "runner": "dart",
                                "from": "self",
                            }
                        ]
                    }
                ],
                "capabilities": [
                     {
                         "runner": "dart",
                         "path": "/svc/fuchsia.component.Runner",
                     }
                ],
            }),
            Ok(())
        ),
        test_cml_environment_with_runners_bad_name(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "realm",
                        "runners": [
                            {
                                "runner": "elf",
                                "from": "parent",
                                "as": "#elf",
                            }
                        ]
                    }
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"#elf\", expected a \
            name that consists of [A-Za-z0-9_.-] and starts with [A-Za-z0-9_]"
        ),
        test_cml_environment_with_runners_duplicate_name(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "realm",
                        "runners": [
                            {
                                "runner": "dart",
                                "from": "parent",
                            },
                            {
                                "runner": "other-dart",
                                "from": "parent",
                                "as": "dart",
                            }
                        ]
                    }
                ],
            }),
            Err(Error::ValidateContexts { err, .. }) if &err == "Duplicate runners registered under name \"dart\": \"other-dart\" and \"dart\"."
        ),
        test_cml_environment_with_runner_from_missing_child(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "realm",
                        "runners": [
                            {
                                "runner": "elf",
                                "from": "#missing_child",
                            }
                        ]
                    }
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"elf\" runner source \"#missing_child\" does not appear in \"children\""
        ),
        test_cml_environment_with_resolvers(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "realm",
                        "resolvers": [
                            {
                                "resolver": "pkg_resolver",
                                "from": "parent",
                                "scheme": "fuchsia-pkg",
                            }
                        ]
                    }
                ],
            }),
            Ok(())
        ),
        test_cml_environment_with_resolvers_bad_scheme(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "realm",
                        "resolvers": [
                            {
                                "resolver": "pkg_resolver",
                                "from": "parent",
                                "scheme": "9scheme",
                            }
                        ]
                    }
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"9scheme\", expected a valid URL scheme"
        ),
        test_cml_environment_with_resolvers_duplicate_scheme(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "realm",
                        "resolvers": [
                            {
                                "resolver": "pkg_resolver",
                                "from": "parent",
                                "scheme": "fuchsia-pkg",
                            },
                            {
                                "resolver": "base_resolver",
                                "from": "parent",
                                "scheme": "fuchsia-pkg",
                            }
                        ]
                    }
                ],
            }),
            Err(Error::ValidateContexts { err, .. }) if &err == "scheme \"fuchsia-pkg\" for resolver \"base_resolver\" is already registered to \"pkg_resolver\"."
        ),
        test_cml_environment_with_resolver_from_missing_child(
            json!({
                "environments": [
                    {
                        "name": "my_env",
                        "extends": "realm",
                        "resolvers": [
                            {
                                "resolver": "pkg_resolver",
                                "from": "#missing_child",
                                "scheme": "fuchsia-pkg",
                            }
                        ]
                    }
                ]
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"pkg_resolver\" resolver source \"#missing_child\" does not appear in \"children\""
        ),

        // constraints
        test_cml_rights_all(
            json!({
                "use": [
                  {
                    "directory": "mydir",
                    "path": "/mydir",
                    "rights": ["connect", "enumerate", "read_bytes", "write_bytes",
                               "execute", "update_attributes", "get_attributes", "traverse",
                               "modify_directory"],
                  },
                ]
            }),
            Ok(())
        ),
        test_cml_rights_invalid(
            json!({
                "use": [
                  {
                    "directory": "mydir",
                    "path": "/mydir",
                    "rights": ["cAnnect", "enumerate"],
                  },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "unknown variant `cAnnect`, expected one of `connect`, `enumerate`, `execute`, `get_attributes`, `modify_directory`, `read_bytes`, `traverse`, `update_attributes`, `write_bytes`, `r*`, `w*`, `x*`, `rw*`, `rx*`"
        ),
        test_cml_rights_duplicate(
            json!({
                "use": [
                  {
                    "directory": "mydir",
                    "path": "/mydir",
                    "rights": ["connect", "connect"],
                  },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: array with duplicate element, expected a nonempty array of rights, with unique elements"
        ),
        test_cml_rights_empty(
            json!({
                "use": [
                  {
                    "directory": "mydir",
                    "path": "/mydir",
                    "rights": [],
                  },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 0, expected a nonempty array of rights, with unique elements"
        ),
        test_cml_rights_alias_star_expansion(
            json!({
                "use": [
                  {
                    "directory": "mydir",
                    "rights": ["r*"],
                    "path": "/mydir",
                  },
                ]
            }),
            Ok(())
        ),
        test_cml_rights_alias_star_expansion_with_longform(
            json!({
                "use": [
                  {
                    "directory": "mydir",
                    "rights": ["w*", "read_bytes"],
                    "path": "/mydir",
                  },
                ]
            }),
            Ok(())
        ),

        test_cml_path(
            json!({
                "capabilities": [
                    {
                        "protocol": "foo",
                        "path": "/foo/in.-_/Bar",
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_path_invalid_empty(
            json!({
                "capabilities": [
                    { "protocol": "foo", "path": "" },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 0, expected a non-empty path no more than fuchsia.io/MAX_PATH_LENGTH bytes in length"
        ),
        test_cml_path_invalid_root(
            json!({
                "capabilities": [
                    { "protocol": "foo", "path": "/" },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"/\", expected a path with leading `/` and non-empty segments, where each segment is no more than fuchsia.io/MAX_NAME_LENGTH bytes in length, cannot be . or .., and cannot contain embedded NULs"
        ),
        test_cml_path_invalid_absolute_is_relative(
            json!({
                "capabilities": [
                    { "protocol": "foo", "path": "foo/bar" },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"foo/bar\", expected a path with leading `/` and non-empty segments, where each segment is no more than fuchsia.io/MAX_NAME_LENGTH bytes in length, cannot be . or .., and cannot contain embedded NULs"
        ),
        test_cml_path_invalid_trailing(
            json!({
                "capabilities": [
                    { "protocol": "foo", "path":"/foo/bar/" },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"/foo/bar/\", expected a path with leading `/` and non-empty segments, where each segment is no more than fuchsia.io/MAX_NAME_LENGTH bytes in length, cannot be . or .., and cannot contain embedded NULs"
        ),
        test_cml_path_too_long(
            json!({
                "capabilities": [
                    { "protocol": "foo", "path": format!("/{}", "a".repeat(4095)) },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 4096, expected a non-empty path no more than fuchsia.io/MAX_PATH_LENGTH bytes in length"
        ),
        test_cml_path_invalid_segment(
            json!({
                "capabilities": [
                    { "protocol": "foo", "path": "/foo/../bar" },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"/foo/../bar\", expected a path with leading `/` and non-empty segments, where each segment is no more than fuchsia.io/MAX_NAME_LENGTH bytes in length, cannot be . or .., and cannot contain embedded NULs"
        ),
        test_cml_relative_path(
            json!({
                "use": [
                    {
                        "directory": "foo",
                        "path": "/foo",
                        "rights": ["r*"],
                        "subdir": "Baz/Bar",
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_relative_path_invalid_empty(
            json!({
                "use": [
                    {
                        "directory": "foo",
                        "path": "/foo",
                        "rights": ["r*"],
                        "subdir": "",
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 0, expected a non-empty path no more than fuchsia.io/MAX_PATH_LENGTH characters in length"
        ),
        test_cml_relative_path_invalid_root(
            json!({
                "use": [
                    {
                        "directory": "foo",
                        "path": "/foo",
                        "rights": ["r*"],
                        "subdir": "/",
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"/\", expected a path with no leading `/` and non-empty segments"
        ),
        test_cml_relative_path_invalid_absolute(
            json!({
                "use": [
                    {
                        "directory": "foo",
                        "path": "/foo",
                        "rights": ["r*"],
                        "subdir": "/bar",
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"/bar\", expected a path with no leading `/` and non-empty segments"
        ),
        test_cml_relative_path_invalid_trailing(
            json!({
                "use": [
                    {
                        "directory": "foo",
                        "path": "/foo",
                        "rights": ["r*"],
                        "subdir": "bar/",
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"bar/\", expected a path with no leading `/` and non-empty segments"
        ),
        test_cml_relative_path_too_long(
            json!({
                "use": [
                    {
                        "directory": "foo",
                        "path": "/foo",
                        "rights": ["r*"],
                        "subdir": format!("{}", "a".repeat(4096)),
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 4096, expected a non-empty path no more than fuchsia.io/MAX_PATH_LENGTH characters in length"
        ),
        test_cml_relative_ref_too_long(
            json!({
                "expose": [
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": &format!("#{}", "a".repeat(256)),
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 257, expected one or an array of \"framework\", \"self\", \"#<child-name>\", or a dictionary path"
        ),
        test_cml_dictionary_ref_invalid_root(
            json!({
                "use": [
                    {
                        "protocol": "a",
                        "from": "bad/a",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"bad/a\", expected \"parent\", \"framework\", \"debug\", \"self\", \"#<capability-name>\", \"#<child-name>\", \"#<collection-name>\", dictionary path, or none"
        ),
        test_cml_dictionary_ref_invalid_path(
            json!({
                "use": [
                    {
                        "protocol": "a",
                        "from": "parent//a",
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"parent//a\", expected \"parent\", \"framework\", \"debug\", \"self\", \"#<capability-name>\", \"#<child-name>\", \"#<collection-name>\", dictionary path, or none"
        ),
        test_cml_dictionary_ref_too_long(
            json!({
                "use": [
                    {
                        "protocol": "a",
                        "from": format!("parent/{}", "a".repeat(4089)),
                    },
                ],
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 4096, expected \"parent\", \"framework\", \"debug\", \"self\", \"#<capability-name>\", \"#<child-name>\", \"#<collection-name>\", dictionary path, or none"
        ),
        test_cml_capability_name(
            json!({
                "use": [
                    {
                        "protocol": "abcdefghijklmnopqrstuvwxyz0123456789_-.",
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_capability_name_invalid(
            json!({
                "use": [
                    {
                        "protocol": "/bad",
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"/bad\", expected a name or nonempty array of names, with unique elements"
        ),
        test_cml_child_name(
            json!({
                "children": [
                    {
                        "name": "abcdefghijklmnopqrstuvwxyz0123456789_-.",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_child_name_invalid(
            json!({
                "children": [
                    {
                        "name": "/bad",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"/bad\", expected a \
            name that consists of [A-Za-z0-9_.-] and starts with [A-Za-z0-9_]"
        ),
        test_cml_child_name_too_long(
            json!({
                "children": [
                    {
                        "name": "a".repeat(256),
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                    }
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 256, expected a non-empty name no more than 255 characters in length"
        ),
        test_cml_url(
            json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "my+awesome-scheme.2://abc123!@$%.com",
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_url_host_pound_invalid(
            json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "my+awesome-scheme.2://abc123!@#$%.com",
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"my+awesome-scheme.2://abc123!@#$%.com\", expected a valid URL"
        ),
        test_cml_url_invalid(
            json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg",
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"fuchsia-pkg\", expected a valid URL"
        ),
        test_cml_url_too_long(
            json!({
                "children": [
                    {
                        "name": "logger",
                        "url": &format!("fuchsia-pkg://{}", "a".repeat(4083)),
                    },
                ]
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 4097, expected a non-empty URL no more than 4096 characters in length"
        ),
        test_cml_duplicate_identifiers_children_collection(
           json!({
               "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    }
               ],
               "collections": [
                   {
                       "name": "logger",
                       "durability": "transient"
                   }
               ]
           }),
           Err(Error::Validate { err, .. }) if &err == "identifier \"logger\" is defined twice, once in \"collections\" and once in \"children\""
        ),
        test_cml_duplicate_identifiers_children_storage(
           json!({
               "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    }
               ],
               "capabilities": [
                    {
                        "storage": "logger",
                        "path": "/logs",
                        "from": "parent"
                    }
                ]
           }),
           Err(Error::Validate { err, .. }) if &err == "identifier \"logger\" is defined twice, once in \"storage\" and once in \"children\""
        ),
        test_cml_duplicate_identifiers_collection_storage(
           json!({
               "collections": [
                    {
                        "name": "logger",
                        "durability": "transient"
                    }
                ],
                "capabilities": [
                    {
                        "storage": "logger",
                        "path": "/logs",
                        "from": "parent"
                    }
                ]
           }),
           Err(Error::Validate { err, .. }) if &err == "identifier \"logger\" is defined twice, once in \"storage\" and once in \"collections\""
        ),
        test_cml_duplicate_identifiers_children_runners(
           json!({
               "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    }
               ],
               "capabilities": [
                    {
                        "runner": "logger",
                        "from": "parent"
                    }
                ]
           }),
           Err(Error::Validate { err, .. }) if &err == "identifier \"logger\" is defined twice, once in \"runner\" and once in \"children\""
        ),
        test_cml_duplicate_identifiers_environments(
            json!({
                "children": [
                     {
                         "name": "logger",
                         "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                     }
                ],
                "environments": [
                     {
                         "name": "logger",
                     }
                 ]
            }),
            Err(Error::Validate { err, .. }) if &err == "identifier \"logger\" is defined twice, once in \"environment\" and once in \"children\""
        ),

        // deny unknown fields
        test_deny_unknown_fields(
            json!(
                {
                    "program": {
                        "runner": "elf",
                        "binary": "bin/app",
                    },
                    "unknown_field": {},
                }
            ),
            Err(Error::Parse { err, .. }) if err.starts_with("unknown field `unknown_field`, expected one of ")
        ),
        test_offer_source_availability_unknown(
            json!({
                "children": [
                    {
                        "name": "foo",
                        "url": "fuchsia-pkg://foo.com/foo#meta/foo.cm"
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "#bar",
                        "to": "#foo",
                        "availability": "optional",
                        "source_availability": "unknown",
                    },
                ],
            }),
            Ok(())
        ),
        test_offer_source_availability_required(
            json!({
                "children": [
                    {
                        "name": "foo",
                        "url": "fuchsia-pkg://foo.com/foo#meta/foo.cm"
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "#bar",
                        "to": "#foo",
                        "source_availability": "required",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"offer\" source \"#bar\" does not appear in \"children\" or \"capabilities\""
        ),
        test_offer_source_availability_omitted(
            json!({
                "children": [
                    {
                        "name": "foo",
                        "url": "fuchsia-pkg://foo.com/foo#meta/foo.cm"
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "#bar",
                        "to": "#foo",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"offer\" source \"#bar\" does not appear in \"children\" or \"capabilities\""
        ),
        test_offer_source_void_availability_required(
            json!({
                "children": [
                    {
                        "name": "foo",
                        "url": "fuchsia-pkg://foo.com/foo#meta/foo.cm"
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "void",
                        "to": "#foo",
                        "availability": "required",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "capabilities with a source of \"void\" must have an availability of \"optional\", capabilities: \"fuchsia.examples.Echo\", from: \"void\""
        ),
        test_offer_source_void_availability_same_as_target(
            json!({
                "children": [
                    {
                        "name": "foo",
                        "url": "fuchsia-pkg://foo.com/foo#meta/foo.cm"
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "void",
                        "to": "#foo",
                        "availability": "same_as_target",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "capabilities with a source of \"void\" must have an availability of \"optional\", capabilities: \"fuchsia.examples.Echo\", from: \"void\""
        ),
        test_offer_source_missing_availability_required(
            json!({
                "children": [
                    {
                        "name": "foo",
                        "url": "fuchsia-pkg://foo.com/foo#meta/foo.cm"
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "#bar",
                        "to": "#foo",
                        "availability": "required",
                        "source_availability": "unknown",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "capabilities with an intentionally missing source must have an availability that is either unset or \"optional\", capabilities: \"fuchsia.examples.Echo\", from: \"#bar\""
        ),
        test_offer_source_missing_availability_same_as_target(
            json!({
                "children": [
                    {
                        "name": "foo",
                        "url": "fuchsia-pkg://foo.com/foo#meta/foo.cm"
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "#bar",
                        "to": "#foo",
                        "availability": "same_as_target",
                        "source_availability": "unknown",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "capabilities with an intentionally missing source must have an availability that is either unset or \"optional\", capabilities: \"fuchsia.examples.Echo\", from: \"#bar\""
        ),
        test_expose_source_availability_unknown(
            json!({
                "expose": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "#bar",
                        "availability": "optional",
                        "source_availability": "unknown",
                    },
                ],
            }),
            Ok(())
        ),
        test_expose_source_availability_required(
            json!({
                "expose": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "#bar",
                        "source_availability": "required",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"expose\" source \"#bar\" does not appear in \"children\" or \"capabilities\""
        ),
        test_expose_source_availability_omitted(
            json!({
                "expose": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "#bar",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"expose\" source \"#bar\" does not appear in \"children\" or \"capabilities\""
        ),
        test_expose_source_void_availability_required(
            json!({
                "expose": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "void",
                        "availability": "required",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "capabilities with a source of \"void\" must have an availability of \"optional\", capabilities: \"fuchsia.examples.Echo\", from: \"void\""
        ),
        test_expose_source_void_availability_same_as_target(
            json!({
                "expose": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "void",
                        "availability": "same_as_target",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "capabilities with a source of \"void\" must have an availability of \"optional\", capabilities: \"fuchsia.examples.Echo\", from: \"void\""
        ),
        test_expose_source_missing_availability_required(
            json!({
                "expose": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "#bar",
                        "availability": "required",
                        "source_availability": "unknown",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "capabilities with an intentionally missing source must have an availability that is either unset or \"optional\", capabilities: \"fuchsia.examples.Echo\", from: \"#bar\""
        ),
        test_expose_source_missing_availability_same_as_target(
            json!({
                "expose": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "from": "#bar",
                        "availability": "same_as_target",
                        "source_availability": "unknown",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "capabilities with an intentionally missing source must have an availability that is either unset or \"optional\", capabilities: \"fuchsia.examples.Echo\", from: \"#bar\""
        ),
    }

    // Tests for services.
    test_validate_cml_with_context! {
        test_cml_validate_use_service(
            json!({
                "use": [
                    { "service": "CoolFonts", "path": "/svc/fuchsia.fonts.Provider" },
                    { "service": "fuchsia.component.Realm", "from": "framework" },
                ],
            }),
            Ok(())
        ),
        test_cml_validate_offer_service(
            json!({
                "offer": [
                    {
                        "service": "fuchsia.logger.Log",
                        "from": "#logger",
                        "to": [ "#echo_server", "#modular" ],
                        "as": "fuchsia.logger.SysLog"
                    },
                    {
                        "service": "fuchsia.fonts.Provider",
                        "from": "parent",
                        "to": [ "#echo_server" ]
                    },
                    {
                        "service": "fuchsia.net.Netstack",
                        "from": "self",
                        "to": [ "#echo_server" ]
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://logger",
                    },
                    {
                        "name": "echo_server",
                        "url": "fuchsia-pkg://echo_server",
                    }
                ],
                "collections": [
                    {
                        "name": "modular",
                        "durability": "transient",
                    },
                ],
                "capabilities": [
                    { "service": "fuchsia.net.Netstack" },
                ],
            }),
            Ok(())
        ),
        test_cml_validate_expose_service(
            json!(
                {
                    "expose": [
                        {
                            "service": "fuchsia.fonts.Provider",
                            "from": "self",
                        },
                        {
                            "service": "fuchsia.logger.Log",
                            "from": "#logger",
                            "as": "logger"
                        },
                    ],
                    "capabilities": [
                        { "service": "fuchsia.fonts.Provider" },
                    ],
                    "children": [
                        {
                            "name": "logger",
                            "url": "fuchsia-pkg://logger",
                        },
                    ]
                }
            ),
            Ok(())
        ),
        test_cml_validate_expose_service_multi_source(
            json!(
                {
                    "expose": [
                        {
                            "service": "fuchsia.my.Service",
                            "from": [ "self", "#a" ],
                        },
                        {
                            "service": "fuchsia.my.Service",
                            "from": "#coll",
                        },
                    ],
                    "capabilities": [
                        { "service": "fuchsia.my.Service" },
                    ],
                    "children": [
                        {
                            "name": "a",
                            "url": "fuchsia-pkg://a",
                        },
                    ],
                    "collections": [
                        {
                            "name": "coll",
                            "durability": "transient",
                        },
                    ],
                }
            ),
            Ok(())
        ),
        test_cml_validate_offer_service_multi_source(
            json!(
                {
                    "offer": [
                        {
                            "service": "fuchsia.my.Service",
                            "from": [ "self", "parent" ],
                            "to": "#b",
                        },
                        {
                            "service": "fuchsia.my.Service",
                            "from": [ "#a", "#coll" ],
                            "to": "#b",
                        },
                    ],
                    "capabilities": [
                        { "service": "fuchsia.my.Service" },
                    ],
                    "children": [
                        {
                            "name": "a",
                            "url": "fuchsia-pkg://a",
                        },
                        {
                            "name": "b",
                            "url": "fuchsia-pkg://b",
                        },
                    ],
                    "collections": [
                        {
                            "name": "coll",
                            "durability": "transient",
                        },
                    ],
                }
            ),
            Ok(())
        ),
        test_cml_service(
            json!({
                "capabilities": [
                    {
                        "protocol": "a",
                        "path": "/minfs",
                    },
                    {
                        "protocol": "b",
                        "path": "/data",
                    },
                    {
                        "protocol": "c",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_service_multi(
            json!({
                "capabilities": [
                    {
                        "service": ["a", "b", "c"],
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_service_all_valid_chars(
            json!({
                "capabilities": [
                    {
                        "service": "abcdefghijklmnopqrstuvwxyz0123456789_-service",
                    },
                ],
            }),
            Ok(())
        ),
    }

    // Tests structured config
    test_validate_cml_with_feature_context! { FeatureSet::from(vec![]), {
        test_cml_configs(
            json!({
                "config": {
                    "verbosity": {
                        "type": "string",
                        "max_size": 20,
                    },
                    "timeout": { "type": "uint64" },
                    "tags": {
                        "type": "vector",
                        "max_count": 10,
                        "element": {
                            "type": "string",
                            "max_size": 50
                        }
                    }
                }
            }),
            Ok(())
        ),

        test_cml_configs_not_object(
            json!({
                "config": "abcd"
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid type: string \"abcd\", expected a map"
        ),

        test_cml_configs_empty(
            json!({
                "config": {
                }
            }),
            Err(Error::Validate { err, .. }) if &err == "'config' section is empty"
        ),

        test_cml_configs_bad_type(
            json!({
                "config": {
                    "verbosity": 123456
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid type: integer `123456`, expected internally tagged enum ConfigValueType"
        ),

        test_cml_configs_unknown_type(
            json!({
                "config": {
                    "verbosity": {
                        "type": "foo"
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "unknown variant `foo`, expected one of `bool`, `uint8`, `uint16`, `uint32`, `uint64`, `int8`, `int16`, `int32`, `int64`, `string`, `vector`"
        ),

        test_cml_configs_no_max_count_vector(
            json!({
                "config": {
                    "tags": {
                        "type": "vector",
                        "element": {
                            "type": "string",
                            "max_size": 50,
                        }
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "missing field `max_count`"
        ),

        test_cml_configs_no_max_size_string(
            json!({
                "config": {
                    "verbosity": {
                        "type": "string",
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "missing field `max_size`"
        ),

        test_cml_configs_no_max_size_string_vector(
            json!({
                "config": {
                    "tags": {
                        "type": "vector",
                        "max_count": 10,
                        "element": {
                            "type": "string",
                        }
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "missing field `max_size`"
        ),

        test_cml_configs_empty_key(
            json!({
                "config": {
                    "": {
                        "type": "bool"
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 0, expected a non-empty name no more than 64 characters in length"
        ),

        test_cml_configs_too_long_key(
            json!({
                "config": {
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": {
                        "type": "bool"
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid length 74, expected a non-empty name no more than 64 characters in length"
        ),
        test_cml_configs_key_starts_with_number(
            json!({
                "config": {
                    "8abcd": { "type": "uint8" }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"8abcd\", expected a name which must start with a letter, can contain letters, numbers, and underscores, but cannot end with an underscore"
        ),

        test_cml_configs_key_ends_with_underscore(
            json!({
                "config": {
                    "abcd_": { "type": "uint8" }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"abcd_\", expected a name which must start with a letter, can contain letters, numbers, and underscores, but cannot end with an underscore"
        ),

        test_cml_configs_capitals_in_key(
            json!({
                "config": {
                    "ABCD": { "type": "uint8" }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"ABCD\", expected a name which must start with a letter, can contain letters, numbers, and underscores, but cannot end with an underscore"
        ),

        test_cml_configs_special_chars_in_key(
            json!({
                "config": {
                    "!@#$": { "type": "uint8" }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"!@#$\", expected a name which must start with a letter, can contain letters, numbers, and underscores, but cannot end with an underscore"
        ),

        test_cml_configs_dashes_in_key(
            json!({
                "config": {
                    "abcd-efgh": { "type": "uint8" }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: string \"abcd-efgh\", expected a name which must start with a letter, can contain letters, numbers, and underscores, but cannot end with an underscore"
        ),

        test_cml_configs_bad_max_size_string(
            json!({
                "config": {
                    "verbosity": {
                        "type": "string",
                        "max_size": "abcd"
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid type: string \"abcd\", expected a nonzero u32"
        ),

        test_cml_configs_zero_max_size_string(
            json!({
                "config": {
                    "verbosity": {
                        "type": "string",
                        "max_size": 0
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: integer `0`, expected a nonzero u32"
        ),

        test_cml_configs_bad_max_count_on_vector(
            json!({
                "config": {
                    "toggles": {
                        "type": "vector",
                        "max_count": "abcd",
                        "element": {
                            "type": "bool"
                        }
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid type: string \"abcd\", expected a nonzero u32"
        ),

        test_cml_configs_zero_max_count_on_vector(
            json!({
                "config": {
                    "toggles": {
                        "type": "vector",
                        "max_count": 0,
                        "element": {
                            "type": "bool"
                        }
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: integer `0`, expected a nonzero u32"
        ),

        test_cml_configs_bad_max_size_string_vector(
            json!({
                "config": {
                    "toggles": {
                        "type": "vector",
                        "max_count": 100,
                        "element": {
                            "type": "string",
                            "max_size": "abcd"
                        }
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid type: string \"abcd\", expected a nonzero u32"
        ),

        test_cml_configs_zero_max_size_string_vector(
            json!({
                "config": {
                    "toggles": {
                        "type": "vector",
                        "max_count": 100,
                        "element": {
                            "type": "string",
                            "max_size": 0
                        }
                    }
                }
            }),
            Err(Error::Parse { err, .. }) if &err == "invalid value: integer `0`, expected a nonzero u32"
        ),
    }}

    // Tests the use of `allow_long_names` when the "AllowLongNames" feature is set.
    test_validate_cml_with_feature_context! { FeatureSet::from(vec![Feature::AllowLongNames]), {
        test_cml_validate_set_allow_long_names_true(
            json!({
                "collections": [
                    {
                        "name": "foo",
                        "durability": "transient",
                        "allow_long_names": true
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_validate_set_allow_long_names_false(
            json!({
                "collections": [
                    {
                        "name": "foo",
                        "durability": "transient",
                        "allow_long_names": false
                    },
                ],
            }),
            Ok(())
        ),
    }}

    // Tests using a dictionary when the UseDictionaries feature is set
    test_validate_cml_with_feature_context! { FeatureSet::from(vec![Feature::UseDictionaries]), {
        test_cml_validate_set_allow_use_dictionaries(
            json!({
                "use": [
                    {
                        "protocol": "fuchsia.examples.Echo",
                        "path": "/svc/fuchsia.examples.Echo",
                    },
                    {
                        "dictionary": "toolbox",
                        "path": "/svc",
                    },
                ],
            }),
            Ok(())
        ),
    }}

    // Tests that two dictionaries can be used at the same path
    test_validate_cml_with_feature_context! { FeatureSet::from(vec![Feature::UseDictionaries]), {
        test_cml_validate_set_allow_use_2_dictionaries_at_same_path(
            json!({
                "use": [
                    {
                        "dictionary": "toolbox-1",
                        "path": "/svc",
                    },
                    {
                        "dictionary": "toolbox-2",
                        "path": "/svc",
                    },
                ],
            }),
            Ok(())
        ),
    }}

    // Tests that using a dictionary fails when the "UseDictionaries" feature is not set.
    test_validate_cml_with_context! {
        test_cml_allow_use_dictionary_without_feature(
            json!({
                "use": [
                    {
                        "dictionary": "foo",
                        "path": "/foo",
                    },
                ],
            }),
            Err(Error::RestrictedFeature(s)) if s == "use_dictionaries"
        ),
    }

    // Tests validate_facets function without the feature set
    test_validate_cml_with_context! {
        test_valid_empty_facets(
            json!({
                "facets": {}
            }),
            Ok(())
        ),

        test_invalid_empty_facets(
            json!({
                "facets": ""
            }),
            Err(err) if err.to_string().contains("invalid type: string")
        ),
        test_valid_empty_fuchsia_test_facet(
            json!({
                "facets": {TEST_FACET_KEY: {}}
            }),
            Ok(())
        ),

        test_valid_allowed_pkg_without_feature(
            json!({
                "facets": {
                    TEST_TYPE_FACET_KEY: "some_realm",
                    TEST_FACET_KEY: {
                        TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY: [ "some_pkg" ]
                    }
                }
            }),
            Ok(())
        ),
    }

    // Tests validate_facets function with the RestrictTestTypeInFacet enabled.
    test_validate_cml_with_feature_context! { FeatureSet::from(vec![Feature::RestrictTestTypeInFacet]), {
        test_valid_empty_facets_with_test_type_feature_enabled(
            json!({
                "facets": {}
            }),
            Ok(())
        ),
        test_valid_empty_fuchsia_test_facet_with_test_type_feature_enabled(
            json!({
                "facets": {TEST_FACET_KEY: {}}
            }),
            Ok(())
        ),

        test_invalid_test_type_with_feature_enabled(
            json!({
                "facets": {
                    TEST_FACET_KEY: {
                        TEST_TYPE_FACET_KEY: "some_realm",
                    }
                }
            }),
            Err(err) if err.to_string().contains(TEST_TYPE_FACET_KEY)
        ),
    }}

    // Tests validate_facets function with the EnableAllowNonHermeticPackagesFeature disabled.
    test_validate_cml_with_feature_context! { FeatureSet::from(vec![Feature::AllowNonHermeticPackages]), {
        test_valid_empty_facets_with_feature_disabled(
            json!({
                "facets": {}
            }),
            Ok(())
        ),
        test_valid_empty_fuchsia_test_facet_with_feature_disabled(
            json!({
                "facets": {TEST_FACET_KEY: {}}
            }),
            Ok(())
        ),

        test_valid_allowed_pkg_with_feature_disabled(
            json!({
                "facets": {
                    TEST_FACET_KEY: {
                        TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY: [ "some_pkg" ]
                    }
                }
            }),
            Ok(())
        ),
    }}

    // Tests validate_facets function with the EnableAllowNonHermeticPackagesFeature enabled.
    test_validate_cml_with_feature_context! { FeatureSet::from(vec![Feature::EnableAllowNonHermeticPackagesFeature]), {
        test_valid_empty_facets_with_feature_enabled(
            json!({
                "facets": {}
            }),
            Ok(())
        ),
        test_valid_empty_fuchsia_test_facet_with_feature_enabled(
            json!({
                "facets": {TEST_FACET_KEY: {}}
            }),
            Ok(())
        ),

        test_invalid_allowed_pkg_with_feature_enabled(
            json!({
                "facets": {
                    TEST_FACET_KEY: {
                        TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY: [ "some_pkg" ]
                    }
                }
            }),
            Err(err) if err.to_string().contains(&Feature::AllowNonHermeticPackages.to_string())
        ),
    }}

    // Tests validate_facets function with the feature enabled and allowed pkg feature set.
    test_validate_cml_with_feature_context! { FeatureSet::from(vec![Feature::EnableAllowNonHermeticPackagesFeature, Feature::AllowNonHermeticPackages]), {
        test_invalid_empty_facets_with_feature_enabled(
            json!({
                "facets": {}
            }),
            Err(err) if err.to_string().contains(&Feature::AllowNonHermeticPackages.to_string())
        ),

        test_invalid_empty_fuchsia_test_facet_with_feature_enabled(
            json!({
                "facets": {TEST_FACET_KEY: {}}
            }),
            Err(err) if err.to_string().contains(&Feature::AllowNonHermeticPackages.to_string())
        ),

        test_valid_allowed_pkg_with_feature_enabled(
            json!({
                "facets": {
                    TEST_FACET_KEY: {
                        TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY: [ "some_pkg" ]
                    }
                }
            }),
            Ok(())
        ),
    }}

    test_validate_cml_with_feature_context! { FeatureSet::from(vec![Feature::DynamicDictionaries]), {
        test_cml_offer_to_dictionary_unsupported(
            json!({
                "offer": [
                    {
                        "event_stream": "p",
                        "from": "parent",
                        "to": "self/dict",
                    },
                ],
                "capabilities": [
                    {
                        "dictionary": "dict",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"offer\" to dictionary \
            \"self/dict\" for \"event_stream\" but dictionaries do not support this type yet."
        ),
        test_cml_dictionary_ref(
            json!({
                "use": [
                    {
                        "protocol": "a",
                        "from": "parent/a",
                    },
                    {
                        "protocol": "b",
                        "from": "#child/a/b",
                    },
                    {
                        "protocol": "c",
                        "from": "self/a/b/c",
                    },
                ],
                "capabilities": [
                    {
                        "dictionary": "a",
                    },
                ],
                "children": [
                    {
                        "name": "child",
                        "url": "fuchsia-pkg://child",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_expose_dictionary_from_self(
            json!({
                "expose": [
                    {
                        "dictionary": "foo_dictionary",
                        "from": "self",
                    },
                ],
                "capabilities": [
                    {
                        "dictionary": "foo_dictionary",
                    },
                ]
            }),
            Ok(())
        ),
        test_cml_offer_to_dictionary_duplicate(
            json!({
                "offer": [
                    {
                        "protocol": "p",
                        "from": "parent",
                        "to": "self/dict",
                    },
                    {
                        "protocol": "p",
                        "from": "#child",
                        "to": "self/dict",
                    },
                ],
                "capabilities": [
                    {
                        "dictionary": "dict",
                    },
                ],
                "children": [
                    {
                        "name": "child",
                        "url": "fuchsia-pkg://child",
                    },
                ],
            }),
            Err(Error::ValidateContexts { err, .. }) if &err == "\"p\" is a duplicate \"offer\" target capability for \"self/dict\""
        ),
        test_cml_offer_to_dictionary_dynamic(
            json!({
                "offer": [
                    {
                        "protocol": "p",
                        "from": "parent",
                        "to": "self/dict",
                    },
                ],
                "capabilities": [
                    {
                        "dictionary": "dict",
                        "path": "/out/dir",
                    },
                ],
            }),
            Err(Error::ValidateContext { err, .. }) if &err == "\"offer\" has dictionary target \"self/dict\" but \"dict\" sets \"path\". Therefore, it is a dynamic dictionary that does not allow offers into it."
        ),
    }}

    // Tests that offering and exposing service capabilities to the same target and target name is
    // allowed.
    test_validate_cml_with_context! {
        test_cml_aggregate_expose(
            json!({
                "expose": [
                    {
                        "service": "fuchsia.foo.Bar",
                        "from": ["#a", "#b"],
                    },
                ],
                "children": [
                    {
                        "name": "a",
                        "url": "fuchsia-pkg://fuchsia.com/a#meta/a.cm",
                    },
                    {
                        "name": "b",
                        "url": "fuchsia-pkg://fuchsia.com/b#meta/b.cm",
                    },
                ],
            }),
            Ok(())
        ),
        test_cml_aggregate_offer(
            json!({
                "offer": [
                    {
                        "service": "fuchsia.foo.Bar",
                        "from": ["#a", "#b"],
                        "to": "#target",
                    },
                ],
                "children": [
                    {
                        "name": "a",
                        "url": "fuchsia-pkg://fuchsia.com/a#meta/a.cm",
                    },
                    {
                        "name": "b",
                        "url": "fuchsia-pkg://fuchsia.com/b#meta/b.cm",
                    },
                    {
                        "name": "target",
                        "url": "fuchsia-pkg://fuchsia.com/target#meta/target.cm",
                    },
                ],
            }),
            Ok(())
        ),
    }

    use crate::translate::test_util::must_parse_cml;
    use crate::translate::{CompileOptions, compile};

    #[test]
    fn test_cml_use_bad_config_from_self() {
        let input = must_parse_cml!({
        "use": [
            {
                "config": "fuchsia.config.MyConfig",
                "key": "my_config",
                "type": "bool",
                "from": "self",
            },
        ],
        });

        let options = CompileOptions::new();
        assert_matches!(compile(&input, options), Err(Error::ValidateContext { .. }));
    }

    // Tests for config capabilities
    test_validate_cml_with_context! {
        a_test_cml_use_config(
        json!({"use": [
            {
                "config": "fuchsia.config.MyConfig",
                "key": "my_config",
                "type": "bool",
            },
        ],}),
        Ok(())
        ),
        test_cml_use_config_good_vector(
        json!({"use": [
            {
                "config": "fuchsia.config.MyConfig",
                "key": "my_config",
                "type": "vector",
                "element": { "type": "bool"},
                "max_count": 1,
            },
        ],}),
        Ok(())
        ),


        test_cml_optional_use_no_config(
        json!({"use": [
            {
                "config": "fuchsia.config.MyConfig",
                "key": "my_config",
                "type": "bool",
                "availability": "optional",
            },
        ],}),
        Err(Error::Validate {err, ..})
        if &err == "Optionally using a config capability without a default requires a matching 'config' section."
        ),
        test_cml_transitional_use_no_config(
        json!({"use": [
            {
                "config": "fuchsia.config.MyConfig",
                "key": "my_config",
                "type": "bool",
                "availability": "transitional",
            },
        ],}),
        Err(Error::Validate {err, ..})
        if &err == "Optionally using a config capability without a default requires a matching 'config' section."
        ),
        test_cml_optional_use_bad_type(
        json!({"use": [
            {
                "config": "fuchsia.config.MyConfig",
                "key": "my_config",
                "type": "bool",
                "availability": "optional",
            },
        ],
        "config": {
            "my_config": { "type": "uint8"}
        }}),
        Err(Error::ValidateContexts {err, ..})
        if &err == "Use and config block differ on type for key 'my_config'"
        ),

        test_cml_optional_use_good(
        json!({"use": [
            {
                "config": "fuchsia.config.MyConfig",
                "key": "my_config",
                "type": "bool",
                "availability": "optional",
            },
        ],
        "config": {
            "my_config": { "type": "bool"},
        }
    }),
    Ok(())
        ),
    test_cml_offer_two_types_bad(
        json!({"offer": [
            {
                "protocol": "fuchsia.protocol.MyProtocol",
                "service": "fuchsia.service.MyService",
                "from": "self",
                "to" : "#child",
            },
        ],
    }),
        Err(Error::ValidateContext {err, ..})
        if &err == "offer declaration has multiple capability types defined: [\"service\", \"protocol\"]"
        ),
    }
}
