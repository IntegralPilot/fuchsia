// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements fuchsia.net.policy.socketproxy.NetworkRegistry.

use anyhow::{Context, Error, anyhow};
use fidl::endpoints::RequestStream;
use fidl_fuchsia_net_policy_socketproxy::{
    self as fnp_socketproxy, Network, NetworkInfo, NetworkRegistryAddError,
    NetworkRegistryRemoveError, NetworkRegistrySetDefaultError, NetworkRegistryUpdateError,
    StarnixNetworksRequest,
};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_inspect_derive::{IValue, Inspect, Unit};
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{SinkExt as _, StreamExt as _, TryStreamExt as _};
use log::{error, info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_posix_socket as fposix_socket;

/// If there are networks registered, but no default has been set, this value
/// will be used, otherwise the mark will be OptionalUint32::Unset(Empty).
pub(crate) const DEFAULT_SOCKET_MARK: u32 = 0;

pub(crate) struct RequestForwarder {
    forwarder_rx: mpsc::Receiver<NetworkRegistryRequest>,
    registry: Option<fnp_socketproxy::NetworkRegistryProxy>,
}

impl RequestForwarder {
    pub(crate) fn new(
        forwarder_rx: mpsc::Receiver<NetworkRegistryRequest>,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self { forwarder_rx, registry: None })
    }

    fn get_registry(&mut self) -> Result<&fnp_socketproxy::NetworkRegistryProxy, anyhow::Error> {
        match &self.registry {
            Some(registry) => Ok(registry),
            None => {
                let registry = connect_to_protocol::<fnp_socketproxy::NetworkRegistryMarker>()
                    .context("error connecting to network registry")?;
                Ok(self.registry.insert(registry))
            }
        }
    }

    pub(crate) async fn run(&mut self) -> Result<(), anyhow::Error> {
        while let Some(request) = self.forwarder_rx.next().await {
            match self.forward_request(request).await {
                Err(e) => {
                    info!(
                        "FIDL error while forwarding request to delegated networks. \
                            Protocol likely not available: {e:?}"
                    );
                    return Err(e);
                }
                Ok(Err(e)) => {
                    error!(
                        "Failed to forward request to delegated networks. Future updates \
                                 will be ignored, and netcfg will not have complete state: {e:?}"
                    );
                    return Err(anyhow!("Delegated registry has inconsistent state: {e:?}"));
                }
                Ok(Ok(_)) => continue,
            }
        }
        error!(
            "RequestForwarder stream ended. No more requests will be \
            forwarded to netcfg"
        );
        Ok(())
    }

    async fn forward_request(
        &mut self,
        request: NetworkRegistryRequest,
    ) -> Result<Result<(), NetworkRegistryError>, anyhow::Error> {
        info!("forwarding Starnix NetworkRegistry change to netcfg: {request:?}");
        let registry = self.get_registry()?;
        let res = match request {
            NetworkRegistryRequest::SetDefault { network_id } => registry
                .set_default(&match network_id {
                    Some(id) => fposix_socket::OptionalUint32::Value(id),
                    None => fposix_socket::OptionalUint32::Unset(fposix_socket::Empty),
                })
                .await
                .context("fidl error forwarding set_default")?
                .map_err(|e| e.into()),
            NetworkRegistryRequest::Add { network } => registry
                .add(&network)
                .await
                .context("fidl error forwarding add")?
                .map_err(|e| e.into()),
            NetworkRegistryRequest::Update { network } => registry
                .update(&network)
                .await
                .context("fidl error forwarding update")?
                .map_err(|e| e.into()),
            NetworkRegistryRequest::Remove { network_id } => registry
                .remove(network_id)
                .await
                .context("fidl error forwarding remove")?
                .map_err(|e| e.into()),
        };
        Ok(res)
    }
}

enum CommonErrors {
    MissingNetworkId,
    MissingNetworkInfo,
}

trait NetworkInfoExt {
    fn mark(&self) -> Option<u32>;
}

impl NetworkInfoExt for NetworkInfo {
    fn mark(&self) -> Option<u32> {
        match self {
            NetworkInfo::Starnix(s) => s.mark,
            // Sockets express using Fuchsia's default network by setting
            // the mark to None.
            NetworkInfo::Fuchsia(_) | _ => None,
        }
    }
}

