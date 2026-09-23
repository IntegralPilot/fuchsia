// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::WeakComponentInstance;
use crate::sandbox_util::take_handle_as_stream;
use anyhow::{Error, format_err};
use async_trait::async_trait;
use capability_source::{CapabilitySource, RemotedAtSource};
use cm_rust::CapabilityTypeName;
use cm_types::{Name, RelativePath};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component_runtime as fruntime;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt, select};
use moniker::Moniker;
use router_error::{Explain, RouterError};
use routing::error::RoutingError;
use runtime_capabilities::{
    Capability, CapabilityBound, Connectable, Connector, Data, Dictionary, DirConnectable,
    DirConnector, Routable, Router, WeakInstanceToken,
};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use vfs::WeakExecutionScope;
use zx::sys::ZX_CHANNEL_MAX_MSG_BYTES;

/// These two constants are needed for computing how many strings we can fit into a FIDL message.
/// There's unfortunately no better solution than doing the math ourselves.
const FIDL_VECTOR_OVERHEAD: usize = 48;
const FIDL_STRING_OVERHEAD: usize = 16;

pub fn serve(
    server_end: zx::Channel,
    _target: WeakComponentInstance,
    weak_source: WeakComponentInstance,
) -> BoxFuture<'static, Result<(), Error>> {
    async move {
        let source = weak_source.upgrade()?;
        let remote_capabilities = source.context.remote_capabilities().clone();
        let weak_scope = source.execution_scope.as_weak();
        let stream = take_handle_as_stream::<fruntime::CapabilitiesMarker>(server_end);
        let moniker = weak_source.moniker.clone();
        let default_target = weak_source.into();
        Capabilities { remote_capabilities, weak_scope, default_target, moniker }
            .handle_stream(stream)
            .boxed()
            .await
    }
    .boxed()
}

/// Capabilities which have had event pair handles assigned to them, allowing normal components to
/// reference and interact with them.
pub struct RemotedRuntimeCapabilities {
    pub(super) remote_capabilities: Arc<Mutex<HashMap<zx::Koid, Capability>>>,
    event_pair_sender: mpsc::UnboundedSender<zx::EventPair>,
    _garbage_collector_task: fasync::Task<()>,
}

impl RemotedRuntimeCapabilities {
    pub fn new() -> Self {
        let (event_pair_sender, mut receiver) = mpsc::unbounded();
        let remote_capabilities = Arc::new(Mutex::new(HashMap::new()));
        let remote_capabilities_hashmap_clone = remote_capabilities.clone();
        let _garbage_collector_task = fasync::Task::spawn(async move {
            let mut event_pair_watchers = FuturesUnordered::new();
            loop {
                let event_pair = if event_pair_watchers.is_empty() {
                    let Some(event_pair) = receiver.next().await else {
                        return;
                    };
                    event_pair
                } else {
                    let mut event_pair_close_fut = event_pair_watchers.next();
                    let mut new_event_pair = receiver.next();
                    select! {
                        closed_event_pair = event_pair_close_fut => {
                            let closed_event_pair: zx::EventPair = closed_event_pair
                                .expect("we checked if event_pair_watchers is empty above");
                            remote_capabilities_hashmap_clone
                                .lock()
                                .remove(&closed_event_pair.basic_info().unwrap().koid)
                                .expect("this should be the only point things get removed, and thus this should never fail");
                            continue;
                        }
                        maybe_event_pair = new_event_pair => {
                            let Some(event_pair) = maybe_event_pair else {
                                return;
                            };
                            event_pair
                        }
                    }
                };
                event_pair_watchers.push(async move {
                    let _ = fasync::OnSignals::new(&event_pair, zx::Signals::EVENTPAIR_PEER_CLOSED)
                        .await;
                    event_pair
                });
            }
        });
        Self { remote_capabilities, event_pair_sender, _garbage_collector_task }
    }

    pub fn store(
        &self,
        event_pair: zx::EventPair,
        capability: impl Into<Capability>,
    ) -> Result<(), fruntime::CapabilitiesError> {
        let koid =
            event_pair.basic_info().map_err(|_| fruntime::CapabilitiesError::InvalidHandle)?.koid;
        let mut guard = self.remote_capabilities.lock();
        if guard.contains_key(&koid) {
            return Err(fruntime::CapabilitiesError::HandleAlreadyRegistered);
        }
        guard.insert(koid, capability.into());
        self.event_pair_sender.unbounded_send(event_pair).expect("the receiver should never be dropped as long as this RemoteRuntimeCapabilities is live");
        Ok(())
    }

    pub fn get<C>(&self, handle: zx::EventPair) -> Result<Arc<C>, fruntime::CapabilitiesError>
    where
        Arc<C>: TryFrom<Capability>,
    {
        let koid = handle
            .basic_info()
            .map_err(|_| fruntime::CapabilitiesError::InvalidHandle)?
            .related_koid;
        let guard = self.remote_capabilities.lock();
        let capability = guard
            .get(&koid)
            .ok_or(fruntime::CapabilitiesError::HandleDoesNotReferenceCapability)?;
        capability
            .clone()
            .try_into()
            .map_err(|_| fruntime::CapabilitiesError::InvalidCapabilityType)
    }

