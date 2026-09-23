// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::Info as DeviceInfo;
use crate::sigproc::{Element, ElementState, Topology};
use anyhow::{Context, Error, anyhow};
use async_utils::event::Event as AsyncEvent;
use async_utils::hanging_get::client::HangingGetStream;
use flex_client::ProxyHasDomain;
use flex_fuchsia_audio_device as fadevice;
use fuchsia_async::Task;
use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::lock::Mutex;
use log::{error, warn};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use zx_status::Status;

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    /// A device was added to the registry.
    Added(Box<DeviceInfo>),
    /// A device was removed from the registry.
    Removed(fadevice::TokenId),
}

pub type DeviceEventSender = UnboundedSender<DeviceEvent>;
pub type DeviceEventReceiver = UnboundedReceiver<DeviceEvent>;

pub struct Registry {
    proxy: fadevice::RegistryProxy,
    devices: Arc<Mutex<BTreeMap<fadevice::TokenId, DeviceInfo>>>,
    devices_initialized: AsyncEvent,
    event_senders: Arc<Mutex<Vec<DeviceEventSender>>>,
    _watch_devices_task: Task<()>,
}

impl Registry {
    pub fn new(proxy: fadevice::RegistryProxy) -> Self {
        let devices = Arc::new(Mutex::new(BTreeMap::new()));
        let devices_initialized = AsyncEvent::new();
        let event_senders = Arc::new(Mutex::new(vec![]));

        let watch_devices_task = Task::spawn({
            let proxy = proxy.clone();
            let devices = devices.clone();
            let devices_initialized = devices_initialized.clone();
            let event_senders = event_senders.clone();
            async move {
                // Pass a clone to watch_devices so we can signal the original on error.
                if let Err(err) =
                    watch_devices(proxy, devices, devices_initialized.clone(), event_senders).await
                {
                    warn!(err:%; "Failed to watch Registry devices");
                    // Ensure we don't hang waiters if the watch task fails.
                    devices_initialized.signal();
                }
            }
        });

        Self {
            proxy,
            devices,
            devices_initialized,
            event_senders,
            _watch_devices_task: watch_devices_task,
        }
    }

    /// Returns information about the device with the given `token_id`.
    ///
    /// Returns None if there is no device with the given ID.
    pub async fn device_info(&self, token_id: fadevice::TokenId) -> Option<DeviceInfo> {
        self.devices_initialized.wait().await;
        self.devices.lock().await.get(&token_id).cloned()
    }

    /// Returns information about all devices in the registry.
    pub async fn device_infos(&self) -> BTreeMap<fadevice::TokenId, DeviceInfo> {
        self.devices_initialized.wait().await;
        self.devices.lock().await.clone()
    }

    /// Returns a [RegistryDevice] that observes the device with the given `token_id`.
    ///
    /// Returns an error if there is no device with the given token ID.
    pub async fn observe(&self, token_id: fadevice::TokenId) -> Result<RegistryDevice, Error> {
        self.devices_initialized.wait().await;

        let info = self
            .devices
            .lock()
            .await
            .get(&token_id)
            .cloned()
            .ok_or_else(|| anyhow!("Device with ID {} does not exist", token_id))?;

        let (observer_proxy, observer_server) =
            self.proxy.domain().create_proxy::<fadevice::ObserverMarker>();

        let _ = self
            .proxy
            .create_observer(fadevice::RegistryCreateObserverRequest {
                token_id: Some(token_id),
                observer_server: Some(observer_server),
                ..Default::default()
            })
            .await
            .context("Failed to call CreateObserver")?
            .map_err(|err| anyhow!("failed to create device observer: {:?}", err))?;

        Ok(RegistryDevice::new(info, observer_proxy))
    }

    /// Returns a channel of device events.
    pub async fn subscribe(&self) -> DeviceEventReceiver {
        let (sender, receiver) = mpsc::unbounded::<DeviceEvent>();
        self.event_senders.lock().await.push(sender);
        receiver
    }
}

