// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use assert_matches::assert_matches;
use cm_rust::{CapabilityTypeName, ComponentDecl, FidlIntoNative, push_box};
use cm_types::{LongName, Name, Path, RelativePath, Url};
use derivative::Derivative;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_io as fio;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Name of the test runner.
///
/// Several functions assume the existence of a runner with this name.
pub const TEST_RUNNER_NAME: &str = "test_runner";

/// Deserialize `object` into a cml::Document and then translate the result
/// to ComponentDecl.
pub fn new_decl_from_json(object: serde_json::Value) -> Result<ComponentDecl, Error> {
    let json_str = serde_json::to_string(&object).context("failed to stringify json value")?;

    let dummy_path: Arc<std::path::Path> =
        Arc::from(std::path::Path::new("programmatic_manifest.cml"));

    let doc = cml::types::document::parse_and_hydrate(dummy_path, &json_str)
        .context("failed to parse and hydrate manifest")?;

    let cm =
        cml::compile(&doc, cml::CompileOptions::default()).context("failed to compile manifest")?;
    Ok(cm.fidl_into_native())
}

/// Builder for constructing a ComponentDecl.
#[derive(Debug, Clone)]
pub struct ComponentDeclBuilder {
    result: ComponentDecl,
}

impl ComponentDeclBuilder {
    /// An empty ComponentDeclBuilder, with no program.
    pub fn new_empty_component() -> Self {
        ComponentDeclBuilder { result: Default::default() }
    }

    /// A ComponentDeclBuilder prefilled with a program and using a runner named "test_runner",
    /// which we assume is offered to us.
    pub fn new() -> Self {
        Self::new_empty_component().program_runner(TEST_RUNNER_NAME)
    }

    /// Add a child element.
    pub fn child(mut self, decl: impl Into<cm_rust::ChildDecl>) -> Self {
        push_box(&mut self.result.children, decl.into());
        self
    }

    /// Add a child with default properties.
    pub fn child_default(self, name: &str) -> Self {
        self.child(ChildBuilder::new().name(name))
    }

    // Add a collection element.
    pub fn collection(mut self, decl: impl Into<cm_rust::CollectionDecl>) -> Self {
        push_box(&mut self.result.collections, decl.into());
        self
    }

    /// Add a collection with default properties.
    pub fn collection_default(self, name: &str) -> Self {
        self.collection(CollectionBuilder::new().name(name))
    }

    /// Add a "program" clause, using the given runner.
    pub fn program_runner(self, runner: &str) -> Self {
        assert!(self.result.program.is_none(), "tried to add program twice");
        self.program(cm_rust::ProgramDecl {
            runner: Some(runner.parse().unwrap()),
            info: fdata::Dictionary { entries: Some(vec![]), ..Default::default() },
        })
    }

    pub fn program(mut self, program: cm_rust::ProgramDecl) -> Self {
        self.result.program = Some(program);
        self
    }

    /// Add an offer decl.
    pub fn offer(mut self, offer: impl Into<cm_rust::offer::OfferDecl>) -> Self {
        push_box(&mut self.result.offers, offer.into());
        self
    }

    /// Add an expose decl.
    pub fn expose(mut self, expose: impl Into<cm_rust::ExposeDecl>) -> Self {
        push_box(&mut self.result.exposes, expose.into());
        self
    }

    /// Add a use decl.
    pub fn use_(mut self, use_: impl Into<cm_rust::UseDecl>) -> Self {
        let use_ = use_.into();
        if let cm_rust::UseDecl::Runner(_) = &use_ {
            assert!(
                self.result.program.as_ref().and_then(|p| p.runner.as_ref()).is_none(),
                "tried to add a use decl for a runner while program.runner is set"
            );
        }
        push_box(&mut self.result.uses, use_);
        self
    }

    // Add a use decl for fuchsia.component.Realm.
    pub fn use_realm(self) -> Self {
        self.use_(
            UseBuilder::protocol()
                .name("fuchsia.component.Realm")
                .source(cm_rust::UseSource::Framework),
        )
    }

    /// Add a capability declaration.
    pub fn capability(mut self, capability: impl Into<cm_rust::CapabilityDecl>) -> Self {
        push_box(&mut self.result.capabilities, capability.into());
        self
    }

    /// Add a default protocol declaration.
    pub fn protocol_default(self, name: &str) -> Self {
        self.capability(CapabilityBuilder::protocol().name(name))
    }

    /// Add a default dictionary declaration.
    pub fn dictionary_default(self, name: &str) -> Self {
        self.capability(CapabilityBuilder::dictionary().name(name))
    }

    /// Add a default runner declaration.
    pub fn runner_default(self, name: &str) -> Self {
        self.capability(CapabilityBuilder::runner().name(name))
    }

    /// Add a default resolver declaration.
    pub fn resolver_default(self, name: &str) -> Self {
        self.capability(CapabilityBuilder::resolver().name(name))
    }

    /// Add a default service declaration.
    pub fn service_default(self, name: &str) -> Self {
        self.capability(CapabilityBuilder::service().name(name))
    }

    /// Add an environment declaration.
    pub fn environment(mut self, environment: impl Into<cm_rust::EnvironmentDecl>) -> Self {
        push_box(&mut self.result.environments, environment.into());
        self
    }

    /// Add a config declaration.
    pub fn config(mut self, config: cm_rust::ConfigDecl) -> Self {
        self.result.config = Some(config);
        self
    }

