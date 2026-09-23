// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use cm_rust::offer::{OfferDecl, OfferDeclCommon, OfferSource};
use cm_rust::{
    Availability, CapabilityDecl, CapabilityTypeName, ConfigValueSpec, DependencyType,
    DirectoryDecl, ExposeDecl, ExposeDeclCommon, ExposeDirectoryDecl, ExposeProtocolDecl,
    ExposeSource, ExposeTarget, FidlIntoNative, NativeIntoFidl, ProtocolDecl, SourceName, UseDecl,
    UseProtocolDecl, UseSource, append_box, push_box,
};
use cm_types::{LongName, Path, RelativePath};
use directed_graph::DirectedGraph;
use fidl::endpoints::{DiscoverableProtocolMarker, ProtocolMarker, Proxy, ServerEnd};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fcdecl;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_component_test as ftest;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_inspect::InspectSinkMarker;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_logger::LogSinkMarker;
use fuchsia_async as fasync;
use fuchsia_component::server as fserver;
use futures::future::BoxFuture;
use futures::lock::Mutex;
use futures::{FutureExt, StreamExt, TryStreamExt, join};
use log::*;
use std::collections::HashMap;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use thiserror::Error;
use url::Url;
use vfs::execution_scope::ExecutionScope;
use zx_status;

#[cfg(fuchsia_api_level_less_than = "HEAD")]
use {fuchsia_sync as _, rand as _};

mod builtin;
mod resolver;
mod runner;

const DIAGNOSTICS_DICT_NAME: &str = "diagnostics";

static BINDER_EXPOSE_DECL: LazyLock<cm_rust::ExposeDecl> = LazyLock::new(|| {
    cm_rust::ExposeDecl::Protocol(cm_rust::ExposeProtocolDecl {
        source: cm_rust::ExposeSource::Framework,
        source_name: fcomponent::BinderMarker::DEBUG_NAME.parse().unwrap(),
        source_dictionary: Default::default(),
        target: cm_rust::ExposeTarget::Parent,
        target_name: fcomponent::BinderMarker::DEBUG_NAME.parse().unwrap(),
        // TODO(https://fxbug.dev/42058594): Support optional exposes.
        availability: cm_rust::Availability::Required,
    })
});

static CAPABILITIES_ROUTED_TO_ALL: LazyLock<[(CapabilityTypeName, cm_types::Name); 3]> =
    LazyLock::new(|| {
        [
            (
                CapabilityTypeName::Dictionary,
                cm_types::Name::from_str(DIAGNOSTICS_DICT_NAME).unwrap(),
            ),
            // TODO(https://fxbug.dev/345827642): remove the explicit routing of LogSink and InspectSink.
            // Components should instead use these protocols from the diagnostics dictionary.
            (
                CapabilityTypeName::Protocol,
                cm_types::Name::from_str(LogSinkMarker::PROTOCOL_NAME).unwrap(),
            ),
            (
                CapabilityTypeName::Protocol,
                cm_types::Name::from_str(InspectSinkMarker::PROTOCOL_NAME).unwrap(),
            ),
        ]
    });

#[cfg(fuchsia_api_level_less_than = "25")]
static CAPABILITIES_ROUTED_TO_ALL: LazyLock<[(CapabilityTypeName, cm_types::Name); 2]> =
    LazyLock::new(|| {
        [
            (
                CapabilityTypeName::Protocol,
                cm_types::Name::from_str(LogSinkMarker::PROTOCOL_NAME).unwrap(),
            ),
            (
                CapabilityTypeName::Protocol,
                cm_types::Name::from_str(InspectSinkMarker::PROTOCOL_NAME).unwrap(),
            ),
        ]
    });

#[fuchsia::main]
async fn main() {
    info!("Started.");

    let mut fs = fserver::ServiceFs::new_local();
    let registry = resolver::Registry::new();
    let runner = runner::Runner::new();

    let registry_clone = registry.clone();
    fs.dir("svc").add_fidl_service(move |stream| registry_clone.run_resolver_service(stream));

    let runner_clone = runner.clone();
    fs.dir("svc").add_fidl_service(move |stream| runner_clone.run_runner_service(stream));

    let execution_scope = ExecutionScope::new();

    let execution_scope_clone = execution_scope.clone();
    fs.dir("svc").add_fidl_service(move |stream| {
        let factory = RealmBuilderFactory::new(
            registry.clone(),
            runner.clone(),
            execution_scope_clone.clone(),
        );
        execution_scope_clone.spawn(async move {
            if let Err(err) = factory.handle_stream(stream).await {
                error!(err:%; "Encountered unexpected error.");
            }
        });
    });

    fs.take_and_serve_directory_handle().expect("Did not receive directory handle.");

    join!(execution_scope.wait(), fs.collect::<()>());
}

/// This struct tracks the contents of the realm, specifically the URLs for manifests in it and IDs
/// for the local components in it. This data is then used after the realm becomes unusable to
/// delete the realm's contents from the runner and resolver.
#[derive(Debug, Default)]
struct ManagedRealmContents {
    urls: Vec<String>,
    local_component_ids: Vec<runner::LocalComponentId>,
}

impl ManagedRealmContents {
    fn add_url(&mut self, url: String) {
        self.urls.push(url);
    }

    fn add_local_component_id(&mut self, local_component_id: runner::LocalComponentId) {
        self.local_component_ids.push(local_component_id);
    }

    async fn delete(&self, registry: Arc<resolver::Registry>, runner: Arc<runner::Runner>) {
        for url in &self.urls {
            registry.delete_manifest(url).await;
        }
        for local_component_id in &self.local_component_ids {
            runner.delete_component(local_component_id).await;
        }
    }

    fn watch_channel_and_delete_on_peer_closed(
        self_: Arc<Mutex<Self>>,
        handle: fcrunner::ComponentRunnerProxy,
        registry: Arc<resolver::Registry>,
        runner: Arc<runner::Runner>,
    ) {
        fasync::Task::spawn(async move {
            let on_closed = handle.as_channel().on_closed();
            // The only possible return values are ok, deadline exceeded, or peer closed. Since
            // we're already looking for peer closed, and we haven't set a deadline, we don't care
            // about the actual contents of any error message here.
            let _ = on_closed.await;
            self_.lock().await.delete(registry, runner).await;
        })
        .detach();
    }
}

struct RealmBuilderFactory {
    registry: Arc<resolver::Registry>,
    runner: Arc<runner::Runner>,
    execution_scope: ExecutionScope,
}

impl RealmBuilderFactory {
    fn new(
        registry: Arc<resolver::Registry>,
        runner: Arc<runner::Runner>,
        execution_scope: ExecutionScope,
    ) -> Self {
        Self { registry, runner, execution_scope }
    }

    async fn handle_stream(
        self,
        mut stream: ftest::RealmBuilderFactoryRequestStream,
    ) -> Result<(), anyhow::Error> {
        while let Some(req) = stream.try_next().await? {
            match req {
                ftest::RealmBuilderFactoryRequest::CreateFromRelativeUrl {
                    pkg_dir_handle,
                    relative_url,
                    realm_server_end,
                    builder_server_end,
                    responder,
                } => {
                    if !is_fragment_only_url(&relative_url) {
                        responder.send(Err(ftest::RealmBuilderError::UrlIsNotRelative))?;
                        continue;
                    }
                    let pkg_dir = pkg_dir_handle.into_proxy();
                    if let Err(e) = pkg_dir.query().await.context(
                        "Invoking `fuchsia.unknown/Queryable.query` on provided `pkg_dir` failed.",
                    ) {
                        responder.send(Err(ftest::RealmBuilderError::InvalidPkgDirHandle))?;
                        return Err(e);
                    }
                    let realm_node = match RealmNode2::load_from_pkg(
                        &relative_url,
                        Clone::clone(&pkg_dir),
                    )
                    .await
                    {
                        Ok(realm_node) => realm_node,
                        Err(err) => {
                            warn!(method = "RealmBuilderFactory.CreateFromRelativeUrl", message:% = err; "");
                            responder.send(Err(err.into()))?;
                            continue;
                        }
                    };
                    self.create_realm_and_builder(
                        realm_node,
                        pkg_dir,
                        realm_server_end,
                        builder_server_end,
                    )?;
                    responder.send(Ok(()))?;
                }
                ftest::RealmBuilderFactoryRequest::Create {
                    pkg_dir_handle,
                    realm_server_end,
                    builder_server_end,
                    responder,
                } => {
                    let pkg_dir = pkg_dir_handle.into_proxy();
                    if let Err(err) = pkg_dir.query().await.context(
                        "Invoking `fuchsia.unknown/Queryable.query` on provided `pkg_dir` failed.",
                    ) {
                        warn!(method = "RealmBuilderFactory.Create", message:% = err; "");
                        responder.send(Err(ftest::RealmBuilderError::InvalidPkgDirHandle))?;
                        continue;
                    }

                    self.create_realm_and_builder(
                        RealmNode2::new(),
                        pkg_dir,
                        realm_server_end,
                        builder_server_end,
                    )?;
                    responder.send(Ok(()))?;
                }
            }
        }
        Ok(())
    }

    fn create_realm_and_builder(
        &self,
        realm_node: RealmNode2,
        pkg_dir: fio::DirectoryProxy,
        realm_server_end: ServerEnd<ftest::RealmMarker>,
        builder_server_end: ServerEnd<ftest::BuilderMarker>,
    ) -> Result<(), anyhow::Error> {
        let runner_proxy_placeholder = Arc::new(Mutex::new(None));
        let realm_contents = Arc::new(Mutex::new(ManagedRealmContents::default()));

        let realm_stream = realm_server_end.into_stream();

        let realm_has_been_built = Arc::new(AtomicBool::new(false));

        let realm = Realm {
            pkg_dir: Clone::clone(&pkg_dir),
            realm_node: realm_node.clone(),
            registry: self.registry.clone(),
            runner: self.runner.clone(),
            runner_proxy_placeholder: runner_proxy_placeholder.clone(),
            realm_path: vec![],
            execution_scope: self.execution_scope.clone(),
            realm_has_been_built: realm_has_been_built.clone(),
            realm_contents: realm_contents.clone(),
        };

        self.execution_scope.spawn(async move {
            if let Err(err) = realm.handle_stream(realm_stream).await {
                error!(err:%; "`Realm` server unexpectedly failed.");
            }
        });

        let builder_stream = builder_server_end.into_stream();

        let builder = Builder {
            pkg_dir: Clone::clone(&pkg_dir),
            realm_node,
            registry: self.registry.clone(),
            runner: self.runner.clone(),
            runner_proxy_placeholder: runner_proxy_placeholder,
            realm_has_been_built: realm_has_been_built,
            realm_contents,
        };
        self.execution_scope.spawn(async move {
            if let Err(err) = builder.handle_stream(builder_stream).await {
                error!(err:%; "`Builder` server unexpectedly failed.");
            }
        });
        Ok(())
    }
}

struct Builder {
    pkg_dir: fio::DirectoryProxy,
    realm_node: RealmNode2,
    registry: Arc<resolver::Registry>,
    runner: Arc<runner::Runner>,
    runner_proxy_placeholder: Arc<Mutex<Option<fcrunner::ComponentRunnerProxy>>>,
    realm_has_been_built: Arc<AtomicBool>,
    realm_contents: Arc<Mutex<ManagedRealmContents>>,
}

impl Builder {
    async fn handle_stream(
        &self,
        stream: ftest::BuilderRequestStream,
    ) -> Result<(), anyhow::Error> {
        let mut build_called_successfully = false;
        let stream_handling_results =
            self.handle_stream_helper(stream, &mut build_called_successfully).await;

        // If we haven't had a successful call to `Build`, then the client hasn't been given a URL
        // for any of the components in this realm and it's not possible for us to ever get
        // resolver or runner requests for any of these components. We can safely clean up the data
        // that's stored in the runner and resolver for this realm.
        if !build_called_successfully {
            self.realm_contents
                .lock()
                .await
                .delete(self.registry.clone(), self.runner.clone())
                .await;
        }
        stream_handling_results
    }

