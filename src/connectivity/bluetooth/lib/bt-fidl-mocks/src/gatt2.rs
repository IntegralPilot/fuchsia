// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::expect::{expect_call, Status};
use anyhow::Error;
use fidl::endpoints::{ClientEnd, ServerEnd};
use fidl_fuchsia_bluetooth::Uuid as FidlUuid;
use fidl_fuchsia_bluetooth_gatt2::{
    self as gatt2, Characteristic, CharacteristicNotifierMarker, ClientControlHandle, ClientMarker,
    ClientProxy, ClientRequest, ClientRequestStream, Handle, ReadByTypeResult, ReadValue,
    RemoteServiceMarker, RemoteServiceProxy, RemoteServiceRequest, RemoteServiceRequestStream,
    ServiceHandle,
};
use fuchsia_bluetooth::types::Uuid;
use log::info;
use std::collections::HashSet;
use zx::MonotonicDuration;

/// Provides a simple mock implementation of `fuchsia.bluetooth.gatt2.RemoteService`.
pub struct RemoteServiceMock {
    stream: RemoteServiceRequestStream,
    timeout: MonotonicDuration,
}

impl RemoteServiceMock {
    pub fn new(
        timeout: MonotonicDuration,
    ) -> Result<(RemoteServiceProxy, RemoteServiceMock), Error> {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<RemoteServiceMarker>();
        Ok((proxy, RemoteServiceMock { stream, timeout }))
    }

    pub fn from_stream(
        stream: RemoteServiceRequestStream,
        timeout: MonotonicDuration,
    ) -> RemoteServiceMock {
        RemoteServiceMock { stream, timeout }
    }

    pub async fn expect_discover_characteristics(
        &mut self,
        characteristics: &Vec<Characteristic>,
    ) -> Result<(), Error> {
        expect_call(&mut self.stream, self.timeout, move |req| match req {
            RemoteServiceRequest::DiscoverCharacteristics { responder } => {
                match responder.send(characteristics) {
                    Ok(_) => Ok(Status::Satisfied(())),
                    Err(e) => Err(e.into()),
                }
            }
            _ => Ok(Status::Pending),
        })
        .await
    }

    /// Wait until a Read By Type message is received with the given `uuid`. `result` will be sent
    /// in response to the matching FIDL request.
    pub async fn expect_read_by_type(
        &mut self,
        expected_uuid: Uuid,
        result: Result<&[ReadByTypeResult], gatt2::Error>,
    ) -> Result<(), Error> {
        let expected_uuid: FidlUuid = expected_uuid.into();
        expect_call(&mut self.stream, self.timeout, move |req| {
            if let RemoteServiceRequest::ReadByType { uuid, responder } = req {
                if uuid == expected_uuid {
                    responder.send(result)?;
                    Ok(Status::Satisfied(()))
                } else {
                    // Send error to unexpected request.
                    responder.send(Err(gatt2::Error::UnlikelyError))?;
                    Ok(Status::Pending)
                }
            } else {
                Ok(Status::Pending)
            }
        })
        .await
    }

    /// Wait until a Read Characteristic request is received with the given handle, then `result`
    /// will be sent.
    pub async fn expect_read_characteristic(
        &mut self,
        expected_handle: u64,
        result: Result<&ReadValue, gatt2::Error>,
    ) -> Result<(), Error> {
        expect_call(&mut self.stream, self.timeout, move |req| match req {
            RemoteServiceRequest::ReadCharacteristic { handle, options: _, responder } => {
                if handle.value == expected_handle {
                    responder.send(result)?;
                    Ok(Status::Satisfied(()))
                } else {
                    responder.send(Err(gatt2::Error::UnlikelyError))?;
                    Ok(Status::Pending)
                }
            }
            x => {
                info!("Received unexpected RemoteServiceRequest: {x:?}");
                Ok(Status::Pending)
            }
        })
        .await
    }

    pub async fn expect_register_characteristic_notifier(
        &mut self,
        handle: Handle,
    ) -> Result<ClientEnd<CharacteristicNotifierMarker>, Error> {
        expect_call(&mut self.stream, self.timeout, move |req| match req {
            RemoteServiceRequest::RegisterCharacteristicNotifier {
                handle: h,
                notifier,
                responder,
            } => {
                if h == handle {
                    responder.send(Ok(()))?;
                    Ok(Status::Satisfied(notifier))
                } else {
                    info!("Got RegisterCharacteristicNotifier for wrong handle: {h:?}, ignoring");
                    responder.send(Err(gatt2::Error::InvalidHandle))?;
                    Ok(Status::Pending)
                }
            }
            x => {
                info!("Received unexpected RemoteServiceRequest: {x:?}");
                Ok(Status::Pending)
            }
        })
        .await
    }
}

/// Mock for the fuchsia.bluetooth.gatt2/Client server. Can be used to expect and intercept requests
/// to connect to GATT services.
pub struct ClientMock {
    stream: ClientRequestStream,
    timeout: MonotonicDuration,
    services: Vec<gatt2::ServiceInfo>,
    /// The handles of the services we have already returned.
    returned_services: HashSet<u64>,
    last_filter: Vec<fidl_fuchsia_bluetooth::Uuid>,
}

