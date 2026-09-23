// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use fidl::endpoints::{
    ClientEnd, DiscoverableProtocolMarker, RequestStream, ServerEnd, ServiceMarker, ServiceProxy,
    create_request_stream,
};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_component_test as ftest;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fprocess;
use fuchsia_async as fasync;
use fuchsia_component::DEFAULT_SERVICE_INSTANCE;
use fuchsia_component::client::Connect;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use futures::lock::Mutex;
use futures::{FutureExt, TryStreamExt, select};
use log::*;
use runner::get_value as get_dictionary_value;
use std::collections::HashMap;
use std::sync::Arc;
use vfs::execution_scope::ExecutionScope;

/// The handles from the framework over which the local component should interact with other
/// components.
pub struct LocalComponentHandles {
    namespace: HashMap<String, fio::DirectoryProxy>,
    numbered_handles: HashMap<u32, zx::NullableHandle>,

    stop_notifier: Arc<Mutex<Option<oneshot::Sender<()>>>>,

    /// The outgoing directory handle for a local component. This can be used to run a ServiceFs
    /// for the component.
    pub outgoing_dir: ServerEnd<fio::DirectoryMarker>,
}

impl LocalComponentHandles {
    fn new(
        fidl_namespace: Vec<fcrunner::ComponentNamespaceEntry>,
        fidl_numbered_handles: Vec<fprocess::HandleInfo>,
        outgoing_dir: ServerEnd<fio::DirectoryMarker>,
    ) -> Result<(Self, Arc<Mutex<Option<oneshot::Sender<()>>>>), Error> {
        let stop_notifier = Arc::new(Mutex::new(None));
        let mut namespace = HashMap::new();
        for namespace_entry in fidl_namespace {
            namespace.insert(
                namespace_entry.path.ok_or_else(|| format_err!("namespace entry missing path"))?,
                namespace_entry
                    .directory
                    .ok_or_else(|| format_err!("namespace entry missing directory handle"))?
                    .into_proxy(),
            );
        }
        let numbered_handles =
            fidl_numbered_handles.into_iter().map(|h| (h.id, h.handle)).collect::<HashMap<_, _>>();
        Ok((
            Self {
                namespace,
                numbered_handles,
                outgoing_dir,
                stop_notifier: stop_notifier.clone(),
            },
            stop_notifier,
        ))
    }

    pub fn take_numbered_handle(&mut self, id: u32) -> Option<zx::NullableHandle> {
        self.numbered_handles.remove(&id)
    }

    pub fn numbered_handles(&self) -> &HashMap<u32, zx::NullableHandle> {
        &self.numbered_handles
    }

    /// Registers a new stop notifier for this component. If this function is called, then realm
    /// builder will deliver a message on the returned oneshot when component manager asks for this
    /// component to stop. It is then this component's responsibility to exit. If it takes too long
    /// to exit (the default is 5 seconds) then it will be killed.
    ///
    /// If this function is not called, then the component is immediately killed when component
    /// manager asks for it to be stopped. Killing the component is performed by dropping the
    /// underlying future, effectively cancelling all pending work.
    ///
    /// If this function is called more than once on a single local component, then it will panic.
    pub async fn register_stop_notifier(&self) -> oneshot::Receiver<()> {
        let mut stop_notifier_guard = self.stop_notifier.lock().await;
        if stop_notifier_guard.is_some() {
            panic!("cannot register multiple stop handlers for a single local component");
        }
        let (sender, receiver) = oneshot::channel();
        *stop_notifier_guard = Some(sender);
        receiver
    }

    /// Connects to a FIDL protocol and returns a proxy to that protocol.
    pub fn connect_to_protocol<T: Connect>(&self) -> Result<T, Error> {
        self.connect_to_named_protocol(T::Protocol::PROTOCOL_NAME)
    }

    /// Connects to a FIDL protocol with the given name and returns a proxy to that protocol.
    pub fn connect_to_named_protocol<T: Connect>(&self, name: &str) -> Result<T, Error> {
        let svc_dir_proxy = self.namespace.get("/svc").ok_or_else(|| {
            format_err!("the component's namespace doesn't have a /svc directory")
        })?;
        T::connect_at_dir_root_with_name(svc_dir_proxy, name)
    }

