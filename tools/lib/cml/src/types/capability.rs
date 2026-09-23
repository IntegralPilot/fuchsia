// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{
    AnyRef, AsClauseContext, CanonicalizeContext, ConfigNestedValueType, ConfigType, Error,
};

use crate::one_or_many::{OneOrMany, always_one_context};
use crate::types::common::*;
use crate::types::right::{Rights, RightsClause};
pub use cm_types::{
    Availability, BorrowedName, BoundedName, DeliveryType, DependencyType, HandleType, Name,
    OnTerminate, ParseError, Path, RelativePath, StartupMode, StorageId, Url,
};
use cml_macro::Reference;
use reference_doc::ReferenceDoc;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

use std::fmt;
use std::sync::Arc;

#[derive(Deserialize, Debug, PartialEq, Clone, ReferenceDoc, Serialize, Default)]
#[serde(deny_unknown_fields)]
#[reference_doc(fields_as = "list")]
pub struct Capability {
    /// The [name](#name) for this service capability. Specifying `path` is valid
    /// only when this value is a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub service: Option<OneOrMany<Name>>,

    /// The [name](#name) for this protocol capability. Specifying `path` is valid
    /// only when this value is a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub protocol: Option<OneOrMany<Name>>,

    /// The [name](#name) for this directory capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub directory: Option<Name>,

    /// The [name](#name) for this storage capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub storage: Option<Name>,

    /// The [name](#name) for this runner capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub runner: Option<Name>,

    /// The [name](#name) for this resolver capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub resolver: Option<Name>,

    /// The [name](#name) for this event_stream capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub event_stream: Option<OneOrMany<Name>>,

    /// The [name](#name) for this dictionary capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub dictionary: Option<Name>,

    /// The [name](#name) for this configuration capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub config: Option<Name>,

    /// The path within the [outgoing directory][glossary.outgoing directory] of the component's
    /// program to source the capability.
    ///
    /// For `protocol` and `service`, defaults to `/svc/${protocol}`, otherwise required.
    ///
    /// For `protocol`, the target of the path MUST be a channel, which tends to speak
    /// the protocol matching the name of this capability.
    ///
    /// For `service`, `directory`, the target of the path MUST be a directory.
    ///
    /// For `runner`, the target of the path MUST be a channel and MUST speak
    /// the protocol `fuchsia.component.runner.ComponentRunner`.
    ///
    /// For `resolver`, the target of the path MUST be a channel and MUST speak
    /// the protocol `fuchsia.component.resolution.Resolver`.
    ///
    /// For `dictionary`, this is optional. If provided, it is a path to a
    /// `fuchsia.component.sandbox/DictionaryRouter` served by the program which should return a
    /// `fuchsia.component.sandbox/DictionaryRef`, by which the program may dynamically provide
    /// a dictionary from itself. If this is set for `dictionary`, `offer` to this dictionary
    /// is not allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Path>,

    /// (`directory` only) The maximum [directory rights][doc-directory-rights] that may be set
    /// when using this directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(json_type = "array of string")]
    pub rights: Option<Rights>,

    /// (`storage` only) The source component of an existing directory capability backing this
    /// storage capability, one of:
    /// - `parent`: The component's parent.
    /// - `self`: This component.
    /// - `#<child-name>`: A [reference](#references) to a child component
    ///     instance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<CapabilityFromRef>,

    /// (`storage` only) The [name](#name) of the directory capability backing the storage. The
    /// capability must be available from the component referenced in `from`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backing_dir: Option<Name>,

    /// (`storage` only) A subdirectory within `backing_dir` where per-component isolated storage
    /// directories are created
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdir: Option<RelativePath>,

    /// (`storage` only) The identifier used to isolated storage for a component, one of:
    /// - `static_instance_id`: The instance ID in the component ID index is used
    ///     as the key for a component's storage. Components which are not listed in
    ///     the component ID index will not be able to use this storage capability.
    /// - `static_instance_id_or_moniker`: If the component is listed in the
    ///     component ID index, the instance ID is used as the key for a component's
    ///     storage. Otherwise, the component's moniker from the storage
    ///     capability is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_id: Option<StorageId>,

