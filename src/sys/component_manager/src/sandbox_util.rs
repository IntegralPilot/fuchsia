// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::{
    ComponentInstance, ExtendedInstance, WeakComponentInstance, WeakExtendedInstance,
};
use ::routing::WeakInstanceTokenExt;
use ::routing::error::{ComponentInstanceError, RoutingError};
use ::routing::policy::GlobalPolicyChecker;
use ::routing::rights::Rights;
use async_trait::async_trait;
use capability_source::CapabilitySource;
use cm_rust::CapabilityTypeName;
use cm_types::RelativePath;
use fidl::AsyncChannel;
use fidl::endpoints::{ProtocolMarker, RequestStream, ServerEnd};
use fidl::epitaph::ChannelEpitaphExt;
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use futures::future::BoxFuture;
use log::warn;
use router_error::{Explain, RouterError};
use routing::subdir::SubDir;
use runtime_capabilities::{
    Connectable, Connector, DirConnectable, DirConnector, Routable, Router, WeakInstanceToken,
};
use std::fmt::Debug;
use std::sync::Arc;
use vfs::execution_scope::{ExecutionScope, WeakExecutionScope};

pub fn take_handle_as_stream<P: ProtocolMarker>(channel: zx::Channel) -> P::RequestStream {
    let channel = AsyncChannel::from_channel(channel);
    P::RequestStream::from_channel(channel)
}

/// Waits for a new message on a receiver, and launches a new async task on a `WeakExecutionScope`
/// to handle each new message from the receiver.
#[derive(Clone)]
pub struct LaunchTaskOnReceive {
    capability_source: CapabilitySource,
    task_to_launch: Arc<
        dyn Fn(
                zx::Channel,
                WeakComponentInstance,
                RelativePath,
                fio::Flags,
            ) -> BoxFuture<'static, Result<(), anyhow::Error>>
            + Sync
            + Send
            + 'static,
    >,
    // Note that we explicitly need a WeakExecutionScope because if our `run` call is scheduled on
    // the same task group as we'll be launching tasks on then if we held a strong reference we
    // would inadvertently give the task group a strong reference to itself and make it
    // un-droppable.
    scope: WeakExecutionScope,
    policy: Option<GlobalPolicyChecker>,
    task_name: String,
}

impl std::fmt::Debug for LaunchTaskOnReceive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchTaskOnReceive").field("task_name", &self.task_name).finish()
    }
}

fn cm_unexpected() -> RouterError {
    RoutingError::from(ComponentInstanceError::ComponentManagerInstanceUnexpected {}).into()
}

impl LaunchTaskOnReceive {
    pub fn new(
        capability_source: CapabilitySource,
        scope: WeakExecutionScope,
        task_name: impl Into<String>,
        policy: Option<GlobalPolicyChecker>,
        task_to_launch: Arc<
            dyn Fn(
                    zx::Channel,
                    WeakComponentInstance,
                    RelativePath,
                    fio::Flags,
                ) -> BoxFuture<'static, Result<(), anyhow::Error>>
                + Sync
                + Send
                + 'static,
        >,
    ) -> Self {
        Self { capability_source, task_to_launch, scope, policy, task_name: task_name.into() }
    }

    pub fn into_sender(self: Arc<Self>, target: WeakComponentInstance) -> Arc<Connector> {
        #[derive(Debug)]
        struct TaskAndTarget {
            task: Arc<LaunchTaskOnReceive>,
            target: WeakComponentInstance,
        }

        impl Connectable for TaskAndTarget {
            fn send(&self, channel: zx::Channel) -> Result<(), ()> {
                self.task.launch_task(
                    channel,
                    self.target.clone(),
                    RelativePath::dot(),
                    fio::PERM_READABLE,
                );
                Ok(())
            }
        }

        Connector::new_sendable(TaskAndTarget { task: self, target })
    }