    async fn handle_stream_helper(
        &self,
        mut stream: ftest::BuilderRequestStream,
        build_called_successfully: &mut bool,
    ) -> Result<(), anyhow::Error> {
        while let Some(req) = stream.try_next().await? {
            match req {
                ftest::BuilderRequest::Build { runner, responder } => {
                    if self.realm_has_been_built.swap(true, Ordering::Relaxed) {
                        warn!(method = "Builder.Build", message:% = RealmBuilderError::BuildAlreadyCalled; "");
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }

                    let runner_proxy = runner.into_proxy();
                    *self.runner_proxy_placeholder.lock().await = Some(Clone::clone(&runner_proxy));
                    let res = self
                        .realm_node
                        .build(
                            self.registry.clone(),
                            self.realm_contents.clone(),
                            vec![],
                            Clone::clone(&self.pkg_dir),
                        )
                        .await;
                    match res {
                        Ok(url) => {
                            responder.send(Ok(&url))?;
                            *build_called_successfully = true;
                            ManagedRealmContents::watch_channel_and_delete_on_peer_closed(
                                self.realm_contents.clone(),
                                runner_proxy,
                                self.registry.clone(),
                                self.runner.clone(),
                            );
                        }
                        Err(err) => {
                            warn!(method = "Builder.Build", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

struct Realm {
    pkg_dir: fio::DirectoryProxy,
    realm_node: RealmNode2,
    registry: Arc<resolver::Registry>,
    runner: Arc<runner::Runner>,
    runner_proxy_placeholder: Arc<Mutex<Option<fcrunner::ComponentRunnerProxy>>>,
    realm_has_been_built: Arc<AtomicBool>,
    realm_path: Vec<String>,
    execution_scope: ExecutionScope,
    realm_contents: Arc<Mutex<ManagedRealmContents>>,
}

impl Realm {
    async fn handle_stream(
        &self,
        mut stream: ftest::RealmRequestStream,
    ) -> Result<(), anyhow::Error> {
        while let Some(req) = stream.try_next().await? {
            match req {
                ftest::RealmRequest::AddChild { name, url, options, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    match self.add_child(name.clone(), url.clone(), options).await {
                        Ok(()) => responder.send(Ok(()))?,
                        Err(err) => {
                            warn!(method = "Realm.AddChild", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::AddChildFromDecl { name, decl, options, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    match self.add_child_from_decl(name.clone(), decl, options).await {
                        Ok(()) => responder.send(Ok(()))?,
                        Err(err) => {
                            warn!(method = "Realm.AddChildFromDecl", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::AddLocalChild { name, options, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    match self.add_local_child(name.clone(), options).await {
                        Ok(()) => responder.send(Ok(()))?,
                        Err(err) => {
                            warn!(method = "Realm.AddLocalChild", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::AddChildRealm { name, options, child_realm, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    match self.add_child_realm(name.clone(), options, child_realm).await {
                        Ok(()) => responder.send(Ok(()))?,
                        Err(err) => {
                            warn!(method = "Realm.AddChildRealm", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::AddChildRealmFromRelativeUrl {
                    name,
                    relative_url,
                    options,
                    child_realm,
                    responder,
                } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    if !is_fragment_only_url(&relative_url) {
                        responder.send(Err(ftest::RealmBuilderError::UrlIsNotRelative))?;
                        continue;
                    }
                    match self
                        .add_child_realm_from_relative_url(
                            name.clone(),
                            relative_url,
                            options,
                            child_realm,
                        )
                        .await
                    {
                        Ok(()) => responder.send(Ok(()))?,
                        Err(err) => {
                            warn!(method = "Realm.AddChildRealmFromRelativeUrl", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                fidl_fuchsia_component_test::RealmRequest::AddChildRealmFromDecl {
                    name,
                    decl,
                    options,
                    child_realm,
                    responder,
                } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }

                    match self
                        .add_child_realm_from_decl(name.clone(), decl, options, child_realm)
                        .await
                    {
                        Ok(()) => responder.send(Ok(()))?,
                        Err(err) => {
                            warn!(method = "Realm.AddChildRealmFromDecl", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::GetComponentDecl { name, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    match self.get_component_decl(name.clone()).await {
                        Ok(decl) => responder.send(Ok(&decl))?,
                        Err(err) => {
                            warn!(method = "Realm.GetComponentDecl", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::ReplaceComponentDecl { name, component_decl, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    match self.replace_component_decl(name.clone(), component_decl).await {
                        Ok(()) => responder.send(Ok(()))?,
                        Err(err) => {
                            warn!(method = "Realm.ReplaceComponentDecl", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::GetRealmDecl { responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    responder.send(Ok(&self.get_realm_decl().await))?;
                }
                ftest::RealmRequest::ReplaceRealmDecl { component_decl, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    match self.replace_realm_decl(component_decl).await {
                        Ok(()) => responder.send(Ok(()))?,
                        Err(err) => {
                            warn!(method = "Realm.ReplaceRealmDecl", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::UseNestedComponentManager {
                    component_manager_relative_url,
                    responder,
                } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    match self.use_nested_component_manager(&component_manager_relative_url).await {
                        Ok(()) => responder.send(Ok(()))?,
                        Err(err) => {
                            warn!(method = "Realm.UseNestedComponentManager", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::AddRoute { capabilities, from, to, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    match self.realm_node.route_capabilities(capabilities, from, None, to).await {
                        Ok(()) => {
                            responder.send(Ok(()))?;
                        }
                        Err(err) => {
                            warn!(method = "Realm.AddRoute", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                #[cfg(fuchsia_api_level_at_least = "29")]
                ftest::RealmRequest::AddRouteFromDictionary {
                    capabilities,
                    from,
                    from_dictionary,
                    to,
                    responder,
                } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    let from_dictionary =
                        if from_dictionary == "." { None } else { Some(from_dictionary) };
                    match self
                        .realm_node
                        .route_capabilities(capabilities, from, from_dictionary, to)
                        .await
                    {
                        Ok(()) => {
                            responder.send(Ok(()))?;
                        }
                        Err(err) => {
                            warn!(method = "Realm.AddRoute", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::ReadOnlyDirectory {
                    name,
                    to,
                    directory_contents,
                    responder,
                } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    match self.read_only_directory(name, to, directory_contents).await {
                        Ok(()) => {
                            responder.send(Ok(()))?;
                        }
                        Err(err) => {
                            warn!(method = "Realm.ReadOnlyDirectory", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                #[cfg(fuchsia_api_level_at_least = "HEAD")]
                ftest::RealmRequest::AddStorage { name, to, storage_admin, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }
                    match self.add_storage(name, to, storage_admin).await {
                        Ok(()) => {
                            responder.send(Ok(()))?;
                        }
                        Err(err) => {
                            warn!(method = "Realm.Storage", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::InitMutableConfigFromPackage { name, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }

                    self.realm_node
                        .get_sub_realm(&name)
                        .await?
                        .state
                        .lock()
                        .await
                        .config_override_policy = ConfigOverridePolicy::LoadPackagedValuesFirst;

                    responder.send(Ok(()))?;
                }
                ftest::RealmRequest::InitMutableConfigToEmpty { name, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }

                    self.realm_node
                        .get_sub_realm(&name)
                        .await?
                        .state
                        .lock()
                        .await
                        .config_override_policy = ConfigOverridePolicy::RequireAllValuesFromBuilder;

                    responder.send(Ok(()))?;
                }
                ftest::RealmRequest::AddCapability { capability, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }

                    match self.realm_node.add_capability(capability).await {
                        Ok(()) => {
                            responder.send(Ok(()))?;
                        }
                        Err(err) => {
                            warn!(method = "Realm.AddCapability", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::AddCollection { collection, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }

                    match self.realm_node.add_collection(collection).await {
                        Ok(()) => {
                            responder.send(Ok(()))?;
                        }
                        Err(err) => {
                            warn!(method = "Realm.AddCollection", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::AddEnvironment { environment, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                        continue;
                    }

                    match self.realm_node.add_environment(environment).await {
                        Ok(()) => {
                            responder.send(Ok(()))?;
                        }
                        Err(err) => {
                            warn!(method = "Realm.AddEnvironment", message:% = err; "");
                            responder.send(Err(err.into()))?;
                        }
                    }
                }
                ftest::RealmRequest::SetConfigValue { name, key, value, responder } => {
                    if self.realm_has_been_built.load(Ordering::Relaxed) {
                        responder.send(Err(ftest::RealmBuilderError::BuildAlreadyCalled))?;
                    } else {
                        match self.set_config_value(name, key, value).await {
                            Ok(()) => {
                                responder.send(Ok(()))?;
                            }
                            Err(err) => {
                                warn!(method = "Realm.SetConfigValue", message:% = err; "");
                                responder.send(Err(err.into()))?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn add_child(
        &self,
        name: String,
        url: String,
        options: ftest::ChildOptions,
    ) -> Result<(), RealmBuilderError> {
        let name = name.parse().map_err(|_| RealmBuilderError::ChildNameInvalid)?;
        if is_fragment_only_url(&url) {
            let child_realm_node =
                RealmNode2::load_from_pkg(&url, Clone::clone(&self.pkg_dir)).await?;
            self.realm_node.add_child(name, options, child_realm_node).await
        } else {
            self.realm_node.add_child_decl(name, url, options).await
        }
    }

    async fn add_child_from_decl(
        &self,
        name: String,
        component_decl: fcdecl::Component,
        options: ftest::ChildOptions,
    ) -> Result<(), RealmBuilderError> {
        let name: LongName = name.parse().map_err(|_| RealmBuilderError::ChildNameInvalid)?;
        if let Err(e) = cm_fidl_validator::validate(&component_decl, &mut DirectedGraph::new()) {
            return Err(RealmBuilderError::InvalidComponentDeclWithName(
                name.to_string(),
                to_tabulated_string(e),
            ));
        }
        let child_realm_node = RealmNode2::new_from_decl(component_decl.fidl_into_native(), false);
        self.realm_node.add_child(name, options, child_realm_node).await
    }

    async fn add_local_child(
        &self,
        name: String,
        options: ftest::ChildOptions,
    ) -> Result<(), RealmBuilderError> {
        let name: LongName = name.parse().map_err(|_| RealmBuilderError::ChildNameInvalid)?;
        let local_component_id =
            self.runner.register_local_component(self.runner_proxy_placeholder.clone()).await;
        self.realm_contents.lock().await.add_local_component_id(local_component_id.clone());
        let mut child_path = self.realm_path.clone();
        child_path.push(name.to_string());
        let child_realm_node = RealmNode2::new_from_decl(
            new_decl_with_program_entries(vec![
                (runner::LOCAL_COMPONENT_ID_KEY.to_string(), local_component_id.into()),
                (ftest::LOCAL_COMPONENT_NAME_KEY.to_string(), child_path.join("/")),
            ]),
            true,
        );
        self.realm_node.add_child(name, options, child_realm_node).await
    }

    async fn add_child_realm(
        &self,
        name: String,
        options: ftest::ChildOptions,
        child_realm_server_end: ServerEnd<ftest::RealmMarker>,
    ) -> Result<(), RealmBuilderError> {
        self.add_child_realm_with_node(name, options, child_realm_server_end, RealmNode2::new())
            .await
    }

    #[allow(unused)]
    async fn add_child_realm_from_relative_url(
        &self,
        name: String,
        relative_url: String,
        options: ftest::ChildOptions,
        child_realm_server_end: ServerEnd<ftest::RealmMarker>,
    ) -> Result<(), RealmBuilderError> {
        let node = RealmNode2::load_from_pkg(&relative_url, Clone::clone(&self.pkg_dir)).await?;
        self.add_child_realm_with_node(name, options, child_realm_server_end, node).await
    }

    #[allow(unused)]
    async fn add_child_realm_from_decl(
        &self,
        name: String,
        decl: fcdecl::Component,
        options: ftest::ChildOptions,
        child_realm_server_end: ServerEnd<ftest::RealmMarker>,
    ) -> Result<(), RealmBuilderError> {
        if let Err(e) = cm_fidl_validator::validate(&decl, &mut DirectedGraph::new()) {
            return Err(RealmBuilderError::InvalidComponentDeclWithName(
                name,
                to_tabulated_string(e),
            ));
        }
        let node = RealmNode2::new_from_decl(decl.fidl_into_native(), true);
        self.add_child_realm_with_node(name, options, child_realm_server_end, node).await
    }

    // `Realm::handle_stream` calls `Realm::add_child_realm` which calls `Realm::handle_stream`.
    // Cycles are not allowed in constructed futures, so we need to place this in a `BoxFuture` to
    // break the cycle.
    fn add_child_realm_with_node(
        &self,
        name: String,
        options: ftest::ChildOptions,
        child_realm_server_end: ServerEnd<ftest::RealmMarker>,
        child_realm_node: RealmNode2,
    ) -> BoxFuture<'static, Result<(), RealmBuilderError>> {
        let mut child_path = self.realm_path.clone();
        child_path.push(name.clone());

        let child_realm = Realm {
            pkg_dir: Clone::clone(&self.pkg_dir),
            realm_node: child_realm_node.clone(),
            registry: self.registry.clone(),
            runner: self.runner.clone(),
            runner_proxy_placeholder: self.runner_proxy_placeholder.clone(),
            realm_path: child_path.clone(),
            execution_scope: self.execution_scope.clone(),
            realm_has_been_built: self.realm_has_been_built.clone(),
            realm_contents: self.realm_contents.clone(),
        };

        let self_realm_node = self.realm_node.clone();
        let self_execution_scope = self.execution_scope.clone();

        async move {
            let name: LongName = name.parse().map_err(|_| RealmBuilderError::ChildNameInvalid)?;
            let child_realm_stream = child_realm_server_end.into_stream();
            self_realm_node.add_child(name, options, child_realm_node).await?;

            self_execution_scope.spawn(async move {
                if let Err(e) = child_realm.handle_stream(child_realm_stream).await {
                    error!(
                        "|Realm| server for child \"{}\" unexpectedly failed: {}",
                        child_path.join("/"),
                        e
                    );
                }
            });

            Ok(())
        }
        .boxed()
    }

    async fn get_component_decl(
        &self,
        name: String,
    ) -> Result<fcdecl::Component, RealmBuilderError> {
        let child_node = self.realm_node.get_sub_realm(&name).await?;
        Ok(child_node.get_decl().await.native_into_fidl())
    }

    async fn replace_component_decl(
        &self,
        name: String,
        component_decl: fcdecl::Component,
    ) -> Result<(), RealmBuilderError> {
        let child_node = self.realm_node.get_sub_realm(&name).await?;
        child_node.replace_decl_with_untrusted(component_decl).await
    }

    async fn get_realm_decl(&self) -> fcdecl::Component {
        self.realm_node.get_decl().await.native_into_fidl()
    }

    async fn replace_realm_decl(
        &self,
        component_decl: fcdecl::Component,
    ) -> Result<(), RealmBuilderError> {
        self.realm_node.replace_decl_with_untrusted(component_decl).await
    }

    #[allow(unused)]
    async fn use_nested_component_manager(
        &self,
        component_manager_fragment_only_url: &str,
    ) -> Result<(), RealmBuilderError> {
        let realm_url = self
            .realm_node
            .build(
                self.registry.clone(),
                self.realm_contents.clone(),
                vec![],
                Clone::clone(&self.pkg_dir),
            )
            .await?;
        {
            let mut state = self.realm_node.state.lock().await;
            let (decl, mutable_children) = nested_component_manager_decl(
                &state.decl,
                &self.pkg_dir,
                &realm_url,
                component_manager_fragment_only_url,
            )
            .await?;
            *state = RealmNodeState {
                decl,
                mutable_children,
                config_value_replacements: Default::default(),
                config_override_policy: Default::default(),
            };
        }
        Ok(())
    }

    async fn set_config_value(
        &self,
        name: String,
        key: String,
        value_spec: fcdecl::ConfigValueSpec,
    ) -> Result<(), RealmBuilderError> {
        let child_node = self.realm_node.get_sub_realm(&name).await?;

        let override_policy = child_node.state.lock().await.config_override_policy;
        if matches!(override_policy, ConfigOverridePolicy::DisallowValuesFromBuilder) {
            return Err(RealmBuilderError::ConfigOverrideUnsupported { name });
        }

        let decl = child_node.get_decl().await;
        let config = decl.config.ok_or(RealmBuilderError::NoConfigSchema(name.clone()))?;
        cm_fidl_validator::validate_value_spec(&value_spec)
            .map_err(|e| RealmBuilderError::ConfigValueInvalid(key.clone(), anyhow::anyhow!(e)))?;
        let value_spec: ConfigValueSpec = value_spec.fidl_into_native();
        for (index, field) in config.fields.iter().enumerate() {
            if field.key == key {
                config_encoder::ConfigField::resolve(value_spec.value.clone(), &field).map_err(
                    |e| RealmBuilderError::ConfigValueInvalid(name.clone(), anyhow::anyhow!(e)),
                )?;
                let mut state_guard = child_node.state.lock().await;
                state_guard.config_value_replacements.insert(index, value_spec);
                return Ok(());
            }
        }

        Err(RealmBuilderError::NoSuchConfigField {
            name,
            key,
            present: config.fields.iter().map(|f| f.key.to_string()).collect::<Vec<String>>(),
        })
    }

    async fn read_only_directory(
        &self,
        directory_name: String,
        to: Vec<fcdecl::Ref>,
        directory_contents: ftest::DirectoryContents,
    ) -> Result<(), RealmBuilderError> {
        // Add a new component to the realm to serve the directory capability from
        let dir_name = directory_name.clone();
        let directory_contents = Arc::new(directory_contents);
        let local_component_id = self
            .runner
            .register_builtin_component(move |_namespace, outgoing_dir| {
                builtin::read_only_directory(
                    dir_name.clone(),
                    directory_contents.clone(),
                    outgoing_dir,
                )
                .boxed()
            })
            .await;
        self.realm_contents.lock().await.add_local_component_id(local_component_id.clone());
        let string_id: String = local_component_id.clone().into();
        let child_name: LongName =
            format!("read-only-directory-{}", string_id).parse().expect("should be valid name");

        let child_realm_node = RealmNode2::new_from_decl(
            new_decl_with_program_entries(vec![(
                runner::LOCAL_COMPONENT_ID_KEY.to_string(),
                local_component_id.into(),
            )]),
            true,
        );
        self.realm_node
            .add_child(child_name.clone(), ftest::ChildOptions::default(), child_realm_node)
            .await?;
        let path = Some(format!("/{}", directory_name));
        self.realm_node
            .route_capabilities(
                vec![ftest::Capability::Directory(ftest::Directory {
                    name: Some(directory_name),
                    rights: Some(fio::R_STAR_DIR),
                    path,
                    ..Default::default()
                })],
                fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: child_name.clone().into(),
                    collection: None,
                }),
                None,
                to,
            )
            .await
    }

    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    async fn add_storage(
        &self,
        storage_name: String,
        to: Vec<fcdecl::Ref>,
        storage_admin: Option<ServerEnd<fcomponent::StorageAdminMarker>>,
    ) -> Result<(), RealmBuilderError> {
        let name = storage_name.clone();
        let storage_admin = fuchsia_sync::Mutex::new(storage_admin);
        let local_component_id = self
            .runner
            .register_builtin_component(move |namespace, outgoing_dir| {
                builtin::storage(name.clone(), namespace, outgoing_dir, storage_admin.lock().take())
                    .boxed()
            })
            .await;
        self.realm_contents.lock().await.add_local_component_id(local_component_id.clone());
        let string_id: String = local_component_id.clone().into();
        let child_name: LongName =
            format!("storage-{}", string_id).parse().expect("should be valid name");

        let child_realm_node = RealmNode2::new_from_decl(
            new_decl_with_program_entries(vec![(
                runner::LOCAL_COMPONENT_ID_KEY.to_string(),
                local_component_id.into(),
            )]),
            true,
        );
        self.realm_node
            .add_child(
                child_name.clone(),
                ftest::ChildOptions {
                    startup: Some(fcdecl::StartupMode::Eager),
                    ..Default::default()
                },
                child_realm_node,
            )
            .await?;

        let child_node = self.realm_node.get_sub_realm(child_name.as_str()).await?;
        let mut child_decl = child_node.get_decl().await.native_into_fidl();
        child_decl.uses.get_or_insert(vec![]).push(fcdecl::Use::Protocol(fcdecl::UseProtocol {
            source: Some(fcdecl::Ref::Parent(fcdecl::ParentRef {})),
            source_name: Some("fuchsia.component.StorageAdmin".to_string()),
            target_path: Some("/svc/fuchsia.component.StorageAdmin".to_string()),
            dependency_type: Some(fcdecl::DependencyType::Strong),
            availability: Some(fcdecl::Availability::Required),
            ..Default::default()
        }));
        child_decl.capabilities.get_or_insert(vec![]).push(fcdecl::Capability::Directory(
            fcdecl::Directory {
                name: Some(storage_name.clone()),
                source_path: Some(format!("/{}", storage_name)),
                rights: Some(fio::RW_STAR_DIR),
                ..Default::default()
            },
        ));
        child_decl.exposes.get_or_insert(vec![]).push(fcdecl::Expose::Directory(
            fcdecl::ExposeDirectory {
                source: Some(fcdecl::Ref::Self_(fcdecl::SelfRef {})),
                source_name: Some(storage_name.clone()),
                target: Some(fcdecl::Ref::Parent(fcdecl::ParentRef {})),
                target_name: Some(storage_name.clone()),
                availability: Some(fcdecl::Availability::Required),
                ..Default::default()
            },
        ));
        child_node.replace_decl_with_untrusted(child_decl).await?;

        let mut realm_decl = self.realm_node.get_decl().await.native_into_fidl();
        realm_decl.capabilities.get_or_insert(vec![]).push(fcdecl::Capability::Storage(
            fcdecl::Storage {
                name: Some(storage_name.clone()),
                source: Some(fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: child_name.clone().into(),
                    collection: None,
                })),
                backing_dir: Some(storage_name.clone()),
                subdir: None,
                storage_id: Some(fcdecl::StorageId::StaticInstanceIdOrMoniker),
                ..Default::default()
            },
        ));
        realm_decl.offers.get_or_insert(vec![]).push(fcdecl::Offer::Protocol(
            fcdecl::OfferProtocol {
                source_name: Some("fuchsia.component.StorageAdmin".to_string()),
                source: Some(fcdecl::Ref::Capability(fcdecl::CapabilityRef {
                    name: storage_name.clone().into(),
                })),
                target: Some(fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: child_name.clone().into(),
                    collection: None,
                })),
                target_name: Some("fuchsia.component.StorageAdmin".to_string()),
                availability: Some(fcdecl::Availability::Required),
                dependency_type: Some(fcdecl::DependencyType::Weak),
                ..Default::default()
            },
        ));
        realm_decl.offers.get_or_insert(vec![]).push(fcdecl::Offer::Storage(
            fcdecl::OfferStorage {
                source_name: Some("data".to_string()),
                source: Some(fcdecl::Ref::Parent(fcdecl::ParentRef {})),
                target: Some(fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: child_name.clone().into(),
                    collection: None,
                })),
                target_name: Some("data".to_string()),
                availability: Some(fcdecl::Availability::Required),
                ..Default::default()
            },
        ));
        for target in to {
            realm_decl.offers.get_or_insert(vec![]).push(fcdecl::Offer::Storage(
                fcdecl::OfferStorage {
                    source_name: Some(storage_name.clone()),
                    source: Some(fcdecl::Ref::Self_(fcdecl::SelfRef {})),
                    target: Some(target.clone()),
                    target_name: Some(storage_name.clone()),
                    availability: Some(fcdecl::Availability::Required),
                    ..Default::default()
                },
            ));
            add_use_decl_if_needed(
                self.realm_node.state.lock().await.deref_mut(),
                target,
                ftest::Capability::Storage(ftest::Storage {
                    name: Some(storage_name.clone()),
                    as_: Some(storage_name.clone()),
                    path: Some(format!("/{}", storage_name)),
                    availability: Some(fcdecl::Availability::Required),
                    ..Default::default()
                }),
            )
            .await?;
        }
        self.realm_node.replace_decl_with_untrusted(realm_decl).await?;

        Ok(())
    }
}

fn new_decl_with_program_entries(entries: Vec<(String, String)>) -> cm_rust::ComponentDecl {
    cm_rust::ComponentDecl {
        program: Some(cm_rust::ProgramDecl {
            runner: Some(runner::RUNNER_NAME.parse().unwrap()),
            info: fdata::Dictionary {
                entries: Some(
                    entries
                        .into_iter()
                        .map(|(key, val)| fdata::DictionaryEntry {
                            key: key,
                            value: Some(Box::new(fdata::DictionaryValue::Str(val))),
                        })
                        .collect(),
                ),
                ..Default::default()
            },
        }),
        ..cm_rust::ComponentDecl::default()
    }
}

type MutableChildMap = HashMap<LongName, (ftest::ChildOptions, RealmNode2)>;

#[derive(Debug, Clone, Default)]
struct RealmNodeState {
    decl: cm_rust::ComponentDecl,

    /// Stores indices to configuration values that must be replaced when the config value file
    /// of a component is read in from the package directory during resolve.
    config_value_replacements: HashMap<usize, cm_rust::ConfigValueSpec>,

    /// Policy for allowing values from SetConfigValue and whether to also load a component's
    /// packaged/default values.
    config_override_policy: ConfigOverridePolicy,

    /// Children stored in this HashMap can be mutated. Children stored in `decl.children` can not.
    /// Any children stored in `mutable_children` do NOT have a corresponding `ChildDecl` stored in
    /// `decl.children`, the two should be fully mutually exclusive.
    ///
    /// Suitable `ChildDecl`s for the contents of `mutable_children` are generated and added to
    /// `decl.children` when `commit()` is called.
    mutable_children: MutableChildMap,
}

impl RealmNodeState {
    // Returns true if a child with the given name exists either as a mutable child or as a
    // ChildDecl in this node's ComponentDecl.
    fn contains_child(&self, child_name: &LongName) -> bool {
        self.decl.children.iter().any(|c| c.name.as_str() == child_name.as_str())
            || self.mutable_children.contains_key(child_name.as_str())
    }

    fn add_child_decl(
        &mut self,
        child_name: LongName,
        child_url: String,
        child_options: ftest::ChildOptions,
    ) -> Result<(), RealmBuilderError> {
        // TODO(https://fxbug.dev/42053123): Validate overrides in cm_fidl_validator before
        // converting them to cm_rust.
        let config_overrides: Option<Box<[_]>> = child_options
            .config_overrides
            .map(|c| c.into_iter().map(|o| o.fidl_into_native()).collect());

        push_box(
            &mut self.decl.children,
            cm_rust::ChildDecl {
                name: child_name,
                url: child_url.parse().map_err(|_| RealmBuilderError::UrlInvalid)?,
                startup: match child_options.startup {
                    Some(fcdecl::StartupMode::Lazy) => fcdecl::StartupMode::Lazy,
                    Some(fcdecl::StartupMode::Eager) => fcdecl::StartupMode::Eager,
                    None => fcdecl::StartupMode::Lazy,
                },
                environment: child_options
                    .environment
                    .map(|e| e.parse().map_err(|_| RealmBuilderError::EnvironmentNameInvalid))
                    .transpose()?,
                on_terminate: match child_options.on_terminate {
                    Some(fcdecl::OnTerminate::None) => Some(fcdecl::OnTerminate::None),
                    Some(fcdecl::OnTerminate::Reboot) => Some(fcdecl::OnTerminate::Reboot),
                    None => None,
                },
                config_overrides,
            },
        );
        Ok(())
    }

    /// Function to route common protocols to every component if they are missing.
    ///
    /// `route_common_protocols_from_parent` should be called after `self.mutable_children` have
    /// been inserted to `self.decl.children`.
    ///
    /// Protocols are matched via their `target_name`.
    fn route_common_protocols_from_parent(&mut self) {
        for child in &self.decl.children {
            for (type_name, capability_name) in &*CAPABILITIES_ROUTED_TO_ALL {
                if self.decl.offers.iter().any(|offer| {
                    offer.target_name() == capability_name
                        && match offer.target() {
                            cm_rust::offer::OfferTarget::Child(child_ref) => {
                                child_ref.name == child.name
                            }
                            cm_rust::offer::OfferTarget::Collection(_) => true,
                            cm_rust::offer::OfferTarget::Capability(_) => false,
                        }
                }) {
                    continue;
                }
                let decl = match type_name {
                    CapabilityTypeName::Protocol => {
                        cm_rust::offer::OfferDecl::Protocol(cm_rust::offer::OfferProtocolDecl {
                            source: cm_rust::offer::OfferSource::Parent,
                            target: cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                                name: child.name.clone(),
                                collection: None,
                            }),
                            source_name: capability_name.clone(),
                            source_dictionary: Default::default(),
                            target_name: capability_name.clone(),
                            dependency_type: cm_rust::DependencyType::Strong,
                            availability: cm_rust::Availability::Required,
                        })
                    }
                    CapabilityTypeName::Dictionary => {
                        cm_rust::offer::OfferDecl::Dictionary(cm_rust::offer::OfferDictionaryDecl {
                            source: cm_rust::offer::OfferSource::Parent,
                            target: cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                                name: child.name.clone(),
                                collection: None,
                            }),
                            source_name: capability_name.clone(),
                            source_dictionary: Default::default(),
                            target_name: capability_name.clone(),
                            dependency_type: cm_rust::DependencyType::Strong,
                            availability: cm_rust::Availability::Required,
                        })
                    }
                    _ => {
                        unreachable!("we don't route other kinds of capabilities to all components")
                    }
                };

                push_box(&mut self.decl.offers, decl);
            }
        }
    }

    // Returns children whose manifest must be updated during invocations to
    // AddRoute.
    fn get_updateable_children(&mut self) -> HashMap<LongName, &mut RealmNode2> {
        self.mutable_children
            .iter_mut()
            .map(|(key, (_options, child))| (key.clone(), child))
            .filter(|(_k, c)| c.update_decl_in_add_route)
            .collect::<HashMap<_, _>>()
    }

    // Whenever this realm node is going to get a new decl we'd like to validate the new
    // hypothetical decl, but the decl likely references children within `self.mutable_children`.
    // Since these children do not (yet) exist in `decl.children`, the decl will fail validation.
    // To get around this, generate hypothetical `fcdecl::Child` structs and add them to
    // `decl.children`, and then run validation.
    fn validate_with_hypothetical_children(
        &self,
        mut decl: fcdecl::Component,
    ) -> Result<(), RealmBuilderError> {
        let child_decls =
            self.mutable_children.iter().map(|(name, _options_and_node)| fcdecl::Child {
                name: Some(name.to_string()),
                url: Some("invalid://url".to_string()),
                startup: Some(fcdecl::StartupMode::Lazy),
                ..Default::default()
            });
        decl.children.get_or_insert(vec![]).extend(child_decls);
        if let Err(e) = cm_fidl_validator::validate(&decl, &mut DirectedGraph::new()) {
            return Err(RealmBuilderError::InvalidComponentDecl(to_tabulated_string(e)));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum ConfigOverridePolicy {
    #[default]
    DisallowValuesFromBuilder,
    LoadPackagedValuesFirst,
    RequireAllValuesFromBuilder,
}

#[derive(Debug, Clone)]
struct RealmNode2 {
    state: Arc<Mutex<RealmNodeState>>,

    /// We shouldn't mutate component decls that are loaded from the test package. Track the source
    /// of this component declaration here, so we know when to treat it as immutable.
    component_loaded_from_pkg: bool,

    /// Flag used to determine if this component's manifest should be updated
    /// when a capability is routed to or from it during invocations of AddRoute.
    pub update_decl_in_add_route: bool,
}

impl RealmNode2 {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RealmNodeState::default())),
            component_loaded_from_pkg: false,
            update_decl_in_add_route: false,
        }
    }

    fn new_from_decl(decl: cm_rust::ComponentDecl, update_decl_in_add_route: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(RealmNodeState { decl, ..RealmNodeState::default() })),
            component_loaded_from_pkg: false,
            update_decl_in_add_route,
        }
    }

    async fn get_decl(&self) -> cm_rust::ComponentDecl {
        self.state.lock().await.decl.clone()
    }

    // Validates `new_decl`, confirms that `new_decl` isn't overwriting anything necessary for the
    // realm builder runner to work, and then replaces this realm's decl with `new_decl`.
    async fn replace_decl_with_untrusted(
        &self,
        new_decl: fcdecl::Component,
    ) -> Result<(), RealmBuilderError> {
        let mut state_guard = self.state.lock().await;
        state_guard.validate_with_hypothetical_children(new_decl.clone())?;
        let new_decl = new_decl.fidl_into_native();
        let () = validate_program_modifications(&state_guard.decl, &new_decl)?;
        state_guard.decl = new_decl;
        Ok(())
    }

    // Replaces the decl for this realm with `new_decl`.
    async fn replace_decl(
        &self,
        new_decl: cm_rust::ComponentDecl,
    ) -> Result<(), RealmBuilderError> {
        self.state.lock().await.decl = new_decl;
        Ok(())
    }

    async fn add_child(
        &self,
        child_name: LongName,
        child_options: ftest::ChildOptions,
        node: RealmNode2,
    ) -> Result<(), RealmBuilderError> {
        let mut state_guard = self.state.lock().await;
        if state_guard.contains_child(&child_name) {
            return Err(RealmBuilderError::ChildAlreadyExists(child_name.to_string()));
        }
        state_guard.mutable_children.insert(child_name.clone().into(), (child_options, node));
        Ok(())
    }

    async fn add_child_decl(
        &self,
        child_name: LongName,
        child_url: String,
        child_options: ftest::ChildOptions,
    ) -> Result<(), RealmBuilderError> {
        let mut state_guard = self.state.lock().await;
        if state_guard.contains_child(&child_name) {
            return Err(RealmBuilderError::ChildAlreadyExists(child_name.to_string()));
        }
        state_guard.add_child_decl(child_name, child_url, child_options)
    }

    fn load_from_pkg(
        fragment_only_url: &str,
        test_pkg_dir: fio::DirectoryProxy,
    ) -> BoxFuture<'_, Result<RealmNode2, RealmBuilderError>> {
        async move {
            let path = fragment_only_url.trim_start_matches('#');

            let file_proxy_res =
                fuchsia_fs::directory::open_file(&test_pkg_dir, &path, fuchsia_fs::PERM_READABLE)
                    .await;
            let file_proxy = match file_proxy_res {
                Ok(file_proxy) => file_proxy,
                Err(fuchsia_fs::node::OpenError::OpenError(zx_status::Status::NOT_FOUND)) => {
                    return Err(RealmBuilderError::DeclNotFound(fragment_only_url.into()));
                }
                Err(e) => {
                    return Err(RealmBuilderError::DeclReadError(
                        fragment_only_url.into(),
                        e.into(),
                    ));
                }
            };

            let fidl_decl = fuchsia_fs::file::read_fidl::<fcdecl::Component>(&file_proxy)
                .await
                .map_err(|e| RealmBuilderError::DeclReadError(fragment_only_url.into(), e))?;
            cm_fidl_validator::validate(&fidl_decl, &mut DirectedGraph::new()).map_err(|e| {
                RealmBuilderError::InvalidComponentDeclWithName(
                    fragment_only_url.into(),
                    to_tabulated_string(e),
                )
            })?;

            let mut self_ = RealmNode2::new_from_decl(fidl_decl.fidl_into_native(), false);
            self_.component_loaded_from_pkg = true;
            let mut state_guard = self_.state.lock().await;

            let children = mem::replace(&mut state_guard.decl.children, Box::from([]));
            for child in children {
                if !is_fragment_only_url(child.url.as_str()) {
                    push_box(&mut state_guard.decl.children, child);
                } else {
                    let child_node =
                        RealmNode2::load_from_pkg(child.url.as_str(), Clone::clone(&test_pkg_dir))
                            .await?;

                    // TODO(https://fxbug.dev/42053123): Validate overrides in cm_fidl_validator before
                    // converting them to cm_rust.
                    let config_overrides: Option<Vec<_>> = child.config_overrides.map(|c| {
                        IntoIterator::into_iter(c).map(|o| o.native_into_fidl()).collect()
                    });

                    let child_options = ftest::ChildOptions {
                        startup: match child.startup {
                            fcdecl::StartupMode::Lazy => Some(fcdecl::StartupMode::Lazy),
                            fcdecl::StartupMode::Eager => Some(fcdecl::StartupMode::Eager),
                        },
                        environment: child.environment.map(|e| e.to_string()),
                        on_terminate: match child.on_terminate {
                            Some(fcdecl::OnTerminate::None) => Some(fcdecl::OnTerminate::None),
                            Some(fcdecl::OnTerminate::Reboot) => Some(fcdecl::OnTerminate::Reboot),
                            None => None,
                        },
                        config_overrides,
                        ..Default::default()
                    };
                    state_guard.mutable_children.insert(child.name, (child_options, child_node));
                }
            }

            drop(state_guard);
            Ok(self_)
        }
        .boxed()
    }

    async fn get_sub_realm(&self, child_name: &str) -> Result<RealmNode2, RealmBuilderError> {
        let state_guard = self.state.lock().await;
        if state_guard.decl.children.iter().any(|c| c.name.as_str() == child_name) {
            return Err(RealmBuilderError::ChildDeclNotVisible(child_name.into()));
        }
        state_guard
            .mutable_children
            .get(child_name)
            .cloned()
            .map(|(_, r)| r)
            .ok_or_else(|| RealmBuilderError::NoSuchChild(child_name.into()))
    }

    async fn add_capability(
        &self,
        capability: fcdecl::Capability,
    ) -> Result<(), RealmBuilderError> {
        let mut state_guard = self.state.lock().await;
        let mut capabilities: Vec<_> =
            IntoIterator::into_iter(state_guard.decl.capabilities.clone())
                .map(|c| c.native_into_fidl())
                .collect();
        capabilities.push(capability.clone());
        let res = cm_fidl_validator::validate_namespace_capabilities(&capabilities);
        // Because we are only validating the capability declarations in isolation, if this is a
        // capability for storage that's backed by a child then we're guaranteed to get an invalid
        // child error. Let's ignore those errors specifically, this component's manifest will be
        // validated again when the realm is built so if the child is truly missing then it'll be
        // caught later.
        if let Err(e) = res {
            let errs = e
                .errs
                .into_iter()
                .filter(|e| match e {
                    cm_fidl_validator::error::Error::InvalidChild(_, _) => false,
                    _ => true,
                })
                .collect::<Vec<_>>();
            if !errs.is_empty() {
                return Err(RealmBuilderError::CapabilityInvalid(anyhow::anyhow!(
                    cm_fidl_validator::error::ErrorList { errs }
                )));
            }
        }
        push_if_not_present_box(&mut state_guard.decl.capabilities, capability.fidl_into_native());
        Ok(())
    }

    async fn add_collection(
        &self,
        collection: fcdecl::Collection,
    ) -> Result<(), RealmBuilderError> {
        let mut state_guard = self.state.lock().await;
        push_if_not_present_box(&mut state_guard.decl.collections, collection.fidl_into_native());
        Ok(())
    }

    async fn add_environment(
        &self,
        environment: fcdecl::Environment,
    ) -> Result<(), RealmBuilderError> {
        let mut state_guard = self.state.lock().await;
        push_if_not_present_box(&mut state_guard.decl.environments, environment.fidl_into_native());
        Ok(())
    }

    async fn route_capabilities(
        &self,
        capabilities: Vec<ftest::Capability>,
        from: fcdecl::Ref,
        from_dictionary: Option<String>,
        to: Vec<fcdecl::Ref>,
    ) -> Result<(), RealmBuilderError> {
        if capabilities.is_empty() {
            return Err(RealmBuilderError::CapabilitiesEmpty);
        }

        let mut state_guard = self.state.lock().await;
        if !contains_child(state_guard.deref(), &from) {
            return Err(RealmBuilderError::NoSuchSource(ref_to_string(&from)));
        }

        for capability in capabilities {
            for target in &to {
                if &from == target {
                    return Err(RealmBuilderError::SourceAndTargetMatch(ref_to_string(&from)));
                }

                if !contains_child(state_guard.deref(), target) {
                    return Err(RealmBuilderError::NoSuchTarget(ref_to_string(&target)));
                }

                if let fcdecl::Ref::Capability(fcdecl::CapabilityRef { name: target_name }) = target
                {
                    if !state_guard.decl.capabilities.iter().any(|c| {
                        matches!(c, cm_rust::CapabilityDecl::Dictionary(cm_rust::DictionaryDecl {
                            name,
                            ..
                        }) if name == target_name)
                    }) {
                        return Err(RealmBuilderError::NoSuchTargetCapability(ref_to_string(
                            target,
                        )));
                    }
                }

                fn check_for_parent_target_error(
                    availability: &Option<fcdecl::Availability>,
                ) -> Result<(), RealmBuilderError> {
                    match availability {
                        Some(fcdecl::Availability::Required) | None => Ok(()),
                        _ => {
                            return Err(RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                                "capability availability cannot be \"SameAsTarget\" or \"Optional\" when the target is the parent",
                            )));
                        }
                    }
                }

                if matches!(target, fcdecl::Ref::Parent(_)) {
                    match &capability {
                        ftest::Capability::Protocol(ftest::Protocol { availability, .. })
                        | ftest::Capability::Directory(ftest::Directory { availability, .. })
                        | ftest::Capability::Storage(ftest::Storage { availability, .. })
                        | ftest::Capability::Service(ftest::Service { availability, .. }) => {
                            check_for_parent_target_error(availability)?;
                        }
                        ftest::Capability::Config(ftest::Config { availability, .. }) => {
                            check_for_parent_target_error(availability)?;
                        }
                        ftest::Capability::Dictionary(ftest::Dictionary {
                            availability, ..
                        }) => {
                            check_for_parent_target_error(availability)?;
                        }
                        ftest::Capability::Resolver(ftest::Resolver { .. })
                        | ftest::Capability::Runner(ftest::Runner { .. }) => {
                            // Resolver and Runner capabilities are always required so we do not
                            // need to validate anything here.
                        }
                        _ => {
                            return Err(RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                                "unknown capability type",
                            )));
                        }
                    }
                    let decl = create_expose_decl(
                        capability.clone(),
                        from.clone(),
                        from_dictionary.clone(),
                        ExposingIn::Realm,
                    )?;
                    push_if_not_present_box(&mut state_guard.decl.exposes, decl);
                } else if matches!(target, fcdecl::Ref::Self_(_)) {
                    if let Some(use_decl) =
                        create_use_decl(capability.clone(), from.clone(), from_dictionary.clone())?
                    {
                        push_if_not_present_box(&mut state_guard.decl.uses, use_decl);
                    }
                } else {
                    let decl = create_offer_decl(
                        capability.clone(),
                        from.clone(),
                        from_dictionary.clone(),
                        target.clone(),
                    )?;
                    push_if_not_present_box(&mut state_guard.decl.offers, decl);
                }

                let () = add_use_decl_if_needed(
                    state_guard.deref_mut(),
                    target.clone(),
                    capability.clone(),
                )
                .await?;
            }

            let () = add_expose_decl_if_needed(
                state_guard.deref_mut(),
                from.clone(),
                from_dictionary.clone(),
                capability.clone(),
            )
            .await?;
        }

        Ok(())
    }

    fn build(
        &self,
        registry: Arc<resolver::Registry>,
        realm_contents: Arc<Mutex<ManagedRealmContents>>,
        walked_path: Vec<String>,
        package_dir: fio::DirectoryProxy,
    ) -> BoxFuture<'static, Result<String, RealmBuilderError>> {
        // This function is much cleaner written recursively, but we can't construct recursive
        // futures as the size isn't knowable to rustc at compile time. Put the recursive call
        // into a boxed future, as the redirection makes this possible
        let self_state = self.state.clone();
        let component_loaded_from_pkg = self.component_loaded_from_pkg;
        async move {
            let mut state_guard = self_state.lock().await;
            // Expose the fuchsia.component.Binder protocol from root in order to give users the ability to manually
            // start the realm.
            if walked_path.is_empty() {
                if !state_guard
                    .decl
                    .exposes
                    .iter()
                    .any(|d| d.target_name().as_str() == fcomponent::BinderMarker::DEBUG_NAME)
                {
                    push_box(&mut state_guard.decl.exposes, BINDER_EXPOSE_DECL.clone());
                }
            }

            let mut mutable_children = state_guard.mutable_children.drain().collect::<Vec<_>>();
            mutable_children.sort_unstable_by_key(|t| t.0.clone());
            for (child_name, (child_options, node)) in mutable_children {
                let mut new_path = walked_path.clone();
                new_path.push(child_name.to_string());

                let child_url = node
                    .build(
                        registry.clone(),
                        realm_contents.clone(),
                        new_path,
                        Clone::clone(&package_dir),
                    )
                    .await?;
                state_guard.add_child_decl(child_name, child_url, child_options)?;
            }

            if !component_loaded_from_pkg {
                state_guard.route_common_protocols_from_parent();
            }

            let name =
                if walked_path.is_empty() { "root".to_string() } else { walked_path.join("-") };
            let decl = state_guard.decl.clone().native_into_fidl();
            let config_value_replacements = state_guard.config_value_replacements.clone();
            match registry
                .validate_and_register(
                    &decl,
                    name.clone(),
                    Clone::clone(&package_dir),
                    config_value_replacements,
                    state_guard.config_override_policy,
                )
                .await
            {
                Ok(url) => {
                    realm_contents.lock().await.add_url(url.clone());
                    Ok(url)
                }
                Err(e) => Err(RealmBuilderError::InvalidComponentDeclWithName(
                    name,
                    to_tabulated_string(e),
                )),
            }
        }
        .boxed()
    }
}

/// Derive a declaration for a nested component manager from a root component `root_decl` with
/// `root_url` and the `component_manager_fragment_only_url`,
async fn nested_component_manager_decl(
    root_decl: &cm_rust::ComponentDecl,
    pkg_dir: &fio::DirectoryProxy,
    root_url: &str,
    component_manager_fragment_only_url: &str,
) -> Result<(cm_rust::ComponentDecl, MutableChildMap), RealmBuilderError> {
    /// The default path of the remote directory through which a nested component
    /// manager serves capabilities exposed by the root component.
    const ROOT_CAPABILITY_PATH: &'static str = "/root-exposed";

    /// The default path of the directory in the nested component manager's
    /// namespace where capabilities offered by the realm builder client are placed.
    const CLIENT_CAPABILITY_PASSTHROUGH_PATH: &'static str = "/parent-offered";

    let (passthrough_cap_decls, passthrough_expose_decls): (Vec<_>, Vec<_>) = root_decl
        .exposes
        .iter()
        .filter_map(|exposed| match exposed {
            ExposeDecl::Protocol(decl) => {
                let source_path =
                    Path::new(format!("{}/{}", ROOT_CAPABILITY_PATH, decl.target_name.clone()))
                        .inspect_err(|e| warn!("invalid capability source path: {}", e))
                        .ok()?;
                let capability = ProtocolDecl {
                    name: decl.target_name.clone(),
                    source_path: Some(source_path),
                    #[cfg(fuchsia_api_level_at_least = "HEAD")]
                    delivery: Default::default(),
                };
                let expose = ExposeProtocolDecl {
                    source: ExposeSource::Self_,
                    source_name: decl.target_name.clone(),
                    source_dictionary: Default::default(),
                    target: ExposeTarget::Parent,
                    target_name: decl.target_name.clone(),
                    availability: decl.availability,
                };
                Some((CapabilityDecl::Protocol(capability), ExposeDecl::Protocol(expose)))
            }
            ExposeDecl::Directory(decl) => {
                let source_path =
                    Path::new(format!("{}/{}", ROOT_CAPABILITY_PATH, decl.target_name.clone()))
                        .inspect_err(|e| warn!("invalid capability source path: {}", e))
                        .ok()?;
                let capability = DirectoryDecl {
                    name: decl.target_name.clone(),
                    source_path: Some(source_path),
                    rights: decl.rights.unwrap_or_else(fio::Operations::default),
                };
                let expose = ExposeDirectoryDecl {
                    source: ExposeSource::Self_,
                    source_name: decl.target_name.clone(),
                    source_dictionary: Default::default(),
                    target: ExposeTarget::Parent,
                    target_name: decl.target_name.clone(),
                    rights: decl.rights,
                    subdir: decl.subdir.clone(),
                    availability: decl.availability,
                };
                Some((CapabilityDecl::Directory(capability), ExposeDecl::Directory(expose)))
            }
            d @ ExposeDecl::Service(_)
            | d @ ExposeDecl::Config(_)
            | d @ ExposeDecl::Runner(_)
            | d @ ExposeDecl::Resolver(_) => {
                warn!(
                    "capability type not supported for nested component manager passthrough: {:?}",
                    d
                );
                None
            }
            d @ ExposeDecl::Dictionary(_) => {
                warn!(
                    "capability type not supported for nested component manager passthrough: {:?}",
                    d
                );
                None
            }
        })
        .unzip();

    let mut passthrough_offer_decls: Vec<OfferDecl> = vec![];
    for offer in &root_decl.offers {
        if *offer.source() != OfferSource::Parent {
            continue;
        }
        // A capability can be offered from the parent multiple times, but we
        // should only add one Use for it.
        if passthrough_offer_decls.iter().any(|d| d.source_name() == offer.source_name()) {
            continue;
        }
        match offer {
            OfferDecl::Protocol(_) => {
                passthrough_offer_decls.push(offer.clone());
            }
            d => {
                warn!(
                    "capability type not supported for nested component manager \
                        passthrough: {d:?}"
                );
            }
        }
    }

    let passthrough_use_decls = passthrough_offer_decls
        .into_iter()
        .map(|offer| match offer {
            OfferDecl::Protocol(decl) => UseDecl::Protocol(UseProtocolDecl {
                source: UseSource::Parent,
                source_name: decl.source_name.clone(),
                target_path: Some(
                    Path::new(format!("{CLIENT_CAPABILITY_PASSTHROUGH_PATH}/{}", decl.source_name))
                        .unwrap(),
                ),
                #[cfg(fuchsia_api_level_at_least = "29")]
                numbered_handle: None,
                dependency_type: DependencyType::Strong,
                availability: Availability::default(),
                source_dictionary: decl.source_dictionary,
            }),
            o => {
                unreachable!("unsupported passthrough not caught above? {o:?}");
            }
        })
        .collect::<Vec<UseDecl>>();

    let mut component_manager_decl;
    let mut component_manager_children;
    {
        let tmp_node =
            RealmNode2::load_from_pkg(component_manager_fragment_only_url, Clone::clone(pkg_dir))
                .await?;
        component_manager_decl = cm_rust::ComponentDecl::default();
        component_manager_children = MutableChildMap::default();
        let mut state = tmp_node.state.lock().await;
        mem::swap(&mut component_manager_decl, &mut state.decl);
        mem::swap(&mut component_manager_children, &mut state.mutable_children);
    }
    match **component_manager_decl
        .program
        .as_mut()
        .expect("component manager's manifest is lacking a program section")
        .info
        .entries
        .get_or_insert(vec![])
        .iter_mut()
        .find(|e| e.key == "args")
        .expect("component manager's manifest doesn't specify a config")
        .value
        .as_mut()
        .expect("component manager's manifest has a malformed 'args' section")
    {
        fdata::DictionaryValue::StrVec(ref mut v) => v.push(root_url.into()),
        _ => panic!(
            "component manager's manifest has a single value for 'args', but we were expecting a vector"
        ),
    }
    append_box(&mut component_manager_decl.capabilities, &mut passthrough_cap_decls.clone());
    append_box(&mut component_manager_decl.exposes, &mut passthrough_expose_decls.clone());
    append_box(&mut component_manager_decl.uses, &mut passthrough_use_decls.clone());
    Ok((component_manager_decl, component_manager_children))
}

async fn add_use_decl_if_needed(
    realm: &mut RealmNodeState,
    ref_: fcdecl::Ref,
    capability: ftest::Capability,
) -> Result<(), RealmBuilderError> {
    match capability {
        // Resolvers don't support Use.
        ftest::Capability::Resolver(_) => return Ok(()),
        // We don't have enough information to generate a Use for a config capability.
        ftest::Capability::Config(_) => return Ok(()),
        _ => {}
    }
    if let fcdecl::Ref::Child(child) = ref_ {
        if let Some(child) = realm.get_updateable_children().get(child.name.as_str()) {
            let mut decl = child.get_decl().await;

            // If the original route contained a source dictionary path, the effect
            // is to extract the capability from that dictionary. So we should not copy the
            // dictionary path to the child's autogenerated `use` declaration.
            let from_dictionary = None;
            if let Some(use_decl) = create_use_decl(
                capability,
                fcdecl::Ref::Parent(fcdecl::ParentRef {}),
                from_dictionary,
            )? {
                push_if_not_present_box(&mut decl.uses, use_decl);
            }
            let () = child.replace_decl(decl).await?;
        }
    }

    Ok(())
}

async fn add_expose_decl_if_needed(
    realm: &mut RealmNodeState,
    from: fcdecl::Ref,
    from_dictionary: Option<String>,
    capability: ftest::Capability,
) -> Result<(), RealmBuilderError> {
    if let fcdecl::Ref::Child(child) = from {
        if let Some(child) = realm.get_updateable_children().get(child.name.as_str()) {
            let mut decl = child.get_decl().await;
            // If the `Capability` contains a source dictionary path, just skip it. We don't
            // support autogenerating dictionaries.
            let is_from_dictionary =
                from_dictionary.is_some() && from_dictionary.as_ref().unwrap() != ".";
            if is_from_dictionary {
                return Ok(());
            }
            // If the `Capability` is already something we expose we don't want to add
            // a capability and expose entry.
            {
                let source_name = match &capability {
                    ftest::Capability::Protocol(c) => c.name.clone(),
                    ftest::Capability::Directory(c) => c.name.clone(),
                    ftest::Capability::Storage(c) => c.name.clone(),
                    ftest::Capability::Service(c) => c.name.clone(),
                    ftest::Capability::EventStream(c) => c.name.clone(),
                    ftest::Capability::Config(c) => c.name.clone(),
                    ftest::Capability::Dictionary(c) => c.name.clone(),
                    ftest::Capability::Resolver(c) => c.name.clone(),
                    ftest::Capability::Runner(c) => c.name.clone(),
                    _ => {
                        return Err(RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                            "Unrecognized capability variant: {:?}.",
                            capability
                        )));
                    }
                };

                if decl
                    .exposes
                    .iter()
                    .any(|x| Some(x.source_name().as_str()) == source_name.as_deref())
                {
                    return Ok(());
                }
            }
            push_if_not_present_box(
                &mut decl.capabilities,
                create_capability_decl(capability.clone())?,
            );
            push_if_not_present_box(
                &mut decl.exposes,
                create_expose_decl(
                    capability,
                    fcdecl::Ref::Self_(fcdecl::SelfRef {}),
                    from_dictionary,
                    ExposingIn::Child,
                )?,
            );
            let () = child.replace_decl(decl).await?;
        }
    }

    Ok(())
}

fn parse_relative_path(path: Option<String>) -> Result<RelativePath, RealmBuilderError> {
    path.map(|p| {
        RelativePath::new(&p).map_err(|_| RealmBuilderError::InvalidRelativePath(p.clone()))
    })
    .transpose()
    .map(|p| p.unwrap_or_default())
}

fn into_dependency_type(type_: &Option<fcdecl::DependencyType>) -> cm_rust::DependencyType {
    type_
        .as_ref()
        .cloned()
        .map(FidlIntoNative::fidl_into_native)
        .unwrap_or(cm_rust::DependencyType::Strong)
}

/// Attempts to produce the target name from the set of "name" and "as" fields from a capability.
fn try_into_source_name(name: &Option<String>) -> Result<cm_types::Name, RealmBuilderError> {
    Ok(name
        .as_ref()
        .ok_or_else(|| {
            RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                "Required field `name` is empty."
            ))
        })?
        .parse()
        .map_err(|_| {
            RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                "Field `name` is not a valid name."
            ))
        })?)
}

/// Attempts to produce the target name from the set of "name" and "as" fields from a capability.
fn try_into_target_name(
    name: &Option<String>,
    as_: &Option<String>,
) -> Result<cm_types::Name, RealmBuilderError> {
    let name = name.as_ref().ok_or_else(|| {
        RealmBuilderError::CapabilityInvalid(anyhow::format_err!("Required field `name` is empty."))
    })?;
    Ok(as_.as_ref().unwrap_or(name).clone().parse().map_err(|_| {
        RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
            "Field `name` is not a valid name."
        ))
    })?)
}

fn try_into_subdir(input: &Option<String>) -> Result<RelativePath, RealmBuilderError> {
    input
        .as_ref()
        .map(|p| {
            RelativePath::new(p).map_err(|_| {
                RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                    "Field `subdir` is not a valid relative path."
                ))
            })
        })
        .unwrap_or_else(|| Ok(Default::default()))
}

/// Attempts to produce a valid path from the "path" field from a capability
fn try_into_capability_path(input: &Option<String>) -> Result<cm_types::Path, RealmBuilderError> {
    input
        .as_ref()
        .ok_or_else(|| {
            RealmBuilderError::CapabilityInvalid(anyhow::format_err!("The `path` field is not set. This field is required when routing to or from a local component. For more information on the `path` field, see https://fuchsia.dev/go/components/realm-builder-reference#Directory."))
        })?
        .as_str()
        .parse()
        .map_err(|e| {
            RealmBuilderError::CapabilityInvalid(anyhow::format_err!("The `path` field is invalid: {:?}. All paths must be `/` delimited strings with a leading slash.", e))
        })
}

/// Attempts to produce a valid CapabilityPath from the "path" field from a capability, and if that
/// fails then attempts to produce a valid CapabilityPath from the "name" field following
/// "/svc/{name}"
fn try_into_service_path(
    name: &Option<String>,
    path: &Option<String>,
) -> Result<cm_types::Path, RealmBuilderError> {
    let name = name.as_ref().ok_or_else(|| {
        RealmBuilderError::CapabilityInvalid(anyhow::format_err!("Required field `name` is empty. This field must be provided. For more information on the `name` field, see https://fuchsia.dev/go/components/realm-builder-reference#Protocol."))
    })?;
    let path = path.as_ref().cloned().unwrap_or_else(|| format!("/svc/{}", name));
    path.as_str().parse().map_err(|e| {
        RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
            "Could not create path for protocol {}. Encountered unexpected error. {:?}",
            name,
            e
        ))
    })
}

fn create_capability_decl(
    capability: ftest::Capability,
) -> Result<cm_rust::CapabilityDecl, RealmBuilderError> {
    Ok(match capability {
        ftest::Capability::Protocol(protocol) => {
            let name = try_into_source_name(&protocol.name)?;
            let source_path = Some(try_into_service_path(&protocol.name, &protocol.path)?);
            cm_rust::CapabilityDecl::Protocol(cm_rust::ProtocolDecl {
                name,
                source_path,
                #[cfg(fuchsia_api_level_at_least = "HEAD")]
                delivery: Default::default(),
            })
        }
        ftest::Capability::Directory(directory) => {
            let name = try_into_source_name(&directory.name)?;
            let source_path = Some(try_into_capability_path(&directory.path)?);
            let rights = directory.rights.ok_or_else(|| RealmBuilderError::CapabilityInvalid(
                anyhow::format_err!(
                    "The `rights` field is not set. This field is required when routing directory capabilities to or from a local component. Required fields are defined at https://fuchsia.dev/go/components/realm-builder-reference#Directory.",
                ),
            ))?;
            cm_rust::CapabilityDecl::Directory(cm_rust::DirectoryDecl { name, source_path, rights })
        }
        ftest::Capability::Storage(_) => {
            return Err(RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                "Storage capabilities with a source of `self` can not be routed. Please use `GetComponentDecl` and `ReplaceComponentDecl` to declare and route this capability."
            )))?;
        }
        ftest::Capability::Service(service) => {
            let name = try_into_source_name(&service.name)?;
            let source_path = Some(try_into_service_path(&service.name, &service.path)?);
            cm_rust::CapabilityDecl::Service(cm_rust::ServiceDecl { name, source_path })
        }
        ftest::Capability::EventStream(event) => {
            let name = try_into_source_name(&event.name)?;
            cm_rust::CapabilityDecl::EventStream(cm_rust::EventStreamDecl { name })
        }
        ftest::Capability::Dictionary(dictionary) => {
            let name = try_into_source_name(&dictionary.name)?;
            cm_rust::CapabilityDecl::Dictionary(cm_rust::DictionaryDecl { name, source_path: None })
        }
        ftest::Capability::Resolver(resolver) => {
            let name = try_into_source_name(&resolver.name)?;
            let source_path = Some(try_into_service_path(&resolver.name, &resolver.path)?);
            cm_rust::CapabilityDecl::Resolver(cm_rust::ResolverDecl { name, source_path })
        }
        ftest::Capability::Runner(runner) => {
            let name = try_into_source_name(&runner.name)?;
            let source_path = Some(try_into_service_path(&runner.name, &runner.path)?);
            cm_rust::CapabilityDecl::Runner(cm_rust::RunnerDecl { name, source_path })
        }
        _ => {
            return Err(RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                "Encountered unsupported capability variant: {:?}.",
                capability
            )));
        }
    })
}

fn get_offer_availability(availability: &Option<fcdecl::Availability>) -> cm_rust::Availability {
    match availability {
        Some(fcdecl::Availability::Optional) => cm_rust::Availability::Optional,
        Some(fcdecl::Availability::SameAsTarget) => cm_rust::Availability::SameAsTarget,
        Some(fcdecl::Availability::Required) => cm_rust::Availability::Required,
        Some(fcdecl::Availability::Transitional) => cm_rust::Availability::Transitional,
        None => {
            // Required is the default.
            cm_rust::Availability::Required
        }
    }
}

fn create_offer_decl(
    capability: ftest::Capability,
    source: fcdecl::Ref,
    #[allow(unused)] source_dictionary: Option<String>,
    target: fcdecl::Ref,
) -> Result<cm_rust::offer::OfferDecl, RealmBuilderError> {
    let source: cm_rust::offer::OfferSource = source.fidl_into_native();
    #[cfg(fuchsia_api_level_at_least = "25")]
    let source_dictionary = parse_relative_path(source_dictionary)?;
    let target: cm_rust::offer::OfferTarget = target.fidl_into_native();

    Ok(match capability {
        ftest::Capability::Protocol(protocol) => {
            let source_name = try_into_source_name(&protocol.name)?;
            let target_name = try_into_target_name(&protocol.name, &protocol.as_)?;
            let dependency_type = into_dependency_type(&protocol.type_);
            let availability = get_offer_availability(&protocol.availability);
            cm_rust::offer::OfferDecl::Protocol(cm_rust::offer::OfferProtocolDecl {
                source,
                source_name,
                source_dictionary,
                target,
                target_name,
                dependency_type,
                availability,
            })
        }
        ftest::Capability::Directory(directory) => {
            let source_name = try_into_source_name(&directory.name)?;
            let target_name = try_into_target_name(&directory.name, &directory.as_)?;
            let dependency_type = into_dependency_type(&directory.type_);
            let availability = get_offer_availability(&directory.availability);
            cm_rust::offer::OfferDecl::Directory(Box::new(cm_rust::offer::OfferDirectoryDecl {
                source,
                source_name,
                source_dictionary,
                target,
                target_name,
                rights: directory.rights,
                subdir: try_into_subdir(&directory.subdir)?,
                dependency_type,
                availability,
            }))
        }
        ftest::Capability::Storage(storage) => {
            let source_name = try_into_source_name(&storage.name)?;
            let target_name = try_into_target_name(&storage.name, &storage.as_)?;
            let availability = get_offer_availability(&storage.availability);
            cm_rust::offer::OfferDecl::Storage(cm_rust::offer::OfferStorageDecl {
                source,
                source_name,
                target,
                target_name,
                availability,
            })
        }
        ftest::Capability::Service(service) => {
            let source_name = try_into_source_name(&service.name)?;
            let target_name = try_into_target_name(&service.name, &service.as_)?;
            let availability = get_offer_availability(&service.availability);
            cm_rust::offer::OfferDecl::Service(Box::new(cm_rust::offer::OfferServiceDecl {
                source,
                source_name,
                source_dictionary,
                target,
                target_name,
                source_instance_filter: None,
                renamed_instances: None,
                availability,
                #[cfg(fuchsia_api_level_at_least = "HEAD")]
                dependency_type: Default::default(),
            }))
        }
        ftest::Capability::EventStream(event_stream) => {
            let source_name = try_into_source_name(&event_stream.name)?;
            let target_name = try_into_target_name(&event_stream.name, &event_stream.as_)?;
            cm_rust::offer::OfferDecl::EventStream(Box::new(cm_rust::offer::OfferEventStreamDecl {
                source,
                source_name,
                target,
                target_name,
                scope: event_stream.scope.as_ref().cloned().map(FidlIntoNative::fidl_into_native),
                availability: cm_rust::Availability::Required,
            }))
        }
        ftest::Capability::Config(config) => {
            let availability = match source {
                cm_rust::offer::OfferSource::Void => cm_rust::Availability::Optional,
                _ => get_offer_availability(&config.availability),
            };
            cm_rust::offer::OfferDecl::Config(cm_rust::offer::OfferConfigurationDecl {
                source,
                source_name: try_into_source_name(&config.name)?,
                target,
                target_name: try_into_target_name(&config.name, &config.as_)?,
                availability,
                source_dictionary: ".".parse().unwrap(),
            })
        }
        ftest::Capability::Dictionary(dictionary) => {
            let dependency_type = into_dependency_type(&dictionary.type_);
            let availability = get_offer_availability(&dictionary.availability);
            cm_rust::offer::OfferDecl::Dictionary(cm_rust::offer::OfferDictionaryDecl {
                source,
                source_name: try_into_source_name(&dictionary.name)?,
                source_dictionary,
                target,
                target_name: try_into_target_name(&dictionary.name, &dictionary.as_)?,
                dependency_type,
                availability,
            })
        }
        ftest::Capability::Resolver(resolver) => {
            let source_name = try_into_source_name(&resolver.name)?;
            let target_name = try_into_target_name(&resolver.name, &resolver.as_)?;
            cm_rust::offer::OfferDecl::Resolver(cm_rust::offer::OfferResolverDecl {
                source,
                source_name,
                source_dictionary,
                target,
                target_name,
            })
        }
        ftest::Capability::Runner(runner) => {
            let source_name = try_into_source_name(&runner.name)?;
            let target_name = try_into_target_name(&runner.name, &runner.as_)?;
            cm_rust::offer::OfferDecl::Runner(cm_rust::offer::OfferRunnerDecl {
                source,
                source_name,
                source_dictionary,
                target,
                target_name,
            })
        }
        _ => {
            return Err(RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                "Encountered unsupported capability variant: {:?}.",
                capability
            )));
        }
    })
}

// We only want to apply the rename for a capability once. If we're handling a route from a local
// component child to the parent, we want to use the source name in the child for the source and
// target names, and apply the rename (where the source_name and target_name fields don't match) in
// the parent. This field is used to track when an expose declaration is being generated for a
// child versus the parent realm.
enum ExposingIn {
    Realm,
    Child,
}

fn create_expose_decl(
    capability: ftest::Capability,
    source: fcdecl::Ref,
    #[allow(unused)] source_dictionary: Option<String>,
    exposing_in: ExposingIn,
) -> Result<cm_rust::ExposeDecl, RealmBuilderError> {
    let source: cm_rust::ExposeSource = source.fidl_into_native();
    #[cfg(fuchsia_api_level_at_least = "25")]
    let source_dictionary = parse_relative_path(source_dictionary)?;

    Ok(match capability {
        ftest::Capability::Protocol(protocol) => {
            let source_name = try_into_source_name(&protocol.name)?;
            let target_name = match exposing_in {
                ExposingIn::Child => try_into_source_name(&protocol.name)?,
                ExposingIn::Realm => try_into_target_name(&protocol.name, &protocol.as_)?,
            };
            cm_rust::ExposeDecl::Protocol(cm_rust::ExposeProtocolDecl {
                source: source,
                source_name,
                source_dictionary,
                target: cm_rust::ExposeTarget::Parent,
                target_name,
                // TODO(https://fxbug.dev/42058594): Support optional exposes.
                availability: cm_rust::Availability::Required,
            })
        }
        ftest::Capability::Directory(directory) => {
            let source_name = try_into_source_name(&directory.name)?;
            let target_name = match exposing_in {
                ExposingIn::Child => try_into_source_name(&directory.name)?,
                ExposingIn::Realm => try_into_target_name(&directory.name, &directory.as_)?,
            };
            // Much like capability renames, we want to only apply the subdir field once. Use the
            // exposing_in field to ensure that we apply the subdir field in the parent, and not in
            // a local child's manifest.
            let subdir = match exposing_in {
                ExposingIn::Child => Default::default(),
                ExposingIn::Realm => try_into_subdir(&directory.subdir)?,
            };
            cm_rust::ExposeDecl::Directory(cm_rust::ExposeDirectoryDecl {
                source,
                source_name,
                source_dictionary,
                target: cm_rust::ExposeTarget::Parent,
                target_name,
                rights: directory.rights,
                subdir,
                // TODO(https://fxbug.dev/42058594): Support optional exposes.
                availability: cm_rust::Availability::Required,
            })
        }
        ftest::Capability::Storage(storage) => {
            let source_name = try_into_source_name(&storage.name)?;
            return Err(RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                "Capability \"{}\" can not be exposed because it's not possible to expose storage capabilities. This is most likely a bug from the Realm Builder library. Please file one at https://bugs.fuchsia.dev under the ComponentFramework>SDK component.",
                source_name
            )));
        }
        ftest::Capability::Service(service) => {
            let source_name = try_into_source_name(&service.name)?;
            let target_name = match exposing_in {
                ExposingIn::Child => try_into_source_name(&service.name)?,
                ExposingIn::Realm => try_into_target_name(&service.name, &service.as_)?,
            };
            cm_rust::ExposeDecl::Service(cm_rust::ExposeServiceDecl {
                source,
                source_name,
                source_dictionary,
                target: cm_rust::ExposeTarget::Parent,
                target_name,
                // TODO(https://fxbug.dev/42058594): Support optional exposes.
                availability: cm_rust::Availability::Required,
            })
        }
        ftest::Capability::Dictionary(dictionary) => {
            let source_name = try_into_source_name(&dictionary.name)?;
            let target_name = match exposing_in {
                ExposingIn::Child => try_into_source_name(&dictionary.name)?,
                ExposingIn::Realm => try_into_target_name(&dictionary.name, &dictionary.as_)?,
            };
            cm_rust::ExposeDecl::Dictionary(cm_rust::ExposeDictionaryDecl {
                source,
                source_name,
                source_dictionary,
                target: cm_rust::ExposeTarget::Parent,
                target_name,
                availability: cm_rust::Availability::Required,
            })
        }
        ftest::Capability::Resolver(resolver) => {
            let source_name = try_into_source_name(&resolver.name)?;
            let target_name = match exposing_in {
                ExposingIn::Child => try_into_source_name(&resolver.name)?,
                ExposingIn::Realm => try_into_target_name(&resolver.name, &resolver.as_)?,
            };
            cm_rust::ExposeDecl::Resolver(cm_rust::ExposeResolverDecl {
                source: source,
                source_name,
                source_dictionary,
                target: cm_rust::ExposeTarget::Parent,
                target_name,
            })
        }
        ftest::Capability::Runner(runner) => {
            let source_name = try_into_source_name(&runner.name)?;
            let target_name = match exposing_in {
                ExposingIn::Child => try_into_source_name(&runner.name)?,
                ExposingIn::Realm => try_into_target_name(&runner.name, &runner.as_)?,
            };
            cm_rust::ExposeDecl::Runner(cm_rust::ExposeRunnerDecl {
                source: source,
                source_name,
                source_dictionary,
                target: cm_rust::ExposeTarget::Parent,
                target_name,
            })
        }
        _ => {
            return Err(RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                "Encountered unsupported capability variant: {:?}.",
                capability
            )));
        }
    })
}

fn check_and_unwrap_use_availability(
    availability: Option<fcdecl::Availability>,
) -> Result<cm_rust::Availability, RealmBuilderError> {
    match availability {
        None => Ok(cm_rust::Availability::Required),
        Some(fcdecl::Availability::SameAsTarget) => {
            Err(RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                "availability can not be \"same_as_target\" if the target is a local component"
            )))
        }
        Some(availability) => Ok(availability.fidl_into_native()),
    }
}

fn create_use_decl(
    capability: ftest::Capability,
    source: fcdecl::Ref,
    #[allow(unused)] source_dictionary: Option<String>,
) -> Result<Option<cm_rust::UseDecl>, RealmBuilderError> {
    let source: cm_rust::UseSource = source.fidl_into_native();
    #[cfg(fuchsia_api_level_at_least = "25")]
    let source_dictionary = parse_relative_path(source_dictionary)?;
    Ok(match capability {
        ftest::Capability::Protocol(protocol) => {
            // If the capability was renamed in the parent's offer declaration, we want to use the
            // post-rename version of it here.
            let source_name = try_into_target_name(&protocol.name, &protocol.as_)?;
            let target_path = try_into_service_path(
                &Some(source_name.clone().native_into_fidl()),
                &protocol.path,
            )?;
            let dependency_type = protocol
                .type_
                .map(FidlIntoNative::fidl_into_native)
                .unwrap_or(cm_rust::DependencyType::Strong);
            Some(cm_rust::UseDecl::Protocol(cm_rust::UseProtocolDecl {
                source,
                source_name,
                source_dictionary,
                target_path: Some(target_path),
                #[cfg(fuchsia_api_level_at_least = "29")]
                numbered_handle: None,
                dependency_type,
                availability: check_and_unwrap_use_availability(protocol.availability)?,
            }))
        }
        ftest::Capability::Directory(directory) => {
            // If the capability was renamed in the parent's offer declaration, we want to use the
            // post-rename version of it here.
            let source_name = try_into_target_name(&directory.name, &directory.as_)?;
            let target_path = try_into_capability_path(&directory.path)?;
            let rights = directory.rights.ok_or_else(|| RealmBuilderError::CapabilityInvalid(
                anyhow::format_err!(
                    "The `rights` field is not set. This field is required when routing directory capabilities to or from a local component.",
                ),
            ))?;
            let dependency_type = directory
                .type_
                .map(FidlIntoNative::fidl_into_native)
                .unwrap_or(cm_rust::DependencyType::Strong);
            Some(cm_rust::UseDecl::Directory(cm_rust::UseDirectoryDecl {
                source,
                source_name,
                source_dictionary,
                target_path,
                rights,
                // We only want to set the sub-directory field once, and if we're generating a use
                // declaration then we've already generated an offer declaration in the parent and
                // we'll set the sub-directory field there.
                subdir: Default::default(),
                dependency_type,
                availability: check_and_unwrap_use_availability(directory.availability)?,
            }))
        }
        ftest::Capability::Storage(storage) => {
            if source != cm_rust::UseSource::Parent {
                unreachable!("storage use source must be parent");
            }
            // If the capability was renamed in the parent's offer declaration, we want to use the
            // post-rename version of it here.
            let source_name = try_into_target_name(&storage.name, &storage.as_)?;
            let target_path = try_into_capability_path(&storage.path)?;
            Some(cm_rust::UseDecl::Storage(cm_rust::UseStorageDecl {
                source_name,
                target_path,
                availability: check_and_unwrap_use_availability(storage.availability)?,
            }))
        }
        ftest::Capability::Service(service) => {
            // If the capability was renamed in the parent's offer declaration, we want to use the
            // post-rename version of it here.
            let source_name = try_into_target_name(&service.name, &service.as_)?;
            let target_path = try_into_service_path(
                &Some(source_name.clone().native_into_fidl()),
                &service.path,
            )?;
            Some(cm_rust::UseDecl::Service(cm_rust::UseServiceDecl {
                source,
                source_name,
                source_dictionary,
                target_path,
                dependency_type: cm_rust::DependencyType::Strong,
                availability: check_and_unwrap_use_availability(service.availability)?,
            }))
        }
        ftest::Capability::EventStream(event) => {
            // If the capability was renamed in the parent's offer declaration, we want to use the
            // post-rename version of it here.
            let source_name = try_into_target_name(&event.name, &event.as_)?;
            let filter = event.filter.as_ref().cloned().map(FidlIntoNative::fidl_into_native);
            let target_path = try_into_capability_path(&event.path)?;
            Some(cm_rust::UseDecl::EventStream(Box::new(cm_rust::UseEventStreamDecl {
                source,
                source_name,
                target_path,
                filter,
                scope: event.scope.as_ref().cloned().map(FidlIntoNative::fidl_into_native),
                availability: cm_rust::Availability::Required,
            })))
        }
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        ftest::Capability::Runner(runner) => {
            // If the capability was renamed in the parent's offer declaration, we want to use the
            // post-rename version of it here.
            let source_name = try_into_target_name(&runner.name, &runner.as_)?;
            Some(cm_rust::UseDecl::Runner(cm_rust::UseRunnerDecl {
                source,
                source_name,
                source_dictionary,
            }))
        }
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        ftest::Capability::Dictionary(dictionary) => {
            // If the capability was renamed in the parent's offer declaration, we want to use the
            // post-rename version of it here.
            let source_name = try_into_target_name(&dictionary.name, &dictionary.as_)?;
            let Ok(target_path) = try_into_capability_path(&dictionary.path) else {
                // Test manager calls add_route to make the diagnostics dictionary available to a
                // local child named "resolver" (in `src/sys/test_manager/src/running_suite.rs`),
                // and does not provide a path. This highlights a conundrum: starting to require
                // `path` be set on dictionaries routed to local children is a breaking change. To
                // allow us to turn on use declaration generation for dictionaries routed to local
                // children in a more gentle way, we won't error out when the path is missing here
                // unlike with the other capability types.
                return Ok(None);
            };
            Some(cm_rust::UseDecl::Dictionary(cm_rust::UseDictionaryDecl {
                source,
                source_name,
                source_dictionary,
                target_path,
                dependency_type: cm_rust::DependencyType::Strong,
                availability: cm_rust::Availability::Required,
            }))
        }
        _ => {
            return Err(RealmBuilderError::CapabilityInvalid(anyhow::format_err!(
                "Encountered unsupported capability variant: {:?}.",
                capability
            )));
        }
    })
}

fn contains_child(realm: &RealmNodeState, ref_: &fcdecl::Ref) -> bool {
    match ref_ {
        fcdecl::Ref::Child(child) => realm
            .decl
            .children
            .iter()
            .map(|c| &c.name)
            .chain(realm.mutable_children.keys())
            .any(|name| child.name.as_str() == name.as_str()),
        _ => true,
    }
}

fn push_if_not_present_box<T: PartialEq>(container: &mut Box<[T]>, value: T) {
    if !container.contains(&value) {
        push_box(container, value)
    }
}

// If this realm node is going have its decl replaced, we need to ensure
// that the program section isn't corrupted.
fn validate_program_modifications(
    old_decl: &cm_rust::ComponentDecl,
    new_decl: &cm_rust::ComponentDecl,
) -> Result<(), RealmBuilderError> {
    if old_decl.program.as_ref().and_then(|p| p.runner.as_ref())
        == Some(&runner::RUNNER_NAME.parse().unwrap())
    {
        let new_decl_program = match new_decl.program.as_ref() {
            Some(program) => program.clone(),
            None => {
                return Err(RealmBuilderError::ImmutableProgram);
            }
        };

        // We know that `old_decl.program` is `Some(_)` because we're inside
        // this `if` clause. Therefore, it's safe to check equality against
        // `Some(new_decl_program)`.
        if old_decl.program != Some(new_decl_program) {
            return Err(RealmBuilderError::ImmutableProgram);
        }
    }

    Ok(())
}

// Since Rust doesn't allow one to implement out-of-crate traits (fmt::Display
// for out-of-crate types (fcdecl::Ref), a convenience function is provided.
// See Rust E0117 for more info.
fn ref_to_string(ref_: &fcdecl::Ref) -> String {
    match ref_ {
        fcdecl::Ref::Child(c) => c.name.to_owned(),
        fcdecl::Ref::Parent(_) => "<parent>".to_owned(),
        fcdecl::Ref::Self_(_) => "<self>".to_owned(),
        fcdecl::Ref::Collection(c) => c.name.to_owned(),
        fcdecl::Ref::Framework(_) => "<framework>".to_owned(),
        fcdecl::Ref::Capability(c) => c.name.to_owned(),
        fcdecl::Ref::Debug(_) => "<debug>".to_owned(),
        fcdecl::Ref::VoidType(_) => "<void>".to_owned(),
        _ => unreachable!("Encountered unknown `Ref` variant."),
    }
}

#[allow(unused)]
#[derive(Debug, Error)]
enum RealmBuilderError {
    /// Child cannot be added to the realm, as there is already a child in the realm with that
    /// name.
    #[error(
        "Unable to add child because one already exists with the name \"{0}\". Child names within a realm must be unique."
    )]
    ChildAlreadyExists(String),

    /// A component declaration failed validation.
    #[error(
        "The constructed component declaration is invalid. Please fix all the listed errors:\n{0}\nFor a reference as to how component declarations are authored, see https://fuchsia.dev/go/components/declaration."
    )]
    InvalidComponentDecl(String),

    /// A component declaration failed validation.
    #[error(
        "The component declaration for child \"{0}\" is invalid. Please fix all the listed errors:\n{1}\nFor a reference as to how component declarations are authored, see https://fuchsia.dev/go/components/declaration."
    )]
    InvalidComponentDeclWithName(String, String),

    #[error("The provided path {0} is not a valid relative path.")]
    InvalidRelativePath(String),

    /// The referenced child does not exist.
    #[error(
        "No child exists with the name \"{0}\". Before fetching or changing its component declaration, a child must be added to the realm with the `AddChild` group of methods."
    )]
    NoSuchChild(String),

    /// The component declaration for the referenced child cannot be viewed nor manipulated by
    /// RealmBuilder because the child was added to the realm using an URL that was not a
    /// relative URL.
    #[error(
        "The component declaration for child {0} cannot be replaced. If you'd like to mutate a component's decl, add it your test package and reference it via a fragment-only URL: https://fuchsia.dev/go/components/url#relative-fragment-only."
    )]
    ChildDeclNotVisible(String),

    /// The source does not exist.
    #[error(
        "Source component for capability is invalid. No child exists with the name \"{0}\". Before a component can be set as a source for a capability, it must be added to the realm with the `AddChild` group of methods."
    )]
    NoSuchSource(String),

    /// A target does not exist.
    #[error(
        "Target component for capability is invalid. No child exists with the name '{0}'. Before a component can be set as a source for a capability, it must be added to the realm with the `AddChild` group of methods."
    )]
    NoSuchTarget(String),

    /// A target does not exist.
    #[error(
        "Target capability for capability is invalid. No dictionary capability exists with the name '{0}'."
    )]
    NoSuchTargetCapability(String),

    /// The `capabilities` field is empty.
    #[error(
        "The `capabilities` field can not be omitted. It is used to specify what capabilities will be routed. Provide at least one capability to route: https://fuchsia.dev/go/components/realm-builder-reference#Realm.AddRoute."
    )]
    CapabilitiesEmpty,

    /// The `targets` field is empty.
    #[error(
        "The `targets` field can not be omitted. It is used to determine what component(s) to route a capability to. Provide at least one component as a target: https://fuchsia.dev/go/components/realm-builder-reference#Realm.AddRoute."
    )]
    TargetsEmpty,

    /// The `from` value is equal to one of the elements in `to`.
    #[error(
        "One of the targets of this route is equal to the source {0:?}. Routing a capability to itself is not supported."
    )]
    SourceAndTargetMatch(String),

    /// The test package does not contain the component declaration referenced by a fragment-only URL.
    #[error(
        "Component \"{0}\" not found in package. Only components added to the test's package can be referenced by fragment-only URLs. Ensure that this component is included in the test's package."
    )]
    DeclNotFound(String),

    /// Encountered an I/O error when attempting to read a component declaration referenced by a
    /// fragment-only URL from the test package.
    #[error("Could not read the manifest for component \"{0}\". {1:?}")]
    DeclReadError(String, fuchsia_fs::file::ReadError),

    /// The `Build` function has been called multiple times on this channel.
    #[error(
        "Build method was called multiple times. This method can only be called once. After it's called, the realm to be constructed can not be changed."
    )]
    BuildAlreadyCalled,

    #[error("Failed to route capability. {0:?}")]
    CapabilityInvalid(anyhow::Error),

    #[error("Invalid child declaration. Field `name` is not a valid name.")]
    ChildNameInvalid,

    #[error("Invalid child declaration. Field `environment` is not a valid name.")]
    EnvironmentNameInvalid,

    #[error("Invalid child declaration. Field `url` is not a valid url.")]
    UrlInvalid,

    /// The handle the client provided is not usable
    #[error("Handle for child realm \"{0}\" is not usable. {1:?}")]
    InvalidChildRealmHandle(String, fidl::Error),

    /// `ReplaceComponentDecl` was called on a local component with a program declaration
    /// that did not match the one from the old component declaration. This could render a
    /// local component non-functional, and is disallowed.
    #[error(
        "Attempted to change `program` section of immutable child. The `program` section of a local component cannot be changed."
    )]
    ImmutableProgram,

    /// The component does not have a config schema defined. Attempting to
    /// set a config value is not allowed.
    #[error(
        "Could not replace config value for child \"{0}\". The component does not have a config schema in its declaration. Only components with a config schema can have their config values modified. For more information about the structured configuration feature, see https://fuchsia.dev/go/components/structured-config."
    )]
    NoConfigSchema(String),

    /// The component's config schema does not have a field with that name.
    #[error(
        "Could not replace config value for child \"{name}\". No field with the name `{key}` is present in the config schema. The fields present in the schema are: {present:?}."
    )]
    NoSuchConfigField { name: String, key: String, present: Vec<String> },

    /// A config value is invalid. This may mean a type mismatch or an issue
    /// with constraints like string/vector length.
    #[error("Could not replace config value for child '{0}'. The value provided is invalid: {1:?}")]
    ConfigValueInvalid(String, anyhow::Error),

    /// The caller never told us how to merge their config overrides with the packaged ones.
    #[error(
        "Could not replace config value for child '{name}' because no override strategy has been selected. First call InitMutableConfigFromPackage or InitMutableConfigToEmpty."
    )]
    ConfigOverrideUnsupported { name: String },
}