    /// (`configuration` only) The type of configuration, one of:
    /// - `bool`: Boolean type.
    /// - `uint8`: Unsigned 8 bit type.
    /// - `uint16`: Unsigned 16 bit type.
    /// - `uint32`: Unsigned 32 bit type.
    /// - `uint64`: Unsigned 64 bit type.
    /// - `int8`: Signed 8 bit type.
    /// - `int16`: Signed 16 bit type.
    /// - `int32`: Signed 32 bit type.
    /// - `int64`: Signed 64 bit type.
    /// - `string`: ASCII string type.
    /// - `vector`: Vector type. See `element` for the type of the element within the vector.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    #[reference_doc(rename = "type")]
    pub config_type: Option<ConfigType>,

    /// (`configuration` only) Only supported if this configuration `type` is 'string'.
    /// This is the max size of the string.
    #[serde(rename = "max_size", skip_serializing_if = "Option::is_none")]
    #[reference_doc(rename = "max_size")]
    pub config_max_size: Option<NonZeroU32>,

    /// (`configuration` only) Only supported if this configuration `type` is 'vector'.
    /// This is the max number of elements in the vector.
    #[serde(rename = "max_count", skip_serializing_if = "Option::is_none")]
    #[reference_doc(rename = "max_count")]
    pub config_max_count: Option<NonZeroU32>,

    /// (`configuration` only) Only supported if this configuration `type` is 'vector'.
    /// This is the type of the elements in the configuration vector.
    ///
    /// Example (simple type):
    ///
    /// ```json5
    /// { type: "uint8" }
    /// ```
    ///
    /// Example (string type):
    ///
    /// ```json5
    /// {
    ///   type: "string",
    ///   max_size: 100,
    /// }
    /// ```
    #[serde(rename = "element", skip_serializing_if = "Option::is_none")]
    #[reference_doc(rename = "element", json_type = "object")]
    pub config_element_type: Option<ConfigNestedValueType>,

    /// (`configuration` only) The value of the configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,

    /// (`protocol` only) Specifies when the framework will open the protocol
    /// from this component's outgoing directory when someone requests the
    /// capability. Allowed values are:
    ///
    /// - `eager`: (default) the framework will open the capability as soon as
    ///   some consumer component requests it.
    /// - `on_readable`: the framework will open the capability when the server
    ///   endpoint pipelined in a connection request becomes readable.
    ///
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<DeliveryType>,
}