    /// Generate the final ComponentDecl.
    pub fn build(self) -> ComponentDecl {
        self.result
    }
}

/// A convenience builder for constructing ChildDecls.
#[derive(Debug, Derivative)]
#[derivative(Default)]
pub struct ChildBuilder {
    name: Option<LongName>,
    url: Option<Url>,
    #[derivative(Default(value = "fdecl::StartupMode::Lazy"))]
    startup: fdecl::StartupMode,
    on_terminate: Option<fdecl::OnTerminate>,
    environment: Option<Name>,
}

impl ChildBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Defaults url to `"test:///{name}"`.
    pub fn name(mut self, name: &str) -> Self {
        self.name = Some(name.parse().unwrap());
        if self.url.is_none() {
            self.url = Some(format!("test:///{name}").parse().unwrap());
        }
        self
    }

    pub fn url(mut self, url: &str) -> Self {
        self.url = Some(url.parse().unwrap());
        self
    }

    pub fn startup(mut self, startup: fdecl::StartupMode) -> Self {
        self.startup = startup;
        self
    }

    pub fn eager(self) -> Self {
        self.startup(fdecl::StartupMode::Eager)
    }

    pub fn on_terminate(mut self, on_terminate: fdecl::OnTerminate) -> Self {
        self.on_terminate = Some(on_terminate);
        self
    }

    pub fn environment(mut self, environment: &str) -> Self {
        self.environment = Some(environment.parse().unwrap());
        self
    }

    pub fn build(self) -> cm_rust::ChildDecl {
        cm_rust::ChildDecl {
            name: self.name.expect("name not set"),
            url: self.url.expect("url not set"),
            startup: self.startup,
            on_terminate: self.on_terminate,
            environment: self.environment,
            config_overrides: None,
        }
    }
}

impl From<ChildBuilder> for cm_rust::ChildDecl {
    fn from(builder: ChildBuilder) -> Self {
        builder.build()
    }
}

/// A convenience builder for constructing CollectionDecls.
#[derive(Debug, Derivative)]
#[derivative(Default)]
pub struct CollectionBuilder {
    name: Option<Name>,
    #[derivative(Default(value = "fdecl::Durability::Transient"))]
    durability: fdecl::Durability,
    environment: Option<Name>,
    #[derivative(Default(value = "cm_types::AllowedOffers::StaticOnly"))]
    allowed_offers: cm_types::AllowedOffers,
    allow_long_names: bool,
    persistent_storage: Option<bool>,
}

impl CollectionBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn name(mut self, name: &str) -> Self {
        self.name = Some(name.parse().unwrap());
        self
    }

    pub fn durability(mut self, durability: fdecl::Durability) -> Self {
        self.durability = durability;
        self
    }

    pub fn environment(mut self, environment: &str) -> Self {
        self.environment = Some(environment.parse().unwrap());
        self
    }

    pub fn allowed_offers(mut self, allowed_offers: cm_types::AllowedOffers) -> Self {
        self.allowed_offers = allowed_offers;
        self
    }

    pub fn allow_long_names(mut self) -> Self {
        self.allow_long_names = true;
        self
    }

    pub fn persistent_storage(mut self, persistent_storage: bool) -> Self {
        self.persistent_storage = Some(persistent_storage);
        self
    }

    pub fn build(self) -> cm_rust::CollectionDecl {
        cm_rust::CollectionDecl {
            name: self.name.expect("name not set"),
            durability: self.durability,
            environment: self.environment,
            allowed_offers: self.allowed_offers,
            allow_long_names: self.allow_long_names,
            persistent_storage: self.persistent_storage,
        }
    }
}

impl From<CollectionBuilder> for cm_rust::CollectionDecl {
    fn from(builder: CollectionBuilder) -> Self {
        builder.build()
    }
}

/// A convenience builder for constructing EnvironmentDecls.
#[derive(Debug, Derivative)]
#[derivative(Default)]
pub struct EnvironmentBuilder {
    name: Option<Name>,
    #[derivative(Default(value = "fdecl::EnvironmentExtends::Realm"))]
    extends: fdecl::EnvironmentExtends,
    runners: Vec<cm_rust::RunnerRegistration>,
    resolvers: Vec<cm_rust::ResolverRegistration>,
    debug_capabilities: Vec<cm_rust::DebugRegistration>,
    stop_timeout_ms: Option<u32>,
}

impl EnvironmentBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn name(mut self, name: &str) -> Self {
        self.name = Some(name.parse().unwrap());
        self
    }

    pub fn extends(mut self, extends: fdecl::EnvironmentExtends) -> Self {
        self.extends = extends;
        self
    }

    pub fn runner(mut self, runner: cm_rust::RunnerRegistration) -> Self {
        self.runners.push(runner);
        self
    }

    pub fn resolver(mut self, resolver: cm_rust::ResolverRegistration) -> Self {
        self.resolvers.push(resolver);
        self
    }

    pub fn debug(mut self, debug: cm_rust::DebugRegistration) -> Self {
        self.debug_capabilities.push(debug);
        self
    }

    pub fn stop_timeout(mut self, timeout_ms: u32) -> Self {
        self.stop_timeout_ms = Some(timeout_ms);
        self
    }

    pub fn build(self) -> cm_rust::EnvironmentDecl {
        cm_rust::EnvironmentDecl {
            name: self.name.expect("name not set"),
            extends: self.extends,
            runners: self.runners.into(),
            resolvers: self.resolvers.into(),
            debug_capabilities: self.debug_capabilities.into(),
            stop_timeout_ms: self.stop_timeout_ms,
        }
    }
}