/// Watches devices added to and removed from the registry and updates
/// `devices` with the current state.
///
/// Signals `devices_initialized` when `devices` is populated with the initial
/// set of devices.
async fn watch_devices(
    proxy: fadevice::RegistryProxy,
    devices: Arc<Mutex<BTreeMap<fadevice::TokenId, DeviceInfo>>>,
    devices_initialized: AsyncEvent,
    event_senders: Arc<Mutex<Vec<DeviceEventSender>>>,
) -> Result<(), Error> {
    let mut devices_initialized = Some(devices_initialized);

    let mut devices_added_stream =
        HangingGetStream::new(proxy.clone(), fadevice::RegistryProxy::watch_devices_added);
    let mut device_removed_stream =
        HangingGetStream::new(proxy, fadevice::RegistryProxy::watch_device_removed);

    loop {
        futures::select! {
            added = devices_added_stream.select_next_some() => {
                let response = added
                    .context("failed to call WatchDevicesAdded")?
                    .map_err(|err| anyhow!("failed to watch for added devices: {:?}", err))?;
                let added_devices = response.devices.ok_or_else(|| anyhow!("missing devices"))?;

                let mut devices = devices.lock().await;
                let mut event_senders = event_senders.lock().await;

                for new_device in added_devices.into_iter() {
                    let token_id = new_device.token_id.ok_or_else(|| anyhow!("device info missing token_id"))?;
                    let device_info = DeviceInfo::from(new_device);
                    for sender in event_senders.iter_mut() {
                        let _ = sender.unbounded_send(DeviceEvent::Added(Box::new(device_info.clone())));
                    }
                    let _ = devices.insert(token_id, device_info);
                }

                if let Some(devices_initialized) = devices_initialized.take() {
                    devices_initialized.signal();
                }
            },
            removed = device_removed_stream.select_next_some() => {
                let response = removed
                    .context("failed to call WatchDeviceRemoved")?
                    .map_err(|err| anyhow!("failed to watch for removed device: {:?}", err))?;
                let token_id = response.token_id.ok_or_else(|| anyhow!("missing token_id"))?;
                let mut devices = devices.lock().await;
                let _ = devices.remove(&token_id);
                for sender in event_senders.lock().await.iter_mut() {
                    let _ = sender.unbounded_send(DeviceEvent::Removed(token_id));
                }
            }
        }
    }
}

pub struct RegistryDevice {
    _info: DeviceInfo,
    _proxy: fadevice::ObserverProxy,

    /// If None, this device does not support signal processing.
    pub signal_processing: Option<SignalProcessing>,
}

impl RegistryDevice {
    pub fn new(info: DeviceInfo, proxy: fadevice::ObserverProxy) -> Self {
        let is_signal_processing_supported = info.0.signal_processing_elements.is_some()
            && info.0.signal_processing_topologies.is_some();
        let signal_processing =
            is_signal_processing_supported.then(|| SignalProcessing::new(proxy.clone()));

        Self { _info: info, _proxy: proxy, signal_processing }
    }
}

/// Client for the composed signal processing `Reader` in a `fuchsia.audio.device.Observer`.
pub struct SignalProcessing {
    proxy: fadevice::ObserverProxy,

    element_states: Arc<Mutex<Option<BTreeMap<fadevice::ElementId, ElementState>>>>,
    topology_id: Arc<Mutex<Option<fadevice::TopologyId>>>,

    element_states_initialized: AsyncEvent,
    topology_id_initialized: AsyncEvent,

    _watch_element_states_task: Task<()>,
    _watch_topology_task: Task<()>,
}