    /// Opens a FIDL service as a directory, which holds instances of the service.
    pub fn open_service<S: ServiceMarker>(&self) -> Result<fio::DirectoryProxy, Error> {
        self.open_named_service(S::SERVICE_NAME)
    }

    /// Opens a FIDL service with the given name as a directory, which holds instances of the
    /// service.
    pub fn open_named_service(&self, name: &str) -> Result<fio::DirectoryProxy, Error> {
        let svc_dir_proxy = self.namespace.get("/svc").ok_or_else(|| {
            format_err!("the component's namespace doesn't have a /svc directory")
        })?;
        fuchsia_fs::directory::open_directory_async(&svc_dir_proxy, name, fio::PERM_READABLE)
            .map_err(Into::into)
    }

    /// Connect to the "default" instance of a FIDL service in the `/svc` directory of
    /// the local component's root namespace.
    pub fn connect_to_service<S: ServiceMarker>(&self) -> Result<S::Proxy, Error> {
        self.connect_to_service_instance::<S>(DEFAULT_SERVICE_INSTANCE)
    }

    /// Connect to an instance of a FIDL service in the `/svc` directory of
    /// the local components's root namespace.
    /// `instance` is a path of one or more components.
    pub fn connect_to_service_instance<S: ServiceMarker>(
        &self,
        instance_name: &str,
    ) -> Result<S::Proxy, Error> {
        self.connect_to_named_service_instance::<S>(S::SERVICE_NAME, instance_name)
    }

    /// Connect to an instance of a FIDL service with the given name in the `/svc` directory of
    /// the local components's root namespace.
    /// `instance` is a path of one or more components.
    pub fn connect_to_named_service_instance<S: ServiceMarker>(
        &self,
        service_name: &str,
        instance_name: &str,
    ) -> Result<S::Proxy, Error> {
        let service_dir = self.open_named_service(service_name)?;
        let directory_proxy = fuchsia_fs::directory::open_directory_async(
            &service_dir,
            instance_name,
            fio::PERM_READABLE,
        )?;
        Ok(S::Proxy::from_member_opener(Box::new(
            fuchsia_component::client::ServiceInstanceDirectory(
                directory_proxy,
                instance_name.to_owned(),
            ),
        )))
    }

    /// Clones a directory from the local component's namespace.
    ///
    /// Note that this function only works on exact matches from the namespace. For example if the
    /// namespace had a `data` entry in it, and the caller wished to open the subdirectory at
    /// `data/assets`, then this function should be called with the argument `data` and the
    /// returned `DirectoryProxy` would then be used to open the subdirectory `assets`. In this
    /// scenario, passing `data/assets` in its entirety to this function would fail.
    ///
    /// ```
    /// let data_dir = handles.clone_from_namespace("data")?;
    /// let assets_dir = fuchsia_fs::directory::open_directory_async(&data_dir, "assets", ...)?;
    /// ```
    pub fn clone_from_namespace(&self, directory_name: &str) -> Result<fio::DirectoryProxy, Error> {
        let dir_proxy = self.namespace.get(&format!("/{}", directory_name)).ok_or_else(|| {
            format_err!(
                "the local component's namespace doesn't have a /{} directory",
                directory_name
            )
        })?;
        fuchsia_fs::directory::clone(&dir_proxy).context("clone")
    }
}

type LocalComponentImplementations = HashMap<
    String,
    Arc<
        dyn Fn(LocalComponentHandles) -> BoxFuture<'static, Result<(), Error>>
            + Sync
            + Send
            + 'static,
    >,
>;

#[derive(Clone, Debug)]
pub struct LocalComponentRunnerBuilder {
    local_component_implementations: Arc<Mutex<Option<LocalComponentImplementations>>>,
}

impl LocalComponentRunnerBuilder {
    pub fn new() -> Self {
        Self { local_component_implementations: Arc::new(Mutex::new(Some(HashMap::new()))) }
    }