impl From<EnvironmentBuilder> for cm_rust::EnvironmentDecl {
    fn from(builder: EnvironmentBuilder) -> Self {
        builder.build()
    }
}

/// A convenience builder for constructing [CapabilityDecl]s.
///
/// To use, call the constructor matching their capability type ([CapabilityBuilder::protocol],
/// [CapabilityBuilder::directory], etc., and then call methods to set properties. When done,
/// call [CapabilityBuilder::build] (or [Into::into]) to generate the [CapabilityDecl].
#[derive(Debug)]
pub struct CapabilityBuilder {
    name: Option<Name>,
    type_: CapabilityTypeName,
    path: Option<Path>,
    dictionary_source: Option<cm_rust::DictionarySource>,
    source_dictionary: Option<RelativePath>,
    rights: fio::Operations,
    subdir: RelativePath,
    backing_dir: Option<Name>,
    storage_source: Option<cm_rust::StorageDirectorySource>,
    storage_id: fdecl::StorageId,
    value: Option<cm_rust::ConfigValue>,
    delivery: cm_rust::DeliveryType,
}

impl CapabilityBuilder {
    pub fn protocol() -> Self {
        Self::new(CapabilityTypeName::Protocol)
    }

    pub fn service() -> Self {
        Self::new(CapabilityTypeName::Service)
    }

    pub fn directory() -> Self {
        Self::new(CapabilityTypeName::Directory)
    }

    pub fn storage() -> Self {
        Self::new(CapabilityTypeName::Storage)
    }

    pub fn runner() -> Self {
        Self::new(CapabilityTypeName::Runner)
    }

    pub fn resolver() -> Self {
        Self::new(CapabilityTypeName::Resolver)
    }

    pub fn dictionary() -> Self {
        Self::new(CapabilityTypeName::Dictionary)
    }

    pub fn config() -> Self {
        Self::new(CapabilityTypeName::Config)
    }

    pub fn name(mut self, name: &str) -> Self {
        self.name = Some(name.parse().unwrap());
        if self.path.is_some() {
            return self;
        }
        match self.type_ {
            CapabilityTypeName::Protocol => {
                self.path = Some(format!("/svc/{name}").parse().unwrap());
            }
            CapabilityTypeName::Service => {
                self.path = Some(format!("/svc/{name}").parse().unwrap());
            }
            CapabilityTypeName::Runner => {
                self.path = Some("/svc/fuchsia.component.runner.ComponentRunner".parse().unwrap());
            }
            CapabilityTypeName::Resolver => {
                self.path = Some("/svc/fuchsia.component.resolution.Resolver".parse().unwrap());
            }
            CapabilityTypeName::Dictionary
            | CapabilityTypeName::Storage
            | CapabilityTypeName::Config
            | CapabilityTypeName::Directory => {}
            CapabilityTypeName::EventStream => unreachable!(),
        }
        self
    }

    fn new(type_: CapabilityTypeName) -> Self {
        Self {
            type_,
            name: None,
            path: None,
            dictionary_source: None,
            source_dictionary: None,
            rights: fio::R_STAR_DIR,
            subdir: Default::default(),
            backing_dir: None,
            storage_source: None,
            storage_id: fdecl::StorageId::StaticInstanceIdOrMoniker,
            value: None,
            delivery: Default::default(),
        }
    }

    pub fn path(mut self, path: &str) -> Self {
        assert_matches!(
            self.type_,
            CapabilityTypeName::Protocol
                | CapabilityTypeName::Service
                | CapabilityTypeName::Directory
                | CapabilityTypeName::Dictionary
                | CapabilityTypeName::Runner
                | CapabilityTypeName::Resolver
        );
        if self.type_ == CapabilityTypeName::Dictionary {
            if self.dictionary_source.is_some() || self.source_dictionary.is_some() {
                panic!("Dictionary path is incompatible with source_dictionary");
            }
        }
        self.path = Some(path.parse().unwrap());
        self
    }

