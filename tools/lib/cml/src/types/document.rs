// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use indexmap::IndexMap;
use itertools::Itertools;

use crate::types::capability::ContextCapability;
use crate::types::child::ContextChild;
use crate::types::collection::ContextCollection;
use crate::types::common::*;
use crate::types::environment::ContextEnvironment;
use crate::types::expose::ContextExpose;
use crate::types::offer::ContextOffer;
use crate::types::program::ContextProgram;
use crate::types::r#use::ContextUse;
use crate::{
    CanonicalizeContext, Capability, CapabilityFromRef, Child, Collection, ConfigKey,
    ConfigValueType, Environment, Error, Expose, Location, Offer, Program, Use, merge_spanned_vec,
};

pub use cm_types::{
    Availability, BorrowedName, BoundedName, DeliveryType, DependencyType, HandleType, Name,
    OnTerminate, ParseError, Path, RelativePath, StartupMode, StorageId, Url,
};
use reference_doc::ReferenceDoc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::{cmp, path};

/// # Component manifest (`.cml`) reference
///
/// A `.cml` file contains a single json5 object literal with the keys below.
///
/// Where string values are expected, a list of valid values is generally documented.
/// The following string value types are reused and must follow specific rules.
///
/// The `.cml` file is compiled into a FIDL wire format (`.cm`) file.
///
/// ## String types
///
/// ### Names {#names}
///
/// Both capabilities and a component's children are named. A name string may
/// consist of one or more of the following characters: `A-Z`, `a-z`, `0-9`,
/// `_`, `.`, `-`. It must not exceed 255 characters in length and may not start
/// with `.` or `-`.
///
/// ### Paths {#paths}
///
/// Paths are sequences of [names](#names) delimited by the `/` character. A path
/// must not exceed 4095 characters in length. Throughout the document,
///
/// - Relative paths cannot start with the `/` character.
/// - Namespace and outgoing directory paths must start with the `/` character.
///
/// ### References {#references}
///
/// A reference string takes the form of `#<name>`, where `<name>` refers to the name of a child:
///
/// - A [static child instance][doc-static-children] whose name is
///     `<name>`, or
/// - A [collection][doc-collections] whose name is `<name>`.
///
/// [doc-static-children]: /docs/concepts/components/v2/realms.md#static-children
/// [doc-collections]: /docs/concepts/components/v2/realms.md#collections
/// [doc-protocol]: /docs/concepts/components/v2/capabilities/protocol.md
/// [doc-dictionaries]: /reference/fidl/fuchsia.component.decl#Dictionary
/// [doc-directory]: /docs/concepts/components/v2/capabilities/directory.md
/// [doc-storage]: /docs/concepts/components/v2/capabilities/storage.md
/// [doc-resolvers]: /docs/concepts/components/v2/capabilities/resolver.md
/// [doc-runners]: /docs/concepts/components/v2/capabilities/runner.md
/// [doc-event]: /docs/concepts/components/v2/capabilities/event.md
/// [doc-service]: /docs/concepts/components/v2/capabilities/service.md
/// [doc-directory-rights]: /docs/concepts/components/v2/capabilities/directory.md#directory-capability-rights
///
/// ## Top-level keys {#document}
#[derive(ReferenceDoc, Deserialize, Debug, Default, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// The optional `include` property describes zero or more other component manifest
    /// files to be merged into this component manifest. For example:
    ///
    /// ```json5
    /// include: [ "syslog/client.shard.cml" ]
    /// ```
    ///
    /// In the example given above, the component manifest is including contents from a
    /// manifest shard provided by the `syslog` library, thus ensuring that the
    /// component functions correctly at runtime if it attempts to write to `syslog`. By
    /// convention such files are called "manifest shards" and end with `.shard.cml`.
    ///
    /// Include paths prepended with `//` are relative to the source root of the Fuchsia
    /// checkout. However, include paths not prepended with `//`, as in the example
    /// above, are resolved from Fuchsia SDK libraries (`//sdk/lib`) that export
    /// component manifest shards.
    ///
    /// For reference, inside the Fuchsia checkout these two include paths are
    /// equivalent:
    ///
    /// * `syslog/client.shard.cml`
    /// * `//sdk/lib/syslog/client.shard.cml`
    ///
    /// You can review the outcome of merging any and all includes into a component
    /// manifest file by invoking the following command:
    ///
    /// Note: The `fx` command below is for developers working in a Fuchsia source
    /// checkout environment.
    ///
    /// ```sh
    /// fx cmc include {{ "<var>" }}cml_file{{ "</var>" }} --includeroot $FUCHSIA_DIR --includepath $FUCHSIA_DIR/sdk/lib
    /// ```
    ///
    /// Includes can cope with duplicate [`use`], [`offer`], [`expose`], or [`capabilities`]
    /// declarations referencing the same capability, as long as the properties are the same. For
    /// example:
    ///
    /// ```json5
    /// // my_component.cml
    /// include: [ "syslog.client.shard.cml" ]
    /// use: [
    ///     {
    ///         protocol: [
    ///             "fuchsia.posix.socket.Provider",
    ///             "fuchsia.logger.LogSink",
    ///         ],
    ///     },
    /// ],
    ///
    /// // syslog.client.shard.cml
    /// use: [
    ///     { protocol: "fuchsia.logger.LogSink" },
    /// ],
    /// ```
    ///
    /// In this example, the contents of the merged file will be the same as my_component.cml --
    /// `fuchsia.logger.LogSink` is deduped.
    ///
    /// However, this would fail to compile:
    ///
    /// ```json5
    /// // my_component.cml
    /// include: [ "syslog.client.shard.cml" ]
    /// use: [
    ///     {
    ///         protocol: "fuchsia.logger.LogSink",
    ///         // properties for fuchsia.logger.LogSink don't match
    ///         from: "#archivist",
    ///     },
    /// ],
    ///
    /// // syslog.client.shard.cml
    /// use: [
    ///     { protocol: "fuchsia.logger.LogSink" },
    /// ],
    /// ```
    ///
    /// An exception to this constraint is the `availability` property. If two routing declarations
    /// are identical, and one availability is stronger than the other, the availability will be
    /// "promoted" to the stronger value (if `availability` is missing, it defaults to `required`).
    /// For example:
    ///
    /// ```json5
    /// // my_component.cml
    /// include: [ "syslog.client.shard.cml" ]
    /// use: [
    ///     {
    ///         protocol: [
    ///             "fuchsia.posix.socket.Provider",
    ///             "fuchsia.logger.LogSink",
    ///         ],
    ///         availability: "optional",
    ///     },
    /// ],
    ///
    /// // syslog.client.shard.cml
    /// use: [
    ///     {
    ///         protocol: "fuchsia.logger.LogSink
    ///         availability: "required",  // This is the default
    ///     },
    /// ],
    /// ```
    ///
    /// Becomes:
    ///
    /// ```json5
    /// use: [
    ///     {
    ///         protocol: "fuchsia.posix.socket.Provider",
    ///         availability: "optional",
    ///     },
    ///     {
    ///         protocol: "fuchsia.logger.LogSink",
    ///         availability: "required",
    ///     },
    /// ],
    /// ```
    ///
    /// Includes are transitive, meaning that shards can have their own includes.
    ///
    /// Include paths can have diamond dependencies. For instance this is valid:
    /// A includes B, A includes C, B includes D, C includes D.
    /// In this case A will transitively include B, C, D.
    ///
    /// Include paths cannot have cycles. For instance this is invalid:
    /// A includes B, B includes A.
    /// A cycle such as the above will result in a compile-time error.
    ///
    /// [`use`]: #use
    /// [`offer`]: #offer
    /// [`expose`]: #expose
    /// [`capabilities`]: #capabilities
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,

    /// Components that are executable include a `program` section. The `program`
    /// section must set the `runner` property to select a [runner][doc-runners] to run
    /// the component. The format of the rest of the `program` section is determined by
    /// that particular runner.
    ///
    /// # ELF runners {#elf-runners}
    ///
    /// If the component uses the ELF runner, `program` must include the following
    /// properties, at a minimum:
    ///
    /// - `runner`: must be set to `"elf"`
    /// - `binary`: Package-relative path to the executable binary
    /// - `args` _(optional)_: List of arguments
    ///
    /// Example:
    ///
    /// ```json5
    /// program: {
    ///     runner: "elf",
    ///     binary: "bin/hippo",
    ///     args: [ "Hello", "hippos!" ],
    /// },
    /// ```
    ///
    /// For a complete list of properties, see: [ELF Runner](/docs/concepts/components/v2/elf_runner.md)
    ///
    /// # Other runners {#other-runners}
    ///
    /// If a component uses a custom runner, values inside the `program` stanza other
    /// than `runner` are specific to the runner. The runner receives the arguments as a
    /// dictionary of key and value pairs. Refer to the specific runner being used to
    /// determine what keys it expects to receive, and how it interprets them.
    ///
    /// [doc-runners]: /docs/concepts/components/v2/capabilities/runner.md
    #[reference_doc(json_type = "object")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<Program>,

    /// The `children` section declares child component instances as described in
    /// [Child component instances][doc-children].
    ///
    /// [doc-children]: /docs/concepts/components/v2/realms.md#child-component-instances
    #[reference_doc(recurse)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<Child>>,

    /// The `collections` section declares collections as described in
    /// [Component collections][doc-collections].
    #[reference_doc(recurse)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collections: Option<Vec<Collection>>,

    /// The `environments` section declares environments as described in
    /// [Environments][doc-environments].
    ///
    /// [doc-environments]: /docs/concepts/components/v2/environments.md
    #[reference_doc(recurse)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environments: Option<Vec<Environment>>,

    /// The `capabilities` section defines capabilities that are provided by this component.
    /// Capabilities that are [offered](#offer) or [exposed](#expose) from `self` must be declared
    /// here.
    ///
    /// # Capability fields
    ///
    /// This supports the following capability keys. Exactly one of these must be set:
    ///
    /// - `protocol`: (_optional `string or array of strings`_)
    /// - `service`: (_optional `string or array of strings`_)
    /// - `directory`: (_optional `string`_)
    /// - `storage`: (_optional `string`_)
    /// - `runner`: (_optional `string`_)
    /// - `resolver`: (_optional `string`_)
    /// - `event_stream`: (_optional `string or array of strings`_)
    /// - `dictionary`: (_optional `string`_)
    /// - `config`: (_optional `string`_)
    ///
    /// # Additional fields
    ///
    /// This supports the following additional fields:
    /// [glossary.outgoing directory]: /docs/glossary/README.md#outgoing-directory
    #[reference_doc(recurse)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<Capability>>,

    /// For executable components, declares capabilities that this
    /// component requires in its [namespace][glossary.namespace] at runtime.
    /// Capabilities are routed from the `parent` unless otherwise specified,
    /// and each capability must have a valid route through all components between
    /// this component and the capability's source.
    ///
    /// # Capability fields
    ///
    /// This supports the following capability keys. Exactly one of these must be set:
    ///
    /// - `service`: (_optional `string or array of strings`_)
    /// - `directory`: (_optional `string`_)
    /// - `protocol`: (_optional `string or array of strings`_)
    /// - `dictionary`: (_optional `string`_)
    /// - `storage`: (_optional `string`_)
    /// - `event_stream`: (_optional `string or array of strings`_)
    /// - `runner`: (_optional `string`_)
    /// - `config`: (_optional `string`_)
    ///
    /// # Additional fields
    ///
    /// This supports the following additional fields:
    /// [glossary.namespace]: /docs/glossary/README.md#namespace
    #[reference_doc(recurse)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#use: Option<Vec<Use>>,

    /// Declares the capabilities that are made available to the parent component or to the
    /// framework. It is valid to `expose` from `self` or from a child component.
    ///
    /// # Capability fields
    ///
    /// This supports the following capability keys. Exactly one of these must be set:
    ///
    /// - `service`: (_optional `string or array of strings`_)
    /// - `protocol`: (_optional `string or array of strings`_)
    /// - `directory`: (_optional `string`_)
    /// - `runner`: (_optional `string`_)
    /// - `resolver`: (_optional `string`_)
    /// - `dictionary`: (_optional `string`_)
    /// - `config`: (_optional `string`_)
    ///
    /// # Additional fields
    ///
    /// This supports the following additional fields:
    #[reference_doc(recurse)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expose: Option<Vec<Expose>>,

    /// Declares the capabilities that are made available to a [child component][doc-children]
    /// instance or a [child collection][doc-collections].
    ///
    /// # Capability fields
    ///
    /// This supports the following capability keys. Exactly one of these must be set:
    ///
    /// - `protocol`: (_optional `string or array of strings`_)
    /// - `service`: (_optional `string or array of strings`_)
    /// - `directory`: (_optional `string`_)
    /// - `storage`: (_optional `string`_)
    /// - `runner`: (_optional `string`_)
    /// - `resolver`: (_optional `string`_)
    /// - `event_stream`: (_optional `string or array of strings`_)
    /// - `dictionary`: (_optional `string`_)
    /// - `config`: (_optional `string`_)
    ///
    /// # Additional fields
    ///
    /// This supports the following additional fields:
    #[reference_doc(recurse)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offer: Option<Vec<Offer>>,

    /// Contains metadata that components may interpret for their own purposes. The component
    /// framework enforces no schema for this section, but third parties may expect their facets to
    /// adhere to a particular schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<IndexMap<String, Value>>,

    /// The configuration schema as defined by a component. Each key represents a single field
    /// in the schema.
    ///
    /// Configuration fields are JSON objects and must define a `type` which can be one of the
    /// following strings:
    /// `bool`, `uint8`, `int8`, `uint16`, `int16`, `uint32`, `int32`, `uint64`, `int64`,
    /// `string`, `vector`
    ///
    /// Example:
    ///
    /// ```json5
    /// config: {
    ///     debug_mode: {
    ///         type: "bool"
    ///     },
    /// }
    /// ```
    ///
    /// Fields are resolved from a component's package by default. To be able to change the values
    /// at runtime a `mutability` specifier is required.
    ///
    /// Example:
    ///
    /// ```json5
    /// config: {
    ///     verbose: {
    ///         type: "bool",
    ///         mutability: [ "parent" ],
    ///     },
    /// },
    /// ```
    ///
    /// Currently `"parent"` is the only mutability specifier supported.
    ///
    /// Strings must define the `max_size` property as a non-zero integer.
    ///
    /// Example:
    ///
    /// ```json5
    /// config: {
    ///     verbosity: {
    ///         type: "string",
    ///         max_size: 20,
    ///     }
    /// }
    /// ```
    ///
    /// Vectors must set the `max_count` property as a non-zero integer. Vectors must also set the
    /// `element` property as a JSON object which describes the element being contained in the
    /// vector. Vectors can contain booleans, integers, and strings but cannot contain other
    /// vectors.
    ///
    /// Example:
    ///
    /// ```json5
    /// config: {
    ///     tags: {
    ///         type: "vector",
    ///         max_count: 20,
    ///         element: {
    ///             type: "string",
    ///             max_size: 50,
    ///         }
    ///     }
    /// }
    /// ```
    #[reference_doc(json_type = "object")]
    #[serde(skip_serializing_if = "Option::is_none")]
    // NB: Unlike other maps the order of these fields matters for the ABI of generated config
    // libraries. Rather than insertion order, we explicitly sort the fields here to dissuade
    // developers from taking a dependency on the source ordering in their manifest. In the future
    // this will hopefully make it easier to pursue layout size optimizations.
    pub config: Option<BTreeMap<ConfigKey, ConfigValueType>>,
}

