// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};
use fidl::endpoints::{create_request_stream, Responder};
use fidl_fuchsia_bluetooth_gatt2 as gatt;
use fuchsia_bluetooth::types::Uuid;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use log::{info, warn};

use crate::host_dispatcher::HostDispatcher;

const GENERIC_ACCESS_SERVICE_UUID: Uuid = Uuid::new16(0x1800);
const GENERIC_ACCESS_DEVICE_NAME_UUID: Uuid = Uuid::new16(0x2A00);
const GENERIC_ACCESS_APPEARANCE_UUID: Uuid = Uuid::new16(0x2A01);
const GENERIC_ACCESS_DEVICE_NAME_ID: u64 = 0x2A00;
const GENERIC_ACCESS_APPEARANCE_ID: u64 = 0x2A01;

fn build_generic_access_service_info() -> gatt::ServiceInfo {
    // The spec says these characteristics should be readable, but optionally writeable. For
    // simplicity, we've disallowed them being peer-writeable. We enable access to these
    // characteristics with no security, as they are sent out freely during advertising anyway.
    let device_name_characteristic = gatt::Characteristic {
        handle: Some(gatt::Handle { value: GENERIC_ACCESS_DEVICE_NAME_ID }),
        type_: Some(GENERIC_ACCESS_DEVICE_NAME_UUID.into()),
        properties: Some(gatt::CharacteristicPropertyBits::READ),
        permissions: Some(gatt::AttributePermissions {
            read: Some(gatt::SecurityRequirements::default()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let appearance_characteristic = gatt::Characteristic {
        handle: Some(gatt::Handle { value: GENERIC_ACCESS_APPEARANCE_ID }),
        type_: Some(GENERIC_ACCESS_APPEARANCE_UUID.into()),
        properties: Some(gatt::CharacteristicPropertyBits::READ),
        permissions: Some(gatt::AttributePermissions {
            read: Some(gatt::SecurityRequirements::default()),
            ..Default::default()
        }),
        ..Default::default()
    };

    gatt::ServiceInfo {
        // This value is ignored as this is a local-only service
        handle: Some(gatt::ServiceHandle { value: 0 }),
        // Secondary services are only rarely used and this is not one of those cases
        kind: Some(gatt::ServiceKind::Primary),
        type_: Some(GENERIC_ACCESS_SERVICE_UUID.into()),
        characteristics: Some(vec![device_name_characteristic, appearance_characteristic]),
        ..Default::default()
    }
}

/// A GasProxy forwards peer Generic Access Service requests received by a BT host to the local GAS
/// task. A GasProxy will be spawned as a task by HostDispatcher whenever a new host is detected.
/// Passing the requests through proxies is preferable to the  task maintaining host state so that
/// we can limit host state to one place, HostDispatcher. This will simplify supporting multiple
/// Bluetooth hosts from within a single HostDispatcher in the future.
pub struct GasProxy {
    service_request_stream: gatt::LocalServiceRequestStream,
    gas_task_channel: mpsc::Sender<gatt::LocalServiceRequest>,
    // We have to hold on to these connections to the Hosts GATT server even though we never use them because
    // otherwise the host will shut down the connection to the Generic Access Server.
    _gatt_server: gatt::Server_Proxy,
}

impl GasProxy {
    pub async fn new(
        gatt_server: gatt::Server_Proxy,
        gas_task_channel: mpsc::Sender<gatt::LocalServiceRequest>,
    ) -> Result<GasProxy, Error> {
        let (service_client, service_request_stream) =
            create_request_stream::<gatt::LocalServiceMarker>();
        let service_info = build_generic_access_service_info();
        gatt_server.publish_service(&service_info, service_client).await?.map_err(|e| {
            format_err!("Failed to publish Generic Access Service to GATT server: {:?}", e)
        })?;
        info!("Published Generic Access Service to local device database.");
        Ok(GasProxy { service_request_stream, gas_task_channel, _gatt_server: gatt_server })
    }

    pub async fn run(mut self) -> Result<(), Error> {
        while let Some(req) = self.service_request_stream.next().await {
            if let Err(send_err) = self.gas_task_channel.send(req?).await {
                if send_err.is_disconnected() {
                    return Ok(());
                }
                return Err(send_err.into());
            }
        }
        Ok(())
    }
}

/// Struct holding the state needed to run the Generic Access Service task, which
/// serves requests to the Generic Access Service from other devices per the BT spec.
/// To avoid shared state it reads back into HostDispatcher to see the values of the
/// service characteristics (name/appearance). The stream of requests is abstracted
/// from being tied to a specific host - HostDispatcher is set up so that when any
/// new host is set up, it ties the sender end of that channel to an instance of
/// the GAS Proxy task, which proxies the requests from that specific host to the
/// sender end of the channel stored in this struct.
pub struct GenericAccessService {
    hd: HostDispatcher,
    generic_access_req_stream: mpsc::Receiver<gatt::LocalServiceRequest>,
}

impl GenericAccessService {
    pub fn build(
        hd: &HostDispatcher,
        request_stream: mpsc::Receiver<gatt::LocalServiceRequest>,
    ) -> Self {
        Self { hd: hd.clone(), generic_access_req_stream: request_stream }
    }

    fn send_read_response(
        &self,
        responder: gatt::LocalServiceReadValueResponder,
        id: u64,
    ) -> Result<(), fidl::Error> {
        match id {
            GENERIC_ACCESS_DEVICE_NAME_ID => responder.send(Ok(self.hd.get_name().as_bytes())),
            GENERIC_ACCESS_APPEARANCE_ID => {
                responder.send(Ok(&self.hd.get_appearance().into_primitive().to_le_bytes()))
            }
            _ => responder.send(Err(gatt::Error::ReadNotPermitted)),
        }
    }

    fn process_service_req(&self, request: gatt::LocalServiceRequest) -> Result<(), Error> {
        match request {
            gatt::LocalServiceRequest::ReadValue { responder, handle, .. } => {
                Ok(self.send_read_response(responder, handle.value)?)
            }
            // Writing to the the available GENERIC_ACCESS service characteristics
            // is optional according to the spec, and it was decided not to implement
            gatt::LocalServiceRequest::WriteValue { responder, .. } => {
                Ok(responder.send(Err(gatt::Error::WriteNotPermitted))?)
            }
            gatt::LocalServiceRequest::PeerUpdate { payload: _, responder } => {
                Ok(responder.drop_without_shutdown())
            }
            // Ignore CharacteristicConfiguration, ValueChangedCredit, etc.
            _ => Ok(()),
        }
    }

    pub async fn run(mut self) {
        while let Some(request) = self.generic_access_req_stream.next().await {
            self.process_service_req(request).unwrap_or_else(|e| {
                warn!("Error handling Generic Access Service Request: {:?}", e);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_helpers::hanging_get::asynchronous as hanging_get;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_bluetooth::{Appearance, PeerId};
    use futures::FutureExt;
    use std::collections::HashMap;
    use {fuchsia_async as fasync, fuchsia_inspect as inspect};

    use crate::host_dispatcher::{NameReplace, DEFAULT_DEVICE_NAME};
    use crate::store::stash::Stash;

    const TEST_DEVICE_APPEARANCE: Appearance = Appearance::Computer;

    // Starts tasks that run the GasProxy and GenericAccessService and returns the associated
    // LocalServiceProxy and HostDispatcher.
    fn setup_generic_access_service() -> (gatt::LocalServiceProxy, HostDispatcher) {
        let (service_client, service_request_stream) =
            create_proxy_and_stream::<gatt::LocalServiceMarker>();
        let (gas_task_channel, generic_access_req_stream) =
            mpsc::channel::<gatt::LocalServiceRequest>(0);
        let (gatt_server, _gatt_server_remote) = create_proxy_and_stream::<gatt::Server_Marker>();
        let gas_proxy = GasProxy {
            service_request_stream,
            gas_task_channel: gas_task_channel.clone(),
            _gatt_server: gatt_server,
        };
        fasync::Task::spawn(gas_proxy.run().map(|r| {
            r.unwrap_or_else(|err| {
                warn!("Error running Generic Access proxy in task: {:?}", err);
            })
        }))
        .detach();
        let stash = Stash::in_memory_mock();
        let inspector = inspect::Inspector::default();
        let system_inspect = inspector.root().create_child("system");
        let watch_peers_broker = hanging_get::HangingGetBroker::new(
            HashMap::new(),
            |_, _| true,
            hanging_get::DEFAULT_CHANNEL_SIZE,
        );
        let watch_hosts_broker = hanging_get::HangingGetBroker::new(
            Vec::new(),
            |_, _| true,
            hanging_get::DEFAULT_CHANNEL_SIZE,
        );
        let dispatcher = HostDispatcher::new(
            TEST_DEVICE_APPEARANCE,
            Default::default(),
            stash,
            system_inspect,
            gas_task_channel,
            watch_peers_broker.new_publisher(),
            watch_peers_broker.new_registrar(),
            watch_hosts_broker.new_publisher(),
            watch_hosts_broker.new_registrar(),
        );
        let service = GenericAccessService { hd: dispatcher.clone(), generic_access_req_stream };
        fasync::Task::spawn(service.run()).detach();
        (service_client, dispatcher)
    }

    #[fuchsia::test]
    async fn test_change_name() {
        let (delegate_client, host_dispatcher) = setup_generic_access_service();
        let expected_device_name = delegate_client
            .read_value(
                &PeerId { value: 1 },
                &gatt::Handle { value: GENERIC_ACCESS_DEVICE_NAME_ID },
                0,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(expected_device_name, DEFAULT_DEVICE_NAME.as_bytes());
        // This is expected to error since there is no host.
        let _ = host_dispatcher
            .set_name("test-generic-access-service-1".to_string(), NameReplace::Replace)
            .await
            .unwrap_err();
        let expected_device_name = delegate_client
            .read_value(
                &PeerId { value: 1 },
                &gatt::Handle { value: GENERIC_ACCESS_DEVICE_NAME_ID },
                0,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(expected_device_name, "test-generic-access-service-1".to_string().as_bytes());
    }

    #[fuchsia::test]
    async fn test_get_appearance() {
        let (service_client, _host_dispatcher) = setup_generic_access_service();
        let read_device_appearance = service_client
            .read_value(
                &PeerId { value: 1 },
                &gatt::Handle { value: GENERIC_ACCESS_APPEARANCE_ID },
                0,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            read_device_appearance,
            TEST_DEVICE_APPEARANCE.into_primitive().to_le_bytes().to_vec()
        );
    }

    #[fuchsia::test]
    async fn test_invalid_request() {
        let (service_client, _host_dispatcher) = setup_generic_access_service();
        let result = service_client
            .write_value(&gatt::LocalServiceWriteValueRequest {
                peer_id: Some(PeerId { value: 1 }),
                handle: Some(gatt::Handle { value: GENERIC_ACCESS_DEVICE_NAME_ID }),
                offset: Some(0),
                value: Some(b"new-name".to_vec()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), gatt::Error::WriteNotPermitted);
    }

    #[fuchsia::test]
    async fn test_gas_proxy() {
        let (service_client, service_request_stream) =
            create_proxy_and_stream::<gatt::LocalServiceMarker>();
        let (gas_task_channel, mut generic_access_req_stream) =
            mpsc::channel::<gatt::LocalServiceRequest>(0);
        let (gatt_server, _gatt_server_remote) = create_proxy_and_stream::<gatt::Server_Marker>();
        let gas_proxy =
            GasProxy { service_request_stream, gas_task_channel, _gatt_server: gatt_server };
        fasync::Task::spawn(gas_proxy.run().map(|r| {
            r.unwrap_or_else(|err| {
                warn!("Error running Generic Access proxy in task: {:?}", err);
            })
        }))
        .detach();
        let _ignored_fut = service_client.read_value(
            &PeerId { value: 1 },
            &gatt::Handle { value: GENERIC_ACCESS_APPEARANCE_ID },
            0,
        );
        let proxied_request = generic_access_req_stream.next().await.unwrap();
        let (_, handle, ..) = proxied_request.into_read_value().expect("ReadValue request");
        assert_eq!(handle.value, GENERIC_ACCESS_APPEARANCE_ID);
    }
}
