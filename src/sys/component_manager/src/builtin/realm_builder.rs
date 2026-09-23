// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module is not for production instances of component_manager. It exists to allow a test
//! driver to define a custom realm with realm builder and to then launch a nested component
//! manager which will run that custom realm, for the sole purposes of integration testing
//! component manager behavior.

use crate::builtin::runner::BuiltinRunnerFactory;
use crate::model::resolver::Resolver;
use ::routing::policy::ScopedPolicyChecker;
use ::routing::resolving::{self, ComponentAddress, ResolvedComponent, ResolverError};
use anyhow::Error;
use async_trait::async_trait;
use directed_graph::DirectedGraph;
use fuchsia_component::client as fclient;
use std::sync::Arc;
use vfs::directory::entry::OpenRequest;
use vfs::remote::remote_dir;
use {fidl_fuchsia_component_resolution as fresolution, fidl_fuchsia_io as fio};

pub static SCHEME: &str = "realm-builder";
pub static RUNNER_NAME: &str = "realm_builder";

/// Resolves component URLs with the "realm-builder" scheme, which supports loading components from
/// the fuchsia.component.resolution.Resolver protocol in component_manager's namespace.
///
/// Also runs components with the "realm-builder" runner, which supports launching components
/// through the fuchsia.component.runner.ComponentRunner protocol in component manager's namespace.
///
/// Both of these protocols are typically implemented by the realm builder library, for use when
/// integration testing a nested component manager.
#[derive(Clone, Debug)]
pub struct RealmBuilderResolver {
    resolver_proxy: fresolution::ResolverProxy,
}

impl RealmBuilderResolver {
    /// Create a new RealmBuilderResolver. This opens connections to the needed protocols
    /// in the namespace.
    pub fn new() -> Result<RealmBuilderResolver, Error> {
        Ok(RealmBuilderResolver {
            resolver_proxy: fclient::connect_to_protocol_at_path::<fresolution::ResolverMarker>(
                "/svc/fuchsia.component.resolver.RealmBuilder",
            )?,
        })
    }

    async fn resolve_async(
        &self,
        component_url: &str,
        some_incoming_context: Option<&fresolution::Context>,
    ) -> Result<fresolution::Component, fresolution::ResolverError> {
        let res = if let Some(context) = some_incoming_context {
            self.resolver_proxy
                .resolve_with_context(component_url, &context)
                .await
                .expect("resolve_with_context failed in realm builder resolver")
        } else {
            self.resolver_proxy
                .resolve(component_url)
                .await
                .expect("resolve failed in realm builder resolver")
        };
        res
    }
}

#[async_trait]
impl Resolver for RealmBuilderResolver {
    async fn resolve(
        &self,
        component_address: &ComponentAddress,
    ) -> Result<ResolvedComponent, ResolverError> {
        let (component_url, some_context) = component_address.to_url_and_context();
        let fresolution::Component {
            url: _,
            decl,
            package,
            config_values,
            resolution_context,
            abi_revision,
            ..
        } = self
            .resolve_async(component_url, some_context.map(|context| context.into()).as_ref())
            .await?;
        let context_to_resolve_children = resolution_context.map(Into::into);
        let mut dependencies = DirectedGraph::new();
        let decl = resolving::read_and_validate_manifest(&decl.unwrap(), &mut dependencies)?;
        let config_values = if let Some(data) = config_values {
            Some(resolving::read_and_validate_config_values(&data)?)
        } else {
            None
        };
        Ok(ResolvedComponent {
            context_to_resolve_children,
            decl,
            package: package.map(|p| p.try_into()).transpose()?,
            config_values,
            abi_revision: abi_revision.map(Into::into),
            dependencies,
        })
    }
}

pub struct RealmBuilderRunnerFactory {}

impl RealmBuilderRunnerFactory {
    pub fn new() -> Self {
        RealmBuilderRunnerFactory {}
    }
}

impl BuiltinRunnerFactory for RealmBuilderRunnerFactory {
    fn get_scoped_runner(
        self: Arc<Self>,
        _checker: ScopedPolicyChecker,
        mut open_request: OpenRequest<'_>,
    ) -> Result<(), zx::Status> {
        open_request.prepend_path(&"fuchsia.component.runner.RealmBuilder".try_into().unwrap());
        open_request.open_remote(remote_dir(
            fuchsia_fs::directory::open_in_namespace("/svc", fio::PERM_READABLE).unwrap(),
        ))
    }
}