impl SignalProcessing {
    fn new(proxy: fadevice::ObserverProxy) -> Self {
        let element_states = Arc::new(Mutex::new(None));
        let topology_id = Arc::new(Mutex::new(None));

        let element_states_initialized = AsyncEvent::new();
        let topology_id_initialized = AsyncEvent::new();

        let watch_element_states_task = Task::spawn({
            let proxy = proxy.clone();
            let element_states = element_states.clone();
            let element_states_initialized = element_states_initialized.clone();
            async move {
                if let Err(err) =
                    watch_element_states(proxy, element_states, element_states_initialized.clone())
                        .await
                {
                    error!(err:%; "Failed to watch Registry element states");
                    // Watching the element states will fail if the device does not support signal
                    // processing. In this case, mark the states as initialized so the getter can
                    // return the initial None value.
                    element_states_initialized.signal();
                }
            }
        });

        let watch_topology_task = Task::spawn({
            let proxy = proxy.clone();
            let topology_id = topology_id.clone();
            let topology_id_initialized = topology_id_initialized.clone();
            async move {
                if let Err(err) =
                    watch_topology(proxy, topology_id, topology_id_initialized.clone()).await
                {
                    error!(err:%; "Failed to watch Registry topology");
                    // Watching the topology ID will fail if the device does not support signal
                    // processing. In this case, mark the ID as initialized so the getter can
                    // return the initial None value.
                    topology_id_initialized.signal();
                }
            }
        });

        Self {
            proxy,
            element_states,
            topology_id,
            element_states_initialized,
            topology_id_initialized,
            _watch_element_states_task: watch_element_states_task,
            _watch_topology_task: watch_topology_task,
        }
    }

    /// Returns this device's signal processing elements, or `None` if the device does not support
    /// signal processing.
    pub async fn elements(&self) -> Result<Option<Vec<Element>>, Error> {
        let response = self
            .proxy
            .get_elements()
            .await
            .context("failed to call GetElements")?
            .map_err(Status::err_from_raw);

        if let Err(Status::NOT_SUPPORTED) = response {
            return Ok(None);
        }

        let elements = response
            .context("failed to get elements")?
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| anyhow!("Invalid element: {}", err))?;

        Ok(Some(elements))
    }

    /// Returns this device's signal processing topologies, or `None` if the device does not
    /// support signal processing.
    pub async fn topologies(&self) -> Result<Option<Vec<Topology>>, Error> {
        let response = self
            .proxy
            .get_topologies()
            .await
            .context("failed to call GetTopologies")?
            .map_err(Status::err_from_raw);

        if let Err(Status::NOT_SUPPORTED) = response {
            return Ok(None);
        }

        let topologies = response
            .context("failed to get topologies")?
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| anyhow!("Invalid topology: {}", err))?;

        Ok(Some(topologies))
    }

    /// Returns the current signal processing topology ID, or `None` if the device does not support
    /// signal processing.
    pub async fn topology_id(&self) -> Option<fadevice::TopologyId> {
        self.topology_id_initialized.wait().await;
        *self.topology_id.lock().await
    }

    /// Returns the state of the signal processing element with the given `element_id`.
    ///
    /// Returns None if there is no element with the given ID, or if the device does not support
    /// signal processing.
    pub async fn element_state(&self, element_id: fadevice::ElementId) -> Option<ElementState> {
        self.element_states_initialized.wait().await;
        self.element_states
            .lock()
            .await
            .as_ref()
            .and_then(|states| states.get(&element_id).cloned())
    }

    /// Returns states of all signal processing elements, or `None` if the device does not support
    /// signal processing.
    pub async fn element_states(&self) -> Option<BTreeMap<fadevice::ElementId, ElementState>> {
        self.element_states_initialized.wait().await;
        self.element_states.lock().await.clone()
    }
}