    pub fn rights(mut self, rights: fio::Operations) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Directory);
        self.rights = rights;
        self
    }

    pub fn backing_dir(mut self, backing_dir: &str) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Storage);
        self.backing_dir = Some(backing_dir.parse().unwrap());
        self
    }

    pub fn value(mut self, value: cm_rust::ConfigValue) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Config);
        self.value = Some(value);
        self
    }

    pub fn source(mut self, source: cm_rust::StorageDirectorySource) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Storage);
        self.storage_source = Some(source);
        self
    }

    pub fn subdir(mut self, subdir: &str) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Storage);
        self.subdir = subdir.parse().unwrap();
        self
    }

    pub fn storage_id(mut self, storage_id: fdecl::StorageId) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Storage);
        self.storage_id = storage_id;
        self
    }

    pub fn delivery(mut self, delivery: cm_rust::DeliveryType) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Protocol);
        self.delivery = delivery;
        self
    }

    pub fn build(self) -> cm_rust::CapabilityDecl {
        match self.type_ {
            CapabilityTypeName::Protocol => {
                cm_rust::CapabilityDecl::Protocol(cm_rust::ProtocolDecl {
                    name: self.name.expect("name not set"),
                    source_path: Some(self.path.expect("path not set")),
                    delivery: self.delivery,
                })
            }
            CapabilityTypeName::Service => cm_rust::CapabilityDecl::Service(cm_rust::ServiceDecl {
                name: self.name.expect("name not set"),
                source_path: Some(self.path.expect("path not set")),
            }),
            CapabilityTypeName::Runner => cm_rust::CapabilityDecl::Runner(cm_rust::RunnerDecl {
                name: self.name.expect("name not set"),
                source_path: Some(self.path.expect("path not set")),
            }),
            CapabilityTypeName::Resolver => {
                cm_rust::CapabilityDecl::Resolver(cm_rust::ResolverDecl {
                    name: self.name.expect("name not set"),
                    source_path: Some(self.path.expect("path not set")),
                })
            }
            CapabilityTypeName::Dictionary => {
                cm_rust::CapabilityDecl::Dictionary(cm_rust::DictionaryDecl {
                    name: self.name.expect("name not set"),
                    source_path: self.path,
                })
            }
            CapabilityTypeName::Storage => cm_rust::CapabilityDecl::Storage(cm_rust::StorageDecl {
                name: self.name.expect("name not set"),
                backing_dir: self.backing_dir.expect("backing_dir not set"),
                source: self.storage_source.expect("source not set"),
                subdir: self.subdir,
                storage_id: self.storage_id,
            }),
            CapabilityTypeName::Directory => {
                cm_rust::CapabilityDecl::Directory(cm_rust::DirectoryDecl {
                    name: self.name.expect("name not set"),
                    source_path: Some(self.path.expect("path not set")),
                    rights: self.rights,
                })
            }
            CapabilityTypeName::Config => {
                cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                    name: self.name.expect("name not set"),
                    value: self.value.expect("value not set"),
                })
            }
            CapabilityTypeName::EventStream => unreachable!(),
        }
    }
}

impl From<CapabilityBuilder> for cm_rust::CapabilityDecl {
    fn from(builder: CapabilityBuilder) -> Self {
        builder.build()
    }
}

/// A convenience builder for constructing [UseDecl]s.
///
/// To use, call the constructor matching their capability type ([UseBuilder::protocol],
/// [UseBuilder::directory], etc.), and then call methods to set properties. When done,
/// call [UseBuilder::build] (or [Into::into]) to generate the [UseDecl].
#[derive(Debug)]
pub struct UseBuilder {
    source_name: Option<Name>,
    type_: CapabilityTypeName,
    source_dictionary: RelativePath,
    source: cm_rust::UseSource,
    target_name: Option<Name>,
    target_path: Option<Path>,
    #[cfg(fuchsia_api_level_at_least = "29")]
    numbered_handle: Option<cm_types::HandleType>,
    dependency_type: cm_rust::DependencyType,
    availability: cm_rust::Availability,
    rights: fio::Operations,
    subdir: RelativePath,
    scope: Option<Box<[cm_rust::EventScope]>>,
    filter: Option<BTreeMap<String, cm_rust::DictionaryValue>>,
    config_type: Option<cm_rust::ConfigValueType>,
}

impl UseBuilder {
    pub fn protocol() -> Self {
        Self::new(CapabilityTypeName::Protocol)
    }

    pub fn service() -> Self {
        Self::new(CapabilityTypeName::Service)
    }

    pub fn directory() -> Self {
        Self::new(CapabilityTypeName::Directory)
    }

    pub fn storage() -> Self {
        Self::new(CapabilityTypeName::Storage)
    }

    pub fn runner() -> Self {
        Self::new(CapabilityTypeName::Runner)
    }

    pub fn event_stream() -> Self {
        Self::new(CapabilityTypeName::EventStream)
    }

    pub fn config() -> Self {
        Self::new(CapabilityTypeName::Config)
    }

    pub fn dictionary() -> Self {
        Self::new(CapabilityTypeName::Dictionary)
    }

    fn new(type_: CapabilityTypeName) -> Self {
        Self {
            type_,
            source: cm_rust::UseSource::Parent,
            source_name: None,
            target_name: None,
            target_path: None,
            #[cfg(fuchsia_api_level_at_least = "29")]
            numbered_handle: None,
            source_dictionary: Default::default(),
            rights: fio::R_STAR_DIR,
            subdir: Default::default(),
            dependency_type: cm_rust::DependencyType::Strong,
            availability: cm_rust::Availability::Required,
            scope: None,
            filter: None,
            config_type: None,
        }
    }

    pub fn config_type(mut self, type_: cm_rust::ConfigValueType) -> Self {
        self.config_type = Some(type_);
        self
    }

    pub fn name(mut self, name: &str) -> Self {
        self.source_name = Some(name.parse().unwrap());
        if self.target_path.is_some() || self.target_name.is_some() {
            return self;
        }
        match self.type_ {
            CapabilityTypeName::Protocol | CapabilityTypeName::Service => {
                self.target_path = Some(format!("/svc/{name}").parse().unwrap());
            }
            CapabilityTypeName::EventStream => {
                self.target_path = Some("/svc/fuchsia.component.EventStream".parse().unwrap());
            }
            CapabilityTypeName::Runner | CapabilityTypeName::Config => {
                self.target_name = self.source_name.clone();
            }
            CapabilityTypeName::Dictionary
            | CapabilityTypeName::Storage
            | CapabilityTypeName::Directory => {}
            CapabilityTypeName::Resolver => unreachable!(),
        }
        self
    }

