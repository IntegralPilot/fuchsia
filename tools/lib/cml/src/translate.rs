// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::Error;
use crate::features::{Feature, FeatureSet};
use crate::types::child::{Child, ContextChild};
use crate::types::collection::{Collection, ContextCollection};
use crate::types::common::ContextCapabilityClause;
use crate::types::document::{Document, DocumentContext};
use crate::types::environment::{
    ContextDebugRegistration, ContextEnvironment, ContextResolverRegistration,
    ContextRunnerRegistration, DebugRegistration, Environment, EnvironmentExtends, EnvironmentRef,
    RunnerRegistration,
};
use crate::types::expose::{ContextExpose, Expose, ExposeFromRef, ExposeToRef};
use crate::types::offer::{
    ContextOffer, Offer, OfferFromRef, OfferToRef, TargetAvailability,
    offer_to_all_would_duplicate_context,
};
use crate::types::program::ContextProgram;
use crate::types::right::RightsClause;
use crate::types::r#use::{ContextUse, Use, UseFromRef};
use crate::validate::CapabilityRequirements;
use crate::{
    AnyRef, AsClauseContext, Availability, Capability, CapabilityClause, ConfigKey,
    ConfigNestedValueType, ConfigRuntimeSource, ConfigType, ConfigValueType, ContextCapability,
    ContextPathClause, ContextSpanned, DictionaryRef, EventScope, FromClauseContext, OneOrMany,
    Path, Program, ResolverRegistration, RootDictionaryRef, SourceAvailability, validate,
};
use cm_rust::NativeIntoFidl;
use cm_types::{self as cm, BorrowedName, Name, StartupMode};
use directed_graph::DirectedGraph;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_io as fio;
use indexmap::IndexMap;
use itertools::Itertools;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::convert::{Into, TryInto};
use std::path::PathBuf;
use std::sync::Arc;

/// Options for CML compilation. Uses the builder pattern.
#[derive(Default, Clone)]
pub struct CompileOptions<'a> {
    file: Option<PathBuf>,
    config_package_path: Option<String>,
    features: Option<&'a FeatureSet>,
    capability_requirements: CapabilityRequirements<'a>,
}

impl<'a> CompileOptions<'a> {
    pub fn new() -> Self {
        Default::default()
    }

    /// The path to the CML file, if applicable. Used for error reporting.
    pub fn file(mut self, file: &std::path::Path) -> CompileOptions<'a> {
        self.file = Some(file.to_path_buf());
        self
    }

    /// The path within the component's package at which to find config value files.
    pub fn config_package_path(mut self, config_package_path: &str) -> CompileOptions<'a> {
        self.config_package_path = Some(config_package_path.to_string());
        self
    }

    /// Which additional features are enabled. Defaults to none.
    pub fn features(mut self, features: &'a FeatureSet) -> CompileOptions<'a> {
        self.features = Some(features);
        self
    }

    /// Require that the component must use or offer particular protocols. Defaults to no
    /// requirements.
    pub fn protocol_requirements(
        mut self,
        protocol_requirements: CapabilityRequirements<'a>,
    ) -> CompileOptions<'a> {
        self.capability_requirements = protocol_requirements;
        self
    }
}

/// Compiles the [DocumentContext] into a FIDL [fdecl::Component].
pub fn compile(
    document: &DocumentContext,
    options: CompileOptions<'_>,
) -> Result<fdecl::Component, Error> {
    validate::validate_cml(
        &document,
        options.features.unwrap_or(&FeatureSet::empty()),
        &options.capability_requirements,
    )?;

    let all_capability_names_owned: BTreeSet<Name> =
        document.all_capability_names().into_iter().collect();

    let all_capability_names: BTreeSet<&BorrowedName> =
        all_capability_names_owned.iter().map(|n| n.as_ref()).collect();

    let all_children = document.all_children_names().iter().cloned().collect();
    let all_collections = document.all_collection_names().iter().cloned().collect();

    let component = fdecl::Component {
        program: document.program.as_ref().map(|p| translate_program(&p.value)).transpose()?,

        uses: document
            .r#use
            .as_ref()
            .map(|u| {
                translate_use(&options, u, &all_capability_names, &all_children, &all_collections)
            })
            .transpose()?,

        exposes: document
            .expose
            .as_ref()
            .map(|e| {
                translate_expose(
                    &options,
                    e,
                    &all_capability_names,
                    &all_collections,
                    &all_children,
                )
            })
            .transpose()?,

        offers: document
            .offer
            .as_ref()
            .map(|o| {
                translate_offer(&options, o, &all_capability_names, &all_children, &all_collections)
            })
            .transpose()?,

        capabilities: document
            .capabilities
            .as_ref()
            .map(|c| translate_capabilities(&options, c, false))
            .transpose()?,

        children: document.children.as_ref().map(|c| translate_children(c)),
        collections: document.collections.as_ref().map(|c| translate_collections(c)),

        environments: document
            .environments
            .as_ref()
            .map(|env| translate_environments(&options, env, &all_capability_names))
            .transpose()?,
        facets: document.facets.clone().map(dictionary_from_nested_spanned_map).transpose()?,

        config: translate_config(&document.config, &document.r#use, &options.config_package_path)?,
        ..Default::default()
    };

    let mut deps = DirectedGraph::new();
    cm_fidl_validator::validate(&component, &mut deps).map_err(Error::fidl_validator)?;

    Ok(component)
}

/// Converts an IndexMap of spanned Values into a fuchsia.data.Dictionary.
pub fn dictionary_from_context_map(
    map: IndexMap<String, ContextSpanned<Value>>,
) -> Result<fdata::Dictionary, Error> {
    let mut entries = Vec::new();

    for (key, spanned_value) in map {
        // We pass each value's specific origin into the converter
        let dictionary_value =
            value_to_dictionary_value(spanned_value.value, &spanned_value.origin)?;

        entries.push(fdata::DictionaryEntry { key, value: dictionary_value });
    }

    // Fuchsia dictionaries should be sorted lexicographically by key
    // to ensure the resulting .cm file is deterministic.
    entries.sort_by(|a, b| a.key.cmp(&b.key));

    Ok(fdata::Dictionary { entries: Some(entries), ..Default::default() })
}

// Converts a Map<String, serde_json::Value> to a fuchsia Dictionary.
fn dictionary_from_map(in_obj: Map<String, Value>) -> Result<fdata::Dictionary, Error> {
    let mut entries = vec![];
    for (key, v) in in_obj {
        let value = value_to_dictionary_value_without_span(v)?;
        entries.push(fdata::DictionaryEntry { key, value });
    }
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(fdata::Dictionary { entries: Some(entries), ..Default::default() })
}

// Converts a serde_json::Value into a fuchsia DictionaryValue.
fn value_to_dictionary_value_without_span(
    value: Value,
) -> Result<Option<Box<fdata::DictionaryValue>>, Error> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(Box::new(fdata::DictionaryValue::Str(s)))),
        Value::Array(arr) => {
            if arr.iter().all(Value::is_string) {
                let strs =
                    arr.into_iter().map(|v| v.as_str().unwrap().to_owned()).collect::<Vec<_>>();
                Ok(Some(Box::new(fdata::DictionaryValue::StrVec(strs))))
            } else if arr.iter().all(Value::is_object) {
                let objs = arr
                    .into_iter()
                    .map(|v| v.as_object().unwrap().clone())
                    .map(|v| dictionary_from_nested_map(v.into_iter().collect()))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Some(Box::new(fdata::DictionaryValue::ObjVec(objs))))
            } else {
                Err(Error::validate(
                    "Values of an array must either exclusively strings or exclusively objects",
                ))
            }
        }
        other => Err(Error::validate(format!(
            "Value must be string, list of strings, or list of objects: {:?}",
            other
        ))),
    }
}

fn value_to_dictionary_value(
    value: Value,
    origin: &Arc<std::path::Path>,
) -> Result<Option<Box<fdata::DictionaryValue>>, Error> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(Box::new(fdata::DictionaryValue::Str(s)))),
        Value::Array(arr) => {
            if arr.is_empty() {
                return Ok(Some(Box::new(fdata::DictionaryValue::StrVec(vec![]))));
            }

            if arr.iter().all(Value::is_string) {
                let strs =
                    arr.into_iter().map(|v| v.as_str().unwrap().to_owned()).collect::<Vec<_>>();
                Ok(Some(Box::new(fdata::DictionaryValue::StrVec(strs))))
            } else if arr.iter().all(Value::is_object) {
                let objs = arr
                    .into_iter()
                    .map(|v| {
                        let obj_map = v.as_object().unwrap().clone().into_iter().collect();
                        dictionary_from_nested_map(obj_map)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Some(Box::new(fdata::DictionaryValue::ObjVec(objs))))
            } else {
                Err(Error::validate_context(
                    "Values of an array must be exclusively strings or exclusively objects",
                    Some(origin.clone()),
                ))
            }
        }
        other => Err(Error::validate_context(
            format!("Value must be string, list of strings, or list of objects: {:?}", other),
            Some(origin.clone()),
        )),
    }
}

/// Converts a [`serde_json::Map<String, serde_json::Value>`] to a [`fuchsia.data.Dictionary`].
///
/// The JSON object is converted as follows:
///
/// * Convert all non-string and string values into DictionaryValue::str.
/// * Flatten nested objects into top-level keys delimited by ".".
/// * Convert array of discrete values into  array of DictionaryValue::str_vec.
/// * Convert array of objects into array of DictionaryValue::obj_vec.
///
/// Values may be null, strings, arrays of strings, arrays of objects, or objects.
///
/// # Example
///
/// ```json
/// {
///   "binary": "bin/app",
///   "lifecycle": {
///     "stop_event": "notify",
///     "nested": {
///       "foo": "bar"
///     }
///   }
/// }
/// ```
///
/// is flattened to:
///
/// ```json
/// {
///   "binary": "bin/app",
///   "lifecycle.stop_event": "notify",
///   "lifecycle.nested.foo": "bar"
/// }
/// ```
fn dictionary_from_nested_map(map: IndexMap<String, Value>) -> Result<fdata::Dictionary, Error> {
    fn key_value_to_entries(
        key: String,
        value: Value,
    ) -> Result<Vec<fdata::DictionaryEntry>, Error> {
        if let Value::Object(map) = value {
            let entries = map
                .into_iter()
                .map(|(k, v)| key_value_to_entries([key.clone(), ".".to_string(), k].concat(), v))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect();
            return Ok(entries);
        }

        let entry_value = value_to_dictionary_value_without_span(value)?;
        Ok(vec![fdata::DictionaryEntry { key, value: entry_value }])
    }

    let mut entries: Vec<_> = map
        .into_iter()
        .map(|(k, v)| key_value_to_entries(k, v))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(fdata::Dictionary { entries: Some(entries), ..Default::default() })
}

fn dictionary_from_nested_spanned_map(
    map: IndexMap<String, ContextSpanned<Value>>,
) -> Result<fdata::Dictionary, Error> {
    fn key_value_to_entries(
        key: String,
        value: Value,
    ) -> Result<Vec<fdata::DictionaryEntry>, Error> {
        if let Value::Object(map) = value {
            let entries = map
                .into_iter()
                .map(|(k, v)| key_value_to_entries([key.clone(), ".".to_string(), k].concat(), v))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect();
            return Ok(entries);
        }

        let entry_value = value_to_dictionary_value_without_span(value)?;
        Ok(vec![fdata::DictionaryEntry { key, value: entry_value }])
    }

    let mut entries: Vec<_> = map
        .into_iter()
        .map(|(k, v)| key_value_to_entries(k, v.value))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(fdata::Dictionary { entries: Some(entries), ..Default::default() })
}

/// Translates a [`ContextProgram`] to a [`fuchsia.component.decl/Program`].
fn translate_program(program: &ContextProgram) -> Result<fdecl::Program, Error> {
    Ok(fdecl::Program {
        runner: program.runner.as_ref().map(|r| r.value.clone().into()),
        info: Some(dictionary_from_nested_map(program.info.clone())?),
        ..Default::default()
    })
}

/// `use` rules consume a single capability from one source (parent|framework).
fn translate_use(
    options: &CompileOptions<'_>,
    use_in: &Vec<ContextSpanned<ContextUse>>,
    all_capability_names: &BTreeSet<&BorrowedName>,
    all_children: &BTreeSet<&BorrowedName>,
    all_collections: &BTreeSet<&BorrowedName>,
) -> Result<Vec<fdecl::Use>, Error> {
    let mut out_uses = vec![];
    for spanned_use in use_in {
        let use_ = &spanned_use.value;
        if let Some(spanned) = use_.service() {
            let n = spanned.value;
            let (source, source_dictionary) = extract_use_source(
                options,
                use_,
                all_capability_names,
                all_children,
                Some(all_collections),
            )?;
            let target_paths =
                all_target_use_paths(use_, use_).ok_or_else(|| Error::internal("no capability"))?;
            let source_names = n.into_iter();
            let availability = extract_use_availability(use_)?;
            for (source_name, target_path) in source_names.into_iter().zip(target_paths.into_iter())
            {
                out_uses.push(fdecl::Use::Service(fdecl::UseService {
                    source: Some(source.clone()),
                    source_name: Some(source_name.to_string()),
                    source_dictionary: source_dictionary.clone(),
                    target_path: Some(target_path.to_string()),
                    dependency_type: Some(
                        use_.dependency
                            .clone()
                            .map(|s| s.value)
                            .unwrap_or(cm::DependencyType::Strong)
                            .into(),
                    ),
                    availability: Some(availability),
                    ..Default::default()
                }));
            }
        } else if let Some(spanned) = use_.protocol() {
            let n = spanned.value;
            let (source, source_dictionary) =
                extract_use_source(options, use_, all_capability_names, all_children, None)?;
            let availability = extract_use_availability(use_)?;
            if use_.numbered_handle.is_some() {
                let OneOrMany::One(source_name) = n else {
                    panic!("numbered_handle: multiple source_name");
                };
                out_uses.push(fdecl::Use::Protocol(fdecl::UseProtocol {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    source_dictionary: source_dictionary,
                    target_path: None,
                    numbered_handle: use_.numbered_handle.as_ref().map(|s| s.value.into()),
                    dependency_type: Some(
                        use_.dependency
                            .clone()
                            .map(|s| s.value)
                            .unwrap_or(cm::DependencyType::Strong)
                            .into(),
                    ),
                    availability: Some(availability),
                    ..Default::default()
                }));
                continue;
            }
            let source_names = n.into_iter();
            let target_paths =
                all_target_use_paths(use_, use_).ok_or_else(|| Error::internal("no capability"))?;
            for (source_name, target_path) in source_names.into_iter().zip(target_paths.into_iter())
            {
                out_uses.push(fdecl::Use::Protocol(fdecl::UseProtocol {
                    source: Some(source.clone()),
                    source_name: Some(source_name.to_string()),
                    source_dictionary: source_dictionary.clone(),
                    target_path: Some(target_path.into()),
                    numbered_handle: use_.numbered_handle.as_ref().map(|s| s.value.into()),
                    dependency_type: Some(
                        use_.dependency
                            .clone()
                            .map(|s| s.value)
                            .unwrap_or(cm::DependencyType::Strong)
                            .into(),
                    ),
                    availability: Some(availability),
                    ..Default::default()
                }));
            }
        } else if let Some(spanned) = &use_.directory {
            let n = &spanned.value;
            let (source, source_dictionary) =
                extract_use_source(options, use_, all_capability_names, all_children, None)?;
            let target_path = one_target_use_path(use_, use_)?;
            let rights = extract_required_rights(use_, "use")?;
            let subdir = extract_use_subdir(use_);
            let availability = extract_use_availability(use_)?;
            out_uses.push(fdecl::Use::Directory(fdecl::UseDirectory {
                source: Some(source),
                source_name: Some(n.clone().into()),
                source_dictionary,
                target_path: Some(target_path.into()),
                rights: Some(rights),
                subdir: subdir.map(|s| s.into()),
                dependency_type: Some(
                    use_.dependency
                        .clone()
                        .map(|s| s.value)
                        .unwrap_or(cm::DependencyType::Strong)
                        .into(),
                ),
                availability: Some(availability),
                ..Default::default()
            }));
        } else if let Some(spanned) = &use_.storage {
            let n = &spanned.value;
            let target_path = one_target_use_path(use_, use_)?;
            let availability = extract_use_availability(use_)?;
            out_uses.push(fdecl::Use::Storage(fdecl::UseStorage {
                source_name: Some(n.clone().into()),
                target_path: Some(target_path.into()),
                availability: Some(availability),
                ..Default::default()
            }));
        } else if let Some(spanned_names) = &use_.event_stream {
            let names = &spanned_names.value;
            let source_names: Vec<String> =
                annotate_type::<Vec<cm_types::Name>>(names.clone().into())
                    .iter()
                    .map(|name| name.to_string())
                    .collect();
            let availability = extract_use_availability(use_)?;
            for name in source_names {
                let scopes = match use_.scope.clone() {
                    Some(v) => Some(annotate_type::<Vec<EventScope>>(v.value.into())),
                    None => None,
                };
                let internal_error = format!(
                    "Internal error in all_target_use_paths when translating an EventStream. \
                    Please file a bug."
                );
                let (source, _source_dictionary) =
                    extract_use_source(options, use_, all_capability_names, all_children, None)?;
                out_uses.push(fdecl::Use::EventStream(fdecl::UseEventStream {
                    source_name: Some(name),
                    scope: match scopes {
                        Some(values) => {
                            let mut output = vec![];
                            for value in &values {
                                if let Some(target) = translate_target_ref(
                                    options,
                                    value.into(),
                                    &all_children,
                                    &all_collections,
                                    &BTreeSet::new(),
                                    Some(&TargetAvailability::Required),
                                )? {
                                    output.push(target);
                                }
                            }
                            Some(output)
                        }
                        None => None,
                    },
                    source: Some(source),
                    target_path: Some(
                        annotate_type::<Vec<cm_types::Path>>(
                            all_target_use_paths(use_, use_)
                                .ok_or_else(|| Error::internal(internal_error.clone()))?
                                .into(),
                        )
                        .iter()
                        .next()
                        .ok_or_else(|| Error::internal(internal_error.clone()))?
                        .to_string(),
                    ),
                    filter: match use_.filter.clone() {
                        Some(dict) => Some(dictionary_from_map(dict.value)?),
                        None => None,
                    },
                    availability: Some(availability),
                    ..Default::default()
                }));
            }
        } else if let Some(spanned) = &use_.runner {
            let n = &spanned.value;
            let (source, source_dictionary) =
                extract_use_source(&options, use_, all_capability_names, all_children, None)?;
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            out_uses.push(fdecl::Use::Runner(fdecl::UseRunner {
                source: Some(source),
                source_name: Some(n.clone().into()),
                source_dictionary,
                ..Default::default()
            }));
        } else if let Some(spanned) = &use_.config {
            let n = &spanned.value;
            let (source, source_dictionary) =
                extract_use_source(&options, use_, all_capability_names, all_children, None)?;
            let target = match &use_.key {
                None => {
                    return Err(Error::validate_context(
                        "\"use config\" must have \"key\" field set.",
                        Some(spanned.origin.clone()),
                    ));
                }
                Some(t) => t.clone(),
            };
            let availability = extract_use_availability(use_)?;
            let type_ = validate::use_config_to_value_type_context(use_)?;

            let default = if let Some(default) = &use_.config_default {
                let value = config_value_file::field::config_value_from_json_value(
                    &default.value,
                    &type_.clone().into(),
                )
                .map_err(|e| Error::InvalidArgs(format!("Error parsing config '{}': {}", n, e)))?;
                Some(value.native_into_fidl())
            } else {
                None
            };

            out_uses.push(fdecl::Use::Config(fdecl::UseConfiguration {
                source: Some(source),
                source_name: Some(n.clone().into()),
                target_name: Some(target.value.into()),
                availability: Some(availability),
                type_: Some(translate_value_type(&type_).0),
                default,
                source_dictionary,
                ..Default::default()
            }));
        } else if let Some(n) = &use_.dictionary {
            let (source, source_dictionary) =
                extract_use_source(options, use_, all_capability_names, all_children, None)?;
            let availability = extract_use_availability(use_)?;
            for source_name in n.value.clone().into_iter() {
                out_uses.push(fdecl::Use::Dictionary(fdecl::UseDictionary {
                    source: Some(source.clone()),
                    source_name: Some(source_name.to_string()),
                    source_dictionary: source_dictionary.clone(),
                    target_path: Some(
                        use_.path().as_ref().expect("no path on use dictionary").value.to_string(),
                    ),
                    dependency_type: Some(
                        use_.dependency
                            .clone()
                            .map(|s| s.value)
                            .unwrap_or(cm::DependencyType::Strong)
                            .into(),
                    ),
                    availability: Some(availability),
                    ..Default::default()
                }));
            }
        } else {
            return Err(Error::internal(format!("no capability in use declaration")));
        };
    }
    Ok(out_uses)
}