/// Watches element state changes on a registry device and updates `element_states` with the
/// current state for each element.
///
/// Signals `element_states_initialized` when `element_states` is populated
/// with the initial set of states.
async fn watch_element_states(
    proxy: fadevice::ObserverProxy,
    element_states: Arc<Mutex<Option<BTreeMap<fadevice::ElementId, ElementState>>>>,
    element_states_initialized: AsyncEvent,
) -> Result<(), Error> {
    let mut element_states_initialized = Some(element_states_initialized);

    let element_ids = {
        let get_elements_response = proxy
            .get_elements()
            .await
            .context("failed to call GetElements")?
            .map_err(Status::err_from_raw);

        if let Err(Status::NOT_SUPPORTED) = get_elements_response {
            element_states_initialized.take().unwrap().signal();
            return Ok(());
        }

        get_elements_response
            .context("failed to get elements")?
            .into_iter()
            .map(|element| element.id.ok_or_else(|| anyhow!("missing element 'id'")))
            .collect::<Result<Vec<_>, _>>()?
    };

    // Contains element IDs for which we haven't received an initial state.
    let mut uninitialized_element_ids = BTreeSet::from_iter(element_ids.iter().copied());

    let state_streams = element_ids.into_iter().map(|element_id| {
        HangingGetStream::new(proxy.clone(), move |p| p.watch_element_state(element_id))
            .map(move |element_state_result| (element_id, element_state_result))
    });

    let mut all_states_stream = futures::stream::select_all(state_streams);

    while let Some((element_id, element_state_result)) = all_states_stream.next().await {
        let element_state: ElementState = element_state_result
            .context("failed to call WatchElementState")?
            .try_into()
            .map_err(|err| anyhow!("Invalid element state: {}", err))?;
        let mut element_states = element_states.lock().await;
        let element_states_map = element_states.get_or_insert_with(BTreeMap::new);
        let _ = element_states_map.insert(element_id, element_state);

        // Signal `element_states_initialized` once all elements have initial states.
        if element_states_initialized.is_some() {
            let _ = uninitialized_element_ids.remove(&element_id);
            if uninitialized_element_ids.is_empty() {
                element_states_initialized.take().unwrap().signal();
            }
        }
    }

    Ok(())
}