    pub fn path(mut self, path: &str) -> Self {
        assert_matches!(
            self.type_,
            CapabilityTypeName::Protocol
                | CapabilityTypeName::Service
                | CapabilityTypeName::Directory
                | CapabilityTypeName::EventStream
                | CapabilityTypeName::Storage
                | CapabilityTypeName::Dictionary
        );
        self.target_path = Some(path.parse().unwrap());
        self
    }

    #[cfg(fuchsia_api_level_at_least = "29")]
    pub fn numbered_handle(mut self, h: impl Into<cm_types::HandleType>) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Protocol);
        self.numbered_handle = Some(h.into());
        self.target_path = None;
        self
    }

    pub fn target_name(mut self, name: &str) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Runner | CapabilityTypeName::Config);
        self.target_name = Some(name.parse().unwrap());
        self
    }

    pub fn from_dictionary(mut self, dictionary: &str) -> Self {
        assert_matches!(
            self.type_,
            CapabilityTypeName::Service
                | CapabilityTypeName::Protocol
                | CapabilityTypeName::Directory
                | CapabilityTypeName::Runner
                | CapabilityTypeName::Config
        );
        self.source_dictionary = dictionary.parse().unwrap();
        self
    }

    pub fn source(mut self, source: cm_rust::UseSource) -> Self {
        assert_matches!(self.type_, t if t != CapabilityTypeName::Storage);
        self.source = source;
        self
    }

    pub fn source_static_child(self, source: &str) -> Self {
        self.source(cm_rust::UseSource::Child(source.parse().unwrap()))
    }

    pub fn availability(mut self, availability: cm_rust::Availability) -> Self {
        assert_matches!(
            self.type_,
            CapabilityTypeName::Protocol
                | CapabilityTypeName::Service
                | CapabilityTypeName::Directory
                | CapabilityTypeName::EventStream
                | CapabilityTypeName::Storage
                | CapabilityTypeName::Config
        );
        self.availability = availability;
        self
    }

    pub fn dependency(mut self, dependency: cm_rust::DependencyType) -> Self {
        assert_matches!(
            self.type_,
            CapabilityTypeName::Protocol
                | CapabilityTypeName::Service
                | CapabilityTypeName::Directory
        );
        self.dependency_type = dependency;
        self
    }

    pub fn rights(mut self, rights: fio::Operations) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Directory);
        self.rights = rights;
        self
    }

    pub fn subdir(mut self, subdir: &str) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Directory);
        self.subdir = subdir.parse().unwrap();
        self
    }

    pub fn scope(mut self, scope: Vec<cm_rust::EventScope>) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::EventStream);
        self.scope = Some(scope.into());
        self
    }

    pub fn filter(mut self, filter: BTreeMap<String, cm_rust::DictionaryValue>) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::EventStream);
        self.filter = Some(filter);
        self
    }

    pub fn build(self) -> cm_rust::UseDecl {
        match self.type_ {
            CapabilityTypeName::Protocol => {
                #[cfg(not(fuchsia_api_level_at_least = "29"))]
                let has_numbered_handle = false;
                #[cfg(fuchsia_api_level_at_least = "29")]
                let has_numbered_handle = self.numbered_handle.is_some();
                if !has_numbered_handle && self.target_path.is_none() {
                    panic!("path not set");
                }
                if has_numbered_handle && self.target_path.is_some() {
                    panic!("path and numbered_handle both set");
                }
                cm_rust::UseDecl::Protocol(cm_rust::UseProtocolDecl {
                    source: self.source,
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target_path: self.target_path,
                    #[cfg(fuchsia_api_level_at_least = "29")]
                    numbered_handle: self.numbered_handle,
                    dependency_type: self.dependency_type,
                    availability: self.availability,
                })
            }
            CapabilityTypeName::Service => cm_rust::UseDecl::Service(cm_rust::UseServiceDecl {
                source: self.source,
                source_name: self.source_name.expect("name not set"),
                source_dictionary: self.source_dictionary,
                target_path: self.target_path.expect("path not set"),
                dependency_type: self.dependency_type,
                availability: self.availability,
            }),
            CapabilityTypeName::Directory => {
                cm_rust::UseDecl::Directory(cm_rust::UseDirectoryDecl {
                    source: self.source,
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target_path: self.target_path.expect("path not set"),
                    rights: self.rights,
                    subdir: self.subdir,
                    dependency_type: self.dependency_type,
                    availability: self.availability,
                })
            }
            CapabilityTypeName::Storage => cm_rust::UseDecl::Storage(cm_rust::UseStorageDecl {
                source_name: self.source_name.expect("name not set"),
                target_path: self.target_path.expect("path not set"),
                availability: self.availability,
            }),
            CapabilityTypeName::EventStream => {
                cm_rust::UseDecl::EventStream(Box::new(cm_rust::UseEventStreamDecl {
                    source: self.source,
                    source_name: self.source_name.expect("name not set"),
                    target_path: self.target_path.expect("path not set"),
                    availability: self.availability,
                    scope: self.scope,
                    filter: self.filter,
                }))
            }
            CapabilityTypeName::Runner => cm_rust::UseDecl::Runner(cm_rust::UseRunnerDecl {
                source: self.source,
                source_name: self.source_name.expect("name not set"),
                source_dictionary: self.source_dictionary,
            }),
            CapabilityTypeName::Config => {
                cm_rust::UseDecl::Config(Box::new(cm_rust::UseConfigurationDecl {
                    source: self.source,
                    source_name: self.source_name.expect("name not set"),
                    target_name: self.target_name.expect("target name not set"),
                    availability: self.availability,
                    type_: self.config_type.expect("config_type not set"),
                    default: None,
                    source_dictionary: self.source_dictionary,
                }))
            }
            CapabilityTypeName::Dictionary => {
                cm_rust::UseDecl::Dictionary(cm_rust::UseDictionaryDecl {
                    source: self.source,
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target_path: self.target_path.expect("path not set"),
                    dependency_type: self.dependency_type,
                    availability: self.availability,
                })
            }
            CapabilityTypeName::Resolver => unreachable!(),
        }
    }
}