// Errors produced when communicating updates to
// the socket proxy.
#[derive(Clone, Debug, Error)]
pub enum NetworkRegistryError {
    #[error("Error during socketproxy Add: {0:?}")]
    Add(NetworkRegistryAddError),
    #[error("Error during socketproxy Remove: {0:?}")]
    Remove(NetworkRegistryRemoveError),
    #[error("Error during socketproxy SetDefault: {0:?}")]
    SetDefault(NetworkRegistrySetDefaultError),
    #[error("Error during socketproxy Update: {0:?}")]
    Update(NetworkRegistryUpdateError),
}

impl From<NetworkRegistryAddError> for NetworkRegistryError {
    fn from(error: NetworkRegistryAddError) -> Self {
        NetworkRegistryError::Add(error)
    }
}

impl From<NetworkRegistryRemoveError> for NetworkRegistryError {
    fn from(error: NetworkRegistryRemoveError) -> Self {
        NetworkRegistryError::Remove(error)
    }
}

impl From<NetworkRegistrySetDefaultError> for NetworkRegistryError {
    fn from(error: NetworkRegistrySetDefaultError) -> Self {
        NetworkRegistryError::SetDefault(error)
    }
}

impl From<NetworkRegistryUpdateError> for NetworkRegistryError {
    fn from(error: NetworkRegistryUpdateError) -> Self {
        NetworkRegistryError::Update(error)
    }
}

#[derive(Debug, Clone, PartialEq)]
/// A generic version of a NetworkRegistry request with the responder removed.
pub enum NetworkRegistryRequest {
    /// Sets the default network.
    ///
    /// The network must have previously been registered by a call to `Add`.
    SetDefault { network_id: Option<u32> },
    /// Add a new network.
    ///
    /// This call will not return until the DNS servers have been successfully
    /// updated in netcfg.
    Add { network: Network },
    /// Update a previously Added network.
    ///
    /// This call will not return until the DNS servers have been
    /// successfully updated in netcfg.
    Update { network: Network },
    /// Remove a previously Added network.
    ///
    /// This call will not return until the DNS servers have been
    /// successfully updated in netcfg.
    Remove { network_id: u32 },
}

impl From<&fnp_socketproxy::NetworkRegistryRequest> for NetworkRegistryRequest {
    fn from(value: &fnp_socketproxy::NetworkRegistryRequest) -> Self {
        match *value {
            fnp_socketproxy::NetworkRegistryRequest::SetDefault { network_id, responder: _ } => {
                NetworkRegistryRequest::SetDefault {
                    network_id: match network_id {
                        fposix_socket::OptionalUint32::Value(v) => Some(v),
                        fposix_socket::OptionalUint32::Unset(_) => None,
                    },
                }
            }
            fnp_socketproxy::NetworkRegistryRequest::Add { ref network, responder: _ } => {
                NetworkRegistryRequest::Add { network: network.clone() }
            }
            fnp_socketproxy::NetworkRegistryRequest::Update { ref network, responder: _ } => {
                NetworkRegistryRequest::Update { network: network.clone() }
            }
            fnp_socketproxy::NetworkRegistryRequest::Remove { network_id, responder: _ } => {
                NetworkRegistryRequest::Remove { network_id }
            }
        }
    }
}
impl From<&StarnixNetworksRequest> for NetworkRegistryRequest {
    fn from(value: &StarnixNetworksRequest) -> Self {
        match *value {
            StarnixNetworksRequest::SetDefault { network_id, responder: _ } => {
                NetworkRegistryRequest::SetDefault {
                    network_id: match network_id {
                        fposix_socket::OptionalUint32::Value(v) => Some(v),
                        fposix_socket::OptionalUint32::Unset(_) => None,
                    },
                }
            }
            StarnixNetworksRequest::Add { ref network, responder: _ } => {
                NetworkRegistryRequest::Add { network: network.clone() }
            }
            StarnixNetworksRequest::Update { ref network, responder: _ } => {
                NetworkRegistryRequest::Update { network: network.clone() }
            }
            StarnixNetworksRequest::Remove { network_id, responder: _ } => {
                NetworkRegistryRequest::Remove { network_id }
            }
        }
    }
}

/// A copy of fnp_socketproxy::Network that ensures that all fields are present.
#[derive(Debug, Clone)]
pub(crate) struct ValidatedNetwork {
    network_id: u32,
    info: NetworkInfo,
}