fn merge_from_context_capability_field<T: ContextCapabilityClause>(
    us: &mut Option<Vec<T>>,
    other: &mut Option<Vec<T>>,
) -> Result<(), Error> {
    // Empty entries are an error, and merging removes empty entries so we first need to check
    // for them.
    for entry in us.iter().flatten().chain(other.iter().flatten()) {
        if entry.names().is_empty() {
            return Err(Error::Validate {
                // TODO change error type
                err: format!("{}: Missing type name: {:#?}", entry.decl_type(), entry),
                filename: None,
            });
        }
    }

    if let Some(all_ours) = us.as_mut() {
        if let Some(all_theirs) = other.take() {
            for mut theirs in all_theirs {
                for ours in &mut *all_ours {
                    compute_diff_context(ours, &mut theirs);
                }
                all_ours.push(theirs);
            }
        }
        // Post-filter step: remove empty entries.
        all_ours.retain(|ours| !ours.names().is_empty())
    } else if let Some(theirs) = other.take() {
        us.replace(theirs);
    }
    Ok(())
}

/// Subtracts the capabilities in `ours` from `theirs` if the declarations match in their type and
/// other fields, resulting in the removal of duplicates between `ours` and `theirs`. Stores the
/// result in `theirs`.
///
/// Inexact matches on `availability` are allowed if there is a partial order between them. The
/// stronger availability is chosen.
fn compute_diff_context<T: ContextCapabilityClause>(ours: &mut T, theirs: &mut T) {
    let our_spanned = ours.names();
    let their_spanned = theirs.names();

    if our_spanned.is_empty() || their_spanned.is_empty() {
        return;
    }

    if ours.capability_type(None).unwrap() != theirs.capability_type(None).unwrap() {
        return;
    }

    let mut ours_check = ours.clone();
    let mut theirs_check = theirs.clone();

    ours_check.set_names(Vec::new());
    theirs_check.set_names(Vec::new());
    ours_check.set_availability(None);
    theirs_check.set_availability(None);

    if ours_check != theirs_check {
        return;
    }

    let our_avail = ours.availability().map(|a| a.value).unwrap_or_default();
    let their_avail = theirs.availability().map(|a| a.value).unwrap_or_default();

    let Some(avail_cmp) = our_avail.partial_cmp(&their_avail) else {
        return;
    };

    let our_raw_set: HashSet<&Name> = our_spanned.iter().map(|s| &s.value).collect();

    let mut remove_from_ours_raw = HashSet::new();
    let mut remove_from_theirs_raw = HashSet::new();

    for item in &their_spanned {
        let name = &item.value;
        if !our_raw_set.contains(name) {
            continue;
        }

        match avail_cmp {
            cmp::Ordering::Less => {
                remove_from_ours_raw.insert(name.clone());
            }
            cmp::Ordering::Greater => {
                remove_from_theirs_raw.insert(name.clone());
            }
            cmp::Ordering::Equal => {
                remove_from_theirs_raw.insert(name.clone());
            }
        }
    }

    if !remove_from_ours_raw.is_empty() {
        let new_ours =
            our_spanned.into_iter().filter(|s| !remove_from_ours_raw.contains(&s.value)).collect();
        ours.set_names(new_ours);
    }

    if !remove_from_theirs_raw.is_empty() {
        let new_theirs = their_spanned
            .into_iter()
            .filter(|s| !remove_from_theirs_raw.contains(&s.value))
            .collect();
        theirs.set_names(new_theirs);
    }
}