/// `expose` rules route a single capability from one or more sources (self|framework|#<child>) to
/// one or more targets (parent|framework).
fn translate_expose(
    options: &CompileOptions<'_>,
    expose_in: &Vec<ContextSpanned<ContextExpose>>,
    all_capability_names: &BTreeSet<&BorrowedName>,
    all_collections: &BTreeSet<&BorrowedName>,
    all_children: &BTreeSet<&BorrowedName>,
) -> Result<Vec<fdecl::Expose>, Error> {
    let mut out_exposes = vec![];
    for spanned_expose in expose_in.iter() {
        let expose = &spanned_expose.value;
        let target = extract_expose_target(expose);
        if let Some(source_names) = expose.service() {
            // When there are many `sources` exposed under the same `target_name`, aggregation
            // will happen during routing.
            let sources = extract_all_expose_sources(options, expose, Some(all_collections));
            let target_names = all_target_capability_names(expose, expose)
                .ok_or_else(|| Error::internal("no capability"))?;
            for (source_name, target_name) in
                source_names.value.into_iter().zip(target_names.into_iter())
            {
                for (source, source_dictionary) in &sources {
                    let DerivedSourceInfo { source, source_dictionary, availability } =
                        derive_source_and_availability(
                            expose.availability.as_ref(),
                            source.clone(),
                            source_dictionary.clone(),
                            expose.source_availability.as_ref(),
                            all_capability_names,
                            all_children,
                            all_collections,
                        );
                    out_exposes.push(fdecl::Expose::Service(fdecl::ExposeService {
                        source: Some(source),
                        source_name: Some(source_name.to_string()),
                        source_dictionary,
                        target_name: Some(target_name.to_string()),
                        target: Some(target.clone()),
                        availability: Some(availability),
                        ..Default::default()
                    }))
                }
            }
        } else if let Some(n) = expose.protocol() {
            let (source, source_dictionary) =
                extract_single_expose_source(options, expose, Some(all_capability_names))?;
            let source_names = n.value.into_iter();
            let target_names = all_target_capability_names(expose, expose)
                .ok_or_else(|| Error::internal("no capability"))?;
            for (source_name, target_name) in source_names.into_iter().zip(target_names.into_iter())
            {
                let DerivedSourceInfo { source, source_dictionary, availability } =
                    derive_source_and_availability(
                        expose.availability.as_ref(),
                        source.clone(),
                        source_dictionary.clone(),
                        expose.source_availability.as_ref(),
                        all_capability_names,
                        all_children,
                        all_collections,
                    );
                out_exposes.push(fdecl::Expose::Protocol(fdecl::ExposeProtocol {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    source_dictionary,
                    target_name: Some(target_name.to_string()),
                    target: Some(target.clone()),
                    availability: Some(availability),
                    ..Default::default()
                }))
            }
        } else if let Some(n) = expose.directory() {
            let (source, source_dictionary) = extract_single_expose_source(options, expose, None)?;
            let source_names = n.value.into_iter();
            let target_names = all_target_capability_names(expose, expose)
                .ok_or_else(|| Error::internal("no capability"))?;
            let rights = extract_expose_rights(expose)?;
            let subdir = extract_expose_subdir(expose);
            for (source_name, target_name) in source_names.into_iter().zip(target_names.into_iter())
            {
                let DerivedSourceInfo { source, source_dictionary, availability } =
                    derive_source_and_availability(
                        expose.availability.as_ref(),
                        source.clone(),
                        source_dictionary.clone(),
                        expose.source_availability.as_ref(),
                        all_capability_names,
                        all_children,
                        all_collections,
                    );
                out_exposes.push(fdecl::Expose::Directory(fdecl::ExposeDirectory {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    source_dictionary,
                    target_name: Some(target_name.to_string()),
                    target: Some(target.clone()),
                    rights,
                    subdir: subdir.as_ref().map(|s| s.clone().into()),
                    availability: Some(availability),
                    ..Default::default()
                }))
            }
        } else if let Some(n) = expose.runner() {
            let (source, source_dictionary) = extract_single_expose_source(options, expose, None)?;
            let source_names = n.value.into_iter();
            let target_names = all_target_capability_names(expose, expose)
                .ok_or_else(|| Error::internal("no capability"))?;
            for (source_name, target_name) in source_names.into_iter().zip(target_names.into_iter())
            {
                out_exposes.push(fdecl::Expose::Runner(fdecl::ExposeRunner {
                    source: Some(source.clone()),
                    source_name: Some(source_name.to_string()),
                    source_dictionary: source_dictionary.clone(),
                    target: Some(target.clone()),
                    target_name: Some(target_name.to_string()),
                    ..Default::default()
                }))
            }
        } else if let Some(n) = expose.resolver() {
            let (source, source_dictionary) = extract_single_expose_source(options, expose, None)?;
            let source_names = n.value.into_iter();
            let target_names = all_target_capability_names(expose, expose)
                .ok_or_else(|| Error::internal("no capability"))?;
            for (source_name, target_name) in source_names.into_iter().zip(target_names.into_iter())
            {
                out_exposes.push(fdecl::Expose::Resolver(fdecl::ExposeResolver {
                    source: Some(source.clone()),
                    source_name: Some(source_name.to_string()),
                    source_dictionary: source_dictionary.clone(),
                    target: Some(target.clone()),
                    target_name: Some(target_name.to_string()),
                    ..Default::default()
                }))
            }
        } else if let Some(n) = expose.dictionary() {
            let (source, source_dictionary) = extract_single_expose_source(options, expose, None)?;
            let source_names = n.value.into_iter();
            let target_names = all_target_capability_names(expose, expose)
                .ok_or_else(|| Error::internal("no capability"))?;
            for (source_name, target_name) in source_names.into_iter().zip(target_names.into_iter())
            {
                let DerivedSourceInfo { source, source_dictionary, availability } =
                    derive_source_and_availability(
                        expose.availability.as_ref(),
                        source.clone(),
                        source_dictionary.clone(),
                        expose.source_availability.as_ref(),
                        all_capability_names,
                        all_children,
                        all_collections,
                    );
                out_exposes.push(fdecl::Expose::Dictionary(fdecl::ExposeDictionary {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    source_dictionary,
                    target_name: Some(target_name.to_string()),
                    target: Some(target.clone()),
                    availability: Some(availability),
                    ..Default::default()
                }))
            }
        } else if let Some(n) = expose.config() {
            let (source, source_dictionary) = extract_single_expose_source(options, expose, None)?;
            let source_names = n.value.into_iter();
            let target_names = all_target_capability_names(expose, expose)
                .ok_or_else(|| Error::internal("no capability"))?;
            for (source_name, target_name) in source_names.into_iter().zip(target_names.into_iter())
            {
                let DerivedSourceInfo { source, source_dictionary, availability } =
                    derive_source_and_availability(
                        expose.availability.as_ref(),
                        source.clone(),
                        source_dictionary.clone(),
                        expose.source_availability.as_ref(),
                        all_capability_names,
                        all_children,
                        all_collections,
                    );
                out_exposes.push(fdecl::Expose::Config(fdecl::ExposeConfiguration {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    source_dictionary,
                    target: Some(target.clone()),
                    target_name: Some(target_name.to_string()),
                    availability: Some(availability),
                    ..Default::default()
                }))
            }
        } else {
            return Err(Error::internal(format!("expose: must specify a known capability")));
        }
    }
    Ok(out_exposes)
}

impl<T> Into<Vec<T>> for OneOrMany<T> {
    fn into(self) -> Vec<T> {
        match self {
            OneOrMany::One(one) => vec![one],
            OneOrMany::Many(many) => many,
        }
    }
}

/// Allows the above Into to work by annotating the type.
fn annotate_type<T>(val: T) -> T {
    val
}

struct DerivedSourceInfo {
    source: fdecl::Ref,
    source_dictionary: Option<String>,
    availability: fdecl::Availability,
}

/// If the `source` is not found and `source_availability` is `Unknown`, returns a `Void` source.
/// Otherwise, returns the source unchanged.
fn derive_source_and_availability(
    availability: Option<&ContextSpanned<Availability>>,
    source: fdecl::Ref,
    source_dictionary: Option<String>,
    source_availability: Option<&ContextSpanned<SourceAvailability>>,
    all_capability_names: &BTreeSet<&BorrowedName>,
    all_children: &BTreeSet<&BorrowedName>,
    all_collections: &BTreeSet<&BorrowedName>,
) -> DerivedSourceInfo {
    let availability = availability.map(|a| match a.value {
        Availability::Required => fdecl::Availability::Required,
        Availability::Optional => fdecl::Availability::Optional,
        Availability::SameAsTarget => fdecl::Availability::SameAsTarget,
        Availability::Transitional => fdecl::Availability::Transitional,
    });
    if source_availability.as_ref().map(|s| s.value.clone()) != Some(SourceAvailability::Unknown) {
        return DerivedSourceInfo {
            source,
            source_dictionary,
            availability: availability.unwrap_or(fdecl::Availability::Required),
        };
    }
    match &source {
        fdecl::Ref::Child(fdecl::ChildRef { name, .. })
            if !all_children.contains(name.as_str()) =>
        {
            DerivedSourceInfo {
                source: fdecl::Ref::VoidType(fdecl::VoidRef {}),
                source_dictionary: None,
                availability: availability.unwrap_or(fdecl::Availability::Optional),
            }
        }
        fdecl::Ref::Collection(fdecl::CollectionRef { name, .. })
            if !all_collections.contains(name.as_str()) =>
        {
            DerivedSourceInfo {
                source: fdecl::Ref::VoidType(fdecl::VoidRef {}),
                source_dictionary: None,
                availability: availability.unwrap_or(fdecl::Availability::Optional),
            }
        }
        fdecl::Ref::Capability(fdecl::CapabilityRef { name, .. })
            if !all_capability_names.contains(name.as_str()) =>
        {
            DerivedSourceInfo {
                source: fdecl::Ref::VoidType(fdecl::VoidRef {}),
                source_dictionary: None,
                availability: availability.unwrap_or(fdecl::Availability::Optional),
            }
        }
        _ => DerivedSourceInfo {
            source,
            source_dictionary,
            availability: availability.unwrap_or(fdecl::Availability::Required),
        },
    }
}

/// Emit a set of direct offers from `offer_to_all` for `target`, unless it
/// overlaps with an offer in `direct_offers`.
fn maybe_generate_direct_offer_from_all(
    offer_to_all: &ContextSpanned<ContextOffer>,
    direct_offers: &[ContextSpanned<ContextOffer>],
    target: &BorrowedName,
) -> Vec<ContextSpanned<ContextOffer>> {
    assert!(offer_to_all.value.protocol.is_some() || offer_to_all.value.dictionary.is_some());
    let mut returned_offers = vec![];

    let protocol_iter = offer_to_all.value.protocol.as_ref().into_iter().flat_map(|spanned| {
        let origin = spanned.origin.clone();
        spanned.value.iter().map(move |individual_protocol| {
            let mut local_offer_spanned = offer_to_all.clone();

            local_offer_spanned.value.protocol = Some(ContextSpanned {
                value: OneOrMany::One(individual_protocol.clone()),
                origin: origin.clone(),
            });
            local_offer_spanned
        })
    });

    let dict_iter = offer_to_all.value.dictionary.as_ref().into_iter().flat_map(|spanned| {
        let origin = spanned.origin.clone();
        spanned.value.iter().map(move |dictionary| {
            let mut local_offer_spanned = offer_to_all.clone();

            local_offer_spanned.value.dictionary = Some(ContextSpanned {
                value: OneOrMany::One(dictionary.clone()),
                origin: origin.clone(),
            });
            local_offer_spanned
        })
    });

    for mut local_offer_spanned in protocol_iter.chain(dict_iter) {
        let disallowed_offer_source = OfferFromRef::Named(target.into());

        if direct_offers.iter().all(|direct| {
            !offer_to_all_would_duplicate_context(&local_offer_spanned, direct, target).unwrap()
        }) && !local_offer_spanned
            .value
            .from
            .value
            .iter()
            .any(|from| from == &disallowed_offer_source)
        {
            local_offer_spanned.value.to = ContextSpanned {
                value: OneOrMany::One(OfferToRef::Named(target.into())),
                origin: local_offer_spanned.origin.clone(),
            };
            returned_offers.push(local_offer_spanned);
        }
    }

    returned_offers
}

fn expand_offer_to_all(
    offers_in: &Vec<ContextSpanned<ContextOffer>>,
    children: &BTreeSet<&BorrowedName>,
    collections: &BTreeSet<&BorrowedName>,
) -> Vec<ContextSpanned<ContextOffer>> {
    let offers_to_all = offers_in
        .iter()
        .filter(|offer| matches!(offer.value.to.value, OneOrMany::One(OfferToRef::All)));

    let mut direct_offers = offers_in
        .iter()
        .filter(|o| !matches!(o.value.to.value, OneOrMany::One(OfferToRef::All)))
        .cloned()
        .collect::<Vec<ContextSpanned<ContextOffer>>>();

    for offer_to_all in offers_to_all {
        for target in children.iter().chain(collections.iter()) {
            let offers = maybe_generate_direct_offer_from_all(offer_to_all, &direct_offers, target);
            for offer in offers {
                direct_offers.push(offer);
            }
        }
    }

    direct_offers
}