impl From<UseBuilder> for cm_rust::UseDecl {
    fn from(builder: UseBuilder) -> Self {
        builder.build()
    }
}

/// A convenience builder for constructing [ExposeDecl]s.
///
/// To use, call the constructor matching their capability type ([ExposeBuilder::protocol],
/// [ExposeBuilder::directory], etc.), and then call methods to set properties. When done,
/// call [ExposeBuilder::build] (or [Into::into]) to generate the [ExposeDecl].
#[derive(Debug)]
pub struct ExposeBuilder {
    source_name: Option<Name>,
    type_: CapabilityTypeName,
    source_dictionary: RelativePath,
    source: Option<cm_rust::ExposeSource>,
    target: cm_rust::ExposeTarget,
    target_name: Option<Name>,
    availability: cm_rust::Availability,
    rights: Option<fio::Operations>,
    subdir: RelativePath,
}

impl ExposeBuilder {
    pub fn protocol() -> Self {
        Self::new(CapabilityTypeName::Protocol)
    }

    pub fn service() -> Self {
        Self::new(CapabilityTypeName::Service)
    }

    pub fn directory() -> Self {
        Self::new(CapabilityTypeName::Directory)
    }

    pub fn runner() -> Self {
        Self::new(CapabilityTypeName::Runner)
    }

    pub fn resolver() -> Self {
        Self::new(CapabilityTypeName::Resolver)
    }

    pub fn dictionary() -> Self {
        Self::new(CapabilityTypeName::Dictionary)
    }

    pub fn config() -> Self {
        Self::new(CapabilityTypeName::Config)
    }

    fn new(type_: CapabilityTypeName) -> Self {
        Self {
            type_,
            source: None,
            target: cm_rust::ExposeTarget::Parent,
            source_name: None,
            target_name: None,
            source_dictionary: Default::default(),
            rights: None,
            subdir: Default::default(),
            availability: cm_rust::Availability::Required,
        }
    }

    pub fn name(mut self, name: &str) -> Self {
        self.source_name = Some(name.parse().unwrap());
        if self.target_name.is_some() {
            return self;
        }
        self.target_name = self.source_name.clone();
        self
    }

    pub fn target_name(mut self, name: &str) -> Self {
        self.target_name = Some(name.parse().unwrap());
        self
    }

    pub fn from_dictionary(mut self, dictionary: &str) -> Self {
        assert_matches!(
            self.type_,
            CapabilityTypeName::Service
                | CapabilityTypeName::Protocol
                | CapabilityTypeName::Directory
                | CapabilityTypeName::Dictionary
                | CapabilityTypeName::Runner
                | CapabilityTypeName::Resolver
                | CapabilityTypeName::Config
        );
        self.source_dictionary = dictionary.parse().unwrap();
        self
    }

    pub fn source(mut self, source: cm_rust::ExposeSource) -> Self {
        self.source = Some(source);
        self
    }

    pub fn source_static_child(self, source: &str) -> Self {
        self.source(cm_rust::ExposeSource::Child(source.parse().unwrap()))
    }

    pub fn target(mut self, target: cm_rust::ExposeTarget) -> Self {
        self.target = target;
        self
    }

    pub fn availability(mut self, availability: cm_rust::Availability) -> Self {
        assert_matches!(
            self.type_,
            CapabilityTypeName::Protocol
                | CapabilityTypeName::Service
                | CapabilityTypeName::Directory
                | CapabilityTypeName::Config
                | CapabilityTypeName::Dictionary
        );
        self.availability = availability;
        self
    }