/// Trait that allows us to merge `serde_json::Map`s into `indexmap::IndexMap`s and vice versa.
trait ValueMap {
    fn get_mut(&mut self, key: &str) -> Option<&mut Value>;
    fn insert(&mut self, key: String, val: Value);
}

impl ValueMap for Map<String, Value> {
    fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.get_mut(key)
    }

    fn insert(&mut self, key: String, val: Value) {
        self.insert(key, val);
    }
}

impl ValueMap for IndexMap<String, Value> {
    fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.get_mut(key)
    }

    fn insert(&mut self, key: String, val: Value) {
        self.insert(key, val);
    }
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct DocumentContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<ContextSpanned<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<ContextSpanned<ContextProgram>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<ContextSpanned<ContextChild>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collections: Option<Vec<ContextSpanned<ContextCollection>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environments: Option<Vec<ContextSpanned<ContextEnvironment>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<ContextSpanned<ContextCapability>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#use: Option<Vec<ContextSpanned<ContextUse>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expose: Option<Vec<ContextSpanned<ContextExpose>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offer: Option<Vec<ContextSpanned<ContextOffer>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<IndexMap<String, ContextSpanned<Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<BTreeMap<ConfigKey, ContextSpanned<ConfigValueType>>>,
}

impl DocumentContext {
    pub fn merge_from(
        &mut self,
        mut other: DocumentContext,
        include_path: &path::Path,
    ) -> Result<(), Error> {
        merge_spanned_vec!(self, other, include);
        self.merge_program(&mut other, include_path)?;
        merge_spanned_vec!(self, other, children);
        merge_spanned_vec!(self, other, collections);
        self.merge_environment(&mut other)?;
        merge_from_context_capability_field(&mut self.capabilities, &mut other.capabilities)?;
        merge_from_context_capability_field(&mut self.r#use, &mut other.r#use)?;
        merge_from_context_capability_field(&mut self.expose, &mut other.expose)?;
        merge_from_context_capability_field(&mut self.offer, &mut other.offer)?;
        self.merge_facets(&mut other, include_path)?;
        self.merge_config(&mut other)?;
        Ok(())
    }

    pub fn canonicalize(&mut self) {
        if let Some(children) = &mut self.children {
            children.sort_by(|a, b| a.value.name.cmp(&b.value.name));
        }
        if let Some(collections) = &mut self.collections {
            collections.sort_by(|a, b| a.value.name.cmp(&b.value.name));
        }
        if let Some(environments) = &mut self.environments {
            environments.sort_by(|a, b| a.value.name.cmp(&b.value.name));
        }
        if let Some(capabilities) = &mut self.capabilities {
            capabilities.canonicalize_context();
        }
        if let Some(offers) = &mut self.offer {
            offers.canonicalize_context();
        }
        if let Some(expose) = &mut self.expose {
            expose.canonicalize_context();
        }
        if let Some(r#use) = &mut self.r#use {
            r#use.canonicalize_context();
        }
    }

    pub fn all_storage_names(&self) -> Vec<&BorrowedName> {
        if let Some(capabilities) = self.capabilities.as_ref() {
            capabilities
                .iter()
                .filter_map(|c| c.value.storage.as_ref().map(|n| n.value.as_ref()))
                .collect()
        } else {
            vec![]
        }
    }

    pub fn all_storage_with_sources<'a>(&'a self) -> HashMap<Name, &'a CapabilityFromRef> {
        if let Some(capabilities) = self.capabilities.as_ref() {
            capabilities
                .iter()
                .filter_map(|cap_wrapper| {
                    let c = &cap_wrapper.value;

                    let storage_span_opt = c.storage.as_ref();
                    let source_span_opt = c.from.as_ref();

                    match (storage_span_opt, source_span_opt) {
                        (Some(s_span), Some(f_span)) => {
                            let name_ref: Name = s_span.value.clone();
                            let source_ref: &CapabilityFromRef = &f_span.value;

                            Some((name_ref, source_ref))
                        }
                        _ => None,
                    }
                })
                .collect()
        } else {
            HashMap::new()
        }
    }

    pub fn all_capability_names(&self) -> HashSet<Name> {
        self.capabilities
            .as_ref()
            .map(|c| {
                c.iter()
                    .flat_map(|capability_wrapper| capability_wrapper.value.names())
                    .map(|spanned_name| spanned_name.value)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn all_collection_names(&self) -> Vec<&BorrowedName> {
        if let Some(collections) = self.collections.as_ref() {
            collections.iter().map(|c| c.value.name.value.as_ref()).collect()
        } else {
            vec![]
        }
    }

    pub fn all_config_names(&self) -> Vec<&BorrowedName> {
        self.capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .filter_map(|cap_wrapper| {
                        let cap = &cap_wrapper.value;

                        cap.config.as_ref().map(|spanned_key| spanned_key.value.as_ref())
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec![])
    }

    pub fn all_children_names(&self) -> Vec<&BorrowedName> {
        self.children
            .as_ref()
            .map(|children| children.iter().map(|c| c.value.name.value.as_ref()).collect())
            .unwrap_or_default()
    }

    pub fn all_dictionaries<'a>(&'a self) -> HashMap<Name, &'a ContextCapability> {
        if let Some(capabilities) = self.capabilities.as_ref() {
            capabilities
                .iter()
                .filter_map(|cap_wrapper| {
                    let cap = &cap_wrapper.value;
                    let dict_span_opt = cap.dictionary.as_ref();

                    dict_span_opt.and_then(|dict_span| {
                        let name_value = &dict_span.value;
                        let name: Name = name_value.clone();
                        Some((name, cap))
                    })
                })
                .collect()
        } else {
            HashMap::new()
        }
    }

    pub fn all_dictionary_names(&self) -> Vec<&BorrowedName> {
        if let Some(capabilities) = self.capabilities.as_ref() {
            capabilities
                .iter()
                .filter_map(|c| c.value.dictionary.as_ref().map(|d| d.value.as_ref()))
                .collect()
        } else {
            vec![]
        }
    }

    pub fn all_environment_names(&self) -> Vec<&BorrowedName> {
        self.environments
            .as_ref()
            .map(|c| c.iter().map(|s| s.value.name.value.as_ref()).collect())
            .unwrap_or_else(|| vec![])
    }

    pub fn all_runner_names(&self) -> Vec<&BorrowedName> {
        self.capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .filter_map(|cap_wrapper| {
                        let cap = &cap_wrapper.value;

                        cap.runner.as_ref().map(|spanned_key| spanned_key.value.as_ref())
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec![])
    }

    pub fn all_resolver_names(&self) -> Vec<&BorrowedName> {
        self.capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .filter_map(|cap_wrapper| {
                        let cap = &cap_wrapper.value;

                        cap.resolver.as_ref().map(|spanned_key| spanned_key.value.as_ref())
                    })
                    .collect()
            })
            //.map(|c| c.iter().filter_map(|c| c.resolver.as_ref().map(Name::as_ref)).collect())
            .unwrap_or_else(|| vec![])
    }

    fn merge_program(
        &mut self,
        other: &mut DocumentContext,
        include_path: &path::Path,
    ) -> Result<(), Error> {
        if other.program.is_none() {
            return Ok(());
        }
        if self.program.is_none() {
            self.program = other.program.clone();
            return Ok(());
        }

        let my_program = &mut self.program.as_mut().unwrap().value;
        let other_wrapper = other.program.as_mut().unwrap();

        let other_origin = other_wrapper.origin.clone();
        let other_program_val = &mut other_wrapper.value;

        if let Some(other_runner) = other_program_val.runner.take() {
            if let Some(my_runner) = my_program.runner.as_ref() {
                if my_runner.value != other_runner.value {
                    return Err(Error::merge(
                        format!(
                            "Manifest include had a conflicting `program.runner`: parent='{}', include='{}'",
                            my_runner.value, other_runner.value
                        ),
                        Some(other_runner.origin),
                    ));
                }
            } else {
                my_program.runner = Some(other_runner);
            }
        }

        Self::merge_maps_unified(
            &mut my_program.info,
            &other_program_val.info,
            "program",
            include_path,
            Some(&other_origin),
            Some(&vec!["environ", "features"]),
        )
    }

    fn merge_environment(&mut self, other: &mut DocumentContext) -> Result<(), Error> {
        if other.environments.is_none() {
            return Ok(());
        }
        if self.environments.is_none() {
            self.environments = Some(vec![]);
        }

        let merged_results = {
            let my_environments = self.environments.as_mut().unwrap();
            let other_environments = other.environments.as_mut().unwrap();

            my_environments.sort_by(|x, y| x.value.name.value.cmp(&y.value.name.value));
            other_environments.sort_by(|x, y| x.value.name.value.cmp(&y.value.name.value));

            let all_environments =
                my_environments.drain(..).merge_by(other_environments.drain(..), |x, y| {
                    x.value.name.value <= y.value.name.value
                });

            let groups = all_environments.chunk_by(|e| e.value.name.value.clone());

            let mut results = vec![];
            for (_name_value, group) in &groups {
                let mut group_iter = group.into_iter();
                let first_wrapper = group_iter.next().expect("chunk cannot be empty");
                let first_origin = first_wrapper.origin.clone();
                let mut merged_inner = first_wrapper.value;

                for subsequent in group_iter {
                    merged_inner.merge_from(subsequent.value)?;
                }

                results.push(ContextSpanned { value: merged_inner, origin: first_origin });
            }
            results
        };

        self.environments = Some(merged_results);
        Ok(())
    }

    fn merge_facets(
        &mut self,
        other: &mut DocumentContext,
        include_path: &path::Path,
    ) -> Result<(), Error> {
        if let None = other.facets {
            return Ok(());
        }
        if let None = self.facets {
            self.facets = Some(Default::default());
        }
        let other_facets = other.facets.as_ref().unwrap();

        for (key, include_spanned) in other_facets {
            let entry_origin = Some(&include_spanned.origin);
            let my_facets = self.facets.as_mut().unwrap();

            if !my_facets.contains_key(key) {
                my_facets.insert(key.clone(), include_spanned.clone());
            } else {
                let self_spanned = my_facets.get_mut(key).unwrap();
                match (&mut self_spanned.value, &include_spanned.value) {
                    (
                        serde_json::Value::Object(self_obj),
                        serde_json::Value::Object(include_obj),
                    ) => {
                        Self::merge_maps_unified(
                            self_obj,
                            include_obj,
                            &format!("facets.{}", key),
                            include_path,
                            entry_origin,
                            None,
                        )?;
                    }
                    (v1, v2) => {
                        if v1 != v2 {
                            return Err(Error::merge(
                                format!(
                                    "Manifest include '{}' had a conflicting value for field \"facets.{}\"",
                                    include_path.display(),
                                    key
                                ),
                                entry_origin.cloned(),
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn merge_config(&mut self, other: &mut DocumentContext) -> Result<(), Error> {
        if other.config.is_none() {
            return Ok(());
        }
        if self.config.is_none() {
            self.config = Some(BTreeMap::new());
        }

        let my_config = self.config.as_mut().unwrap();
        let other_config = other.config.as_ref().unwrap();

        for (key, other_spanned) in other_config {
            if let Some(my_spanned) = my_config.get(key) {
                if my_spanned.value != other_spanned.value {
                    return Err(Error::merge(
                        format!("Conflicting configuration key found: '{}'", key),
                        Some(other_spanned.origin.clone()),
                    ));
                }
            } else {
                my_config.insert(key.clone(), other_spanned.clone());
            }
        }
        Ok(())
    }

    fn merge_maps_unified<'s, Source, Dest>(
        self_map: &mut Dest,
        include_map: Source,
        outer_key: &str,
        include_path: &path::Path,
        origin: Option<&Arc<path::Path>>,
        allow_array_concatenation_keys: Option<&Vec<&str>>,
    ) -> Result<(), Error>
    where
        Source: IntoIterator<Item = (&'s String, &'s serde_json::Value)>,
        Dest: ValueMap,
    {
        for (key, include_val) in include_map {
            match self_map.get_mut(key) {
                None => {
                    self_map.insert(key.clone(), include_val.clone());
                }
                Some(self_val) => match (self_val, include_val) {
                    (serde_json::Value::Object(s_inner), serde_json::Value::Object(i_inner)) => {
                        let combined_key = format!("{}.{}", outer_key, key);
                        Self::merge_maps_unified(
                            s_inner,
                            i_inner,
                            &combined_key,
                            include_path,
                            origin,
                            allow_array_concatenation_keys,
                        )?;
                    }
                    (serde_json::Value::Array(s_arr), serde_json::Value::Array(i_arr)) => {
                        let is_allowed = allow_array_concatenation_keys
                            .map_or(true, |keys| keys.contains(&key.as_str()));

                        if is_allowed {
                            s_arr.extend(i_arr.clone());
                        } else if s_arr != i_arr {
                            return Err(Error::merge(
                                format!(
                                    "Conflicting array values for field \"{}.{}\"",
                                    outer_key, key
                                ),
                                origin.cloned(),
                            ));
                        }
                    }
                    (v1, v2) if v1 == v2 => {}
                    _ => {
                        return Err(Error::merge(
                            format!(
                                "Manifest include '{}' had a conflicting value for field \"{}.{}\"",
                                include_path.display(),
                                outer_key,
                                key
                            ),
                            origin.cloned(),
                        ));
                    }
                },
            }
        }
        Ok(())
    }

    pub fn includes(&self) -> Vec<String> {
        self.include
            .as_ref()
            .map(|includes| includes.iter().map(|s| s.value.clone()).collect())
            .unwrap_or_default()
    }
}

pub fn parse_and_hydrate(
    file_arc: Arc<std::path::Path>,
    buffer: &String,
) -> Result<DocumentContext, Error> {
    let parsed_doc: Document = serde_json5::from_str(buffer).map_err(|e| {
        let serde_json5::Error::Message { location, msg } = e;
        let location = location.map(|l| Location { line: l.line, column: l.column });
        Error::parse(msg, location, Some(&file_arc.clone()))
    })?;

    let include = parsed_doc.include.map(|raw_includes| {
        raw_includes
            .into_iter()
            .map(|path| hydrate_simple(path, &file_arc))
            .collect::<Vec<ContextSpanned<String>>>()
    });

    let facets = parsed_doc.facets.map(|raw_facets| {
        raw_facets
            .into_iter()
            .map(|(key, val)| (key, hydrate_simple(val, &file_arc)))
            .collect::<IndexMap<String, ContextSpanned<serde_json::Value>>>()
    });

    let config = parsed_doc.config.map(|raw_config| {
        raw_config
            .into_iter()
            .map(|(key, val)| (key, hydrate_simple(val, &file_arc)))
            .collect::<BTreeMap<ConfigKey, ContextSpanned<ConfigValueType>>>()
    });

    Ok(DocumentContext {
        include,
        program: hydrate_opt(parsed_doc.program, &file_arc)?,
        children: hydrate_list(parsed_doc.children, &file_arc)?,
        collections: hydrate_list(parsed_doc.collections, &file_arc)?,
        environments: hydrate_list(parsed_doc.environments, &file_arc)?,
        capabilities: hydrate_list(parsed_doc.capabilities, &file_arc)?,
        r#use: hydrate_list(parsed_doc.r#use, &file_arc)?,
        expose: hydrate_list(parsed_doc.expose, &file_arc)?,
        offer: hydrate_list(parsed_doc.offer, &file_arc)?,
        facets,
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OneOrMany;
    use difference::Changeset;
    use serde_json::{json, to_string_pretty, to_value};
    use std::path;
    use std::path::Path;
    use test_case::test_case;

    fn document_context(contents: &str) -> DocumentContext {
        let file_arc = Arc::from(std::path::Path::new("test.cml"));
        parse_and_hydrate(file_arc, &contents.to_string()).unwrap()
    }

    macro_rules! assert_json_eq {
        ($a:expr, $e:expr) => {{
            if $a != $e {
                let expected = to_string_pretty(&$e).unwrap();
                let actual = to_string_pretty(&$a).unwrap();
                assert_eq!(
                    $a,
                    $e,
                    "JSON actual != expected. Diffs:\n\n{}",
                    Changeset::new(&actual, &expected, "\n")
                );
            }
        }};
    }

    #[test]
    fn test_includes() {
        let buffer = r##"{}"##;
        let empty_document = document_context(buffer);
        assert_eq!(empty_document.includes(), Vec::<String>::new());

        let buffer = r##"{"include": []}"##;
        let empty_include = document_context(buffer);
        assert_eq!(empty_include.includes(), Vec::<String>::new());

        let buffer = r##"{ "include": [ "foo.cml", "bar.cml" ]}"##;
        let include_doc = document_context(buffer);

        assert_eq!(include_doc.includes(), vec!["foo.cml", "bar.cml"]);
    }

    #[test]
    fn test_merge_same_section() {
        let mut some = document_context(r##"{ "use": [{ "protocol": "foo" }] }"##);
        let other = document_context(r##"{ "use": [{ "protocol": "bar" }] }"##);
        some.merge_from(other, &Path::new("some/path")).unwrap();
        let uses = some.r#use.as_ref().unwrap();
        assert_eq!(uses.len(), 2);
        let get_protocol = |u: &ContextSpanned<ContextUse>| -> String {
            let proto_wrapper = u.value.protocol.as_ref().expect("Missing protocol");

            match &proto_wrapper.value {
                OneOrMany::One(name) => name.to_string(),
                OneOrMany::Many(_) => panic!("Expected single protocol, found list"),
            }
        };

        assert_eq!(get_protocol(&uses[0]), "foo");
        assert_eq!(get_protocol(&uses[1]), "bar");
    }

    #[test]
    fn test_merge_upgraded_availability() {
        let mut some =
            document_context(r##"{ "use": [{ "protocol": "foo", "availability": "optional" }] }"##);
        let other1 = document_context(r##"{ "use": [{ "protocol": "foo" }] }"##);
        let other2 = document_context(
            r##"{ "use": [{ "protocol": "foo", "availability": "transitional" }] }"##,
        );
        let other3 = document_context(
            r##"{ "use": [{ "protocol": "foo", "availability": "same_as_target" }] }"##,
        );
        some.merge_from(other1, &Path::new("some/path")).unwrap();
        some.merge_from(other2, &Path::new("some/path")).unwrap();
        some.merge_from(other3, &Path::new("some/path")).unwrap();

        let uses = some.r#use.as_ref().unwrap();
        assert_eq!(uses.len(), 2);
        assert_eq!(
            uses[0].protocol().as_ref().unwrap().value,
            OneOrMany::One("foo".parse::<Name>().unwrap().as_ref())
        );
        assert!(uses[0].availability().is_none());
        assert_eq!(
            uses[1].protocol().as_ref().unwrap().value,
            OneOrMany::One("foo".parse::<Name>().unwrap().as_ref())
        );
        assert_eq!(uses[1].availability().as_ref().unwrap().value, Availability::SameAsTarget,);
    }

    #[test]
    fn test_merge_different_sections() {
        let mut some = document_context(r##"{ "use": [{ "protocol": "foo" }] }"##);
        let other = document_context(r##"{ "expose": [{ "protocol": "bar", "from": "self" }] }"##);
        some.merge_from(other, &Path::new("some/path")).unwrap();
        let uses = some.r#use.as_ref().unwrap();
        let exposes = some.expose.as_ref().unwrap();
        assert_eq!(uses.len(), 1);
        assert_eq!(exposes.len(), 1);
        assert_eq!(
            uses[0].protocol().as_ref().unwrap().value,
            OneOrMany::One("foo".parse::<Name>().unwrap().as_ref())
        );
        assert_eq!(
            exposes[0].protocol().as_ref().unwrap().value,
            OneOrMany::One("bar".parse::<Name>().unwrap().as_ref())
        );
    }

    #[test]
    fn test_merge_environments() {
        let mut some = document_context(
            r##"
        { "environments": [
            {
                "name": "one",
                "extends": "realm"
            },
            {
                "name": "two",
                "extends": "none",
                "runners": [
                    {
                        "runner": "r1",
                        "from": "#c1"
                    },
                    {
                        "runner": "r2",
                        "from": "#c2"
                    }
                ],
                "resolvers": [
                    {
                        "resolver": "res1",
                        "from": "#c1",
                        "scheme": "foo"
                    }
                ],
                "debug": [
                    {
                        "protocol": "baz",
                        "from": "#c2"
                    }
                ]
            }
        ]}"##,
        );
        let other = document_context(
            r##"
        { "environments": [
            {
                "name": "two",
                "__stop_timeout_ms": 100,
                "runners": [
                    {
                        "runner": "r3",
                        "from": "#c3"
                    }
                ],
                "resolvers": [
                    {
                        "resolver": "res2",
                        "from": "#c1",
                        "scheme": "bar"
                    }
                ],
                "debug": [
                    {
                        "protocol": "faz",
                        "from": "#c2"
                    }
                ]
            },
            {
                "name": "three",
                "__stop_timeout_ms": 1000
            }
        ]}"##,
        );
        some.merge_from(other, &Path::new("some/path")).unwrap();
        assert_eq!(
            to_value(some).unwrap(),
            json!({"environments": [
                {
                    "name": "one",
                    "extends": "realm",
                },
                {
                    "name": "three",
                    "__stop_timeout_ms": 1000,
                },
                {
                    "name": "two",
                    "extends": "none",
                    "__stop_timeout_ms": 100,
                    "runners": [
                        {
                            "runner": "r1",
                            "from": "#c1",
                        },
                        {
                            "runner": "r2",
                            "from": "#c2",
                        },
                        {
                            "runner": "r3",
                            "from": "#c3",
                        },
                    ],
                    "resolvers": [
                        {
                            "resolver": "res1",
                            "from": "#c1",
                            "scheme": "foo",
                        },
                        {
                            "resolver": "res2",
                            "from": "#c1",
                            "scheme": "bar",
                        },
                    ],
                    "debug": [
                        {
                            "protocol": "baz",
                            "from": "#c2"
                        },
                        {
                            "protocol": "faz",
                            "from": "#c2"
                        }
                    ]
                },
            ]})
        );
    }

    #[test]
    fn test_merge_environments_errors() {
        {
            let mut some =
                document_context(r##"{"environments": [{"name": "one", "extends": "realm"}]}"##);
            let other =
                document_context(r##"{"environments": [{"name": "one", "extends": "none"}]}"##);
            assert!(some.merge_from(other, &Path::new("some/path")).is_err());
        }
        {
            let mut some = document_context(
                r##"{"environments": [{"name": "one", "__stop_timeout_ms": 10}]}"##,
            );
            let other = document_context(
                r##"{"environments": [{"name": "one", "__stop_timeout_ms": 20}]}"##,
            );
            assert!(some.merge_from(other, &Path::new("some/path")).is_err());
        }

        // It's ok if the values match.
        {
            let mut some =
                document_context(r##"{"environments": [{"name": "one", "extends": "realm"}]}"##);
            let other =
                document_context(r##"{"environments": [{"name": "one", "extends": "realm"}]}"##);
            some.merge_from(other, &Path::new("some/path")).unwrap();
            assert_eq!(
                to_value(some).unwrap(),
                json!({"environments": [{"name": "one", "extends": "realm"}]})
            );
        }
        {
            let mut some = document_context(
                r##"{"environments": [{"name": "one", "__stop_timeout_ms": 10}]}"##,
            );
            let other = document_context(
                r##"{"environments": [{"name": "one", "__stop_timeout_ms": 10}]}"##,
            );
            some.merge_from(other, &Path::new("some/path")).unwrap();
            assert_eq!(
                to_value(some).unwrap(),
                json!({"environments": [{"name": "one", "__stop_timeout_ms": 10}]})
            );
        }
    }

    #[test]
    fn test_merge_from_other_config() {
        let mut some = document_context(r##"{}"##);
        let other = document_context(r##"{ "config": { "bar": { "type": "bool" } } }"##);

        some.merge_from(other, &path::Path::new("some/path")).unwrap();
        let expected = document_context(r##"{ "config": { "bar": { "type": "bool" } } }"##);
        assert_eq!(some.config, expected.config);
    }

    #[test]
    fn test_merge_from_some_config() {
        let mut some = document_context(r##"{ "config": { "bar": { "type": "bool" } } }"##);
        let other = document_context(r##"{}"##);

        some.merge_from(other, &path::Path::new("some/path")).unwrap();
        let expected = document_context(r##"{ "config": { "bar": { "type": "bool" } } }"##);
        assert_eq!(some.config, expected.config);
    }

    #[test]
    fn test_merge_from_config() {
        let mut some = document_context(r##"{ "config": { "foo": { "type": "bool" } } }"##);
        let other = document_context(r##"{ "config": { "bar": { "type": "bool" } } }"##);
        some.merge_from(other, &path::Path::new("some/path")).unwrap();

        assert_eq!(
            to_value(some).unwrap(),
            json!({
                "config": {
                    "foo": { "type": "bool" },
                    "bar": { "type": "bool" }
                }
            }),
        );
    }

    #[test]
    fn test_merge_from_config_dedupe_identical_fields() {
        let mut some = document_context(r##"{ "config": { "foo": { "type": "bool" } } }"##);
        let other = document_context(r##"{ "config": { "foo": { "type": "bool" } } }"##);
        some.merge_from(other, &path::Path::new("some/path")).unwrap();

        assert_eq!(to_value(some).unwrap(), json!({ "config": { "foo": { "type": "bool" } } }));
    }

    #[test]
    fn test_merge_from_config_conflicting_keys() {
        let mut some = document_context(r##"{ "config": { "foo": { "type": "bool" } } }"##);
        let other = document_context(r##"{ "config": { "foo": { "type": "uint8" } } }"##);

        assert_matches::assert_matches!(
            some.merge_from(other, &path::Path::new("some/path")),
            Err(Error::Merge { err, .. })
                if err == "Conflicting configuration key found: 'foo'"
        );
    }

    #[test]
    fn test_canonicalize_context() {
        let mut some = document_context(
            &json!({
                "children": [
                    // Will be sorted by name
                    { "name": "b_child", "url": "http://foo/b" },
                    { "name": "a_child", "url": "http://foo/a" },
                ],
                "environments": [
                    // Will be sorted by name
                    { "name": "b_env" },
                    { "name": "a_env" },
                ],
                "collections": [
                    // Will be sorted by name
                    { "name": "b_coll", "durability": "transient" },
                    { "name": "a_coll", "durability": "transient" },
                ],
                // Will have entries sorted by capability type, then
                // by capability name (using the first entry in Many cases).
                "capabilities": [
                    // Will be merged with "bar"
                    { "protocol": ["foo"] },
                    { "protocol": "bar" },
                    // Will not be merged, but will be sorted before "bar"
                    { "protocol": "arg", "path": "/arg" },
                    // Will have list of names sorted
                    { "service": ["b", "a"] },
                    // Will have list of names sorted
                    { "event_stream": ["b", "a"] },
                    { "runner": "myrunner" },
                    // The following two will *not* be merged, because they have a `path`.
                    { "runner": "mypathrunner1", "path": "/foo" },
                    { "runner": "mypathrunner2", "path": "/foo" },
                ],
                // Same rules as for "capabilities".
                "offer": [
                    // Will be sorted after "bar"
                    { "protocol": "baz", "from": "#a_child", "to": "#c_child"  },
                    // The following two entries will be merged
                    { "protocol": ["foo"], "from": "#a_child", "to": "#b_child"  },
                    { "protocol": "bar", "from": "#a_child", "to": "#b_child"  },
                    // Will have list of names sorted
                    { "service": ["b", "a"], "from": "#a_child", "to": "#b_child"  },
                    // Will have list of names sorted
                    {
                        "event_stream": ["b", "a"],
                        "from": "#a_child",
                        "to": "#b_child",
                        "scope": ["#b", "#c", "#a"]  // Also gets sorted
                    },
                    { "runner": [ "myrunner", "a" ], "from": "#a_child", "to": "#b_child"  },
                    { "runner": [ "b" ], "from": "#a_child", "to": "#b_child"  },
                    { "directory": [ "b" ], "from": "#a_child", "to": "#b_child"  },
                ],
                "expose": [
                    { "protocol": ["foo"], "from": "#a_child" },
                    { "protocol": "bar", "from": "#a_child" },  // Will appear before protocol: foo
                    // Will have list of names sorted
                    { "service": ["b", "a"], "from": "#a_child" },
                    // Will have list of names sorted
                    {
                        "event_stream": ["b", "a"],
                        "from": "#a_child",
                        "scope": ["#b", "#c", "#a"]  // Also gets sorted
                    },
                    { "runner": [ "myrunner", "a" ], "from": "#a_child" },
                    { "runner": [ "b" ], "from": "#a_child" },
                    { "directory": [ "b" ], "from": "#a_child" },
                ],
                "use": [
                    // Will be sorted after "baz"
                    { "protocol": ["zazzle"], "path": "/zazbaz" },
                    // These will be merged
                    { "protocol": ["foo"] },
                    { "protocol": "bar" },
                    // Will have list of names sorted
                    { "service": ["b", "a"] },
                    // Will have list of names sorted
                    { "event_stream": ["b", "a"], "scope": ["#b", "#a"] },
                ],
            })
            .to_string(),
        );
        some.canonicalize();

        assert_json_eq!(
            some,
            document_context(&json!({
                "children": [
                    { "name": "a_child", "url": "http://foo/a" },
                    { "name": "b_child", "url": "http://foo/b" },
                ],
                "collections": [
                    { "name": "a_coll", "durability": "transient" },
                    { "name": "b_coll", "durability": "transient" },
                ],
                "environments": [
                    { "name": "a_env" },
                    { "name": "b_env" },
                ],
                "capabilities": [
                    { "event_stream": ["a", "b"] },
                    { "protocol": "arg", "path": "/arg" },
                    { "protocol": ["bar", "foo"] },
                    { "runner": "mypathrunner1", "path": "/foo" },
                    { "runner": "mypathrunner2", "path": "/foo" },
                    { "runner": "myrunner" },
                    { "service": ["a", "b"] },
                ],
                "use": [
                    { "event_stream": ["a", "b"], "scope": ["#a", "#b"] },
                    { "protocol": ["bar", "foo"] },
                    { "protocol": "zazzle", "path": "/zazbaz" },
                    { "service": ["a", "b"] },
                ],
                "offer": [
                    { "directory": "b", "from": "#a_child", "to": "#b_child" },
                    {
                        "event_stream": ["a", "b"],
                        "from": "#a_child",
                        "to": "#b_child",
                        "scope": ["#a", "#b", "#c"],
                    },
                    { "protocol": ["bar", "foo"], "from": "#a_child", "to": "#b_child" },
                    { "protocol": "baz", "from": "#a_child", "to": "#c_child"  },
                    { "runner": [ "a", "b", "myrunner" ], "from": "#a_child", "to": "#b_child" },
                    { "service": ["a", "b"], "from": "#a_child", "to": "#b_child" },
                ],
                "expose": [
                    { "directory": "b", "from": "#a_child" },
                    {
                        "event_stream": ["a", "b"],
                        "from": "#a_child",
                        "scope": ["#a", "#b", "#c"],
                    },
                    { "protocol": ["bar", "foo"], "from": "#a_child" },
                    { "runner": [ "a", "b", "myrunner" ], "from": "#a_child" },
                    { "service": ["a", "b"], "from": "#a_child" },
                ],
            }).to_string())
        )
    }

    #[test]
    fn deny_unknown_config_type_fields() {
        let contents =
            json!({ "config": { "foo": { "type": "bool", "unknown": "should error" } } });
        let file_arc = Arc::from(std::path::Path::new("test.cml"));
        parse_and_hydrate(file_arc, &contents.to_string())
            .expect_err("must reject unknown config field attributes");
    }

    #[test]
    fn deny_unknown_config_nested_type_fields() {
        let input = json!({
            "config": {
                "foo": {
                    "type": "vector",
                    "max_count": 10,
                    "element": {
                        "type": "bool",
                        "unknown": "should error"
                    },

                }
            }
        });

        let file_arc = Arc::from(std::path::Path::new("test.cml"));
        parse_and_hydrate(file_arc, &input.to_string())
            .expect_err("must reject unknown config field attributes");
    }

    #[test]
    fn test_merge_from_program() {
        let mut some =
            document_context(&json!({ "program": { "binary": "bin/hello_world" } }).to_string());
        let other = document_context(&json!({ "program": { "runner": "elf" } }).to_string());
        some.merge_from(other, &Path::new("some/path")).unwrap();
        let expected = document_context(
            &json!({ "program": { "binary": "bin/hello_world", "runner": "elf" } }).to_string(),
        );
        assert_eq!(some.program, expected.program);
    }

    #[test]
    fn test_merge_from_program_without_runner() {
        let mut some = document_context(
            &json!({ "program": { "binary": "bin/hello_world", "runner": "elf" } }).to_string(),
        );
        // https://fxbug.dev/42160240: merging with a document that doesn't have a runner doesn't override the
        // runner that we already have assigned.
        let other = document_context(&json!({ "program": {} }).to_string());
        some.merge_from(other, &Path::new("some/path")).unwrap();
        let expected = document_context(
            &json!({ "program": { "binary": "bin/hello_world", "runner": "elf" } }).to_string(),
        );
        assert_eq!(some.program, expected.program);
    }

    #[test]
    fn test_merge_from_program_overlapping_environ() {
        // It's ok to merge `program.environ` by concatenating the arrays together.
        let mut some = document_context(&json!({ "program": { "environ": ["1"] } }).to_string());
        let other = document_context(&json!({ "program": { "environ": ["2"] } }).to_string());
        some.merge_from(other, &Path::new("some/path")).unwrap();
        let expected =
            document_context(&json!({ "program": { "environ": ["1", "2"] } }).to_string());
        assert_eq!(some.program, expected.program);
    }

    #[test]
    fn test_merge_from_program_overlapping_runner() {
        // It's ok to merge `program.runner = "elf"` with `program.runner = "elf"`.
        let mut some = document_context(
            &json!({ "program": { "binary": "bin/hello_world", "runner": "elf" } }).to_string(),
        );
        let other = document_context(&json!({ "program": { "runner": "elf" } }).to_string());
        some.merge_from(other, &Path::new("some/path")).unwrap();
        let expected = document_context(
            &json!({ "program": { "binary": "bin/hello_world", "runner": "elf" } }).to_string(),
        );
        assert_eq!(some.program, expected.program);
    }

    #[test]
    fn test_merge_from_program_error_runner() {
        let mut some = document_context(&json!({ "program": { "runner": "elf" } }).to_string());
        let other = document_context(&json!({ "program": { "runner": "fle" } }).to_string());
        assert_matches::assert_matches!(
            some.merge_from(other, &Path::new("some/path")),
            Err(Error::Merge {  err, .. })
                if err == format!("Manifest include had a conflicting `program.runner`: parent='elf', include='fle'"));
    }

    #[test]
    fn test_merge_from_program_error_binary() {
        let mut some =
            document_context(&json!({ "program": { "binary": "bin/hello_world" } }).to_string());
        let other =
            document_context(&json!({ "program": { "binary": "bin/hola_mundo" } }).to_string());
        assert_matches::assert_matches!(
            some.merge_from(other, &Path::new("some/path")),
            Err(Error::Merge {  err, .. })
                if err == format!("Manifest include 'some/path' had a conflicting value for field \"program.binary\""));
    }

    #[test]
    fn test_merge_from_program_error_args() {
        let mut some =
            document_context(&json!({ "program": { "args": ["a".to_owned()] } }).to_string());
        let other =
            document_context(&json!({ "program": { "args": ["b".to_owned()] } }).to_string());
        assert_matches::assert_matches!(
            some.merge_from(other, &Path::new("some/path")),
            Err(Error::Merge {  err, .. })
                if err == format!("Conflicting array values for field \"program.args\""));
    }

    #[test_case(
        document_context(&json!({ "facets": { "my.key": "my.value" } }).to_string()),
        document_context(&json!({ "facets": { "other.key": "other.value" } }).to_string()),
        document_context(&json!({ "facets": { "my.key": "my.value",  "other.key": "other.value" } }).to_string())
        ; "two separate keys"
    )]
    #[test_case(
        document_context(&json!({ "facets": { "my.key": "my.value" } }).to_string()),
        document_context(&json!({ "facets": {} }).to_string()),
        document_context(&json!({ "facets": { "my.key": "my.value" } }).to_string())
        ; "empty other facet"
    )]
    #[test_case(
        document_context(&json!({ "facets": {} }).to_string()),
        document_context(&json!({ "facets": { "other.key": "other.value" } }).to_string()),
        document_context(&json!({ "facets": { "other.key": "other.value" } }).to_string())
        ; "empty my facet"
    )]
    #[test_case(
        document_context(&json!({ "facets": { "key": { "type": "some_type" } } }).to_string()),
        document_context(&json!({ "facets": { "key": { "runner": "some_runner"} } }).to_string()),
        document_context(&json!({ "facets": { "key": { "type": "some_type", "runner": "some_runner" } } }).to_string())
        ; "nested facet key"
    )]
    #[test_case(
        document_context(&json!({ "facets": { "key": { "type": "some_type", "nested_key": { "type": "new type" }}}}).to_string()),
        document_context(&json!({ "facets": { "key": { "nested_key": { "runner": "some_runner" }} } }).to_string()),
        document_context(&json!({ "facets": { "key": { "type": "some_type", "nested_key": { "runner": "some_runner", "type": "new type" }}}}).to_string())
        ; "double nested facet key"
    )]
    #[test_case(
        document_context(&json!({ "facets": { "key": { "array_key": ["value_1", "value_2"] } } }).to_string()),
        document_context(&json!({ "facets": { "key": { "array_key": ["value_3", "value_4"] } } }).to_string()),
        document_context(&json!({ "facets": { "key": { "array_key": ["value_1", "value_2", "value_3", "value_4"] } } }).to_string())
        ; "merge array values" // failing
    )]
    fn test_merge_from_facets(
        mut my: DocumentContext,
        other: DocumentContext,
        expected: DocumentContext,
    ) {
        my.merge_from(other, &Path::new("some/path")).unwrap();
        assert_eq!(my.facets, expected.facets);
    }

    #[test_case(
        document_context(&json!({ "facets": { "key": "my.value" }}).to_string()),
        document_context(&json!({ "facets": { "key": "other.value" }}).to_string()),
        "facets.key"
        ; "conflict first level keys" // failing
    )]
    #[test_case(
        document_context(&json!({ "facets": { "key":  {"type": "cts" }}}).to_string()),
        document_context(&json!({ "facets": { "key":  {"type": "system" }}}).to_string()),
        "facets.key.type"
        ; "conflict second level keys"
    )]
    #[test_case(
        document_context(&json!({ "facets": { "key":  {"type": {"key": "value" }}}}).to_string()),
        document_context(&json!({ "facets": { "key":  {"type": "system" }}}).to_string()),
        "facets.key.type"
        ; "incompatible self nested type"
    )]
    #[test_case(
        document_context(&json!({ "facets": { "key":  {"type": "system" }}}).to_string()),
        document_context(&json!({ "facets": { "key":  {"type":  {"key": "value" }}}}).to_string()),
        "facets.key.type"
        ; "incompatible other nested type"
    )]
    #[test_case(
        document_context(&json!({ "facets": { "key":  {"type": {"key": "my.value" }}}}).to_string()),
        document_context(&json!({ "facets": { "key":  {"type":  {"key": "some.value" }}}}).to_string()),
        "facets.key.type.key"
        ; "conflict third level keys"
    )]
    #[test_case(
        document_context(&json!({ "facets": { "key":  {"type": [ "value_1" ]}}}).to_string()),
        document_context(&json!({ "facets": { "key":  {"type":  "value_2" }}}).to_string()),
        "facets.key.type"
        ; "incompatible keys"
    )]
    fn test_merge_from_facet_error(mut my: DocumentContext, other: DocumentContext, field: &str) {
        assert_matches::assert_matches!(
            my.merge_from(other, &path::Path::new("some/path")),
            Err(Error::Merge {  err, .. })
                if err == format!("Manifest include 'some/path' had a conflicting value for field \"{}\"", field)
        );
    }

    #[test_case("protocol")]
    #[test_case("service")]
    #[test_case("event_stream")]
    fn test_merge_from_duplicate_use_array(typename: &str) {
        let mut my = document_context(&json!({ "use": [{ typename: "a" }]}).to_string());
        let other = document_context(
            &json!({ "use": [
                { typename: ["a", "b"], "availability": "optional"}
            ]})
            .to_string(),
        );
        let result = document_context(
            &json!({ "use": [
                { typename: "a" },
                { typename: "b", "availability": "optional" },
            ]})
            .to_string(),
        );

        my.merge_from(other, &path::Path::new("some/path")).unwrap();
        assert_eq!(my, result);
    }

    #[test_case("directory")]
    #[test_case("storage")]
    fn test_merge_from_duplicate_use_noarray(typename: &str) {
        let mut my =
            document_context(&json!({ "use": [{ typename: "a", "path": "/a"}]}).to_string());
        let other = document_context(
            &json!({ "use": [
                { typename: "a", "path": "/a", "availability": "optional" },
                { typename: "b", "path": "/b", "availability": "optional" },
            ]})
            .to_string(),
        );
        let result = document_context(
            &json!({ "use": [
                { typename: "a", "path": "/a" },
                { typename: "b", "path": "/b", "availability": "optional" },
            ]})
            .to_string(),
        );
        my.merge_from(other, &path::Path::new("some/path")).unwrap();
        assert_eq!(my, result);
    }

    #[test_case("protocol")]
    #[test_case("service")]
    #[test_case("event_stream")]
    fn test_merge_from_duplicate_capabilities_array(typename: &str) {
        let mut my = document_context(&json!({ "capabilities": [{ typename: "a" }]}).to_string());
        let other =
            document_context(&json!({ "capabilities": [ { typename: ["a", "b"] } ]}).to_string());
        let result = document_context(
            &json!({ "capabilities": [ { typename: "a" }, { typename: "b" } ]}).to_string(),
        );

        my.merge_from(other, &path::Path::new("some/path")).unwrap();
        assert_eq!(my, result);
    }

    #[test_case("directory")]
    #[test_case("storage")]
    #[test_case("runner")]
    #[test_case("resolver")]
    fn test_merge_from_duplicate_capabilities_noarray(typename: &str) {
        let mut my = document_context(
            &json!({ "capabilities": [{ typename: "a", "path": "/a"}]}).to_string(),
        );
        let other = document_context(
            &json!({ "capabilities": [
                { typename: "a", "path": "/a" },
                { typename: "b", "path": "/b" },
            ]})
            .to_string(),
        );
        let result = document_context(
            &json!({ "capabilities": [
                { typename: "a", "path": "/a" },
                { typename: "b", "path": "/b" },
            ]})
            .to_string(),
        );
        my.merge_from(other, &path::Path::new("some/path")).unwrap();
        assert_eq!(my, result);
    }

    #[test]
    fn test_merge_with_empty_names() {
        // This document is an error because there is no capability name.
        let mut my = document_context(&json!({ "capabilities": [{ "path": "/a"}]}).to_string());

        let other = document_context(
            &json!({ "capabilities": [
                { "directory": "a", "path": "/a" },
                { "directory": "b", "path": "/b" },
            ]})
            .to_string(),
        );
        my.merge_from(other, &path::Path::new("some/path")).unwrap_err();
    }

    #[test_case("protocol")]
    #[test_case("service")]
    #[test_case("event_stream")]
    #[test_case("directory")]
    #[test_case("storage")]
    #[test_case("runner")]
    #[test_case("resolver")]
    fn test_merge_from_duplicate_offers(typename: &str) {
        let mut my = document_context(
            &json!({ "offer": [{ typename: "a", "from": "self", "to": "#c" }]}).to_string(),
        );
        let other = document_context(
            &json!({ "offer": [
                { typename: ["a", "b"], "from": "self", "to": "#c", "availability": "optional" }
            ]})
            .to_string(),
        );
        let result = document_context(
            &json!({ "offer": [
                { typename: "a", "from": "self", "to": "#c" },
                { typename: "b", "from": "self", "to": "#c", "availability": "optional" },
            ]})
            .to_string(),
        );

        my.merge_from(other, &path::Path::new("some/path")).unwrap();
        assert_eq!(my, result);
    }

    #[test_case("protocol")]
    #[test_case("service")]
    #[test_case("event_stream")]
    #[test_case("directory")]
    #[test_case("runner")]
    #[test_case("resolver")]
    fn test_merge_from_duplicate_exposes(typename: &str) {
        let mut my =
            document_context(&json!({ "expose": [{ typename: "a", "from": "self" }]}).to_string());
        let other = document_context(
            &json!({ "expose": [
                { typename: ["a", "b"], "from": "self" }
            ]})
            .to_string(),
        );
        let result = document_context(
            &json!({ "expose": [
                { typename: "a", "from": "self" },
                { typename: "b", "from": "self" },
            ]})
            .to_string(),
        );

        my.merge_from(other, &path::Path::new("some/path")).unwrap();
        assert_eq!(my, result);
    }

    #[test_case(
        document_context(&json!({ "use": [
            { "protocol": "a", "availability": "required" },
            { "protocol": "b", "availability": "optional" },
            { "protocol": "c", "availability": "transitional" },
            { "protocol": "d", "availability": "same_as_target" },
        ]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["a"], "availability": "required" },
            { "protocol": ["b"], "availability": "optional" },
            { "protocol": ["c"], "availability": "transitional" },
            { "protocol": ["d"], "availability": "same_as_target" },
        ]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": "a", "availability": "required" },
            { "protocol": "b", "availability": "optional" },
            { "protocol": "c", "availability": "transitional" },
            { "protocol": "d", "availability": "same_as_target" },
        ]}).to_string())
        ; "merge both same"
    )]
    #[test_case(
        document_context(&json!({ "use": [
            { "protocol": "a", "availability": "optional" },
            { "protocol": "b", "availability": "transitional" },
            { "protocol": "c", "availability": "transitional" },
        ]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["a", "x"], "availability": "required" },
            { "protocol": ["b", "y"], "availability": "optional" },
            { "protocol": ["c", "z"], "availability": "required" },
        ]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["a", "x"], "availability": "required" },
            { "protocol": ["b", "y"], "availability": "optional" },
            { "protocol": ["c", "z"], "availability": "required" },
        ]}).to_string())
        ; "merge with upgrade"
    )]
    #[test_case(
        document_context(&json!({ "use": [
            { "protocol": "a", "availability": "required" },
            { "protocol": "b", "availability": "optional" },
            { "protocol": "c", "availability": "required" },
        ]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["a", "x"], "availability": "optional" },
            { "protocol": ["b", "y"], "availability": "transitional" },
            { "protocol": ["c", "z"], "availability": "transitional" },
        ]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": "a", "availability": "required" },
            { "protocol": "b", "availability": "optional" },
            { "protocol": "c", "availability": "required" },
            { "protocol": "x", "availability": "optional" },
            { "protocol": "y", "availability": "transitional" },
            { "protocol": "z", "availability": "transitional" },
        ]}).to_string())
        ; "merge with downgrade"
    )]
    #[test_case(
        document_context(&json!({ "use": [
            { "protocol": "a", "availability": "optional" },
            { "protocol": "b", "availability": "transitional" },
            { "protocol": "c", "availability": "transitional" },
        ]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["a", "x"], "availability": "same_as_target" },
            { "protocol": ["b", "y"], "availability": "same_as_target" },
            { "protocol": ["c", "z"], "availability": "same_as_target" },
        ]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": "a", "availability": "optional" },
            { "protocol": "b", "availability": "transitional" },
            { "protocol": "c", "availability": "transitional" },
            { "protocol": ["a", "x"], "availability": "same_as_target" },
            { "protocol": ["b", "y"], "availability": "same_as_target" },
            { "protocol": ["c", "z"], "availability": "same_as_target" },
        ]}).to_string())
        ; "merge with no replacement"
    )]
    #[test_case(
        document_context(&json!({ "use": [
            { "protocol": ["a", "b", "c"], "availability": "optional" },
            { "protocol": "d", "availability": "same_as_target" },
            { "protocol": ["e", "f"] },
        ]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["c", "e", "g"] },
            { "protocol": ["d", "h"] },
            { "protocol": ["f", "i"], "availability": "transitional" },
        ]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["a", "b"], "availability": "optional" },
            { "protocol": "d", "availability": "same_as_target" },
            { "protocol": ["e", "f"] },
            { "protocol": ["c", "g"] },
            { "protocol": ["d", "h"] },
            { "protocol": "i", "availability": "transitional" },
        ]}).to_string())
        ; "merge multiple"
    )]

    fn test_merge_from_duplicate_capability_availability(
        mut my: DocumentContext,
        other: DocumentContext,
        result: DocumentContext,
    ) {
        my.merge_from(other, &path::Path::new("some/path")).unwrap();
        assert_eq!(my, result);
    }

    #[test_case(
        document_context(&json!({ "use": [{ "protocol": ["a", "b"] }]}).to_string()),
        document_context(&json!({ "use": [{ "protocol": ["c", "d"] }]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["a", "b"] }, { "protocol": ["c", "d"] }
        ]}).to_string())
        ; "merge capabilities with disjoint sets"
    )]
    #[test_case(
        document_context(&json!({ "use": [
            { "protocol": ["a"] },
            { "protocol": "b" },
        ]}).to_string()),
        document_context(&json!({ "use": [{ "protocol": ["a", "b"] }]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["a"] }, { "protocol": "b" },
        ]}).to_string())
        ; "merge capabilities with equal set"
    )]
    #[test_case(
        document_context(&json!({ "use": [
            { "protocol": ["a", "b"] },
            { "protocol": "c" },
        ]}).to_string()),
        document_context(&json!({ "use": [{ "protocol": ["a", "b"] }]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["a", "b"] }, { "protocol": "c" },
        ]}).to_string())
        ; "merge capabilities with subset"
    )]
    #[test_case(
        document_context(&json!({ "use": [
            { "protocol": ["a", "b"] },
        ]}).to_string()),
        document_context(&json!({ "use": [{ "protocol": ["a", "b", "c"] }]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["a", "b"] },
            { "protocol": "c" },
        ]}).to_string())
        ; "merge capabilities with superset"
    )]
    #[test_case(
        document_context(&json!({ "use": [
            { "protocol": ["a", "b"] },
        ]}).to_string()),
        document_context(&json!({ "use": [{ "protocol": ["b", "c", "d"] }]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["a", "b"] }, { "protocol": ["c", "d"] }
        ]}).to_string())
        ; "merge capabilities with intersection"
    )]
    #[test_case(
        document_context(&json!({ "use": [{ "protocol": ["a", "b"] }]}).to_string()),
        document_context(&json!({ "use": [
            { "protocol": ["c", "b", "d"] },
            { "protocol": ["e", "d"] },
        ]}).to_string()),
        document_context(&json!({ "use": [
            {"protocol": ["a", "b"] },
            {"protocol": ["c", "d"] },
            {"protocol": "e" }]}).to_string())
        ; "merge capabilities from multiple arrays"
    )]
    #[test_case(
        document_context(&json!({ "use": [{ "protocol": "foo.bar.Baz", "from": "self"}]}).to_string()),
        document_context(&json!({ "use": [{ "service": "foo.bar.Baz", "from": "self"}]}).to_string()),
        document_context(&json!({ "use": [
            {"protocol": "foo.bar.Baz", "from": "self"},
            {"service": "foo.bar.Baz", "from": "self"}]}).to_string())
        ; "merge capabilities, types don't match"
    )]
    #[test_case(
        document_context(&json!({ "use": [{ "protocol": "foo.bar.Baz", "from": "self"}]}).to_string()),
        document_context(&json!({ "use": [{ "protocol": "foo.bar.Baz" }]}).to_string()),
        document_context(&json!({ "use": [
            {"protocol": "foo.bar.Baz", "from": "self"},
            {"protocol": "foo.bar.Baz"}]}).to_string())
        ; "merge capabilities, fields don't match"
    )]

    fn test_merge_from_duplicate_capability(
        mut my: DocumentContext,
        other: DocumentContext,
        result: DocumentContext,
    ) {
        my.merge_from(other, &path::Path::new("some/path")).unwrap();
        assert_eq!(my, result);
    }
}