/// `offer` rules route multiple capabilities from multiple sources to multiple targets.
fn translate_offer(
    options: &CompileOptions<'_>,
    offer_in: &Vec<ContextSpanned<ContextOffer>>,
    all_capability_names: &BTreeSet<&BorrowedName>,
    all_children: &BTreeSet<&BorrowedName>,
    all_collections: &BTreeSet<&BorrowedName>,
) -> Result<Vec<fdecl::Offer>, Error> {
    let mut out_offers = vec![];
    let expanded_offers = expand_offer_to_all(offer_in, all_children, all_collections);
    for offer_spanned in &expanded_offers {
        let offer = &offer_spanned.value;
        if let Some(n) = offer.service() {
            let entries = extract_offer_sources_and_targets(
                options,
                offer,
                n.value,
                all_capability_names,
                all_children,
                all_collections,
            )?;
            for (source, source_dictionary, source_name, target, target_name) in entries {
                let DerivedSourceInfo { source, source_dictionary, availability } =
                    derive_source_and_availability(
                        offer.availability.as_ref(),
                        source,
                        source_dictionary,
                        offer.source_availability.as_ref(),
                        all_capability_names,
                        all_children,
                        all_collections,
                    );
                out_offers.push(fdecl::Offer::Service(fdecl::OfferService {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    source_dictionary,
                    target: Some(target),
                    target_name: Some(target_name.to_string()),
                    availability: Some(availability),
                    #[cfg(fuchsia_api_level_at_least = "HEAD")]
                    dependency_type: Some(
                        offer
                            .dependency
                            .clone()
                            .map(|s| s.value)
                            .unwrap_or(cm::DependencyType::Strong)
                            .into(),
                    ),
                    ..Default::default()
                }));
            }
        } else if let Some(n) = offer.protocol() {
            let entries = extract_offer_sources_and_targets(
                options,
                offer,
                n.value,
                all_capability_names,
                all_children,
                all_collections,
            )?;
            for (source, source_dictionary, source_name, target, target_name) in entries {
                let DerivedSourceInfo { source, source_dictionary, availability } =
                    derive_source_and_availability(
                        offer.availability.as_ref(),
                        source,
                        source_dictionary,
                        offer.source_availability.as_ref(),
                        all_capability_names,
                        all_children,
                        all_collections,
                    );
                out_offers.push(fdecl::Offer::Protocol(fdecl::OfferProtocol {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    source_dictionary,
                    target: Some(target),
                    target_name: Some(target_name.to_string()),
                    dependency_type: Some(
                        offer
                            .dependency
                            .clone()
                            .map(|s| s.value)
                            .unwrap_or(cm::DependencyType::Strong)
                            .into(),
                    ),
                    availability: Some(availability),
                    ..Default::default()
                }));
            }
        } else if let Some(n) = offer.directory() {
            let entries = extract_offer_sources_and_targets(
                options,
                offer,
                n.value,
                all_capability_names,
                all_children,
                all_collections,
            )?;
            for (source, source_dictionary, source_name, target, target_name) in entries {
                let DerivedSourceInfo { source, source_dictionary, availability } =
                    derive_source_and_availability(
                        offer.availability.as_ref(),
                        source,
                        source_dictionary,
                        offer.source_availability.as_ref(),
                        all_capability_names,
                        all_children,
                        all_collections,
                    );
                out_offers.push(fdecl::Offer::Directory(fdecl::OfferDirectory {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    source_dictionary,
                    target: Some(target),
                    target_name: Some(target_name.to_string()),
                    rights: extract_offer_rights(&offer)?,
                    subdir: extract_offer_subdir(&offer).map(|s| s.into()),
                    dependency_type: Some(
                        offer
                            .dependency
                            .clone()
                            .map(|s| s.value)
                            .unwrap_or(cm::DependencyType::Strong)
                            .into(),
                    ),
                    availability: Some(availability),
                    ..Default::default()
                }));
            }
        } else if let Some(n) = offer.storage() {
            let entries = extract_offer_sources_and_targets(
                options,
                offer,
                n.value,
                all_capability_names,
                all_children,
                all_collections,
            )?;
            for (source, source_dictionary, source_name, target, target_name) in entries {
                let DerivedSourceInfo { source, source_dictionary: _, availability } =
                    derive_source_and_availability(
                        offer.availability.as_ref(),
                        source,
                        source_dictionary,
                        offer.source_availability.as_ref(),
                        all_capability_names,
                        all_children,
                        all_collections,
                    );
                out_offers.push(fdecl::Offer::Storage(fdecl::OfferStorage {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    target: Some(target),
                    target_name: Some(target_name.to_string()),
                    availability: Some(availability),
                    ..Default::default()
                }));
            }
        } else if let Some(n) = offer.runner() {
            let entries = extract_offer_sources_and_targets(
                options,
                offer,
                n.value,
                all_capability_names,
                all_children,
                all_collections,
            )?;
            for (source, source_dictionary, source_name, target, target_name) in entries {
                out_offers.push(fdecl::Offer::Runner(fdecl::OfferRunner {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    source_dictionary,
                    target: Some(target),
                    target_name: Some(target_name.to_string()),
                    ..Default::default()
                }));
            }
        } else if let Some(n) = offer.resolver() {
            let entries = extract_offer_sources_and_targets(
                options,
                offer,
                n.value,
                all_capability_names,
                all_children,
                all_collections,
            )?;
            for (source, source_dictionary, source_name, target, target_name) in entries {
                out_offers.push(fdecl::Offer::Resolver(fdecl::OfferResolver {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    source_dictionary,
                    target: Some(target),
                    target_name: Some(target_name.to_string()),
                    ..Default::default()
                }));
            }
        } else if let Some(n) = offer.event_stream() {
            let entries = extract_offer_sources_and_targets(
                options,
                offer,
                n.value,
                all_capability_names,
                all_children,
                all_collections,
            )?;
            for (source, source_dictionary, source_name, target, target_name) in entries {
                let DerivedSourceInfo { source, source_dictionary: _, availability } =
                    derive_source_and_availability(
                        offer.availability.as_ref(),
                        source,
                        source_dictionary,
                        offer.source_availability.as_ref(),
                        all_capability_names,
                        all_children,
                        all_collections,
                    );
                let scopes = match offer.scope.clone() {
                    Some(value) => Some(annotate_type::<Vec<EventScope>>(value.value.into())),
                    None => None,
                };
                out_offers.push(fdecl::Offer::EventStream(fdecl::OfferEventStream {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    target: Some(target),
                    target_name: Some(target_name.to_string()),
                    scope: match scopes {
                        Some(values) => {
                            let mut output = vec![];
                            for value in &values {
                                if let Some(target) = translate_target_ref(
                                    options,
                                    value.into(),
                                    &all_children,
                                    &all_collections,
                                    &BTreeSet::new(),
                                    offer.target_availability.clone().map(|s| s.value).as_ref(),
                                )? {
                                    output.push(target);
                                }
                            }
                            Some(output)
                        }
                        None => None,
                    },
                    availability: Some(availability),
                    ..Default::default()
                }));
            }
        } else if let Some(n) = offer.dictionary() {
            let entries = extract_offer_sources_and_targets(
                options,
                offer,
                n.value,
                all_capability_names,
                all_children,
                all_collections,
            )?;
            for (source, source_dictionary, source_name, target, target_name) in entries {
                let DerivedSourceInfo { source, source_dictionary, availability } =
                    derive_source_and_availability(
                        offer.availability.as_ref(),
                        source,
                        source_dictionary,
                        offer.source_availability.as_ref(),
                        all_capability_names,
                        all_children,
                        all_collections,
                    );
                out_offers.push(fdecl::Offer::Dictionary(fdecl::OfferDictionary {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    source_dictionary,
                    target: Some(target),
                    target_name: Some(target_name.to_string()),
                    dependency_type: Some(
                        offer
                            .dependency
                            .clone()
                            .map(|s| s.value)
                            .unwrap_or(cm::DependencyType::Strong)
                            .into(),
                    ),
                    availability: Some(availability),
                    ..Default::default()
                }));
            }
        } else if let Some(n) = offer.config() {
            let entries = extract_offer_sources_and_targets(
                options,
                offer,
                n.value,
                all_capability_names,
                all_children,
                all_collections,
            )?;
            for (source, source_dictionary, source_name, target, target_name) in entries {
                let DerivedSourceInfo { source, source_dictionary, availability } =
                    derive_source_and_availability(
                        offer.availability.as_ref(),
                        source,
                        source_dictionary,
                        offer.source_availability.as_ref(),
                        all_capability_names,
                        all_children,
                        all_collections,
                    );
                out_offers.push(fdecl::Offer::Config(fdecl::OfferConfiguration {
                    source: Some(source),
                    source_name: Some(source_name.to_string()),
                    target: Some(target),
                    target_name: Some(target_name.to_string()),
                    availability: Some(availability),
                    source_dictionary,
                    ..Default::default()
                }));
            }
        } else {
            return Err(Error::internal(format!("no capability")));
        }
    }
    Ok(out_offers)
}

fn translate_children(children_in: &Vec<ContextSpanned<ContextChild>>) -> Vec<fdecl::Child> {
    let mut out_children = vec![];
    for child_raw in children_in.iter() {
        let child = &child_raw.value;
        out_children.push(fdecl::Child {
            name: Some(child.name.value.clone().into()),
            url: Some(child.url.value.clone().into()),
            startup: Some(child.startup.value.clone().into()),
            environment: extract_environment_ref(child.environment.as_ref()).map(|e| e.into()),
            on_terminate: child.on_terminate.as_ref().map(|r| r.value.clone().into()),
            ..Default::default()
        });
    }
    out_children
}

fn translate_collections(
    collections_in: &Vec<ContextSpanned<ContextCollection>>,
) -> Vec<fdecl::Collection> {
    let mut out_collections = vec![];
    for collection_raw in collections_in.iter() {
        let collection = &collection_raw.value;
        out_collections.push(fdecl::Collection {
            name: Some(collection.name.value.clone().into()),
            durability: Some(collection.durability.value.clone().into()),
            environment: extract_environment_ref(collection.environment.as_ref()).map(|e| e.into()),
            allowed_offers: collection.allowed_offers.as_ref().map(|a| a.value.clone().into()),
            allow_long_names: collection.allow_long_names.as_ref().map(|a| a.value.into()),
            persistent_storage: collection.persistent_storage.as_ref().map(|a| a.value.into()),
            ..Default::default()
        });
    }
    out_collections
}

/// Translates a nested value type to a [`fuchsia.config.decl.ConfigType`]
fn translate_nested_value_type(nested_type: &ConfigNestedValueType) -> fdecl::ConfigType {
    let layout = match nested_type {
        ConfigNestedValueType::Bool {} => fdecl::ConfigTypeLayout::Bool,
        ConfigNestedValueType::Uint8 {} => fdecl::ConfigTypeLayout::Uint8,
        ConfigNestedValueType::Uint16 {} => fdecl::ConfigTypeLayout::Uint16,
        ConfigNestedValueType::Uint32 {} => fdecl::ConfigTypeLayout::Uint32,
        ConfigNestedValueType::Uint64 {} => fdecl::ConfigTypeLayout::Uint64,
        ConfigNestedValueType::Int8 {} => fdecl::ConfigTypeLayout::Int8,
        ConfigNestedValueType::Int16 {} => fdecl::ConfigTypeLayout::Int16,
        ConfigNestedValueType::Int32 {} => fdecl::ConfigTypeLayout::Int32,
        ConfigNestedValueType::Int64 {} => fdecl::ConfigTypeLayout::Int64,
        ConfigNestedValueType::String { .. } => fdecl::ConfigTypeLayout::String,
    };
    let constraints = match nested_type {
        ConfigNestedValueType::String { max_size } => {
            vec![fdecl::LayoutConstraint::MaxSize(max_size.get())]
        }
        _ => vec![],
    };
    fdecl::ConfigType {
        layout,
        constraints,
        // This optional is not necessary, but without it,
        // FIDL compilation complains because of a possible include-cycle.
        // Bug: https://fxbug.dev/42145148
        parameters: Some(vec![]),
    }
}

/// Translates a value type to a [`fuchsia.sys2.ConfigType`]
fn translate_value_type(
    value_type: &ConfigValueType,
) -> (fdecl::ConfigType, fdecl::ConfigMutability) {
    let (layout, source_mutability) = match value_type {
        ConfigValueType::Bool { mutability } => (fdecl::ConfigTypeLayout::Bool, mutability),
        ConfigValueType::Uint8 { mutability } => (fdecl::ConfigTypeLayout::Uint8, mutability),
        ConfigValueType::Uint16 { mutability } => (fdecl::ConfigTypeLayout::Uint16, mutability),
        ConfigValueType::Uint32 { mutability } => (fdecl::ConfigTypeLayout::Uint32, mutability),
        ConfigValueType::Uint64 { mutability } => (fdecl::ConfigTypeLayout::Uint64, mutability),
        ConfigValueType::Int8 { mutability } => (fdecl::ConfigTypeLayout::Int8, mutability),
        ConfigValueType::Int16 { mutability } => (fdecl::ConfigTypeLayout::Int16, mutability),
        ConfigValueType::Int32 { mutability } => (fdecl::ConfigTypeLayout::Int32, mutability),
        ConfigValueType::Int64 { mutability } => (fdecl::ConfigTypeLayout::Int64, mutability),
        ConfigValueType::String { mutability, .. } => (fdecl::ConfigTypeLayout::String, mutability),
        ConfigValueType::Vector { mutability, .. } => (fdecl::ConfigTypeLayout::Vector, mutability),
    };
    let (constraints, parameters) = match value_type {
        ConfigValueType::String { max_size, .. } => {
            (vec![fdecl::LayoutConstraint::MaxSize(max_size.get())], vec![])
        }
        ConfigValueType::Vector { max_count, element, .. } => {
            let nested_type = translate_nested_value_type(element);
            (
                vec![fdecl::LayoutConstraint::MaxSize(max_count.get())],
                vec![fdecl::LayoutParameter::NestedType(nested_type)],
            )
        }
        _ => (vec![], vec![]),
    };
    let mut mutability = fdecl::ConfigMutability::empty();
    if let Some(source_mutability) = source_mutability {
        for source in source_mutability {
            match source {
                ConfigRuntimeSource::Parent => mutability |= fdecl::ConfigMutability::PARENT,
            }
        }
    }
    (
        fdecl::ConfigType {
            layout,
            constraints,
            // This optional is not necessary, but without it,
            // FIDL compilation complains because of a possible include-cycle.
            // Bug: https://fxbug.dev/42145148
            parameters: Some(parameters),
        },
        mutability,
    )
}

/// Create the `fdecl::ConfigSchema` from the fields of the `config` block and the config capability
/// Use decls.
fn translate_config(
    fields: &Option<BTreeMap<ConfigKey, ContextSpanned<ConfigValueType>>>,
    uses: &Option<Vec<ContextSpanned<ContextUse>>>,
    package_path: &Option<String>,
) -> Result<Option<fdecl::ConfigSchema>, Error> {
    let mut use_fields: BTreeMap<ConfigKey, ContextSpanned<ConfigValueType>> = uses
        .iter()
        .flatten()
        .filter_map(|u| {
            if u.value.config.is_none() {
                return None;
            }
            let key = ConfigKey(u.value.key.clone().expect("key should be set").value.into());

            let config_type_raw = validate::use_config_to_value_type_context(&u.value)
                .expect("config type should be valid");

            let config_type = ContextSpanned { value: config_type_raw, origin: u.origin.clone() };

            Some((key, config_type))
        })
        .collect();

    for (key, value) in fields.iter().flatten() {
        if use_fields.contains_key(key) {
            if use_fields.get(key).map(|v| &v.value) != Some(&value.value) {
                return Err(Error::validate_context(
                    format!(
                        "Config error: `use` and `config` block contain key '{}' with different types",
                        key
                    ),
                    Some(value.origin.clone()),
                ));
            }
        }
        use_fields.insert(key.clone(), value.clone());
    }

    if use_fields.is_empty() {
        return Ok(None);
    }

    let source = match fields.as_ref().map_or(true, |f| f.is_empty()) {
        true => fdecl::ConfigValueSource::Capabilities(fdecl::ConfigSourceCapabilities::default()),
        _ => {
            let Some(package_path) = package_path.as_ref() else {
                return Err(Error::invalid_args(
                    "can't translate config: no package path for value file",
                ));
            };
            fdecl::ConfigValueSource::PackagePath(package_path.to_owned())
        }
    };

    let mut fidl_fields = vec![];
    let mut hasher = Sha256::new();

    for (key, value) in &use_fields {
        let (type_, mutability) = translate_value_type(&value.value);

        fidl_fields.push(fdecl::ConfigField {
            key: Some(key.to_string()),
            type_: Some(type_),
            mutability: Some(mutability),
            ..Default::default()
        });

        hasher.update(key.as_str());
        value.value.update_digest(&mut hasher);
    }

    let hash = hasher.finalize();
    let checksum = fdecl::ConfigChecksum::Sha256(*hash.as_ref());

    Ok(Some(fdecl::ConfigSchema {
        fields: Some(fidl_fields),
        checksum: Some(checksum),
        value_source: Some(source),
        ..Default::default()
    }))
}

fn translate_environments(
    options: &CompileOptions<'_>,
    envs_in: &Vec<ContextSpanned<ContextEnvironment>>,
    all_capability_names: &BTreeSet<&BorrowedName>,
) -> Result<Vec<fdecl::Environment>, Error> {
    envs_in
        .iter()
        .map(|cs_env| {
            let env = &cs_env.value;
            Ok(fdecl::Environment {
                name: Some(env.name.value.clone().into()),
                extends: match &env.extends {
                    Some(spanned) => match spanned.value {
                        EnvironmentExtends::Realm => Some(fdecl::EnvironmentExtends::Realm),
                        EnvironmentExtends::None => Some(fdecl::EnvironmentExtends::None),
                    },
                    None => Some(fdecl::EnvironmentExtends::None),
                },
                runners: env
                    .runners
                    .as_ref()
                    .map(|runners| {
                        runners
                            .iter()
                            .map(|r| translate_runner_registration(options, &r.value))
                            .collect::<Result<Vec<_>, Error>>()
                    })
                    .transpose()?,
                resolvers: env
                    .resolvers
                    .as_ref()
                    .map(|resolvers| {
                        resolvers
                            .iter()
                            .map(|r| translate_resolver_registration(options, &r.value))
                            .collect::<Result<Vec<_>, Error>>()
                    })
                    .transpose()?,
                debug_capabilities: env
                    .debug
                    .as_ref()
                    .map(|debug_capabiltities| {
                        translate_debug_capabilities(
                            options,
                            debug_capabiltities,
                            all_capability_names,
                        )
                    })
                    .transpose()?,
                stop_timeout_ms: env.stop_timeout_ms.clone().map(|s| s.value.0),
                ..Default::default()
            })
        })
        .collect()
}

fn translate_runner_registration(
    options: &CompileOptions<'_>,
    reg: &ContextRunnerRegistration,
) -> Result<fdecl::RunnerRegistration, Error> {
    let (source, _source_dictionary) = extract_single_offer_source(options, reg, None)?;
    Ok(fdecl::RunnerRegistration {
        source_name: Some(reg.runner.value.clone().into()),
        source: Some(source),
        target_name: Some(
            reg.r#as.as_ref().map(|s| &s.value).unwrap_or(&reg.runner.value).to_string(),
        ),
        ..Default::default()
    })
}

fn translate_resolver_registration(
    options: &CompileOptions<'_>,
    reg: &ContextResolverRegistration,
) -> Result<fdecl::ResolverRegistration, Error> {
    let (source, _source_dictionary) = extract_single_offer_source(options, reg, None)?;
    Ok(fdecl::ResolverRegistration {
        resolver: Some(reg.resolver.value.clone().into()),
        source: Some(source),
        scheme: Some(
            reg.scheme
                .value
                .as_str()
                .parse::<cm_types::UrlScheme>()
                .map_err(|e| Error::internal(format!("invalid URL scheme: {}", e)))?
                .into(),
        ),
        ..Default::default()
    })
}

fn translate_debug_capabilities(
    options: &CompileOptions<'_>,
    capabilities: &Vec<ContextSpanned<ContextDebugRegistration>>,
    all_capability_names: &BTreeSet<&BorrowedName>,
) -> Result<Vec<fdecl::DebugRegistration>, Error> {
    let mut out_capabilities = vec![];
    for spanned_capability in capabilities {
        let capability = &spanned_capability.value;
        if let Some(n) = capability.protocol() {
            let (source, _source_dictionary) =
                extract_single_offer_source(options, capability, Some(all_capability_names))?;
            let targets = all_target_capability_names(capability, capability)
                .ok_or_else(|| Error::internal("no capability"))?;
            let source_names = n;
            for target_name in targets {
                // When multiple source names are provided, there is no way to alias each one, so
                // source_name == target_name.
                // When one source name is provided, source_name may be aliased to a different
                // target_name, so we source_names[0] to derive the source_name.
                //
                // TODO: This logic could be simplified to use iter::zip() if
                // extract_all_targets_for_each_child returned separate vectors for targets and
                // target_names instead of the cross product of them.
                let source_name = if source_names.value.len() == 1 {
                    *source_names.value.iter().next().unwrap()
                } else {
                    target_name
                };
                out_capabilities.push(fdecl::DebugRegistration::Protocol(
                    fdecl::DebugProtocolRegistration {
                        source: Some(source.clone()),
                        source_name: Some(source_name.to_string()),
                        target_name: Some(target_name.to_string()),
                        ..Default::default()
                    },
                ));
            }
        }
    }
    Ok(out_capabilities)
}

fn extract_use_source(
    options: &CompileOptions<'_>,
    in_obj: &ContextUse,
    all_capability_names: &BTreeSet<&BorrowedName>,
    all_children_names: &BTreeSet<&BorrowedName>,
    all_collection_names: Option<&BTreeSet<&BorrowedName>>,
) -> Result<(fdecl::Ref, Option<String>), Error> {
    let ref_ = match in_obj.from.as_ref() {
        Some(spanned) => match &spanned.value {
            UseFromRef::Parent => fdecl::Ref::Parent(fdecl::ParentRef {}),
            UseFromRef::Framework => fdecl::Ref::Framework(fdecl::FrameworkRef {}),
            UseFromRef::Debug => fdecl::Ref::Debug(fdecl::DebugRef {}),
            UseFromRef::Self_ => fdecl::Ref::Self_(fdecl::SelfRef {}),
            UseFromRef::Named(name) => {
                if all_children_names.contains::<BorrowedName>(name.as_ref()) {
                    fdecl::Ref::Child(fdecl::ChildRef {
                        name: name.clone().into(),
                        collection: None,
                    })
                } else if all_collection_names.is_some()
                    && all_collection_names.unwrap().contains::<BorrowedName>(name.as_ref())
                {
                    fdecl::Ref::Collection(fdecl::CollectionRef { name: name.to_string() })
                } else if all_capability_names.contains::<BorrowedName>(name.as_ref()) {
                    fdecl::Ref::Capability(fdecl::CapabilityRef { name: name.to_string() })
                } else {
                    return Err(Error::validate_context(
                        format!(
                            "use: from value \"{}\" does not match any child, collection, or capability",
                            name
                        ),
                        Some(spanned.origin.clone()),
                    ));
                }
            }
            UseFromRef::Dictionary(d) => {
                return Ok(dictionary_ref_to_source(&d));
            }
        },
        None => fdecl::Ref::Parent(fdecl::ParentRef {}), // Default value.
    };
    Ok((ref_, None))
}