trait ValidateNetworkExt {
    fn validate(self) -> Result<ValidatedNetwork, CommonErrors>;
}

impl ValidateNetworkExt for Network {
    fn validate(self) -> Result<ValidatedNetwork, CommonErrors> {
        match self {
            Network { network_id: None, .. } => Err(CommonErrors::MissingNetworkId),
            Network { info: None, .. } => Err(CommonErrors::MissingNetworkInfo),
            Network { network_id: Some(network_id), info: Some(info), .. } => {
                Ok(ValidatedNetwork { network_id, info })
            }
        }
    }
}

macro_rules! common_errors_impl {
    ($($p:ty),+) => {
        $(
            impl From<CommonErrors> for $p {
                fn from(value: CommonErrors) -> Self {
                    use CommonErrors::*;
                    match value {
                        MissingNetworkId => <$p>::MissingNetworkId,
                        MissingNetworkInfo => <$p>::MissingNetworkInfo,
                    }
                }
            }
        )+
    }
}

common_errors_impl!(
    fnp_socketproxy::NetworkRegistryAddError,
    fnp_socketproxy::NetworkRegistryUpdateError
);

/// NetworkRegistry tracks the networks that have been registered.
#[derive(Inspect, Debug, Default)]
struct NetworkRegistry {
    networks: IValue<RegisteredNetworks>,

    inspect_node: fuchsia_inspect::Node,
}

impl NetworkRegistry {
    /// Returns current socket mark for the default network.
    pub(crate) fn current_mark(&self) -> Option<u32> {
        self.networks.current_mark()
    }
}

#[derive(Unit, Debug, Default)]
struct MethodInspect {
    successes: u32,
    errors: u32,
}

#[derive(Unit, Default, Debug)]
struct RegisteredNetworks {
    default_network_id: Option<u32>,

    #[inspect(skip)]
    /// A mapping from network id to ValidatedNetwork for each registered network.
    networks: HashMap<u32, ValidatedNetwork>,

    adds: MethodInspect,
    removes: MethodInspect,
    set_defaults: MethodInspect,
    updates: MethodInspect,
}

impl RegisteredNetworks {
    fn add_network(&mut self, network: Network) -> fnp_socketproxy::NetworkRegistryAddResult {
        let network = network.validate()?;
        #[allow(clippy::map_entry, reason = "mass allow for https://fxbug.dev/381896734")]
        if self.networks.contains_key(&network.network_id) {
            self.adds.errors += 1;
            Err(fnp_socketproxy::NetworkRegistryAddError::DuplicateNetworkId)
        } else {
            let _: Option<_> = self.networks.insert(network.network_id, network);
            self.adds.successes += 1;
            Ok(())
        }
    }

    /// Empties the registered networks.
    pub(crate) fn clear(&mut self) {
        self.default_network_id = None;
        self.networks.clear();
    }

    fn update_network(&mut self, network: Network) -> fnp_socketproxy::NetworkRegistryUpdateResult {
        let network = network.validate()?;
        let network_id = network.network_id;
        *self
            .networks
            .get_mut(&network_id)
            .ok_or(fnp_socketproxy::NetworkRegistryUpdateError::NotFound)
            .inspect(|_| self.updates.successes += 1)
            .inspect_err(|_| self.updates.errors += 1)? = network;
        Ok(())
    }

    fn remove_network(&mut self, network_id: u32) -> fnp_socketproxy::NetworkRegistryRemoveResult {
        if self.default_network_id == Some(network_id) {
            self.removes.errors += 1;
            return Err(fnp_socketproxy::NetworkRegistryRemoveError::CannotRemoveDefaultNetwork);
        }
        match self.networks.remove(&network_id) {
            Some(_) => {
                self.removes.successes += 1;
                Ok(())
            }
            None => {
                self.removes.errors += 1;
                Err(fnp_socketproxy::NetworkRegistryRemoveError::NotFound)
            }
        }
    }

    /// Update the currently set default network id.
    ///
    /// If `network_id` is None, the default network id will be unset.
    fn set_default_network(
        &mut self,
        network_id: Option<u32>,
    ) -> fnp_socketproxy::NetworkRegistrySetDefaultResult {
        if let Some(network_id) = network_id {
            if !self.networks.contains_key(&network_id) {
                self.set_defaults.errors += 1;
                return Err(fnp_socketproxy::NetworkRegistrySetDefaultError::NotFound);
            }
        }
        self.set_defaults.successes += 1;
        self.default_network_id = network_id;

        Ok(())
    }