/// Watches topology changes on a registry device and updates `topology_id`
/// when the topology changes.
///
/// Signals `topology_id_initialized` when `topology_id` is populated
/// with the initial topology.
async fn watch_topology(
    proxy: fadevice::ObserverProxy,
    topology_id: Arc<Mutex<Option<fadevice::TopologyId>>>,
    topology_id_initialized: AsyncEvent,
) -> Result<(), Error> {
    let mut topology_id_initialized = Some(topology_id_initialized);

    let mut topology_stream =
        HangingGetStream::new(proxy.clone(), fadevice::ObserverProxy::watch_topology);

    while let Some(topology_result) = topology_stream.next().await {
        let new_topology_id = topology_result.context("failed to call WatchTopology")?;

        *topology_id.lock().await = Some(new_topology_id);

        if let Some(topology_id_initialized) = topology_id_initialized.take() {
            topology_id_initialized.signal();
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use std::rc::Rc;

    use super::*;
    use async_utils::hanging_get::server::{HangingGet, Publisher};
    #[cfg(feature = "fdomain")]
    use fidl_test_util::fdomain::spawn_local_stream_handler;
    #[cfg(not(feature = "fdomain"))]
    use fidl_test_util::spawn_local_stream_handler;

    type AddedResponse = fadevice::RegistryWatchDevicesAddedResponse;
    type AddedResponder = fadevice::RegistryWatchDevicesAddedResponder;
    type AddedNotifyFn = Box<dyn Fn(&AddedResponse, AddedResponder) -> bool>;
    type AddedPublisher = Publisher<AddedResponse, AddedResponder, AddedNotifyFn>;

    type RemovedResponse = fadevice::RegistryWatchDeviceRemovedResponse;
    type RemovedResponder = fadevice::RegistryWatchDeviceRemovedResponder;
    type RemovedNotifyFn = Box<dyn Fn(&RemovedResponse, RemovedResponder) -> bool>;
    type RemovedPublisher = Publisher<RemovedResponse, RemovedResponder, RemovedNotifyFn>;

    fn serve_registry(
        initial_devices: Vec<fadevice::Info>,
    ) -> (fadevice::RegistryProxy, AddedPublisher, RemovedPublisher) {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let initial_added_response =
            AddedResponse { devices: Some(initial_devices), ..Default::default() };
        let watch_devices_added_notify: AddedNotifyFn =
            Box::new(|response, responder: AddedResponder| {
                responder.send(Ok(response)).expect("failed to send response");
                true
            });
        let mut added_broker = HangingGet::new(initial_added_response, watch_devices_added_notify);
        let added_publisher = added_broker.new_publisher();

        let watch_device_removed_notify: RemovedNotifyFn =
            Box::new(|response, responder: RemovedResponder| {
                responder.send(Ok(response)).expect("failed to send response");
                true
            });
        let mut removed_broker = HangingGet::new_unknown_state(watch_device_removed_notify);
        let removed_publisher = removed_broker.new_publisher();

        let added_subscriber = Rc::new(Mutex::new(added_broker.new_subscriber()));
        let removed_subscriber = Rc::new(Mutex::new(removed_broker.new_subscriber()));

        let proxy = spawn_local_stream_handler(
            #[cfg(feature = "fdomain")]
            &client,
            move |request| {
                let added_subscriber = added_subscriber.clone();
                let removed_subscriber = removed_subscriber.clone();
                async move {
                    match request {
                        fadevice::RegistryRequest::WatchDevicesAdded { responder } => {
                            added_subscriber.lock().await.register(responder).unwrap()
                        }
                        fadevice::RegistryRequest::WatchDeviceRemoved { responder } => {
                            removed_subscriber.lock().await.register(responder).unwrap()
                        }
                        _ => unimplemented!(),
                    }
                }
            },
        );

        (proxy, added_publisher, removed_publisher)
    }

    fn added_response(devices: Vec<fadevice::Info>) -> fadevice::RegistryWatchDevicesAddedResponse {
        fadevice::RegistryWatchDevicesAddedResponse { devices: Some(devices), ..Default::default() }
    }

    fn removed_response(
        token_id: fadevice::TokenId,
    ) -> fadevice::RegistryWatchDeviceRemovedResponse {
        fadevice::RegistryWatchDeviceRemovedResponse {
            token_id: Some(token_id),
            ..Default::default()
        }
    }

    #[fuchsia::test]
    async fn test_device_info() {
        let initial_devices = vec![fadevice::Info { token_id: Some(1), ..Default::default() }];
        let (registry_proxy, _added_publisher, _removed_publisher) =
            serve_registry(initial_devices);
        let registry = Registry::new(registry_proxy);

        assert!(registry.device_info(1).await.is_some());
        assert!(registry.device_info(2).await.is_none());
    }

    #[fuchsia::test]
    async fn test_subscribe() {
        let initial_devices = vec![];
        let (registry_proxy, added_publisher, removed_publisher) = serve_registry(initial_devices);
        let registry = Registry::new(registry_proxy);

        registry.devices_initialized.wait().await;

        let mut events_receiver = registry.subscribe().await;

        // Publish a WatchDevicesAdded response with two devices and verify that we receive it.
        added_publisher.set(added_response(vec![
            fadevice::Info { token_id: Some(1), ..Default::default() },
            fadevice::Info { token_id: Some(2), ..Default::default() },
        ]));

        // There should be two events, one for each device.
        let events: Vec<_> = events_receiver.by_ref().take(2).collect().await;

        let mut added_token_ids: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                DeviceEvent::Added(info) => Some(info.token_id()),
                _ => None,
            })
            .collect();
        added_token_ids.sort();
        assert_eq!(added_token_ids, vec![1, 2]);

        // Publish a WatchDeviceRemoved response and verify that we receive it.
        removed_publisher.set(removed_response(2));

        // There should be one event.
        let events: Vec<_> = events_receiver.take(1).collect().await;

        let mut removed_token_ids: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                DeviceEvent::Removed(token_id) => Some(*token_id),
                _ => None,
            })
            .collect();
        removed_token_ids.sort();
        assert_eq!(removed_token_ids, vec![2]);
    }
}
