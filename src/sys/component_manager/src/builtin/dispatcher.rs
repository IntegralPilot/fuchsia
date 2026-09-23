// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, format_err};
use cm_rust::CapabilityTypeName;
use cm_types::NamespacePath;
use diagnostics_log::{Publisher, PublisherOptions};
use dispatcher_config::Config;
use fidl::endpoints::{DiscoverableProtocolMarker, ServerEnd};
use fuchsia_component::client::connect::connect_to_protocol_at_dir_root;
use fuchsia_component::directory::AsRefDirectory;
use fuchsia_component::{runtime, server};
use futures::{FutureExt, StreamExt};
use log::Log;
use namespace::Namespace;
use std::str::FromStr;
use std::sync::Arc;
use vfs::ExecutionScope;
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl,
    fidl_fuchsia_component_internal as finternal, fidl_fuchsia_component_runtime as fruntime,
    fidl_fuchsia_io as fio,
};

#[derive(Clone)]
struct Logger(Option<Publisher>);

impl Logger {
    fn log(&self, error: Error) {
        let mut builder = log::Record::builder();
        builder.level(log::Level::Warn);

        let log_it = |record| {
            if let Some(p) = &self.0 {
                p.log(record);
            } else {
                log::logger().log(record);
            }
        };

        log_it(&builder.args(format_args!("failed to run dispatcher component: {error}")).build());
    }
}

/// The dispatcher is a built-in component that expects configuration capabilities giving it a
/// capability type, capability name, and component URL. It will expose a dictionary named "output"
/// that contains a capability of a given name and type. Any route requests that go to that
/// capability will cause the dispatcher to create a new dynamic child and forward the request to
/// the child under the same capability name. The new dynamic child will be offered everything that
/// is offered to the dispatcher, and the dispatcher will destroy the child when it stops running.
pub struct Dispatcher {
    realm_proxy: fcomponent::RealmProxy,
    capabilities_proxy: fruntime::CapabilitiesProxy,
    sandbox_retriever_proxy: finternal::ComponentSandboxRetrieverProxy,
    config: Arc<Config>,
    scope: ExecutionScope,
}

impl Dispatcher {
    pub async fn run(
        mut namespace: Namespace,
        outgoing_dir: ServerEnd<fio::DirectoryMarker>,
        config: Option<zx::Vmo>,
    ) {
        let Some(svc_dir) = namespace.remove(&NamespacePath::new("/svc").unwrap()) else {
            log::error!("dispatcher is missing svc dir");
            return;
        };
        let svc_dir_proxy = svc_dir.into_proxy();
        let logger = Logger(
            get_logger(&svc_dir_proxy)
                .await
                .inspect_err(|error| {
                    log::warn!(error:?; "unable to get logger for dispatcher component");
                })
                .ok(),
        );
        if let Err(err) = Self::run_inner(svc_dir_proxy, outgoing_dir, config, logger.clone()).await
        {
            logger.log(err);
        }
    }

    async fn run_inner(
        svc_dir_proxy: fio::DirectoryProxy,
        outgoing_dir: ServerEnd<fio::DirectoryMarker>,
        config: Option<zx::Vmo>,
        logger: Logger,
    ) -> Result<(), anyhow::Error> {
        let (realm_proxy, realm_server_end) =
            fidl::endpoints::create_proxy::<fcomponent::RealmMarker>();
        svc_dir_proxy
            .as_ref_directory()
            .open(
                fcomponent::RealmMarker::PROTOCOL_NAME,
                fio::Flags::PROTOCOL_SERVICE,
                realm_server_end.into_channel().into(),
            )
            .context("failed to open protocol")?;
        let (capabilities_proxy, capabilities_server_end) =
            fidl::endpoints::create_proxy::<fruntime::CapabilitiesMarker>();
        svc_dir_proxy
            .as_ref_directory()
            .open(
                fruntime::CapabilitiesMarker::PROTOCOL_NAME,
                fio::Flags::PROTOCOL_SERVICE,
                capabilities_server_end.into_channel().into(),
            )
            .context("failed to open protocol")?;
        let (sandbox_retriever_proxy, sandbox_retriever_server_end) =
            fidl::endpoints::create_proxy::<finternal::ComponentSandboxRetrieverMarker>();
        svc_dir_proxy
            .as_ref_directory()
            .open(
                finternal::ComponentSandboxRetrieverMarker::PROTOCOL_NAME,
                fio::Flags::PROTOCOL_SERVICE,
                sandbox_retriever_server_end.into_channel().into(),
            )
            .context("failed to open protocol")?;

        let config = Arc::new(
            Config::from_vmo(&config.context("dispatcher is missing config vmo")?)
                .context("failed to parse config")?,
        );

        let self_ = Arc::new(Self {
            realm_proxy,
            capabilities_proxy,
            sandbox_retriever_proxy,
            config,
            scope: ExecutionScope::new(),
        });

        let mut fs = server::ServiceFs::new();
        fs.dir("svc").add_fidl_service(move |stream: fruntime::DictionaryRouterRequestStream| {
            let self_clone = self_.clone();
            let logger = logger.clone();
            let stream = runtime::DictionaryRouterReceiver { stream };
            self_.scope.spawn(async move {
                if let Err(err) = self_clone.handle_router_stream(stream).await {
                    logger.log(err);
                }
            });
        });
        fs.serve_connection(outgoing_dir).context("failed to serve outgoing directory")?;
        fs.collect::<()>().await;
        Ok(())
    }