impl From<RealmBuilderError> for ftest::RealmBuilderError {
    fn from(err: RealmBuilderError) -> Self {
        match err {
            RealmBuilderError::ChildAlreadyExists(_) => Self::ChildAlreadyExists,
            RealmBuilderError::InvalidComponentDecl(_) => Self::InvalidComponentDecl,
            RealmBuilderError::InvalidComponentDeclWithName(_, _) => Self::InvalidComponentDecl,
            RealmBuilderError::InvalidRelativePath(_) => Self::InvalidComponentDecl,
            RealmBuilderError::NoSuchChild(_) => Self::NoSuchChild,
            RealmBuilderError::ChildNameInvalid => Self::InvalidComponentDecl,
            RealmBuilderError::EnvironmentNameInvalid => Self::InvalidComponentDecl,
            RealmBuilderError::UrlInvalid => Self::InvalidComponentDecl,
            RealmBuilderError::ChildDeclNotVisible(_) => Self::ChildDeclNotVisible,
            RealmBuilderError::NoSuchSource(_) => Self::NoSuchSource,
            RealmBuilderError::NoSuchTarget(_) => Self::NoSuchTarget,
            RealmBuilderError::NoSuchTargetCapability(_) => Self::NoSuchTarget,
            RealmBuilderError::CapabilitiesEmpty => Self::CapabilitiesEmpty,
            RealmBuilderError::TargetsEmpty => Self::TargetsEmpty,
            RealmBuilderError::SourceAndTargetMatch(_) => Self::SourceAndTargetMatch,
            RealmBuilderError::DeclNotFound(_) => Self::DeclNotFound,
            RealmBuilderError::DeclReadError(_, _) => Self::DeclReadError,
            RealmBuilderError::BuildAlreadyCalled => Self::BuildAlreadyCalled,
            RealmBuilderError::CapabilityInvalid(_) => Self::CapabilityInvalid,
            RealmBuilderError::InvalidChildRealmHandle(_, _) => Self::InvalidChildRealmHandle,
            RealmBuilderError::ImmutableProgram => Self::ImmutableProgram,
            RealmBuilderError::NoConfigSchema(_) => Self::NoConfigSchema,
            RealmBuilderError::NoSuchConfigField { .. } => Self::NoSuchConfigField,
            RealmBuilderError::ConfigValueInvalid(_, _) => Self::ConfigValueInvalid,
            RealmBuilderError::ConfigOverrideUnsupported { .. } => Self::ConfigOverrideUnsupported,
        }
    }
}