fn extract_use_availability(in_obj: &ContextUse) -> Result<fdecl::Availability, Error> {
    match in_obj.availability.as_ref() {
        Some(spanned) => match spanned.value {
            Availability::Required => Ok(fdecl::Availability::Required),
            Availability::Optional => Ok(fdecl::Availability::Optional),
            Availability::Transitional => Ok(fdecl::Availability::Transitional),
            Availability::SameAsTarget => Err(Error::internal(
                "availability \"same_as_target\" not supported for use declarations",
            )),
        },
        None => Ok(fdecl::Availability::Required),
    }
}

fn extract_use_subdir(in_obj: &ContextUse) -> Option<cm::RelativePath> {
    in_obj.subdir.clone().map(|s| s.value)
}

fn extract_expose_subdir(in_obj: &ContextExpose) -> Option<cm::RelativePath> {
    in_obj.subdir.clone().map(|s| s.value)
}

fn extract_offer_subdir(in_obj: &ContextOffer) -> Option<cm::RelativePath> {
    in_obj.subdir.clone().map(|s| s.value)
}

fn extract_expose_rights(in_obj: &ContextExpose) -> Result<Option<fio::Operations>, Error> {
    match in_obj.rights.as_ref() {
        Some(spanned) => {
            let rights_tokens = &spanned.value;
            let mut rights = Vec::new();
            for token in rights_tokens.0.iter() {
                rights.append(&mut token.expand())
            }
            if rights.is_empty() {
                return Err(Error::missing_rights(
                    "Rights provided to expose are not well formed.",
                ));
            }
            let mut seen_rights = BTreeSet::new();
            let mut operations: fio::Operations = fio::Operations::empty();
            for right in rights.iter() {
                if seen_rights.contains(&right) {
                    return Err(Error::duplicate_rights(
                        "Rights provided to expose are not well formed.",
                    ));
                }
                seen_rights.insert(right);
                operations |= *right;
            }

            Ok(Some(operations))
        }
        // Unlike use rights, expose rights can take a None value
        None => Ok(None),
    }
}

fn expose_source_from_ref(
    options: &CompileOptions<'_>,
    reference: &ExposeFromRef,
    all_capability_names: Option<&BTreeSet<&BorrowedName>>,
    all_collections: Option<&BTreeSet<&BorrowedName>>,
) -> (fdecl::Ref, Option<String>) {
    let ref_ = match reference {
        ExposeFromRef::Named(name) => {
            if all_capability_names.is_some()
                && all_capability_names.unwrap().contains::<BorrowedName>(name.as_ref())
            {
                fdecl::Ref::Capability(fdecl::CapabilityRef { name: name.to_string() })
            } else if all_collections.is_some()
                && all_collections.unwrap().contains::<BorrowedName>(name.as_ref())
            {
                fdecl::Ref::Collection(fdecl::CollectionRef { name: name.to_string() })
            } else {
                fdecl::Ref::Child(fdecl::ChildRef { name: name.to_string(), collection: None })
            }
        }
        ExposeFromRef::Framework => fdecl::Ref::Framework(fdecl::FrameworkRef {}),
        ExposeFromRef::Self_ => fdecl::Ref::Self_(fdecl::SelfRef {}),
        ExposeFromRef::Void => fdecl::Ref::VoidType(fdecl::VoidRef {}),
        ExposeFromRef::Dictionary(d) => {
            return dictionary_ref_to_source(&d);
        }
    };
    (ref_, None)
}

fn extract_single_expose_source(
    options: &CompileOptions<'_>,
    in_obj: &ContextExpose,
    all_capability_names: Option<&BTreeSet<&BorrowedName>>,
) -> Result<(fdecl::Ref, Option<String>), Error> {
    match &in_obj.from.value {
        OneOrMany::One(reference) => {
            Ok(expose_source_from_ref(options, &reference, all_capability_names, None))
        }
        OneOrMany::Many(many) => Err(Error::internal(format!(
            "multiple unexpected \"from\" clauses for \"expose\": {:?}",
            many
        ))),
    }
}

fn extract_all_expose_sources(
    options: &CompileOptions<'_>,
    in_obj: &ContextExpose,
    all_collections: Option<&BTreeSet<&BorrowedName>>,
) -> Vec<(fdecl::Ref, Option<String>)> {
    in_obj
        .from
        .value
        .iter()
        .map(|e| expose_source_from_ref(options, e, None, all_collections))
        .collect()
}

fn extract_offer_rights(in_obj: &ContextOffer) -> Result<Option<fio::Operations>, Error> {
    match in_obj.rights.as_ref() {
        Some(cs_rights) => {
            let rights_token = &cs_rights.value;
            let mut rights = Vec::new();
            for token in rights_token.0.iter() {
                rights.append(&mut token.expand())
            }
            if rights.is_empty() {
                return Err(Error::missing_rights("Rights provided to offer are not well formed."));
            }
            let mut seen_rights = BTreeSet::new();
            let mut operations: fio::Operations = fio::Operations::empty();
            for right in rights.iter() {
                if seen_rights.contains(&right) {
                    return Err(Error::duplicate_rights(
                        "Rights provided to offer are not well formed.",
                    ));
                }
                seen_rights.insert(right);
                operations |= *right;
            }

            Ok(Some(operations))
        }
        // Unlike use rights, offer rights can take a None value
        None => Ok(None),
    }
}

fn extract_single_offer_source<T>(
    options: &CompileOptions<'_>,
    in_obj: &T,
    all_capability_names: Option<&BTreeSet<&BorrowedName>>,
) -> Result<(fdecl::Ref, Option<String>), Error>
where
    T: FromClauseContext,
{
    match in_obj.from_().value {
        OneOrMany::One(reference) => {
            Ok(any_ref_to_decl(options, reference, all_capability_names, None))
        }
        many => {
            return Err(Error::internal(format!(
                "multiple unexpected \"from\" clauses for \"offer\": {}",
                many
            )));
        }
    }
}

fn extract_all_offer_sources<T: FromClauseContext>(
    options: &CompileOptions<'_>,
    in_obj: &T,
    all_capability_names: &BTreeSet<&BorrowedName>,
    all_collections: &BTreeSet<&BorrowedName>,
) -> Vec<(fdecl::Ref, Option<String>)> {
    in_obj
        .from_()
        .value
        .into_iter()
        .map(|r| {
            any_ref_to_decl(options, r.clone(), Some(all_capability_names), Some(all_collections))
        })
        .collect()
}

fn translate_target_ref(
    options: &CompileOptions<'_>,
    reference: AnyRef<'_>,
    all_children: &BTreeSet<&BorrowedName>,
    all_collections: &BTreeSet<&BorrowedName>,
    all_capabilities: &BTreeSet<&BorrowedName>,
    target_availability: Option<&TargetAvailability>,
) -> Result<Option<fdecl::Ref>, Error> {
    match reference {
        AnyRef::Named(name) if all_children.contains::<BorrowedName>(name) => {
            Ok(Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: name.to_string(),
                collection: None,
            })))
        }
        AnyRef::Named(name) if all_collections.contains::<BorrowedName>(name) => {
            Ok(Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: name.to_string() })))
        }
        AnyRef::Named(name) if all_capabilities.contains::<BorrowedName>(name) => {
            Ok(Some(fdecl::Ref::Capability(fdecl::CapabilityRef { name: name.to_string() })))
        }
        AnyRef::OwnDictionary(name) if all_capabilities.contains::<BorrowedName>(name) => {
            Ok(Some(fdecl::Ref::Capability(fdecl::CapabilityRef { name: name.to_string() })))
        }
        AnyRef::Named(_) | AnyRef::OwnDictionary(_)
            if target_availability == Some(&TargetAvailability::Unknown) =>
        {
            Ok(None)
        }
        AnyRef::Named(_) => Err(Error::internal(format!("dangling reference: \"{}\"", reference))),
        _ => Err(Error::internal(format!("invalid child reference: \"{}\"", reference))),
    }
}

// Return a list of (source, source capability id, source dictionary, target,
// target capability id) expressed in the `offer`.
fn extract_offer_sources_and_targets<'a>(
    options: &CompileOptions<'_>,
    offer: &'a ContextOffer,
    source_names: OneOrMany<&'a BorrowedName>,
    all_capability_names: &BTreeSet<&BorrowedName>,
    all_children: &BTreeSet<&BorrowedName>,
    all_collections: &BTreeSet<&BorrowedName>,
) -> Result<Vec<(fdecl::Ref, Option<String>, &'a BorrowedName, fdecl::Ref, &'a BorrowedName)>, Error>
{
    let mut out = vec![];

    let sources = extract_all_offer_sources(options, offer, all_capability_names, all_collections);
    let target_names = all_target_capability_names(offer, offer)
        .ok_or_else(|| Error::internal("no capability".to_string()))?;

    for (source, source_dictionary) in sources {
        for to in &offer.to.value {
            for target_name in &target_names {
                // When multiple source names are provided, there is no way to alias each one,
                // so we can assume source_name == target_name.  When one source name is provided,
                // source_name may be aliased to a different target_name, so we use
                // source_names[0] to obtain the source_name.
                let source_name = if source_names.len() == 1 {
                    source_names.iter().next().unwrap()
                } else {
                    target_name
                };
                if let Some(target) = translate_target_ref(
                    options,
                    to.into(),
                    all_children,
                    all_collections,
                    all_capability_names,
                    offer.target_availability.clone().map(|s| s.value).as_ref(),
                )? {
                    out.push((
                        source.clone(),
                        source_dictionary.clone(),
                        *source_name,
                        target,
                        *target_name,
                    ));
                }
            }
        }
    }
    Ok(out)
}

/// Return the target paths specified in the given use declaration.
fn all_target_use_paths<T, U>(in_obj: &T, to_obj: &U) -> Option<OneOrMany<Path>>
where
    T: ContextCapabilityClause,
    U: ContextPathClause,
{
    if let Some(n) = in_obj.service() {
        Some(svc_paths_from_names(n.value, to_obj))
    } else if let Some(n) = in_obj.protocol() {
        Some(svc_paths_from_names(n.value, to_obj))
    } else if let Some(_) = in_obj.directory() {
        let path = &to_obj.path().expect("no path on use directory").value;
        Some(OneOrMany::One(path.clone()))
    } else if let Some(_) = in_obj.storage() {
        let path = &to_obj.path().expect("no path on use storage").value;
        Some(OneOrMany::One(path.clone()))
    } else if let Some(_) = in_obj.event_stream() {
        let default_path = Path::new("/svc/fuchsia.component.EventStream").unwrap();
        let path = to_obj.path().map(|s| &s.value).unwrap_or(&default_path);
        Some(OneOrMany::One(path.clone()))
    } else {
        None
    }
}

/// Returns the list of paths derived from a `use` declaration with `names` and `to_obj`. `to_obj`
/// must be a declaration that has a `path` clause.
fn svc_paths_from_names<T>(names: OneOrMany<&BorrowedName>, to_obj: &T) -> OneOrMany<Path>
where
    T: ContextPathClause,
{
    match names {
        OneOrMany::One(n) => {
            if let Some(path) = to_obj.path() {
                OneOrMany::One(path.value.clone())
            } else {
                OneOrMany::One(format!("/svc/{}", n).parse().unwrap())
            }
        }
        OneOrMany::Many(v) => {
            let many = v.iter().map(|n| format!("/svc/{}", n).parse().unwrap()).collect();
            OneOrMany::Many(many)
        }
    }
}

/// Return the single target path specified in the given use declaration.
fn one_target_use_path<T, U>(in_obj: &T, to_obj: &U) -> Result<Path, Error>
where
    T: ContextCapabilityClause,
    U: ContextPathClause,
{
    match all_target_use_paths(in_obj, to_obj) {
        Some(OneOrMany::One(target_name)) => Ok(target_name),
        Some(OneOrMany::Many(_)) => {
            Err(Error::internal("expecting one capability, but multiple provided"))
        }
        _ => Err(Error::internal("expecting one capability, but none provided")),
    }
}

/// Return the target names or paths specified in the given capability.
fn all_target_capability_names<'a, T, U>(
    in_obj: &'a T,
    to_obj: &'a U,
) -> Option<OneOrMany<&'a BorrowedName>>
where
    T: ContextCapabilityClause,
    U: AsClauseContext + ContextPathClause,
{
    if let Some(as_) = to_obj.r#as() {
        // We've already validated that when `as` is specified, only 1 source id exists.
        Some(OneOrMany::One(as_.value))
    } else {
        if let Some(n) = in_obj.service() {
            Some(n.value)
        } else if let Some(n) = in_obj.protocol() {
            Some(n.value)
        } else if let Some(n) = in_obj.directory() {
            Some(n.value)
        } else if let Some(n) = in_obj.storage() {
            Some(n.value)
        } else if let Some(n) = in_obj.runner() {
            Some(n.value)
        } else if let Some(n) = in_obj.resolver() {
            Some(n.value)
        } else if let Some(n) = in_obj.event_stream() {
            Some(n.value)
        } else if let Some(n) = in_obj.dictionary() {
            Some(n.value)
        } else if let Some(n) = in_obj.config() {
            Some(n.value)
        } else {
            None
        }
    }
}

fn extract_expose_target(in_obj: &ContextExpose) -> fdecl::Ref {
    match &in_obj.to {
        Some(spanned) => match &spanned.value {
            ExposeToRef::Parent => fdecl::Ref::Parent(fdecl::ParentRef {}),
            ExposeToRef::Framework => fdecl::Ref::Framework(fdecl::FrameworkRef {}),
        },
        None => fdecl::Ref::Parent(fdecl::ParentRef {}),
    }
}

fn extract_environment_ref(r: Option<&ContextSpanned<EnvironmentRef>>) -> Option<cm::Name> {
    r.map(|r| {
        let EnvironmentRef::Named(name) = &r.value;
        name.clone()
    })
}

pub fn translate_capabilities(
    options: &CompileOptions<'_>,
    capabilities_in: &Vec<ContextSpanned<ContextCapability>>,
    as_builtin: bool,
) -> Result<Vec<fdecl::Capability>, Error> {
    let mut out_capabilities = vec![];
    for cs_capability in capabilities_in {
        let capability = &cs_capability.value;
        if let Some(service) = &capability.service {
            for n in service.value.iter() {
                let source_path = match as_builtin {
                    true => None,
                    false => Some(
                        capability
                            .path
                            .clone()
                            .map(|s| s.value)
                            .unwrap_or_else(|| format!("/svc/{}", n).parse().unwrap())
                            .into(),
                    ),
                };
                out_capabilities.push(fdecl::Capability::Service(fdecl::Service {
                    name: Some(n.clone().into()),
                    source_path,
                    ..Default::default()
                }));
            }
        } else if let Some(protocol) = &capability.protocol {
            for n in protocol.value.iter() {
                let source_path = match as_builtin {
                    true => None,
                    false => Some(
                        capability
                            .path
                            .clone()
                            .map(|s| s.value)
                            .unwrap_or_else(|| format!("/svc/{}", n).parse().unwrap())
                            .into(),
                    ),
                };
                out_capabilities.push(fdecl::Capability::Protocol(fdecl::Protocol {
                    name: Some(n.clone().into()),
                    source_path,
                    #[cfg(fuchsia_api_level_at_least = "HEAD")]
                    delivery: capability.delivery.as_ref().map(|s| s.value.into()),
                    ..Default::default()
                }));
            }
        } else if let Some(n) = &capability.directory {
            let source_path = match as_builtin {
                true => None,
                false => Some(
                    capability.path.as_ref().expect("missing source path").value.clone().into(),
                ),
            };
            let rights = extract_required_rights(capability, "capability")?;
            out_capabilities.push(fdecl::Capability::Directory(fdecl::Directory {
                name: Some(n.value.clone().into()),
                source_path,
                rights: Some(rights),
                ..Default::default()
            }));
        } else if let Some(n) = &capability.storage {
            if as_builtin {
                return Err(Error::internal(format!(
                    "built-in storage capabilities are not supported"
                )));
            }
            let backing_dir = capability
                .backing_dir
                .as_ref()
                .expect("storage has no path or backing_dir")
                .value
                .clone()
                .into();

            let (source, _source_dictionary) = any_ref_to_decl(
                options,
                (&capability.from.as_ref().unwrap().value).into(),
                None,
                None,
            );
            out_capabilities.push(fdecl::Capability::Storage(fdecl::Storage {
                name: Some(n.value.clone().into()),
                backing_dir: Some(backing_dir),
                subdir: capability.subdir.as_ref().map(|s| s.value.clone().into()),
                source: Some(source),
                storage_id: Some(
                    capability
                        .storage_id
                        .as_ref()
                        .expect("storage is missing storage_id")
                        .value
                        .clone()
                        .into(),
                ),
                ..Default::default()
            }));
        } else if let Some(n) = &capability.runner {
            let source_path = match as_builtin {
                true => None,
                false => Some(
                    capability.path.as_ref().expect("missing source path").value.clone().into(),
                ),
            };
            out_capabilities.push(fdecl::Capability::Runner(fdecl::Runner {
                name: Some(n.value.clone().into()),
                source_path,
                ..Default::default()
            }));
        } else if let Some(n) = &capability.resolver {
            let source_path = match as_builtin {
                true => None,
                false => Some(
                    capability.path.as_ref().expect("missing source path").value.clone().into(),
                ),
            };
            out_capabilities.push(fdecl::Capability::Resolver(fdecl::Resolver {
                name: Some(n.value.clone().into()),
                source_path,
                ..Default::default()
            }));
        } else if let Some(ns) = &capability.event_stream {
            if !as_builtin {
                return Err(Error::internal(format!(
                    "event_stream capabilities may only be declared as built-in capabilities"
                )));
            }
            for n in &ns.value {
                out_capabilities.push(fdecl::Capability::EventStream(fdecl::EventStream {
                    name: Some(n.clone().into()),
                    ..Default::default()
                }));
            }
        } else if let Some(n) = &capability.dictionary {
            out_capabilities.push(fdecl::Capability::Dictionary(fdecl::Dictionary {
                name: Some(n.value.clone().into()),
                source_path: capability.path.as_ref().map(|s| s.value.clone().into()),
                ..Default::default()
            }));
        } else if let Some(c) = &capability.config {
            let value = configuration_to_value(
                &c.value,
                &capability,
                &capability.config_type,
                &capability.value,
            )?;
            out_capabilities.push(fdecl::Capability::Config(fdecl::Configuration {
                name: Some(c.value.clone().into()),
                value: Some(value),
                ..Default::default()
            }));
        } else {
            return Err(Error::internal(format!("no capability declaration recognized")));
        }
    }
    Ok(out_capabilities)
}