    pub fn into_dir_connector(
        self: Arc<Self>,
        target: WeakComponentInstance,
        relative_path: RelativePath,
        allowed_flags: fio::Flags,
    ) -> Arc<DirConnector> {
        #[derive(Debug)]
        struct TaskAndTarget {
            task: Arc<LaunchTaskOnReceive>,
            target: WeakComponentInstance,
            relative_path: RelativePath,
            allowed_flags: fio::Flags,
        }

        impl DirConnectable for TaskAndTarget {
            fn maximum_flags(&self) -> fio::Flags {
                self.allowed_flags
            }

            fn send(
                &self,
                dir: ServerEnd<fio::DirectoryMarker>,
                subdir: RelativePath,
                flags: Option<fio::Flags>,
            ) -> Result<(), ()> {
                let mut relative_path = self.relative_path.clone();
                if !subdir.is_dot() {
                    let subdir_str = format!("{subdir}");
                    let success = relative_path.extend(subdir);
                    if !success {
                        log::warn!("path too long! {relative_path}/{subdir_str}");
                        return Err(());
                    }
                }
                let allowed_flags = fio::Flags::from_bits(self.allowed_flags.bits()).unwrap();
                let flags = flags.unwrap_or(allowed_flags | fio::Flags::PROTOCOL_DIRECTORY);
                self.task.launch_task(
                    dir.into_channel(),
                    self.target.clone(),
                    relative_path,
                    flags,
                );
                Ok(())
            }
        }

        DirConnector::new_sendable(TaskAndTarget {
            task: self,
            target,
            relative_path,
            allowed_flags: fio::Flags::from_bits(allowed_flags.bits()).expect("invalid flags"),
        })
    }

    pub fn into_router(self) -> Arc<Router<Connector>> {
        #[derive(Debug)]
        struct LaunchTaskRouter {
            inner: Arc<LaunchTaskOnReceive>,
        }
        #[async_trait]
        impl Routable<Connector> for LaunchTaskRouter {
            async fn route(
                &self,
                _request: RouteRequest,
                target: Arc<WeakInstanceToken>,
            ) -> Result<Option<Arc<Connector>>, RouterError> {
                let WeakExtendedInstance::Component(target) = target.to_instance() else {
                    return Err(cm_unexpected());
                };
                let conn = self.inner.clone().into_sender(target);
                Ok(Some(conn))
            }

            async fn route_debug(
                &self,
                _request: RouteRequest,
                _target: Arc<WeakInstanceToken>,
            ) -> Result<CapabilitySource, RouterError> {
                Ok(self.inner.capability_source.clone())
            }
        }
        Router::<Connector>::new(LaunchTaskRouter { inner: Arc::new(self) })
    }

    pub fn into_dir_router(self) -> Arc<Router<DirConnector>> {
        #[derive(Debug)]
        struct LaunchTaskRouter {
            inner: Arc<LaunchTaskOnReceive>,
        }
        #[async_trait]
        impl Routable<DirConnector> for LaunchTaskRouter {
            async fn route(
                &self,
                request: RouteRequest,
                target: Arc<WeakInstanceToken>,
            ) -> Result<Option<Arc<DirConnector>>, RouterError> {
                let WeakExtendedInstance::Component(target) = target.to_instance() else {
                    return Err(cm_unexpected());
                };
                let subdir = SubDir::new(&request.sub_directory_path.unwrap()).unwrap();
                let rights = Rights::from(request.directory_rights.unwrap());
                let conn =
                    self.inner.clone().into_dir_connector(target, subdir.into(), rights.into());
                Ok(Some(conn))
            }

            async fn route_debug(
                &self,
                _request: RouteRequest,
                _target: Arc<WeakInstanceToken>,
            ) -> Result<CapabilitySource, RouterError> {
                Ok(self.inner.capability_source.clone())
            }
        }
        Router::<DirConnector>::new(LaunchTaskRouter { inner: Arc::new(self) })
    }

    fn launch_task(
        &self,
        channel: zx::Channel,
        instance: WeakComponentInstance,
        relative_path: RelativePath,
        flags: fio::Flags,
    ) {
        if let Some(policy_checker) = &self.policy {
            if let Err(_e) =
                policy_checker.can_route_capability(&self.capability_source, &instance.moniker)
            {
                // The `can_route_capability` function above will log an error, so we don't
                // have to.
                let _ = channel.close_with_epitaph(zx::Status::ACCESS_DENIED);
                return;
            }
        }

        let fut = (self.task_to_launch)(channel, instance, relative_path, flags);
        let task_name = self.task_name.clone();
        self.scope.spawn(async move {
            if let Err(error) = fut.await {
                warn!(error:%; "{} failed", task_name);
            }
        });
    }
}

/// Porcelain methods on [`Routable`] objects.
pub trait RoutableExt {
    /// Returns a router that resolves with a [`runtime_capabilities::Connector`] that watches for
    /// the channel to be readable, then delegates to the current router. The wait
    /// is performed in the provided `scope`.
    fn on_readable(self, scope: ExecutionScope) -> Arc<Router<Connector>>;
}

