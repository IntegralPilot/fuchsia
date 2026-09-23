// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::{ComponentInstance, WeakComponentInstance, WeakExtendedInstance};
use crate::sandbox_util::LaunchTaskOnReceive;
use ::routing::component_instance::ComponentInstanceInterface;
use ::routing::error::RoutingError;
use async_trait::async_trait;
use capability_source::{CapabilitySource, FrameworkSource, InternalCapability};
use clonable_error::ClonableError;
use cm_types::Url;
use errors::ResolveActionError;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_internal as finternal;
use fidl_fuchsia_component_runtime as fruntime;
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_sys2 as fsys;
use futures::FutureExt;
use futures::future::BoxFuture;
use log::warn;
use moniker::Moniker;
use router_error::RouterError;
use routing::component_instance::ResolvedInstanceInterface;
use routing::resolving::{ComponentAddress, ResolverError};
use runtime_capabilities::{Dictionary, Routable, Router, WeakInstanceToken};
use std::sync::Arc;

pub mod binder;
pub mod capabilities;
pub mod capability_store;
pub mod component_sandbox_retriever;
pub mod config_override;
pub mod controller;
pub mod introspector;
pub mod lifecycle_controller;
pub mod namespace;
pub mod realm;
pub mod realm_query;
pub mod route_validator;

/// Returns a router that returns a dictionary containing routers for all of the framework
/// capabilities scoped to the component `scope`. Making this a Router instead of a Dictionary
/// saves memory compared to generating a Dictionary of framework capabilities for each component
/// up front.
pub(crate) fn get_framework_router(scope: &Arc<ComponentInstance>) -> Arc<Router<Dictionary>> {
    Router::new(FrameworkRouter { scope: scope.moniker.clone() })
}

struct FrameworkRouter {
    scope: Moniker,
}