    fn current_mark(&self) -> Option<u32> {
        match (self.default_network_id, self.networks.is_empty()) {
            (None, false) => Some(DEFAULT_SOCKET_MARK),
            (id, _) => id.and_then(|id| self.networks[&id].info.mark()),
        }
    }

    fn len(&self) -> usize {
        self.networks.len()
    }
}

/// The default network state reported by Netcfg via `Networks.WatchDefault`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum NetcfgMarkState {
    /// Netcfg has no default network; fall back to Starnix.
    #[default]
    NoDefault,
    /// Netcfg has an active default network with the given mark (None = unmarked).
    Default(Option<u32>),
}

#[derive(Inspect, Clone, Debug, Default)]
pub struct NetworkRegistries {
    starnix: Arc<Mutex<NetworkRegistry>>,
    #[inspect(skip)]
    netcfg: Arc<Mutex<NetcfgMarkState>>,
}

impl NetworkRegistries {
    // Precedence order for socket mark resolution:
    // 1. Netcfg `WatchDefault` property mark (the primary source of truth).
    // 2. Starnix registry (fallback if Netcfg mark is unset).
    async fn current_mark(&self) -> Option<u32> {
        {
            let netcfg = self.netcfg.lock().await;
            match *netcfg {
                NetcfgMarkState::Default(mark) => return mark,
                NetcfgMarkState::NoDefault => {}
            }
        }
        self.starnix.lock().await.current_mark()
    }
}

#[derive(Inspect, Debug)]
pub struct Registry {
    #[inspect(forward)]
    networks: NetworkRegistries,
    // Reflects the marks that are set on the sockets vended
    // by this component.
    marks: Arc<Mutex<crate::SocketMarks>>,
    forwarder_tx: mpsc::Sender<NetworkRegistryRequest>,
    starnix_occupant: Mutex<()>,
}

impl Registry {
    pub(crate) fn new(
        marks: Arc<Mutex<crate::SocketMarks>>,
        forwarder_tx: mpsc::Sender<NetworkRegistryRequest>,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self {
            networks: Default::default(),
            marks,
            forwarder_tx,
            starnix_occupant: Default::default(),
        })
    }
}

impl Registry {
    pub(crate) async fn run_starnix(
        &self,
        stream: fnp_socketproxy::StarnixNetworksRequestStream,
    ) -> Result<(), Error> {
        let _occupant = match self.starnix_occupant.try_lock() {
            Some(o) => o,
            None => {
                warn!("Only one connection to StarnixNetworks is allowed at a time");
                stream.control_handle().shutdown_with_epitaph(fidl::Status::ACCESS_DENIED);
                return Ok(());
            }
        };

        info!("Starting fuchsia.net.policy.socketproxy.StarnixNetworks server");
        self.networks.starnix.lock().await.networks.as_mut().clear();
        stream
            .map(|result| result.context("failed request"))
            .try_for_each(|request| {
                async {
                    self.forwarder_tx.clone().feed((&request).into()).await.unwrap_or_else(|e| {
                        if !e.is_disconnected() {
                            // Log if the feed fails for reasons other than disconnection.
                            error!("Unable to feed request forward: {e:?}")
                        }
                    });
                    let mut network_registry = self.networks.starnix.lock().await;
                    let mut networks = network_registry.networks.as_mut();
                    let (op, send): (
                        _,
                        Box<dyn FnOnce() -> Result<(), _> + Send + Sync + 'static>,
                    ) = match request {
                        StarnixNetworksRequest::SetDefault { network_id, responder } => {
                            let result = networks.set_default_network(match network_id {
                                fposix_socket::OptionalUint32::Value(value) => Some(value),
                                fposix_socket::OptionalUint32::Unset(_) => None,
                            });
                            ("set default", Box::new(move || responder.send(result)))
                        }
                        StarnixNetworksRequest::Add { network, responder } => {
                            let result = networks.add_network(network);
                            ("add", Box::new(move || responder.send(result)))
                        }
                        StarnixNetworksRequest::Update { network, responder } => {
                            let result = networks.update_network(network);
                            ("update", Box::new(move || responder.send(result)))
                        }
                        StarnixNetworksRequest::Remove { network_id, responder } => {
                            let result = networks.remove_network(network_id);
                            ("remove", Box::new(move || responder.send(result)))
                        }
                    };
                    let new_mark = networks.current_mark();
                    info!(
                        "Starnix registry {op}. mark: {new_mark:?}, networks count: {}",
                        networks.len()
                    );
                    std::mem::drop(networks);
                    std::mem::drop(network_registry);

                    self.handle_state_changed().await;
                    send().context("error sending response")?;
                    Ok(())
                }
            })
            .await
    }

