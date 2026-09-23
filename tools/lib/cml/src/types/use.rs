// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::common::*;
use crate::{
    AnyRef, CanonicalizeContext, ConfigNestedValueType, ConfigType, DictionaryRef, Error,
    EventScope, FromClauseContext,
};

use crate::one_or_many::{OneOrMany, always_one_context};
use crate::types::right::{Rights, RightsClause};
pub use cm_types::{
    Availability, BorrowedName, DependencyType, HandleType, Name, OnTerminate, ParseError, Path,
    RelativePath, StartupMode, Url,
};
use cml_macro::Reference;
use reference_doc::ReferenceDoc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::num::NonZeroU32;

use std::fmt;
use std::sync::Arc;

/// A reference in a `use from`.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Reference)]
#[reference(
    expected = "\"parent\", \"framework\", \"debug\", \"self\", \"#<capability-name>\", \"#<child-name>\", \"#<collection-name>\", dictionary path, or none"
)]
pub enum UseFromRef {
    /// A reference to the parent.
    Parent,
    /// A reference to the framework.
    Framework,
    /// A reference to debug.
    Debug,
    /// A reference to a child, collection, or a capability declared on self.
    ///
    /// A reference to a capability must be one of the following:
    /// - A dictionary capability.
    /// - A protocol that references a storage capability declared in the same component,
    ///   which will cause the framework to host a fuchsia.sys2.StorageAdmin protocol for the
    ///   component.
    ///
    /// A reference to a collection must be a service capability.
    ///
    /// This cannot be used to directly access capabilities that a component itself declares.
    Named(Name),
    /// A reference to this component.
    Self_,
    /// A reference to a dictionary.
    Dictionary(DictionaryRef),
}

/// Example:
///
/// ```json5
/// use: [
///     {
///         protocol: [
///             "fuchsia.ui.scenic.Scenic",
///             "fuchsia.accessibility.Manager",
///         ]
///     },
///     {
///         directory: "themes",
///         path: "/data/themes",
///         rights: [ "r*" ],
///     },
///     {
///         storage: "persistent",
///         path: "/data",
///     },
///     {
///         event_stream: [
///             "started",
///             "stopped",
///         ],
///         from: "framework",
///     },
///     {
///         runner: "own_test_runner".
///         from: "#test_runner",
///     },
/// ],
/// ```
#[derive(Deserialize, Debug, Default, PartialEq, Clone, ReferenceDoc, Serialize)]
#[serde(deny_unknown_fields)]
#[reference_doc(fields_as = "list", top_level_doc_after_fields)]
pub struct Use {
    /// When using a service capability, the [name](#name) of a [service capability][doc-service].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub service: Option<OneOrMany<Name>>,

    /// When using a protocol capability, the [name](#name) of a [protocol capability][doc-protocol].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub protocol: Option<OneOrMany<Name>>,

    /// When using a directory capability, the [name](#name) of a [directory capability][doc-directory].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub directory: Option<Name>,

    /// When using a storage capability, the [name](#name) of a [storage capability][doc-storage].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub storage: Option<Name>,

    /// When using an event stream capability, the [name](#name) of an [event stream capability][doc-event].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub event_stream: Option<OneOrMany<Name>>,

    /// When using a runner capability, the [name](#name) of a [runner capability][doc-runners].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub runner: Option<Name>,

    /// When using a configuration capability, the [name](#name) of a [configuration capability][doc-configuration].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub config: Option<Name>,

    /// When using a dictionary capability, the [name](#name) of a [dictionary capability][doc-dictionary].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(skip = true)]
    pub dictionary: Option<OneOrMany<Name>>,

    /// The source of the capability. Defaults to `parent`.  One of:
    /// - `parent`: The component's parent.
    /// - `debug`: One of [`debug_capabilities`][fidl-environment-decl] in the
    ///     environment assigned to this component.
    /// - `framework`: The Component Framework runtime.
    /// - `self`: This component.
    /// - `#<capability-name>`: The name of another capability from which the
    ///     requested capability is derived.
    /// - `#<child-name>`: A [reference](#references) to a child component
    ///     instance.
    ///
    /// [fidl-environment-decl]: /reference/fidl/fuchsia.component.decl#Environment
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<UseFromRef>,

    /// The path at which to install the capability in the component's namespace. For protocols,
    /// defaults to `/svc/${protocol}`.  Required for `directory` and `storage`. This property is
    /// disallowed for declarations with arrays of capability names and for runner capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Path>,

    /// A processargs ordinal (aka. "numbered handle") over which a channel to this protocol will
    /// be delivered to the component's processargs.
    ///
    // TODO: We could support strings like "PA_*", but it's not clear that's necessary since usage
    // of this feature is expected to be limited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numbered_handle: Option<HandleType>,

    /// (`directory` only) the maximum [directory rights][doc-directory-rights] to apply to
    /// the directory in the component's namespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reference_doc(json_type = "array of string")]
    pub rights: Option<Rights>,

