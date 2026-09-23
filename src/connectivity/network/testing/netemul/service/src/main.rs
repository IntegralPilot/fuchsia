// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, anyhow};
use cm_rust::FidlIntoNative as _;
use component_events::events::Event;
use fidl::endpoints::{DiscoverableProtocolMarker, ServerEnd, ServiceMarker as _};
use fidl_fuchsia_component_test as ftest;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_hardware_network as fhwnet;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_logger as flogger;
use fidl_fuchsia_netemul::{
    self as fnetemul, ChildDef, ChildUses, ManagedRealmMarker, ManagedRealmRequest,
    NETEMUL_SERVICES_COMPONENT_NAME, RealmOptions, SandboxRequest, SandboxRequestStream,
};
use fidl_fuchsia_netemul_network as fnetemul_network;
use fidl_fuchsia_sys2 as fsys2;
use fidl_fuchsia_tracing_provider as ftracing_provider;
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceFsDir};
use fuchsia_component_test::{
    self as fcomponent, Capability, ChildOptions, LocalComponentHandles, RealmBuilder,
    RealmBuilderParams, RealmInstance, Ref, Route,
};
use futures::channel::mpsc;
use futures::stream::FusedStream as _;
use futures::{FutureExt as _, SinkExt as _, StreamExt as _, TryFutureExt as _, TryStreamExt as _};
use log::{debug, error, info, warn};
use std::borrow::Cow;
use std::collections::hash_map::{Entry, HashMap};
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use vfs::ObjectRequestRef;
use vfs::directory::entry::{EntryInfo, OpenRequest};
use vfs::directory::helper::DirectlyMutable as _;
use vfs::directory::immutable::simple::Simple as SimpleImmutableDir;
use vfs::remote::RemoteLike;

type Result<T = (), E = anyhow::Error> = std::result::Result<T, E>;

const REALM_COLLECTION_NAME: &str = "netemul";
const DEVFS: &str = "dev";
const DEVFS_PATH: &str = "/dev";
const DEVFS_CAPABILITY: &str = "dev-topological";
const CUSTOM_ARTIFACTS_PATH: &str = "/custom_artifacts";
const CUSTOM_ARTIFACTS_CAPABILITY: &str = "custom_artifacts";
const HWNETWORK_SERVICE_MEMBER_NAME: &str = "device";
const DEVICE_TOPOLOGY_MEMBER_NAME: &str = "device_topology";

#[derive(Error, Debug)]
enum CreateRealmError {
    #[error("source not provided")]
    SourceNotProvided,
    #[error("name not provided")]
    NameNotProvided,
    #[error("capability source not provided")]
    CapabilityNameNotProvided,
    #[error("duplicate capability '{0}' used by component '{1}'")]
    DuplicateCapabilityUse(String, String),
    #[error("cannot modify program arguments of component without a program: '{0}'")]
    ModifiedNonexistentProgram(String),
    #[error("realm builder error: {0:?}")]
    RealmBuilderError(#[from] fcomponent::error::Error),
    #[error("storage capability variant not provided")]
    StorageCapabilityVariantNotProvided,
    #[error("storage capability path not provided")]
    StorageCapabilityPathNotProvided,
    #[error("devfs capability name not provided")]
    DevfsCapabilityNameNotProvided,
    #[error("invalid devfs subdirectory '{0}'")]
    InvalidDevfsSubdirectory(String),
    #[error("duplicate protocol '{0}' exposed by component '{1}'")]
    DuplicateProtocolExposed(String, String),
    #[error("is_weak provided for capability that does not support it '{0}")]
    WeakBindingUnsupported(String),
}

impl From<CreateRealmError> for zx::Status {
    fn from(err: CreateRealmError) -> zx::Status {
        match err {
            CreateRealmError::SourceNotProvided
            | CreateRealmError::NameNotProvided
            | CreateRealmError::CapabilityNameNotProvided
            | CreateRealmError::DuplicateCapabilityUse(String { .. }, String { .. })
            | CreateRealmError::ModifiedNonexistentProgram(String { .. })
            | CreateRealmError::StorageCapabilityVariantNotProvided
            | CreateRealmError::StorageCapabilityPathNotProvided
            | CreateRealmError::DevfsCapabilityNameNotProvided
            | CreateRealmError::InvalidDevfsSubdirectory(String { .. })
            | CreateRealmError::DuplicateProtocolExposed(String { .. }, String { .. })
            | CreateRealmError::WeakBindingUnsupported(String { .. }) => zx::Status::INVALID_ARGS,
            CreateRealmError::RealmBuilderError(error) => match error {
                // The following types of errors from the realm builder library are likely due to
                // client error (e.g. attempting to create a realm with an invalid configuration).
                fcomponent::error::Error::ServerError(
                    ftest::RealmBuilderError::ChildAlreadyExists
                    | ftest::RealmBuilderError::InvalidManifestExtension
                    | ftest::RealmBuilderError::InvalidComponentDecl
                    | ftest::RealmBuilderError::NoSuchChild
                    | ftest::RealmBuilderError::ChildDeclNotVisible
                    | ftest::RealmBuilderError::NoSuchSource
                    | ftest::RealmBuilderError::NoSuchTarget
                    | ftest::RealmBuilderError::CapabilitiesEmpty
                    | ftest::RealmBuilderError::TargetsEmpty
                    | ftest::RealmBuilderError::SourceAndTargetMatch
                    | ftest::RealmBuilderError::DeclNotFound
                    | ftest::RealmBuilderError::CapabilityInvalid
                    | ftest::RealmBuilderError::ImmutableProgram,
                ) => zx::Status::INVALID_ARGS,
                // The following types of realm builder errors are unlikely to be attributable to
                // the client, and are more likely to indicate e.g. a transport error or an
                // unexpected failure in the underlying system.
                fcomponent::error::Error::FidlError(e) => {
                    let _: fidl::Error = e;
                    zx::Status::INTERNAL
                }
                fcomponent::error::Error::FailedToOpenPkgDir(_)
                | fcomponent::error::Error::ConnectToServer(anyhow::Error { .. })
                | fcomponent::error::Error::FailedToCreateChild(anyhow::Error { .. })
                | fcomponent::error::Error::FailedToDestroyChild(anyhow::Error { .. })
                | fcomponent::error::Error::FailedToBind(anyhow::Error { .. }) => {
                    zx::Status::INTERNAL
                }
                fcomponent::error::Error::ServerError(e) => {
                    let _: ftest::RealmBuilderError = e;
                    zx::Status::INTERNAL
                }
                fcomponent::error::Error::RefUsedInWrongRealm(
                    fcomponent::Ref { .. },
                    String { .. },
                ) => zx::Status::INTERNAL,
                fcomponent::error::Error::DestroyWaiterTaken
                | fcomponent::error::Error::MissingSource
                | fcomponent::error::Error::CannotStartRootComponent(_)
                | fcomponent::error::Error::FromDictionaryNotSupported(_) => zx::Status::INTERNAL,
            },
        }
    }
}

struct StorageVariant(fnetemul::StorageVariant);

impl std::fmt::Display for StorageVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = match self {
            Self(fnetemul::StorageVariant::Data) => "data",
            Self(fnetemul::StorageVariant::Cache) => "cache",
            Self(fnetemul::StorageVariant::Tmp) => "tmp",
            Self(fnetemul::StorageVariant::CustomArtifacts) => "custom_artifacts",
        };
        write!(f, "{}", v)
    }
}