    async fn handle_router_stream(
        self: Arc<Self>,
        mut stream: runtime::DictionaryRouterReceiver,
    ) -> Result<(), anyhow::Error> {
        let type_to_dispatch = CapabilityTypeName::from_str(&self.config.type_to_dispatch)
            .map_err(|_| {
                format_err!("unrecognized capability type {:?}", self.config.type_to_dispatch)
            })?;

        let dictionary = runtime::Dictionary::new_with_proxy(self.capabilities_proxy.clone()).await;
        let router = self.clone().create_router(type_to_dispatch).await;
        dictionary.insert(&self.config.what_to_dispatch, router).await;
        while let Some((_request, _instance_token, event_pair, responder)) = stream.next().await {
            dictionary.associate_with_handle(event_pair).await;
            let _ = responder.send(Ok(fruntime::RouterResponse::Success));
        }
        Ok(())
    }

    async fn create_router(
        self: Arc<Self>,
        type_to_dispatch: CapabilityTypeName,
    ) -> runtime::Capability {
        let self_clone = self.clone();
        match type_to_dispatch {
            CapabilityTypeName::EventStream
            | CapabilityTypeName::Resolver
            | CapabilityTypeName::Runner
            | CapabilityTypeName::Protocol => {
                let (router, receiver) =
                    runtime::ConnectorRouter::new_with_proxy(self.capabilities_proxy.clone()).await;
                self.scope.spawn(receiver.handle_with(move |request, instance_token| {
                    let self_clone = self_clone.clone();
                    async move {
                        let child_output = self_clone
                            .launch_child_and_get_output()
                            .await
                            .map_err(|_| zx::Status::NOT_FOUND)?;
                        let Some(runtime::Capability::ConnectorRouter(router)) =
                            child_output.get(&self_clone.config.what_to_dispatch).await
                        else {
                            return Err(zx::Status::NOT_FOUND);
                        };
                        router.route(request, &instance_token).await
                    }
                    .boxed()
                }));
                router.into()
            }
            CapabilityTypeName::Directory
            | CapabilityTypeName::Storage
            | CapabilityTypeName::Service => {
                let (router, receiver) =
                    runtime::DirConnectorRouter::new_with_proxy(self.capabilities_proxy.clone())
                        .await;
                self.scope.spawn(receiver.handle_with(move |request, instance_token| {
                    let self_clone = self_clone.clone();
                    async move {
                        let child_output = self_clone
                            .launch_child_and_get_output()
                            .await
                            .map_err(|_| zx::Status::NOT_FOUND)?;
                        let Some(runtime::Capability::DirConnectorRouter(router)) =
                            child_output.get(&self_clone.config.what_to_dispatch).await
                        else {
                            return Err(zx::Status::NOT_FOUND);
                        };
                        router.route(request, &instance_token).await
                    }
                    .boxed()
                }));
                router.into()
            }
            CapabilityTypeName::Dictionary => {
                let (router, receiver) =
                    runtime::DictionaryRouter::new_with_proxy(self.capabilities_proxy.clone())
                        .await;
                self.scope.spawn(receiver.handle_with(move |request, instance_token| {
                    let self_clone = self_clone.clone();
                    async move {
                        let child_output = self_clone
                            .launch_child_and_get_output()
                            .await
                            .map_err(|_| zx::Status::NOT_FOUND)?;
                        let Some(runtime::Capability::DictionaryRouter(router)) =
                            child_output.get(&self_clone.config.what_to_dispatch).await
                        else {
                            return Err(zx::Status::NOT_FOUND);
                        };
                        router.route(request, &instance_token).await
                    }
                    .boxed()
                }));
                router.into()
            }
            CapabilityTypeName::Config => {
                let (router, receiver) =
                    runtime::DataRouter::new_with_proxy(self.capabilities_proxy.clone()).await;
                self.scope.spawn(receiver.handle_with(move |request, instance_token| {
                    let self_clone = self_clone.clone();
                    async move {
                        let child_output = self_clone
                            .launch_child_and_get_output()
                            .await
                            .map_err(|_| zx::Status::NOT_FOUND)?;
                        let Some(runtime::Capability::DataRouter(router)) =
                            child_output.get(&self_clone.config.what_to_dispatch).await
                        else {
                            return Err(zx::Status::NOT_FOUND);
                        };
                        router.route(request, &instance_token).await
                    }
                    .boxed()
                }));
                router.into()
            }
        }
    }