pub fn extract_required_rights<T>(in_obj: &T, keyword: &str) -> Result<fio::Operations, Error>
where
    T: RightsClause,
{
    match in_obj.rights() {
        Some(rights_tokens) => {
            let mut rights = Vec::new();
            for token in rights_tokens.0.iter() {
                rights.append(&mut token.expand())
            }
            if rights.is_empty() {
                return Err(Error::missing_rights(format!(
                    "Rights provided to `{}` are not well formed.",
                    keyword
                )));
            }
            let mut seen_rights = BTreeSet::new();
            let mut operations: fio::Operations = fio::Operations::empty();
            for right in rights.iter() {
                if seen_rights.contains(&right) {
                    return Err(Error::duplicate_rights(format!(
                        "Rights provided to `{}` are not well formed.",
                        keyword
                    )));
                }
                seen_rights.insert(right);
                operations |= *right;
            }

            Ok(operations)
        }
        None => Err(Error::internal(format!(
            "No `{}` rights provided but required for directories",
            keyword
        ))),
    }
}

/// Takes an `AnyRef` and returns the `fdecl::Ref` equivalent and the dictionary path, if
/// the ref was a dictionary ref.
pub fn any_ref_to_decl(
    options: &CompileOptions<'_>,
    reference: AnyRef<'_>,
    all_capability_names: Option<&BTreeSet<&BorrowedName>>,
    all_collection_names: Option<&BTreeSet<&BorrowedName>>,
) -> (fdecl::Ref, Option<String>) {
    let ref_ = match reference {
        AnyRef::Named(name) => {
            if all_capability_names.is_some()
                && all_capability_names.unwrap().contains::<BorrowedName>(name)
            {
                fdecl::Ref::Capability(fdecl::CapabilityRef { name: name.to_string() })
            } else if all_collection_names.is_some()
                && all_collection_names.unwrap().contains::<BorrowedName>(name)
            {
                fdecl::Ref::Collection(fdecl::CollectionRef { name: name.to_string() })
            } else {
                fdecl::Ref::Child(fdecl::ChildRef { name: name.to_string(), collection: None })
            }
        }
        AnyRef::Framework => fdecl::Ref::Framework(fdecl::FrameworkRef {}),
        AnyRef::Debug => fdecl::Ref::Debug(fdecl::DebugRef {}),
        AnyRef::Parent => fdecl::Ref::Parent(fdecl::ParentRef {}),
        AnyRef::Self_ => fdecl::Ref::Self_(fdecl::SelfRef {}),
        AnyRef::Void => fdecl::Ref::VoidType(fdecl::VoidRef {}),
        AnyRef::Dictionary(d) => {
            return dictionary_ref_to_source(&d);
        }
        AnyRef::OwnDictionary(name) => {
            fdecl::Ref::Capability(fdecl::CapabilityRef { name: name.to_string() })
        }
    };
    (ref_, None)
}

/// Takes a `DictionaryRef` and returns the `fdecl::Ref` equivalent and the dictionary path.
fn dictionary_ref_to_source(d: &DictionaryRef) -> (fdecl::Ref, Option<String>) {
    #[allow(unused)]
    let root = match &d.root {
        RootDictionaryRef::Named(name) => {
            fdecl::Ref::Child(fdecl::ChildRef { name: name.clone().into(), collection: None })
        }
        RootDictionaryRef::Parent => fdecl::Ref::Parent(fdecl::ParentRef {}),
        RootDictionaryRef::Self_ => fdecl::Ref::Self_(fdecl::SelfRef {}),
    };
    (root, Some(d.path.to_string()))
}

fn configuration_to_value(
    name: &BorrowedName,
    capability: &ContextCapability,
    config_type: &Option<ContextSpanned<ConfigType>>,
    value: &Option<ContextSpanned<serde_json::Value>>,
) -> Result<fdecl::ConfigValue, Error> {
    let Some(config_type) = config_type.as_ref() else {
        return Err(Error::InvalidArgs(format!(
            "Configuration field '{}' must have 'type' set",
            name
        )));
    };
    let Some(value) = value.as_ref() else {
        return Err(Error::InvalidArgs(format!(
            "Configuration field '{}' must have 'value' set",
            name
        )));
    };

    let config_type = match config_type.value {
        ConfigType::Bool => cm_rust::ConfigValueType::Bool,
        ConfigType::Uint8 => cm_rust::ConfigValueType::Uint8,
        ConfigType::Uint16 => cm_rust::ConfigValueType::Uint16,
        ConfigType::Uint32 => cm_rust::ConfigValueType::Uint32,
        ConfigType::Uint64 => cm_rust::ConfigValueType::Uint64,
        ConfigType::Int8 => cm_rust::ConfigValueType::Int8,
        ConfigType::Int16 => cm_rust::ConfigValueType::Int16,
        ConfigType::Int32 => cm_rust::ConfigValueType::Int32,
        ConfigType::Int64 => cm_rust::ConfigValueType::Int64,
        ConfigType::String => {
            let Some(max_size) = capability.config_max_size.as_ref() else {
                return Err(Error::InvalidArgs(format!(
                    "Configuration field '{}' must have 'max_size' set",
                    name
                )));
            };
            let size_val: u32 = max_size.value.get();
            cm_rust::ConfigValueType::String { max_size: size_val }
        }
        ConfigType::Vector => {
            let Some(ref element) = capability.config_element_type else {
                return Err(Error::InvalidArgs(format!(
                    "Configuration field '{}' must have 'element_type' set",
                    name
                )));
            };
            let Some(max_count) = capability.config_max_count.as_ref() else {
                return Err(Error::InvalidArgs(format!(
                    "Configuration field '{}' must have 'max_count' set",
                    name
                )));
            };
            let max_count_val = max_count.value.get();
            let nested_type = match element.value {
                ConfigNestedValueType::Bool { .. } => cm_rust::ConfigNestedValueType::Bool,
                ConfigNestedValueType::Uint8 { .. } => cm_rust::ConfigNestedValueType::Uint8,
                ConfigNestedValueType::Uint16 { .. } => cm_rust::ConfigNestedValueType::Uint16,
                ConfigNestedValueType::Uint32 { .. } => cm_rust::ConfigNestedValueType::Uint32,
                ConfigNestedValueType::Uint64 { .. } => cm_rust::ConfigNestedValueType::Uint64,
                ConfigNestedValueType::Int8 { .. } => cm_rust::ConfigNestedValueType::Int8,
                ConfigNestedValueType::Int16 { .. } => cm_rust::ConfigNestedValueType::Int16,
                ConfigNestedValueType::Int32 { .. } => cm_rust::ConfigNestedValueType::Int32,
                ConfigNestedValueType::Int64 { .. } => cm_rust::ConfigNestedValueType::Int64,
                ConfigNestedValueType::String { max_size } => {
                    cm_rust::ConfigNestedValueType::String { max_size: max_size.get().into() }
                }
            };
            cm_rust::ConfigValueType::Vector { max_count: max_count_val.into(), nested_type }
        }
    };
    let value = config_value_file::field::config_value_from_json_value(&value.value, &config_type)
        .map_err(|e| Error::InvalidArgs(format!("Error parsing config '{}': {}", name, e)))?;
    Ok(value.native_into_fidl())
}

#[cfg(test)]
pub mod test_util {
    /// Construct a CML [DocumentContext] from the provided JSON literal expression or panic if error.
    macro_rules! must_parse_cml {
       ($($input:tt)+) => {
            {
                let json_str = serde_json::json!($($input)+).to_string();
                let dummy_path = std::sync::Arc::from(std::path::Path::new("macro_generated.cml"));

                crate::types::document::parse_and_hydrate(dummy_path, &json_str)
                    .expect("CML parsing and hydration failed")
            }
        };
    }
    pub(crate) use must_parse_cml;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::features::Feature;
    use crate::translate::test_util::must_parse_cml;
    use crate::types::common::synthetic_span;
    use crate::types::offer::create_offer;
    use crate::{
        CapabilityClause, Document, FromClause, OneOrMany, Path, Program, load_cml_with_context,
    };
    use assert_matches::assert_matches;
    use cm_fidl_validator::error::{AvailabilityList, DeclField, Error as CmFidlError, ErrorList};
    use cm_types::{self as cm, Name};
    use difference::Changeset;
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_data as fdata;
    use fidl_fuchsia_io as fio;
    use serde_json::{Map, Value, json};
    use std::collections::BTreeSet;
    use std::convert::Into;
    use std::str::FromStr;