#[derive(Debug, Eq, Hash, PartialEq)]
enum UniqueCapability<'a> {
    DevFs { name: Cow<'a, str> },
    Protocol { proto_name: Cow<'a, str> },
    Configuration { name: Cow<'a, str> },
    Storage { mount_path: Cow<'a, str> },
    Service { service_name: Cow<'a, str> },
}

impl<'a> UniqueCapability<'a> {
    fn new_protocol<P: DiscoverableProtocolMarker>() -> Self {
        Self::Protocol { proto_name: P::PROTOCOL_NAME.into() }
    }
}

type ExposedProtocols = HashMap<String, Vec<String>>;

async fn create_realm_instance(
    RealmOptions { name, children, .. }: RealmOptions,
    prefix: &str,
    devfs: Arc<SimpleImmutableDir>,
    devfs_proxy: fio::DirectoryProxy,
    hwnetwork_service_proxy: fio::DirectoryProxy,
    custom_artifacts_proxy: fio::DirectoryProxy,
) -> Result<(RealmInstance, ExposedProtocols), CreateRealmError> {
    // Keep track of dependencies between child components in the test realm in order to create the
    // relevant routes at the end. RealmBuilder doesn't allow creating routes between components if
    // the components haven't both been created yet, so we wait until all components have been
    // created to add routes between them.
    let mut child_dep_routes = Vec::new();
    // Keep track of all components with modified program arguments, so that once the realm is built
    // those components can be extracted and modified.
    let mut modified_program_args = HashMap::new();

    let realm_name =
        name.map(|name| format!("{}-{}", prefix, name)).unwrap_or_else(|| prefix.to_string());
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new()
            .in_collection(REALM_COLLECTION_NAME.to_string())
            .realm_name(realm_name.clone()),
    )
    .await?;
    let custom_artifacts_clone = Clone::clone(&custom_artifacts_proxy);
    let netemul_services = builder
        .add_local_child(
            NETEMUL_SERVICES_COMPONENT_NAME,
            move |handles: LocalComponentHandles| {
                let devfs_proxy = Clone::clone(&devfs_proxy);
                let custom_artifacts_proxy = Clone::clone(&custom_artifacts_clone);
                let hwnetwork_service_proxy = Clone::clone(&hwnetwork_service_proxy);
                Box::pin(async {
                    let mut fs = ServiceFs::new();
                    let _ = fs
                        .dir("svc")
                        .add_remote(fhwnet::ServiceMarker::SERVICE_NAME, hwnetwork_service_proxy);
                    fs.add_remote(DEVFS, devfs_proxy)
                        .add_remote(CUSTOM_ARTIFACTS_CAPABILITY, custom_artifacts_proxy)
                        .serve_connection(handles.outgoing_dir)?
                        .collect::<()>()
                        .await;
                    Ok(())
                })
            },
            ChildOptions::new(),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::service_by_name(fhwnet::ServiceMarker::SERVICE_NAME).as_(
                    generate_protocol_name_alias(
                        fhwnet::ServiceMarker::SERVICE_NAME,
                        NETEMUL_SERVICES_COMPONENT_NAME,
                    ),
                ))
                .from(&netemul_services)
                .to(Ref::parent()),
        )
        .await?;

    // Keep track of the protocols that exist in the realm along with the children that offer
    // that protocol.
    let mut capability_from_children: ExposedProtocols = HashMap::from_iter(std::iter::once((
        fhwnet::ServiceMarker::SERVICE_NAME.to_string(),
        vec![NETEMUL_SERVICES_COMPONENT_NAME.to_string()],
    )));
    let mut children_with_uses = vec![];
    for ChildDef {
        source,
        name,
        exposes,
        uses,
        program_args,
        eager,
        config_values,
        __source_breaking: fidl::marker::SourceBreaking,
    } in children.unwrap_or_default()
    {
        let source = source.ok_or(CreateRealmError::SourceNotProvided)?;
        let name = name.ok_or(CreateRealmError::NameNotProvided)?;
        let diagnostics_name = format!("{name}-diagnostics");
        let mut child = ChildOptions::new();
        if eager.unwrap_or(false) {
            child = child.eager();
        }
        let child_ref = match source {
            fnetemul::ChildSource::Component(url) => {
                builder.add_child(&name, &url, child).await.map_err(|e| {
                    error!("error adding child {name} with URL: {url}: {e:?}");
                    e
                })?
            }
            fnetemul::ChildSource::Mock(dir) => {
                let dir = dir.into_proxy();
                builder
                    .add_local_child(
                        &name,
                        move |mock_handles: LocalComponentHandles| {
                            futures::future::ready(
                                dir.clone(mock_handles.outgoing_dir.into_channel().into())
                                    .context("cloning directory for mock handles"),
                            )
                            // The lifetime of the mock child component is tied
                            // to that of this future. Make the future never
                            // return, so that the mock child is kept alive.
                            .and_then(|()| futures::future::pending())
                            .boxed()
                        },
                        child,
                    )
                    .await?
            }
        };
        if let Some(program_args) = program_args {
            // This assertion should always pass because `RealmBuilder::add_child` will have
            // failed already if a component with the same moniker already exists in the realm.
            assert_eq!(modified_program_args.insert(name.clone(), program_args), None);
        }
        if let Some(exposes) = exposes {
            for exposed in exposes {
                let offering_children: &mut Vec<String> =
                    capability_from_children.entry(exposed.clone()).or_default();
                // Surface an error if two of the same child exists, or a child
                // exposes more than one instance of the same protocol.
                if offering_children.contains(&name) {
                    return Err(CreateRealmError::DuplicateProtocolExposed(exposed.clone(), name));
                }
                // Add the protocol under the generated alias.
                let aliased_protocol = generate_protocol_name_alias(&exposed, &name);
                builder
                    .add_route(
                        Route::new()
                            .capability(Capability::protocol_by_name(exposed).as_(aliased_protocol))
                            .from(&child_ref)
                            .to(Ref::parent()),
                    )
                    .await?;
                offering_children.push(name.clone());
            }
        }
        // TODO(https://fxbug.dev/324494668): delete when Netstack2 is gone.
        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::directory("diagnostics")
                            .path("/diagnostics")
                            .rights(fio::R_STAR_DIR)
                            .as_(diagnostics_name.clone()),
                    )
                    .from(&child_ref)
                    .to(Ref::parent()),
            )
            .await?;
        let config_values = config_values.unwrap_or_default();
        if !config_values.is_empty() {
            builder.init_mutable_config_from_package(&child_ref).await?;
            for fnetemul::ChildConfigValue { key, value } in config_values {
                builder.set_config_value(&child_ref, &key, value.fidl_into_native()).await?;
            }
        }
        children_with_uses.push((name, uses));
    }
    // Iterate through the "uses" statements after we iterate through all the "exposes"
    // statements, so that if a capability specifies `dynamically_offer_from_void` then
    // there is enough information to infer whether the capability is present.
    for (name, uses) in children_with_uses {
        let child_ref = fuchsia_component_test::ChildRef::from(name.clone());
        if let Some(uses) = uses {
            match uses {
                ChildUses::Capabilities(caps) => {
                    // TODO(https://github.com/rust-lang/rust/issues/60896): use std's HashSet.
                    type HashSet<T> = HashMap<T, ()>;
                    let mut unique_caps = HashSet::new();
                    for cap in caps {
                        // TODO(https://fxbug.dev/42157043): consider introducing an abstraction here
                        // over the (fnetemul::Capability, CapabilityRoute, String) triple that is
                        // defined here for each of the built-in netemul capabilities, corresponding
                        // to their FIDL representation, routing logic, and capability name.
                        let cap = match cap {
                            fnetemul::Capability::NetemulDevfs(fnetemul::DevfsDep {
                                name: capability_name,
                                subdir,
                                ..
                            }) => {
                                let capability_name = capability_name
                                    .ok_or(CreateRealmError::DevfsCapabilityNameNotProvided)?;
                                if let Some(subdir) = subdir.as_ref() {
                                    let _: Arc<SimpleImmutableDir> = open_or_create_dir(
                                        devfs.clone(),
                                        &std::path::Path::new(subdir),
                                    )
                                    .await
                                    .map_err(|e| {
                                        error!(
                                            "failed to create subdirectory '{}' in devfs: {}",
                                            subdir, e
                                        );
                                        CreateRealmError::InvalidDevfsSubdirectory(
                                            subdir.to_string(),
                                        )
                                    })?;
                                }
                                let mut capability = Capability::directory(DEVFS_CAPABILITY)
                                    .rights(fio::R_STAR_DIR)
                                    .path(DEVFS_PATH)
                                    .as_(capability_name.clone());
                                if let Some(subdir) = subdir {
                                    capability = capability.subdir(subdir);
                                }
                                builder
                                    .add_route(
                                        Route::new()
                                            .capability(capability)
                                            .from(&netemul_services)
                                            .to(&child_ref),
                                    )
                                    .await?;
                                UniqueCapability::DevFs { name: capability_name.into() }
                            }
                            fnetemul::Capability::NetemulNetworkContext(fnetemul::Empty {}) => {
                                builder
                                    .add_route(
                                        Route::new()
                                            .capability(Capability::protocol::<
                                                fnetemul_network::NetworkContextMarker,
                                            >(
                                            ))
                                            .from(Ref::parent())
                                            .to(&child_ref),
                                    )
                                    .await?;
                                UniqueCapability::new_protocol::<
                                    fnetemul_network::NetworkContextMarker,
                                >()
                            }
                            fnetemul::Capability::LogSink(fnetemul::Empty {}) => {
                                builder
                                    .add_route(
                                        Route::new()
                                            .capability(Capability::protocol::<
                                                flogger::LogSinkMarker,
                                            >(
                                            ))
                                            .from(Ref::parent())
                                            .to(&child_ref),
                                    )
                                    .await?;
                                UniqueCapability::new_protocol::<flogger::LogSinkMarker>()
                            }
                            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                                name: source,
                                capability,
                                is_weak,
                                dynamically_offer_from_void,
                                ..
                            }) => {
                                let exposed_capability = capability
                                    .ok_or(CreateRealmError::CapabilityNameNotProvided)?;
                                let source = |name| {
                                    source
                                        .and_then(|source| {
                                            // If the `dynamically_offer_from_void` option is set
                                            // for the Realm, when there is a child in the realm
                                            // offering the capability, route the capability from
                                            // that child. Otherwise, route the capability from
                                            // void and mark it as optional.
                                            // Don't consider offering from void.
                                            if !dynamically_offer_from_void.unwrap_or(false) {
                                                return Some(Ref::child(source));
                                            }
                                            // Offer from void if we don't have offering chidlren.
                                            capability_from_children.get(name).and_then(
                                                |offering_children| {
                                                    offering_children
                                                        .contains(&source)
                                                        .then_some(Ref::child(source))
                                                },
                                            )
                                        })
                                        .unwrap_or_else(|| Ref::void())
                                };
                                let is_weak = is_weak.unwrap_or(false);
                                match exposed_capability {
                                    fnetemul::ExposedCapability::Protocol(capability) => {
                                        let source = source(&capability);
                                        debug!(
                                            "{}routing capability '{}' from component '{}' to '{}'",
                                            if is_weak { "weakly " } else { "" },
                                            capability,
                                            source,
                                            name
                                        );
                                        child_dep_routes.push(
                                            Route::new()
                                                .capability({
                                                    let mut cap =
                                                        Capability::protocol_by_name(&capability);
                                                    if source == Ref::void() {
                                                        cap = cap.optional();
                                                    }
                                                    if is_weak { cap.weak() } else { cap }
                                                })
                                                .from(source)
                                                .to(&child_ref),
                                        );
                                        UniqueCapability::Protocol { proto_name: capability.into() }
                                    }
                                    fnetemul::ExposedCapability::Configuration(capability) => {
                                        let source = source(&capability);
                                        if is_weak {
                                            error!(
                                                "is_weak is unsupported for capabilities of type \
                                                Configuration"
                                            );
                                            return Err(CreateRealmError::WeakBindingUnsupported(
                                                capability,
                                            ));
                                        }
                                        debug!(
                                            "routing capability '{}' from component '{}' to '{}'",
                                            capability, source, name
                                        );
                                        child_dep_routes.push(
                                            Route::new()
                                                .capability(Capability::configuration(&capability))
                                                .from(source)
                                                .to(&child_ref),
                                        );
                                        UniqueCapability::Configuration { name: capability.into() }
                                    }
                                    fnetemul::ExposedCapability::Service(capability) => {
                                        let source = source(&capability);
                                        if is_weak {
                                            error!(
                                                "is_weak is unsupported for capabilities of type \
                                                Service"
                                            );
                                            return Err(CreateRealmError::WeakBindingUnsupported(
                                                capability,
                                            ));
                                        }
                                        debug!(
                                            "routing capability '{}' from component '{}' to '{}'",
                                            capability, source, name
                                        );
                                        child_dep_routes.push(
                                            Route::new()
                                                .capability(Capability::service_by_name(
                                                    &capability,
                                                ))
                                                .from(source)
                                                .to(&child_ref),
                                        );
                                        UniqueCapability::Service {
                                            service_name: capability.into(),
                                        }
                                    }
                                }
                            }
                            fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                                variant,
                                path,
                                ..
                            }) => {
                                let variant = variant
                                    .ok_or(CreateRealmError::StorageCapabilityVariantNotProvided)?;

                                let mount_path =
                                    path.ok_or(CreateRealmError::StorageCapabilityPathNotProvided)?;

                                let route = match variant {
                                    fnetemul::StorageVariant::Data
                                    | fnetemul::StorageVariant::Cache
                                    | fnetemul::StorageVariant::Tmp => {
                                        let capability = Capability::storage(
                                            StorageVariant(variant).to_string(),
                                        )
                                        .path(mount_path.to_string());
                                        Route::new()
                                            .capability(capability)
                                            .from(Ref::parent())
                                            .to(&child_ref)
                                    }
                                    // We proxy `custom_artifacts` storage as a directory to the
                                    // managed realm so that dynamically created components can
                                    // write artifacts there without them being deleted when the
                                    // realm is torn down, which is what happens by default for per-
                                    // component storage.
                                    //
                                    // TODO(https://fxbug.dev/378163044): when it is supported,
                                    // enable persistent storage for the netemul component
                                    // collection to prevent custom artifacts being destroyed, and
                                    // directly route the `custom_artifacts` storage capability into
                                    // the managed realm without a proxy.
                                    fnetemul::StorageVariant::CustomArtifacts => {
                                        // Create an isolated directory for this component so it
                                        // doesn't overwrite custom artifacts from other components.
                                        let dir_name = format!("{realm_name}-{name}");
                                        let _dir = fuchsia_fs::directory::create_directory(
                                            &custom_artifacts_proxy,
                                            &dir_name,
                                            fio::PERM_READABLE | fio::PERM_WRITABLE,
                                        )
                                        .await
                                        .expect("open isolated custom artifacts directory");

                                        let capability =
                                            Capability::directory(CUSTOM_ARTIFACTS_CAPABILITY)
                                                .rights(fio::RW_STAR_DIR)
                                                .subdir(&dir_name)
                                                .path(CUSTOM_ARTIFACTS_PATH)
                                                .as_(CUSTOM_ARTIFACTS_CAPABILITY);
                                        Route::new()
                                            .capability(capability)
                                            .from(&netemul_services)
                                            .to(&child_ref)
                                    }
                                };

                                builder.add_route(route).await?;
                                UniqueCapability::Storage { mount_path: mount_path.into() }
                            }
                            fnetemul::Capability::TracingProvider(fnetemul::Empty) => {
                                builder
                                    .add_route(
                                        Route::new()
                                            .capability(Capability::protocol::<
                                                ftracing_provider::RegistryMarker,
                                            >(
                                            ))
                                            .from(Ref::parent())
                                            .to(&child_ref),
                                    )
                                    .await?;
                                UniqueCapability::new_protocol::<ftracing_provider::RegistryMarker>(
                                )
                            }
                        };
                        match unique_caps.entry(cap) {
                            Entry::Occupied(entry) => {
                                let (cap, ()) = entry.remove_entry();
                                return Err(CreateRealmError::DuplicateCapabilityUse(
                                    format!("{:?}", cap),
                                    name,
                                ));
                            }
                            Entry::Vacant(entry) => {
                                let _: &mut () = entry.insert(());
                            }
                        }
                    }
                }
            }
        }
    }
    for route in child_dep_routes {
        builder.add_route(route).await?;
    }
    // Override the program args section of the component declaration for components that specified
    // args.
    for (component, program_args) in modified_program_args {
        let mut decl = builder.get_component_decl(component.as_str()).await?;
        let cm_rust::ComponentDecl { program, .. } = &mut decl;
        // Create `program` if it is None.
        let cm_rust::ProgramDecl { runner: _, info } = if let Some(program) = program.as_mut() {
            program
        } else {
            return Err(CreateRealmError::ModifiedNonexistentProgram(component));
        };
        let fdata::Dictionary { entries, .. } = info;
        // Create `entries` if it is None.
        let entries = entries.get_or_insert_with(|| Vec::default());
        // Create an "args" entry if there is none and replace whatever is currently in the "args"
        // entry with the program arguments passed in.
        const ARGS_KEY: &str = "args";
        let args_value = Some(Box::new(fdata::DictionaryValue::StrVec(program_args)));
        match entries.iter_mut().find_map(
            |fdata::DictionaryEntry { key, value }| {
                if key == ARGS_KEY { Some(value) } else { None }
            },
        ) {
            Some(args) => *args = args_value,
            None => {
                entries
                    .push(fdata::DictionaryEntry { key: ARGS_KEY.to_string(), value: args_value });
            }
        };
        builder.replace_component_decl(component.as_str(), decl).await?;
    }

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fsys2::LifecycleControllerMarker>())
                .from(Ref::framework())
                .to(Ref::parent()),
        )
        .await?;

    info!("creating new ManagedRealm with name '{realm_name}'");
    builder.build().await.map(|realm| (realm, capability_from_children)).map_err(Into::into)
}

// Form the aliased protocol name using the name of the protocol and the name
// of the component that exposes the protocol. Protocol names are aliased as
// `{protocol_name}_{child_name}` to avoid naming collisions when the same
// protocol is offered by multiple children.
fn generate_protocol_name_alias(protocol_name: &str, component_source: &str) -> String {
    format!("{protocol_name}_{component_source}")
}

fn resolve_capability_path(
    protocol_name: &str,
    child_name: Option<String>,
    capability_from_children: &ExposedProtocols,
) -> String {
    match child_name {
        Some(child) => generate_protocol_name_alias(protocol_name, &child),
        None => match capability_from_children
            .get(protocol_name)
            .map(|children| children.first())
            .flatten()
        {
            Some(child) => generate_protocol_name_alias(protocol_name, &child),
            None => protocol_name.to_string(),
        },
    }
}

struct ManagedRealm {
    scope: vfs::execution_scope::ExecutionScope,
    server_end: ServerEnd<ManagedRealmMarker>,
    realm: RealmInstance,
    devfs: Arc<SimpleImmutableDir>,
    hwnetwork_service: Arc<SimpleImmutableDir>,
    capability_from_children: ExposedProtocols,
}

// This represents a device in devfs. It can serve both the device's FIDL protocol as well
// as fuchsia.device/Controller protocol.
// As a DirectoryEntry, the DevfsDevice acts as both a directory, and a protocol (similar to how
// devfs works).
struct DevfsDevice {
    device: fidl_fuchsia_netemul_network::DeviceProxy_Proxy,
    path: String,
}

// TODO(https://fxbug.dev/326325522): This is an abuse of directories.
impl RemoteLike for DevfsDevice {
    fn open(
        self: Arc<Self>,
        _scope: vfs::execution_scope::ExecutionScope,
        path: vfs::path::Path,
        _flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), zx::Status> {
        // If we are opening the device directly we get the device protocol.
        if path.is_dot() || path.is_empty() {
            self.device.serve_device(object_request.take().into_server_end()).map_err(|e| {
                error!("failed to serve device on path {}: {}", self.path, e);
                zx::Status::INTERNAL
            })?;
            return Ok(());
        }
        // If we are opening "device_controller" then we get fuchsia.device/Controller.
        if path.as_ref() == "device_controller" {
            self.device.serve_controller(object_request.take().into_server_end()).map_err(|e| {
                error!("failed to serve controller on path {}: {}", self.path, e);
                zx::Status::INTERNAL
            })?;
            return Ok(());
        }

        // Failed to serve device or controller
        error!("failed to serve device or controller: Bad path {}", path.as_ref());
        Err(zx::Status::BAD_PATH)
    }
}

impl vfs::directory::entry::DirectoryEntry for DevfsDevice {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
        request.open_remote(self)
    }
}

impl vfs::directory::entry::GetEntryInfo for DevfsDevice {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(1, fio::DirentType::Directory)
    }
}

fn realm_moniker(realm: &RealmInstance) -> String {
    format!("{}:{}", REALM_COLLECTION_NAME, realm.root.child_name())
}