    /// (`directory` only) A subdirectory within the directory capability to provide in the
    /// component's namespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdir: Option<RelativePath>,

    /// (`event_stream` only) When defined the event stream will contain events about only the
    /// components defined in the scope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<OneOrMany<EventScope>>,

    /// (`event_stream` only) Capability requested event streams require specifying a filter
    /// referring to the protocol to which the events in the event stream apply. The content of the
    /// filter will be an object mapping from "name" to the "protocol name".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<Map<String, Value>>,

    /// The type of dependency between the source and
    /// this component, one of:
    /// - `strong`: a strong dependency, which is used to determine shutdown
    ///     ordering. Component manager is guaranteed to stop the target before the
    ///     source. This is the default.
    /// - `weak`: a weak dependency, which is ignored during shutdown. When component manager
    ///     stops the parent realm, the source may stop before the clients. Clients of weak
    ///     dependencies must be able to handle these dependencies becoming unavailable.
    /// This property is disallowed for runner capabilities, which are always a `strong` dependency.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency: Option<DependencyType>,

    /// The expectations around this capability's availability. One
    /// of:
    /// - `required` (default): a required dependency, the component is unable to perform its
    ///     work without this capability.
    /// - `optional`: an optional dependency, the component will be able to function without this
    ///     capability (although if the capability is unavailable some functionality may be
    ///     disabled).
    /// - `transitional`: the source may omit the route completely without even having to route
    ///     from `void`. Used for soft transitions that introduce new capabilities.
    /// This property is disallowed for runner capabilities, which are always `required`.
    ///
    /// For more information, see the
    /// [availability](/docs/concepts/components/v2/capabilities/availability.md) documentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<Availability>,

    /// (`config` only) The configuration key in the component's `config` block that this capability
    /// will set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<Name>,

    /// (`config` only) The type of configuration, one of:
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
    /// - `vector`: Vector type. See `element` for the type of the element within the vector
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

    /// (`configuration` only) The default value of this configuration.
    /// Default values are used if the capability is optional and routed from `void`.
    /// This is only supported if `availability` is not `required``.
    #[serde(rename = "default", skip_serializing_if = "Option::is_none")]
    #[reference_doc(rename = "default")]
    pub config_default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextUse {
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
    pub event_stream: Option<ContextSpanned<OneOrMany<Name>>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner: Option<ContextSpanned<Name>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<ContextSpanned<Name>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub dictionary: Option<ContextSpanned<OneOrMany<Name>>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<ContextSpanned<UseFromRef>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<ContextSpanned<Path>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub numbered_handle: Option<ContextSpanned<HandleType>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub rights: Option<ContextSpanned<Rights>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdir: Option<ContextSpanned<RelativePath>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<ContextSpanned<OneOrMany<EventScope>>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<ContextSpanned<Map<String, Value>>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency: Option<ContextSpanned<DependencyType>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<ContextSpanned<Availability>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<ContextSpanned<Name>>,

    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub config_type: Option<ContextSpanned<ConfigType>>,

    #[serde(rename = "max_size", skip_serializing_if = "Option::is_none")]
    pub config_max_size: Option<ContextSpanned<NonZeroU32>>,

    #[serde(rename = "max_count", skip_serializing_if = "Option::is_none")]
    pub config_max_count: Option<ContextSpanned<NonZeroU32>>,

    #[serde(rename = "element", skip_serializing_if = "Option::is_none")]
    pub config_element_type: Option<ContextSpanned<ConfigNestedValueType>>,

    #[serde(rename = "default", skip_serializing_if = "Option::is_none")]
    pub config_default: Option<ContextSpanned<serde_json::Value>>,
}

impl ContextCapabilityClause for ContextUse {
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
        None
    }
    fn event_stream(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        option_one_or_many_as_ref_context(&self.event_stream)
    }
    fn dictionary(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        option_one_or_many_as_ref_context(&self.dictionary)
    }
    fn config(&self) -> Option<ContextSpanned<OneOrMany<&BorrowedName>>> {
        self.config.as_ref().map(|s| ContextSpanned {
            value: OneOrMany::One((s.value).as_ref()),
            origin: s.origin.clone(),
        })
    }