/// A reference in a `storage from`.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Reference)]
#[reference(expected = "\"parent\", \"self\", or \"#<child-name>\"")]
pub enum CapabilityFromRef {
    /// A reference to a child.
    Named(Name),
    /// A reference to the parent.
    Parent,
    /// A reference to this component.
    Self_,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextCapability {
    #[serde(skip)]
    pub origin: Arc<std::path::Path>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<ContextSpanned<OneOrMany<Name>>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ContextSpanned<OneOrMany<Name>>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<ContextSpanned<Name>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<ContextSpanned<Name>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner: Option<ContextSpanned<Name>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolver: Option<ContextSpanned<Name>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_stream: Option<ContextSpanned<OneOrMany<Name>>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub dictionary: Option<ContextSpanned<Name>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<ContextSpanned<Name>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<ContextSpanned<Path>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub rights: Option<ContextSpanned<Rights>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<ContextSpanned<CapabilityFromRef>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub backing_dir: Option<ContextSpanned<Name>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdir: Option<ContextSpanned<RelativePath>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_id: Option<ContextSpanned<StorageId>>,

    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub config_type: Option<ContextSpanned<ConfigType>>,

    #[serde(rename = "max_size", skip_serializing_if = "Option::is_none")]
    pub config_max_size: Option<ContextSpanned<NonZeroU32>>,

    #[serde(rename = "max_count", skip_serializing_if = "Option::is_none")]
    pub config_max_count: Option<ContextSpanned<NonZeroU32>>,

    #[serde(rename = "element", skip_serializing_if = "Option::is_none")]
    pub config_element_type: Option<ContextSpanned<ConfigNestedValueType>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<ContextSpanned<serde_json::Value>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<ContextSpanned<DeliveryType>>,
}

impl Default for ContextCapability {
    fn default() -> Self {
        Self {
            origin: Arc::from(std::path::Path::new("")),
            service: None,
            protocol: None,
            directory: None,
            storage: None,
            runner: None,
            resolver: None,
            event_stream: None,
            dictionary: None,
            config: None,
            path: None,
            rights: None,
            from: None,
            backing_dir: None,
            subdir: None,
            storage_id: None,
            config_type: None,
            config_max_size: None,
            config_max_count: None,
            config_element_type: None,
            value: None,
            delivery: None,
        }
    }
}

impl CanonicalizeContext for ContextCapability {
    fn canonicalize_context(&mut self) {
        // Sort the names of the capabilities. Only capabilities with OneOrMany values are included here.
        if let Some(service) = &mut self.service {
            service.value.canonicalize_context()
        } else if let Some(protocol) = &mut self.protocol {
            protocol.value.canonicalize_context()
        } else if let Some(event_stream) = &mut self.event_stream {
            event_stream.value.canonicalize_context()
        }
    }
}

impl RightsClause for ContextCapability {
    fn rights(&self) -> Option<&Rights> {
        self.rights.as_ref().map(|r| &r.value)
    }
}

impl ContextCapabilityClause for ContextCapability {
    fn service(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        option_one_or_many_as_ref_context(&self.service)
    }
    fn protocol(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        option_one_or_many_as_ref_context(&self.protocol)
    }
    fn directory(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.directory.as_ref().map(|s| ContextSpanned {
            value: OneOrMany::One((s.value).as_ref()),
            origin: s.origin.clone(),
        })
    }
    fn storage(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.storage.as_ref().map(|s| ContextSpanned {
            value: OneOrMany::One((s.value).as_ref()),
            origin: s.origin.clone(),
        })
    }
    fn runner(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.runner.as_ref().map(|s| ContextSpanned {
            value: OneOrMany::One((s.value).as_ref()),
            origin: s.origin.clone(),
        })
    }
    fn resolver(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.resolver.as_ref().map(|s| ContextSpanned {
            value: OneOrMany::One((s.value).as_ref()),
            origin: s.origin.clone(),
        })
    }
    fn event_stream(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        option_one_or_many_as_ref_context(&self.event_stream)
    }
    fn dictionary(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.dictionary.as_ref().map(|s| ContextSpanned {
            value: OneOrMany::One((s.value).as_ref()),
            origin: s.origin.clone(),
        })
    }
    fn config(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.config.as_ref().map(|s| ContextSpanned {
            value: OneOrMany::One((s.value).as_ref()),
            origin: s.origin.clone(),
        })
    }

    fn decl_type(&self) -> &'static str {
        "capability"
    }
    fn supported(&self) -> &[&'static str] {
        &[
            "service",
            "protocol",
            "directory",
            "storage",
            "event_stream",
            "runner",
            "resolver",
            "config",
            "dictionary",
        ]
    }
    fn are_many_names_allowed(&self) -> bool {
        ["service", "protocol", "event_stream"].contains(&self.capability_type(None).unwrap())
    }

    fn set_service(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.service = o;
    }
    fn set_protocol(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.protocol = o;
    }
    fn set_directory(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.directory = always_one_context(o);
    }
    fn set_storage(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.storage = always_one_context(o);
    }
    fn set_runner(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.runner = always_one_context(o);
    }
    fn set_resolver(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.resolver = always_one_context(o);
    }
    fn set_event_stream(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.event_stream = o;
    }
    fn set_dictionary(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.dictionary = always_one_context(o);
    }
    fn set_config(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.config = always_one_context(o);
    }

    /// Returns the origin of this capability.
    fn origin(&self) -> &Arc<std::path::Path> {
        &self.origin
    }

    fn availability(&self) -> Option<ContextSpanned<Availability>> {
        None
    }
    fn set_availability(&mut self, _a: Option<ContextSpanned<Availability>>) {}
}