async fn add_hwnetwork_service(
    hwnetwork_service: &Arc<SimpleImmutableDir>,
    instance_name: String,
    device_proxy: fnetemul_network::DeviceProxy_Proxy,
) -> Result<(), zx::Status> {
    let instance_dir = vfs::directory::immutable::simple();
    hwnetwork_service.add_entry(&instance_name, instance_dir.clone()).inspect_err(|err| {
        error!("failed to create service for {instance_name}: {err:?}");
    })?;
    let (device_controller, device_controller_server_end) = fidl::endpoints::create_proxy();
    device_proxy.serve_controller(device_controller_server_end).map_err(|err| {
        error!(
            "failed to serve controller on path svc/{}/{}/{}: {:?}",
            fhwnet::ServiceMarker::SERVICE_NAME,
            instance_name,
            DEVICE_TOPOLOGY_MEMBER_NAME,
            err
        );
        zx::Status::INTERNAL
    })?;
    instance_dir
        .add_entry(
            HWNETWORK_SERVICE_MEMBER_NAME,
            vfs::service::endpoint(move |_scope, request| {
                match device_proxy.serve_device(request.into_zx_channel().into()) {
                    Err(e) => {
                        error!(
                            "failed to serve device on path svc/{}/{}/{}: {}",
                            fhwnet::ServiceMarker::SERVICE_NAME,
                            instance_name,
                            HWNETWORK_SERVICE_MEMBER_NAME,
                            e
                        );
                    }
                    Ok(()) => {}
                }
            }),
        )
        .expect("adding the member service should always succeed");
    // TODO(https://fxbug.dev/501161302): Remove or replace once netcfg no
    // longer needs topo path.
    instance_dir
        .add_entry(
            DEVICE_TOPOLOGY_MEMBER_NAME,
            vfs::service::host::<fidl_fuchsia_device_fs::TopologicalPathRequestStream, _, _>(
                move |requests| {
                    requests
                        .try_for_each({
                            let device_controller = device_controller.clone();
                            move |request| {
                                let device_controller = device_controller.clone();
                                async move {
                                    use fidl_fuchsia_device_fs::TopologicalPathRequest;
                                    match request {
                                        TopologicalPathRequest::GetTopologicalPath {
                                            responder,
                                        } => {
                                            match device_controller.get_topological_path().await? {
                                                Ok(path) => responder.send(Ok(&path)),
                                                Err(err) => responder.send(Err(err)),
                                            }
                                        }
                                    }
                                }
                            }
                        })
                        .map(|result| {
                            result.unwrap_or_else(|err| {
                                error!("error while serving device_topology: {err:?}");
                            })
                        })
                },
            ),
        )
        .expect("adding the device topology should always succeed");
    Ok(())
}

impl ManagedRealm {
    async fn run_service(self) -> Result {
        let Self { scope, server_end, realm, devfs, hwnetwork_service, capability_from_children } =
            self;
        let mut stream = server_end.into_stream();
        while let Some(request) = stream.try_next().await.context("FIDL error")? {
            match request {
                ManagedRealmRequest::GetMoniker { responder } => {
                    let moniker = realm_moniker(&realm);
                    responder.send(&moniker).context("responding to GetMoniker request")?;
                }
                ManagedRealmRequest::ConnectToProtocol {
                    protocol_name,
                    child_name,
                    req,
                    control_handle: _,
                } => {
                    let protocol_path = resolve_capability_path(
                        &protocol_name,
                        child_name,
                        &capability_from_children,
                    );
                    realm
                        .root
                        .get_exposed_dir()
                        .open(
                            &protocol_path,
                            fio::Flags::PROTOCOL_SERVICE,
                            &Default::default(),
                            req,
                        )
                        .with_context(|| {
                            format!("failed to open protocol {} in directory", protocol_path)
                        })?;
                }
                ManagedRealmRequest::ConnectToService {
                    service_name,
                    child_name,
                    req,
                    control_handle: _,
                } => {
                    let service_path = resolve_capability_path(
                        &service_name,
                        child_name,
                        &capability_from_children,
                    );
                    realm
                        .root
                        .get_exposed_dir()
                        .open(
                            &service_path,
                            fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                            &Default::default(),
                            req,
                        )
                        .with_context(|| {
                            format!("failed to open service {} in directory", service_path)
                        })?;
                }
                ManagedRealmRequest::GetDevfs { devfs: server_end, control_handle: _ } => {
                    // On errors `server_end` will be closed with an epitaph.
                    vfs::directory::serve_on(
                        devfs.clone(),
                        fio::PERM_READABLE,
                        scope.clone(),
                        server_end,
                    );
                }
                ManagedRealmRequest::AddDevice { path, device, responder } => {
                    // ClientEnd::into_proxy should only return an Err when there is no executor, so
                    // this is not expected to ever cause a panic.
                    let device = device.into_proxy();
                    let devfs = devfs.clone();
                    let hwnetwork_service = hwnetwork_service.clone();
                    let response = (|| async move {
                        let (parent_path, device_name) =
                            split_path_into_dir_and_file_name(&std::path::Path::new(&path))
                                .map_err(|e| {
                                    error!(
                                        "failed to split path '{}' into directory and filename: {}",
                                        path, e
                                    );
                                    zx::Status::INVALID_ARGS
                                })?;
                        let dir = open_or_create_dir(devfs, parent_path).await.map_err(|e| {
                            error!("failed to open or create path '{}': {}", path, e);
                            zx::Status::INVALID_ARGS
                        })?;
                        let response = dir.add_entry(
                            device_name,
                            Arc::new(DevfsDevice { device: device.clone(), path: path.clone() }),
                        );
                        match response {
                            Ok(()) => {
                                info!("adding virtual device at path '{}/{}'", DEVFS_PATH, path)
                            }
                            Err(e) => {
                                if e == zx::Status::ALREADY_EXISTS {
                                    warn!(
                                        "cannot add device at path '{}/{}': path is already in use",
                                        DEVFS_PATH, path
                                    )
                                } else {
                                    error!(
                                        "unexpected error adding entry at path '{}/{}': {}",
                                        DEVFS_PATH, path, e
                                    )
                                }
                            }
                        }
                        response?;
                        add_hwnetwork_service(
                            &hwnetwork_service,
                            device_name.to_string(),
                            device.clone(),
                        )
                        .await
                    })()
                    .await;
                    responder
                        .send(response.map_err(zx::Status::into_raw))
                        .context("responding to AddDevice request")?;
                }
                ManagedRealmRequest::RemoveDevice { path, responder } => {
                    let devfs = devfs.clone();
                    let hwnetwork_service = hwnetwork_service.clone();
                    let response = (|| async move {
                        let (parent_path, device_name) =
                            split_path_into_dir_and_file_name(&std::path::Path::new(&path))
                                .map_err(|e| {
                                    error!(
                                        "failed to split path '{}' into directory and filename: {}",
                                        path, e
                                    );
                                    zx::Status::INVALID_ARGS
                                })?;
                        let dir = open_or_create_dir(devfs, parent_path).await.map_err(|e| {
                            error!("failed to open or create path '{}': {}", path, e);
                            zx::Status::INVALID_ARGS
                        })?;
                        let response = match dir.remove_entry(device_name, false) {
                            Ok(entry) => {
                                if let Some(entry) = entry {
                                    let _: Arc<_> = entry;
                                    info!(
                                        "removing virtual device at path '{}/{}'",
                                        DEVFS_PATH, path
                                    );
                                    Ok(())
                                } else {
                                    warn!(
                                        "cannot remove device at path '{}/{}': path is not \
                                        currently bound to a device",
                                        DEVFS_PATH, path,
                                    );
                                    Err(zx::Status::NOT_FOUND)
                                }
                            }
                            Err(e) => {
                                error!(
                                    "error removing device at path '{}/{}': {}",
                                    DEVFS_PATH, path, e
                                );
                                Err(e)
                            }
                        };
                        response?;
                        match hwnetwork_service
                            .remove_entry(device_name, true /* must be directory */)?
                        {
                            Some(_) => Ok(()),
                            None => Err(zx::Status::NOT_FOUND),
                        }
                    })()
                    .await;
                    responder
                        .send(response.map_err(zx::Status::into_raw))
                        .context("responding to RemoveDevice request")?;
                }
                ManagedRealmRequest::StartChildComponent { child_name, responder } => {
                    let response = async {
                        let lifecycle =
                            fuchsia_component::client::connect_to_protocol_at_dir_root::<
                                fsys2::LifecycleControllerMarker,
                            >(realm.root.get_exposed_dir())
                            .map_err(|e: anyhow::Error| {
                                error!("failed to open proxy to lifecycle controller: {}", e);
                                zx::Status::INTERNAL
                            })?;
                        let (_client, server) = fidl::endpoints::create_endpoints();
                        lifecycle
                            .start_instance(&format!("./{}", child_name), server)
                            .await
                            .map_err(|e: fidl::Error| {
                                error!("failed to call LifecycleController/StartInstance: {}", e);
                                zx::Status::INTERNAL
                            })?
                            .map_err(|e| {
                                warn!("failed to start child component '{}': {:?}", child_name, e);
                                match e {
                                    fsys2::StartError::InstanceNotFound
                                    | fsys2::StartError::PackageNotFound
                                    | fsys2::StartError::ManifestNotFound => zx::Status::NOT_FOUND,
                                    fsys2::StartError::BadMoniker => zx::Status::INVALID_ARGS,
                                    fsys2::StartError::Internal => zx::Status::INTERNAL,
                                    other => unreachable!(
                                        "unrecognized fuchsia.sys2/StartError variant: {:?}",
                                        other,
                                    ),
                                }
                            })?;
                        Ok(())
                    }
                    .await;
                    responder
                        .send(response.map_err(zx::Status::into_raw))
                        .context("responding to StartChildComponent request")?;
                }
                ManagedRealmRequest::StopChildComponent { child_name, responder } => {
                    let response = async {
                        let lifecycle =
                            fuchsia_component::client::connect_to_protocol_at_dir_root::<
                                fsys2::LifecycleControllerMarker,
                            >(realm.root.get_exposed_dir())
                            .map_err(|e: anyhow::Error| {
                                error!("failed to open proxy to lifecycle controller: {}", e);
                                zx::Status::INTERNAL
                            })?;
                        lifecycle
                            .stop_instance(&format!("./{}", child_name))
                            .await
                            .map_err(|e: fidl::Error| {
                                error!("failed to call LifecycleController/StopInstance: {}", e);
                                zx::Status::INTERNAL
                            })?
                            .map_err(|e| {
                                warn!("failed to stop child component '{}': {:?}", child_name, e);
                                match e {
                                    fsys2::StopError::InstanceNotFound => zx::Status::NOT_FOUND,
                                    fsys2::StopError::BadMoniker => zx::Status::INVALID_ARGS,
                                    fsys2::StopError::Internal => zx::Status::INTERNAL,
                                    other => unreachable!(
                                        "unrecognized fuchsia.sys2/StopError variant: {:?}",
                                        other,
                                    ),
                                }
                            })?;
                        Ok(())
                    }
                    .await;
                    responder
                        .send(response.map_err(zx::Status::into_raw))
                        .context("responding to StopChildComponent request")?;
                }
                ManagedRealmRequest::OpenDiagnosticsDirectory {
                    child_name,
                    directory,
                    control_handle: _,
                } => {
                    let flags = fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY;
                    realm
                        .root
                        .get_exposed_dir()
                        .open(
                            &format!("{child_name}-diagnostics"),
                            flags,
                            &Default::default(),
                            directory.into_channel(),
                        )
                        .context(format!("opening diagnostics dir for {child_name}"))?;
                }
                ManagedRealmRequest::GetCrashListener { listener, control_handle: _ } => {
                    // We must create the event stream now, to ensure that it's
                    // going to observe any new things happening to the realm
                    // (like shutdown) after this point given we spawn the
                    // listener to its own task.
                    let event_stream =
                        component_events::events::EventStream::open_at_path("/events/stopped")
                            .await
                            .context("failed to subscribe to `Stopped` events")?;
                    CrashListener {
                        realm_moniker: realm_moniker(&realm),
                        server_end: listener,
                        event_stream,
                    }
                    .spawn();
                }
                ManagedRealmRequest::Shutdown { control_handle } => {
                    realm.destroy().await.context("destroy realm")?;
                    control_handle
                        .send_on_shutdown()
                        .unwrap_or_else(|e| error!("failed to send OnShutdown event: {:?}", e));
                    break;
                }
            }
        }
        Ok(())
    }
}

struct CrashListener {
    event_stream: component_events::events::EventStream,
    realm_moniker: String,
    server_end: fidl::endpoints::ServerEnd<fnetemul::CrashListenerMarker>,
}

impl CrashListener {
    fn spawn(self) {
        // Given the listeners don't really hold any resources that need
        // synchronization. We can just spawn them on the current scope in fully
        // detached state. They can observe parent realm termination by the
        // event stream they create themselves.
        let _: fasync::JoinHandle<()> = fasync::Scope::current().spawn(async move {
            self.serve().await.unwrap_or_else(|e| error!("error serving CrashListener: {e:?}"))
        });
    }

    fn event_to_crash_moniker(
        event: fidl_fuchsia_component::Event,
        realm_moniker: &String,
        matcher: &component_events::matcher::EventMatcher,
    ) -> Result<Option<String>> {
        let descriptor = component_events::descriptor::EventDescriptor::try_from(&event)
            .context("create event descriptor")?;
        // NB: matches return an error variant
        // containing the non-matching details. We just
        // care that it didn't match.
        if matcher.matches(&descriptor).is_err() {
            return Ok(None);
        }
        let stopped =
            component_events::events::Stopped::try_from(event).context("not stopped event")?;
        let component_events::events::StoppedPayload { status, exit_code } =
            stopped.result().map_err(|e| anyhow!("stopped error event: {e:?}"))?;
        // Extract a relative moniker. Expects here are
        // guaranteed by the created matcher regex above.
        let moniker = stopped
            .target_moniker()
            .strip_prefix(realm_moniker)
            .expect("moniker should start with realm_moniker");
        // This is the realm itself shutting down, pass an
        // empty string down.
        if moniker.is_empty() {
            return Ok(Some(String::new()));
        }
        let moniker = moniker.strip_prefix('/').expect("moniker should contain separator");
        // An empty child moniker should not be allowed, something must
        // be after the separator.
        assert_ne!(moniker, "");
        info!(
            "{realm_moniker}: observed component '{moniker}' stop with \
            status={status:?} exit_code={exit_code:?}"
        );
        let bad = match (status, exit_code) {
            // A clean exit from CF means ok.
            (component_events::events::ExitStatus::Clean, _) => false,
            // A kill return code means the component
            // doesn't implement shutdown, so it was just
            // killed.
            (
                component_events::events::ExitStatus::Crash(_),
                Some(zx::sys::ZX_TASK_RETCODE_SYSCALL_KILL),
            ) => false,
            // Empty realm components with no elf backing don't generate exit
            // codes.
            (component_events::events::ExitStatus::Crash(status), None) => {
                // Components without program that get killed on shutdown
                // produce specifically this exit code. Notably the mock
                // components served by netemul itself.
                let instance_died_i32 =
                    i32::try_from(fidl_fuchsia_component::Error::InstanceDied.into_primitive())
                        .unwrap();

                *status != instance_died_i32
            }
            // Assume everything else is a bad exit.
            (component_events::events::ExitStatus::Crash(_), Some(_)) => true,
        };
        Ok(bad.then(|| moniker.to_string()))
    }