#[async_trait]
impl Routable<Dictionary> for FrameworkRouter {
    async fn route(
        &self,
        _request: fruntime::RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<Dictionary>>, RouterError> {
        let target = target
            .inner
            .as_any()
            .downcast_ref::<WeakExtendedInstance>()
            .ok_or(RouterError::Unknown)?;
        let component = match target {
            WeakExtendedInstance::Component(c) if c.moniker == self.scope => c,
            WeakExtendedInstance::AboveRoot(_) | WeakExtendedInstance::Component(_) => {
                // Note that framework dictionaries are currently _always_ routed with the `target`
                // set to the component whose sandbox owns this dictionary, and thus two invariants
                // will always hold:
                //
                // - `target` will always be a `WeakExtendedInstance::Component`
                // - the moniker for `target` will match `self.scope`
                //
                // If either of the above two invariants are broken, then there's been changes to
                // the logic where framework dictionaries are routed. We use a panic here to try to
                // make the bugs that change would cause louder and more immediately obvious to
                // whoever is making those changes, so that they can either put back the previous
                // logic or rewrite the logic here to match the new behavior.
                panic!(
                    "framework dictionary target is unexpected for scope {}: {target:?}",
                    self.scope
                );
            }
        };
        let component = component.upgrade().map_err(RoutingError::from)?;

        let framework_dictionary = Dictionary::new();
        add_protocol::<fcomponent::BinderMarker>(&component, &framework_dictionary, binder::serve);
        add_protocol::<fruntime::CapabilitiesMarker>(
            &component,
            &framework_dictionary,
            capabilities::serve,
        );
        add_protocol::<fsandbox::CapabilityStoreMarker>(
            &component,
            &framework_dictionary,
            capability_store::serve,
        );
        if component.context.runtime_config().enable_introspection {
            add_protocol::<fsys::ConfigOverrideMarker>(
                &component,
                &framework_dictionary,
                config_override::serve,
            );
            add_protocol::<fsys::LifecycleControllerMarker>(
                &component,
                &framework_dictionary,
                lifecycle_controller::serve,
            );
            add_protocol::<fsys::RealmQueryMarker>(
                &component,
                &framework_dictionary,
                realm_query::serve,
            );
            add_protocol::<fsys::RouteValidatorMarker>(
                &component,
                &framework_dictionary,
                route_validator::serve,
            );
        }
        add_protocol::<fcomponent::IntrospectorMarker>(
            &component,
            &framework_dictionary,
            introspector::serve,
        );
        add_protocol::<fcomponent::NamespaceMarker>(
            &component,
            &framework_dictionary,
            namespace::serve,
        );
        add_protocol::<fcomponent::RealmMarker>(&component, &framework_dictionary, realm::serve);
        add_pkg_dir(&component, &framework_dictionary);
        add_protocol::<finternal::ComponentSandboxRetrieverMarker>(
            &component,
            &framework_dictionary,
            component_sandbox_retriever::serve,
        );
        #[cfg(test)]
        {
            let extra_framework_capabilities =
                component.context.extra_framework_capabilities.lock();
            for (name, capability) in extra_framework_capabilities.iter() {
                // Internal capabilities added for a test should preempt existing ones that have
                // the same name.
                let _ = framework_dictionary.insert(name.clone(), capability.clone());
            }
        }
        Ok(Some(framework_dictionary))
    }

    async fn route_debug(
        &self,
        _request: fruntime::RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        panic!("framework router does not support debug routes");
    }
}

fn add_protocol<P: DiscoverableProtocolMarker>(
    component: &Arc<ComponentInstance>,
    dict: &Dictionary,
    task_to_launch: impl Fn(
        zx::Channel,
        /*target: */ WeakComponentInstance,
        /*scope: */ WeakComponentInstance,
    ) -> BoxFuture<'static, Result<(), anyhow::Error>>
    + Sync
    + Send
    + 'static,
) {
    let capability_source = CapabilitySource::Framework(FrameworkSource {
        capability: InternalCapability::Protocol(P::PROTOCOL_NAME.parse().unwrap()),
        moniker: component.moniker.clone(),
    });
    // Dictionary inserts succeed even when they return an error.
    let source = component.as_weak();
    let prev = dict.insert(
        P::PROTOCOL_NAME.parse().unwrap(),
        LaunchTaskOnReceive::new(
            capability_source,
            component.execution_scope.as_weak(),
            format!("framework dispatcher for {}", P::PROTOCOL_NAME),
            Some(component.context.policy().clone()),
            Arc::new(move |chan, target, _path, _rights| {
                task_to_launch(chan, target, source.clone())
            }),
        )
        .into_router()
        .into(),
    );
    assert!(prev.is_none(), "conflict found in framework dictionary");
}

fn add_pkg_dir(component: &Arc<ComponentInstance>, dict: &Dictionary) {
    let weak_source_component = component.as_weak();
    let launch_task_on_receive = LaunchTaskOnReceive::new(
        CapabilitySource::Framework(FrameworkSource {
            capability: InternalCapability::Directory("pkg".parse().unwrap()),
            moniker: component.moniker.clone(),
        }),
        component.execution_scope.as_weak(),
        "framework_pkg_directory",
        Some(component.context.policy().clone()),
        Arc::new(move |channel, _weak_target_component, relative_path, rights| {
            let weak_source_component = weak_source_component.clone();
            async move {
                let source_component = weak_source_component.upgrade()?;
                let resolved_state = source_component.lock_resolved_state().await?;
                let package =
                    resolved_state.resolved_component.package.as_ref().ok_or_else(|| {
                        anyhow::format_err!(
                            "source component {} missing package",
                            source_component.moniker
                        )
                    })?;
                let flags = fio::Flags::from_bits(rights.bits())
                    .expect("failed to convert operations to flags");
                let path: String = relative_path.clone().into();
                fio::DirectoryProxy::open(
                    &package.package_dir,
                    &path,
                    flags,
                    &fio::Options::default(),
                    channel,
                )?;
                Ok(())
            }
            .boxed()
        }),
    );
    let prev = dict.insert("pkg".parse().unwrap(), launch_task_on_receive.into_dir_router().into());
    assert!(prev.is_none(), "conflict with pkg directory in framework dictionary");
}

/// Re-resolve an already resolved component to retrieve its component
/// declaration. This allows us to save memory by dropping the component decl
/// from the ResolvedInstanceState of a component.
pub(crate) async fn resolve_with_pinned_url(
    component: &Arc<ComponentInstance>,
) -> Result<cm_rust::ComponentDecl, ResolveError> {
    let mut address;
    let mut package;
    {
        let state = component.lock_state().await;
        let resolved = state
            .get_resolved_state()
            .ok_or(ResolveError::InstanceNotResolved { url: component.url().clone() })?;
        address = resolved.address().await.map_err(|_| ResolveError::BadUrl {
            url: component.url().clone(),
            moniker: component.moniker.clone(),
        })?;
        package = resolved.package().map(|p| Clone::clone(&p.package_dir));
    };
    let can_pin = match &address {
        ComponentAddress::Absolute { url, .. } => match url.scheme() {
            "fuchsia-pkg" => {
                // Only fuchsia-pkg urls support pinning.
                true
            }
            _ => false,
        },
        ComponentAddress::RelativePath { .. } => {
            // The Context token already pins the package, we don't need to modify the url.
            false
        }
    };
    if !can_pin {
        package = None;
    }
    if let Some(package) = package {
        let (meta_file, server_end) = fidl::endpoints::create_proxy::<fio::FileMarker>();
        package
            .open(
                "meta",
                fio::PERM_READABLE | fio::Flags::PROTOCOL_FILE,
                &Default::default(),
                server_end.into(),
            )
            .map_err(|err| {
                warn!(err:%, url:% = component.url();
                      "resolve_with_pinned_url: failed to open package");
                ResolveError::PackageOpenFailed { url: component.url().clone(), err }
            })?;
        let merkle = fuchsia_fs::file::read_to_string(&meta_file).await.map_err(|err| {
            warn!(err:%, url:% = component.url();
                  "resolve_with_pinned_url: failed to open package");
            ResolveError::PackageReadFailed { url: component.url().clone(), err }
        })?;
        match &mut address {
            ComponentAddress::Absolute { url } => {
                url.set_query(Some(&format!("hash={merkle}")));
            }
            ComponentAddress::RelativePath { .. } => {
                unreachable!("resolve_with_pinned_url: RelativePath is never pinned");
            }
        }
    }

    let component_info = match component.perform_resolve(None, &address).await {
        Ok(c) => c,
        // This was a request to the base resolver, which does not support
        // pinning, or the request was made without an active package server.
        Err(err @ ResolverError::MalformedUrl(_))
        | Err(err @ ResolverError::PackageNotFound(_)) => {
            match &mut address {
                ComponentAddress::Absolute { url } => {
                    if url.query().is_none() {
                        warn!(err:%, url:% = component.url();
                              "resolve_with_pinned_url: resolution failed");
                        return Err(ResolveError::ReresolveFailed {
                            url: Url::new(url.as_str()).unwrap(),
                            err,
                        });
                    } else {
                        // Try again without a hash pin in the query string.
                        url.set_query(None);
                    }
                }
                ComponentAddress::RelativePath { .. } => {
                    return Err(ResolveError::ReresolveFailed {
                        url: component.url().clone(),
                        err,
                    });
                }
            }
            component.perform_resolve(None, &address).await.map_err(|err| {
                warn!(err:%, url:% = component.url();
                      "resolve_with_pinned_url: re-resolution failed");
                ResolveError::ReresolveFailed { url: component.url().clone(), err }
            })?
        }
        Err(err) => {
            warn!(err:%, url:% = component.url();
                    "resolve_with_pinned_url: resolution failed");
            return Err(ResolveError::ReresolveFailed { url: component.url().clone(), err });
        }
    };
    Ok(component_info.decl)
}

#[derive(Debug)]
pub(crate) enum ResolveError {
    InstanceNotResolved { url: Url },
    BadUrl { url: Url, moniker: Moniker },
    PackageOpenFailed { url: Url, err: fidl::Error },
    PackageReadFailed { url: Url, err: fuchsia_fs::file::ReadError },
    ReresolveFailed { url: Url, err: ResolverError },
}

impl From<ResolveError> for fsys::GetDeclarationError {
    fn from(value: ResolveError) -> Self {
        match value {
            ResolveError::InstanceNotResolved { .. } => Self::InstanceNotResolved,
            ResolveError::BadUrl { .. } => Self::BadUrl,
            ResolveError::PackageOpenFailed { .. } => Self::PackageOpenFailed,
            ResolveError::PackageReadFailed { .. } => Self::PackageReadFailed,
            ResolveError::ReresolveFailed { .. } => Self::ResolveFailed,
        }
    }
}

impl From<ResolveError> for fsys::RouteValidatorError {
    fn from(value: ResolveError) -> Self {
        match value {
            ResolveError::InstanceNotResolved { .. } => Self::InstanceNotResolved,
            ResolveError::BadUrl { .. } => Self::Internal,
            ResolveError::PackageOpenFailed { .. } => Self::InstanceNotResolved,
            ResolveError::PackageReadFailed { .. } => Self::InstanceNotResolved,
            ResolveError::ReresolveFailed { .. } => Self::InstanceNotReresolved,
        }
    }
}

impl From<ResolveError> for fcomponent::Error {
    fn from(value: ResolveError) -> Self {
        match value {
            ResolveError::InstanceNotResolved { .. } => Self::InstanceNotFound,
            ResolveError::BadUrl { .. } => Self::Internal,
            ResolveError::PackageOpenFailed { .. } => Self::InstanceCannotResolve,
            ResolveError::PackageReadFailed { .. } => Self::InstanceCannotResolve,
            ResolveError::ReresolveFailed { .. } => Self::InstanceCannotResolve,
        }
    }
}

impl From<ResolveError> for ResolveActionError {
    fn from(value: ResolveError) -> Self {
        match value {
            ResolveError::InstanceNotResolved { url, .. } => ResolveActionError::ResolverError {
                url,
                err: Box::new(ResolverError::Internal(ClonableError::from(anyhow::anyhow!(
                    "instance is not resolved"
                )))),
            },
            ResolveError::BadUrl { url, moniker } => {
                ResolveActionError::ComponentAddressParseError {
                    url,
                    moniker,
                    err: Box::new(ResolverError::MalformedUrl(ClonableError::from(
                        anyhow::anyhow!("bad url"),
                    ))),
                }
            }
            ResolveError::PackageOpenFailed { url, err, .. } => ResolveActionError::ResolverError {
                url,
                err: Box::new(ResolverError::Io(ClonableError::from(anyhow::Error::from(err)))),
            },
            ResolveError::PackageReadFailed { url, err, .. } => ResolveActionError::ResolverError {
                url,
                err: Box::new(ResolverError::Io(ClonableError::from(anyhow::Error::from(err)))),
            },
            ResolveError::ReresolveFailed { url, err } => {
                ResolveActionError::ResolverError { url, err: Box::new(err) }
            }
        }
    }
}