impl PartialEq for ContextCapability {
    fn eq(&self, other: &Self) -> bool {
        macro_rules! cmp {
            ($field:ident) => {
                match (&self.$field, &other.$field) {
                    (Some(a), Some(b)) => a.value == b.value,
                    (None, None) => true,
                    _ => false,
                }
            };
        }

        cmp!(service)
            && cmp!(protocol)
            && cmp!(directory)
            && cmp!(storage)
            && cmp!(runner)
            && cmp!(resolver)
            && cmp!(dictionary)
            && cmp!(config)
            && cmp!(path)
            && cmp!(rights)
            && cmp!(from)
            && cmp!(event_stream)
            && cmp!(backing_dir)
            && cmp!(subdir)
            && cmp!(storage_id)
            && cmp!(config_type)
            && cmp!(config_max_size)
            && cmp!(config_max_count)
            && cmp!(config_element_type)
            && cmp!(value)
            && cmp!(delivery)
    }
}

impl Eq for ContextCapability {}

impl ContextPathClause for ContextCapability {
    fn path(&self) -> Option<&ContextSpanned<Path>> {
        self.path.as_ref()
    }
}

impl AsClauseContext for ContextCapability {
    fn r#as(&self) -> Option<ContextSpanned<&BorrowedName>> {
        None
    }
}

impl Hydrate for Capability {
    type Output = ContextCapability;

    fn hydrate(self, file: &Arc<std::path::Path>) -> Result<Self::Output, Error> {
        Ok(ContextCapability {
            origin: file.clone(),
            service: hydrate_opt_simple(self.service, file),
            protocol: hydrate_opt_simple(self.protocol, file),
            directory: hydrate_opt_simple(self.directory, file),
            storage: hydrate_opt_simple(self.storage, file),
            runner: hydrate_opt_simple(self.runner, file),
            resolver: hydrate_opt_simple(self.resolver, file),
            dictionary: hydrate_opt_simple(self.dictionary, file),
            config: hydrate_opt_simple(self.config, file),
            path: hydrate_opt_simple(self.path, file),
            rights: hydrate_opt_simple(self.rights, file),
            from: hydrate_opt_simple(self.from, file),
            event_stream: hydrate_opt_simple(self.event_stream, file),
            backing_dir: hydrate_opt_simple(self.backing_dir, file),
            subdir: hydrate_opt_simple(self.subdir, file),
            storage_id: hydrate_opt_simple(self.storage_id, file),
            config_type: hydrate_opt_simple(self.config_type, file),
            config_max_size: hydrate_opt_simple(self.config_max_size, file),
            config_max_count: hydrate_opt_simple(self.config_max_count, file),
            config_element_type: hydrate_opt_simple(self.config_element_type, file),
            value: hydrate_opt_simple(self.value, file),
            delivery: hydrate_opt_simple(self.delivery, file),
        })
    }
}

/// Converts Capability -> CS ContextCapability
pub fn span_capability(cap: Capability) -> ContextSpanned<ContextCapability> {
    let context_cap = ContextCapability {
        origin: Arc::from(std::path::Path::new("programmatic_manifest.cml")),
        service: cap.service.map(synthetic_span),
        protocol: cap.protocol.map(synthetic_span),
        directory: cap.directory.map(synthetic_span),
        storage: cap.storage.map(synthetic_span),
        runner: cap.runner.map(synthetic_span),
        resolver: cap.resolver.map(synthetic_span),
        event_stream: cap.event_stream.map(synthetic_span),
        dictionary: cap.dictionary.map(synthetic_span),
        config: cap.config.map(synthetic_span),
        path: cap.path.map(synthetic_span),
        rights: cap.rights.map(synthetic_span),
        from: cap.from.map(synthetic_span),
        backing_dir: cap.backing_dir.map(synthetic_span),
        subdir: cap.subdir.map(synthetic_span),
        storage_id: cap.storage_id.map(synthetic_span),
        config_type: cap.config_type.map(synthetic_span),
        config_max_size: cap.config_max_size.map(synthetic_span),
        config_max_count: cap.config_max_count.map(synthetic_span),
        config_element_type: cap.config_element_type.map(synthetic_span),
        value: cap.value.map(synthetic_span),
        delivery: cap.delivery.map(synthetic_span),
    };

    synthetic_span(context_cap)
}