    pub(crate) async fn register_local_component<I>(
        &self,
        name: String,
        implementation: I,
    ) -> Result<(), ftest::RealmBuilderError>
    where
        I: Fn(LocalComponentHandles) -> BoxFuture<'static, Result<(), Error>>
            + Sync
            + Send
            + 'static,
    {
        self.local_component_implementations
            .lock()
            .await
            .as_mut()
            .ok_or(ftest::RealmBuilderError::BuildAlreadyCalled)?
            .insert(name, Arc::new(implementation));
        Ok(())
    }

    pub(crate) async fn build(
        self,
    ) -> Result<
        (ClientEnd<fcrunner::ComponentRunnerMarker>, fasync::Task<()>),
        ftest::RealmBuilderError,
    > {
        let local_component_implementations = self
            .local_component_implementations
            .lock()
            .await
            .take()
            .ok_or(ftest::RealmBuilderError::BuildAlreadyCalled)?;
        let (runner_client_end, runner_request_stream) =
            create_request_stream::<fcrunner::ComponentRunnerMarker>();
        let runner = LocalComponentRunner::new(local_component_implementations);
        let runner_task = fasync::Task::spawn(async move {
            if let Err(e) = runner.handle_stream(runner_request_stream).await {
                error!("failed to run local component runner: {:?}", e);
            }
        });

        Ok((runner_client_end, runner_task))
    }
}

pub struct LocalComponentRunner {
    execution_scope: ExecutionScope,
    local_component_implementations: HashMap<
        String,
        Arc<
            dyn Fn(LocalComponentHandles) -> BoxFuture<'static, Result<(), Error>>
                + Sync
                + Send
                + 'static,
        >,
    >,
}

impl Drop for LocalComponentRunner {
    fn drop(&mut self) {
        self.execution_scope.shutdown();
    }
}

impl LocalComponentRunner {
    fn new(
        local_component_implementations: HashMap<
            String,
            Arc<
                dyn Fn(LocalComponentHandles) -> BoxFuture<'static, Result<(), Error>>
                    + Sync
                    + Send
                    + 'static,
            >,
        >,
    ) -> Self {
        Self { local_component_implementations, execution_scope: ExecutionScope::new() }
    }