    pub fn rights(mut self, rights: fio::Operations) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Directory);
        self.rights = Some(rights);
        self
    }

    pub fn subdir(mut self, subdir: &str) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Directory);
        self.subdir = subdir.parse().unwrap();
        self
    }

    pub fn build(self) -> cm_rust::ExposeDecl {
        match self.type_ {
            CapabilityTypeName::Protocol => {
                cm_rust::ExposeDecl::Protocol(cm_rust::ExposeProtocolDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target,
                    target_name: self.target_name.expect("name not set"),
                    availability: self.availability,
                })
            }
            CapabilityTypeName::Service => {
                cm_rust::ExposeDecl::Service(cm_rust::ExposeServiceDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target,
                    target_name: self.target_name.expect("name not set"),
                    availability: self.availability,
                })
            }
            CapabilityTypeName::Directory => {
                cm_rust::ExposeDecl::Directory(cm_rust::ExposeDirectoryDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target,
                    target_name: self.target_name.expect("name not set"),
                    rights: self.rights,
                    subdir: self.subdir,
                    availability: self.availability,
                })
            }
            CapabilityTypeName::Runner => cm_rust::ExposeDecl::Runner(cm_rust::ExposeRunnerDecl {
                source: self.source.expect("source not set"),
                source_name: self.source_name.expect("name not set"),
                source_dictionary: self.source_dictionary,
                target: self.target,
                target_name: self.target_name.expect("name not set"),
            }),
            CapabilityTypeName::Resolver => {
                cm_rust::ExposeDecl::Resolver(cm_rust::ExposeResolverDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target,
                    target_name: self.target_name.expect("name not set"),
                })
            }
            CapabilityTypeName::Config => {
                cm_rust::ExposeDecl::Config(cm_rust::ExposeConfigurationDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target,
                    target_name: self.target_name.expect("name not set"),
                    availability: self.availability,
                })
            }
            CapabilityTypeName::Dictionary => {
                cm_rust::ExposeDecl::Dictionary(cm_rust::ExposeDictionaryDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target,
                    target_name: self.target_name.expect("name not set"),
                    availability: self.availability,
                })
            }
            CapabilityTypeName::EventStream | CapabilityTypeName::Storage => unreachable!(),
        }
    }
}

impl From<ExposeBuilder> for cm_rust::ExposeDecl {
    fn from(builder: ExposeBuilder) -> Self {
        builder.build()
    }
}

pub fn offer_source_static_child(name: &str) -> cm_rust::offer::OfferSource {
    cm_rust::offer::OfferSource::Child(cm_rust::ChildRef {
        name: name.parse().unwrap(),
        collection: None,
    })
}

pub fn offer_target_static_child(name: &str) -> cm_rust::offer::OfferTarget {
    cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
        name: name.parse().unwrap(),
        collection: None,
    })
}

/// A convenience builder for constructing [OfferDecl]s.
///
/// To use, call the constructor matching their capability type ([OfferBuilder::protocol],
/// [OfferBuilder::directory], etc.), and then call methods to set properties. When done,
/// call [OfferBuilder::build] (or [Into::into]) to generate the [OfferDecl].
#[derive(Debug)]
pub struct OfferBuilder {
    source_name: Option<Name>,
    type_: CapabilityTypeName,
    source_dictionary: RelativePath,
    source: Option<cm_rust::offer::OfferSource>,
    target: Option<cm_rust::offer::OfferTarget>,
    target_name: Option<Name>,
    source_instance_filter: Option<Vec<Name>>,
    renamed_instances: Option<Vec<cm_rust::NameMapping>>,
    rights: Option<fio::Operations>,
    subdir: RelativePath,
    scope: Option<Box<[cm_rust::EventScope]>>,
    dependency_type: cm_rust::DependencyType,
    availability: cm_rust::Availability,
}

impl OfferBuilder {
    pub fn protocol() -> Self {
        Self::new(CapabilityTypeName::Protocol)
    }

    pub fn service() -> Self {
        Self::new(CapabilityTypeName::Service)
    }

    pub fn directory() -> Self {
        Self::new(CapabilityTypeName::Directory)
    }

    pub fn storage() -> Self {
        Self::new(CapabilityTypeName::Storage)
    }

    pub fn runner() -> Self {
        Self::new(CapabilityTypeName::Runner)
    }

    pub fn resolver() -> Self {
        Self::new(CapabilityTypeName::Resolver)
    }

    pub fn dictionary() -> Self {
        Self::new(CapabilityTypeName::Dictionary)
    }

    pub fn event_stream() -> Self {
        Self::new(CapabilityTypeName::EventStream)
    }

    pub fn config() -> Self {
        Self::new(CapabilityTypeName::Config)
    }

    fn new(type_: CapabilityTypeName) -> Self {
        Self {
            type_,
            source: None,
            target: None,
            source_name: None,
            target_name: None,
            source_dictionary: Default::default(),
            source_instance_filter: None,
            renamed_instances: None,
            rights: None,
            subdir: Default::default(),
            scope: None,
            dependency_type: cm_rust::DependencyType::Strong,
            availability: cm_rust::Availability::Required,
        }
    }

    pub fn name(mut self, name: &str) -> Self {
        self.source_name = Some(name.parse().unwrap());
        if self.target_name.is_some() {
            return self;
        }
        self.target_name = self.source_name.clone();
        self
    }

    pub fn target_name(mut self, name: &str) -> Self {
        self.target_name = Some(name.parse().unwrap());
        self
    }

    pub fn from_dictionary(mut self, dictionary: &str) -> Self {
        assert_matches!(
            self.type_,
            CapabilityTypeName::Service
                | CapabilityTypeName::Protocol
                | CapabilityTypeName::Directory
                | CapabilityTypeName::Dictionary
                | CapabilityTypeName::Runner
                | CapabilityTypeName::Resolver
                | CapabilityTypeName::Config
        );
        self.source_dictionary = dictionary.parse().unwrap();
        self
    }

    pub fn source(mut self, source: cm_rust::offer::OfferSource) -> Self {
        self.source = Some(source);
        self
    }

    pub fn source_static_child(self, source: &str) -> Self {
        self.source(offer_source_static_child(source))
    }

    pub fn target(mut self, target: cm_rust::offer::OfferTarget) -> Self {
        self.target = Some(target);
        self
    }

    pub fn target_static_child(self, target: &str) -> Self {
        self.target(offer_target_static_child(target))
    }