impl RoutableExt for Arc<Router<Connector>> {
    fn on_readable(self, scope: ExecutionScope) -> Arc<Router<Connector>> {
        #[derive(Debug)]
        struct OnReadableRouter {
            router: Arc<Router<Connector>>,
            scope: ExecutionScope,
        }

        #[async_trait]
        impl Routable<Connector> for OnReadableRouter {
            async fn route(
                &self,
                request: RouteRequest,
                target_token: Arc<WeakInstanceToken>,
            ) -> Result<Option<Arc<Connector>>, RouterError> {
                let ExtendedInstance::Component(target) =
                    target_token.clone().to_instance().upgrade().map_err(RoutingError::from)?
                else {
                    return Err(cm_unexpected());
                };
                #[derive(Debug)]
                struct DefaultRoutable<F: Send + Sync + 'static> {
                    router: Arc<Router<Connector>>,
                    default_fn: F,
                }
                #[async_trait]
                impl<F: Fn() -> RouteRequest + Send + Sync + 'static> Routable<Connector> for DefaultRoutable<F> {
                    async fn route(
                        &self,
                        request: RouteRequest,
                        target: Arc<WeakInstanceToken>,
                    ) -> Result<Option<Arc<Connector>>, RouterError> {
                        let request = if request != RouteRequest::default() {
                            request
                        } else {
                            (self.default_fn)()
                        };
                        self.router.route(request, target).await
                    }

                    async fn route_debug(
                        &self,
                        request: RouteRequest,
                        target: Arc<WeakInstanceToken>,
                    ) -> Result<CapabilitySource, RouterError> {
                        let request = if request != RouteRequest::default() {
                            request
                        } else {
                            (self.default_fn)()
                        };
                        self.router.route_debug(request, target).await
                    }
                }

                let router = Router::new(DefaultRoutable {
                    router: self.router.clone(),
                    default_fn: move || request.clone(),
                });

                // Wrap the router in something that will wait until the channel is readable.
                #[derive(Debug)]
                struct OnReadable {
                    scope: ExecutionScope,
                    target: Arc<ComponentInstance>,
                    router: Arc<Router<Connector>>,
                    target_token: Arc<WeakInstanceToken>,
                }
                impl Connectable for OnReadable {
                    fn send(&self, channel: zx::Channel) -> Result<(), ()> {
                        let router = self.router.clone();
                        let target = self.target.clone();
                        let target_token = self.target_token.clone();
                        self.scope.spawn(async move {
                            match Self::send_inner(&router, &target, &channel, target_token).await {
                                Ok(conn) => {
                                    // We're in an async task, and the original function already
                                    // returned Ok. There's nothing we can do with this result.
                                    let _ = conn.send(channel);
                                }
                                Err(e) => {
                                    let _ = channel.close_with_epitaph(e);
                                }
                            }
                        });
                        Ok(())
                    }
                }
                impl OnReadable {
                    async fn send_inner(
                        router: &Arc<Router<Connector>>,
                        target: &Arc<ComponentInstance>,
                        channel: &fidl::Channel,
                        target_token: Arc<WeakInstanceToken>,
                    ) -> Result<Arc<Connector>, zx::Status> {
                        let signals = fasync::OnSignalsRef::new(
                            channel.as_handle_ref(),
                            fidl::Signals::OBJECT_READABLE | fidl::Signals::CHANNEL_PEER_CLOSED,
                        )
                        .await
                        .unwrap();
                        if !signals.contains(fidl::Signals::OBJECT_READABLE) {
                            return Err(zx::Status::PEER_CLOSED);
                        }
                        let conn = match router
                            .route(RouteRequest::default(), target_token)
                            .await
                            .and_then(|resp| match resp {
                                Some(c) => Ok(c),
                                None => Err(RoutingError::RouteUnexpectedUnavailable {
                                    type_name: CapabilityTypeName::Protocol,
                                    moniker: target.moniker.clone().into(),
                                }
                                .into()),
                            }) {
                            Ok(c) => c,
                            Err(err) => {
                                // TODO(https://fxbug.dev/319754472): Improve the fidelity of error
                                // logging. This should log into the component's log sink using the
                                // proper `report_routing_failure`, but that function requires a
                                // legacy `RouteRequest` at the moment.
                                target
                                    .log(
                                        log::Level::Warn,
                                        format!(
                                            "Request was not available for target component `{}`: `{}`",
                                            target.moniker, err
                                        ),
                                        &[]
                                    )
                                    .await;
                                return Err(err.as_zx_status());
                            }
                        };
                        Ok(conn)
                    }
                }

                let on_readable =
                    OnReadable { scope: self.scope.clone(), router, target, target_token };
                Ok(Some(Connector::new_sendable(on_readable)))
            }