impl ClientMock {
    pub fn new(timeout: MonotonicDuration) -> Result<(ClientProxy, ClientMock), Error> {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<ClientMarker>();
        Ok((proxy, Self::from_stream(stream, timeout)))
    }

    pub fn from_stream(stream: ClientRequestStream, timeout: MonotonicDuration) -> Self {
        Self {
            stream,
            timeout,
            services: Vec::new(),
            returned_services: HashSet::new(),
            last_filter: vec![Uuid::new16(0xffff).into()],
        }
    }

    pub fn add_service(&mut self, service: gatt2::ServiceInfo) {
        self.services.push(service);
    }

    pub async fn expect_watch_services(&mut self) -> Result<(), Error> {
        // The services we haven't returned before
        let unseen_services: Vec<_> = self
            .services
            .iter()
            .filter(|x| !self.returned_services.contains(&x.handle.unwrap().value))
            .cloned()
            .collect();
        let all_services = self.services.clone();
        let last_filter = self.last_filter.clone();
        expect_call(&mut self.stream, self.timeout, |req| match req {
            ClientRequest::WatchServices { uuids, responder } => {
                let services = if uuids != last_filter {
                    self.returned_services.clear();
                    self.last_filter = uuids.clone();
                    all_services.clone()
                } else {
                    unseen_services.clone()
                };
                if uuids.is_empty() {
                    responder.send(services.as_slice(), &[])?;
                    services.iter().for_each(|s| {
                        let _ = self.returned_services.insert(s.handle.unwrap().value);
                    });
                } else {
                    let matched: Vec<gatt2::ServiceInfo> = services
                        .iter()
                        .filter(|x| uuids.iter().find(|u| x.type_ == Some(**u)).is_some())
                        .cloned()
                        .collect();
                    responder.send(matched.as_slice(), &[])?;
                    matched.iter().for_each(|s| {
                        let _ = self.returned_services.insert(s.handle.unwrap().value);
                    });
                }
                Ok(Status::Satisfied(()))
            }
            x => {
                info!("Received unexpected gatt2::Client Request: {x:?}");
                Ok(Status::Pending)
            }
        })
        .await
    }

    pub async fn expect_connect_to_service(
        &mut self,
        handle: ServiceHandle,
    ) -> Result<(ClientControlHandle, ServerEnd<RemoteServiceMarker>), Error> {
        expect_call(&mut self.stream, self.timeout, move |req| match req {
            ClientRequest::ConnectToService { handle: h, service, control_handle }
                if h == handle =>
            {
                Ok(Status::Satisfied((control_handle, service)))
            }
            _ => Ok(Status::Pending),
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeout_duration;
    use futures::join;

    #[fuchsia::test]
    async fn test_expect_read_by_type() {
        let (proxy, mut mock) =
            RemoteServiceMock::new(timeout_duration()).expect("failed to create mock");
        let uuid = Uuid::new16(0x180d);
        let result = Ok(&[][..]);

        let fidl_uuid: FidlUuid = uuid.clone().into();
        let read_by_type_fut = proxy.read_by_type(&fidl_uuid);
        let expect_fut = mock.expect_read_by_type(uuid, result);

        let (read_by_type_result, expect_result) = join!(read_by_type_fut, expect_fut);
        let _ = read_by_type_result.expect("read by type request failed");
        let _ = expect_result.expect("expectation not satisfied");
    }

    #[fuchsia::test]
    async fn test_watch_services() {
        let (proxy, mut mock) = ClientMock::new(timeout_duration()).expect("failed to create mock");

        mock.add_service(gatt2::ServiceInfo {
            handle: Some(gatt2::ServiceHandle { value: 1 }),
            kind: Some(gatt2::ServiceKind::Primary),
            type_: Some(Uuid::new16(0x100d).into()),
            ..Default::default()
        });

        let expect_watch_services_fut = mock.expect_watch_services();
        let watch_services_fut = proxy.watch_services(&[]);

        let (expect_result, watch_result) = join!(expect_watch_services_fut, watch_services_fut);

        let (services_updated, _services_removed) = watch_result.unwrap();
        assert_eq!(services_updated.len(), 1);
        assert!(expect_result.is_ok());

        mock.add_service(gatt2::ServiceInfo {
            handle: Some(gatt2::ServiceHandle { value: 2 }),
            kind: Some(gatt2::ServiceKind::Primary),
            type_: Some(Uuid::new16(0x100f).into()),
            ..Default::default()
        });

        let expect_watch_services_fut = mock.expect_watch_services();
        let watch_services_fut = proxy.watch_services(&[]);

        let (expect_result, watch_result) = join!(expect_watch_services_fut, watch_services_fut);

        let (services_updated, _services_removed) = watch_result.unwrap();
        assert_eq!(services_updated.len(), 1);
        assert!(services_updated[0].handle.is_some_and(|x| x.value == 2));
        assert!(expect_result.is_ok());
    }
}