    async fn launch_child_and_get_output(&self) -> Result<runtime::Dictionary, anyhow::Error> {
        let component_input_capabilities = self.get_component_input_capabilities().await?;
        let child_name = format!("worker-{:x}", rand::random::<u64>());
        let (controller_proxy, controller_server_end) =
            fidl::endpoints::create_proxy::<fcomponent::ControllerMarker>();
        self.realm_proxy
            .create_child(
                &fdecl::CollectionRef { name: "workers".to_string() },
                &fdecl::Child {
                    name: Some(child_name.clone()),
                    url: Some(self.config.who_to_dispatch_to.clone()),
                    startup: Some(fdecl::StartupMode::Lazy),
                    ..Default::default()
                },
                fcomponent::CreateChildArgs {
                    controller: Some(controller_server_end),
                    additional_inputs: Some(component_input_capabilities),
                    ..Default::default()
                },
            )
            .await
            .context("FIDL error talking to ourselves")?
            .map_err(|e| format_err!("failed to create child: {e:?}"))?;
        self.spawn_wait_for_exit(controller_proxy).await;
        let child_output_handle = self
            .realm_proxy
            .get_child_output_dictionary(&fdecl::ChildRef {
                name: child_name.clone(),
                collection: Some("workers".to_string()),
            })
            .await
            .context("FIDL error talking to ourselves")?
            .map_err(|e| format_err!("failed to get output dictionary: {e:?}"))?;
        Ok(runtime::Dictionary {
            handle: child_output_handle,
            capabilities_proxy: self.capabilities_proxy.clone(),
        })
    }

    async fn get_component_input_capabilities(&self) -> Result<zx::EventPair, anyhow::Error> {
        let sandbox = self
            .sandbox_retriever_proxy
            .get_my_sandbox()
            .await
            .context("failed to use framework protocol from built-in component")?;
        let component_input = runtime::Dictionary {
            handle: sandbox.component_input.expect("missing component input"),
            capabilities_proxy: self.capabilities_proxy.clone(),
        };
        let child_cap = component_input.get("parent").await.unwrap();
        let child = runtime::Dictionary::try_from(child_cap).unwrap();
        child.remove("diagnostics").await;
        Ok(child.handle)
    }

    async fn spawn_wait_for_exit(&self, controller_proxy: fcomponent::ControllerProxy) {
        let (execution_controller_proxy, execution_controller_server_end) =
            fidl::endpoints::create_proxy::<fcomponent::ExecutionControllerMarker>();
        controller_proxy
            .start(fcomponent::StartChildArgs::default(), execution_controller_server_end)
            .await
            .expect("FIDL error talking to ourselves")
            .expect("failed to start child");
        self.scope.spawn(async move {
            let mut event_stream = execution_controller_proxy.take_event_stream();
            match event_stream.next().await {
                Some(Ok(fcomponent::ExecutionControllerEvent::OnStop { .. })) => {
                    controller_proxy
                        .destroy()
                        .await
                        .expect("FIDL error talking to ourselves")
                        .expect("failed to destroy child");
                }
                event => panic!("unexpected execution controller event: {:?}", event),
            }
        });
    }
}

async fn get_logger(svc_dir: &fio::DirectoryProxy) -> Result<Publisher, Error> {
    let log_sink_client = connect_to_protocol_at_dir_root(svc_dir)?;
    Ok(Publisher::new_async(PublisherOptions::default().use_log_sink(log_sink_client)).await?)
}