fn is_fragment_only_url(url: &str) -> bool {
    if url.len() == 0 || url.chars().nth(0) != Some('#') {
        return false;
    }
    if Url::parse(url) != Err(url::ParseError::RelativeUrlWithoutBase) {
        return false;
    }
    true
}

// Formats an ErrorList into a tabulated string. This format is used to create
// more readable user error messages.
fn to_tabulated_string(errors: cm_fidl_validator::error::ErrorList) -> String {
    let mut output = String::new();
    for (i, err) in errors.errs.iter().enumerate() {
        let is_last_element = errors.errs.len() - i == 1;
        output.push_str(&format!("  {}. {}", i + 1, err));
        if !is_last_element {
            output.push('\n');
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use cm_rust_testing::*;
    use difference::Changeset;
    use fidl::endpoints::{
        ClientEnd, create_endpoints, create_proxy, create_proxy_and_stream, create_request_stream,
    };
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_mem as fmem;
    use fuchsia_async as fasync;
    use std::time::Duration;
    use test_case::test_case;

    /// Assert that two ComponentTrees are equivalent.
    ///
    /// Output: the prettified debug output compared line-by-line,
    /// with a colorized diff. Green is the expected version, red is the actual.
    macro_rules! assert_decls_eq {
        ($actual:expr, $($expected:tt)+) => {{

            let actual = $actual.clone();
            let expected = {$($expected)+}.clone();
            if actual != expected {
                let actual = format!("{:#?}", actual);
                let expected = format!("{:#?}", expected);

                panic!("{}", Changeset::new(&actual, &expected, "\n"));
            }
        }}
    }

    #[derive(Debug, Clone, PartialEq)]
    struct ComponentTree {
        decl: cm_rust::ComponentDecl,
        children: Vec<(LongName, ftest::ChildOptions, ComponentTree)>,
    }

    impl ComponentTree {
        fn new_from_resolver(
            url: &str,
            registry: Arc<resolver::Registry>,
        ) -> BoxFuture<'_, Option<ComponentTree>> {
            async move {
                let decl_from_resolver = match registry.get_decl_for_url(url).await {
                    Some(decl) => decl,
                    None => return None,
                };

                let mut self_ = ComponentTree { decl: decl_from_resolver, children: vec![] };
                let children = mem::replace(&mut self_.decl.children, Box::from([]));
                for child in children {
                    match Self::new_from_resolver(child.url.as_str(), registry.clone()).await {
                        None => {
                            push_box(&mut self_.decl.children, child);
                        }
                        Some(child_tree) => {
                            let child_options = ftest::ChildOptions {
                                startup: match child.startup {
                                    fcdecl::StartupMode::Eager => Some(fcdecl::StartupMode::Eager),
                                    fcdecl::StartupMode::Lazy => None,
                                },
                                environment: child.environment.map(|e| e.to_string()),
                                on_terminate: match child.on_terminate {
                                    Some(fcdecl::OnTerminate::None) => {
                                        Some(fcdecl::OnTerminate::None)
                                    }
                                    Some(fcdecl::OnTerminate::Reboot) => {
                                        Some(fcdecl::OnTerminate::Reboot)
                                    }
                                    None => None,
                                },
                                config_overrides: match child.config_overrides {
                                    Some(overrides) => Some(overrides.native_into_fidl()),
                                    None => None,
                                },
                                ..Default::default()
                            };
                            self_.children.push((child.name, child_options, child_tree));
                        }
                    }
                }
                Some(self_)
            }
            .boxed()
        }

        // Adds the `BINDER_EXPOSE_DECL` to the root component in the tree
        fn add_binder_expose(&mut self) {
            push_box(&mut self.decl.exposes, BINDER_EXPOSE_DECL.clone());
        }

        fn add_recursive_automatic_decls(&mut self) {
            let create_offer_decl =
                |kind: CapabilityTypeName, child_name: &str, name: cm_types::Name| match kind {
                    CapabilityTypeName::Protocol => OfferBuilder::protocol()
                        .name(name.as_str())
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child(child_name)
                        .build(),
                    CapabilityTypeName::Dictionary => OfferBuilder::dictionary()
                        .name(name.as_str())
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child(child_name)
                        .build(),
                    _ => unreachable!("we only call this with protocols and dictionaries"),
                };

            for child in &self.decl.children {
                for (kind, capability) in &*CAPABILITIES_ROUTED_TO_ALL {
                    push_box(
                        &mut self.decl.offers,
                        create_offer_decl(*kind, child.name.as_ref(), capability.clone()),
                    );
                }
            }

            for (child_name, _, _) in &self.children {
                for (kind, capability) in &*CAPABILITIES_ROUTED_TO_ALL {
                    push_box(
                        &mut self.decl.offers,
                        create_offer_decl(*kind, child_name.as_ref(), capability.clone()),
                    );
                }
            }

            for (_, _, child_tree) in &mut self.children {
                child_tree.add_recursive_automatic_decls();
            }
        }

        fn add_auto_decls(&mut self) {
            self.add_binder_expose();
            self.add_recursive_automatic_decls();
        }
    }

    fn tree_to_realm_node(tree: ComponentTree) -> BoxFuture<'static, RealmNode2> {
        async move {
            let node = RealmNode2::new_from_decl(tree.decl, false);
            for (child_name, options, tree) in tree.children {
                let child_node = tree_to_realm_node(tree).await;
                node.state.lock().await.mutable_children.insert(child_name, (options, child_node));
            }
            node
        }
        .boxed()
    }

    // Builds the given ComponentTree, and returns the root URL and the resolver that holds the
    // built declarations
    async fn build_tree(
        tree: &mut ComponentTree,
    ) -> Result<(String, Arc<resolver::Registry>), ftest::RealmBuilderError> {
        let res = build_tree_helper(tree.clone()).await;

        // We want to be able to check our component tree against the registry later, but the
        // builder automatically puts stuff into the root realm when building. Add that to our
        // local tree here, so that our tree looks the same as what hopefully got put in the
        // resolver.
        tree.add_auto_decls();

        res
    }

    fn launch_builder_task(
        realm_node: RealmNode2,
        registry: Arc<resolver::Registry>,
        runner: Arc<runner::Runner>,
        runner_proxy_placeholder: Arc<Mutex<Option<fcrunner::ComponentRunnerProxy>>>,
        realm_has_been_built: Arc<AtomicBool>,
        realm_contents: Arc<Mutex<ManagedRealmContents>>,
    ) -> (ftest::BuilderProxy, fasync::Task<()>) {
        let pkg_dir =
            fuchsia_fs::directory::open_in_namespace("/pkg", fuchsia_fs::PERM_READABLE).unwrap();
        let builder = Builder {
            pkg_dir,
            realm_node,
            registry,
            runner,
            runner_proxy_placeholder,
            realm_has_been_built,
            realm_contents,
        };

        let (builder_proxy, builder_stream) = create_proxy_and_stream::<ftest::BuilderMarker>();

        let builder_stream_task = fasync::Task::local(async move {
            builder.handle_stream(builder_stream).await.expect("failed to handle builder stream");
        });
        (builder_proxy, builder_stream_task)
    }

    async fn build_tree_helper(
        tree: ComponentTree,
    ) -> Result<(String, Arc<resolver::Registry>), ftest::RealmBuilderError> {
        let realm_node = tree_to_realm_node(tree).await;

        let registry = resolver::Registry::new();
        let runner = runner::Runner::new();
        let (builder_proxy, _builder_stream_task) = launch_builder_task(
            realm_node,
            registry.clone(),
            runner,
            Arc::new(Mutex::new(None)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(ManagedRealmContents::default())),
        );

        let (runner_client_end, runner_server_end) = create_endpoints();
        // Park the server end.  We don't want to close it here because the client end is monitored
        // and clean-up tasks run when it gets peer closed.
        fasync::Task::spawn(async move {
            let _runner_server_end = runner_server_end;
            let () = futures::future::pending().await;
        })
        .detach();
        let res =
            builder_proxy.build(runner_client_end).await.expect("failed to send build command");
        match res {
            Ok(url) => Ok((url, registry)),
            Err(e) => Err(e),
        }
    }

    // Holds the task for handling a new realm stream and a new builder stream, along with proxies
    // for those streams and the registry and runner the tasks will manipulate.
    #[allow(unused)]
    struct RealmAndBuilderTask {
        realm_proxy: ftest::RealmProxy,
        builder_proxy: ftest::BuilderProxy,
        registry: Arc<resolver::Registry>,
        runner: Arc<runner::Runner>,
        realm_and_builder_task: Option<fasync::Task<()>>,
        runner_stream: fcrunner::ComponentRunnerRequestStream,
        runner_client_end: Option<ClientEnd<fcrunner::ComponentRunnerMarker>>,
        realm_contents: Arc<Mutex<ManagedRealmContents>>,
    }

    impl RealmAndBuilderTask {
        fn new() -> Self {
            let (realm_proxy, realm_stream) = create_proxy_and_stream::<ftest::RealmMarker>();
            let pkg_dir = fuchsia_fs::directory::open_in_namespace(
                "/pkg",
                fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_EXECUTABLE,
            )
            .unwrap();
            let realm_root = RealmNode2::new();

            let registry = resolver::Registry::new();
            let runner = runner::Runner::new();
            let runner_proxy_placeholder = Arc::new(Mutex::new(None));
            let realm_contents = Arc::new(Mutex::new(ManagedRealmContents::default()));

            let realm_has_been_built = Arc::new(AtomicBool::new(false));

            let (builder_proxy, builder_task) = launch_builder_task(
                realm_root.clone(),
                registry.clone(),
                runner.clone(),
                runner_proxy_placeholder.clone(),
                realm_has_been_built.clone(),
                realm_contents.clone(),
            );

            let realm = Realm {
                pkg_dir,
                realm_node: realm_root,
                registry: registry.clone(),
                runner: runner.clone(),
                runner_proxy_placeholder,
                realm_path: vec![],
                execution_scope: ExecutionScope::new(),
                realm_has_been_built,
                realm_contents: realm_contents.clone(),
            };

            let realm_and_builder_task = fasync::Task::local(async move {
                realm.handle_stream(realm_stream).await.expect("failed to handle realm stream");
                builder_task.await;
            });
            let (runner_client_end, runner_stream) =
                create_request_stream::<fcrunner::ComponentRunnerMarker>();
            Self {
                realm_proxy,
                builder_proxy,
                registry,
                runner,
                realm_and_builder_task: Some(realm_and_builder_task),
                runner_stream,
                runner_client_end: Some(runner_client_end),
                realm_contents,
            }
        }

        async fn call_build(&mut self) -> Result<String, ftest::RealmBuilderError> {
            self.builder_proxy
                .build(self.runner_client_end.take().expect("call_build called twice"))
                .await
                .expect("failed to send build command")
        }

        // Calls `Builder.Build` on `self.builder_proxy`, which should populate `self.registry`
        // with the contents of the realm and then return the URL for the root of this realm. That
        // URL is then used to look up the `ComponentTree` that ended up in the resolver, which can
        // be `assert_eq`'d against what the tree is expected to be.
        async fn call_build_and_get_tree(&mut self) -> ComponentTree {
            let url = self.call_build().await.expect("builder unexpectedly returned an error");
            ComponentTree::new_from_resolver(&url, self.registry.clone())
                .await
                .expect("tree missing from resolver")
        }

        async fn add_child_or_panic(&self, name: &str, url: &str, options: ftest::ChildOptions) {
            let () = self
                .realm_proxy
                .add_child(name, url, &options)
                .await
                .expect("failed to make Realm.AddChild call")
                .expect("failed to add child");
        }

        async fn add_route_or_panic(
            &self,
            capabilities: Vec<ftest::Capability>,
            from: fcdecl::Ref,
            tos: Vec<fcdecl::Ref>,
        ) {
            self.add_route_from_dictionary_or_panic(capabilities, from, ".", tos).await
        }

        async fn add_route_from_dictionary_or_panic(
            &self,
            capabilities: Vec<ftest::Capability>,
            from: fcdecl::Ref,
            from_dictionary: &str,
            tos: Vec<fcdecl::Ref>,
        ) {
            let () = self
                .realm_proxy
                .add_route_from_dictionary(&capabilities, &from, from_dictionary, &tos)
                .await
                .expect("failed to make Realm.AddRouteFromDictionary call")
                .expect("failed to add route");
        }
    }

    #[fuchsia::test]
    async fn build_called_twice() {
        let realm_node = RealmNode2::new();

        let (builder_proxy, _builder_stream_task) = launch_builder_task(
            realm_node,
            resolver::Registry::new(),
            runner::Runner::new(),
            Arc::new(Mutex::new(None)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(ManagedRealmContents::default())),
        );

        let (runner_client_end, runner_server_end) = create_endpoints();
        drop(runner_server_end);
        let res =
            builder_proxy.build(runner_client_end).await.expect("failed to send build command");
        assert!(res.is_ok());

        let (runner_client_end, runner_server_end) = create_endpoints();
        drop(runner_server_end);
        let res =
            builder_proxy.build(runner_client_end).await.expect("failed to send build command");
        assert_eq!(Err(ftest::RealmBuilderError::BuildAlreadyCalled), res);
    }

    #[fuchsia::test]
    async fn build_empty_realm() {
        let mut tree = ComponentTree { decl: cm_rust::ComponentDecl::default(), children: vec![] };
        let (root_url, registry) = build_tree(&mut tree).await.expect("failed to build tree");
        let tree_from_resolver = ComponentTree::new_from_resolver(&root_url, registry).await;
        assert_decls_eq!(tree_from_resolver.unwrap(), tree);
    }

    #[fuchsia::test]
    async fn building_invalid_realm_errors() {
        let mut tree = ComponentTree {
            decl: ComponentDeclBuilder::new()
                .offer(
                    OfferBuilder::protocol()
                        .name(LogSinkMarker::PROTOCOL_NAME)
                        .source(cm_rust::offer::OfferSource::Parent)
                        // This doesn't exist
                        .target_static_child("a"),
                )
                .build(),
            children: vec![],
        };
        let error = build_tree(&mut tree).await.expect_err("builder didn't notice invalid decl");
        assert_eq!(error, ftest::RealmBuilderError::InvalidComponentDecl);
    }

    #[fuchsia::test]
    async fn build_realm_expect_automatic_routing() {
        let mut expected_output_tree = ComponentTree {
            decl: ComponentDeclBuilder::new()
                .offer(
                    OfferBuilder::dictionary()
                        .name(DIAGNOSTICS_DICT_NAME)
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a"),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name(LogSinkMarker::PROTOCOL_NAME)
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a"),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name(InspectSinkMarker::PROTOCOL_NAME)
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a"),
                )
                .offer(
                    OfferBuilder::dictionary()
                        .name(DIAGNOSTICS_DICT_NAME)
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("b"),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name(LogSinkMarker::PROTOCOL_NAME)
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("b"),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name(InspectSinkMarker::PROTOCOL_NAME)
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("b"),
                )
                .child_default("a")
                .build(),
            children: vec![(
                "b".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree {
                    decl: ComponentDeclBuilder::new()
                        .offer(
                            OfferBuilder::dictionary()
                                .name(DIAGNOSTICS_DICT_NAME)
                                .source(cm_rust::offer::OfferSource::Parent)
                                .target_static_child("b_child_static"),
                        )
                        .offer(
                            OfferBuilder::protocol()
                                .name(LogSinkMarker::PROTOCOL_NAME)
                                .source(cm_rust::offer::OfferSource::Parent)
                                .target_static_child("b_child_static"),
                        )
                        .offer(
                            OfferBuilder::protocol()
                                .name(InspectSinkMarker::PROTOCOL_NAME)
                                .source(cm_rust::offer::OfferSource::Parent)
                                .target_static_child("b_child_static"),
                        )
                        .offer(
                            OfferBuilder::dictionary()
                                .name(DIAGNOSTICS_DICT_NAME)
                                .source(cm_rust::offer::OfferSource::Parent)
                                .target_static_child("b_child_dynamic"),
                        )
                        .offer(
                            OfferBuilder::protocol()
                                .name(LogSinkMarker::PROTOCOL_NAME)
                                .source(cm_rust::offer::OfferSource::Parent)
                                .target_static_child("b_child_dynamic"),
                        )
                        .offer(
                            OfferBuilder::protocol()
                                .name(InspectSinkMarker::PROTOCOL_NAME)
                                .source(cm_rust::offer::OfferSource::Parent)
                                .target_static_child("b_child_dynamic"),
                        )
                        .child_default("b_child_static")
                        .build(),
                    children: vec![(
                        "b_child_dynamic".parse().unwrap(),
                        ftest::ChildOptions::default(),
                        ComponentTree {
                            decl: cm_rust::ComponentDecl { ..cm_rust::ComponentDecl::default() },
                            children: vec![],
                        },
                    )],
                },
            )],
        };

        // only binder, don't call the full automatic routing function, or
        // it will duplicate the expected output.
        expected_output_tree.add_binder_expose();

        let mut input_tree = ComponentTree {
            decl: ComponentDeclBuilder::new().child_default("a").build(),
            children: vec![(
                "b".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree {
                    decl: ComponentDeclBuilder::new().child_default("b_child_static").build(),
                    children: vec![(
                        "b_child_dynamic".parse().unwrap(),
                        ftest::ChildOptions::default(),
                        ComponentTree {
                            decl: cm_rust::ComponentDecl { ..cm_rust::ComponentDecl::default() },
                            children: vec![],
                        },
                    )],
                },
            )],
        };

        let (root_url, registry) = build_tree(&mut input_tree).await.expect("failed to build tree");
        let tree_from_resolver = ComponentTree::new_from_resolver(&root_url, registry).await;
        assert_decls_eq!(tree_from_resolver.unwrap(), expected_output_tree);
    }

    #[fuchsia::test]
    async fn build_realm_with_child_decl() {
        let mut tree = ComponentTree {
            decl: cm_rust::ComponentDecl {
                children: Box::from([ChildBuilder::new().name("a").build()]),
                ..cm_rust::ComponentDecl::default()
            },
            children: vec![],
        };
        let (root_url, registry) = build_tree(&mut tree).await.expect("failed to build tree");
        let tree_from_resolver = ComponentTree::new_from_resolver(&root_url, registry).await;
        assert_decls_eq!(tree_from_resolver.unwrap(), tree);
    }

    #[fuchsia::test]
    async fn build_realm_with_mutable_child() {
        let mut tree = ComponentTree {
            decl: cm_rust::ComponentDecl::default(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree { decl: cm_rust::ComponentDecl::default(), children: vec![] },
            )],
        };
        let (root_url, registry) = build_tree(&mut tree).await.expect("failed to build tree");
        let tree_from_resolver = ComponentTree::new_from_resolver(&root_url, registry).await;
        assert_decls_eq!(tree_from_resolver.unwrap(), tree);
    }

    #[fuchsia::test]
    async fn build_realm_with_child_decl_and_mutable_child() {
        let mut tree = ComponentTree {
            decl: cm_rust::ComponentDecl {
                children: Box::from([ChildBuilder::new().name("a").build()]),
                ..cm_rust::ComponentDecl::default()
            },
            children: vec![(
                "b".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree { decl: cm_rust::ComponentDecl::default(), children: vec![] },
            )],
        };
        let (root_url, registry) = build_tree(&mut tree).await.expect("failed to build tree");
        let tree_from_resolver = ComponentTree::new_from_resolver(&root_url, registry).await;
        assert_decls_eq!(tree_from_resolver.unwrap(), tree);
    }

    #[fuchsia::test]
    async fn build_realm_with_mutable_grandchild() {
        let mut tree = ComponentTree {
            decl: cm_rust::ComponentDecl::default(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree {
                    decl: cm_rust::ComponentDecl::default(),
                    children: vec![(
                        "b".parse().unwrap(),
                        ftest::ChildOptions::default(),
                        ComponentTree { decl: cm_rust::ComponentDecl::default(), children: vec![] },
                    )],
                },
            )],
        };
        let (root_url, registry) = build_tree(&mut tree).await.expect("failed to build tree");
        let tree_from_resolver = ComponentTree::new_from_resolver(&root_url, registry).await;
        assert_decls_eq!(tree_from_resolver.unwrap(), tree);
    }

    #[fuchsia::test]
    async fn build_realm_with_eager_mutable_child() {
        let mut tree = ComponentTree {
            decl: cm_rust::ComponentDecl::default(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions {
                    startup: Some(fcdecl::StartupMode::Eager),
                    ..Default::default()
                },
                ComponentTree { decl: cm_rust::ComponentDecl::default(), children: vec![] },
            )],
        };
        let (root_url, registry) = build_tree(&mut tree).await.expect("failed to build tree");
        let tree_from_resolver = ComponentTree::new_from_resolver(&root_url, registry).await;
        assert_decls_eq!(tree_from_resolver.unwrap(), tree);
    }

    #[fuchsia::test]
    async fn build_realm_with_mutable_child_in_a_new_environment() {
        let mut tree = ComponentTree {
            decl: ComponentDeclBuilder::new()
                .environment(
                    EnvironmentBuilder::new()
                        .name("new-env")
                        .extends(fcdecl::EnvironmentExtends::None)
                        .resolver(cm_rust::ResolverRegistration {
                            resolver: "test".parse().unwrap(),
                            source: cm_rust::RegistrationSource::Parent,
                            scheme: "test".to_string(),
                        })
                        .stop_timeout(1),
                )
                .build(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions {
                    environment: Some("new-env".to_string()),
                    ..Default::default()
                },
                ComponentTree { decl: cm_rust::ComponentDecl::default(), children: vec![] },
            )],
        };
        let (root_url, registry) = build_tree(&mut tree).await.expect("failed to build tree");
        let tree_from_resolver = ComponentTree::new_from_resolver(&root_url, registry).await;
        assert_decls_eq!(tree_from_resolver.unwrap(), tree);
    }

    #[fuchsia::test]
    async fn build_realm_with_mutable_child_with_on_terminate() {
        let mut tree = ComponentTree {
            decl: cm_rust::ComponentDecl::default(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions {
                    on_terminate: Some(fcdecl::OnTerminate::Reboot),
                    ..Default::default()
                },
                ComponentTree { decl: cm_rust::ComponentDecl::default(), children: vec![] },
            )],
        };
        let (root_url, registry) = build_tree(&mut tree).await.expect("failed to build tree");
        let tree_from_resolver = ComponentTree::new_from_resolver(&root_url, registry).await;
        assert_decls_eq!(tree_from_resolver.unwrap(), tree);
    }

    #[fuchsia::test]
    async fn build_fills_in_the_runner_proxy() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();

        // Add two local children
        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_local_child returned an error");
        realm_and_builder_task
            .realm_proxy
            .add_local_child("b", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_local_child")
            .expect("add_local_child returned an error");

        // Confirm that the local component runner has entries for the two children we just added
        let local_component_proxies = realm_and_builder_task.runner.local_component_proxies().await;
        // "a" was added first, so it gets 0
        assert!(local_component_proxies.contains_key(&"0".to_string()));
        // "b" was added second, so it gets 1
        assert!(local_component_proxies.contains_key(&"1".to_string()));

        // Confirm that the entries in the local_components runner for these children does not have a
        // `fcrunner::ComponentRunnerProxy` for these children, as this value is supposed to be
        // populated with the channel provided by `Builder.Build`, and we haven't called that yet.
        let get_runner_proxy =
            |local_component_proxies: &HashMap<_, _>, id: &str| match local_component_proxies
                .clone()
                .remove(&id.to_string())
            {
                Some(runner::ComponentImplementer::RunnerProxy(rp)) => rp,
                Some(_) => {
                    panic!("unexpected component implementer")
                }
                None => panic!("value unexpectedly missing"),
            };

        assert!(get_runner_proxy(&local_component_proxies, "0").lock().await.is_none());
        assert!(get_runner_proxy(&local_component_proxies, "1").lock().await.is_none());

        // Call `Builder.Build`, and confirm that the entries for our local children in the local
        // component runner now has a `fcrunner::ComponentRunnerProxy`.
        let _ = realm_and_builder_task.call_build().await.expect("build failed");

        assert!(get_runner_proxy(&local_component_proxies, "0").lock().await.is_some());
        assert!(get_runner_proxy(&local_component_proxies, "1").lock().await.is_some());

        // Confirm that the `fcrunner::ComponentRunnerProxy` for one of the local children has the
        // value we expect, by writing a value into it and seeing the same value come out on the
        // other side of our channel.
        let example_program = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: "hippos".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str("rule!".to_string()))),
            }]),
            ..Default::default()
        };

        let (_controller_client_end, controller_server_end) =
            create_endpoints::<fcrunner::ComponentControllerMarker>();
        let runner_proxy_for_a =
            get_runner_proxy(&local_component_proxies, "0").lock().await.clone().unwrap();
        runner_proxy_for_a
            .start(
                fcrunner::ComponentStartInfo {
                    program: Some(example_program.clone()),
                    ..Default::default()
                },
                controller_server_end,
            )
            .expect("failed to write start message");
        assert_matches!(
            realm_and_builder_task
                .runner_stream
                .try_next()
                .await
                .expect("failed to read from runner_stream"),
            Some(fcrunner::ComponentRunnerRequest::Start { start_info, .. })
                if start_info.program == Some(example_program)
        );
    }

    #[fuchsia::test]
    async fn add_child() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child("a", "test:///a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let mut expected_tree = ComponentTree {
            decl: ComponentDeclBuilder::new_empty_component().child_default("a").build(),
            children: vec![],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[fuchsia::test]
    async fn add_absolute_child_that_conflicts_with_child_decl() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child("a", "test:///a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .add_child("a", "test:///a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect_err("add_child was supposed to return an error");
        assert_eq!(err, ftest::RealmBuilderError::ChildAlreadyExists);
    }

    #[fuchsia::test]
    async fn add_absolute_child_that_conflicts_with_mutable_child() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child(
                "a",
                "#meta/realm_builder_server_unit_tests.cm",
                &ftest::ChildOptions::default(),
            )
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .add_child("a", "test:///a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect_err("add_child was supposed to return an error");
        assert_eq!(err, ftest::RealmBuilderError::ChildAlreadyExists);
    }

    #[fuchsia::test]
    async fn add_relative_child_that_conflicts_with_child_decl() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child("a", "test:///a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .add_child(
                "a",
                "#meta/realm_builder_server_unit_tests.cm",
                &ftest::ChildOptions::default(),
            )
            .await
            .expect("failed to call add_child")
            .expect_err("add_child was supposed to return an error");
        assert_eq!(err, ftest::RealmBuilderError::ChildAlreadyExists);
    }

    #[fuchsia::test]
    async fn add_relative_child_that_conflicts_with_mutable_child() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child(
                "a",
                "#meta/realm_builder_server_unit_tests.cm",
                &ftest::ChildOptions::default(),
            )
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .add_child(
                "a",
                "#meta/realm_builder_server_unit_tests.cm",
                &ftest::ChildOptions::default(),
            )
            .await
            .expect("failed to call add_child")
            .expect_err("add_child was supposed to return an error");
        assert_eq!(err, ftest::RealmBuilderError::ChildAlreadyExists);
    }

    #[fuchsia::test]
    async fn add_relative_child() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child(
                "a",
                "#meta/realm_builder_server_unit_tests.cm",
                &ftest::ChildOptions::default(),
            )
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;

        let a_decl_file = fuchsia_fs::file::open_in_namespace(
            "/pkg/meta/realm_builder_server_unit_tests.cm",
            fuchsia_fs::PERM_READABLE,
        )
        .expect("failed to open manifest");
        let a_decl = fuchsia_fs::file::read_fidl::<fcdecl::Component>(&a_decl_file)
            .await
            .expect("failed to read manifest")
            .fidl_into_native();

        let mut expected_tree = ComponentTree {
            decl: cm_rust::ComponentDecl::default(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree { decl: a_decl, children: vec![] },
            )],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[fuchsia::test]
    async fn add_relative_child_with_child() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child(
                "realm_with_child",
                "#meta/realm_with_child.cm",
                &ftest::ChildOptions {
                    startup: Some(fcdecl::StartupMode::Eager),
                    ..Default::default()
                },
            )
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;

        let realm_with_child_decl_file = fuchsia_fs::file::open_in_namespace(
            "/pkg/meta/realm_with_child.cm",
            fuchsia_fs::PERM_READABLE,
        )
        .expect("failed to open manifest");
        let mut realm_with_child_decl: cm_rust::ComponentDecl =
            fuchsia_fs::file::read_fidl::<fcdecl::Component>(&realm_with_child_decl_file)
                .await
                .expect("failed to read manifest")
                .fidl_into_native();

        // The "a" child is rewritten by realm builder
        realm_with_child_decl.children = IntoIterator::into_iter(realm_with_child_decl.children)
            .filter(|c| c.name.as_str() != "a")
            .collect();

        let a_decl_file =
            fuchsia_fs::file::open_in_namespace("/pkg/meta/a.cm", fuchsia_fs::PERM_READABLE)
                .expect("failed to open manifest");
        let a_decl = fuchsia_fs::file::read_fidl::<fcdecl::Component>(&a_decl_file)
            .await
            .expect("failed to read manifest")
            .fidl_into_native();

        let mut expected_tree = ComponentTree {
            decl: ComponentDeclBuilder::new_empty_component()
                .offer(
                    OfferBuilder::dictionary()
                        .name(DIAGNOSTICS_DICT_NAME)
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("realm_with_child"),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name(LogSinkMarker::PROTOCOL_NAME)
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("realm_with_child"),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name(InspectSinkMarker::PROTOCOL_NAME)
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("realm_with_child"),
                )
                .build(),
            children: vec![(
                "realm_with_child".parse().unwrap(),
                ftest::ChildOptions {
                    startup: Some(fcdecl::StartupMode::Eager),
                    ..Default::default()
                },
                ComponentTree {
                    decl: realm_with_child_decl,
                    children: vec![(
                        "a".parse().unwrap(),
                        ftest::ChildOptions::default(),
                        ComponentTree { decl: a_decl, children: vec![] },
                    )],
                },
            )],
        };

        expected_tree.add_binder_expose();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[fuchsia::test]
    async fn add_child_from_decl() {
        let a_decl = ComponentDeclBuilder::new()
            .program(cm_rust::ProgramDecl {
                runner: Some("hippo".parse().unwrap()),
                info: fdata::Dictionary::default(),
            })
            .use_(UseBuilder::protocol().name("example.Hippo"))
            .build();

        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child_from_decl(
                "a",
                &a_decl.clone().native_into_fidl(),
                &ftest::ChildOptions::default(),
            )
            .await
            .expect("failed to call add_child")
            .expect("add_child_from_decl returned an error");
        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let mut expected_tree = ComponentTree {
            decl: cm_rust::ComponentDecl::default(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree { decl: a_decl, children: vec![] },
            )],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[fuchsia::test]
    async fn add_child_from_decl_that_conflicts_with_child_decl() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child("a", "test:///a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .add_child_from_decl(
                "a",
                &fcdecl::Component::default(),
                &ftest::ChildOptions::default(),
            )
            .await
            .expect("failed to call add_child")
            .expect_err("add_child_from_decl was supposed to error");
        assert_eq!(err, ftest::RealmBuilderError::ChildAlreadyExists);
    }

    #[fuchsia::test]
    async fn add_child_from_decl_that_conflicts_with_mutable_child() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child(
                "a",
                "#meta/realm_builder_server_unit_tests.cm",
                &ftest::ChildOptions::default(),
            )
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .add_child_from_decl(
                "a",
                &fcdecl::Component::default(),
                &ftest::ChildOptions::default(),
            )
            .await
            .expect("failed to call add_child")
            .expect_err("add_child_from_decl was supposed to error");
        assert_eq!(err, ftest::RealmBuilderError::ChildAlreadyExists);
    }

    #[fuchsia::test]
    async fn add_route_does_not_mutate_children_added_from_decl() {
        let a_decl = ComponentDeclBuilder::new()
            .program(cm_rust::ProgramDecl {
                runner: Some("hippo".parse().unwrap()),
                info: fdata::Dictionary::default(),
            })
            .use_(UseBuilder::protocol().name("example.Hippo").path("/svc/non-default-path"))
            .build()
            .native_into_fidl();

        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child_from_decl("a", &a_decl, &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child_from_decl returned an error");
        realm_and_builder_task
            .add_route_or_panic(
                vec![ftest::Capability::Protocol(ftest::Protocol {
                    name: Some("example.Hippo".to_owned()),
                    type_: Some(fcdecl::DependencyType::Strong),
                    ..Default::default()
                })],
                fcdecl::Ref::Parent(fcdecl::ParentRef {}),
                vec![fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "a".parse().unwrap(),
                    collection: None,
                })],
            )
            .await;
        let resulting_a_decl = realm_and_builder_task
            .realm_proxy
            .get_component_decl("a")
            .await
            .expect("failed to call get_component_decl")
            .expect("get_component_decl returned an error");
        assert_eq!(a_decl, resulting_a_decl);
    }

    #[fuchsia::test]
    async fn add_local_child() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let a_decl = ComponentDeclBuilder::new()
            .program(cm_rust::ProgramDecl {
                runner: Some(crate::runner::RUNNER_NAME.parse().unwrap()),
                info: fdata::Dictionary {
                    entries: Some(vec![
                        fdata::DictionaryEntry {
                            key: runner::LOCAL_COMPONENT_ID_KEY.to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str("0".to_string()))),
                        },
                        fdata::DictionaryEntry {
                            key: ftest::LOCAL_COMPONENT_NAME_KEY.to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str("a".to_string()))),
                        },
                    ]),
                    ..Default::default()
                },
            })
            .build();
        let mut expected_tree = ComponentTree {
            decl: cm_rust::ComponentDecl::default(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree { decl: a_decl, children: vec![] },
            )],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
        assert!(
            realm_and_builder_task
                .runner
                .local_component_proxies()
                .await
                .contains_key(&"0".to_string())
        );
    }

    #[fuchsia::test]
    async fn add_local_child_that_conflicts_with_child_decl() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child("a", "test:///a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect_err("add_local_child was supposed to error");
        assert_eq!(err, ftest::RealmBuilderError::ChildAlreadyExists);
    }

    #[fuchsia::test]
    async fn add_local_child_that_conflicts_with_mutable_child() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child(
                "a",
                "#meta/realm_builder_server_unit_tests.cm",
                &ftest::ChildOptions::default(),
            )
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect_err("add_local_child was supposed to error");
        assert_eq!(err, ftest::RealmBuilderError::ChildAlreadyExists);
    }

    #[fuchsia::test]
    async fn add_route_from_dictionary() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .add_child_or_panic("a", "test:///a", ftest::ChildOptions::default())
            .await;
        realm_and_builder_task
            .add_child_or_panic("b", "test:///b", ftest::ChildOptions::default())
            .await;

        // Assert that parent -> child routes generate proper offer decls.
        realm_and_builder_task
            .add_route_from_dictionary_or_panic(
                vec![
                    ftest::Capability::Protocol(ftest::Protocol {
                        name: Some("fuchsia.examples.Hippo".to_owned()),
                        as_: Some("fuchsia.examples.Elephant".to_owned()),
                        type_: Some(fcdecl::DependencyType::Strong),
                        ..Default::default()
                    }),
                    ftest::Capability::Directory(ftest::Directory {
                        name: Some("config-data".to_owned()),
                        rights: Some(fio::RW_STAR_DIR),
                        subdir: Some("component".to_owned()),
                        ..Default::default()
                    }),
                    ftest::Capability::Storage(ftest::Storage {
                        name: Some("temp".to_string()),
                        as_: Some("data".to_string()),
                        ..Default::default()
                    }),
                    ftest::Capability::Service(ftest::Service {
                        name: Some("fuchsia.examples.Whale".to_string()),
                        as_: Some("fuchsia.examples.Orca".to_string()),
                        ..Default::default()
                    }),
                    ftest::Capability::EventStream(ftest::EventStream {
                        name: Some("started".to_string()),
                        as_: Some("started_event".to_string()),
                        ..Default::default()
                    }),
                    ftest::Capability::Dictionary(ftest::Dictionary {
                        name: Some("dict".to_string()),
                        as_: Some("dict2".to_string()),
                        type_: Some(fcdecl::DependencyType::Weak),
                        ..Default::default()
                    }),
                ],
                fcdecl::Ref::Parent(fcdecl::ParentRef {}),
                "source/dict",
                vec![fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "a".parse().unwrap(),
                    collection: None,
                })],
            )
            .await;

        // Assert that child -> (child, capability) routes generate proper offer decls.
        realm_and_builder_task
            .realm_proxy
            .add_capability(&fcdecl::Capability::Dictionary(fcdecl::Dictionary {
                name: Some("my_dict".into()),
                ..Default::default()
            }))
            .await
            .unwrap()
            .unwrap();
        realm_and_builder_task
            .add_route_from_dictionary_or_panic(
                vec![ftest::Capability::Protocol(ftest::Protocol {
                    name: Some("fuchsia.examples.Echo".to_owned()),
                    ..Default::default()
                })],
                fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "a".parse().unwrap(),
                    collection: None,
                }),
                "source/dict",
                vec![
                    fcdecl::Ref::Child(fcdecl::ChildRef {
                        name: "b".parse().unwrap(),
                        collection: None,
                    }),
                    fcdecl::Ref::Capability(fcdecl::CapabilityRef {
                        name: "my_dict".parse().unwrap(),
                    }),
                ],
            )
            .await;

        // Assert that child -> parent routes generate proper expose decls.
        realm_and_builder_task
            .add_route_from_dictionary_or_panic(
                vec![
                    ftest::Capability::Protocol(ftest::Protocol {
                        name: Some("fuchsia.examples.Echo".to_owned()),
                        type_: Some(fcdecl::DependencyType::Weak),
                        ..Default::default()
                    }),
                    ftest::Capability::Dictionary(ftest::Dictionary {
                        name: Some("dict".into()),
                        as_: Some("dict2".into()),
                        ..Default::default()
                    }),
                ],
                fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "a".parse().unwrap(),
                    collection: None,
                }),
                "source/dict",
                vec![fcdecl::Ref::Parent(fcdecl::ParentRef {})],
            )
            .await;

        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let mut expected_tree = ComponentTree {
            decl: ComponentDeclBuilder::new_empty_component()
                .child_default("a")
                .child_default("b")
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.examples.Hippo")
                        .target_name("fuchsia.examples.Elephant")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .from_dictionary("source/dict")
                        .target_static_child("a"),
                )
                .offer(
                    OfferBuilder::directory()
                        .name("config-data")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a")
                        .rights(fio::RW_STAR_DIR)
                        .subdir("component")
                        .from_dictionary("source/dict"),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("temp")
                        .target_name("data")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a"),
                )
                .offer(
                    OfferBuilder::service()
                        .name("fuchsia.examples.Whale")
                        .target_name("fuchsia.examples.Orca")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .from_dictionary("source/dict")
                        .target_static_child("a"),
                )
                .offer(
                    OfferBuilder::event_stream()
                        .name("started")
                        .target_name("started_event")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a"),
                )
                .offer(
                    OfferBuilder::dictionary()
                        .name("dict")
                        .target_name("dict2")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .from_dictionary("source/dict")
                        .target_static_child("a")
                        .dependency(cm_rust::DependencyType::Weak),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.examples.Echo")
                        .source_static_child("a")
                        .from_dictionary("source/dict")
                        .target_static_child("b"),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.examples.Echo")
                        .source_static_child("a")
                        .from_dictionary("source/dict")
                        .target(cm_rust::offer::OfferTarget::Capability(
                            "my_dict".parse().unwrap(),
                        )),
                )
                .expose(
                    ExposeBuilder::protocol()
                        .name("fuchsia.examples.Echo")
                        .source_static_child("a")
                        .from_dictionary("source/dict"),
                )
                .expose(
                    ExposeBuilder::dictionary()
                        .name("dict")
                        .target_name("dict2")
                        .source_static_child("a")
                        .from_dictionary("source/dict"),
                )
                .dictionary_default("my_dict")
                .build(),
            children: vec![],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[fuchsia::test]
    async fn add_optional_route() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .add_child_or_panic("a", "test:///a", ftest::ChildOptions::default())
            .await;
        realm_and_builder_task
            .realm_proxy
            .add_local_child("b", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_local_child")
            .expect("add_local_child returned an error");

        // Assert that parent -> child optional capabilities generate proper offer decls.
        realm_and_builder_task
            .add_route_or_panic(
                vec![
                    ftest::Capability::Protocol(ftest::Protocol {
                        name: Some("fuchsia.examples.Hippo".to_owned()),
                        as_: Some("fuchsia.examples.Elephant".to_owned()),
                        type_: Some(fcdecl::DependencyType::Strong),
                        availability: Some(fcdecl::Availability::Optional),
                        ..Default::default()
                    }),
                    ftest::Capability::Directory(ftest::Directory {
                        name: Some("config-data".to_owned()),
                        rights: Some(fio::RW_STAR_DIR),
                        path: Some("/config-data".to_owned()),
                        subdir: Some("component".to_owned()),
                        availability: Some(fcdecl::Availability::Optional),
                        ..Default::default()
                    }),
                    ftest::Capability::Storage(ftest::Storage {
                        name: Some("temp".to_string()),
                        as_: Some("data".to_string()),
                        path: Some("/data".to_string()),
                        availability: Some(fcdecl::Availability::Optional),
                        ..Default::default()
                    }),
                    ftest::Capability::Service(ftest::Service {
                        name: Some("fuchsia.examples.Whale".to_string()),
                        as_: Some("fuchsia.examples.Orca".to_string()),
                        availability: Some(fcdecl::Availability::Optional),
                        ..Default::default()
                    }),
                ],
                fcdecl::Ref::Parent(fcdecl::ParentRef {}),
                vec![
                    fcdecl::Ref::Child(fcdecl::ChildRef {
                        name: "a".parse().unwrap(),
                        collection: None,
                    }),
                    fcdecl::Ref::Child(fcdecl::ChildRef {
                        name: "b".parse().unwrap(),
                        collection: None,
                    }),
                ],
            )
            .await;

        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let b_decl = ComponentDeclBuilder::new_empty_component()
            .program(cm_rust::ProgramDecl {
                runner: Some(crate::runner::RUNNER_NAME.parse().unwrap()),
                info: fdata::Dictionary {
                    entries: Some(vec![
                        fdata::DictionaryEntry {
                            key: runner::LOCAL_COMPONENT_ID_KEY.to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str("0".to_string()))),
                        },
                        fdata::DictionaryEntry {
                            key: ftest::LOCAL_COMPONENT_NAME_KEY.to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str("b".to_string()))),
                        },
                    ]),
                    ..Default::default()
                },
            })
            .use_(
                UseBuilder::protocol()
                    .name("fuchsia.examples.Elephant")
                    .availability(cm_rust::Availability::Optional),
            )
            .use_(
                UseBuilder::directory()
                    .name("config-data")
                    .path("/config-data")
                    .rights(fio::RW_STAR_DIR)
                    .availability(cm_rust::Availability::Optional),
            )
            .use_(
                UseBuilder::storage()
                    .name("data")
                    .path("/data")
                    .availability(cm_rust::Availability::Optional),
            )
            .use_(
                UseBuilder::service()
                    .name("fuchsia.examples.Orca")
                    .availability(cm_rust::Availability::Optional),
            )
            .build();
        let mut expected_tree = ComponentTree {
            decl: ComponentDeclBuilder::new_empty_component()
                .child_default("a")
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.examples.Hippo")
                        .target_name("fuchsia.examples.Elephant")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a")
                        .availability(cm_rust::Availability::Optional),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.examples.Hippo")
                        .target_name("fuchsia.examples.Elephant")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("b")
                        .availability(cm_rust::Availability::Optional),
                )
                .offer(
                    OfferBuilder::directory()
                        .name("config-data")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a")
                        .rights(fio::RW_STAR_DIR)
                        .subdir("component")
                        .availability(cm_rust::Availability::Optional),
                )
                .offer(
                    OfferBuilder::directory()
                        .name("config-data")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("b")
                        .rights(fio::RW_STAR_DIR)
                        .subdir("component")
                        .availability(cm_rust::Availability::Optional),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("temp")
                        .target_name("data")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a")
                        .availability(cm_rust::Availability::Optional),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("temp")
                        .target_name("data")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("b")
                        .availability(cm_rust::Availability::Optional),
                )
                .offer(
                    OfferBuilder::service()
                        .name("fuchsia.examples.Whale")
                        .target_name("fuchsia.examples.Orca")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a")
                        .availability(cm_rust::Availability::Optional),
                )
                .offer(
                    OfferBuilder::service()
                        .name("fuchsia.examples.Whale")
                        .target_name("fuchsia.examples.Orca")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("b")
                        .availability(cm_rust::Availability::Optional),
                )
                .build(),
            children: vec![(
                "b".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree { decl: b_decl, children: vec![] },
            )],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[fuchsia::test]
    async fn add_same_as_target_route() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .add_child_or_panic("a", "test:///a", ftest::ChildOptions::default())
            .await;

        // Assert that parent -> child optional capabilities generate proper offer decls.
        realm_and_builder_task
            .add_route_or_panic(
                vec![
                    ftest::Capability::Protocol(ftest::Protocol {
                        name: Some("fuchsia.examples.Hippo".to_owned()),
                        as_: Some("fuchsia.examples.Elephant".to_owned()),
                        type_: Some(fcdecl::DependencyType::Strong),
                        availability: Some(fcdecl::Availability::SameAsTarget),
                        ..Default::default()
                    }),
                    ftest::Capability::Directory(ftest::Directory {
                        name: Some("config-data".to_owned()),
                        rights: Some(fio::RW_STAR_DIR),
                        path: Some("/config-data".to_owned()),
                        subdir: Some("component".to_owned()),
                        availability: Some(fcdecl::Availability::SameAsTarget),
                        ..Default::default()
                    }),
                    ftest::Capability::Storage(ftest::Storage {
                        name: Some("temp".to_string()),
                        as_: Some("data".to_string()),
                        path: Some("/data".to_string()),
                        availability: Some(fcdecl::Availability::SameAsTarget),
                        ..Default::default()
                    }),
                    ftest::Capability::Service(ftest::Service {
                        name: Some("fuchsia.examples.Whale".to_string()),
                        as_: Some("fuchsia.examples.Orca".to_string()),
                        availability: Some(fcdecl::Availability::SameAsTarget),
                        ..Default::default()
                    }),
                ],
                fcdecl::Ref::Parent(fcdecl::ParentRef {}),
                vec![fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "a".parse().unwrap(),
                    collection: None,
                })],
            )
            .await;

        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let mut expected_tree = ComponentTree {
            decl: ComponentDeclBuilder::new_empty_component()
                .child_default("a")
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.examples.Hippo")
                        .target_name("fuchsia.examples.Elephant")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a")
                        .availability(cm_rust::Availability::SameAsTarget),
                )
                .offer(
                    OfferBuilder::directory()
                        .name("config-data")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a")
                        .rights(fio::RW_STAR_DIR)
                        .subdir("component")
                        .availability(cm_rust::Availability::SameAsTarget),
                )
                .offer(
                    OfferBuilder::storage()
                        .name("temp")
                        .target_name("data")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a")
                        .availability(cm_rust::Availability::SameAsTarget),
                )
                .offer(
                    OfferBuilder::service()
                        .name("fuchsia.examples.Whale")
                        .target_name("fuchsia.examples.Orca")
                        .source(cm_rust::offer::OfferSource::Parent)
                        .target_static_child("a")
                        .availability(cm_rust::Availability::SameAsTarget),
                )
                .build(),
            children: vec![],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[fuchsia::test]
    async fn same_as_target_route_to_local_component() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_local_child")
            .expect("add_local_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .add_route(
                &[ftest::Capability::Protocol(ftest::Protocol {
                    name: Some("fuchsia.examples.Hippo".to_owned()),
                    as_: Some("fuchsia.examples.Elephant".to_owned()),
                    type_: Some(fcdecl::DependencyType::Strong),
                    availability: Some(fcdecl::Availability::SameAsTarget),
                    ..Default::default()
                })],
                &fcdecl::Ref::Parent(fcdecl::ParentRef {}),
                &[fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "a".parse().unwrap(),
                    collection: None,
                })],
            )
            .await
            .expect("failed to call add_route")
            .expect_err("add_route should have returned an error");
        assert_eq!(err, ftest::RealmBuilderError::CapabilityInvalid);
    }

    #[fuchsia::test]
    async fn add_route_duplicate_decls() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call AddChildFromDecl")
            .expect("call failed");
        realm_and_builder_task
            .add_child_or_panic("b", "test:///b", ftest::ChildOptions::default())
            .await;
        realm_and_builder_task
            .add_child_or_panic("c", "test:///c", ftest::ChildOptions::default())
            .await;

        // Routing protocol from `a` should yield one and only one ExposeDecl.
        realm_and_builder_task
            .add_route_or_panic(
                vec![ftest::Capability::Protocol(ftest::Protocol {
                    name: Some("fuchsia.examples.Hippo".to_owned()),
                    ..Default::default()
                })],
                fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "a".parse().unwrap(),
                    collection: None,
                }),
                vec![fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "b".parse().unwrap(),
                    collection: None,
                })],
            )
            .await;
        realm_and_builder_task
            .add_route_or_panic(
                vec![ftest::Capability::Protocol(ftest::Protocol {
                    name: Some("fuchsia.examples.Hippo".to_owned()),
                    ..Default::default()
                })],
                fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "a".parse().unwrap(),
                    collection: None,
                }),
                vec![fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "c".parse().unwrap(),
                    collection: None,
                })],
            )
            .await;

        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let mut expected_tree = ComponentTree {
            decl: ComponentDeclBuilder::new_empty_component()
                .child_default("b")
                .child_default("c")
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.examples.Hippo")
                        .source_static_child("a")
                        .target_static_child("b"),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.examples.Hippo")
                        .source_static_child("a")
                        .target_static_child("c"),
                )
                .build(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree {
                    decl: ComponentDeclBuilder::new_empty_component()
                        .program(cm_rust::ProgramDecl {
                            runner: Some(crate::runner::RUNNER_NAME.parse().unwrap()),
                            info: fdata::Dictionary {
                                entries: Some(vec![
                                    fdata::DictionaryEntry {
                                        key: runner::LOCAL_COMPONENT_ID_KEY.to_string(),
                                        value: Some(Box::new(fdata::DictionaryValue::Str(
                                            "0".to_string(),
                                        ))),
                                    },
                                    fdata::DictionaryEntry {
                                        key: ftest::LOCAL_COMPONENT_NAME_KEY.to_string(),
                                        value: Some(Box::new(fdata::DictionaryValue::Str(
                                            "a".to_string(),
                                        ))),
                                    },
                                ]),
                                ..Default::default()
                            },
                        })
                        .protocol_default("fuchsia.examples.Hippo")
                        .expose(
                            ExposeBuilder::protocol()
                                .name("fuchsia.examples.Hippo")
                                .source(cm_rust::ExposeSource::Self_),
                        )
                        .build(),
                    children: vec![],
                },
            )],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[fuchsia::test]
    async fn realm_and_builder_exit_when_proxies_are_closed() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call AddChildFromDecl")
            .expect("call failed");
        let task = realm_and_builder_task.realm_and_builder_task.take();
        drop(realm_and_builder_task);
        task.unwrap().await;
    }

    #[fuchsia::test]
    async fn close_before_build_removes_local_components() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        assert_eq!(0, realm_and_builder_task.realm_contents.lock().await.urls.len());
        assert_eq!(0, realm_and_builder_task.realm_contents.lock().await.local_component_ids.len());
        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call AddChildFromDecl")
            .expect("call failed");
        assert_eq!(1, realm_and_builder_task.realm_contents.lock().await.local_component_ids.len());
        let runner = realm_and_builder_task.runner.clone();
        assert_eq!(1, runner.local_component_proxies().await.len());
        let task = realm_and_builder_task.realm_and_builder_task.take();
        drop(realm_and_builder_task);
        // Once the realm and builder tasks have finished executing, the state created by the
        // `add_local_child` call should be deleted.
        task.unwrap().await;
        assert_eq!(0, runner.local_component_proxies().await.len());
    }

    #[fuchsia::test]
    async fn closing_the_component_controller_removes_manifests_and_local_components() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();

        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call AddChildFromDecl")
            .expect("call failed");
        realm_and_builder_task
            .add_child_or_panic(
                "b",
                "#meta/realm_builder_server_unit_tests.cm",
                ftest::ChildOptions::default(),
            )
            .await;
        realm_and_builder_task
            .add_child_or_panic("c", "test:///c", ftest::ChildOptions::default())
            .await;

        let runner = realm_and_builder_task.runner.clone();
        let registry = realm_and_builder_task.registry.clone();
        assert_eq!(1, runner.local_component_proxies().await.len());

        let _ = realm_and_builder_task.call_build_and_get_tree().await;
        assert_eq!(3, registry.get_component_urls().await.len());
        // Dropping this closes the ComponentController channel we used when calling Build
        let task = realm_and_builder_task.realm_and_builder_task.take();
        drop(realm_and_builder_task);
        task.unwrap().await;

        // The work to garbage collect the manifests and local component we created happens
        // asynchronously, it's not guaranteed to be completed within any specific deadline. To
        // work around this, let's check every 100 milliseconds until the test times out.
        loop {
            if 0 == runner.local_component_proxies().await.len()
                && 0 == registry.get_component_urls().await.len()
            {
                break;
            }
            fasync::Timer::new(Duration::from_millis(100)).await;
        }
    }

    #[fuchsia::test]
    async fn add_route_mutates_decl() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();

        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call AddChildFromDecl")
            .expect("call failed");
        realm_and_builder_task
            .add_child_or_panic("b", "test:///b", ftest::ChildOptions::default())
            .await;
        realm_and_builder_task
            .add_child_or_panic("c", "test:///c", ftest::ChildOptions::default())
            .await;
        realm_and_builder_task
            .add_route_or_panic(
                vec![ftest::Capability::Protocol(ftest::Protocol {
                    name: Some("fuchsia.examples.Echo".to_owned()),
                    ..Default::default()
                })],
                fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "a".parse().unwrap(),
                    collection: None,
                }),
                vec![fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "b".parse().unwrap(),
                    collection: None,
                })],
            )
            .await;
        realm_and_builder_task
            .add_route_or_panic(
                vec![ftest::Capability::Protocol(ftest::Protocol {
                    name: Some("fuchsia.examples.RandonNumberGenerator".to_owned()),
                    ..Default::default()
                })],
                fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "c".parse().unwrap(),
                    collection: None,
                }),
                vec![fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "a".parse().unwrap(),
                    collection: None,
                })],
            )
            .await;

        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let mut expected_tree = ComponentTree {
            decl: ComponentDeclBuilder::new_empty_component()
                .child_default("b")
                .child_default("c")
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.examples.Echo")
                        .source_static_child("a")
                        .target_static_child("b"),
                )
                .offer(
                    OfferBuilder::protocol()
                        .name("fuchsia.examples.RandonNumberGenerator")
                        .source_static_child("c")
                        .target_static_child("a"),
                )
                .build(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree {
                    decl: ComponentDeclBuilder::new_empty_component()
                        .program(cm_rust::ProgramDecl {
                            runner: Some(crate::runner::RUNNER_NAME.parse().unwrap()),
                            info: fdata::Dictionary {
                                entries: Some(vec![
                                    fdata::DictionaryEntry {
                                        key: runner::LOCAL_COMPONENT_ID_KEY.to_string(),
                                        value: Some(Box::new(fdata::DictionaryValue::Str(
                                            "0".to_string(),
                                        ))),
                                    },
                                    fdata::DictionaryEntry {
                                        key: ftest::LOCAL_COMPONENT_NAME_KEY.to_string(),
                                        value: Some(Box::new(fdata::DictionaryValue::Str(
                                            "a".to_string(),
                                        ))),
                                    },
                                ]),
                                ..Default::default()
                            },
                        })
                        .protocol_default("fuchsia.examples.Echo")
                        .use_(UseBuilder::protocol().name("fuchsia.examples.RandonNumberGenerator"))
                        .expose(
                            ExposeBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .source(cm_rust::ExposeSource::Self_),
                        )
                        .build(),
                    children: vec![],
                },
            )],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[fuchsia::test]
    async fn add_child_to_child_realm() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        let (child_realm_proxy, child_realm_server_end) = create_proxy::<ftest::RealmMarker>();
        realm_and_builder_task
            .realm_proxy
            .add_child_realm("a", &ftest::ChildOptions::default(), child_realm_server_end)
            .await
            .expect("failed to call add_child_realm")
            .expect("add_child_realm returned an error");
        child_realm_proxy
            .add_child("b", "test:///b", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let mut expected_tree = ComponentTree {
            decl: cm_rust::ComponentDecl::default(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree {
                    decl: ComponentDeclBuilder::new_empty_component().child_default("b").build(),
                    children: vec![],
                },
            )],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[fuchsia::test]
    async fn get_component_decl() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let a_decl = realm_and_builder_task
            .realm_proxy
            .get_component_decl("a")
            .await
            .expect("failed to call get_component_decl")
            .expect("get_component_decl returned an error");
        assert_eq!(
            a_decl,
            ComponentDeclBuilder::new_empty_component()
                .program(cm_rust::ProgramDecl {
                    runner: Some(crate::runner::RUNNER_NAME.parse().unwrap()),
                    info: fdata::Dictionary {
                        entries: Some(vec![
                            fdata::DictionaryEntry {
                                key: runner::LOCAL_COMPONENT_ID_KEY.to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::Str("0".to_string()))),
                            },
                            fdata::DictionaryEntry {
                                key: ftest::LOCAL_COMPONENT_NAME_KEY.to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::Str("a".to_string()))),
                            },
                        ]),
                        ..Default::default()
                    },
                })
                .build()
                .native_into_fidl(),
        );
    }

    #[fuchsia::test]
    async fn get_component_decl_for_nonexistent_child() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        let err = realm_and_builder_task
            .realm_proxy
            .get_component_decl("a")
            .await
            .expect("failed to call get_component_decl")
            .expect_err("get_component_decl did not return an error");
        assert_eq!(err, ftest::RealmBuilderError::NoSuchChild);
    }

    #[fuchsia::test]
    async fn get_component_decl_for_child_behind_child_decl() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child("a", "test:///a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .get_component_decl("a")
            .await
            .expect("failed to call get_component_decl")
            .expect_err("get_component_decl did not return an error");
        assert_eq!(err, ftest::RealmBuilderError::ChildDeclNotVisible);
    }

    #[fuchsia::test]
    async fn replace_component_decl() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let mut a_decl: cm_rust::ComponentDecl = realm_and_builder_task
            .realm_proxy
            .get_component_decl("a")
            .await
            .expect("failed to call get_component_decl")
            .expect("get_component_decl returned an error")
            .fidl_into_native();
        push_box(&mut a_decl.uses, UseBuilder::protocol().name("example.Hippo").build());
        realm_and_builder_task
            .realm_proxy
            .replace_component_decl("a", &a_decl.clone().native_into_fidl())
            .await
            .expect("failed to call replace_component_decl")
            .expect("replace_component_decl returned an error");
        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let mut expected_tree = ComponentTree {
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree { decl: a_decl, children: vec![] },
            )],
            decl: cm_rust::ComponentDecl::default(),
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[test_case(vec![
        create_valid_capability()],
        fcdecl::Ref::Child(fcdecl::ChildRef { name: "unknown".parse().unwrap(),
            collection: None
        }),
        vec![],
        ftest::RealmBuilderError::NoSuchSource ; "no_such_source")]
    #[test_case(vec![
        create_valid_capability()],
        fcdecl::Ref::Child(fcdecl::ChildRef { name: "a".parse().unwrap(),
            collection: None
        }),
        vec![
            fcdecl::Ref::Child(fcdecl::ChildRef { name: "unknown".parse().unwrap(),
                collection: None
            }),
        ],
        ftest::RealmBuilderError::NoSuchTarget ; "no_such_target")]
    #[test_case(vec![
        create_valid_capability()],
        fcdecl::Ref::Child(fcdecl::ChildRef { name: "a".parse().unwrap(),
            collection: None
        }),
        vec![
            fcdecl::Ref::Child(fcdecl::ChildRef { name: "a".parse().unwrap(),
                collection: None
            }),
        ],
        ftest::RealmBuilderError::SourceAndTargetMatch ; "source_and_target_match")]
    #[test_case(vec![],
        fcdecl::Ref::Child(fcdecl::ChildRef { name: "a".parse().unwrap(),
            collection: None
        }),
        vec![fcdecl::Ref::Parent(fcdecl::ParentRef {})],
        ftest::RealmBuilderError::CapabilitiesEmpty ; "capabilities_empty")]
    #[fuchsia::test]
    async fn add_route_error(
        capabilities: Vec<ftest::Capability>,
        from: fcdecl::Ref,
        to: Vec<fcdecl::Ref>,
        expected_err: ftest::RealmBuilderError,
    ) {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .add_child_or_panic("a", "test:///a", ftest::ChildOptions::default())
            .await;

        let err = realm_and_builder_task
            .realm_proxy
            .add_route(&capabilities, &from, &to)
            .await
            .expect("failed to call AddRoute")
            .expect_err("AddRoute succeeded unexpectedly");

        assert_eq!(err, expected_err);
    }

    fn create_valid_capability() -> ftest::Capability {
        ftest::Capability::Protocol(ftest::Protocol {
            name: Some("fuchsia.examples.Hippo".to_owned()),
            as_: None,
            type_: None,
            ..Default::default()
        })
    }

    #[fuchsia::test]
    async fn add_local_child_to_child_realm() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        let (child_realm_proxy, child_realm_server_end) = create_proxy::<ftest::RealmMarker>();
        realm_and_builder_task
            .realm_proxy
            .add_child_realm("a", &ftest::ChildOptions::default(), child_realm_server_end)
            .await
            .expect("failed to call add_child_realm")
            .expect("add_child_realm returned an error");
        child_realm_proxy
            .add_local_child("b", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let b_decl = ComponentDeclBuilder::new_empty_component()
            .program(cm_rust::ProgramDecl {
                runner: Some(crate::runner::RUNNER_NAME.parse().unwrap()),
                info: fdata::Dictionary {
                    entries: Some(vec![
                        fdata::DictionaryEntry {
                            key: runner::LOCAL_COMPONENT_ID_KEY.to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str("0".to_string()))),
                        },
                        fdata::DictionaryEntry {
                            key: ftest::LOCAL_COMPONENT_NAME_KEY.to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str("a/b".to_string()))),
                        },
                    ]),
                    ..Default::default()
                },
            })
            .build();
        let mut expected_tree = ComponentTree {
            decl: cm_rust::ComponentDecl::default(),
            children: vec![(
                "a".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree {
                    decl: cm_rust::ComponentDecl::default(),
                    children: vec![(
                        "b".parse().unwrap(),
                        ftest::ChildOptions::default(),
                        ComponentTree { decl: b_decl, children: vec![] },
                    )],
                },
            )],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
    }

    #[fuchsia::test]
    async fn replace_component_decl_immutable_program() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .replace_component_decl("a", &fcdecl::Component::default())
            .await
            .expect("failed to call replace_component_decl")
            .expect_err("replace_component_decl did not return an error");
        assert_eq!(err, ftest::RealmBuilderError::ImmutableProgram);
    }

    #[fuchsia::test]
    async fn replace_component_decl_for_nonexistent_child() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        let err = realm_and_builder_task
            .realm_proxy
            .replace_component_decl("a", &fcdecl::Component::default())
            .await
            .expect("failed to call replace_component_decl")
            .expect_err("replace_component_decl did not return an error");
        assert_eq!(err, ftest::RealmBuilderError::NoSuchChild);
    }

    #[fuchsia::test]
    async fn replace_component_decl_for_child_behind_child_decl() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child("a", "test:///a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let err = realm_and_builder_task
            .realm_proxy
            .replace_component_decl("a", &fcdecl::Component::default())
            .await
            .expect("failed to call replace_component_decl")
            .expect_err("replace_component_decl did not return an error");
        assert_eq!(err, ftest::RealmBuilderError::ChildDeclNotVisible);
    }

    #[fuchsia::test]
    async fn get_and_replace_realm_decl() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        let mut realm_decl = realm_and_builder_task
            .realm_proxy
            .get_realm_decl()
            .await
            .expect("failed to call get_realm_decl")
            .expect("get_realm_decl returned an error");
        realm_decl.children = Some(vec![fcdecl::Child {
            name: Some("example-child".to_string()),
            url: Some("example://url".to_string()),
            startup: Some(fcdecl::StartupMode::Eager),
            ..Default::default()
        }]);
        realm_and_builder_task
            .realm_proxy
            .replace_realm_decl(&realm_decl)
            .await
            .expect("failed to call replace_realm_decl")
            .expect("replace_realm_decl returned an error");
        assert_eq!(
            realm_decl,
            realm_and_builder_task
                .realm_proxy
                .get_realm_decl()
                .await
                .expect("failed to call get_realm_decl")
                .expect("get_realm_decl returned an error"),
        );
    }

    #[fuchsia::test]
    async fn replace_decl_enforces_validation() {
        let realm_and_builder_task = RealmAndBuilderTask::new();
        let realm_decl = fcdecl::Component {
            children: Some(vec![fcdecl::Child {
                name: Some("example-child".to_string()),
                url: Some("example://url".to_string()),
                startup: Some(fcdecl::StartupMode::Eager),
                environment: Some("i-dont-exist".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let err = realm_and_builder_task
            .realm_proxy
            .replace_realm_decl(&realm_decl)
            .await
            .expect("failed to call replace_realm_decl")
            .expect_err("replace_realm_decl did not return an error");
        assert_eq!(err, ftest::RealmBuilderError::InvalidComponentDecl);
    }

    #[fuchsia::test]
    async fn all_functions_error_after_build() {
        let mut rabt = RealmAndBuilderTask::new();
        let (child_realm_proxy, child_realm_server_end) = create_proxy::<ftest::RealmMarker>();
        rabt.realm_proxy
            .add_child_realm("a", &ftest::ChildOptions::default(), child_realm_server_end)
            .await
            .expect("failed to call add_child_realm")
            .expect("add_child_realm returned an error");
        child_realm_proxy
            .add_local_child("b", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        let _tree_from_resolver = rabt.call_build_and_get_tree().await;

        async fn assert_err<V: std::fmt::Debug>(
            fut: impl futures::Future<Output = Result<Result<V, ftest::RealmBuilderError>, fidl::Error>>,
        ) {
            assert_eq!(
                ftest::RealmBuilderError::BuildAlreadyCalled,
                fut.await.expect("failed to call function").expect_err("expected an error"),
            );
        }
        let empty_opts = || ftest::ChildOptions::default();
        let empty_decl = || fcdecl::Component::default();

        assert_err(rabt.realm_proxy.add_child("a", "test:///a", &empty_opts())).await;
        assert_err(rabt.realm_proxy.add_child_from_decl("a", &empty_decl(), &empty_opts())).await;
        assert_err(rabt.realm_proxy.add_local_child("a", &empty_opts())).await;
        let (_child_realm_proxy, server_end) = create_proxy::<ftest::RealmMarker>();
        assert_err(rabt.realm_proxy.add_child_realm("a", &empty_opts(), server_end)).await;
        assert_err(rabt.realm_proxy.get_component_decl("b")).await;
        assert_err(rabt.realm_proxy.replace_component_decl("b", &empty_decl())).await;
        assert_err(rabt.realm_proxy.replace_realm_decl(&empty_decl())).await;
        assert_err(rabt.realm_proxy.add_route(&[], &fcdecl::Ref::Self_(fcdecl::SelfRef {}), &[]))
            .await;

        assert_err(child_realm_proxy.add_child("a", "test:///a", &empty_opts())).await;
        assert_err(child_realm_proxy.add_child_from_decl("a", &empty_decl(), &empty_opts())).await;
        assert_err(child_realm_proxy.add_local_child("a", &empty_opts())).await;
        let (_child_realm_proxy, server_end) = create_proxy::<ftest::RealmMarker>();
        assert_err(child_realm_proxy.add_child_realm("a", &empty_opts(), server_end)).await;
        assert_err(child_realm_proxy.get_component_decl("b")).await;
        assert_err(child_realm_proxy.replace_component_decl("b", &empty_decl())).await;
        assert_err(child_realm_proxy.replace_realm_decl(&empty_decl())).await;
        assert_err(child_realm_proxy.add_route(&[], &fcdecl::Ref::Self_(fcdecl::SelfRef {}), &[]))
            .await;
    }

    #[fuchsia::test]
    async fn read_only_directory() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_child("a", "test:///a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_child")
            .expect("add_child returned an error");
        realm_and_builder_task
            .realm_proxy
            .read_only_directory(
                "data",
                &[fcdecl::Ref::Child(fcdecl::ChildRef {
                    name: "a".parse().unwrap(),
                    collection: None,
                })],
                ftest::DirectoryContents {
                    entries: vec![ftest::DirectoryEntry {
                        file_path: "hippos".to_string(),
                        file_contents: {
                            let value = "rule!";
                            let vmo =
                                zx::Vmo::create(value.len() as u64).expect("failed to create vmo");
                            vmo.write(value.as_bytes(), 0).expect("failed to write to vmo");
                            fmem::Buffer { vmo, size: value.len() as u64 }
                        },
                    }],
                },
            )
            .await
            .expect("failed to call read_only_directory")
            .expect("read_only_directory returned an error");
        let tree_from_resolver = realm_and_builder_task.call_build_and_get_tree().await;
        let read_only_dir_decl = ComponentDeclBuilder::new_empty_component()
            .program(cm_rust::ProgramDecl {
                runner: Some(crate::runner::RUNNER_NAME.parse().unwrap()),
                info: fdata::Dictionary {
                    entries: Some(vec![fdata::DictionaryEntry {
                        key: runner::LOCAL_COMPONENT_ID_KEY.to_string(),
                        value: Some(Box::new(fdata::DictionaryValue::Str("0".to_string()))),
                    }]),
                    ..Default::default()
                },
            })
            .capability(CapabilityBuilder::directory().name("data").path("/data"))
            .expose(
                ExposeBuilder::directory()
                    .name("data")
                    .source(cm_rust::ExposeSource::Self_)
                    .rights(fio::R_STAR_DIR),
            )
            .build();
        let mut expected_tree = ComponentTree {
            decl: ComponentDeclBuilder::new_empty_component()
                .child_default("a")
                .offer(
                    OfferBuilder::directory()
                        .name("data")
                        .source_static_child("read-only-directory-0")
                        .target_static_child("a")
                        .rights(fio::R_STAR_DIR),
                )
                .build(),
            children: vec![(
                "read-only-directory-0".parse().unwrap(),
                ftest::ChildOptions::default(),
                ComponentTree { decl: read_only_dir_decl, children: vec![] },
            )],
        };
        expected_tree.add_auto_decls();
        assert_decls_eq!(tree_from_resolver, expected_tree);
        assert!(
            realm_and_builder_task
                .runner
                .local_component_proxies()
                .await
                .contains_key(&"0".to_string())
        );
    }

    // Test the code paths `load_fragment_only_url` and `load_absolute_url` correctly read the abi revision
    // from the test package. This uses `launch_builder_task` to pass in the test's package directory.
    #[fuchsia::test]
    async fn test_rb_pkg_abi_revision() {
        let mut realm_and_builder_task = RealmAndBuilderTask::new();
        realm_and_builder_task
            .realm_proxy
            .add_local_child("a", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_local_child")
            .expect("add_child_local returned an error");
        realm_and_builder_task
            .realm_proxy
            .add_local_child("b", &ftest::ChildOptions::default())
            .await
            .expect("failed to call add_local_child")
            .expect("add_local_child returned an error");

        realm_and_builder_task.call_build().await.expect("failed to build realm");
        let registry = realm_and_builder_task.registry;

        // Exercise the code path for `resolver::load_fragment_only_url()`
        // Note: resolve expects a fully qualified URL to pass to `load_fragment_only_url` which will
        // load component 'a' using its url fragment (#meta/a.cm) at the relative path meta/a.cml.
        // Please see comment in `resolver::load_fragment_only_url()` for more information.
        let res = registry.resolve("realm-builder://0/a#meta/a.cm").await.unwrap();
        let abi_revision = res.abi_revision.expect("abi revision should be set in test package");
        version_history_data::HISTORY
            .check_abi_revision_for_runtime(abi_revision.into())
            .expect("ABI revision should be supported");

        // Exercise the code path for `resolver::load_absolute_url()`
        // load component 'b' identified by its absolute path
        let res = registry.resolve("realm-builder://1/b").await.unwrap();
        let abi_revision = res.abi_revision.expect("abi revision should be set in test package");
        version_history_data::HISTORY
            .check_abi_revision_for_runtime(abi_revision.into())
            .expect("ABI revision should be supported");
    }

    // TODO(https://fxbug.dev/42169660): The following test is impossible to write until sub-realms are supported
    // #[fuchsia::test]
    // async fn replace_component_decl_where_decl_children_conflict_with_mutable_children() {
    // }
}