    pub(crate) async fn set_netcfg_mark(&self, state: NetcfgMarkState) {
        *self.networks.netcfg.lock().await = state;
        self.handle_state_changed().await;
    }

    pub(crate) async fn handle_state_changed(&self) {
        // Ensure the mark is updated prior to sending out the response
        // and dropping the registry.
        let mark = self.networks.current_mark().await;
        self.marks.lock().await.set_mark(fnet::MARK_DOMAIN_SO_MARK, mark);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fuchsia_component::server::ServiceFs;
    use fuchsia_component_test::{
        Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmInstance, Ref, Route,
    };
    use futures::channel::mpsc::Receiver;
    use futures::future;
    use net_declare::fidl_ip;
    use pretty_assertions::assert_eq;
    use socket_proxy_testing::ToNetwork;
    use test_case::test_case;

    #[derive(Clone, Debug)]
    enum Op<N: ToNetwork> {
        SetDefault {
            network_id: Option<u32>,
            result: Result<(), fnp_socketproxy::NetworkRegistrySetDefaultError>,
        },
        Add {
            network: N,
            result: Result<(), fnp_socketproxy::NetworkRegistryAddError>,
        },
        Update {
            network: N,
            result: Result<(), fnp_socketproxy::NetworkRegistryUpdateError>,
        },
        Remove {
            network_id: u32,
            result: Result<(), fnp_socketproxy::NetworkRegistryRemoveError>,
        },
    }

    impl<N: ToNetwork + Clone> From<&Op<N>> for NetworkRegistryRequest {
        fn from(value: &Op<N>) -> Self {
            match value {
                Op::SetDefault { network_id, result: _ } => {
                    NetworkRegistryRequest::SetDefault { network_id: *network_id }
                }
                Op::Add { network, result: _ } => {
                    NetworkRegistryRequest::Add { network: network.clone().to_network() }
                }
                Op::Update { network, result: _ } => {
                    NetworkRegistryRequest::Update { network: network.clone().to_network() }
                }
                Op::Remove { network_id, result: _ } => {
                    NetworkRegistryRequest::Remove { network_id: *network_id }
                }
            }
        }
    }

    impl<N: ToNetwork + Clone> Op<N> {
        async fn execute_starnix(
            &self,
            starnix: &fnp_socketproxy::StarnixNetworksProxy,
        ) -> Result<(), Error> {
            match self {
                Op::SetDefault { network_id, result } => {
                    assert_eq!(
                        starnix
                            .set_default(&match network_id {
                                Some(value) => fposix_socket::OptionalUint32::Value(*value),
                                None => fposix_socket::OptionalUint32::Unset(fposix_socket::Empty),
                            })
                            .await?,
                        *result
                    )
                }
                Op::Add { network, result } => {
                    assert_eq!(starnix.add(&network.to_network()).await?, *result)
                }
                Op::Update { network, result } => {
                    assert_eq!(starnix.update(&network.to_network()).await?, *result)
                }
                Op::Remove { network_id, result } => {
                    assert_eq!(starnix.remove(*network_id).await?, *result)
                }
            }
            Ok(())
        }
    }

    enum IncomingService {
        StarnixNetworks(fnp_socketproxy::StarnixNetworksRequestStream),
    }

    async fn run_registry(
        handles: LocalComponentHandles,
        starnix_networks: Arc<Mutex<NetworkRegistry>>,
        marks: Arc<Mutex<crate::SocketMarks>>,
        forwarder_tx: mpsc::Sender<NetworkRegistryRequest>,
    ) -> Result<(), Error> {
        let mut fs = ServiceFs::new();
        let _ = fs.dir("svc").add_fidl_service(IncomingService::StarnixNetworks);
        let _ = fs.serve_connection(handles.outgoing_dir)?;

        let registry = Registry {
            networks: NetworkRegistries { starnix: starnix_networks, netcfg: Default::default() },
            marks,
            forwarder_tx,
            starnix_occupant: Default::default(),
        };

        fs.for_each_concurrent(0, |service| async {
            match service {
                IncomingService::StarnixNetworks(stream) => registry.run_starnix(stream).await,
            }
            .unwrap_or_else(|e| error!("{e:?}"))
        })
        .await;

        Ok(())
    }

    async fn setup_test() -> Result<(RealmInstance, Receiver<NetworkRegistryRequest>), Error> {
        let builder = RealmBuilder::new().await?;
        let starnix_networks = Arc::new(Mutex::new(Default::default()));
        let (forwarder_tx, forwarder_rx) = mpsc::channel(1);
        let marks = Arc::new(Mutex::new(crate::SocketMarks::default()));
        let registry = builder
            .add_local_child(
                "registry",
                {
                    let starnix_networks = starnix_networks.clone();
                    let marks = marks.clone();
                    move |handles: LocalComponentHandles| {
                        Box::pin(run_registry(
                            handles,
                            starnix_networks.clone(),
                            marks.clone(),
                            forwarder_tx.clone(),
                        ))
                    }
                },
                ChildOptions::new(),
            )
            .await?;

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fnp_socketproxy::StarnixNetworksMarker>())
                    .from(&registry)
                    .to(Ref::parent()),
            )
            .await?;