    async fn serve(self) -> Result {
        let Self { realm_moniker, server_end, event_stream } = self;

        let event_stream =
            futures::stream::try_unfold(event_stream, |mut event_stream| async move {
                let event = event_stream.next().await.context("next event")?;
                Ok::<_, anyhow::Error>(Some((event, event_stream)))
            });
        let matcher = component_events::matcher::EventMatcher::ok()
            .moniker_regex(format!("^{}", realm_moniker))
            .r#type(fidl_fuchsia_component::EventType::Stopped);
        let crash_stream = event_stream
            .try_filter_map(|event| {
                futures::future::ready(Self::event_to_crash_moniker(
                    event,
                    &realm_moniker,
                    &matcher,
                ))
            })
            .try_take_while(|moniker| futures::future::ok(!moniker.is_empty()))
            .fuse();

        let mut observed_crash = vec![];
        let mut request_stream = server_end.into_stream();
        let mut crash_stream = pin!(crash_stream);
        let mut hanging_responder = None;
        loop {
            enum Work {
                Request(Option<fnetemul::CrashListenerRequest>),
                NewData,
            }
            let work = futures::select! {
                r = request_stream.try_next() => {
                    Work::Request(r.context("serving CrashListener")?)
                },
                r = crash_stream.try_next() => {
                    match r.context("observing stop event stream")? {
                        Some(moniker) => {
                            observed_crash.push(moniker);
                        }
                        None => {}
                    }
                    Work::NewData
                }
            };

            match work {
                Work::Request(Some(req)) => {
                    let fnetemul::CrashListenerRequest::Next { responder: new_responder } = req;
                    if hanging_responder.is_some() {
                        return Err(anyhow!("called Next with hanging response already"));
                    }
                    hanging_responder = Some(new_responder);
                }
                Work::Request(None) => {
                    // Request stream terminated, the other end must've hung up
                    // before observing all output.
                    break;
                }
                Work::NewData => {}
            }

            if !observed_crash.is_empty() {
                if let Some(responder) = hanging_responder.take() {
                    responder.send(&observed_crash[..]).context("responding with crashes")?;
                    observed_crash.clear();
                }
                continue;
            }
            if crash_stream.is_terminated() {
                if let Some(responder) = hanging_responder.take() {
                    responder.send(&[]).context("responding with sentinel")?;
                    break;
                }
            }
        }
        Ok(())
    }
}

fn split_path_into_dir_and_file_name<'a>(
    path: &'a std::path::Path,
) -> Result<(&'a std::path::Path, &'a str)> {
    let file_name = path
        .file_name()
        .context("path does not end in a normal file or directory name")?
        .to_str()
        .context("invalid file name")?;
    let parent = path.parent().context("path terminates in a root")?;

    Ok((parent, file_name))
}

async fn open_or_create_dir(
    root: Arc<SimpleImmutableDir>,
    path: &std::path::Path,
) -> Result<Arc<SimpleImmutableDir>> {
    let root = futures::stream::iter(path.components())
        .map(Ok)
        .try_fold(root, |root, component| async move {
            let entry = match component {
                std::path::Component::Prefix(_) | std::path::Component::ParentDir => {
                    Err(anyhow!("path cannot contain prefix or parent component ('..')"))
                }
                component => component.as_os_str().to_str().context("invalid path component"),
            }?;
            // Get a handle to the entry, and create it if it doesn't already exist.
            let entry = match root.get_entry(entry) {
                Ok(entry) => entry,
                Err(status) => match status {
                    zx::Status::NOT_FOUND => {
                        root.add_entry(entry, vfs::directory::immutable::simple::simple())
                            .context("failed to add directory entry")?;
                        root.get_entry(entry).context("failed to get directory entry")?
                    }
                    status => {
                        return Err(anyhow!(
                            "got unexpected error on get entry '{}': expected {}, got {}",
                            entry,
                            zx::Status::NOT_FOUND,
                            status,
                        ));
                    }
                },
            };
            // Downcast the entry to a directory so that we can perform directory operations on it.
            Ok(entry
                .into_any()
                .downcast::<SimpleImmutableDir>()
                .expect("could not downcast entry to a directory"))
        })
        .await?;
    Ok(root)
}

async fn handle_sandbox(
    stream: SandboxRequestStream,
    sandbox_name: impl std::fmt::Display,
) -> Result {
    let (tx, rx) = mpsc::channel(1);
    let realm_index = AtomicU64::new(0);

    let network_context =
        fuchsia_component::client::connect_to_protocol::<fnetemul_network::NetworkContextMarker>()
            .context("connect to network context")?;
    let custom_artifacts = fuchsia_fs::directory::open_in_namespace(
        CUSTOM_ARTIFACTS_PATH,
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    )
    .context("open custom artifacts directory")?;

    let mut sandbox_fut =
        pin!(stream.err_into::<anyhow::Error>().try_for_each_concurrent(None, |request| {
            let mut tx = tx.clone();
            let sandbox_name = &sandbox_name;
            let realm_index = &realm_index;
            let network_context = &network_context;
            let custom_artifacts = Clone::clone(&custom_artifacts);
            async move {
                match request {
                    SandboxRequest::CreateRealm {
                        realm: server_end,
                        options,
                        control_handle: _,
                    } => {
                        let index = realm_index.fetch_add(1, Ordering::SeqCst);
                        let prefix = format!("{}{}", sandbox_name, index);
                        let devfs = vfs::directory::immutable::simple::simple();
                        let devfs_proxy = vfs::directory::serve_read_only(
                            devfs.clone(),
                            vfs::execution_scope::ExecutionScope::new(),
                        );
                        let hwnetwork_service = vfs::directory::immutable::simple::simple();
                        let hwnetwork_service_proxy = vfs::directory::serve_read_only(
                            hwnetwork_service.clone(),
                            vfs::execution_scope::ExecutionScope::new(),
                        );
                        match create_realm_instance(
                            options,
                            &prefix,
                            devfs.clone(),
                            devfs_proxy,
                            hwnetwork_service_proxy,
                            custom_artifacts,
                        )
                        .await
                        {
                            Ok((realm, capability_from_children)) => tx
                                .send(ManagedRealm {
                                    scope: vfs::execution_scope::ExecutionScope::new(),
                                    server_end,
                                    realm,
                                    devfs,
                                    hwnetwork_service,
                                    capability_from_children,
                                })
                                .await
                                .expect("receiver should not be closed"),
                            Err(e) => {
                                error!("error creating ManagedRealm: {}", e);
                                server_end
                                    .close_with_epitaph(zx::Status::from(e))
                                    .unwrap_or_else(|e| error!("error sending epitaph: {:?}", e))
                            }
                        }
                    }
                    SandboxRequest::GetNetworkContext {
                        network_context: server_end,
                        control_handle: _,
                    } => network_context
                        .clone(server_end)
                        .unwrap_or_else(|e| error!("error cloning NetworkContext: {:?}", e)),
                }
                Ok(())
            }
        }));
    let mut realms_fut = pin!(
        rx.for_each_concurrent(None, |realm| async {
            let name = realm.realm.root.child_name().to_owned();
            realm
                .run_service()
                .await
                .unwrap_or_else(|e| error!("error managing realm '{}': {:?}", name, e))
        })
        .fuse()
    );
    futures::select! {
        result = sandbox_fut => Ok(result?),
        () = realms_fut => unreachable!("realms_fut should never complete"),
    }
}