    /// Identical to `get` but returns a generic Capability
    pub fn get_capability(
        &self,
        handle: zx::EventPair,
    ) -> Result<Capability, fruntime::CapabilitiesError> {
        let koid = handle
            .basic_info()
            .map_err(|_| fruntime::CapabilitiesError::InvalidHandle)?
            .related_koid;
        let guard = self.remote_capabilities.lock();
        let capability = guard
            .get(&koid)
            .ok_or(fruntime::CapabilitiesError::HandleDoesNotReferenceCapability)?;
        Ok(capability.clone())
    }
}

struct Capabilities {
    remote_capabilities: Arc<RemotedRuntimeCapabilities>,
    weak_scope: WeakExecutionScope,
    default_target: Arc<WeakInstanceToken>,
    moniker: Moniker,
}

impl Capabilities {
    pub async fn handle_stream(
        self,
        mut stream: fruntime::CapabilitiesRequestStream,
    ) -> Result<(), Error> {
        while let Some(Ok(request)) = stream.next().await {
            self.handle_request(request).boxed().await?;
        }
        Ok(())
    }

    async fn handle_request(&self, request: fruntime::CapabilitiesRequest) -> Result<(), Error> {
        match request {
            fruntime::CapabilitiesRequest::ConnectorCreate {
                connector,
                receiver_client_end,
                responder,
                ..
            } => {
                let cap = Connector::new_sendable(RemoteReceiver {
                    remote_receiver: receiver_client_end.into_proxy(),
                });
                let res = self.remote_capabilities.store(connector, cap);
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DirConnectorCreate {
                dir_connector,
                receiver_client_end,
                responder,
                ..
            } => {
                let cap = DirConnector::new_sendable(RemoteDirReceiver {
                    remote_receiver: receiver_client_end.into_proxy(),
                });
                let res = self.remote_capabilities.store(dir_connector, cap);
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DictionaryCreate { dictionary, responder, .. } => {
                let res = self.remote_capabilities.store(dictionary, Dictionary::new());
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DataCreate { data_handle, data, responder, .. } => {
                let data_res = data_from_remote(data);
                let res =
                    data_res.and_then(|data| self.remote_capabilities.store(data_handle, data));
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::ConnectorRouterCreate {
                router,
                router_client_end,
                responder,
                ..
            } => {
                let remote_router = Router::<Connector>::new(RemoteRouter::new(
                    router_client_end.into_proxy(),
                    self.remote_capabilities.clone(),
                    self.moniker.clone(),
                ));
                let res = self.remote_capabilities.store(router, remote_router);
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DirConnectorRouterCreate {
                router,
                router_client_end,
                responder,
                ..
            } => {
                let remote_router = Router::<DirConnector>::new(RemoteRouter::new(
                    router_client_end.into_proxy(),
                    self.remote_capabilities.clone(),
                    self.moniker.clone(),
                ));
                let res = self.remote_capabilities.store(router, remote_router);
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DictionaryRouterCreate {
                router,
                router_client_end,
                responder,
                ..
            } => {
                let remote_router = Router::<Dictionary>::new(RemoteRouter::new(
                    router_client_end.into_proxy(),
                    self.remote_capabilities.clone(),
                    self.moniker.clone(),
                ));
                let res = self.remote_capabilities.store(router, remote_router);
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DataRouterCreate {
                router,
                router_client_end,
                responder,
                ..
            } => {
                let remote_router = Router::<Data>::new(RemoteRouter::new(
                    router_client_end.into_proxy(),
                    self.remote_capabilities.clone(),
                    self.moniker.clone(),
                ));
                let res = self.remote_capabilities.store(router, remote_router);
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::InstanceTokenCreate {
                instance_token,
                responder,
                ..
            } => {
                let res =
                    self.remote_capabilities.store(instance_token, self.default_target.clone());
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::ConnectorOpen {
                connector, channel, responder, ..
            } => {
                let res = self.remote_capabilities.get::<Connector>(connector).map(|c| {
                    let _ = c.send(channel);
                });
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DirConnectorOpen { payload, responder, .. } => {
                let res = (|| {
                    let invalid_args = fruntime::CapabilitiesError::InvalidArgs;
                    let handle = payload.dir_connector.ok_or(invalid_args)?;
                    let channel = payload.channel.ok_or(invalid_args)?;
                    let path_str = payload.path.unwrap_or_else(|| ".".to_string());
                    let path = RelativePath::new(path_str).map_err(|_| invalid_args)?;
                    let dir_connector = self.remote_capabilities.get::<DirConnector>(handle)?;
                    let _ = dir_connector.send(channel, path, payload.flags);
                    Ok(())
                })();
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DictionaryInsert {
                dictionary,
                key,
                value,
                responder,
                ..
            } => {
                let res = (|| {
                    let dictionary = self.remote_capabilities.get::<Dictionary>(dictionary)?;
                    let key =
                        Name::new(key).map_err(|_| fruntime::CapabilitiesError::InvalidArgs)?;
                    let value = self.remote_capabilities.get_capability(value)?;
                    let _ = dictionary.insert(key, value);
                    Ok(())
                })();
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DictionaryGet {
                dictionary,
                key,
                value,
                responder,
                ..
            } => {
                let res = (|| {
                    let dictionary = self.remote_capabilities.get::<Dictionary>(dictionary)?;
                    let key =
                        Name::new(key).map_err(|_| fruntime::CapabilitiesError::InvalidArgs)?;
                    let cap = dictionary
                        .get(&key)
                        .ok_or(fruntime::CapabilitiesError::NoSuchCapability)?;
                    let type_ = capability_as_type(&cap)
                        .map_err(|_| fruntime::CapabilitiesError::InvalidArgs)?;
                    self.remote_capabilities.store(value, cap)?;
                    Ok(type_)
                })();
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DictionaryRemove { payload, responder, .. } => {
                let res = (|| {
                    let handle =
                        payload.dictionary.ok_or(fruntime::CapabilitiesError::InvalidArgs)?;
                    let dictionary = self.remote_capabilities.get::<Dictionary>(handle)?;
                    let key = payload.key.ok_or(fruntime::CapabilitiesError::InvalidArgs)?;
                    let key =
                        Name::new(key).map_err(|_| fruntime::CapabilitiesError::InvalidArgs)?;
                    let cap = dictionary
                        .remove(&key)
                        .ok_or(fruntime::CapabilitiesError::NoSuchCapability)?;
                    let type_ = capability_as_type(&cap)
                        .map_err(|_| fruntime::CapabilitiesError::InvalidArgs)?;
                    if let Some(value) = payload.value {
                        self.remote_capabilities.store(value, cap)?;
                    }
                    Ok(type_)
                })();
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DictionaryIterateKeys {
                dictionary,
                key_iterator,
                responder,
                ..
            } => {
                let res = (|| {
                    let dictionary = self.remote_capabilities.get::<Dictionary>(dictionary)?;
                    self.weak_scope
                        .spawn(handle_key_iterator_stream(dictionary, key_iterator.into_stream()));
                    Ok(())
                })();
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::DataGet { data_handle, responder, .. } => {
                match self.remote_capabilities.get(data_handle) {
                    Ok(data) => {
                        let _ = responder.send(Ok(&data_to_remote(data)));
                    }
                    Err(e) => {
                        let _ = responder.send(Err(e));
                    }
                }
            }
            fruntime::CapabilitiesRequest::CapabilityAssociateHandle {
                capability_handle,
                other_handle,
                responder,
                ..
            } => {
                let res = (|| {
                    let capability: Capability =
                        self.remote_capabilities.get_capability(capability_handle)?;
                    self.remote_capabilities.store(other_handle, capability)
                })();
                let _ = responder.send(res);
            }
            fruntime::CapabilitiesRequest::ConnectorRouterRoute {
                router,
                request,
                instance_token,
                connector,
                responder,
                ..
            } => {
                let res = route_from_remote::<Connector>(
                    &self.remote_capabilities,
                    router,
                    request,
                    instance_token,
                    connector,
                )
                .await;
                let _ = responder.send(res.map_err(zx::Status::into_raw));
            }
            fruntime::CapabilitiesRequest::DirConnectorRouterRoute {
                router,
                request,
                instance_token,
                dir_connector,
                responder,
                ..
            } => {
                let res = route_from_remote::<DirConnector>(
                    &self.remote_capabilities,
                    router,
                    request,
                    instance_token,
                    dir_connector,
                )
                .await;
                let _ = responder.send(res.map_err(zx::Status::into_raw));
            }
            fruntime::CapabilitiesRequest::DictionaryRouterRoute {
                router,
                request,
                instance_token,
                dictionary,
                responder,
                ..
            } => {
                let res = route_from_remote::<Dictionary>(
                    &self.remote_capabilities,
                    router,
                    request,
                    instance_token,
                    dictionary,
                )
                .await;
                let _ = responder.send(res.map_err(zx::Status::into_raw));
            }
            fruntime::CapabilitiesRequest::DataRouterRoute {
                router,
                request,
                instance_token,
                data,
                responder,
                ..
            } => {
                let res = route_from_remote::<Data>(
                    &self.remote_capabilities,
                    router,
                    request,
                    instance_token,
                    data,
                )
                .await;
                let _ = responder.send(res.map_err(zx::Status::into_raw));
            }
            request => return Err(format_err!("unknown request type: {request:?}")),
        }
        Ok(())
    }
}

#[derive(Debug)]
struct RemoteReceiver {
    remote_receiver: fruntime::ReceiverProxy,
}

impl Connectable for RemoteReceiver {
    fn send(&self, channel: zx::Channel) -> Result<(), ()> {
        let _ = self.remote_receiver.receive(channel);
        Ok(())
    }
}

#[derive(Debug)]
struct RemoteDirReceiver {
    remote_receiver: fruntime::DirReceiverProxy,
}

impl DirConnectable for RemoteDirReceiver {
    fn maximum_flags(&self) -> fio::Flags {
        // Asking a DirConnector implemented outside of component manager for its maximum allowed
        // rights isn't part of the FIDL contract, so those DirConnectors will have to implement
        // rights checking on their own.
        fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE
    }

    fn send(
        &self,
        server_end: ServerEnd<fio::DirectoryMarker>,
        subdir: RelativePath,
        flags: Option<fio::Flags>,
    ) -> Result<(), ()> {
        let _ = self.remote_receiver.receive(
            server_end,
            subdir.to_path_buf().to_str().expect("subdir was invalid string"),
            flags.unwrap_or(
                fio::PERM_READABLE
                    | fio::Flags::PERM_INHERIT_WRITE
                    | fio::Flags::PERM_INHERIT_EXECUTE,
            ),
        );
        Ok(())
    }
}

fn data_to_remote(data: Arc<Data>) -> fruntime::Data {
    match &*data {
        Data::Bytes(bytes) => fruntime::Data::Bytes(bytes.to_vec()),
        Data::String(string) => fruntime::Data::String(string.to_string()),
        Data::Int64(num) => fruntime::Data::Int64(*num),
        Data::Uint64(num) => fruntime::Data::Uint64(*num),
    }
}

fn data_from_remote(data: fruntime::Data) -> Result<Arc<Data>, fruntime::CapabilitiesError> {
    match data {
        fruntime::Data::Bytes(bytes) => Ok(Arc::new(Data::Bytes(bytes.into()))),
        fruntime::Data::String(string) => Ok(Arc::new(Data::String(string.into()))),
        fruntime::Data::Int64(num) => Ok(Arc::new(Data::Int64(num))),
        fruntime::Data::Uint64(num) => Ok(Arc::new(Data::Uint64(num))),
        _ => Err(fruntime::CapabilitiesError::InvalidData),
    }
}

async fn route_from_remote<C>(
    remote_capabilities: &RemotedRuntimeCapabilities,
    router: zx::EventPair,
    request: fruntime::RouteRequest,
    target: zx::EventPair,
    capability_result: zx::EventPair,
) -> Result<fruntime::RouterResponse, zx::Status>
where
    C: CapabilityBound + Send + Sync,
    Arc<C>: Into<Capability>,
    Arc<Router<C>>: TryFrom<Capability>,
{
    let router =
        remote_capabilities.get::<Router<C>>(router).map_err(|_| zx::Status::INVALID_ARGS)?;
    let target = remote_capabilities
        .get::<WeakInstanceToken>(target)
        .map_err(|_| zx::Status::INVALID_ARGS)?;
    match router.route(request, target).await {
        Ok(Some(cap)) => {
            remote_capabilities
                .store(capability_result, cap)
                .map_err(|_| zx::Status::INVALID_ARGS)?;
            Ok(fruntime::RouterResponse::Success)
        }
        Ok(None) => Ok(fruntime::RouterResponse::Unavailable),
        Err(e) => Err(e.as_zx_status()),
    }
}

pub struct RemoteRouter<R: RemoteRoutable + Send + Sync> {
    router_client_end: R,
    remote_capabilities: Arc<RemotedRuntimeCapabilities>,
    moniker: Moniker,
}

impl<R: RemoteRoutable + Send + Sync> RemoteRouter<R> {
    pub fn new(
        remote: R,
        remote_capabilities: Arc<RemotedRuntimeCapabilities>,
        moniker: Moniker,
    ) -> Self {
        Self { router_client_end: remote, remote_capabilities, moniker }
    }
}

#[async_trait]
impl<C, R> Routable<C> for RemoteRouter<R>
where
    C: CapabilityBound + Send + Sync,
    Arc<C>: TryFrom<Capability>,
    R: RemoteRoutable + Send + Sync,
{
    async fn route(
        &self,
        request: fruntime::RouteRequest,
        target_token: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<C>>, RouterError> {
        let (token, token_other_end) = zx::EventPair::create();
        self.remote_capabilities
            .store(token_other_end, target_token)
            .expect("this should be infallible");
        // This block won't be Send if we use &self after the await. To work around this, take a
        // clone of remote_capabilities here to use later.
        let remote_capabilities = self.remote_capabilities.clone();
        let (e1, e2) = zx::EventPair::create();
        let result = self.router_client_end.route(request, token, e1).await;
        match result {
            Ok(Ok(fruntime::RouterResponse::Success)) => {
                if let Ok(cap) = remote_capabilities.get::<C>(e2) {
                    Ok(Some(cap))
                } else {
                    Err(RoutingError::RemoteFIDLError { moniker: self.moniker.clone() }.into())
                }
            }
            Ok(Ok(fruntime::RouterResponse::Unavailable)) => Ok(None),
            Ok(Ok(_)) => {
                Err(RoutingError::RemoteFIDLError { moniker: self.moniker.clone() }.into())
            }
            Ok(Err(e)) => Err(RoutingError::RemoteRouterError {
                moniker: self.moniker.clone(),
                error_code: e,
            }
            .into()),
            Err(_e) => Err(RoutingError::RemoteFIDLError { moniker: self.moniker.clone() }.into()),
        }
    }

    async fn route_debug(
        &self,
        request: fruntime::RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        let type_name: Option<CapabilityTypeName> =
            request.build_type_name.and_then(|s| CapabilityTypeName::from_str(s.as_str()).ok());
        Ok(CapabilitySource::RemotedAt(RemotedAtSource {
            moniker: self.moniker.clone(),
            type_name,
        })
        .try_into()
        .unwrap())
    }
}

#[async_trait]
pub trait RemoteRoutable {
    async fn route(
        &self,
        request: fruntime::RouteRequest,
        instance_token: zx::EventPair,
        event_pair: zx::EventPair,
    ) -> Result<Result<fruntime::RouterResponse, i32>, fidl::Error>;
}

#[async_trait]
impl RemoteRoutable for fruntime::ConnectorRouterProxy {
    async fn route(
        &self,
        request: fruntime::RouteRequest,
        instance_token: zx::EventPair,
        event_pair: zx::EventPair,
    ) -> Result<Result<fruntime::RouterResponse, i32>, fidl::Error> {
        self.route(&request, instance_token, event_pair).await
    }
}

#[async_trait]
impl RemoteRoutable for fruntime::DirConnectorRouterProxy {
    async fn route(
        &self,
        request: fruntime::RouteRequest,
        instance_token: zx::EventPair,
        event_pair: zx::EventPair,
    ) -> Result<Result<fruntime::RouterResponse, i32>, fidl::Error> {
        self.route(&request, instance_token, event_pair).await
    }
}

#[async_trait]
impl RemoteRoutable for fruntime::DictionaryRouterProxy {
    async fn route(
        &self,
        request: fruntime::RouteRequest,
        instance_token: zx::EventPair,
        event_pair: zx::EventPair,
    ) -> Result<Result<fruntime::RouterResponse, i32>, fidl::Error> {
        self.route(&request, instance_token, event_pair).await
    }
}

#[async_trait]
impl RemoteRoutable for fruntime::DataRouterProxy {
    async fn route(
        &self,
        request: fruntime::RouteRequest,
        instance_token: zx::EventPair,
        event_pair: zx::EventPair,
    ) -> Result<Result<fruntime::RouterResponse, i32>, fidl::Error> {
        self.route(&request, instance_token, event_pair).await
    }
}

fn capability_as_type(cap: &Capability) -> Result<fruntime::CapabilityType, Error> {
    match cap {
        Capability::Connector(_) => Ok(fruntime::CapabilityType::Connector),
        Capability::DirConnector(_) => Ok(fruntime::CapabilityType::DirConnector),
        Capability::Dictionary(_) => Ok(fruntime::CapabilityType::Dictionary),
        Capability::Data(_) => Ok(fruntime::CapabilityType::Data),
        Capability::ConnectorRouter(_) => Ok(fruntime::CapabilityType::ConnectorRouter),
        Capability::DirConnectorRouter(_) => Ok(fruntime::CapabilityType::DirConnectorRouter),
        Capability::DictionaryRouter(_) => Ok(fruntime::CapabilityType::DictionaryRouter),
        Capability::DataRouter(_) => Ok(fruntime::CapabilityType::DataRouter),
        Capability::Instance(_) => Ok(fruntime::CapabilityType::InstanceToken),
        other_value => Err(format_err!("unexpected capability type: {other_value:?}")),
    }
}

async fn handle_key_iterator_stream(
    dictionary: Arc<Dictionary>,
    mut stream: fruntime::DictionaryKeyIteratorRequestStream,
) {
    fn round_up_to_nearest_8(num: usize) -> usize {
        num + 7 & !7
    }

    let mut dictionary_iterator = dictionary.snapshot_keys_as_strings().into_iter().peekable();

    while let Some(Ok(request)) = stream.next().await {
        match request {
            fruntime::DictionaryKeyIteratorRequest::GetNext { responder, .. } => {
                let mut next_elements = vec![];
                let mut bytes_used: usize = FIDL_VECTOR_OVERHEAD;
                while let Some(next_element) = dictionary_iterator.peek() {
                    // A FIDL string takes up the number of bytes of the string rounded up to
                    // the nearest 8 plus the overhead size for the type.
                    bytes_used += FIDL_STRING_OVERHEAD;
                    // String::len returns number of bytes, not characters.
                    bytes_used += round_up_to_nearest_8(next_element.len());
                    if bytes_used > ZX_CHANNEL_MAX_MSG_BYTES as usize {
                        break;
                    }
                    next_elements.push(dictionary_iterator.next().unwrap());
                }
                let _ = responder.send(&next_elements);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use vfs::ExecutionScope;
    use zx::AsHandleRef;

    fn new_connection()
    -> (fruntime::CapabilitiesProxy, Arc<RemotedRuntimeCapabilities>, ExecutionScope) {
        let scope = ExecutionScope::new();
        let remote_capabilities = Arc::new(RemotedRuntimeCapabilities::new());
        let proxy = secondary_connection(&scope, &remote_capabilities);
        (proxy, remote_capabilities, scope)
    }

    fn secondary_connection(
        scope: &ExecutionScope,
        remote_capabilities: &Arc<RemotedRuntimeCapabilities>,
    ) -> fruntime::CapabilitiesProxy {
        let capabilities = Capabilities {
            remote_capabilities: remote_capabilities.clone(),
            weak_scope: scope.as_weak(),
            default_target: WeakInstanceToken::new_invalid(),
            moniker: Moniker::root(),
        };
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fruntime::CapabilitiesMarker>();
        scope.spawn(async move { capabilities.handle_stream(stream).await.unwrap() });
        proxy
    }

    async fn create_connector(
        proxy: &fruntime::CapabilitiesProxy,
    ) -> (zx::EventPair, fruntime::ReceiverRequestStream) {
        let (receiver_client_end, receiver_stream) =
            fidl::endpoints::create_request_stream::<fruntime::ReceiverMarker>();
        let (connector, connector_other_end) = zx::EventPair::create();
        proxy.connector_create(connector_other_end, receiver_client_end).await.unwrap().unwrap();
        (connector, receiver_stream)
    }

    async fn create_dir_connector(
        proxy: &fruntime::CapabilitiesProxy,
    ) -> (zx::EventPair, fruntime::DirReceiverRequestStream) {
        let (dir_receiver_client_end, dir_receiver_stream) =
            fidl::endpoints::create_request_stream::<fruntime::DirReceiverMarker>();
        let (dir_connector, dir_connector_other_end) = zx::EventPair::create();
        proxy
            .dir_connector_create(dir_connector_other_end, dir_receiver_client_end)
            .await
            .unwrap()
            .unwrap();
        (dir_connector, dir_receiver_stream)
    }

    async fn test_connector_is_connected(
        proxy: &fruntime::CapabilitiesProxy,
        connector: &zx::EventPair,
        receiver_stream: &mut fruntime::ReceiverRequestStream,
    ) {
        let (c1, c2) = zx::Channel::create();
        proxy
            .connector_open(connector.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(), c1)
            .await
            .unwrap()
            .unwrap();
        let received_channel = match receiver_stream.next().await {
            Some(Ok(fruntime::ReceiverRequest::Receive { channel, .. })) => channel,
            other_message => panic!("unexpected message: {other_message:?}"),
        };
        assert_eq!(
            c2.basic_info().unwrap().koid,
            received_channel.basic_info().unwrap().related_koid
        );
    }

    async fn test_dir_connector_is_connected(
        proxy: &fruntime::CapabilitiesProxy,
        dir_connector: &zx::EventPair,
        dir_receiver_stream: &mut fruntime::DirReceiverRequestStream,
    ) {
        let (client_end, server_end) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
        proxy
            .dir_connector_open(fruntime::CapabilitiesDirConnectorOpenRequest {
                dir_connector: Some(
                    dir_connector.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                ),
                channel: Some(server_end),
                ..Default::default()
            })
            .await
            .unwrap()
            .unwrap();
        let received_server_end = match dir_receiver_stream.next().await {
            Some(Ok(fruntime::DirReceiverRequest::Receive { channel, .. })) => channel,
            other_message => panic!("unexpected message: {other_message:?}"),
        };
        assert_eq!(
            client_end.as_handle_ref().basic_info().unwrap().koid,
            received_server_end.as_handle_ref().basic_info().unwrap().related_koid
        );
    }

    async fn assert_no_remote_capabilities(remoted_capabilities: &RemotedRuntimeCapabilities) {
        // Cleanup happens asynchronously and there's no way for us to block on it from here, so we
        // have to rely on a timer to wait for cleanup to happen.
        fuchsia_async::Timer::new(std::time::Duration::from_secs(1)).await;
        assert_eq!(0, remoted_capabilities.remote_capabilities.lock().len());
    }

    #[fuchsia::test]
    async fn create_connector_test() {
        let (proxy, remote_capabilities, _scope) = new_connection();
        let (connector, mut receiver_stream) = create_connector(&proxy).await;

        test_connector_is_connected(&proxy, &connector, &mut receiver_stream).await;
        assert_matches!(remote_capabilities.get::<Connector>(connector), Ok(_));
        assert_no_remote_capabilities(&remote_capabilities).await;
    }

    #[fuchsia::test]
    async fn create_dir_connector_test() {
        let (proxy, remote_capabilities, _scope) = new_connection();
        let (dir_connector, mut dir_receiver_stream) = create_dir_connector(&proxy).await;

        test_dir_connector_is_connected(&proxy, &dir_connector, &mut dir_receiver_stream).await;
        assert_matches!(remote_capabilities.get::<DirConnector>(dir_connector), Ok(_));
        assert_no_remote_capabilities(&remote_capabilities).await;
    }

    #[fuchsia::test]
    async fn create_dictionary_test() {
        let (proxy, remote_capabilities, _scope) = new_connection();
        let (dictionary, dictionary_other_end) = zx::EventPair::create();
        proxy.dictionary_create(dictionary_other_end).await.unwrap().unwrap();

        assert_matches!(remote_capabilities.get::<Dictionary>(dictionary), Ok(_));
        assert_no_remote_capabilities(&remote_capabilities).await;
    }

    #[fuchsia::test]
    async fn insert_get_remove_dictionary_test() {
        let (proxy, remote_capabilities, _scope) = new_connection();
        let (dictionary, dictionary_other_end) = zx::EventPair::create();
        proxy.dictionary_create(dictionary_other_end).await.unwrap().unwrap();

        let (data, data_other_end) = zx::EventPair::create();
        proxy.data_create(data_other_end, &fruntime::Data::Int64(1)).await.unwrap().unwrap();

        proxy
            .dictionary_insert(
                dictionary.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                "a",
                data,
            )
            .await
            .unwrap()
            .unwrap();

        let (data_2, data_2_other_end) = zx::EventPair::create();
        let capability_type = proxy
            .dictionary_get(
                dictionary.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                "a",
                data_2_other_end,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(capability_type, fruntime::CapabilityType::Data);
        assert_eq!(proxy.data_get(data_2).await.unwrap(), Ok(fruntime::Data::Int64(1)),);

        let (data_3, data_3_other_end) = zx::EventPair::create();
        let capability_type_2 = proxy
            .dictionary_remove(fruntime::CapabilitiesDictionaryRemoveRequest {
                dictionary: Some(dictionary.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
                key: Some("a".to_string()),
                value: Some(data_3_other_end),
                ..Default::default()
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(capability_type_2, fruntime::CapabilityType::Data);
        assert_eq!(proxy.data_get(data_3).await.unwrap(), Ok(fruntime::Data::Int64(1)),);
        drop(dictionary);
        assert_no_remote_capabilities(&remote_capabilities).await;
    }

    #[fuchsia::test]
    async fn iterate_keys_dictionary_test() {
        let (proxy, _remote_capabilities, _scope) = new_connection();
        let (dictionary, dictionary_other_end) = zx::EventPair::create();
        proxy.dictionary_create(dictionary_other_end).await.unwrap().unwrap();

        let mut expected_dictionary_contents = vec![];
        // We create enough dictionary entries to be sure that they can't be given back to us in a
        // single message.
        for i in 0..(ZX_CHANNEL_MAX_MSG_BYTES / 100) {
            // formats the number as hex with up to 98 leading 0s. The formatter also prepends
            // "0x", meaning this is always 100 characters long (the max name size)
            let name = format!("{i:#098x}");
            let (data, data_other_end) = zx::EventPair::create();
            proxy.data_create(data_other_end, &fruntime::Data::Int64(1)).await.unwrap().unwrap();
            proxy
                .dictionary_insert(
                    dictionary.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                    &name,
                    data,
                )
                .await
                .unwrap()
                .unwrap();
            expected_dictionary_contents.push(name);
        }
        let (iterator_proxy, iterator_server_end) =
            fidl::endpoints::create_proxy::<fruntime::DictionaryKeyIteratorMarker>();
        proxy
            .dictionary_iterate_keys(
                dictionary.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                iterator_server_end,
            )
            .await
            .unwrap()
            .unwrap();
        let mut actual_dictionary_contents = vec![];
        loop {
            let mut next_elements = iterator_proxy.get_next().await.unwrap();
            if next_elements.is_empty() {
                break;
            }
            actual_dictionary_contents.append(&mut next_elements);
        }
        expected_dictionary_contents.sort_unstable();
        actual_dictionary_contents.sort_unstable();
        assert_eq!(expected_dictionary_contents, actual_dictionary_contents);
    }

    #[fuchsia::test]
    async fn connector_router_test() {
        let (proxy, remote_capabilities, scope) = new_connection();
        let (router_client_end, mut router_stream) =
            fidl::endpoints::create_request_stream::<fruntime::ConnectorRouterMarker>();
        let (router, router_other_end) = zx::EventPair::create();
        proxy.connector_router_create(router_other_end, router_client_end).await.unwrap().unwrap();

        let (instance_token, instance_token_other_end) = zx::EventPair::create();
        proxy.instance_token_create(instance_token_other_end).await.unwrap().unwrap();

        let (connector, connector_other_end) = zx::EventPair::create();
        let success_route_fut = proxy.connector_router_route(
            router.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            &Default::default(),
            instance_token,
            connector_other_end,
        );
        let mut receiver_stream = match router_stream.next().await {
            Some(Ok(fruntime::ConnectorRouterRequest::Route { handle, responder, .. })) => {
                // We can't make more calls on the first proxy until the route call completes (FIDL
                // calls occur serially on a given channel), so we need a second connection here to
                // call `connector_create`.
                let proxy_2 = secondary_connection(&scope, &remote_capabilities);
                let (receiver_client_end, receiver_request_stream) =
                    fidl::endpoints::create_request_stream::<fruntime::ReceiverMarker>();
                proxy_2.connector_create(handle, receiver_client_end).await.unwrap().unwrap();
                responder.send(Ok(fruntime::RouterResponse::Success)).unwrap();
                receiver_request_stream
            }
            other_message => panic!("unexpected message: {other_message:?}"),
        };
        assert_eq!(Ok(fruntime::RouterResponse::Success), success_route_fut.await.unwrap());
        test_connector_is_connected(&proxy, &connector, &mut receiver_stream).await;
        assert_matches!(remote_capabilities.get::<Connector>(connector), Ok(_));
        drop(router);
        assert_no_remote_capabilities(&remote_capabilities).await;
    }

    #[fuchsia::test]
    async fn dir_connector_router_test() {
        let (proxy, remote_capabilities, scope) = new_connection();
        let (router_client_end, mut router_stream) =
            fidl::endpoints::create_request_stream::<fruntime::DirConnectorRouterMarker>();
        let (router, router_other_end) = zx::EventPair::create();
        proxy
            .dir_connector_router_create(router_other_end, router_client_end)
            .await
            .unwrap()
            .unwrap();

        let (instance_token, instance_token_other_end) = zx::EventPair::create();
        proxy.instance_token_create(instance_token_other_end).await.unwrap().unwrap();

        let (dir_connector, dir_connector_other_end) = zx::EventPair::create();
        let success_route_fut = proxy.dir_connector_router_route(
            router.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            &Default::default(),
            instance_token,
            dir_connector_other_end,
        );
        let mut receiver_stream = match router_stream.next().await {
            Some(Ok(fruntime::DirConnectorRouterRequest::Route { handle, responder, .. })) => {
                // We can't make more calls on the first proxy until the route call completes (FIDL
                // calls occur serially on a given channel), so we need a second connection here to
                // call `dir_connector_create`.
                let proxy_2 = secondary_connection(&scope, &remote_capabilities);
                let (receiver_client_end, receiver_request_stream) =
                    fidl::endpoints::create_request_stream::<fruntime::DirReceiverMarker>();
                proxy_2.dir_connector_create(handle, receiver_client_end).await.unwrap().unwrap();
                responder.send(Ok(fruntime::RouterResponse::Success)).unwrap();
                receiver_request_stream
            }
            other_message => panic!("unexpected message: {other_message:?}"),
        };
        assert_eq!(Ok(fruntime::RouterResponse::Success), success_route_fut.await.unwrap());
        test_dir_connector_is_connected(&proxy, &dir_connector, &mut receiver_stream).await;
        assert_matches!(remote_capabilities.get::<DirConnector>(dir_connector), Ok(_));
        drop(router);
        assert_no_remote_capabilities(&remote_capabilities).await;
    }

    #[fuchsia::test]
    async fn dictionary_router_test() {
        let (proxy, remote_capabilities, scope) = new_connection();
        let (router_client_end, mut router_stream) =
            fidl::endpoints::create_request_stream::<fruntime::DictionaryRouterMarker>();
        let (router, router_other_end) = zx::EventPair::create();
        proxy.dictionary_router_create(router_other_end, router_client_end).await.unwrap().unwrap();

        let (instance_token, instance_token_other_end) = zx::EventPair::create();
        proxy.instance_token_create(instance_token_other_end).await.unwrap().unwrap();

        let (dictionary, dictionary_other_end) = zx::EventPair::create();
        let success_route_fut = proxy.dictionary_router_route(
            router.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            &Default::default(),
            instance_token,
            dictionary_other_end,
        );
        match router_stream.next().await {
            Some(Ok(fruntime::DictionaryRouterRequest::Route { handle, responder, .. })) => {
                // We can't make more calls on the first proxy until the route call completes (FIDL
                // calls occur serially on a given channel), so we need a second connection here to
                // call `dictionary_create`.
                let proxy_2 = secondary_connection(&scope, &remote_capabilities);
                proxy_2.dictionary_create(handle).await.unwrap().unwrap();
                responder.send(Ok(fruntime::RouterResponse::Success)).unwrap();
            }
            other_message => panic!("unexpected message: {other_message:?}"),
        }
        assert_eq!(Ok(fruntime::RouterResponse::Success), success_route_fut.await.unwrap());

        assert_matches!(remote_capabilities.get::<Dictionary>(dictionary), Ok(_));
        drop(router);
        assert_no_remote_capabilities(&remote_capabilities).await;
    }

    #[fuchsia::test]
    async fn data_router_test() {
        let (proxy, remote_capabilities, scope) = new_connection();
        let (router_client_end, mut router_stream) =
            fidl::endpoints::create_request_stream::<fruntime::DataRouterMarker>();
        let (router, router_other_end) = zx::EventPair::create();
        proxy.data_router_create(router_other_end, router_client_end).await.unwrap().unwrap();

        let (instance_token, instance_token_other_end) = zx::EventPair::create();
        proxy.instance_token_create(instance_token_other_end).await.unwrap().unwrap();

        let (data, data_other_end) = zx::EventPair::create();
        let success_route_fut = proxy.data_router_route(
            router.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            &Default::default(),
            instance_token,
            data_other_end,
        );
        match router_stream.next().await {
            Some(Ok(fruntime::DataRouterRequest::Route { handle, responder, .. })) => {
                // We can't make more calls on the first proxy until the route call completes (FIDL
                // calls occur serially on a given channel), so we need a second connection here to
                // call `dictionary_create`.
                let proxy_2 = secondary_connection(&scope, &remote_capabilities);
                proxy_2.data_create(handle, &fruntime::Data::Int64(1)).await.unwrap().unwrap();
                responder.send(Ok(fruntime::RouterResponse::Success)).unwrap();
            }
            other_message => panic!("unexpected message: {other_message:?}"),
        };
        assert_eq!(Ok(fruntime::RouterResponse::Success), success_route_fut.await.unwrap());

        assert_matches!(remote_capabilities.get::<Data>(data), Ok(_));
        drop(router);
        assert_no_remote_capabilities(&remote_capabilities).await;
    }

    #[fuchsia::test]
    async fn connector_router_debug_route_test() {
        let (proxy, remote_capabilities, _scope) = new_connection();
        let (router_client_end, _router_stream) =
            fidl::endpoints::create_request_stream::<fruntime::ConnectorRouterMarker>();
        let (router, router_other_end) = zx::EventPair::create();
        proxy.connector_router_create(router_other_end, router_client_end).await.unwrap().unwrap();

        let router: Arc<Router<Connector>> = remote_capabilities.get(router).unwrap();

        let capability_source = router
            .route_debug(fruntime::RouteRequest::default(), WeakInstanceToken::new_invalid())
            .await
            .expect("unexpected response from router");
        assert_eq!(
            capability_source,
            CapabilitySource::RemotedAt(RemotedAtSource {
                moniker: Moniker::root(),
                type_name: None,
            })
        );

        drop(router);
        assert_no_remote_capabilities(&remote_capabilities).await;
    }

    #[fuchsia::test]
    async fn connector_router_receiver_closed_route_test() {
        let (proxy, remote_capabilities, _scope) = new_connection();
        let (router_client_end, router_stream) =
            fidl::endpoints::create_request_stream::<fruntime::ConnectorRouterMarker>();
        let (router, router_other_end) = zx::EventPair::create();
        proxy.connector_router_create(router_other_end, router_client_end).await.unwrap().unwrap();

        let router: Arc<Router<Connector>> = remote_capabilities.get(router).unwrap();
        drop(router_stream);

        let router_err = match router
            .route(fruntime::RouteRequest::default(), WeakInstanceToken::new_invalid())
            .await
        {
            Ok(val) => panic!("unexpected success: {val:?}"),
            Err(e) => e,
        };

        let routing_error = RoutingError::try_from(router_err).unwrap();

        assert_eq!(routing_error, RoutingError::RemoteFIDLError { moniker: Moniker::root() });

        drop(router);
        assert_no_remote_capabilities(&remote_capabilities).await;
    }
}