            async fn route_debug(
                &self,
                request: RouteRequest,
                target_token: Arc<WeakInstanceToken>,
            ) -> Result<CapabilitySource, RouterError> {
                return self.router.route_debug(request, target_token).await;
            }
        }

        Router::<Connector>::new(OnReadableRouter { router: self, scope })
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
pub mod tests {
    use crate::model::context::ModelContext;

    use super::*;
    use ::routing::component_instance::ComponentInstanceInterface;
    use assert_matches::assert_matches;
    use capability_source::{InternalCapability, VoidSource};
    use cm_rust::{Availability, NativeIntoFidl};
    use cm_types::{Name, RelativePath};
    use fuchsia_async::TestExecutor;
    use moniker::Moniker;
    use routing::DictExt;
    use routing::bedrock::structured_dict::ComponentInput;
    use runtime_capabilities::{Capability, CapabilityBound, Dictionary, Routable};
    use std::pin::pin;
    use std::sync::Weak;
    use std::task::Poll;
    use vfs::directory::entry::OpenRequest;
    use vfs::{Path, ToObjectRequest};

    #[fuchsia::test]
    async fn get_capability() {
        let sub_dict = Dictionary::new();
        let prev =
            sub_dict.insert("bar".parse().unwrap(), Capability::Dictionary(Dictionary::new()));
        assert!(prev.is_none(), "dict entry already exists");
        let (_, sender) = Connector::new();
        let prev = sub_dict.insert("baz".parse().unwrap(), sender.into());
        assert!(prev.is_none(), "dict entry already exists");

        let test_dict = Dictionary::new();
        let prev = test_dict.insert("foo".parse().unwrap(), Capability::Dictionary(sub_dict));
        assert!(prev.is_none(), "dict entry already exists");

        assert!(test_dict.get_capability(&RelativePath::dot()).is_some());
        assert!(test_dict.get_capability(&RelativePath::new("nonexistent").unwrap()).is_none());
        assert!(test_dict.get_capability(&RelativePath::new("foo").unwrap()).is_some());
        assert!(test_dict.get_capability(&RelativePath::new("foo/bar").unwrap()).is_some());
        assert!(test_dict.get_capability(&RelativePath::new("foo/nonexistent").unwrap()).is_none());
        assert!(test_dict.get_capability(&RelativePath::new("foo/baz").unwrap()).is_some());
    }

    #[fuchsia::test]
    async fn insert_capability() {
        let test_dict = Dictionary::new();
        assert!(
            test_dict
                .insert_capability(&RelativePath::new("foo/bar").unwrap(), Dictionary::new().into())
                .is_none()
        );
        assert!(test_dict.get_capability(&RelativePath::new("foo/bar").unwrap()).is_some());

        let (_, sender) = Connector::new();
        assert!(
            test_dict
                .insert_capability(&RelativePath::new("foo/baz").unwrap(), sender.into())
                .is_none()
        );
        assert!(test_dict.get_capability(&RelativePath::new("foo/baz").unwrap()).is_some());
    }

    #[fuchsia::test]
    async fn remove_capability() {
        let test_dict = Dictionary::new();
        assert!(
            test_dict
                .insert_capability(&RelativePath::new("foo/bar").unwrap(), Dictionary::new().into())
                .is_none()
        );
        assert!(test_dict.get_capability(&RelativePath::new("foo/bar").unwrap()).is_some());

        test_dict.remove_capability(&RelativePath::new("foo/bar").unwrap());
        assert!(test_dict.get_capability(&RelativePath::new("foo/bar").unwrap()).is_none());
        assert!(test_dict.get_capability(&RelativePath::new("foo").unwrap()).is_some());

        test_dict.remove_capability(&RelativePath::new("foo").unwrap());
        assert!(test_dict.get_capability(&RelativePath::new("foo").unwrap()).is_none());
    }

    #[derive(Debug, Clone)]
    struct RouteCounter {
        connector: Arc<Connector>,
        counter: Arc<test_util::Counter>,
    }

    impl RouteCounter {
        fn new(connector: Arc<Connector>) -> Self {
            Self { connector, counter: Arc::new(test_util::Counter::new(0)) }
        }

        fn count(&self) -> usize {
            self.counter.get()
        }
    }

    #[async_trait]
    impl Routable<Connector> for RouteCounter {
        async fn route(
            &self,
            _: RouteRequest,
            _: Arc<WeakInstanceToken>,
        ) -> Result<Option<Arc<Connector>>, RouterError> {
            self.counter.inc();
            Ok(Some(self.connector.clone()))
        }