    async fn handle_stream(
        &self,
        mut runner_request_stream: fcrunner::ComponentRunnerRequestStream,
    ) -> Result<(), Error> {
        while let Some(req) = runner_request_stream.try_next().await? {
            match req {
                fcrunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                    let program = start_info
                        .program
                        .ok_or_else(|| format_err!("program is missing from start_info"))?;
                    let namespace = start_info
                        .ns
                        .ok_or_else(|| format_err!("namespace is missing from start_info"))?;
                    let numbered_handles = start_info.numbered_handles.unwrap_or_default();
                    let outgoing_dir = start_info
                        .outgoing_dir
                        .ok_or_else(|| format_err!("outgoing_dir is missing from start_info"))?;
                    let _runtime_dir_server_end: ServerEnd<fio::DirectoryMarker> = start_info
                        .runtime_dir
                        .ok_or_else(|| format_err!("runtime_dir is missing from start_info"))?;

                    let local_component_name = extract_local_component_name(program)?;
                    let local_component_implementation = self
                        .local_component_implementations
                        .get(&local_component_name)
                        .ok_or_else(|| {
                            format_err!("no such local component: {:?}", local_component_name)
                        })?
                        .clone();
                    let (component_handles, stop_notifier) =
                        LocalComponentHandles::new(namespace, numbered_handles, outgoing_dir)?;

                    let mut controller_request_stream = controller.into_stream();
                    self.execution_scope.spawn(async move {
                        let mut local_component_implementation_fut =
                            (*local_component_implementation)(component_handles).fuse();
                        let controller_control_handle = controller_request_stream.control_handle();
                        let mut controller_request_fut =
                            controller_request_stream.try_next().fuse();
                        loop {
                            select! {
                                res = local_component_implementation_fut => {
                                    match res {
                                        Err(e) => {
                                            error!(
                                                "the local component {:?} returned an error: {:?}",
                                                local_component_name,
                                                e,
                                            );
                                            let epitaph = zx::Status::try_from_raw(fcomponent::Error::InstanceDied.into_primitive() as i32).unwrap();
                                            controller_control_handle.shutdown_with_epitaph(Err(epitaph));
                                        }
                                        Ok(()) => controller_control_handle.shutdown_with_epitaph(Ok(())),
                                    };
                                    return;
                                }
                                req_res = controller_request_fut => {
                                    match req_res.expect("invalid controller request") {
                                        Some(fcrunner::ComponentControllerRequest::Stop { .. }) => {
                                            if let Some(stop_notifier) =
                                                stop_notifier.lock().await.take()
                                            {
                                                // If the local component happened to exit the same
                                                // moment that the component controller stop
                                                // request was received, then the receiver is
                                                // already dropped. Let's ignore any errors about
                                                // sending this.
                                                let _ = stop_notifier.send(());

                                                // Repopulate the `controller_request_fut` field so
                                                // that we'll be able to see the `Kill` request.
                                                controller_request_fut = controller_request_stream.try_next().fuse();
                                            } else {
                                                controller_control_handle.shutdown_with_epitaph(
                                                    Err(zx::Status::try_from_raw(fcomponent::Error::InstanceDied.into_primitive() as i32).unwrap()),
                                                );
                                                return;
                                            }
                                        }
                                        Some(fcrunner::ComponentControllerRequest::Kill { .. }) => {
                                            controller_control_handle.shutdown_with_epitaph(
                                                Err(zx::Status::try_from_raw(fcomponent::Error::InstanceDied.into_primitive() as i32).unwrap()),
                                            );
                                            return;
                                        }
                                        _ => return,
                                    }
                                }
                            };
                        }
                    });
                }
                fcrunner::ComponentRunnerRequest::_UnknownMethod { ordinal, .. } => {
                    warn!(ordinal:%; "Unknown ComponentController request");
                }
            }
        }
        Ok(())
    }
}