    macro_rules! test_compile_context {
        (
            $(
                $(#[$m:meta])*
                $test_name:ident => {
                    $(features = $features:expr,)?
                    input = $input:expr,
                    output = $expected:expr,
                },
            )+
        ) => {
            $(
                $(#[$m])*
                #[test]
                fn $test_name() {
                    let fake_file = std::path::Path::new("test.cml");
                    let input_str = serde_json::to_string(&$input).expect("failed to serialize input json");
                    let document = crate::load_cml_with_context(&input_str, fake_file).expect("should work");

                    let options = CompileOptions::new()
                        .file(&fake_file)
                        .config_package_path("fake.cvf");

                    $(
                        let features = $features;
                        let options = options.features(&features);
                    )?

                    let actual_context = compile(&document, options).expect("compilation failed");

                    if actual_context != $expected {
                        let e = format!("{:#?}", $expected);
                        let a = format!("{:#?}", actual_context);
                        panic!("Test {} failed comparison:\n{}", stringify!($test_name), Changeset::new(&a, &e, "\n"));
                    }
                }
            )+
        };
    }

    fn default_component_decl() -> fdecl::Component {
        fdecl::Component::default()
    }

    test_compile_context! {
        test_compile_empty_dep => {
            input = json!({}),
            output = default_component_decl(),
        },

        test_compile_empty_includes => {
            input = json!({ "include": [] }),
            output = default_component_decl(),
        },

        test_compile_offer_to_all_and_diff_sources => {
            input = json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                    },
                ],
                "collections": [
                    {
                        "name": "coll",
                        "durability": "transient",
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.logger.LogSink",
                        "from": "parent",
                        "to": "all",
                    },
                    {
                        "protocol": "fuchsia.logger.LogSink",
                        "from": "framework",
                        "to": "#logger",
                        "as": "LogSink2",
                    },
                ],
            }),
            output = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Framework(fdecl::FrameworkRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".into(),
                            collection: None,
                        })),
                        target_name: Some("LogSink2".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll".into(),
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![fdecl::Child {
                    name: Some("logger".into()),
                    url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".into()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    ..Default::default()
                }]),
                collections: Some(vec![fdecl::Collection {
                    name: Some("coll".into()),
                    durability: Some(fdecl::Durability::Transient),
                    ..Default::default()
                }]),
                ..default_component_decl()
            },
        },

        test_compile_offer_to_all => {
            input = json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                    },
                    {
                        "name": "something",
                        "url": "fuchsia-pkg://fuchsia.com/something/stable#meta/something.cm",
                    },
                ],
                "collections": [
                    {
                        "name": "coll",
                        "durability": "transient",
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.logger.LogSink",
                        "from": "parent",
                        "to": "all",
                    },
                    {
                        "protocol": "fuchsia.inspect.InspectSink",
                        "from": "parent",
                        "to": "all",
                    },
                    {
                        "protocol": "fuchsia.logger.LegacyLog",
                        "from": "parent",
                        "to": "#logger",
                    },
                ],
            }),
            output = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LegacyLog".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.LegacyLog".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "something".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll".into(),
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.inspect.InspectSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.inspect.InspectSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.inspect.InspectSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "something".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.inspect.InspectSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.inspect.InspectSink".into()),
                        target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll".into(),
                        })),
                        target_name: Some("fuchsia.inspect.InspectSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".into()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".into()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("something".into()),
                        url: Some(
                            "fuchsia-pkg://fuchsia.com/something/stable#meta/something.cm".into(),
                        ),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                collections: Some(vec![fdecl::Collection {
                    name: Some("coll".into()),
                    durability: Some(fdecl::Durability::Transient),
                    ..Default::default()
                }]),
                ..default_component_decl()
            },
        },

        test_compile_offer_to_all_hides_individual_duplicate_routes => {
            input = json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                    },
                    {
                        "name": "something",
                        "url": "fuchsia-pkg://fuchsia.com/something/stable#meta/something.cm",
                    },
                    {
                        "name": "something-v2",
                        "url": "fuchsia-pkg://fuchsia.com/something/stable#meta/something-v2.cm",
                    },
                ],
                "collections": [
                    {
                        "name": "coll",
                        "durability": "transient",
                    },
                    {
                        "name": "coll2",
                        "durability": "transient",
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.logger.LogSink",
                        "from": "parent",
                        "to": "#logger",
                    },
                    {
                        "protocol": "fuchsia.logger.LogSink",
                        "from": "parent",
                        "to": "all",
                    },
                    {
                        "protocol": "fuchsia.logger.LogSink",
                        "from": "parent",
                        "to": [ "#something", "#something-v2", "#coll2"],
                    },
                    {
                        "protocol": "fuchsia.logger.LogSink",
                        "from": "parent",
                        "to": "#coll",
                    },
                ],
            }),
            output = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "something".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "something-v2".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll2".into(),
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                            name: "coll".into(),
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".into()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".into()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("something".into()),
                        url: Some(
                            "fuchsia-pkg://fuchsia.com/something/stable#meta/something.cm".into(),
                        ),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("something-v2".into()),
                        url: Some(
                            "fuchsia-pkg://fuchsia.com/something/stable#meta/something-v2.cm".into(),
                        ),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                collections: Some(vec![fdecl::Collection {
                    name: Some("coll".into()),
                    durability: Some(fdecl::Durability::Transient),
                    ..Default::default()
                }, fdecl::Collection {
                    name: Some("coll2".into()),
                    durability: Some(fdecl::Durability::Transient),
                    ..Default::default()
                }]),
                ..default_component_decl()
            },
        },

        test_compile_offer_to_all_from_child => {
            input = json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                    },
                    {
                        "name": "something",
                        "url": "fuchsia-pkg://fuchsia.com/something/stable#meta/something.cm",
                    },
                    {
                        "name": "something-v2",
                        "url": "fuchsia-pkg://fuchsia.com/something/stable#meta/something-v2.cm",
                    },
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.logger.LogSink",
                        "from": "#logger",
                        "to": "all",
                    },
                ],
            }),
            output = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".into(),
                            collection: None,
                        })),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "something".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "logger".into(),
                            collection: None,
                        })),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "something-v2".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".into()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".into()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("something".into()),
                        url: Some(
                            "fuchsia-pkg://fuchsia.com/something/stable#meta/something.cm".into(),
                        ),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("something-v2".into()),
                        url: Some(
                            "fuchsia-pkg://fuchsia.com/something/stable#meta/something-v2.cm".into(),
                        ),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                ..default_component_decl()
            },
        },

        test_compile_offer_multiple_protocols_to_single_array_syntax_and_all => {
            input = json!({
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
            }),
            output = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "something".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.inspect.InspectSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "something".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.inspect.InspectSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("something".into()),
                        url: Some(
                            "fuchsia-pkg://fuchsia.com/something/stable#meta/something.cm".into(),
                        ),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                ..default_component_decl()
            },
        },

        test_compile_offer_to_all_array_and_single => {
            input = json!({
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
            }),
            output = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.logger.LogSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "something".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.logger.LogSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        source_name: Some("fuchsia.inspect.InspectSink".into()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "something".into(),
                            collection: None,
                        })),
                        target_name: Some("fuchsia.inspect.InspectSink".into()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("something".into()),
                        url: Some(
                            "fuchsia-pkg://fuchsia.com/something/stable#meta/something.cm".into(),
                        ),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                ..default_component_decl()
            },
        },

        test_compile_program => {
            input = json!({
                "program": {
                    "runner": "elf",
                    "binary": "bin/app",
                },
            }),
            output = fdecl::Component {
                program: Some(fdecl::Program {
                    runner: Some("elf".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: Some(vec![fdata::DictionaryEntry {
                            key: "binary".to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str("bin/app".to_string()))),
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..default_component_decl()
            },
        },

        test_compile_program_with_use_runner => {
            input = json!({
                "program": {
                    "binary": "bin/app",
                },
                "use": [
                    { "runner": "elf", "from": "parent", },
                ],
            }),
            output = fdecl::Component {
                program: Some(fdecl::Program {
                    runner: None,
                    info: Some(fdata::Dictionary {
                        entries: Some(vec![fdata::DictionaryEntry {
                            key: "binary".to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str("bin/app".to_string()))),
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                uses: Some(vec![
                    fdecl::Use::Runner (
                        fdecl::UseRunner {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("elf".to_string()),
                            ..Default::default()
                        }
                    ),
                ]),
                ..default_component_decl()
            },
        },

        test_compile_program_with_nested_objects => {
            input = json!({
                "program": {
                    "runner": "elf",
                    "binary": "bin/app",
                    "one": {
                        "two": {
                            "three.four": {
                                "five": "six"
                            }
                        },
                    }
                },
            }),
            output = fdecl::Component {
                program: Some(fdecl::Program {
                    runner: Some("elf".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: Some(vec![
                            fdata::DictionaryEntry {
                                key: "binary".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::Str("bin/app".to_string()))),
                            },
                            fdata::DictionaryEntry {
                                key: "one.two.three.four.five".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::Str("six".to_string()))),
                            },
                        ]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..default_component_decl()
            },
        },

        test_compile_program_with_array_of_objects => {
            input = json!({
                "program": {
                    "runner": "elf",
                    "binary": "bin/app",
                    "networks": [
                        {
                            "endpoints": [
                                {
                                    "name": "device",
                                    "mac": "aa:bb:cc:dd:ee:ff"
                                },
                                {
                                    "name": "emu",
                                    "mac": "ff:ee:dd:cc:bb:aa"
                                },
                            ],
                            "name": "external_network"
                        }
                    ],
                },
            }),
            output = fdecl::Component {
                program: Some(fdecl::Program {
                    runner: Some("elf".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: Some(vec![
                            fdata::DictionaryEntry {
                                key: "binary".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::Str("bin/app".to_string()))),
                            },
                            fdata::DictionaryEntry {
                                key: "networks".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::ObjVec(vec![
                                    fdata::Dictionary {
                                        entries: Some(vec![
                                            fdata::DictionaryEntry {
                                                key: "endpoints".to_string(),
                                                value: Some(Box::new(fdata::DictionaryValue::ObjVec(vec![
                                                    fdata::Dictionary {
                                                        entries: Some(vec![
                                                            fdata::DictionaryEntry {
                                                                key: "mac".to_string(),
                                                                value: Some(Box::new(fdata::DictionaryValue::Str("aa:bb:cc:dd:ee:ff".to_string()))),
                                                            },
                                                            fdata::DictionaryEntry {
                                                                key: "name".to_string(),
                                                                value: Some(Box::new(fdata::DictionaryValue::Str("device".to_string()))),
                                                            }
                                                        ]),
                                                        ..Default::default()
                                                    },
                                                    fdata::Dictionary {
                                                        entries: Some(vec![
                                                            fdata::DictionaryEntry {
                                                                key: "mac".to_string(),
                                                                value: Some(Box::new(fdata::DictionaryValue::Str("ff:ee:dd:cc:bb:aa".to_string()))),
                                                            },
                                                            fdata::DictionaryEntry {
                                                                key: "name".to_string(),
                                                                value: Some(Box::new(fdata::DictionaryValue::Str("emu".to_string()))),
                                                            }
                                                        ]),
                                                        ..Default::default()
                                                    },
                                                ])))
                                            },
                                            fdata::DictionaryEntry {
                                                key: "name".to_string(),
                                                value: Some(Box::new(fdata::DictionaryValue::Str("external_network".to_string()))),
                                            },
                                        ]),
                                        ..Default::default()
                                    }
                                ]))),
                            },
                        ]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..default_component_decl()
            },
        },

        test_compile_use => {
            features = FeatureSet::from(vec![Feature::UseDictionaries]),
            input = json!({
                "use": [
                    {
                        "protocol": "LegacyCoolFonts",
                        "path": "/svc/fuchsia.fonts.LegacyProvider",
                        "availability": "optional",
                    },
                    {
                        "protocol": "LegacyCoolFonts",
                        "numbered_handle": 0xab,
                    },
                    { "protocol": "fuchsia.sys2.LegacyRealm", "from": "framework" },
                    { "protocol": "fuchsia.sys2.StorageAdmin", "from": "#data-storage" },
                    { "protocol": "fuchsia.sys2.DebugProto", "from": "debug" },
                    { "protocol": "fuchsia.sys2.DictionaryProto", "from": "#logger/in/dict" },
                    { "protocol": "fuchsia.sys2.Echo", "from": "self", "availability": "transitional" },
                    { "service": "fuchsia.sys2.EchoService", "from": "parent/dict", },
                    { "directory": "assets", "rights" : ["read_bytes"], "path": "/data/assets" },
                    {
                        "directory": "config",
                        "path": "/data/config",
                        "from": "parent",
                        "rights": ["read_bytes"],
                        "subdir": "fonts",
                    },
                    { "storage": "hippos", "path": "/hippos" },
                    { "storage": "cache", "path": "/tmp" },
                    {
                        "event_stream": "bar_stream",
                    },
                    {
                        "event_stream": ["foobar", "stream"],
                        "scope": ["#logger", "#modular"],
                        "path": "/event_stream/another",
                    },
                    { "runner": "usain", "from": "parent", },
                    {
                        "dictionary": "toolbox",
                        "path": "/svc",
                    },
                ],
                "capabilities": [
                    { "protocol": "fuchsia.sys2.Echo" },
                    {
                        "config": "fuchsia.config.Config",
                        "type": "bool",
                        "value": true,
                    },
                    {
                        "storage": "data-storage",
                        "from": "parent",
                        "backing_dir": "minfs",
                        "storage_id": "static_instance_id_or_moniker",
                    }
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                        "environment": "#env_one"
                    }
                ],
                "collections": [
                    {
                        "name": "modular",
                        "durability": "transient",
                    },
                ],
                "environments": [
                    {
                        "name": "env_one",
                        "extends": "realm",
                    }
                ]
            }),
            output = fdecl::Component {
                uses: Some(vec![
                    fdecl::Use::Protocol (
                        fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("LegacyCoolFonts".to_string()),
                            target_path: Some("/svc/fuchsia.fonts.LegacyProvider".to_string()),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Protocol (
                        fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("LegacyCoolFonts".to_string()),
                            numbered_handle: Some(0xab),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Protocol (
                        fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Framework(fdecl::FrameworkRef {})),
                            source_name: Some("fuchsia.sys2.LegacyRealm".to_string()),
                            target_path: Some("/svc/fuchsia.sys2.LegacyRealm".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Protocol (
                        fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Capability(fdecl::CapabilityRef { name: "data-storage".to_string() })),
                            source_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            target_path: Some("/svc/fuchsia.sys2.StorageAdmin".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Protocol (
                        fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Debug(fdecl::DebugRef {})),
                            source_name: Some("fuchsia.sys2.DebugProto".to_string()),
                            target_path: Some("/svc/fuchsia.sys2.DebugProto".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Protocol (
                        fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".into(),
                                collection: None,
                            })),
                            source_dictionary: Some("in/dict".into()),
                            source_name: Some("fuchsia.sys2.DictionaryProto".to_string()),
                            target_path: Some("/svc/fuchsia.sys2.DictionaryProto".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Protocol (
                        fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("fuchsia.sys2.Echo".to_string()),
                            target_path: Some("/svc/fuchsia.sys2.Echo".to_string()),
                            availability: Some(fdecl::Availability::Transitional),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Service (
                        fdecl::UseService {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_dictionary: Some("dict".into()),
                            source_name: Some("fuchsia.sys2.EchoService".to_string()),
                            target_path: Some("/svc/fuchsia.sys2.EchoService".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Directory (
                        fdecl::UseDirectory {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("assets".to_string()),
                            target_path: Some("/data/assets".to_string()),
                            rights: Some(fio::Operations::READ_BYTES),
                            subdir: None,
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Directory (
                        fdecl::UseDirectory {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("config".to_string()),
                            target_path: Some("/data/config".to_string()),
                            rights: Some(fio::Operations::READ_BYTES),
                            subdir: Some("fonts".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Storage (
                        fdecl::UseStorage {
                            source_name: Some("hippos".to_string()),
                            target_path: Some("/hippos".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Storage (
                        fdecl::UseStorage {
                            source_name: Some("cache".to_string()),
                            target_path: Some("/tmp".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::EventStream(fdecl::UseEventStream {
                        source_name: Some("bar_stream".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target_path: Some("/svc/fuchsia.component.EventStream".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Use::EventStream(fdecl::UseEventStream {
                        source_name: Some("foobar".to_string()),
                        scope: Some(vec![fdecl::Ref::Child(fdecl::ChildRef{name:"logger".to_string(), collection: None}), fdecl::Ref::Collection(fdecl::CollectionRef{name:"modular".to_string()})]),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target_path: Some("/event_stream/another".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Use::EventStream(fdecl::UseEventStream {
                        source_name: Some("stream".to_string()),
                        scope: Some(vec![fdecl::Ref::Child(fdecl::ChildRef{name:"logger".to_string(), collection: None}), fdecl::Ref::Collection(fdecl::CollectionRef{name:"modular".to_string()})]),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target_path: Some("/event_stream/another".to_string()),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                    fdecl::Use::Runner(fdecl::UseRunner {
                        source_name: Some("usain".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        ..Default::default()
                    }),
                    fdecl::Use::Dictionary(fdecl::UseDictionary {
                        source_name: Some("toolbox".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef{})),
                        target_path: Some("/svc".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    }),
                ]),
                collections: Some(vec![
                    fdecl::Collection{
                        name:Some("modular".to_string()),
                        durability:Some(fdecl::Durability::Transient),
                        ..Default::default()
                    },
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Protocol(fdecl::Protocol {
                        name: Some("fuchsia.sys2.Echo".to_string()),
                        source_path: Some("/svc/fuchsia.sys2.Echo".to_string()),
                        ..Default::default()
                    }),
                    fdecl::Capability::Config(fdecl::Configuration {
                        name: Some("fuchsia.config.Config".to_string()),
                        value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Bool(true))),
                        ..Default::default()
                    }),
                    fdecl::Capability::Storage(fdecl::Storage {
                        name: Some("data-storage".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        backing_dir: Some("minfs".to_string()),
                        subdir: None,
                        storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                        ..Default::default()
                    }),
                ]),
                children: Some(vec![
                    fdecl::Child{
                        name:Some("logger".to_string()),
                        url:Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup:Some(fdecl::StartupMode::Lazy),
                        environment: Some("env_one".to_string()),
                        ..Default::default()
                    }
                ]),
                environments: Some(vec![
                    fdecl::Environment {
                        name: Some("env_one".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::Realm),
                        ..Default::default()
                    },
                ]),
                config: None,
                ..default_component_decl()
            },
        },

        test_compile_expose => {
            input = json!({
                "expose": [
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.LegacyLog",
                        "to": "parent"
                    },
                    {
                        "protocol": [ "A", "B" ],
                        "from": "self",
                        "to": "parent"
                    },
                    {
                        "protocol": "C",
                        "from": "#data-storage",
                    },
                    {
                        "protocol": "D",
                        "from": "#logger/in/dict",
                        "as": "E",
                    },
                    {
                        "service": "F",
                        "from": "#logger/in/dict",
                    },
                    {
                        "service": "svc",
                        "from": [ "#logger", "#coll", "self" ],
                    },
                    {
                        "directory": "blob",
                        "from": "self",
                        "to": "framework",
                        "rights": ["r*"],
                    },
                    {
                        "directory": [ "blob2", "blob3" ],
                        "from": "#logger",
                        "to": "parent",
                    },
                    { "directory": "hub", "from": "framework" },
                    { "runner": "web", "from": "#logger", "to": "parent", "as": "web-rename" },
                    { "runner": [ "runner_a", "runner_b" ], "from": "#logger" },
                    { "resolver": "my_resolver", "from": "#logger", "to": "parent", "as": "pkg_resolver" },
                    { "resolver": [ "resolver_a", "resolver_b" ], "from": "#logger" },
                    { "dictionary": [ "dictionary_a", "dictionary_b" ], "from": "#logger" },
                ],
                "capabilities": [
                    { "protocol": "A" },
                    { "protocol": "B" },
                    { "service": "svc" },
                    {
                        "directory": "blob",
                        "path": "/volumes/blobfs/blob",
                        "rights": ["r*"],
                    },
                    {
                        "runner": "web",
                        "path": "/svc/fuchsia.component.ComponentRunner",
                    },
                    {
                        "storage": "data-storage",
                        "from": "parent",
                        "backing_dir": "minfs",
                        "storage_id": "static_instance_id_or_moniker",
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    },
                ],
                "collections": [
                    {
                        "name": "coll",
                        "durability": "transient",
                    },
                ],
            }),
            output = fdecl::Component {
                exposes: Some(vec![
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("A".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("A".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("B".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("B".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                                name: "data-storage".to_string(),
                            })),
                            source_name: Some("C".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("C".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_dictionary: Some("in/dict".into()),
                            source_name: Some("D".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("E".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".into(),
                                collection: None,
                            })),
                            source_name: Some("F".into()),
                            source_dictionary: Some("in/dict".into()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("F".into()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".into(),
                                collection: None,
                            })),
                            source_name: Some("svc".into()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("svc".into()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "coll".into(),
                            })),
                            source_name: Some("svc".into()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("svc".into()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("svc".into()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("svc".into()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Directory (
                        fdecl::ExposeDirectory {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("blob".to_string()),
                            target: Some(fdecl::Ref::Framework(fdecl::FrameworkRef {})),
                            target_name: Some("blob".to_string()),
                            rights: Some(
                                fio::Operations::CONNECT | fio::Operations::ENUMERATE |
                                fio::Operations::TRAVERSE | fio::Operations::READ_BYTES |
                                fio::Operations::GET_ATTRIBUTES
                            ),
                            subdir: None,
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Directory (
                        fdecl::ExposeDirectory {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("blob2".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("blob2".to_string()),
                            rights: None,
                            subdir: None,
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Directory (
                        fdecl::ExposeDirectory {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("blob3".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("blob3".to_string()),
                            rights: None,
                            subdir: None,
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Directory (
                        fdecl::ExposeDirectory {
                            source: Some(fdecl::Ref::Framework(fdecl::FrameworkRef {})),
                            source_name: Some("hub".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("hub".to_string()),
                            rights: None,
                            subdir: None,
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Runner (
                        fdecl::ExposeRunner {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("web".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("web-rename".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Runner (
                        fdecl::ExposeRunner {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("runner_a".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("runner_a".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Runner (
                        fdecl::ExposeRunner {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("runner_b".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("runner_b".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Resolver (
                        fdecl::ExposeResolver {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("my_resolver".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("pkg_resolver".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Resolver (
                        fdecl::ExposeResolver {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("resolver_a".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("resolver_a".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Resolver (
                        fdecl::ExposeResolver {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("resolver_b".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("resolver_b".to_string()),
                            ..Default::default()
                        }
                    ),
                   fdecl::Expose::Dictionary (
                        fdecl::ExposeDictionary {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("dictionary_a".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("dictionary_a".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Dictionary (
                        fdecl::ExposeDictionary {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("dictionary_b".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("dictionary_b".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                ]),
                offers: None,
                capabilities: Some(vec![
                    fdecl::Capability::Protocol (
                        fdecl::Protocol {
                            name: Some("A".to_string()),
                            source_path: Some("/svc/A".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Protocol (
                        fdecl::Protocol {
                            name: Some("B".to_string()),
                            source_path: Some("/svc/B".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Service (
                        fdecl::Service {
                            name: Some("svc".to_string()),
                            source_path: Some("/svc/svc".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Directory (
                        fdecl::Directory {
                            name: Some("blob".to_string()),
                            source_path: Some("/volumes/blobfs/blob".to_string()),
                            rights: Some(fio::Operations::CONNECT | fio::Operations::ENUMERATE |
                                fio::Operations::TRAVERSE | fio::Operations::READ_BYTES |
                                fio::Operations::GET_ATTRIBUTES
                            ),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Runner (
                        fdecl::Runner {
                            name: Some("web".to_string()),
                            source_path: Some("/svc/fuchsia.component.ComponentRunner".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Storage(fdecl::Storage {
                        name: Some("data-storage".to_string()),
                        source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                        backing_dir: Some("minfs".to_string()),
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
                        ..Default::default()
                    }
                ]),
                collections: Some(vec![
                    fdecl::Collection {
                        name: Some("coll".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        ..Default::default()
                    }
                ]),
                ..default_component_decl()
            },
        },

        test_compile_expose_other_availability => {
            input = json!({
                "expose": [
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.LegacyLog_default",
                        "to": "parent"
                    },
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.LegacyLog_required",
                        "to": "parent",
                        "availability": "required"
                    },
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.LegacyLog_optional",
                        "to": "parent",
                        "availability": "optional"
                    },
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.LegacyLog_same_as_target",
                        "to": "parent",
                        "availability": "same_as_target"
                    },
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.LegacyLog_transitional",
                        "to": "parent",
                        "availability": "transitional"
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    },
                ],
            }),
            output = fdecl::Component {
                exposes: Some(vec![
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("fuchsia.logger.LegacyLog_default".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("fuchsia.logger.LegacyLog_required".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("fuchsia.logger.LegacyLog_optional".to_string()),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("fuchsia.logger.LegacyLog_same_as_target".to_string()),
                            availability: Some(fdecl::Availability::SameAsTarget),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("fuchsia.logger.LegacyLog_transitional".to_string()),
                            availability: Some(fdecl::Availability::Transitional),
                            ..Default::default()
                        }
                    ),
                ]),
                offers: None,
                capabilities: None,
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    }
                ]),
                ..default_component_decl()
            },
        },

        test_compile_expose_source_availability_unknown => {
            input = json!({
                "expose": [
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#non-existent",
                        "as": "fuchsia.logger.LegacyLog_non_existent",
                        "availability": "optional",
                        "source_availability": "unknown"
                    },
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#non-existent/dict",
                        "as": "fuchsia.logger.LegacyLog_non_existent2",
                        "availability": "optional",
                        "source_availability": "unknown"
                    },
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.LegacyLog_child_exist",
                        "availability": "optional",
                        "source_availability": "unknown"
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    },
                ],
            }),
            output = fdecl::Component {
                exposes: Some(vec![
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::VoidType(fdecl::VoidRef { })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("fuchsia.logger.LegacyLog_non_existent".to_string()),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::VoidType(fdecl::VoidRef { })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("fuchsia.logger.LegacyLog_non_existent2".to_string()),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Protocol (
                        fdecl::ExposeProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("fuchsia.logger.LegacyLog_child_exist".to_string()),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    }
                ]),
                ..default_component_decl()
            },
        },

        test_compile_offer_target_availability_unknown => {
            input = json!({
                "offer": [
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#logger",
                        "to": "#non-existent",
                        "target_availability": "unknown",
                    },
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#logger",
                        "to": "self/non-existent-dict",
                        "target_availability": "unknown",
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    },
                ],
            }),
            output = fdecl::Component {
                offers: Some(vec![]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                ..default_component_decl()
            },
        },

        test_compile_offer_source_availability_unknown => {
            input = json!({
                "offer": [
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#non-existent",
                        "as": "fuchsia.logger.LegacyLog_non_existent",
                        "to": "#target",
                        "availability": "optional",
                        "source_availability": "unknown"
                    },
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#non-existent/dict",
                        "as": "fuchsia.logger.LegacyLog_non_existent2",
                        "to": "#target",
                        "availability": "optional",
                        "source_availability": "unknown"
                    },
                    {
                        "protocol": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.LegacyLog_child_exist",
                        "to": "#target",
                        "availability": "optional",
                        "source_availability": "unknown"
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    },
                    {
                        "name": "target",
                        "url": "#meta/target.cm"
                    },
                ],
            }),
            output = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::VoidType(fdecl::VoidRef { })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "target".to_string(),
                                collection: None,
                            })),
                            target_name: Some("fuchsia.logger.LegacyLog_non_existent".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::VoidType(fdecl::VoidRef { })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "target".to_string(),
                                collection: None,
                            })),
                            target_name: Some("fuchsia.logger.LegacyLog_non_existent2".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "target".to_string(),
                                collection: None,
                            })),
                            target_name: Some("fuchsia.logger.LegacyLog_child_exist".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("target".to_string()),
                        url: Some("#meta/target.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                ..default_component_decl()
            },
        },

        test_compile_offer => {
            input = json!({
                "offer": [
                    {
                        "protocol": "fuchsia.logger.LegacyLog",
                        "from": "#logger",
                        "to": "#netstack", // Verifies compilation of singleton "to:".
                        "dependency": "weak"
                    },
                    {
                        "protocol": "fuchsia.logger.LegacyLog",
                        "from": "#logger",
                        "to": [ "#modular" ], // Verifies compilation of "to:" as array of one element.
                        "as": "fuchsia.logger.LegacySysLog",
                        "dependency": "strong"
                    },
                    {
                        "protocol": [
                            "fuchsia.setui.SetUiService",
                            "fuchsia.test.service.Name"
                        ],
                        "from": "parent",
                        "to": [ "#modular" ],
                        "availability": "optional"
                    },
                    {
                        "protocol": "fuchsia.sys2.StorageAdmin",
                        "from": "#data",
                        "to": [ "#modular" ],
                    },
                    {
                        "protocol": "fuchsia.sys2.FromDict",
                        "from": "parent/in/dict",
                        "to": [ "#modular" ],
                    },
                    {
                        "service": "svc",
                        "from": [ "parent", "self", "#logger", "#modular" ],
                        "to": "#netstack",
                    },
                    {
                        "service": "fuchsia.sys2.FromDictService",
                        "from": [ "parent/in/dict"],
                        "to": "#modular",
                        "dependency": "weak",
                    },
                    {
                        "directory": "assets",
                        "from": "parent",
                        "to": [ "#netstack" ],
                        "dependency": "weak",
                        "availability": "same_as_target"
                    },
                    {
                        "directory": [ "assets2", "assets3" ],
                        "from": "parent",
                        "to": [ "#modular", "#netstack" ],
                    },
                    {
                        "directory": "data",
                        "from": "parent",
                        "to": [ "#modular" ],
                        "as": "assets",
                        "subdir": "index/file",
                        "dependency": "strong"
                    },
                    {
                        "directory": "hub",
                        "from": "framework",
                        "to": [ "#modular" ],
                        "as": "hub",
                    },
                    {
                        "storage": "data",
                        "from": "self",
                        "to": [
                            "#netstack",
                            "#modular"
                        ],
                    },
                    {
                        "storage": [ "storage_a", "storage_b" ],
                        "from": "parent",
                        "to": "#netstack",
                    },
                    {
                        "runner": "elf",
                        "from": "parent",
                        "to": [ "#modular" ],
                        "as": "elf-renamed",
                    },
                    {
                        "runner": [ "runner_a", "runner_b" ],
                        "from": "parent",
                        "to": "#netstack",
                    },
                    {
                        "resolver": "my_resolver",
                        "from": "parent",
                        "to": [ "#modular" ],
                        "as": "pkg_resolver",
                    },
                    {
                        "resolver": [ "resolver_a", "resolver_b" ],
                        "from": "parent",
                        "to": "#netstack",
                    },
                    {
                        "dictionary": [ "dictionary_a", "dictionary_b" ],
                        "from": "parent",
                        "to": "#netstack",
                    },
                    {
                        "event_stream": [
                            "running",
                            "started",
                        ],
                        "from": "parent",
                        "to": "#netstack",
                    },
                    {
                        "event_stream": "stopped",
                        "from": "parent",
                        "to": "#netstack",
                        "as": "some_other_event",
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm"
                    },
                    {
                        "name": "netstack",
                        "url": "fuchsia-pkg://fuchsia.com/netstack/stable#meta/netstack.cm"
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
                        "service": "svc",
                    },
                    {
                        "storage": "data",
                        "backing_dir": "minfs",
                        "from": "#logger",
                        "storage_id": "static_instance_id_or_moniker",
                    },
                ],
            }),
            output = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.LegacyLog".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Weak),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.LegacyLog".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("fuchsia.logger.LegacySysLog".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("fuchsia.setui.SetUiService".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("fuchsia.setui.SetUiService".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("fuchsia.test.service.Name".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("fuchsia.test.service.Name".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                                name: "data".to_string(),
                            })),
                            source_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("fuchsia.sys2.StorageAdmin".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_dictionary: Some("in/dict".into()),
                            source_name: Some("fuchsia.sys2.FromDict".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("fuchsia.sys2.FromDict".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("svc".into()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".into(),
                                collection: None,
                            })),
                            target_name: Some("svc".into()),
                            availability: Some(fdecl::Availability::Required),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("svc".into()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".into(),
                                collection: None,
                            })),
                            target_name: Some("svc".into()),
                            availability: Some(fdecl::Availability::Required),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".into(),
                                collection: None,
                            })),
                            source_name: Some("svc".into()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".into(),
                                collection: None,
                            })),
                            target_name: Some("svc".into()),
                            availability: Some(fdecl::Availability::Required),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".into(),
                            })),
                            source_name: Some("svc".into()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".into(),
                                collection: None,
                            })),
                            target_name: Some("svc".into()),
                            availability: Some(fdecl::Availability::Required),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("fuchsia.sys2.FromDictService".into()),
                            source_dictionary: Some("in/dict".into()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".into(),
                            })),
                            target_name: Some("fuchsia.sys2.FromDictService".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            dependency_type: Some(fdecl::DependencyType::Weak),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Directory (
                        fdecl::OfferDirectory {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("assets".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("assets".to_string()),
                            rights: None,
                            subdir: None,
                            dependency_type: Some(fdecl::DependencyType::Weak),
                            availability: Some(fdecl::Availability::SameAsTarget),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Directory (
                        fdecl::OfferDirectory {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("assets2".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("assets2".to_string()),
                            rights: None,
                            subdir: None,
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Directory (
                        fdecl::OfferDirectory {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("assets3".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("assets3".to_string()),
                            rights: None,
                            subdir: None,
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Directory (
                        fdecl::OfferDirectory {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("assets2".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("assets2".to_string()),
                            rights: None,
                            subdir: None,
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Directory (
                        fdecl::OfferDirectory {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("assets3".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("assets3".to_string()),
                            rights: None,
                            subdir: None,
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Directory (
                        fdecl::OfferDirectory {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("data".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("assets".to_string()),
                            rights: None,
                            subdir: Some("index/file".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Directory (
                        fdecl::OfferDirectory {
                            source: Some(fdecl::Ref::Framework(fdecl::FrameworkRef {})),
                            source_name: Some("hub".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("hub".to_string()),
                            rights: None,
                            subdir: None,
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Storage (
                        fdecl::OfferStorage {
                            source_name: Some("data".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("data".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Storage (
                        fdecl::OfferStorage {
                            source_name: Some("data".to_string()),
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("data".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Storage (
                        fdecl::OfferStorage {
                            source_name: Some("storage_a".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("storage_a".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Storage (
                        fdecl::OfferStorage {
                            source_name: Some("storage_b".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("storage_b".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Runner (
                        fdecl::OfferRunner {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("elf".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("elf-renamed".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Runner (
                        fdecl::OfferRunner {
                            source_name: Some("runner_a".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("runner_a".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Runner (
                        fdecl::OfferRunner {
                            source_name: Some("runner_b".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("runner_b".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Resolver (
                        fdecl::OfferResolver {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("my_resolver".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("pkg_resolver".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Resolver (
                        fdecl::OfferResolver {
                            source_name: Some("resolver_a".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("resolver_a".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Resolver (
                        fdecl::OfferResolver {
                            source_name: Some("resolver_b".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("resolver_b".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Dictionary (
                        fdecl::OfferDictionary {
                            source_name: Some("dictionary_a".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("dictionary_a".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Dictionary (
                        fdecl::OfferDictionary {
                            source_name: Some("dictionary_b".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("dictionary_b".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::EventStream (
                        fdecl::OfferEventStream {
                            source_name: Some("running".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("running".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::EventStream (
                        fdecl::OfferEventStream {
                            source_name: Some("started".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("started".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::EventStream (
                        fdecl::OfferEventStream {
                            source_name: Some("stopped".to_string()),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("some_other_event".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Service (
                        fdecl::Service {
                            name: Some("svc".into()),
                            source_path: Some("/svc/svc".into()),
                            ..Default::default()
                        },
                    ),
                    fdecl::Capability::Storage (
                        fdecl::Storage {
                            name: Some("data".to_string()),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            backing_dir: Some("minfs".to_string()),
                            subdir: None,
                            storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                            ..Default::default()
                        }
                    )
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("netstack".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/netstack/stable#meta/netstack.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                ]),
                collections: Some(vec![
                    fdecl::Collection {
                        name: Some("modular".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        environment: None,
                        allowed_offers: None,
                        ..Default::default()
                    }
                ]),
                ..default_component_decl()
            },
        },

        test_compile_offer_route_to_dictionary => {
            input = json!({
                "offer": [
                    {
                        "protocol": "A",
                        "from": "parent/dict/1",
                        "to": "self/dict",
                    },
                    {
                        "runner": "B",
                        "from": "#child",
                        "to": "self/dict",
                    },
                    {
                        "config": "B",
                        "from": "parent/dict/2",
                        "to": "self/dict",
                        "as": "C",
                    },
                ],
                "children": [
                    {
                        "name": "child",
                        "url": "fuchsia-pkg://child"
                    },
                ],
                "capabilities": [
                    {
                        "dictionary": "dict",
                    },
                ],
            }),
            output = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_dictionary: Some("dict/1".into()),
                            source_name: Some("A".into()),
                            target: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                                name: "dict".to_string(),
                            })),
                            target_name: Some("A".into()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Runner (
                        fdecl::OfferRunner {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "child".into(),
                                collection: None,
                            })),
                            source_name: Some("B".into()),
                            target: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                                name: "dict".to_string(),
                            })),
                            target_name: Some("B".into()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Config (
                        fdecl::OfferConfiguration {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_dictionary: Some("dict/2".into()),
                            source_name: Some("B".into()),
                            target: Some(fdecl::Ref::Capability(fdecl::CapabilityRef {
                                name: "dict".to_string(),
                            })),
                            target_name: Some("C".into()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Dictionary (
                        fdecl::Dictionary {
                            name: Some("dict".into()),
                            ..Default::default()
                        }
                    )
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("child".to_string()),
                        url: Some("fuchsia-pkg://child".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ]),
                ..default_component_decl()
            },
        },


        test_compile_children => {
            input = json!({
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                    },
                    {
                        "name": "gmail",
                        "url": "https://www.google.com/gmail",
                        "startup": "eager",
                    },
                    {
                        "name": "echo",
                        "url": "fuchsia-pkg://fuchsia.com/echo/stable#meta/echo.cm",
                        "startup": "lazy",
                        "on_terminate": "reboot",
                        "environment": "#myenv",
                    },
                ],
                "environments": [
                    {
                        "name": "myenv",
                        "extends": "realm",
                    },
                ],
            }),
            output = fdecl::Component {
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("gmail".to_string()),
                        url: Some("https://www.google.com/gmail".to_string()),
                        startup: Some(fdecl::StartupMode::Eager),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("echo".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/echo/stable#meta/echo.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: Some("myenv".to_string()),
                        on_terminate: Some(fdecl::OnTerminate::Reboot),
                        ..Default::default()
                    }
                ]),
                environments: Some(vec![
                    fdecl::Environment {
                        name: Some("myenv".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::Realm),
                        runners: None,
                        resolvers: None,
                        stop_timeout_ms: None,
                        ..Default::default()
                    }
                ]),
                ..default_component_decl()
            },
        },

        test_compile_collections => {
            input = json!({
                "collections": [
                    {
                        "name": "modular",
                        "durability": "single_run",
                    },
                    {
                        "name": "tests",
                        "durability": "transient",
                        "environment": "#myenv",
                    },
                ],
                "environments": [
                    {
                        "name": "myenv",
                        "extends": "realm",
                    }
                ],
            }),
            output = fdecl::Component {
                collections: Some(vec![
                    fdecl::Collection {
                        name: Some("modular".to_string()),
                        durability: Some(fdecl::Durability::SingleRun),
                        environment: None,
                        allowed_offers: None,
                        ..Default::default()
                    },
                    fdecl::Collection {
                        name: Some("tests".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        environment: Some("myenv".to_string()),
                        allowed_offers: None,
                        ..Default::default()
                    }
                ]),
                environments: Some(vec![
                    fdecl::Environment {
                        name: Some("myenv".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::Realm),
                        runners: None,
                        resolvers: None,
                        stop_timeout_ms: None,
                        ..Default::default()
                    }
                ]),
                ..default_component_decl()
            },
        },

        test_compile_capabilities => {
            features = FeatureSet::from(vec![Feature::DynamicDictionaries]),
            input = json!({
                "capabilities": [
                    {
                        "protocol": "myprotocol",
                        "path": "/protocol",
                    },
                    {
                        "protocol": "myprotocol2",
                    },
                    {
                        "protocol": [ "myprotocol3", "myprotocol4" ],
                    },
                    {
                        "directory": "mydirectory",
                        "path": "/directory",
                        "rights": [ "connect" ],
                    },
                    {
                        "storage": "mystorage",
                        "backing_dir": "storage",
                        "from": "#minfs",
                        "storage_id": "static_instance_id_or_moniker",
                    },
                    {
                        "storage": "mystorage2",
                        "backing_dir": "storage2",
                        "from": "#minfs",
                        "storage_id": "static_instance_id",
                    },
                    {
                        "runner": "myrunner",
                        "path": "/runner",
                    },
                    {
                        "resolver": "myresolver",
                        "path": "/resolver"
                    },
                    {
                        "dictionary": "dict1",
                    },
                    {
                        "dictionary": "dict2",
                        "path": "/in/a",
                    },
                ],
                "children": [
                    {
                        "name": "minfs",
                        "url": "fuchsia-pkg://fuchsia.com/minfs/stable#meta/minfs.cm",
                    },
                ]
            }),
            output = fdecl::Component {
                capabilities: Some(vec![
                    fdecl::Capability::Protocol (
                        fdecl::Protocol {
                            name: Some("myprotocol".to_string()),
                            source_path: Some("/protocol".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Protocol (
                        fdecl::Protocol {
                            name: Some("myprotocol2".to_string()),
                            source_path: Some("/svc/myprotocol2".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Protocol (
                        fdecl::Protocol {
                            name: Some("myprotocol3".to_string()),
                            source_path: Some("/svc/myprotocol3".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Protocol (
                        fdecl::Protocol {
                            name: Some("myprotocol4".to_string()),
                            source_path: Some("/svc/myprotocol4".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Directory (
                        fdecl::Directory {
                            name: Some("mydirectory".to_string()),
                            source_path: Some("/directory".to_string()),
                            rights: Some(fio::Operations::CONNECT),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Storage (
                        fdecl::Storage {
                            name: Some("mystorage".to_string()),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "minfs".to_string(),
                                collection: None,
                            })),
                            backing_dir: Some("storage".to_string()),
                            subdir: None,
                            storage_id: Some(fdecl::StorageId::StaticInstanceIdOrMoniker),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Storage (
                        fdecl::Storage {
                            name: Some("mystorage2".to_string()),
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "minfs".to_string(),
                                collection: None,
                            })),
                            backing_dir: Some("storage2".to_string()),
                            subdir: None,
                            storage_id: Some(fdecl::StorageId::StaticInstanceId),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Runner (
                        fdecl::Runner {
                            name: Some("myrunner".to_string()),
                            source_path: Some("/runner".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Resolver (
                        fdecl::Resolver {
                            name: Some("myresolver".to_string()),
                            source_path: Some("/resolver".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Dictionary (
                        fdecl::Dictionary {
                            name: Some("dict1".into()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Dictionary (
                        fdecl::Dictionary {
                            name: Some("dict2".into()),
                            source: None,
                            source_dictionary: None,
                            source_path: Some("/in/a".into()),
                            ..Default::default()
                        }
                    ),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("minfs".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/minfs/stable#meta/minfs.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    }
                ]),
                ..default_component_decl()
            },
        },

        test_compile_facets => {
            input = json!({
                "facets": {
                    "title": "foo",
                    "authors": [ "me", "you" ],
                    "year": "2018",
                    "metadata": {
                        "publisher": "The Books Publisher",
                    }
                }
            }),
            output = fdecl::Component {
                facets: Some(fdata::Dictionary {
                        entries: Some(vec![
                            fdata::DictionaryEntry {
                                key: "authors".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::StrVec(vec!["me".to_owned(), "you".to_owned()]))),
                            },
                            fdata::DictionaryEntry {
                                key: "metadata.publisher".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::Str("The Books Publisher".to_string()))),
                            },
                            fdata::DictionaryEntry {
                                key: "title".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::Str("foo".to_string()))),
                            },
                            fdata::DictionaryEntry {
                                key: "year".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::Str("2018".to_string()))),
                            },
                        ]),
                        ..Default::default()
                    }
            ),
            ..default_component_decl()
            },
        },

        test_compile_environment => {
            input = json!({
                "environments": [
                    {
                        "name": "myenv",
                        "__stop_timeout_ms": 10u32,
                    },
                    {
                        "name": "myenv2",
                        "extends": "realm",
                    },
                    {
                        "name": "myenv3",
                        "extends": "none",
                        "__stop_timeout_ms": 8000u32,
                    }
                ],
            }),
            output = fdecl::Component {
                environments: Some(vec![
                    fdecl::Environment {
                        name: Some("myenv".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::None),
                        runners: None,
                        resolvers: None,
                        stop_timeout_ms: Some(10),
                        ..Default::default()
                    },
                    fdecl::Environment {
                        name: Some("myenv2".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::Realm),
                        runners: None,
                        resolvers: None,
                        stop_timeout_ms: None,
                        ..Default::default()
                    },
                    fdecl::Environment {
                        name: Some("myenv3".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::None),
                        runners: None,
                        resolvers: None,
                        stop_timeout_ms: Some(8000),
                        ..Default::default()
                    },
                ]),
                ..default_component_decl()
            },
        },

        test_compile_environment_with_runner_and_resolver => {
            input = json!({
                "environments": [
                    {
                        "name": "myenv",
                        "extends": "realm",
                        "runners": [
                            {
                                "runner": "dart",
                                "from": "parent",
                            }
                        ],
                        "resolvers": [
                            {
                                "resolver": "pkg_resolver",
                                "from": "parent",
                                "scheme": "fuchsia-pkg",
                            }
                        ],
                    },
                ],
            }),
            output = fdecl::Component {
                environments: Some(vec![
                    fdecl::Environment {
                        name: Some("myenv".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::Realm),
                        runners: Some(vec![
                            fdecl::RunnerRegistration {
                                source_name: Some("dart".to_string()),
                                source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                                target_name: Some("dart".to_string()),
                                ..Default::default()
                            }
                        ]),
                        resolvers: Some(vec![
                            fdecl::ResolverRegistration {
                                resolver: Some("pkg_resolver".to_string()),
                                source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                                scheme: Some("fuchsia-pkg".to_string()),
                                ..Default::default()
                            }
                        ]),
                        stop_timeout_ms: None,
                        ..Default::default()
                    },
                ]),
                ..default_component_decl()
            },
        },

        test_compile_environment_with_runner_alias => {
            input = json!({
                "environments": [
                    {
                        "name": "myenv",
                        "extends": "realm",
                        "runners": [
                            {
                                "runner": "dart",
                                "from": "parent",
                                "as": "my-dart",
                            }
                        ],
                    },
                ],
            }),
            output = fdecl::Component {
                environments: Some(vec![
                    fdecl::Environment {
                        name: Some("myenv".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::Realm),
                        runners: Some(vec![
                            fdecl::RunnerRegistration {
                                source_name: Some("dart".to_string()),
                                source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                                target_name: Some("my-dart".to_string()),
                                ..Default::default()
                            }
                        ]),
                        resolvers: None,
                        stop_timeout_ms: None,
                        ..Default::default()
                    },
                ]),
                ..default_component_decl()
            },
        },

        test_compile_environment_with_debug => {
            input = json!({
                "capabilities": [
                    {
                        "protocol": "fuchsia.serve.service",
                    },
                ],
                "environments": [
                    {
                        "name": "myenv",
                        "extends": "realm",
                        "debug": [
                            {
                                "protocol": "fuchsia.serve.service",
                                "from": "self",
                                "as": "my-service",
                            }
                        ],
                    },
                ],
            }),
            output = fdecl::Component {
                capabilities: Some(vec![
                    fdecl::Capability::Protocol(
                        fdecl::Protocol {
                            name : Some("fuchsia.serve.service".to_owned()),
                            source_path: Some("/svc/fuchsia.serve.service".to_owned()),
                            ..Default::default()
                        }
                    )
                ]),
                environments: Some(vec![
                    fdecl::Environment {
                        name: Some("myenv".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::Realm),
                        debug_capabilities: Some(vec![
                            fdecl::DebugRegistration::Protocol( fdecl::DebugProtocolRegistration {
                                source_name: Some("fuchsia.serve.service".to_string()),
                                source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                                target_name: Some("my-service".to_string()),
                                ..Default::default()
                            }),
                        ]),
                        resolvers: None,
                        runners: None,
                        stop_timeout_ms: None,
                        ..Default::default()
                    },
                ]),
                ..default_component_decl()
            },
        },


        test_compile_configuration_capability => {
            input = json!({
                "capabilities": [
                    {
                        "config": "fuchsia.config.true",
                        "type": "bool",
                        "value": true,
                    },
                    {
                        "config": "fuchsia.config.false",
                        "type": "bool",
                        "value": false,
                    },
                ],
            }),
            output = fdecl::Component {
                capabilities: Some(vec![
                    fdecl::Capability::Config (
                        fdecl::Configuration {
                            name: Some("fuchsia.config.true".to_string()),
                            value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Bool(true))),
                            ..Default::default()
                        }),
                    fdecl::Capability::Config (
                        fdecl::Configuration {
                            name: Some("fuchsia.config.false".to_string()),
                            value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Bool(false))),
                            ..Default::default()
                        }),
                ]),
                ..default_component_decl()
            },
        },

        test_compile_all_sections => {
            input = json!({
                "program": {
                    "runner": "elf",
                    "binary": "bin/app",
                },
                "use": [
                    { "protocol": "LegacyCoolFonts", "path": "/svc/fuchsia.fonts.LegacyProvider" },
                    { "protocol": [ "ReallyGoodFonts", "IWouldNeverUseTheseFonts"]},
                    { "protocol":  "DebugProtocol", "from": "debug"},
                ],
                "expose": [
                    { "directory": "blobfs", "from": "self", "rights": ["r*"]},
                ],
                "offer": [
                    {
                        "protocol": "fuchsia.logger.LegacyLog",
                        "from": "#logger",
                        "to": [ "#netstack", "#modular" ],
                        "dependency": "weak"
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                    },
                    {
                        "name": "netstack",
                        "url": "fuchsia-pkg://fuchsia.com/netstack/stable#meta/netstack.cm",
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
                        "directory": "blobfs",
                        "path": "/volumes/blobfs",
                        "rights": [ "r*" ],
                    },
                    {
                        "runner": "myrunner",
                        "path": "/runner",
                    },
                    {
                        "protocol": "fuchsia.serve.service",
                    }
                ],
                "facets": {
                    "author": "Fuchsia",
                    "year": "2018",
                },
                "environments": [
                    {
                        "name": "myenv",
                        "extends": "realm",
                        "debug": [
                            {
                                "protocol": "fuchsia.serve.service",
                                "from": "self",
                                "as": "my-service",
                            },
                            {
                                "protocol": "fuchsia.logger.LegacyLog",
                                "from": "#logger",
                            }
                        ]
                    }
                ],
            }),
            output = fdecl::Component {
                program: Some(fdecl::Program {
                    runner: Some("elf".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: Some(vec![fdata::DictionaryEntry {
                            key: "binary".to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str("bin/app".to_string()))),
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                uses: Some(vec![
                    fdecl::Use::Protocol (
                        fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("LegacyCoolFonts".to_string()),
                            target_path: Some("/svc/fuchsia.fonts.LegacyProvider".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Protocol (
                        fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("ReallyGoodFonts".to_string()),
                            target_path: Some("/svc/ReallyGoodFonts".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Protocol (
                        fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("IWouldNeverUseTheseFonts".to_string()),
                            target_path: Some("/svc/IWouldNeverUseTheseFonts".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Protocol (
                        fdecl::UseProtocol {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Debug(fdecl::DebugRef {})),
                            source_name: Some("DebugProtocol".to_string()),
                            target_path: Some("/svc/DebugProtocol".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                ]),
                exposes: Some(vec![
                    fdecl::Expose::Directory (
                        fdecl::ExposeDirectory {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("blobfs".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            target_name: Some("blobfs".to_string()),
                            rights: Some(
                                fio::Operations::CONNECT | fio::Operations::ENUMERATE |
                                fio::Operations::TRAVERSE | fio::Operations::READ_BYTES |
                                fio::Operations::GET_ATTRIBUTES
                            ),
                            subdir: None,
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                ]),
                offers: Some(vec![
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.LegacyLog".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Weak),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Protocol (
                        fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.LegacyLog".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "modular".to_string(),
                            })),
                            target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Weak),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Directory (
                        fdecl::Directory {
                            name: Some("blobfs".to_string()),
                            source_path: Some("/volumes/blobfs".to_string()),
                            rights: Some(fio::Operations::CONNECT | fio::Operations::ENUMERATE |
                                fio::Operations::TRAVERSE | fio::Operations::READ_BYTES |
                                fio::Operations::GET_ATTRIBUTES
                            ),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Runner (
                        fdecl::Runner {
                            name: Some("myrunner".to_string()),
                            source_path: Some("/runner".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Protocol(
                        fdecl::Protocol {
                            name : Some("fuchsia.serve.service".to_owned()),
                            source_path: Some("/svc/fuchsia.serve.service".to_owned()),
                            ..Default::default()
                        }
                    )
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("netstack".to_string()),
                        url: Some("fuchsia-pkg://fuchsia.com/netstack/stable#meta/netstack.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                ]),
                collections: Some(vec![
                    fdecl::Collection {
                        name: Some("modular".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        environment: None,
                        allowed_offers: None,
                        ..Default::default()
                    }
                ]),
                environments: Some(vec![
                    fdecl::Environment {
                        name: Some("myenv".to_string()),
                        extends: Some(fdecl::EnvironmentExtends::Realm),
                        runners: None,
                        resolvers: None,
                        stop_timeout_ms: None,
                        debug_capabilities: Some(vec![
                            fdecl::DebugRegistration::Protocol( fdecl::DebugProtocolRegistration {
                                source_name: Some("fuchsia.serve.service".to_string()),
                                source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                                target_name: Some("my-service".to_string()),
                                ..Default::default()
                            }),
                            fdecl::DebugRegistration::Protocol( fdecl::DebugProtocolRegistration {
                                source_name: Some("fuchsia.logger.LegacyLog".to_string()),
                                source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                    name: "logger".to_string(),
                                    collection: None,
                                })),
                                target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                                ..Default::default()
                            }),
                        ]),
                        ..Default::default()
                    }
                ]),
                facets: Some(fdata::Dictionary {
                        entries: Some(vec![
                            fdata::DictionaryEntry {
                                key: "author".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::Str("Fuchsia".to_string()))),
                            },
                            fdata::DictionaryEntry {
                                key: "year".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::Str("2018".to_string()))),
                            },
                        ]),
                        ..Default::default()
                    }),
                ..Default::default()
            },
        },
    }

    #[test]
    fn test_maybe_generate_specialization_from_all() {
        let offer = create_offer(
            "fuchsia.logger.LegacyLog",
            OneOrMany::One(OfferFromRef::Parent {}),
            OneOrMany::One(OfferToRef::All),
        );

        let mut offer_set = vec![create_offer(
            "fuchsia.logger.LogSink",
            OneOrMany::One(OfferFromRef::Parent {}),
            OneOrMany::One(OfferToRef::All),
        )];

        let result = maybe_generate_direct_offer_from_all(
            &offer,
            &offer_set,
            &Name::from_str("something").unwrap(),
        );

        assert_matches!(&result[..], [ContextSpanned { value: ContextOffer { protocol: Some(protocol_span), from, to, .. }, .. }] => {
            assert_eq!(
                protocol_span.value,
                OneOrMany::One(Name::from_str("fuchsia.logger.LegacyLog").unwrap()),
            );
            assert_eq!(from.value, OneOrMany::One(OfferFromRef::Parent {}));
            assert_eq!(
                to.value,
                OneOrMany::One(OfferToRef::Named(Name::from_str("something").unwrap())),
            );
        });

        offer_set.push(create_offer(
            "fuchsia.inspect.InspectSink",
            OneOrMany::One(OfferFromRef::Parent {}),
            OneOrMany::One(OfferToRef::Named(Name::from_str("something").unwrap())),
        ));

        let result = maybe_generate_direct_offer_from_all(
            &offer,
            &offer_set,
            &Name::from_str("something").unwrap(),
        );

        assert_matches!(&result[..], [ContextSpanned { value: ContextOffer { protocol: Some(protocol_span), from, to, .. }, .. }] => {
            assert_eq!(
                protocol_span.value,
                OneOrMany::One(Name::from_str("fuchsia.logger.LegacyLog").unwrap()),
            );
            assert_eq!(from.value, OneOrMany::One(OfferFromRef::Parent {}));
            assert_eq!(
                to.value,
                OneOrMany::One(OfferToRef::Named(Name::from_str("something").unwrap())),
            );
        });

        offer_set.push(create_offer(
            "fuchsia.logger.LegacyLog",
            OneOrMany::One(OfferFromRef::Parent {}),
            OneOrMany::One(OfferToRef::Named(Name::from_str("something").unwrap())),
        ));

        assert!(
            maybe_generate_direct_offer_from_all(
                &offer,
                &offer_set,
                &Name::from_str("something").unwrap()
            )
            .is_empty()
        );
    }

    #[test]
    fn test_expose_void_service_capability() {
        let input = must_parse_cml!({
            "expose": [
                {
                    "service": "fuchsia.foo.Bar",
                    "from": [ "#non_existent_child" ],
                    "source_availability": "unknown",
                },
            ],
        });
        let result = compile(&input, CompileOptions::default());
        assert_matches!(result, Ok(_));
    }

    /// Different availabilities aggregated by several service expose declarations is an error.
    #[test]
    fn test_aggregated_capabilities_must_use_same_availability_expose() {
        // Same availability.
        let input = must_parse_cml!({
            "expose": [
                {
                    "service": "fuchsia.foo.Bar",
                    "from": [ "#a", "#b" ],
                    "availability": "optional",
                },
            ],
            "collections": [
                {
                    "name": "a",
                    "durability": "transient",
                },
                {
                    "name": "b",
                    "durability": "transient",
                },
            ],
        });
        let result = compile(&input, CompileOptions::default());
        assert_matches!(result, Ok(_));

        // Different availability.
        let input = must_parse_cml!({
            "expose": [
                {
                    "service": "fuchsia.foo.Bar",
                    "from": [ "#a", "#non_existent" ],
                    "source_availability": "unknown",
                },
            ],
            "collections": [
                {
                    "name": "a",
                    "durability": "transient",
                },
            ],
        });
        let result = compile(&input, CompileOptions::default());
        assert_matches!(
            result,
            Err(Error::FidlValidator  { errs: ErrorList { errs } })
            if matches!(
                &errs[..],
                [
                    CmFidlError::DifferentAvailabilityInAggregation(AvailabilityList(availabilities)),
                ]
                if matches!(
                    &availabilities[..],
                    [ fdecl::Availability::Required, fdecl::Availability::Optional, ]
                )
            )
        );
    }

    #[test]
    fn test_aggregated_capabilities_must_use_same_availability_offer() {
        // Same availability.
        let input = must_parse_cml!({
            "offer": [
                {
                    "service": "fuchsia.foo.Bar",
                    "from": [ "#a", "#b" ],
                    "to": "#c",
                    "availability": "optional",
                },
            ],
            "collections": [
                {
                    "name": "a",
                    "durability": "transient",
                },
                {
                    "name": "b",
                    "durability": "transient",
                },
            ],
            "children": [
                {
                    "name": "c",
                    "url": "fuchsia-pkg://fuchsia.com/c/c#meta/c.cm",
                },
            ],
        });
        let result = compile(&input, CompileOptions::default());
        assert_matches!(result, Ok(_));

        // Different availability.
        let input = must_parse_cml!({
            "offer": [
                {
                    "service": "fuchsia.foo.Bar",
                    "from": [ "#a", "#non_existent" ],
                    "to": "#c",
                    "source_availability": "unknown",
                },
            ],
            "collections": [
                {
                    "name": "a",
                    "durability": "transient",
                },
            ],
            "children": [
                {
                    "name": "c",
                    "url": "fuchsia-pkg://fuchsia.com/c/c#meta/c.cm",
                },
            ],
        });
        let result = compile(&input, CompileOptions::default());
        assert_matches!(
            result,
            Err(Error::FidlValidator  { errs: ErrorList { errs } })
            if matches!(
                &errs[..],
                [
                    CmFidlError::DifferentAvailabilityInAggregation(AvailabilityList(availabilities)),
                ]
                if matches!(
                    &availabilities[..],
                    [ fdecl::Availability::Required, fdecl::Availability::Optional, ]
                )
            )
        );
    }

    #[test]
    fn test_compile_offer_to_all_exact_duplicate_disallowed() {
        let input = must_parse_cml!({
            "children": [
                {
                    "name": "logger",
                    "url": "fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm",
                },
            ],
            "offer": [
                {
                    "protocol": "fuchsia.logger.LogSink",
                    "from": "parent",
                    "to": "all",
                },
                {
                    "protocol": "fuchsia.logger.LogSink",
                    "from": "parent",
                    "to": "all",
                },
            ],
        });
        assert_matches!(
            compile(&input, CompileOptions::default()),
            Err(Error::ValidateContexts { err, .. })
            if &err == "Protocol(s) [\"fuchsia.logger.LogSink\"] offered to \"all\" multiple times"
        );
    }

    #[test]
    fn test_compile_use_config() {
        let input = must_parse_cml!({
            "use": [
                    {
                        "config": "fuchsia.config.Config",
                        "key" : "my_config",
                        "type": "bool",
                    }
            ],
        });
        let options = CompileOptions::new().config_package_path("fake.cvf");
        let actual = compile(&input, options).unwrap();
        let type_ = fdecl::ConfigType {
            layout: fdecl::ConfigTypeLayout::Bool,
            parameters: Some(vec![]),
            constraints: vec![],
        };
        assert_eq!(
            actual.uses.unwrap(),
            vec![fdecl::Use::Config(fdecl::UseConfiguration {
                source_name: Some("fuchsia.config.Config".to_string()),
                source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                target_name: Some("my_config".to_string()),
                availability: Some(fdecl::Availability::Required),
                type_: Some(type_.clone()),
                ..Default::default()
            })]
        );
        assert_eq!(
            actual.config.unwrap().fields.unwrap(),
            [fdecl::ConfigField {
                key: Some("my_config".to_string()),
                type_: Some(type_),
                mutability: Some(fdecl::ConfigMutability::default()),
                ..Default::default()
            }]
            .to_vec(),
        );
    }

    #[test]
    fn test_compile_use_config_optional_bad_type() {
        let input = must_parse_cml!({
            "use": [
                    {
                        "config": "fuchsia.config.Config",
                        "key" : "my_config",
                        "type": "bool",
                        "availability": "optional",
                    }
            ],
        "config": {
            "my_config": { "type": "int8"},
        }
        });
        let options = CompileOptions::new().config_package_path("fake.cvf");
        assert_matches!(
            compile(&input, options),
            Err(Error::ValidateContexts { err, .. })
            if &err == "Use and config block differ on type for key 'my_config'"
        );
    }

    #[test]
    fn test_config_source_from_package() {
        let input = must_parse_cml!({
            "use": [
                    {
                        "config": "fuchsia.config.Config",
                        "key" : "my_config",
                        "type": "bool",
                        "availability": "optional",
                    }
            ],
        "config": {
            "my_config": { "type": "bool"},
        }
        });
        let options = CompileOptions::new().config_package_path("fake.cvf");
        let decl = compile(&input, options).unwrap();
        let config = decl.config.unwrap();
        assert_eq!(
            config.value_source,
            Some(fdecl::ConfigValueSource::PackagePath("fake.cvf".into()))
        );
    }

    #[test]
    fn test_config_source_from_capabilities() {
        let input = must_parse_cml!({
            "use": [
                    {
                        "config": "fuchsia.config.Config",
                        "key" : "my_config",
                        "type": "bool",
                    }
            ],
        });
        let options = CompileOptions::new().config_package_path("fake.cvf");
        let decl = compile(&input, options).unwrap();
        let config = decl.config.unwrap();
        assert_eq!(
            config.value_source,
            Some(
                fdecl::ConfigValueSource::Capabilities(fdecl::ConfigSourceCapabilities::default())
            )
        );
    }

    #[test]
    fn test_config_default() {
        let input = must_parse_cml!({
            "use": [
                    {
                        "config": "fuchsia.config.Config",
                        "key" : "my_config",
                        "type": "bool",
                        "availability": "optional",
                        "default": true
                    }
            ],
        });
        let options = CompileOptions::new().config_package_path("fake.cvf");
        let decl = compile(&input, options).unwrap();
        assert_matches!(
            decl.uses.as_ref().unwrap()[0],
            fdecl::Use::Config(fdecl::UseConfiguration {
                default: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Bool(true))),
                ..
            })
        );
    }

    #[test]
    fn test_config_default_bad_type() {
        let input = must_parse_cml!({
            "use": [
                    {
                        "config": "fuchsia.config.Config",
                        "key" : "my_config",
                        "type": "bool",
                        "availability": "optional",
                        "default": 5
                    }
            ],
        });
        let options = CompileOptions::new().config_package_path("fake.cvf");
        assert_matches!(compile(&input, options), Err(Error::InvalidArgs(_)));
    }

    #[test]
    fn test_compile_protocol_delivery_type() {
        let input = must_parse_cml!({
            "capabilities": [
                {
                    "protocol": "fuchsia.echo.Echo",
                    "delivery": "on_readable",
                }
            ],
        });
        let features = FeatureSet::from(vec![Feature::DeliveryType]);
        let options = CompileOptions::new().features(&features);
        let decl = compile(&input, options).unwrap();
        assert_matches!(
            decl.capabilities.as_ref().unwrap()[0],
            fdecl::Capability::Protocol(fdecl::Protocol {
                delivery: Some(fdecl::DeliveryType::OnReadable),
                ..
            })
        );
    }

    #[test]
    fn test_compile_protocol_setting_delivery_type_requires_feature_flag() {
        let input = must_parse_cml!({
            "capabilities": [
                {
                    "protocol": "fuchsia.echo.Echo",
                    "delivery": "on_readable",
                }
            ],
        });
        assert_matches!(
            compile(&input, CompileOptions::new()),
            Err(Error::RestrictedFeature(feature))
            if feature == "delivery_type"
        );
    }
}