        let realm = builder.build().await?;

        Ok((realm, forwarder_rx))
    }

    #[test_case(&[
        Op::Add { network: 1, result: Ok(()) },
        Op::Update { network: 1, result: Ok(()) },
        Op::Remove { network_id: 1, result: Ok(()) },
    ]; "normal operation")]
    #[test_case(&[
        Op::Add { network: 1, result: Ok(()) },
        Op::Add { network: 1, result: Err(fnp_socketproxy::NetworkRegistryAddError::DuplicateNetworkId) },
    ]; "duplicate add")]
    #[test_case(&[
        Op::Update { network: 1, result: Err(fnp_socketproxy::NetworkRegistryUpdateError::NotFound) },
    ]; "update missing")]
    #[test_case(&[
        Op::<u32>::Remove { network_id: 1, result: Err(fnp_socketproxy::NetworkRegistryRemoveError::NotFound) },
    ]; "remove missing")]
    #[test_case(&[
        Op::<u32>::SetDefault { network_id: Some(1), result: Err(fnp_socketproxy::NetworkRegistrySetDefaultError::NotFound) },
    ]; "set default missing")]
    #[test_case(&[
        Op::Add { network: 1, result: Ok(()) },
        Op::SetDefault { network_id: Some(1), result: Ok(()) },
        Op::Remove { network_id: 1, result: Err(fnp_socketproxy::NetworkRegistryRemoveError::CannotRemoveDefaultNetwork)},
    ]; "remove default network")]
    #[test_case(&[
        Op::Add { network: 1, result: Ok(()) },
        Op::SetDefault { network_id: Some(1), result: Ok(()) },
        Op::Remove { network_id: 1, result: Err(fnp_socketproxy::NetworkRegistryRemoveError::CannotRemoveDefaultNetwork)},
        Op::Add { network: 2, result: Ok(()) },
        Op::SetDefault { network_id: Some(2), result: Ok(()) },
        Op::Remove { network_id: 1, result: Ok(()) },
    ]; "remove formerly default network")]
    #[test_case(&[
        Op::Add { network: 1, result: Ok(()) },
        Op::SetDefault { network_id: Some(1), result: Ok(()) },
        Op::Remove { network_id: 1, result: Err(fnp_socketproxy::NetworkRegistryRemoveError::CannotRemoveDefaultNetwork)},
        Op::SetDefault { network_id: None, result: Ok(()) },
        Op::Remove { network_id: 1, result: Ok(()) },
    ]; "remove last network")]
    #[test_case(&[
        Op::Add { network: 1, result: Ok(()) },
        Op::Update { network: 1, result: Ok(()) },
        Op::Add { network: 2, result: Ok(()) },
        Op::Add { network: 3, result: Ok(()) },
        Op::Add { network: 4, result: Ok(()) },
        Op::Update { network: 4, result: Ok(()) },
        Op::Update { network: 2, result: Ok(()) },
        Op::Update { network: 3, result: Ok(()) },
        Op::Add { network: 5, result: Ok(()) },
        Op::Update { network: 5, result: Ok(()) },
        Op::Add { network: 6, result: Ok(()) },
        Op::Add { network: 7, result: Ok(()) },
        Op::Add { network: 8, result: Ok(()) },
        Op::Update { network: 8, result: Ok(()) },
        Op::Update { network: 6, result: Ok(()) },
        Op::Add { network: 9, result: Ok(()) },
        Op::Update { network: 9, result: Ok(()) },
        Op::Update { network: 7, result: Ok(()) },
        Op::Add { network: 10, result: Ok(()) },
        Op::Update { network: 10, result: Ok(()) },
    ]; "many updates")]
    #[fuchsia::test]
    async fn test_operations<N: ToNetwork + Clone>(operations: &[Op<N>]) -> Result<(), Error> {
        let (realm, _) = setup_test().await?;
        let starnix_networks = realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .context("While connecting to StarnixNetworks")?;
        for op in operations {
            op.execute_starnix(&starnix_networks).await?;
        }

        Ok(())
    }

    #[test_case(&[
        Op::Add { network: (1, vec![fidl_ip!("192.0.2.0")]), result: Ok(()) },
    ]
    ; "Add but no default")]
    #[test_case(&[
        Op::Add { network: (1, vec![fidl_ip!("192.0.2.0")]), result: Ok(()) },
        Op::SetDefault { network_id: Some(1), result: Ok(()) },
    ]
    ; "Add and set default")]
    #[test_case(&[
        Op::Add { network: (1, vec![fidl_ip!("192.0.2.0")]), result: Ok(()) },
        Op::Add { network: (2, vec![fidl_ip!("192.0.2.0")]), result: Ok(()) },
        Op::SetDefault { network_id: Some(1), result: Ok(()) },
        Op::SetDefault { network_id: Some(2), result: Ok(()) },
    ]
    ; "Add two and set default")]
    #[test_case(&[
        Op::Add { network: (1, vec![fidl_ip!("192.0.2.0")]), result: Ok(()) },
        Op::SetDefault { network_id: Some(1), result: Ok(()) },
        Op::SetDefault { network_id: None, result: Ok(()) },
        Op::Remove { network_id: 1, result: Ok(()) },
    ]
    ; "Add default and delete")]
    #[fuchsia::test]
    async fn test_forward_network_update<N: ToNetwork + Clone + std::fmt::Debug>(
        operations: &[Op<N>],
    ) -> Result<(), Error> {
        let (realm, mut forwarder_rx) = setup_test().await?;
        let starnix_networks = realm
            .root
            .connect_to_protocol_at_exposed_dir()
            .context("While connecting to StarnixNetworks")?;

        let (_, seen_updates) = future::join(
            async move {
                for op in operations {
                    op.execute_starnix(&starnix_networks).await?;
                }
                std::mem::drop(realm);
                Ok::<(), anyhow::Error>(())
            },
            async move {
                let mut forwarded_requests = Vec::new();
                while let Some(req) = forwarder_rx.next().await {
                    forwarded_requests.push(req);
                }
                forwarded_requests
            },
        )
        .await;

        let expected_updates =
            operations.iter().map(NetworkRegistryRequest::from).collect::<Vec<_>>();
        assert_eq!(expected_updates, seen_updates);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_mark_resolution_precedence() {
        let starnix = Arc::new(Mutex::new(NetworkRegistry::default()));
        let netcfg = Arc::new(Mutex::new(NetcfgMarkState::NoDefault));

        let registries = NetworkRegistries { starnix: starnix.clone(), netcfg: netcfg.clone() };

        // No networks registered.
        assert_eq!(registries.current_mark().await, None);

        // Starnix default network sets mark.
        starnix.lock().await.networks.as_mut().add_network(1.to_network()).unwrap();
        starnix.lock().await.networks.as_mut().set_default_network(Some(1)).unwrap();
        assert_eq!(registries.current_mark().await, Some(1));

        // Active unmarked Netcfg default network overrides Starnix.
        *netcfg.lock().await = NetcfgMarkState::Default(None);
        assert_eq!(registries.current_mark().await, None);

        // Active marked Netcfg default network overrides Starnix.
        *netcfg.lock().await = NetcfgMarkState::Default(Some(456));
        assert_eq!(registries.current_mark().await, Some(456));

        // Netcfg loses default network; falls back to Starnix.
        *netcfg.lock().await = NetcfgMarkState::NoDefault;
        assert_eq!(registries.current_mark().await, Some(1));
    }
}