    fn decl_type(&self) -> &'static str {
        "use"
    }
    fn supported(&self) -> &[&'static str] {
        &[
            "service",
            "protocol",
            "directory",
            "storage",
            "event_stream",
            "runner",
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
    fn set_runner(&mut self, _o: Option<ContextSpanned<OneOrMany<Name>>>) {}
    fn set_resolver(&mut self, _o: Option<ContextSpanned<OneOrMany<Name>>>) {}
    fn set_event_stream(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.event_stream = o;
    }
    fn set_dictionary(&mut self, _o: Option<ContextSpanned<OneOrMany<Name>>>) {}
    fn set_config(&mut self, o: Option<ContextSpanned<OneOrMany<Name>>>) {
        self.config = always_one_context(o);
    }

    fn origin(&self) -> &Arc<std::path::Path> {
        &self.origin
    }

    fn availability(&self) -> Option<ContextSpanned<Availability>> {
        self.availability.clone()
    }
    fn set_availability(&mut self, a: Option<ContextSpanned<Availability>>) {
        self.availability = a;
    }
}

impl CanonicalizeContext for ContextUse {
    fn canonicalize_context(&mut self) {
        // Sort the names of the capabilities. Only capabilities with OneOrMany values are included here.
        if let Some(service) = &mut self.service {
            service.value.canonicalize_context();
        } else if let Some(protocol) = &mut self.protocol {
            protocol.value.canonicalize_context();
        } else if let Some(event_stream) = &mut self.event_stream {
            event_stream.value.canonicalize_context();
            if let Some(scope) = &mut self.scope {
                scope.value.canonicalize_context();
            }
        }
    }
}

impl FromClauseContext for ContextUse {
    fn from_(&self) -> ContextSpanned<OneOrMany<AnyRef<'_>>> {
        match &self.from {
            Some(from) => {
                return ContextSpanned {
                    value: OneOrMany::One(AnyRef::from(&from.value)),
                    origin: from.origin.clone(),
                };
            }
            // Default for `use`.
            None => {
                return ContextSpanned {
                    value: OneOrMany::One(AnyRef::Parent),
                    origin: self.origin.clone(),
                };
            }
        }
    }
}

impl RightsClause for ContextUse {
    fn rights(&self) -> Option<&Rights> {
        self.rights.as_ref().map(|r| &r.value)
    }
}

impl PartialEq for ContextUse {
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
            && cmp!(event_stream)
            && cmp!(runner)
            && cmp!(config)
            && cmp!(dictionary)
            && cmp!(from)
            && cmp!(path)
            && cmp!(numbered_handle)
            && cmp!(rights)
            && cmp!(subdir)
            && cmp!(scope)
            && cmp!(filter)
            && cmp!(dependency)
            && cmp!(availability)
            && cmp!(key)
            && cmp!(config_type)
            && cmp!(config_max_size)
            && cmp!(config_max_count)
            && cmp!(config_element_type)
            && cmp!(config_default)
    }
}

impl Eq for ContextUse {}

impl Default for ContextUse {
    fn default() -> Self {
        let synthetic_origin: Arc<std::path::Path> = Arc::from(std::path::Path::new("synthetic"));

        Self {
            origin: synthetic_origin,
            service: None,
            protocol: None,
            directory: None,
            storage: None,
            event_stream: None,
            runner: None,
            config: None,
            dictionary: None,
            from: None,
            path: None,
            numbered_handle: None,
            rights: None,
            subdir: None,
            scope: None,
            filter: None,
            dependency: None,
            availability: None,
            key: None,
            config_type: None,
            config_max_size: None,
            config_max_count: None,
            config_element_type: None,
            config_default: None,
        }
    }
}

impl ContextPathClause for ContextUse {
    fn path(&self) -> Option<&ContextSpanned<Path>> {
        self.path.as_ref()
    }
}

impl Hydrate for Use {
    type Output = ContextUse;

    fn hydrate(self, file: &Arc<std::path::Path>) -> Result<Self::Output, Error> {
        Ok(ContextUse {
            origin: file.clone(),
            service: hydrate_opt_simple(self.service, file),
            protocol: hydrate_opt_simple(self.protocol, file),
            directory: hydrate_opt_simple(self.directory, file),
            storage: hydrate_opt_simple(self.storage, file),
            event_stream: hydrate_opt_simple(self.event_stream, file),
            runner: hydrate_opt_simple(self.runner, file),
            config: hydrate_opt_simple(self.config, file),
            dictionary: hydrate_opt_simple(self.dictionary, file),
            from: hydrate_opt_simple(self.from, file),
            path: hydrate_opt_simple(self.path, file),
            numbered_handle: hydrate_opt_simple(self.numbered_handle, file),
            rights: hydrate_opt_simple(self.rights, file),
            subdir: hydrate_opt_simple(self.subdir, file),
            scope: hydrate_opt_simple(self.scope, file),
            filter: hydrate_opt_simple(self.filter, file),
            dependency: hydrate_opt_simple(self.dependency, file),
            availability: hydrate_opt_simple(self.availability, file),
            key: hydrate_opt_simple(self.key, file),
            config_type: hydrate_opt_simple(self.config_type, file),
            config_max_size: hydrate_opt_simple(self.config_max_size, file),
            config_max_count: hydrate_opt_simple(self.config_max_count, file),
            config_element_type: hydrate_opt_simple(self.config_element_type, file),
            config_default: hydrate_opt_simple(self.config_default, file),
        })
    }
}