        async fn route_debug(
            &self,
            _: RouteRequest,
            _: Arc<WeakInstanceToken>,
        ) -> Result<CapabilitySource, RouterError> {
            unimplemented!("should not be called during tests");
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn router_on_readable_client_writes() {
        let (receiver, sender) = Connector::new();
        let scope = ExecutionScope::new();
        let (client_end, server_end) = zx::Channel::create();

        let route_counter = RouteCounter::new(sender);
        let router = Router::new(route_counter.clone()).on_readable(scope.clone());

        let mut receive = pin!(receiver.receive());
        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);

        let component = ComponentInstance::new_root(
            ComponentInput::default(),
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "test:///root".parse().unwrap(),
        )
        .await;
        let request = RouteRequest {
            availability: Some(Availability::Required.native_into_fidl()),
            ..Default::default()
        };
        let Some(conn) = router.route(request, component.as_weak().into()).await.unwrap() else {
            panic!();
        };

        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);
        assert_eq!(route_counter.count(), 0);

        let mut object_request = fio::Flags::PROTOCOL_SERVICE.to_object_request(server_end);
        conn.try_into_directory_entry(scope.clone(), component.as_weak().into())
            .unwrap()
            .open_entry(OpenRequest::new(
                scope.clone(),
                fio::Flags::PROTOCOL_SERVICE,
                Path::dot(),
                &mut object_request,
            ))
            .unwrap();

        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);
        assert_eq!(route_counter.count(), 0);

        client_end.write(&[0], &mut []).unwrap();
        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Ready(Some(_)));
        scope.wait().await;
        assert_eq!(route_counter.count(), 1);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn router_on_readable_client_closes() {
        let (receiver, sender) = Connector::new();
        let scope = ExecutionScope::new();
        let (client_end, server_end) = zx::Channel::create();

        let route_counter = RouteCounter::new(sender.into());
        let router = Router::new(route_counter.clone()).on_readable(scope.clone());

        let mut receive = pin!(receiver.receive());
        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);

        let component = ComponentInstance::new_root(
            ComponentInput::default(),
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "test:///root".parse().unwrap(),
        )
        .await;
        let request = RouteRequest {
            availability: Some(Availability::Required.native_into_fidl()),
            ..Default::default()
        };
        let Some(conn) = router.route(request, component.as_weak().into()).await.unwrap() else {
            panic!();
        };

        let mut object_request = fio::Flags::PROTOCOL_SERVICE.to_object_request(server_end);
        conn.try_into_directory_entry(scope.clone(), WeakInstanceToken::new_invalid())
            .unwrap()
            .open_entry(OpenRequest::new(
                scope.clone(),
                fio::Flags::PROTOCOL_SERVICE,
                Path::dot(),
                &mut object_request,
            ))
            .unwrap();

        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);
        assert_matches!(
            TestExecutor::poll_until_stalled(Box::pin(scope.clone().wait())).await,
            Poll::Pending
        );
        assert_eq!(route_counter.count(), 0);

        drop(client_end);
        assert_matches!(TestExecutor::poll_until_stalled(&mut receive).await, Poll::Pending);
        scope.wait().await;
        assert_eq!(route_counter.count(), 0);
    }

    #[fuchsia::test]
    async fn router_on_readable_debug() {
        let scope = ExecutionScope::new();

        let source_moniker: Moniker = "source".try_into().unwrap();
        let mut source = WeakComponentInstance::invalid();
        source.moniker = source_moniker;
        struct DebugRouter;
        #[async_trait]
        impl Routable<Connector> for DebugRouter {
            async fn route(
                &self,
                _request: RouteRequest,
                _target: Arc<WeakInstanceToken>,
            ) -> Result<Option<Arc<Connector>>, RouterError> {
                panic!("non-debug routing unexpected");
            }

            async fn route_debug(
                &self,
                _request: RouteRequest,
                _target: Arc<WeakInstanceToken>,
            ) -> Result<CapabilitySource, RouterError> {
                Ok(CapabilitySource::Void(VoidSource {
                    capability: InternalCapability::Protocol(Name::new("a").unwrap()),
                    moniker: Moniker::root(),
                }))
            }
        }
        let debug_router = Router::new(DebugRouter {});
        let router = debug_router.clone().on_readable(scope.clone());

        let target = ComponentInstance::new_root(
            ComponentInput::default(),
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "test:///target".parse().unwrap(),
        )
        .await;
        let request = RouteRequest {
            availability: Some(Availability::Required.native_into_fidl()),
            ..Default::default()
        };
        let resp = router.route_debug(request, target.as_weak().into()).await.unwrap();
        assert_matches!(resp, CapabilitySource::Void(_));
    }
}