#[fuchsia::main()]
async fn main() -> Result {
    info!("starting...");

    let mut fs = ServiceFs::new_local();
    let _: &mut ServiceFsDir<'_, _> = fs.dir("svc").add_fidl_service(|s: SandboxRequestStream| s);
    let _: &mut ServiceFs<_> = fs.take_and_serve_directory_handle()?;

    let sandbox_index = AtomicU64::new(0);
    fs.for_each_concurrent(None, |stream| async {
        let index = sandbox_index.fetch_add(1, Ordering::SeqCst);
        handle_sandbox(stream, index)
            .await
            .unwrap_or_else(|e| error!("error handling SandboxRequestStream: {:?}", e))
    })
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_rust::NativeIntoFidl as _;
    use diagnostics_assertions::assert_data_tree;
    use fidl::endpoints::Proxy as _;
    use fidl_fuchsia_device as fdevice;
    use fidl_fuchsia_netemul as fnetemul;
    use fidl_fuchsia_netemul_test::{self as fnetemul_test, CounterMarker};
    use fixture::fixture;
    use fuchsia_component::client::Service;
    use fuchsia_fs::directory as fvfs_watcher;
    use std::convert::TryFrom as _;
    use test_case::test_case;

    // We can't just use a counter for the sandbox identifier, as we do in `main`, because tests
    // each run in separate processes, but use the same backing collection of components created
    // through `RealmBuilder`. If we used a counter, it wouldn't be shared across processes, and
    // would cause name collisions between the `RealmInstance` monikers.
    fn setup_sandbox_service(
        sandbox_name: &str,
    ) -> (fnetemul::SandboxProxy, impl futures::Future<Output = ()> + '_) {
        let (sandbox_proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fnetemul::SandboxMarker>();
        (sandbox_proxy, async move {
            handle_sandbox(stream, sandbox_name).await.expect("handle_sandbox error")
        })
    }

    async fn with_sandbox<F, Fut>(name: &str, test: F)
    where
        F: FnOnce(fnetemul::SandboxProxy) -> Fut,
        Fut: futures::Future<Output = ()>,
    {
        let (sandbox, fut) = setup_sandbox_service(name);
        let ((), ()) = futures::future::join(fut, test(sandbox)).await;
    }

    struct TestRealm {
        realm: fnetemul::ManagedRealmProxy,
    }

    impl TestRealm {
        fn new(sandbox: &fnetemul::SandboxProxy, options: fnetemul::RealmOptions) -> TestRealm {
            let (realm, server) = fidl::endpoints::create_proxy::<fnetemul::ManagedRealmMarker>();
            sandbox
                .create_realm(server, options)
                .expect("fuchsia.netemul/Sandbox.create_realm call failed");
            TestRealm { realm }
        }

        fn connect_to_protocol<S: DiscoverableProtocolMarker>(&self) -> S::Proxy {
            self._connect_to_protocol::<S>(None)
        }

        fn connect_to_service<S: fidl::endpoints::ServiceMarker>(&self, marker: S) -> Service<S> {
            let (dir_proxy, server_end) =
                fidl::endpoints::create_proxy::<fidl_fuchsia_io::DirectoryMarker>();
            self.realm
                .connect_to_service(S::SERVICE_NAME, None, server_end.into_channel())
                .expect("calling connect to service");
            Service::from_service_dir_proxy(dir_proxy, marker)
        }

        fn connect_to_protocol_from_child<S: DiscoverableProtocolMarker>(
            &self,
            child: &str,
        ) -> S::Proxy {
            self._connect_to_protocol::<S>(Some(child))
        }

        fn _connect_to_protocol<S: DiscoverableProtocolMarker>(
            &self,
            child: Option<&str>,
        ) -> S::Proxy {
            let (proxy, server_end) = fidl::endpoints::create_proxy::<S>();
            self.realm
                .connect_to_protocol(S::PROTOCOL_NAME, child, server_end.into_channel())
                .with_context(|| format!("{} from child {child:?}", S::DEBUG_NAME))
                .expect("failed to connect");
            proxy
        }
    }

    const COUNTER_COMPONENT_NAME: &str = "counter";
    const COUNTER_ALTERNATIVE_COMPONENT_NAME: &str = "counter-alternative";
    const COUNTER_URL: &str = "#meta/counter.cm";
    const COUNTER_ALTERNATIVE_URL: &str = "#meta/counter-alternative.cm";
    const COUNTER_WITHOUT_PROGRAM_URL: &str = "#meta/counter-without-program.cm";
    const COUNTER_WITH_SHUTDOWN_PROGRAM_URL: &str = "#meta/counter-with-shutdown.cm";
    const COUNTER_CONFIGURATION_NAME: &str = "fuchsia.netemul.test.Config";
    const COUNTER_A_PROTOCOL_NAME: &str = "fuchsia.netemul.test.CounterA";
    const COUNTER_B_PROTOCOL_NAME: &str = "fuchsia.netemul.test.CounterB";
    const DATA_PATH: &str = "/data";
    const CACHE_PATH: &str = "/cache";

    fn counter_config_cap() -> fnetemul::Capability {
        fnetemul::Capability::ChildDep(fnetemul::ChildDep {
            capability: Some(fidl_fuchsia_netemul::ExposedCapability::Configuration(
                COUNTER_CONFIGURATION_NAME.to_string(),
            )),
            ..Default::default()
        })
    }

    fn counter_component() -> fnetemul::ChildDef {
        fnetemul::ChildDef {
            source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
            name: Some(COUNTER_COMPONENT_NAME.to_string()),
            exposes: Some(vec![CounterMarker::PROTOCOL_NAME.to_string()]),
            uses: Some(fnetemul::ChildUses::Capabilities(vec![
                fnetemul::Capability::LogSink(fnetemul::Empty {}),
                counter_config_cap(),
            ])),
            ..Default::default()
        }
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn can_connect_to_single_protocol(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![counter_component()]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();
        assert_eq!(
            counter.increment().await.expect("fuchsia.netemul.test/Counter.increment call failed"),
            1,
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn multiple_realms(sandbox: fnetemul::SandboxProxy) {
        let realm_a = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                name: Some("a".to_string()),
                children: Some(vec![counter_component()]),
                ..Default::default()
            },
        );
        let realm_b = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                name: Some("b".to_string()),
                children: Some(vec![counter_component()]),
                ..Default::default()
            },
        );
        let counter_a = realm_a.connect_to_protocol::<CounterMarker>();
        let counter_b = realm_b.connect_to_protocol::<CounterMarker>();
        assert_eq!(
            counter_a
                .increment()
                .await
                .expect("fuchsia.netemul.test/Counter.increment call failed"),
            1,
        );
        for i in 1..=10 {
            assert_eq!(
                counter_b
                    .increment()
                    .await
                    .expect("fuchsia.netemul.test/Counter.increment call failed"),
                i,
            );
        }
        assert_eq!(
            counter_a
                .increment()
                .await
                .expect("fuchsia.netemul.test/Counter.increment call failed"),
            2,
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn use_protocol_from_first_child(sandbox: fnetemul::SandboxProxy) {
        // The starting value for the child that can specify program args.
        const STARTING_VALUE: u32 = 9000;

        // Instantiate `counter_alternative` first and the counter from
        // `connect_to_protocol` should start at 0.
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![
                    // `counter_alternative` has a starting value of 0.
                    fnetemul::ChildDef {
                        source: Some(fnetemul::ChildSource::Component(
                            COUNTER_ALTERNATIVE_URL.to_string(),
                        )),
                        name: Some(COUNTER_ALTERNATIVE_COMPONENT_NAME.to_string()),
                        exposes: Some(vec![CounterMarker::PROTOCOL_NAME.to_string()]),
                        ..Default::default()
                    },
                    fnetemul::ChildDef {
                        program_args: Some(vec![
                            "--starting-value".to_string(),
                            STARTING_VALUE.to_string(),
                        ]),
                        ..counter_component()
                    },
                ]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();
        assert_eq!(counter.increment().await.expect("failed to increment counter"), 1,);

        // Instantiate `counter_with_args` first and the counter from
        // `connect_to_protocol` should start at 9000.
        let realm2 = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![
                    // `counter_with_args` has a starting value of 9000.
                    fnetemul::ChildDef {
                        program_args: Some(vec![
                            "--starting-value".to_string(),
                            STARTING_VALUE.to_string(),
                        ]),
                        ..counter_component()
                    },
                    fnetemul::ChildDef {
                        source: Some(fnetemul::ChildSource::Component(
                            COUNTER_ALTERNATIVE_URL.to_string(),
                        )),
                        name: Some(COUNTER_ALTERNATIVE_COMPONENT_NAME.to_string()),
                        exposes: Some(vec![CounterMarker::PROTOCOL_NAME.to_string()]),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
        );
        let counter2 = realm2.connect_to_protocol::<CounterMarker>();
        assert_eq!(
            counter2.increment().await.expect("failed to increment counter"),
            STARTING_VALUE + 1,
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn use_protocol_from_specified_child(sandbox: fnetemul::SandboxProxy) {
        // The starting value for the child that can specify program args.
        const STARTING_VALUE: u32 = 9000;

        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![
                    // `counter_alternative` has a starting value of 0.
                    fnetemul::ChildDef {
                        source: Some(fnetemul::ChildSource::Component(
                            COUNTER_ALTERNATIVE_URL.to_string(),
                        )),
                        name: Some(COUNTER_ALTERNATIVE_COMPONENT_NAME.to_string()),
                        exposes: Some(vec![CounterMarker::PROTOCOL_NAME.to_string()]),
                        ..Default::default()
                    },
                    // `counter_with_args` has a starting value of 9000.
                    fnetemul::ChildDef {
                        program_args: Some(vec![
                            "--starting-value".to_string(),
                            STARTING_VALUE.to_string(),
                        ]),
                        ..counter_component()
                    },
                ]),
                ..Default::default()
            },
        );

        // Ensure that when we specify the child that exposes the protocol,
        // we're communicating with the correct one.
        let counter_alternative = realm
            .connect_to_protocol_from_child::<CounterMarker>(COUNTER_ALTERNATIVE_COMPONENT_NAME);
        assert_eq!(counter_alternative.increment().await.expect("failed to increment counter"), 1);

        let counter_with_args =
            realm.connect_to_protocol_from_child::<CounterMarker>(COUNTER_COMPONENT_NAME);
        assert_eq!(
            counter_with_args.increment().await.expect("failed to increment counter"),
            STARTING_VALUE + 1,
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn use_protocol_from_nonexistent_child(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![counter_component()]),
                ..Default::default()
            },
        );

        let counter = realm
            .connect_to_protocol_from_child::<CounterMarker>(COUNTER_ALTERNATIVE_COMPONENT_NAME);
        // Calling `increment()` on the proxy should fail because this protocol
        // only exists in `COUNTER_COMPONENT_NAME`.
        match counter.increment().await {
            Err(fidl::Error::ClientChannelClosed {
                epitaph,
                protocol_name: <CounterMarker as fidl::endpoints::ProtocolMarker>::DEBUG_NAME,
                ..
            }) if epitaph == zx::Status::NOT_FOUND => (),
            event => panic!(
                "expected channel close with epitaph NOT_FOUND, got \
                 unexpected event on realm channel: {:?}",
                event
            ),
        }
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn drop_realm_destroys_children(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![counter_component()]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();
        assert_eq!(
            counter.increment().await.expect("fuchsia.netemul.test/Counter.increment call failed"),
            1,
        );
        drop(realm);
        assert_eq!(
            counter.on_closed().await,
            Ok(zx::Signals::CHANNEL_PEER_CLOSED),
            "`CounterProxy` should be closed when `ManagedRealmProxy` is dropped",
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn shutdown_realm_destroys_children(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![counter_component()]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();
        assert_eq!(
            counter.increment().await.expect("fuchsia.netemul.test/Counter.increment call failed"),
            1,
        );
        let TestRealm { realm } = realm;
        realm.shutdown().expect("failed to call shutdown");
        let events = realm
            .take_event_stream()
            .try_collect::<Vec<_>>()
            .await
            .expect("error on realm event stream");
        // Ensure there are no more events sent on the event stream after `OnShutdown`.
        assert_matches::assert_matches!(events[..], [fnetemul::ManagedRealmEvent::OnShutdown {}]);
        assert_eq!(
            counter.on_closed().await,
            Ok(zx::Signals::CHANNEL_PEER_CLOSED),
            "counter proxy should be closed when managed realm is shut down",
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn drop_sandbox_destroys_realms(sandbox: fnetemul::SandboxProxy) {
        const REALMS_COUNT: usize = 10;
        let realms = std::iter::repeat(())
            .take(REALMS_COUNT)
            .map(|()| {
                TestRealm::new(
                    &sandbox,
                    fnetemul::RealmOptions {
                        children: Some(vec![counter_component()]),
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>();

        let mut counters = vec![];
        for realm in &realms {
            let counter = realm.connect_to_protocol::<CounterMarker>();
            assert_eq!(
                counter
                    .increment()
                    .await
                    .expect("fuchsia.netemul.test/Counter.increment call failed"),
                1,
            );
            counters.push(counter);
        }
        drop(sandbox);
        for counter in counters {
            assert_eq!(
                counter.on_closed().await,
                Ok(zx::Signals::CHANNEL_PEER_CLOSED),
                "`CounterProxy` should be closed when `SandboxProxy` is dropped",
            );
        }
        for realm in realms {
            let TestRealm { realm } = realm;
            assert_eq!(
                realm.on_closed().await,
                Ok(zx::Signals::CHANNEL_PEER_CLOSED),
                "`ManagedRealmProxy` should be closed when `SandboxProxy` is dropped",
            );
        }
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn set_realm_name(sandbox: fnetemul::SandboxProxy) {
        let TestRealm { realm } = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                name: Some("test-realm-name".to_string()),
                children: Some(vec![counter_component()]),
                ..Default::default()
            },
        );
        assert_eq!(
            realm
                .get_moniker()
                .await
                .expect("fuchsia.netemul/ManagedRealm.get_moniker call failed"),
            format!("{}:set_realm_name0-test-realm-name", REALM_COLLECTION_NAME),
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn auto_generated_realm_name(sandbox: fnetemul::SandboxProxy) {
        const REALMS_COUNT: usize = 10;
        for i in 0..REALMS_COUNT {
            let TestRealm { realm } = TestRealm::new(
                &sandbox,
                fnetemul::RealmOptions {
                    name: None,
                    children: Some(vec![counter_component()]),
                    ..Default::default()
                },
            );
            assert_eq!(
                realm
                    .get_moniker()
                    .await
                    .expect("fuchsia.netemul/ManagedRealm.get_moniker call failed"),
                format!("{}:auto_generated_realm_name{}", REALM_COLLECTION_NAME, i),
            );
        }
    }

    async fn expect_single_inspect_node<Fut: futures::Future<Output = ()>>(
        realm: &TestRealm,
        component_moniker: &str,
        f: impl Fn(diagnostics_hierarchy::DiagnosticsHierarchy) -> Fut,
    ) {
        let TestRealm { realm } = realm;
        let realm_moniker = realm.get_moniker().await.expect("failed to get moniker");
        let data = diagnostics_reader::ArchiveReader::inspect()
            .add_selector(diagnostics_reader::ComponentSelector::new(vec![
                selectors::sanitize_string_for_selectors(&realm_moniker).into_owned(),
                component_moniker.into(),
            ]))
            .snapshot()
            .await
            .expect("failed to get inspect data")
            .into_iter()
            .map(
                |diagnostics_data::InspectData {
                     data_source: _,
                     metadata: _,
                     moniker: _,
                     payload,
                     version: _,
                 }| payload,
            )
            .collect::<Vec<_>>();
        match &data[..] {
            [Some(datum)] => f(datum.clone()).await,
            data => panic!("there should be exactly one matching inspect node; found {:?}", data),
        }
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn inspect(sandbox: fnetemul::SandboxProxy) {
        const REALMS_COUNT: usize = 10;
        let realms = std::iter::repeat(())
            .take(REALMS_COUNT)
            .map(|()| {
                TestRealm::new(
                    &sandbox,
                    fnetemul::RealmOptions {
                        children: Some(vec![counter_component()]),
                        ..Default::default()
                    },
                )
            })
            // Collect the `TestRealm`s because we want all the test realms to be alive for the
            // duration of the test.
            //
            // Each `TestRealm` owns a `ManagedRealmProxy`, which has RAII semantics: when the proxy
            // is dropped, the backing test realm managed by the sandbox is also destroyed.
            .collect::<Vec<_>>();
        for (i, realm) in realms.iter().enumerate() {
            let i = u32::try_from(i).unwrap();
            let counter = realm.connect_to_protocol::<CounterMarker>();
            for j in 1..=i {
                assert_eq!(
                    counter.increment().await.unwrap_or_else(|e| panic!(
                        "fuchsia.netemul.test/Counter.increment call failed on realm {}: {:?}",
                        i, e
                    )),
                    j,
                );
            }
            let () =
                expect_single_inspect_node(&realm, COUNTER_COMPONENT_NAME, |data| async move {
                    assert_data_tree!(data, root: {
                        counter: {
                            count: u64::from(i),
                        }
                    });
                })
                .await;
        }
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn eager_component(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![fnetemul::ChildDef {
                    eager: Some(true),
                    ..counter_component()
                }]),
                ..Default::default()
            },
        );

        // Connect to fuchsia.component.Binder to start the test realm.
        let binder_proxy = realm.connect_to_protocol::<fidl_fuchsia_component::BinderMarker>();

        // Receive Signal if fuchsia.component.Binder channel is closed prematurely.
        // This channel is scoped to the runtime of the component, so it should
        // not be closed before the component stops.
        let mut binder_fut = pin!(binder_proxy.on_closed().fuse());

        // Hold Future object of main assertion of the test so that we can join!
        // with the binder channel event stream below.
        let mut assert_fut = pin!(
            // Without binding to the child by connecting to its exposed protocol, we should be able to
            // see its inspect data since it has been started eagerly.
            expect_single_inspect_node(&realm, COUNTER_COMPONENT_NAME, |data| async move {
                assert_data_tree!(data, root: {
                    counter: {
                        count: 0u64,
                    }
                });
            })
            .fuse()
        );

        futures::select! {
            () = assert_fut => {},
            signals = binder_fut => panic!("binder channel closed with: {:?}", signals)
        };
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn network_context(sandbox: fnetemul::SandboxProxy) {
        let (network_ctx, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::NetworkContextMarker>();
        sandbox.get_network_context(server).expect("calling get network context");
        let (endpoint_mgr, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::EndpointManagerMarker>();
        network_ctx.get_endpoint_manager(server).expect("calling get endpoint manager");
        let endpoints = endpoint_mgr.list_endpoints().await.expect("calling list endpoints");
        assert_eq!(endpoints, Vec::<String>::new());

        let name = "ep";
        let (status, endpoint) = endpoint_mgr
            .create_endpoint(
                &name,
                &fnetemul_network::EndpointConfig {
                    mtu: 1500,
                    mac: None,
                    port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                    checksum_offload: false,
                },
            )
            .await
            .expect("calling create endpoint");
        zx::Status::ok(status).expect("endpoint creation");
        let endpoint = endpoint.expect("endpoint creation").into_proxy();
        assert_eq!(endpoint.get_name().await.expect("calling get name"), name);
        assert_eq!(
            endpoint.get_config().await.expect("calling get config"),
            fnetemul_network::EndpointConfig {
                mtu: 1500,
                mac: None,
                port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                checksum_offload: false,
            }
        );
    }

    fn get_network_manager(
        sandbox: &fnetemul::SandboxProxy,
    ) -> fnetemul_network::NetworkManagerProxy {
        let (network_ctx, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::NetworkContextMarker>();
        sandbox.get_network_context(server).expect("calling get network context");
        let (network_mgr, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::NetworkManagerMarker>();
        network_ctx.get_network_manager(server).expect("calling get network manager");
        network_mgr
    }

    #[fuchsia::test]
    async fn network_context_per_sandbox_connection() {
        let (sandbox1, sandbox1_fut) = setup_sandbox_service("sandbox_1");
        let (sandbox2, sandbox2_fut) = setup_sandbox_service("sandbox_2");
        let test = async move {
            let net_mgr1 = get_network_manager(&sandbox1);
            let net_mgr2 = get_network_manager(&sandbox2);

            let (status, _network) = net_mgr1
                .create_network("network", &fnetemul_network::NetworkConfig::default())
                .await
                .expect("calling create network");
            zx::Status::ok(status).expect("network creation");
            let (status, _network) = net_mgr1
                .create_network("network", &fnetemul_network::NetworkConfig::default())
                .await
                .expect("calling create network");
            assert_eq!(zx::Status::ok(status), Err(zx::Status::ALREADY_EXISTS));
            // Try re-connecting to the network manager for sandbox1 to ensure
            // it connects us to the same network manager instead of spawning a
            // new one.
            let net_mgr1 = get_network_manager(&sandbox1);
            let (status, _network) = net_mgr1
                .create_network("network", &fnetemul_network::NetworkConfig::default())
                .await
                .expect("calling create network");
            assert_eq!(zx::Status::ok(status), Err(zx::Status::ALREADY_EXISTS));

            let (status, _network) = net_mgr2
                .create_network("network", &fnetemul_network::NetworkConfig::default())
                .await
                .expect("calling create network");
            zx::Status::ok(status).expect("network creation");
            drop(sandbox1);
            drop(sandbox2);
        };
        let ((), (), ()) = futures::future::join3(
            sandbox1_fut.map(|()| info!("sandbox1_fut complete")),
            sandbox2_fut.map(|()| info!("sandbox2_fut complete")),
            test.map(|()| info!("test complete")),
        )
        .await;
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn network_context_used_by_child(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![
                    fnetemul::ChildDef {
                        source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                        name: Some("counter-with-network-context".to_string()),
                        exposes: Some(vec![CounterMarker::PROTOCOL_NAME.to_string()]),
                        uses: Some(fnetemul::ChildUses::Capabilities(vec![
                            fnetemul::Capability::LogSink(fnetemul::Empty {}),
                            fnetemul::Capability::NetemulNetworkContext(fnetemul::Empty {}),
                            counter_config_cap(),
                        ])),
                        ..Default::default()
                    },
                    // TODO(https://fxbug.dev/42144060): when we can allow ERROR logs for routing
                    // errors, add a child component that does not `use` NetworkContext, and verify
                    // that we cannot get at NetworkContext through it. It should result in a
                    // zx::Status::UNAVAILABLE error.
                ]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();
        let (network_context, server_end) =
            fidl::endpoints::create_proxy::<fnetemul_network::NetworkContextMarker>();
        counter
            .connect_to_protocol(
                fnetemul_network::NetworkContextMarker::PROTOCOL_NAME,
                server_end.into_channel(),
            )
            .expect("failed to connect to network context through counter");
        assert_matches::assert_matches!(
            network_context.setup(&[]).await,
            Ok((zx::sys::ZX_OK, Some(_setup_handle)))
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test(logging = false)]
    async fn create_realm_invalid_options(sandbox: fnetemul::SandboxProxy) {
        // TODO(https://github.com/frondeus/test-case/issues/37): consider using the #[test_case]
        // macro to define these cases statically, if we can access the name of the test case from
        // the test case body. This is necessary in order to avoid creating sandboxes with colliding
        // names at runtime.
        //
        // Note, however, that rustfmt struggles with macros, and using test-case for this test
        // would result in a lot of large struct literals defined as macro arguments of
        // #[test_case]. This may be more readable as an auto-formatted array.
        //
        // TODO(https://fxbug.dev/42156282): refactor how we specify the test cases to make it easier
        // to tell why a given case is invalid.
        struct TestCase<'a> {
            name: &'a str,
            children: Vec<fnetemul::ChildDef>,
            epitaph: zx::Status,
        }
        let cases = [
            TestCase {
                name: "no source provided",
                children: vec![fnetemul::ChildDef {
                    source: None,
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "no name provided",
                children: vec![fnetemul::ChildDef {
                    source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                    name: None,
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "capability not specified for child dependency",
                children: vec![fnetemul::ChildDef {
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                    uses: Some(fnetemul::ChildUses::Capabilities(vec![
                        fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                            name: Some("component".to_string()),
                            capability: None,
                            ..Default::default()
                        }),
                    ])),
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "duplicate capability used by child",
                children: vec![fnetemul::ChildDef {
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                    uses: Some(fnetemul::ChildUses::Capabilities(vec![
                        fnetemul::Capability::LogSink(fnetemul::Empty {}),
                        fnetemul::Capability::LogSink(fnetemul::Empty {}),
                    ])),
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "child manually depends on a duplicate of a netemul-provided capability",
                children: vec![fnetemul::ChildDef {
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                    uses: Some(fnetemul::ChildUses::Capabilities(vec![
                        fnetemul::Capability::LogSink(fnetemul::Empty {}),
                        fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                            name: Some("root".to_string()),
                            capability: Some(fnetemul::ExposedCapability::Protocol(
                                flogger::LogSinkMarker::PROTOCOL_NAME.to_string(),
                            )),
                            ..Default::default()
                        }),
                    ])),
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "child depends on nonexistent child",
                children: vec![
                    counter_component(),
                    fnetemul::ChildDef {
                        name: Some("counter-b".to_string()),
                        source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                        uses: Some(fnetemul::ChildUses::Capabilities(vec![
                            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                                // counter-a does not exist.
                                name: Some("counter-a".to_string()),
                                capability: Some(fnetemul::ExposedCapability::Protocol(
                                    CounterMarker::PROTOCOL_NAME.to_string(),
                                )),
                                ..Default::default()
                            }),
                        ])),
                        ..Default::default()
                    },
                ],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "child depends on storage without variant",
                children: vec![fnetemul::ChildDef {
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                    uses: Some(fnetemul::ChildUses::Capabilities(vec![
                        fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                            variant: None,
                            path: Some(DATA_PATH.to_string()),
                            ..Default::default()
                        }),
                    ])),
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "child depends on storage without path",
                children: vec![fnetemul::ChildDef {
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                    uses: Some(fnetemul::ChildUses::Capabilities(vec![
                        fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                            variant: Some(fnetemul::StorageVariant::Data),
                            path: None,
                            ..Default::default()
                        }),
                    ])),
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "duplicate components",
                children: vec![
                    fnetemul::ChildDef {
                        name: Some(COUNTER_COMPONENT_NAME.to_string()),
                        source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                        ..Default::default()
                    },
                    fnetemul::ChildDef {
                        name: Some(COUNTER_COMPONENT_NAME.to_string()),
                        source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                        ..Default::default()
                    },
                ],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "duplicate protocol from same component",
                children: vec![fnetemul::ChildDef {
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                    exposes: Some(vec![
                        CounterMarker::PROTOCOL_NAME.to_string(),
                        CounterMarker::PROTOCOL_NAME.to_string(),
                    ]),
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "storage capabilities use duplicate paths",
                children: vec![fnetemul::ChildDef {
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                    uses: Some(fnetemul::ChildUses::Capabilities(vec![
                        fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                            variant: Some(fnetemul::StorageVariant::Data),
                            path: Some(DATA_PATH.to_string()),
                            ..Default::default()
                        }),
                        fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                            variant: Some(fnetemul::StorageVariant::Data),
                            path: Some(DATA_PATH.to_string()),
                            ..Default::default()
                        }),
                    ])),
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "devfs capability name not provided",
                children: vec![fnetemul::ChildDef {
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                    uses: Some(fnetemul::ChildUses::Capabilities(vec![
                        fnetemul::Capability::NetemulDevfs(fnetemul::DevfsDep {
                            name: None,
                            ..Default::default()
                        }),
                    ])),
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "invalid subdirectory of devfs requested",
                children: vec![fnetemul::ChildDef {
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                    uses: Some(fnetemul::ChildUses::Capabilities(vec![
                        fnetemul::Capability::NetemulDevfs(fnetemul::DevfsDep {
                            name: Some("does-not-matter".to_string()),
                            subdir: Some("..".to_string()),
                            ..Default::default()
                        }),
                    ])),
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "dependency cycle between child components",
                children: vec![
                    fnetemul::ChildDef {
                        name: Some("counter-a".to_string()),
                        source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                        uses: Some(fnetemul::ChildUses::Capabilities(vec![
                            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                                name: Some("counter-b".to_string()),
                                capability: Some(fnetemul::ExposedCapability::Protocol(
                                    CounterMarker::PROTOCOL_NAME.to_string(),
                                )),
                                ..Default::default()
                            }),
                        ])),
                        ..Default::default()
                    },
                    fnetemul::ChildDef {
                        name: Some("counter-b".to_string()),
                        source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                        uses: Some(fnetemul::ChildUses::Capabilities(vec![
                            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                                name: Some("counter-a".to_string()),
                                capability: Some(fnetemul::ExposedCapability::Protocol(
                                    CounterMarker::PROTOCOL_NAME.to_string(),
                                )),
                                ..Default::default()
                            }),
                        ])),
                        ..Default::default()
                    },
                ],
                epitaph: zx::Status::INVALID_ARGS,
            },
            TestCase {
                name: "overriden program args for component without program",
                children: vec![fnetemul::ChildDef {
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    source: Some(fnetemul::ChildSource::Component(
                        COUNTER_WITHOUT_PROGRAM_URL.to_string(),
                    )),
                    program_args: Some(vec![]),
                    ..Default::default()
                }],
                epitaph: zx::Status::INVALID_ARGS,
            },
        ];
        for TestCase { name, children, epitaph } in cases {
            let TestRealm { realm } = TestRealm::new(
                &sandbox,
                fnetemul::RealmOptions { children: Some(children), ..Default::default() },
            );
            match realm.take_event_stream().next().await.unwrap_or_else(|| {
                panic!("test case failed: \"{}\": epitaph should be sent on realm channel", name)
            }) {
                Err(fidl::Error::ClientChannelClosed {
                    epitaph: actual_epitaph,
                    protocol_name:
                        <ManagedRealmMarker as fidl::endpoints::ProtocolMarker>::DEBUG_NAME,
                    ..
                }) if actual_epitaph == epitaph => (),
                event => panic!(
                    "test case failed: \"{}\": expected channel close with epitaph {}, got \
                     unexpected event on realm channel: {:?}",
                    name, epitaph, event
                ),
            }
        }
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn child_dep(sandbox: fnetemul::SandboxProxy) {
        let TestRealm { realm } = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![
                    fnetemul::ChildDef {
                        source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                        name: Some("counter-a".to_string()),
                        exposes: Some(vec![COUNTER_A_PROTOCOL_NAME.to_string()]),
                        uses: Some(fnetemul::ChildUses::Capabilities(vec![
                            fnetemul::Capability::LogSink(fnetemul::Empty {}),
                            fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                                name: Some("counter-b".to_string()),
                                capability: Some(fnetemul::ExposedCapability::Protocol(
                                    COUNTER_B_PROTOCOL_NAME.to_string(),
                                )),
                                ..Default::default()
                            }),
                            counter_config_cap(),
                        ])),
                        ..Default::default()
                    },
                    fnetemul::ChildDef {
                        source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                        name: Some("counter-b".to_string()),
                        exposes: Some(vec![COUNTER_B_PROTOCOL_NAME.to_string()]),
                        uses: Some(fnetemul::ChildUses::Capabilities(vec![
                            fnetemul::Capability::LogSink(fnetemul::Empty {}),
                            counter_config_cap(),
                        ])),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
        );
        let counter_a = {
            let (counter_a, server_end) = fidl::endpoints::create_proxy::<CounterMarker>();
            realm
                .connect_to_protocol(COUNTER_A_PROTOCOL_NAME, None, server_end.into_channel())
                .expect("failed to connect to CounterA protocol");
            counter_a
        };
        // counter-a should have access to counter-b's exposed protocol.
        let (counter_b, server_end) = fidl::endpoints::create_proxy::<CounterMarker>();
        counter_a
            .connect_to_protocol(COUNTER_B_PROTOCOL_NAME, server_end.into_channel())
            .expect("fuchsia.netemul.test/CounterA.connect_to_protocol call failed");
        assert_eq!(
            counter_b
                .increment()
                .await
                .expect("fuchsia.netemul.test/CounterB.increment call failed"),
            1,
        );
        // The counter-b protocol that counter-a has access to should be the same one accessible
        // through the test realm.
        let counter_b = {
            let (counter_b, server_end) = fidl::endpoints::create_proxy::<CounterMarker>();
            realm
                .connect_to_protocol(COUNTER_B_PROTOCOL_NAME, None, server_end.into_channel())
                .expect("failed to connect to CounterB protocol");
            counter_b
        };
        assert_eq!(
            counter_b
                .increment()
                .await
                .expect("fuchsia.netemul.test/CounterB.increment call failed"),
            2,
        );
        // TODO(https://fxbug.dev/42144060): once we can allow the ERROR logs that result from the
        // routing failure, verify that counter-b does *not* have access to counter-a's protocol.
    }

    async fn create_endpoint(
        sandbox: &fnetemul::SandboxProxy,
        name: &str,
        config: fnetemul_network::EndpointConfig,
    ) -> fnetemul_network::EndpointProxy {
        let (network_ctx, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::NetworkContextMarker>();
        sandbox.get_network_context(server).expect("calling get network context");
        let (endpoint_mgr, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::EndpointManagerMarker>();
        network_ctx.get_endpoint_manager(server).expect("calling get endpoint manager");
        let (status, endpoint) =
            endpoint_mgr.create_endpoint(name, &config).await.expect("calling create endpoint");
        zx::Status::ok(status).expect("endpoint creation");
        endpoint.expect("endpoint creation").into_proxy()
    }

    fn get_device_proxy(
        endpoint: &fnetemul_network::EndpointProxy,
    ) -> fidl::endpoints::ClientEnd<fnetemul_network::DeviceProxy_Marker> {
        let (device_proxy, server) =
            fidl::endpoints::create_endpoints::<fnetemul_network::DeviceProxy_Marker>();
        endpoint.get_proxy_(server).expect("failed to get device proxy from netdevice endpoint");
        device_proxy
    }

    async fn get_devfs_watcher(realm: &fnetemul::ManagedRealmProxy) -> fvfs_watcher::Watcher {
        let (devfs, server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        realm.get_devfs(server).expect("calling get devfs");
        fvfs_watcher::Watcher::new(&devfs).await.expect("watcher creation")
    }

    async fn wait_for_event_on_path(
        watcher: &mut fvfs_watcher::Watcher,
        event: fvfs_watcher::WatchEvent,
        path: &std::path::Path,
    ) {
        watcher
            .try_filter_map(|fvfs_watcher::WatchMessage { event: actual, filename }| {
                futures::future::ok((actual == event && filename == path).then(|| ()))
            })
            .try_next()
            .await
            .expect("error watching directory")
            .unwrap_or_else(|| {
                panic!("watcher stream expired before expected event {:?} was observed", event)
            });
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn devfs(sandbox: fnetemul::SandboxProxy) {
        let TestRealm { realm } = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![counter_component()]),
                ..Default::default()
            },
        );
        let mut watcher = get_devfs_watcher(&realm).await;

        const TEST_DEVICE_NAME: &str = "test";
        let endpoint = create_endpoint(
            &sandbox,
            TEST_DEVICE_NAME,
            fnetemul_network::EndpointConfig {
                mtu: 1500,
                mac: None,
                port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                checksum_offload: false,
            },
        )
        .await;

        realm
            .add_device(TEST_DEVICE_NAME, get_device_proxy(&endpoint))
            .await
            .expect("calling add device")
            .map_err(zx::Status::err_from_raw)
            .expect("error adding device");
        wait_for_event_on_path(
            &mut watcher,
            fvfs_watcher::WatchEvent::ADD_FILE,
            &std::path::Path::new(TEST_DEVICE_NAME),
        )
        .await;
        assert_eq!(
            realm
                .add_device(TEST_DEVICE_NAME, get_device_proxy(&endpoint))
                .await
                .expect("calling add device")
                .map_err(zx::Status::err_from_raw)
                .expect_err("adding a duplicate device should fail"),
            zx::Status::ALREADY_EXISTS,
        );

        // Check the device's `fuchsia.device/Controller.GetTopologicalPath`.
        let (controller, server_end) = fidl::endpoints::create_proxy::<fdevice::ControllerMarker>();
        get_device_proxy(&endpoint)
            .into_proxy()
            .serve_controller(server_end)
            .expect("failed to serve device");
        let path = controller
            .get_topological_path()
            .await
            .expect("calling get topological path")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to get topological path");
        assert!(path.contains(TEST_DEVICE_NAME));

        realm
            .remove_device(TEST_DEVICE_NAME)
            .await
            .expect("calling remove device")
            .map_err(zx::Status::err_from_raw)
            .expect("error removing device");
        wait_for_event_on_path(
            &mut watcher,
            fvfs_watcher::WatchEvent::REMOVE_FILE,
            &std::path::Path::new(TEST_DEVICE_NAME),
        )
        .await;
        assert_eq!(
            realm
                .remove_device(TEST_DEVICE_NAME)
                .await
                .expect("calling remove device")
                .map_err(zx::Status::err_from_raw)
                .expect_err("removing a nonexistent device should fail"),
            zx::Status::NOT_FOUND,
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn hwnetwork_service(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(&sandbox, fnetemul::RealmOptions { ..Default::default() });

        const TEST_DEVICE_NAME: &str = "test";
        let endpoint = create_endpoint(
            &sandbox,
            TEST_DEVICE_NAME,
            fnetemul_network::EndpointConfig {
                mtu: 1500,
                mac: None,
                port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                checksum_offload: false,
            },
        )
        .await;

        realm
            .realm
            .add_device(TEST_DEVICE_NAME, get_device_proxy(&endpoint))
            .await
            .expect("calling add device")
            .map_err(zx::Status::err_from_raw)
            .expect("error adding device");

        let service = realm.connect_to_service(fhwnet::ServiceMarker);

        // Read the directory to see if the device instance is there.
        let instances = service.clone().enumerate().await.expect("enumerate failed");

        let names: Vec<_> = instances.iter().map(|p| p.instance_name()).collect();
        assert_eq!(names, vec![TEST_DEVICE_NAME]);

        // Verify we can connect to the members of the service.
        let instance =
            service.connect_to_instance(TEST_DEVICE_NAME).expect("connect to instance failed");
        let _device = instance.connect_to_device().expect("connect to device failed");
        let device_topology =
            instance.connect_to_device_topology().expect("connect to device topology failed");

        // Verify topology works by calling get_topological_path
        let topo_path = device_topology
            .get_topological_path()
            .await
            .expect("get_topological_path failed")
            .expect("get_topological_path returned error");
        assert!(!topo_path.is_empty());
        assert!(topo_path.contains(TEST_DEVICE_NAME));

        // Adding the "test" device again causes an error.
        let result = realm
            .realm
            .add_device(TEST_DEVICE_NAME, get_device_proxy(&endpoint))
            .await
            .expect("calling add device again");
        assert_eq!(
            result.map_err(zx::Status::err_from_raw),
            Err(zx::Status::ALREADY_EXISTS),
            "Expected ALREADY_EXISTS when adding duplicate device"
        );

        // We can remove the "test" device so that it can no longer be enumerated.
        realm
            .realm
            .remove_device(TEST_DEVICE_NAME)
            .await
            .expect("calling remove device")
            .map_err(zx::Status::err_from_raw)
            .expect("error removing device");

        // Reuse the service proxy to check that it reflects the removal.
        let instances = service.enumerate().await.expect("enumerate failed after removal");
        let names: Vec<_> = instances.iter().map(|p| p.instance_name()).collect();
        assert_eq!(names, Vec::<&str>::new());

        // Removing it again causes an error.
        let result =
            realm.realm.remove_device(TEST_DEVICE_NAME).await.expect("calling remove device again");
        assert_eq!(
            result.map_err(zx::Status::err_from_raw),
            Err(zx::Status::NOT_FOUND),
            "Expected NOT_FOUND when removing non-existent device"
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn hwnetwork_service_watch(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(&sandbox, fnetemul::RealmOptions { ..Default::default() });

        const TEST_DEVICE_NAME: &str = "test";
        let endpoint = create_endpoint(
            &sandbox,
            TEST_DEVICE_NAME,
            fnetemul_network::EndpointConfig {
                mtu: 1500,
                mac: None,
                port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                checksum_offload: false,
            },
        )
        .await;

        let service = realm.connect_to_service(fhwnet::ServiceMarker);

        // Start watching.
        let mut stream = service.watch().await.expect("watch failed");

        // Add the device.
        realm
            .realm
            .add_device(TEST_DEVICE_NAME, get_device_proxy(&endpoint))
            .await
            .expect("calling add device")
            .map_err(zx::Status::err_from_raw)
            .expect("error adding device");

        // Wait for the event.
        let event = futures::StreamExt::next(&mut stream)
            .await
            .expect("stream ended unexpectedly")
            .expect("watch error");
        assert_eq!(event.instance_name(), TEST_DEVICE_NAME);
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn service_per_realm(sandbox: fnetemul::SandboxProxy) {
        const TEST_DEVICE_NAME: &str = "test";
        let endpoint = create_endpoint(
            &sandbox,
            TEST_DEVICE_NAME,
            fnetemul_network::EndpointConfig {
                mtu: 1500,
                mac: None,
                port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                checksum_offload: false,
            },
        )
        .await;
        let realm_a = TestRealm::new(&sandbox, fnetemul::RealmOptions { ..Default::default() });
        let realm_b = TestRealm::new(&sandbox, fnetemul::RealmOptions { ..Default::default() });

        realm_a
            .realm
            .add_device(TEST_DEVICE_NAME, get_device_proxy(&endpoint))
            .await
            .expect("calling add device")
            .map_err(zx::Status::err_from_raw)
            .expect("error adding device");

        // Connect to service directory in realm_a.
        let service_a = realm_a.connect_to_service(fhwnet::ServiceMarker);
        let instances_a = service_a.enumerate().await.expect("enumerate failed");
        let names_a: Vec<_> = instances_a.iter().map(|p| p.instance_name()).collect();
        assert_eq!(names_a, vec![TEST_DEVICE_NAME]);

        // Connect to service directory in realm_b.
        let service_b = realm_b.connect_to_service(fhwnet::ServiceMarker);
        let instances_b = service_b.enumerate().await.expect("enumerate failed");
        let names_b: Vec<_> = instances_b.iter().map(|p| p.instance_name()).collect();
        assert_eq!(names_b, Vec::<&str>::new());
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn service_used_by_child(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![fnetemul::ChildDef {
                    source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                    name: Some("counter-with-service".to_string()),
                    exposes: Some(vec![CounterMarker::PROTOCOL_NAME.to_string()]),
                    uses: Some(fnetemul::ChildUses::Capabilities(vec![
                        fnetemul::Capability::LogSink(fnetemul::Empty {}),
                        fnetemul::Capability::ChildDep(fnetemul::ChildDep {
                            name: Some(fnetemul::NETEMUL_SERVICES_COMPONENT_NAME.to_string()),
                            capability: Some(fnetemul::ExposedCapability::Service(
                                fhwnet::ServiceMarker::SERVICE_NAME.to_string(),
                            )),
                            ..Default::default()
                        }),
                        counter_config_cap(),
                    ])),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();

        const TEST_DEVICE_NAME: &str = "test";
        let endpoint = create_endpoint(
            &sandbox,
            TEST_DEVICE_NAME,
            fnetemul_network::EndpointConfig {
                mtu: 1500,
                mac: None,
                port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                checksum_offload: false,
            },
        )
        .await;
        realm
            .realm
            .add_device(TEST_DEVICE_NAME, get_device_proxy(&endpoint))
            .await
            .expect("FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("error adding device");

        // Connect to the service directory through the child.
        let (dir_proxy, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        counter
            .open_in_namespace(
                &format!("/svc/{}", fhwnet::ServiceMarker::SERVICE_NAME),
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                server_end.into_channel(),
            )
            .expect("failed to connect to service through counter");

        // Use Service client to check for the device.
        let service = Service::from_service_dir_proxy(dir_proxy, fhwnet::ServiceMarker);
        let instances = service.enumerate().await.expect("enumerate failed");
        let names: Vec<_> = instances.iter().map(|p| p.instance_name()).collect();
        assert_eq!(names, vec![TEST_DEVICE_NAME]);
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn devfs_per_realm(sandbox: fnetemul::SandboxProxy) {
        const TEST_DEVICE_NAME: &str = "test";
        let endpoint = create_endpoint(
            &sandbox,
            TEST_DEVICE_NAME,
            fnetemul_network::EndpointConfig {
                mtu: 1500,
                mac: None,
                port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                checksum_offload: false,
            },
        )
        .await;
        let (TestRealm { realm: realm_a }, TestRealm { realm: realm_b }) = (
            TestRealm::new(
                &sandbox,
                fnetemul::RealmOptions {
                    children: Some(vec![counter_component()]),
                    ..Default::default()
                },
            ),
            TestRealm::new(
                &sandbox,
                fnetemul::RealmOptions {
                    children: Some(vec![counter_component()]),
                    ..Default::default()
                },
            ),
        );
        let mut watcher_a = get_devfs_watcher(&realm_a).await;
        realm_a
            .add_device(TEST_DEVICE_NAME, get_device_proxy(&endpoint))
            .await
            .expect("calling add device")
            .map_err(zx::Status::err_from_raw)
            .expect("error adding device");
        wait_for_event_on_path(
            &mut watcher_a,
            fvfs_watcher::WatchEvent::ADD_FILE,
            &std::path::Path::new(TEST_DEVICE_NAME),
        )
        .await;
        // Expect not to see a matching device in `realm_b`'s devfs.
        let devfs_b = {
            let (devfs, server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            realm_b.get_devfs(server).expect("calling get devfs");
            devfs
        };
        let (status, buf) = devfs_b.read_dirents(fio::MAX_BUF).await.expect("calling read dirents");
        zx::Status::ok(status).expect("failed reading directory entries");
        assert_eq!(
            fuchsia_fs::directory::parse_dir_entries(&buf)
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .expect("failed parsing directory entries"),
            &[fuchsia_fs::directory::DirEntry {
                name: ".".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Directory
            }],
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn devfs_used_by_child(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![
                    fnetemul::ChildDef {
                        source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                        name: Some("counter-with-devfs".to_string()),
                        exposes: Some(vec![CounterMarker::PROTOCOL_NAME.to_string()]),
                        uses: Some(fnetemul::ChildUses::Capabilities(vec![
                            fnetemul::Capability::LogSink(fnetemul::Empty {}),
                            fnetemul::Capability::NetemulDevfs(fnetemul::DevfsDep {
                                name: Some("test-specific-devfs".to_string()),
                                subdir: None,
                                ..Default::default()
                            }),
                            counter_config_cap(),
                        ])),
                        ..Default::default()
                    },
                    // TODO(https://fxbug.dev/42144060): when we can allow ERROR logs for routing
                    // errors, add a child component that does not `use` `devfs`, and verify that we
                    // cannot get at the realm's `devfs` through it. It should result in a
                    // zx::Status::UNAVAILABLE error.
                ]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();

        const TEST_DEVICE_NAME: &str = "test";
        let endpoint = create_endpoint(
            &sandbox,
            TEST_DEVICE_NAME,
            fnetemul_network::EndpointConfig {
                mtu: 1500,
                mac: None,
                port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                checksum_offload: false,
            },
        )
        .await;
        realm
            .realm
            .add_device(TEST_DEVICE_NAME, get_device_proxy(&endpoint))
            .await
            .expect("FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("error adding device");

        // Expect the device to implement `fuchsia.device/Controller.GetTopologicalPath`.
        let (controller, server_end) = fidl::endpoints::create_proxy::<fdevice::ControllerMarker>();
        counter
            .open_in_namespace(
                &format!("{}/{}/device_controller", DEVFS_PATH, TEST_DEVICE_NAME),
                fio::PERM_READABLE,
                server_end.into_channel(),
            )
            .expect("failed to connect to device through counter");
        let path = controller
            .get_topological_path()
            .await
            .expect("FIDL error")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to get topological path");
        assert!(path.contains(TEST_DEVICE_NAME));
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn storage_used_by_child(sandbox: fnetemul::SandboxProxy) {
        fn connect_to_counter(
            realm: &fnetemul::ManagedRealmProxy,
            name: &str,
        ) -> fnetemul_test::CounterProxy {
            let (counter, server_end) = fidl::endpoints::create_proxy::<CounterMarker>();
            realm
                .connect_to_protocol(name, None, server_end.into_channel())
                .expect("failed to connect to counter protocol");
            counter
        }
        const COUNTER_WITH_STORAGE: &str = "counter-with-storage";
        const COUNTER_WITHOUT_STORAGE: &str = "counter-without-storage";
        let TestRealm { realm } = TestRealm::new(
            &sandbox,
            RealmOptions {
                children: Some(vec![
                    fnetemul::ChildDef {
                        source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                        name: Some(COUNTER_WITH_STORAGE.to_string()),
                        exposes: Some(vec![COUNTER_A_PROTOCOL_NAME.to_string()]),
                        uses: Some(fnetemul::ChildUses::Capabilities(vec![
                            fnetemul::Capability::LogSink(fnetemul::Empty {}),
                            fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                                variant: Some(fnetemul::StorageVariant::Data),
                                path: Some(String::from(DATA_PATH)),
                                ..Default::default()
                            }),
                            fnetemul::Capability::StorageDep(fnetemul::StorageDep {
                                variant: Some(fnetemul::StorageVariant::Cache),
                                path: Some(String::from(CACHE_PATH)),
                                ..Default::default()
                            }),
                            counter_config_cap(),
                        ])),
                        ..Default::default()
                    },
                    fnetemul::ChildDef {
                        source: Some(fnetemul::ChildSource::Component(COUNTER_URL.to_string())),
                        name: Some(COUNTER_WITHOUT_STORAGE.to_string()),
                        exposes: Some(vec![COUNTER_B_PROTOCOL_NAME.to_string()]),
                        uses: Some(fnetemul::ChildUses::Capabilities(vec![
                            fnetemul::Capability::LogSink(fnetemul::Empty {}),
                            counter_config_cap(),
                        ])),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
        );
        let counter_storage = connect_to_counter(&realm, COUNTER_A_PROTOCOL_NAME);
        let counter_without_storage = connect_to_counter(&realm, COUNTER_B_PROTOCOL_NAME);

        for dir in [CACHE_PATH, DATA_PATH] {
            counter_storage
                .try_open_directory(dir)
                .await
                .unwrap_or_else(|e| panic!("calling open {}: {:?}", dir, e))
                .map_err(zx::Status::err_from_raw)
                .unwrap_or_else(|e| panic!("failed to open {}: {:?}", dir, e));
            let result = counter_without_storage
                .try_open_directory(dir)
                .await
                .unwrap_or_else(|e| panic!("calling open {}: {:?}", dir, e))
                .map_err(zx::Status::err_from_raw);
            assert_eq!(result, Err(zx::Status::NOT_FOUND), "opening {}", dir);
        }
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn start_child_component_starts_child(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![counter_component()]),
                ..Default::default()
            },
        );
        let TestRealm { realm } = realm;
        realm
            .start_child_component(COUNTER_COMPONENT_NAME)
            .await
            .expect("calling start child component")
            .map_err(zx::Status::err_from_raw)
            .expect("start child component failed");

        // Without connecting to a protocol exposed by the child, and without
        // enabling eager startup, we should be able to see its inspect data
        // since it has been started explicitly.
        expect_single_inspect_node(
            &TestRealm { realm },
            COUNTER_COMPONENT_NAME,
            |data| async move {
                assert_data_tree!(data, root: {
                    counter: {
                        count: 0u64,
                    }
                });
            },
        )
        .await;
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn stop_child_component_stops_child(sandbox: fnetemul::SandboxProxy) {
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![counter_component()]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();
        assert_eq!(counter.increment().await.expect("failed to increment counter"), 1);
        let TestRealm { realm } = realm;
        realm
            .stop_child_component(COUNTER_COMPONENT_NAME)
            .await
            .expect("calling stop child component")
            .map_err(zx::Status::err_from_raw)
            .expect("stop child component failed");
        let err =
            counter.increment().await.expect_err("increment call on stopped child should fail");
        assert_matches::assert_matches!(
            err,
            fidl::Error::ClientChannelClosed { epitaph, protocol_name, .. }
                if epitaph == zx::Status::PEER_CLOSED &&
                    protocol_name == CounterMarker::PROTOCOL_NAME
        );
    }

    #[derive(Debug)]
    enum StartStop {
        Start,
        Stop,
    }

    const INVALID_MONIKER: &str = "com/.\\/\\.ponent";

    #[test_case(StartStop::Start, COUNTER_COMPONENT_NAME, zx::Status::NOT_FOUND; "start nonexistent component")]
    #[test_case(StartStop::Stop, COUNTER_COMPONENT_NAME, zx::Status::NOT_FOUND; "stop nonexistent component")]
    #[test_case(StartStop::Start, INVALID_MONIKER, zx::Status::INVALID_ARGS; "start invalid component")]
    #[test_case(StartStop::Stop, INVALID_MONIKER, zx::Status::INVALID_ARGS; "stop invalid component")]
    #[fuchsia::test]
    async fn start_stop_child_component_errors(
        action: StartStop,
        moniker: &str,
        expected_status: zx::Status,
    ) {
        // TODO(https://fxbug.dev/42075656): Use #[fixture] for this once it integrates
        // well with #[test_case].
        with_sandbox(
            &format!("{action:?}_child_component_errors_{expected_status}"),
            |sandbox| async move {
                let TestRealm { realm } =
                    TestRealm::new(&sandbox, fnetemul::RealmOptions::default());
                let err = match action {
                    StartStop::Start => realm.start_child_component(moniker),
                    StartStop::Stop => realm.stop_child_component(moniker),
                }
                .await
                .unwrap_or_else(|e| panic!("failed to {:?} child component: {:?}", action, e))
                .map_err(zx::Status::err_from_raw);
                assert_eq!(err, Err(expected_status));
            },
        )
        .await
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn devfs_intermediate_directories(sandbox: fnetemul::SandboxProxy) {
        let TestRealm { realm } = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![counter_component()]),
                ..Default::default()
            },
        );
        const CLASS_DIR: &str = "class";
        const NETWORK_DIR: &str = "network";
        const TEST_DEVICE_NAME: &str = "ep0";
        let ethernet_path = format!("{}/{}", CLASS_DIR, NETWORK_DIR);
        let test_device_path = format!("{}/{}/{}", CLASS_DIR, NETWORK_DIR, TEST_DEVICE_NAME);
        let endpoint = create_endpoint(
            &sandbox,
            TEST_DEVICE_NAME,
            fnetemul_network::EndpointConfig {
                mtu: 1500,
                mac: None,
                port_class: fidl_fuchsia_hardware_network::PortClass::Virtual,
                checksum_offload: false,
            },
        )
        .await;

        let (devfs, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        realm.get_devfs(server_end).expect("calling get devfs");
        let mut dev_watcher = fvfs_watcher::Watcher::new(&devfs).await.expect("watcher creation");
        realm
            .add_device(&test_device_path, get_device_proxy(&endpoint))
            .await
            .expect("calling add device")
            .map_err(zx::Status::err_from_raw)
            .expect("error adding device");
        wait_for_event_on_path(
            &mut dev_watcher,
            fvfs_watcher::WatchEvent::ADD_FILE,
            &std::path::Path::new(CLASS_DIR),
        )
        .await;

        let (network, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        devfs
            .open(
                &ethernet_path,
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                &Default::default(),
                server_end.into_channel(),
            )
            .expect("calling open");
        let mut watcher = fvfs_watcher::Watcher::new(&network).await.expect("watcher creation");
        wait_for_event_on_path(
            &mut watcher,
            fvfs_watcher::WatchEvent::EXISTING,
            &std::path::Path::new(TEST_DEVICE_NAME),
        )
        .await;
        realm
            .remove_device(&test_device_path)
            .await
            .expect("calling remove device")
            .map_err(zx::Status::err_from_raw)
            .expect("error removing device");
        wait_for_event_on_path(
            &mut watcher,
            fvfs_watcher::WatchEvent::REMOVE_FILE,
            &std::path::Path::new(TEST_DEVICE_NAME),
        )
        .await;
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test(logging = false)]
    async fn add_remove_device_invalid_path(sandbox: fnetemul::SandboxProxy) {
        let TestRealm { realm } = TestRealm::new(&sandbox, fnetemul::RealmOptions::default());
        const INVALID_FILE_PATH: &str = "class/ethernet/..";
        let (device_proxy, _server) =
            fidl::endpoints::create_endpoints::<fnetemul_network::DeviceProxy_Marker>();
        let err = realm
            .add_device(INVALID_FILE_PATH, device_proxy)
            .await
            .expect("calling add device")
            .map_err(zx::Status::err_from_raw)
            .expect_err("add device with invalid path should fail");
        assert_eq!(err, zx::Status::INVALID_ARGS);
        let err = realm
            .remove_device(INVALID_FILE_PATH)
            .await
            .expect("calling remove device")
            .map_err(zx::Status::err_from_raw)
            .expect_err("remove device with invalid path should fail");
        assert_eq!(err, zx::Status::INVALID_ARGS);
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn devfs_subdirs_created_on_request(sandbox: fnetemul::SandboxProxy) {
        const DEVFS_SUBDIR_USER_URL: &str = "#meta/devfs-subdir-user.cm";
        const SUBDIR: &str = "class/ethernet";
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![fnetemul::ChildDef {
                    source: Some(fnetemul::ChildSource::Component(
                        DEVFS_SUBDIR_USER_URL.to_string(),
                    )),
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    exposes: Some(vec![CounterMarker::PROTOCOL_NAME.to_string()]),
                    uses: Some(fnetemul::ChildUses::Capabilities(vec![
                        fnetemul::Capability::LogSink(fnetemul::Empty {}),
                        fnetemul::Capability::NetemulDevfs(fnetemul::DevfsDep {
                            name: Some("dev-class-ethernet".to_string()),
                            subdir: Some(SUBDIR.to_string()),
                            ..Default::default()
                        }),
                    ])),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();
        let path = format!("{}/{}", DEVFS_PATH, SUBDIR);

        let (ethernet, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        counter
            .open_in_namespace(&path, fio::PERM_READABLE, server_end.into_channel())
            .unwrap_or_else(|e| panic!("failed to connect to {} through counter: {:?}", path, e));
        let (status, buf) =
            ethernet.read_dirents(fio::MAX_BUF).await.expect("calling read dirents");
        zx::Status::ok(status).expect("failed reading directory entries");
        assert_eq!(
            fuchsia_fs::directory::parse_dir_entries(&buf)
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .expect("failed parsing directory entries"),
            &[fuchsia_fs::directory::DirEntry {
                name: ".".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Directory
            }],
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn override_config_values(sandbox: fnetemul::SandboxProxy) {
        const STARTING_VALUE: u32 = 9000;
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![fnetemul::ChildDef {
                    program_args: Some(vec!["--starting-value-from-config".to_string()]),
                    config_values: Some(vec![fnetemul::ChildConfigValue {
                        key: "starting_value".to_string(),
                        value: cm_rust::ConfigValue::from(STARTING_VALUE).native_into_fidl(),
                    }]),
                    ..counter_component()
                }]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();
        assert_eq!(
            counter.increment().await.expect("failed to increment counter"),
            STARTING_VALUE + 1,
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn override_program_args(sandbox: fnetemul::SandboxProxy) {
        const STARTING_VALUE: u32 = 9000;
        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![fnetemul::ChildDef {
                    program_args: Some(vec![
                        "--starting-value".to_string(),
                        STARTING_VALUE.to_string(),
                    ]),
                    ..counter_component()
                }]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();
        assert_eq!(
            counter.increment().await.expect("failed to increment counter"),
            STARTING_VALUE + 1,
        );
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn mock_child(sandbox: fnetemul::SandboxProxy) {
        let (mock_dir, server_end) = fidl::endpoints::create_endpoints();

        let mut fs = ServiceFs::new();
        let _: &mut ServiceFsDir<'_, _> =
            fs.dir("svc").add_fidl_service(|s: fnetemul_test::CounterRequestStream| s);

        let _: &mut ServiceFs<_> = fs.serve_connection(server_end).expect("serve connection");

        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions {
                children: Some(vec![fnetemul::ChildDef {
                    source: Some(fnetemul::ChildSource::Mock(mock_dir)),
                    name: Some(COUNTER_COMPONENT_NAME.to_string()),
                    exposes: Some(vec![CounterMarker::PROTOCOL_NAME.to_string()]),
                    uses: Some(fnetemul::ChildUses::Capabilities(vec![
                        fnetemul::Capability::LogSink(fnetemul::Empty {}),
                    ])),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        );
        let counter = realm.connect_to_protocol::<CounterMarker>();
        let counter_fut = counter.increment();

        let counter_request = fs
            .flatten()
            .try_next()
            .await
            .expect("next request")
            .expect("service fs ended unexpectedly");

        const RESPONSE_VALUE: u32 = 1234;
        match counter_request {
            fnetemul_test::CounterRequest::Increment { responder } => {
                responder.send(RESPONSE_VALUE).expect("failed to send response");
            }
            r => panic!("unexpected request {:?}", r),
        }

        assert_eq!(counter_fut.await.expect("increment failed"), RESPONSE_VALUE);
    }

    #[fixture(with_sandbox)]
    #[fuchsia::test]
    async fn crash_stop_listener(sandbox: fnetemul::SandboxProxy) {
        #[derive(Copy, Clone, Debug)]
        enum Variant {
            WithShutdownAbort,
            WithShutdownClean,
            WithoutShutdown,
        }

        let counter_spec = std::iter::repeat_n(
            [Variant::WithShutdownAbort, Variant::WithShutdownClean, Variant::WithoutShutdown]
                .into_iter(),
            // Create enough components to prove there are no races or missed
            // events.
            5,
        )
        .flatten()
        .enumerate()
        .map(|(i, variant)| {
            let name = format!("counter{i}");
            let (url, abort) = match variant {
                Variant::WithoutShutdown => (COUNTER_URL, false),
                Variant::WithShutdownAbort => (COUNTER_WITH_SHUTDOWN_PROGRAM_URL, true),
                Variant::WithShutdownClean => (COUNTER_WITH_SHUTDOWN_PROGRAM_URL, false),
            };
            (name, url, abort)
        });

        let children = counter_spec.clone().map(|(name, url, _abort)| fnetemul::ChildDef {
            name: Some(name),
            source: Some(fnetemul::ChildSource::Component(url.to_string())),
            ..counter_component()
        });

        // Add a mock child to check it doesn't count as a crash exit.
        let mock_name = "mock";
        let (mock_dir, server_end) = fidl::endpoints::create_endpoints();
        let mut fs = ServiceFs::new();
        let _: &mut ServiceFsDir<'_, _> =
            fs.dir("svc").add_fidl_service(|s: fnetemul_test::CounterRequestStream| s);
        let _: &mut ServiceFs<_> = fs.serve_connection(server_end).expect("serve connection");
        let children = children.chain(std::iter::once(fnetemul::ChildDef {
            source: Some(fnetemul::ChildSource::Mock(mock_dir)),
            name: Some(mock_name.to_string()),
            exposes: Some(vec![CounterMarker::PROTOCOL_NAME.to_string()]),
            uses: Some(fnetemul::ChildUses::Capabilities(vec![fnetemul::Capability::LogSink(
                fnetemul::Empty {},
            )])),
            ..Default::default()
        }));

        let realm = TestRealm::new(
            &sandbox,
            fnetemul::RealmOptions { children: Some(children.collect()), ..Default::default() },
        );

        let realm_ref = &realm;
        let mut expect_failed = futures::stream::iter(counter_spec)
            .filter_map(|(name, _, abort)| async move {
                let counter = realm_ref.connect_to_protocol_from_child::<CounterMarker>(&name);
                counter.set_abort_on_shutdown(abort).await.expect("calling panic on shutdown");
                abort.then_some(name)
            })
            .collect::<Vec<_>>()
            .await;

        // Make sure the mock component is alive.
        {
            let counter = realm.connect_to_protocol_from_child::<CounterMarker>(mock_name);
            let _counter_fut = counter.increment();
            let _counter_request = fs
                .flatten()
                .try_next()
                .await
                .expect("next request")
                .expect("service fs ended unexpectedly")
                .into_increment()
                .expect("observed wrong request");
        }

        // Shutdown everything and check what's reported as a crash.
        let TestRealm { realm } = realm;
        let (listener, server_end) =
            fidl::endpoints::create_proxy::<fnetemul::CrashListenerMarker>();
        realm.get_crash_listener(server_end).expect("new crash listener");
        realm.shutdown().expect("calling shutdown");

        let mut failed_monikers = vec![];
        loop {
            let crashed = listener.next().await.expect("next crash");
            if crashed.is_empty() {
                // The channel should close shortly after observing the final
                // sentinel.
                let _: zx::Signals =
                    listener.as_channel().on_closed().await.expect("waiting channel close");
                break;
            }
            failed_monikers.extend(crashed);
        }
        expect_failed.sort();
        failed_monikers.sort();
        assert_eq!(failed_monikers, expect_failed);
    }
}