    pub fn target_capability(self, target: &str) -> Self {
        self.target(cm_rust::offer::OfferTarget::Capability(target.parse().unwrap()))
    }

    pub fn availability(mut self, availability: cm_rust::Availability) -> Self {
        assert_matches!(
            self.type_,
            CapabilityTypeName::Protocol
                | CapabilityTypeName::Service
                | CapabilityTypeName::Directory
                | CapabilityTypeName::Storage
                | CapabilityTypeName::EventStream
                | CapabilityTypeName::Config
                | CapabilityTypeName::Dictionary
        );
        self.availability = availability;
        self
    }

    pub fn dependency(mut self, dependency: cm_rust::DependencyType) -> Self {
        assert_matches!(
            self.type_,
            CapabilityTypeName::Protocol
                | CapabilityTypeName::Directory
                | CapabilityTypeName::Dictionary
        );
        self.dependency_type = dependency;
        self
    }

    pub fn source_instance_filter<'a>(mut self, filter: impl IntoIterator<Item = &'a str>) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Service);
        self.source_instance_filter =
            Some(filter.into_iter().map(|s| s.parse().unwrap()).collect());
        self
    }

    pub fn renamed_instances<'a, 'b>(
        mut self,
        mapping: impl IntoIterator<Item = (&'a str, &'b str)>,
    ) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Service);
        self.renamed_instances = Some(
            mapping
                .into_iter()
                .map(|(s, t)| cm_rust::NameMapping {
                    source_name: s.parse().unwrap(),
                    target_name: t.parse().unwrap(),
                })
                .collect(),
        );
        self
    }

    pub fn rights(mut self, rights: fio::Operations) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Directory);
        self.rights = Some(rights);
        self
    }

    pub fn subdir(mut self, subdir: &str) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::Directory);
        self.subdir = subdir.parse().unwrap();
        self
    }

    pub fn scope(mut self, scope: Vec<cm_rust::EventScope>) -> Self {
        assert_matches!(self.type_, CapabilityTypeName::EventStream);
        self.scope = Some(scope.into());
        self
    }

    pub fn build(self) -> cm_rust::offer::OfferDecl {
        match self.type_ {
            CapabilityTypeName::Protocol => {
                cm_rust::offer::OfferDecl::Protocol(cm_rust::offer::OfferProtocolDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target.expect("target not set"),
                    target_name: self.target_name.expect("name not set"),
                    dependency_type: self.dependency_type,
                    availability: self.availability,
                })
            }
            CapabilityTypeName::Service => {
                cm_rust::offer::OfferDecl::Service(Box::new(cm_rust::offer::OfferServiceDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target.expect("target is not set"),
                    target_name: self.target_name.expect("name not set"),
                    source_instance_filter: self.source_instance_filter.map(Into::into),
                    renamed_instances: self.renamed_instances.map(Into::into),
                    availability: self.availability,
                    dependency_type: Default::default(),
                }))
            }
            CapabilityTypeName::Directory => {
                cm_rust::offer::OfferDecl::Directory(Box::new(cm_rust::offer::OfferDirectoryDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target.expect("target is not set"),
                    target_name: self.target_name.expect("name not set"),
                    rights: self.rights,
                    subdir: self.subdir,
                    dependency_type: self.dependency_type,
                    availability: self.availability,
                }))
            }
            CapabilityTypeName::Storage => {
                cm_rust::offer::OfferDecl::Storage(cm_rust::offer::OfferStorageDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    target: self.target.expect("target is not set"),
                    target_name: self.target_name.expect("name not set"),
                    availability: self.availability,
                })
            }
            CapabilityTypeName::EventStream => cm_rust::offer::OfferDecl::EventStream(Box::new(
                cm_rust::offer::OfferEventStreamDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    target: self.target.expect("target is not set"),
                    target_name: self.target_name.expect("name not set"),
                    availability: self.availability,
                    scope: self.scope,
                },
            )),
            CapabilityTypeName::Runner => {
                cm_rust::offer::OfferDecl::Runner(cm_rust::offer::OfferRunnerDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target.expect("target is not set"),
                    target_name: self.target_name.expect("name not set"),
                })
            }
            CapabilityTypeName::Resolver => {
                cm_rust::offer::OfferDecl::Resolver(cm_rust::offer::OfferResolverDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target.expect("target is not set"),
                    target_name: self.target_name.expect("name not set"),
                })
            }
            CapabilityTypeName::Config => {
                cm_rust::offer::OfferDecl::Config(cm_rust::offer::OfferConfigurationDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    target: self.target.expect("target not set"),
                    target_name: self.target_name.expect("name not set"),
                    availability: self.availability,
                    source_dictionary: self.source_dictionary,
                })
            }
            CapabilityTypeName::Dictionary => {
                cm_rust::offer::OfferDecl::Dictionary(cm_rust::offer::OfferDictionaryDecl {
                    source: self.source.expect("source not set"),
                    source_name: self.source_name.expect("name not set"),
                    source_dictionary: self.source_dictionary,
                    target: self.target.expect("target not set"),
                    target_name: self.target_name.expect("name not set"),
                    dependency_type: self.dependency_type,
                    availability: self.availability,
                })
            }
        }
    }
}

impl From<OfferBuilder> for cm_rust::offer::OfferDecl {
    fn from(builder: OfferBuilder) -> Self {
        builder.build()
    }
}