fn extract_local_component_name(dict: fdata::Dictionary) -> Result<String, Error> {
    let entry_value = get_dictionary_value(&dict, ftest::LOCAL_COMPONENT_NAME_KEY)
        .ok_or_else(|| format_err!("program section is missing component name"))?;
    if let fdata::DictionaryValue::Str(s) = entry_value {
        return Ok(s.clone());
    } else {
        return Err(format_err!("malformed program section"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::{Proxy as _, create_proxy};
    use futures::future::pending;
    use zx::AsHandleRef;

    #[fuchsia::test]
    async fn runner_builder_correctly_stores_a_function() {
        let runner_builder = LocalComponentRunnerBuilder::new();
        let (sender, receiver) = oneshot::channel();
        let sender = Arc::new(Mutex::new(Some(sender)));

        let component_name = "test".to_string();

        runner_builder
            .register_local_component(component_name.clone(), move |_handles| {
                let sender = sender.clone();
                async move {
                    let sender = sender.lock().await.take().expect("local component invoked twice");
                    sender.send(()).expect("failed to send");
                    Ok(())
                }
                .boxed()
            })
            .await
            .unwrap();

        let (_, outgoing_dir) = create_proxy();
        let handles = LocalComponentHandles {
            namespace: HashMap::new(),
            numbered_handles: HashMap::new(),
            outgoing_dir,
            stop_notifier: Arc::new(Mutex::new(None)),
        };
        let local_component_implementation = runner_builder
            .local_component_implementations
            .lock()
            .await
            .as_ref()
            .unwrap()
            .get(&component_name)
            .expect("local component missing from runner builder")
            .clone();

        (*local_component_implementation)(handles)
            .await
            .expect("local component implementation failed");
        let () = receiver.await.expect("failed to receive");
    }

    struct RunnerAndHandles {
        _runner_task: fasync::Task<()>,
        _component_runner_proxy: fcrunner::ComponentRunnerProxy,
        _runtime_dir_proxy: fio::DirectoryProxy,
        outgoing_dir_proxy: fio::DirectoryProxy,
        controller_proxy: fcrunner::ComponentControllerProxy,
    }

    async fn build_and_start(
        runner_builder: LocalComponentRunnerBuilder,
        component_to_start: String,
    ) -> RunnerAndHandles {
        let (component_runner_client_end, runner_task) = runner_builder.build().await.unwrap();
        let component_runner_proxy = component_runner_client_end.into_proxy();

        let (runtime_dir_proxy, runtime_dir_server_end) = create_proxy();
        let (outgoing_dir_proxy, outgoing_dir_server_end) = create_proxy();
        let (controller_proxy, controller_server_end) = create_proxy();
        component_runner_proxy
            .start(
                fcrunner::ComponentStartInfo {
                    resolved_url: Some("test://test".to_string()),
                    program: Some(fdata::Dictionary {
                        entries: Some(vec![fdata::DictionaryEntry {
                            key: ftest::LOCAL_COMPONENT_NAME_KEY.to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str(component_to_start))),
                        }]),
                        ..Default::default()
                    }),
                    ns: Some(vec![]),
                    outgoing_dir: Some(outgoing_dir_server_end),
                    runtime_dir: Some(runtime_dir_server_end),
                    numbered_handles: Some(vec![]),
                    ..Default::default()
                },
                controller_server_end,
            )
            .expect("failed to send start");

        RunnerAndHandles {
            _runner_task: runner_task,
            _component_runner_proxy: component_runner_proxy,
            _runtime_dir_proxy: runtime_dir_proxy,
            outgoing_dir_proxy,
            controller_proxy,
        }
    }

    #[fuchsia::test]
    async fn the_runner_runs_a_component() {
        let runner_builder = LocalComponentRunnerBuilder::new();
        let (sender, receiver) = oneshot::channel();
        let sender = Arc::new(Mutex::new(Some(sender)));

        let component_name = "test".to_string();

        runner_builder
            .register_local_component(component_name.clone(), move |_handles| {
                let sender = sender.clone();
                async move {
                    let sender = sender.lock().await.take().expect("local component invoked twice");
                    sender.send(()).expect("failed to send");
                    Ok(())
                }
                .boxed()
            })
            .await
            .unwrap();

        let _runner_and_handles = build_and_start(runner_builder, component_name).await;

        let () = receiver.await.expect("failed to receive");
    }

    #[fuchsia::test]
    async fn the_runner_gives_the_component_its_outgoing_dir() {
        let runner_builder = LocalComponentRunnerBuilder::new();
        let (sender, receiver) = oneshot::channel::<ServerEnd<fio::DirectoryMarker>>();
        let sender = Arc::new(Mutex::new(Some(sender)));

        let component_name = "test".to_string();

        runner_builder
            .register_local_component(component_name.clone(), move |handles| {
                let sender = sender.clone();
                async move {
                    let _ = &handles;
                    sender
                        .lock()
                        .await
                        .take()
                        .expect("local component invoked twice")
                        .send(handles.outgoing_dir)
                        .expect("failed to send");
                    Ok(())
                }
                .boxed()
            })
            .await
            .unwrap();

        let runner_and_handles = build_and_start(runner_builder, component_name.clone()).await;

        let outgoing_dir_server_end = receiver.await.expect("failed to receive");

        assert_eq!(
            outgoing_dir_server_end
                .into_channel()
                .as_handle_ref()
                .basic_info()
                .expect("failed to get basic info")
                .koid,
            runner_and_handles
                .outgoing_dir_proxy
                .into_channel()
                .expect("failed to convert to channel")
                .as_handle_ref()
                .basic_info()
                .expect("failed to get basic info")
                .related_koid,
        );
    }

    #[fuchsia::test]
    async fn controller_stop_will_stop_a_component() {
        let runner_builder = LocalComponentRunnerBuilder::new();
        let (sender, receiver) = oneshot::channel::<()>();
        let sender = Arc::new(Mutex::new(Some(sender)));

        let component_name = "test".to_string();

        runner_builder
            .register_local_component(component_name.clone(), move |_handles| {
                let sender = sender.clone();
                async move {
                    let _sender =
                        sender.lock().await.take().expect("local component invoked twice");
                    // Don't use sender, we want to detect when it gets dropped, which causes an error
                    // to appear on receiver.
                    pending().await
                }
                .boxed()
            })
            .await
            .unwrap();

        let runner_and_handles = build_and_start(runner_builder, component_name).await;
        runner_and_handles.controller_proxy.stop().expect("failed to send stop");

        assert_eq!(Err(oneshot::Canceled), receiver.await);
    }

    #[fuchsia::test]
    async fn controller_kill_will_kill_a_component() {
        let runner_builder = LocalComponentRunnerBuilder::new();
        let (sender, receiver) = oneshot::channel::<()>();
        let sender = Arc::new(Mutex::new(Some(sender)));

        let component_name = "test".to_string();

        runner_builder
            .register_local_component(component_name.clone(), move |_handles| {
                let sender = sender.clone();
                async move {
                    let _sender =
                        sender.lock().await.take().expect("local component invoked twice");
                    // Don't use sender, we want to detect when it gets dropped, which causes an error
                    // to appear on receiver.
                    pending().await
                }
                .boxed()
            })
            .await
            .unwrap();

        let runner_and_handles = build_and_start(runner_builder, component_name).await;
        runner_and_handles.controller_proxy.kill().expect("failed to send stop");

        assert_eq!(Err(oneshot::Canceled), receiver.await);
    }

    #[fuchsia::test]
    async fn stopping_a_component_calls_the_notifier() {
        let runner_builder = LocalComponentRunnerBuilder::new();
        let (notifier_registered_sender, notifier_registered_receiver) = oneshot::channel::<()>();
        let notifier_registered_sender = Arc::new(Mutex::new(Some(notifier_registered_sender)));

        let (notifier_fired_sender, notifier_fired_receiver) = oneshot::channel::<()>();
        let notifier_fired_sender = Arc::new(Mutex::new(Some(notifier_fired_sender)));

        let component_name = "test".to_string();

        runner_builder
            .register_local_component(component_name.clone(), move |handles| {
                let notifier_registered_sender = notifier_registered_sender.clone();
                let notifier_fired_sender = notifier_fired_sender.clone();
                async move {
                    let stop_notifier = handles.register_stop_notifier().await;

                    let sender = notifier_registered_sender
                        .lock()
                        .await
                        .take()
                        .expect("local component invoked twice");
                    sender.send(()).expect("failed to send that the stop notifier was registered");

                    stop_notifier.await.expect("failed to wait for stop notification");

                    let sender = notifier_fired_sender
                        .lock()
                        .await
                        .take()
                        .expect("local component invoked twice");
                    sender
                        .send(())
                        .expect("failed to send that the stop notifier received a message");

                    Ok(())
                }
                .boxed()
            })
            .await
            .unwrap();

        let runner_and_handles = build_and_start(runner_builder, component_name).await;

        // Wait for the component to have started and registered a stop notifier
        assert_matches!(notifier_registered_receiver.await, Ok(()));

        // Ask to stop the component
        runner_and_handles.controller_proxy.stop().expect("failed to send stop");

        // Wait for the component to have received the stop message
        assert_matches!(notifier_fired_receiver.await, Ok(()));
    }

    #[fuchsia::test]
    async fn dropping_the_runner_will_kill_a_component() {
        let runner_builder = LocalComponentRunnerBuilder::new();
        let (sender, receiver) = oneshot::channel::<()>();
        let sender = Arc::new(Mutex::new(Some(sender)));

        let component_name = "test".to_string();

        runner_builder
            .register_local_component(component_name.clone(), move |_handles| {
                let sender = sender.clone();
                async move {
                    let _sender =
                        sender.lock().await.take().expect("local component invoked twice");
                    // Don't use sender, we want to detect when it gets dropped, which causes an error
                    // to appear on receiver.
                    pending().await
                }
                .boxed()
            })
            .await
            .unwrap();

        let runner_and_handles = build_and_start(runner_builder, component_name).await;
        drop(runner_and_handles);

        assert_eq!(Err(oneshot::Canceled), receiver.await);
    }
}
