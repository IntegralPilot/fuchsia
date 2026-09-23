// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::{PeerId, Uuid};
use bt_gatt::*;
use fidl::EventPair;
use fidl::client::QueryResponseFut;
use fidl::endpoints::RequestStream;
use fidl_fuchsia_bluetooth as fidl_bt;
use fidl_fuchsia_bluetooth_gatt2 as fidl_gatt2;
use fidl_fuchsia_bluetooth_le as fidl_le;
use fidl_gatt2::{
    LocalServiceControlHandle, LocalServiceRequestStream, ServerPublishServiceResult,
    ValueChangedParameters,
};
use fidl_le::{ConnectionProxy, ScanResultWatcherProxy};
use fuchsia_async::{self as fasync, TimeoutExt};
use fuchsia_sync::Mutex;
use futures::future::{FusedFuture, Ready};
use futures::stream::FusedStream;
use futures::{Future, FutureExt, Stream, StreamExt};
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use zx;

#[cfg(test)]
mod test;

pub mod pii;

mod periodic_advertising;
pub use periodic_advertising::PeriodicAdvertising;

pub struct FuchsiaTypes {}

impl bt_gatt::GattTypes for FuchsiaTypes {
    type Central = Central;
    type ScanResultStream = ScanResultStream;
    type Client = Client;
    type ConnectFuture = Ready<Result<Self::Client>>;
    type PeriodicAdvertising = PeriodicAdvertising;

    type PeerServiceHandle = PeerServiceHandle;
    type FindServicesFut = fasync::Task<Result<Vec<PeerServiceHandle>>>;
    type PeerService = PeerService;
    type ServiceConnectFut = Ready<Result<PeerService>>;

    type ReadFut<'a> = ReadFuture<'a>;
    type WriteFut<'a> = WriteFuture<'a>;
    type CharacteristicDiscoveryFut = CharacteristicResultFut;
    type NotificationStream = CharacteristicNotificationStream;
}

impl bt_gatt::ServerTypes for FuchsiaTypes {
    type Server = Server;
    type LocalService = LocalService;
    type LocalServiceFut = LocalServiceFut;
    type ServiceEventStream = LocalEventStream;
    type ServiceWriteType = Vec<u8>;
    type ReadResponder = ReadResponder;
    type WriteResponder = WriteResponder;
    type IndicateConfirmationStream = IndicateConfirmationStream;
}

#[derive(Clone)]
pub struct Central {
    proxy: fidl_le::CentralProxy,
}

impl Central {
    pub fn new(proxy: fidl_le::CentralProxy) -> Self {
        Self { proxy }
    }
}

pub(crate) fn to_fidl_peer_id(id: &PeerId) -> fidl_fuchsia_bluetooth::PeerId {
    fidl_fuchsia_bluetooth::PeerId { value: id.0 }
}

fn filter_into_fidl(filter: &central::ScanFilter) -> fidl_le::Filter {
    use central::Filter::*;
    let mut fidl_filter = fidl_le::Filter::default();
    for filter in &filter.filters {
        match filter {
            ServiceUuid(uuid) => {
                fidl_filter.service_uuid = Some(to_fidl_uuid(uuid));
            }
            HasServiceData(uuid) => {
                fidl_filter.service_data_uuid = Some(to_fidl_uuid(uuid));
            }
            HasManufacturerData(id) => fidl_filter.manufacturer_id = Some(*id),
            IsConnectable => fidl_filter.connectable = Some(true),
            MatchesName(partial_name) => fidl_filter.name = Some(partial_name.clone()),
            MaxPathLoss(path_loss) => fidl_filter.max_path_loss = Some(*path_loss),
        }
    }
    fidl_filter
}

pub(crate) fn to_fidl_uuid(uuid: &Uuid) -> fidl_fuchsia_bluetooth::Uuid {
    let uuid: uuid::Uuid = (*uuid).into();
    let uuid: fuchsia_bluetooth::types::Uuid = uuid.into();
    uuid.into()
}

impl bt_gatt::Central<FuchsiaTypes> for Central {
    fn scan(&self, filters: &[central::ScanFilter]) -> ScanResultStream {
        let scan_options = fidl_le::ScanOptions {
            filters: Some(filters.iter().map(filter_into_fidl).collect()),
            ..Default::default()
        };
        let (proxy, server_end) =
            fidl::endpoints::create_proxy::<fidl_le::ScanResultWatcherMarker>();
        let scan_stopped_fut = self.proxy.scan(&scan_options, server_end);
        ScanResultStream::new(proxy, scan_stopped_fut)
    }

    fn periodic_advertising(
        &self,
    ) -> bt_gatt::Result<<FuchsiaTypes as GattTypes>::PeriodicAdvertising> {
        Ok(PeriodicAdvertising { proxy: self.proxy.clone() })
    }

    fn connect(&self, peer_id: PeerId) -> <FuchsiaTypes as GattTypes>::ConnectFuture {
        use futures::future::ready;
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fidl_le::ConnectionMarker>();
        if let Err(e) =
            self.proxy.connect(&to_fidl_peer_id(&peer_id), &Default::default(), server_end)
        {
            return ready(Err(types::Error::Other(Box::new(e))));
        }
        let (client_proxy, server_end) =
            fidl::endpoints::create_proxy::<fidl_gatt2::ClientMarker>();
        if let Err(e) = proxy.request_gatt_client(server_end) {
            return ready(Err(types::Error::Other(Box::new(e))));
        }
        return ready(Ok(Client::new(peer_id, proxy, client_proxy)));
    }
}

pub fn to_gatt_uuid(uuid: &fidl_bt::Uuid) -> Uuid {
    let uuid: fuchsia_bluetooth::types::Uuid = uuid.into();
    let uuid: uuid::Uuid = uuid.into();
    uuid.into()
}

pub fn to_gatt_peer_id(id: &fidl_bt::PeerId) -> bt_common::PeerId {
    bt_common::PeerId(id.value)
}

fn to_gatt_gatt_error(err: &fidl_gatt2::Error) -> bt_gatt::types::Error {
    match bt_gatt::types::GattError::try_from(*err as u32) {
        Ok(gatt_er) => gatt_er.into(),
        Err(e) => e,
    }
}

fn to_fidl_gatt_error(err: &bt_gatt::types::GattError) -> fidl_gatt2::Error {
    // These match up.
    fidl_gatt2::Error::from_primitive(*err as u32).unwrap()
}

fn to_fidl_writemode(mode: &bt_gatt::types::WriteMode) -> fidl_gatt2::WriteMode {
    use bt_gatt::types::WriteMode;
    match mode {
        WriteMode::None => fidl_gatt2::WriteMode::Default,
        WriteMode::Reliable => fidl_gatt2::WriteMode::Reliable,
        WriteMode::WithoutResponse => fidl_gatt2::WriteMode::WithoutResponse,
    }
}

pub(crate) fn to_gatt_scan_data(
    data: fidl_le::ScanData,
) -> Vec<bt_gatt::central::AdvertisingDatum> {
    use bt_gatt::central::AdvertisingDatum::*;
    let mut ret = Vec::new();
    if let Some(appearance) = data.appearance {
        ret.push(Appearance(appearance.into_primitive()));
    }
    if let Some(level) = data.tx_power {
        ret.push(TxPowerLevel(level));
    }
    if let Some(uuids) = data.service_uuids {
        ret.push(Services(uuids.iter().map(to_gatt_uuid).collect()));
    }
    if let Some(datas) = data.service_data {
        let mut datas = datas
            .into_iter()
            .map(|fidl_le::ServiceData { uuid, data }| ServiceData(to_gatt_uuid(&uuid), data))
            .collect();
        ret.append(&mut datas);
    }
    if let Some(manuf_data) = data.manufacturer_data {
        let mut manufs = manuf_data
            .into_iter()
            .map(|fidl_le::ManufacturerData { company_id, data }| {
                ManufacturerData(company_id, data)
            })
            .collect();
        ret.append(&mut manufs);
    }
    if let Some(uris) = data.uris {
        for uri in uris {
            ret.push(Uri(uri));
        }
    }
    if let Some(name) = data.broadcast_name {
        ret.push(BroadcastName(name));
    }
    if let Some(rsi) = data.resolvable_set_identifier {
        ret.push(ResolvableSetIdentifier(rsi));
    }
    ret
}

fn to_gatt_scan_result(peer: &fidl_le::Peer) -> bt_gatt::central::ScanResult {
    bt_gatt::central::ScanResult {
        id: to_gatt_peer_id(&peer.id.unwrap()),
        connectable: peer.connectable.unwrap_or_default(),
        name: peer.name.clone().map_or(bt_gatt::central::PeerName::Unknown, |n| {
            bt_gatt::central::PeerName::CompleteName(n)
        }),
        advertised: peer.data.as_ref().cloned().map_or(Vec::new(), to_gatt_scan_data),
        advertising_sid: peer.advertising_sid,
        periodic_advertising_interval: peer.periodic_advertising_interval,
    }
}

fn to_gatt_handle(handle: &fidl_gatt2::Handle) -> bt_gatt::types::Handle {
    bt_gatt::types::Handle(handle.value)
}

fn to_fidl_handle(handle: &bt_gatt::types::Handle) -> fidl_gatt2::Handle {
    fidl_gatt2::Handle { value: handle.0 }
}

/// UUID for Client Characteristic Configuration (u16 for matching)
static CCC_UUID_U16: u16 = 0x2902;

fn to_gatt_descriptor(d: &fidl_gatt2::Descriptor) -> Option<bt_gatt::types::Descriptor> {
    let uuid = to_gatt_uuid(&d.type_.unwrap());
    let desc_type = match uuid.to_u16() {
        // CCC is handled elsewhere
        Some(x) if x == CCC_UUID_U16 => return None,
        _ => bt_gatt::types::DescriptorType::Other { uuid },
    };
    Some(bt_gatt::types::Descriptor {
        handle: to_gatt_handle(&d.handle.unwrap()),
        permissions: bt_gatt::types::AttributePermissions::default(),
        r#type: desc_type,
    })
}

fn to_gatt_characteristic(c: &fidl_gatt2::Characteristic) -> bt_gatt::types::Characteristic {
    let mut property_bits = Vec::new();
    let properties = c.properties.unwrap();
    if properties.contains(fidl_gatt2::CharacteristicPropertyBits::BROADCAST) {
        property_bits.push(bt_gatt::types::CharacteristicProperty::Broadcast);
    }
    if properties.contains(fidl_gatt2::CharacteristicPropertyBits::READ) {
        property_bits.push(bt_gatt::types::CharacteristicProperty::Read);
    }
    if properties.contains(fidl_gatt2::CharacteristicPropertyBits::WRITE) {
        property_bits.push(bt_gatt::types::CharacteristicProperty::Write);
    }
    if properties.contains(fidl_gatt2::CharacteristicPropertyBits::WRITE_WITHOUT_RESPONSE) {
        property_bits.push(bt_gatt::types::CharacteristicProperty::WriteWithoutResponse);
    }
    if properties.contains(fidl_gatt2::CharacteristicPropertyBits::NOTIFY) {
        property_bits.push(bt_gatt::types::CharacteristicProperty::Notify);
    }
    if properties.contains(fidl_gatt2::CharacteristicPropertyBits::INDICATE) {
        property_bits.push(bt_gatt::types::CharacteristicProperty::Indicate);
    }
    if properties.contains(fidl_gatt2::CharacteristicPropertyBits::AUTHENTICATED_SIGNED_WRITES) {
        property_bits.push(bt_gatt::types::CharacteristicProperty::AuthenticatedSignedWrites);
    }
    if properties.contains(fidl_gatt2::CharacteristicPropertyBits::RELIABLE_WRITE) {
        property_bits.push(bt_gatt::types::CharacteristicProperty::ReliableWrite);
    }
    if properties.contains(fidl_gatt2::CharacteristicPropertyBits::WRITABLE_AUXILIARIES) {
        property_bits.push(bt_gatt::types::CharacteristicProperty::WritableAuxiliaries);
    }
    let descriptors = c
        .descriptors
        .as_ref()
        .map_or(Vec::new(), |d| d.iter().filter_map(to_gatt_descriptor).collect());
    bt_gatt::types::Characteristic {
        handle: to_gatt_handle(&c.handle.unwrap()),
        uuid: to_gatt_uuid(&c.type_.unwrap()),
        properties: bt_gatt::types::CharacteristicProperties(property_bits),
        permissions: bt_gatt::types::AttributePermissions::default(),
        descriptors,
    }
}

pub enum ScanResultStream {
    Running {
        proxy: ScanResultWatcherProxy,
        active_watch: Option<QueryResponseFut<Vec<fidl_le::Peer>>>,
        queued: Vec<fidl_le::Peer>,
        // TODO: decide if we need to have this complete before we return None from the scan.
        _complete_fut: QueryResponseFut<()>,
    },
    Terminated,
}

impl ScanResultStream {
    fn new(proxy: ScanResultWatcherProxy, complete_fut: QueryResponseFut<()>) -> Self {
        Self::Running { proxy, _complete_fut: complete_fut, active_watch: None, queued: Vec::new() }
    }
}

impl FusedStream for ScanResultStream {
    fn is_terminated(&self) -> bool {
        matches!(self, Self::Terminated)
    }
}

impl Stream for ScanResultStream {
    type Item = bt_gatt::Result<bt_gatt::central::ScanResult>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = Pin::into_inner(self);
        if this.is_terminated() {
            return Poll::Ready(None);
        }
        let Self::Running { proxy, _complete_fut, active_watch, queued } = this else {
            unreachable!()
        };
        if active_watch.is_none() {
            *active_watch = Some(proxy.watch());
        }
        loop {
            if let Some(next) = queued.pop() {
                return Poll::Ready(Some(Ok(to_gatt_scan_result(&next))));
            }
            if let Some(fut) = active_watch {
                let watch_result = futures::ready!(fut.poll_unpin(cx));
                let Ok(mut new_peers) = watch_result else {
                    *this = Self::Terminated;
                    return Poll::Ready(Some(Err(types::Error::Other(Box::new(
                        watch_result.unwrap_err(),
                    )))));
                };
                queued.append(&mut new_peers);
                *active_watch = Some(proxy.watch());
            }
        }
    }
}

enum ReadQueryFut {
    Char(QueryResponseFut<fidl_gatt2::RemoteServiceReadCharacteristicResult>),
    Desc(QueryResponseFut<fidl_gatt2::RemoteServiceReadDescriptorResult>),
}

enum QueryError {
    Gatt(bt_gatt::types::Error),
    Fidl(fidl::Error),
}

impl Future for ReadQueryFut {
    type Output = std::result::Result<fidl_gatt2::ReadValue, QueryError>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let res = match self.get_mut() {
            ReadQueryFut::Char(c) => match futures::ready!(c.poll_unpin(cx)) {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(e)) => Err(QueryError::Gatt(to_gatt_gatt_error(&e))),
                Err(fidl_error) => Err(QueryError::Fidl(fidl_error)),
            },
            ReadQueryFut::Desc(c) => match futures::ready!(c.poll_unpin(cx)) {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(e)) => Err(QueryError::Gatt(to_gatt_gatt_error(&e))),
                Err(fidl_error) => Err(QueryError::Fidl(fidl_error)),
            },
        };
        Poll::Ready(res)
    }
}

pub struct ReadFuture<'a> {
    peer_id: bt_common::PeerId,
    read_fut: ReadQueryFut,
    target: &'a mut [u8],
}

impl Future for ReadFuture<'_> {
    type Output = Result<(usize, bool)>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        match futures::ready!(self.read_fut.poll_unpin(cx)) {
            Ok(fidl_gatt2::ReadValue { value, maybe_truncated, .. }) => {
                let value = value.unwrap();
                self.target[..value.len()].copy_from_slice(value.as_slice());
                Poll::Ready(Ok((value.len(), maybe_truncated.unwrap())))
            }
            Err(QueryError::Gatt(e)) => Poll::Ready(Err(e)),
            Err(QueryError::Fidl(fidl_error)) => {
                if fidl_error.is_closed() {
                    Poll::Ready(Err(bt_gatt::types::Error::PeerDisconnected(self.peer_id)))
                } else {
                    Poll::Ready(Err(bt_gatt::types::Error::Other(Box::new(fidl_error))))
                }
            }
        }
    }
}

enum WriteQueryFut {
    Char(QueryResponseFut<fidl_gatt2::RemoteServiceWriteCharacteristicResult>),
    Desc(QueryResponseFut<fidl_gatt2::RemoteServiceWriteDescriptorResult>),
}

impl Future for WriteQueryFut {
    type Output = std::result::Result<(), QueryError>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let res = match self.get_mut() {
            WriteQueryFut::Char(c) => match futures::ready!(c.poll_unpin(cx)) {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(QueryError::Gatt(to_gatt_gatt_error(&e))),
                Err(fidl_error) => Err(QueryError::Fidl(fidl_error)),
            },
            WriteQueryFut::Desc(c) => match futures::ready!(c.poll_unpin(cx)) {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(QueryError::Gatt(to_gatt_gatt_error(&e))),
                Err(fidl_error) => Err(QueryError::Fidl(fidl_error)),
            },
        };
        Poll::Ready(res)
    }
}

pub struct WriteFuture<'a> {
    peer_id: bt_common::PeerId,
    write_fut: WriteQueryFut,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

impl WriteFuture<'_> {
    fn new(peer_id: bt_common::PeerId, write_fut: WriteQueryFut) -> Self {
        Self { peer_id, write_fut, _lifetime: std::marker::PhantomData }
    }
}

impl Future for WriteFuture<'_> {
    type Output = Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        match futures::ready!(self.write_fut.poll_unpin(cx)) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(QueryError::Gatt(error)) => Poll::Ready(Err(error)),
            Err(QueryError::Fidl(fidl_error)) => {
                if fidl_error.is_closed() {
                    Poll::Ready(Err(bt_gatt::types::Error::PeerDisconnected(self.peer_id)))
                } else {
                    Poll::Ready(Err(bt_gatt::types::Error::Other(Box::new(fidl_error))))
                }
            }
        }
    }
}

pub struct CharacteristicNotificationStream {
    peer_id: bt_common::PeerId,
    error: Option<bt_gatt::types::Error>,
    stream: Option<fidl_gatt2::CharacteristicNotifierRequestStream>,
    result: Option<QueryResponseFut<fidl_gatt2::RemoteServiceRegisterCharacteristicNotifierResult>>,
}

impl Stream for CharacteristicNotificationStream {
    type Item = Result<client::CharacteristicNotification>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let Self { error, stream, peer_id, result } = self.get_mut();
        loop {
            if let Some(error) = error.take() {
                return Poll::Ready(Some(Err(error)));
            }
            if let Some(result_fut) = result {
                if let Poll::Ready(maybe_error) = result_fut.poll_unpin(cx) {
                    *result = None;
                    match maybe_error {
                        Ok(Ok(())) => {}
                        Ok(Err(gatt_error)) => {
                            *error = Some(to_gatt_gatt_error(&gatt_error));
                            continue;
                        }
                        Err(fidl_error) => {
                            *error = Some(bt_gatt::types::Error::Other(Box::new(fidl_error)));
                            continue;
                        }
                    }
                }
            }
            if let Some(next) = stream.as_mut() {
                let next = futures::ready!(next.poll_next_unpin(cx));
                let res = match next {
                    Some(Ok(fidl_gatt2::CharacteristicNotifierRequest::OnNotification {
                        value,
                        responder,
                    })) => {
                        let _ = responder.send();
                        Some(Ok(client::CharacteristicNotification {
                            handle: to_gatt_handle(&value.handle.unwrap()),
                            value: value.value.unwrap(),
                            maybe_truncated: value.maybe_truncated.unwrap(),
                        }))
                    }
                    Some(Err(fidl_error)) => {
                        *stream = None;
                        *error = Some(if fidl_error.is_closed() {
                            bt_gatt::types::Error::PeerDisconnected(*peer_id)
                        } else {
                            bt_gatt::types::Error::Other(Box::new(fidl_error))
                        });
                        continue;
                    }
                    None => {
                        *stream = None;
                        None
                    }
                };
                return Poll::Ready(res);
            }
            panic!("Polled while is_terminated");
        }
    }
}

pub struct CharacteristicResultFut {
    get_characteristics_fut: QueryResponseFut<Vec<fidl_gatt2::Characteristic>>,
    filter_uuid: Option<Uuid>,
}

impl Future for CharacteristicResultFut {
    type Output = Result<Vec<types::Characteristic>>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result = futures::ready!(this.get_characteristics_fut.poll_unpin(cx));
        let Ok(vec) = result else {
            return Poll::Ready(Err(types::Error::Other(Box::new(result.unwrap_err()))));
        };
        let chars = vec.iter().map(to_gatt_characteristic);
        let chars = if let Some(uuid) = this.filter_uuid {
            chars.filter(|c| c.uuid == uuid).collect()
        } else {
            chars.collect()
        };
        // TODO: Fetch the well-known Descriptors for these Characteristics.
        Poll::Ready(Ok(chars))
    }
}

pub struct PeerService {
    peer_id: bt_common::PeerId,
    proxy: fidl_gatt2::RemoteServiceProxy,
}

impl bt_gatt::client::PeerService<FuchsiaTypes> for PeerService {
    fn discover_characteristics(&self, uuid: Option<Uuid>) -> CharacteristicResultFut {
        let get_characteristics_fut = self.proxy.discover_characteristics();
        CharacteristicResultFut { get_characteristics_fut, filter_uuid: uuid }
    }

    fn read_characteristic<'a>(
        &self,
        handle: &types::Handle,
        offset: u16,
        buf: &'a mut [u8],
    ) -> <FuchsiaTypes as GattTypes>::ReadFut<'a> {
        let max_bytes = buf.len().try_into().unwrap_or(u16::MAX);
        let read_fut = self.proxy.read_characteristic(
            &to_fidl_handle(handle),
            &fidl_gatt2::ReadOptions::LongRead(fidl_gatt2::LongReadOptions {
                offset: Some(offset),
                max_bytes: Some(max_bytes),
                ..Default::default()
            }),
        );
        ReadFuture { peer_id: self.peer_id, read_fut: ReadQueryFut::Char(read_fut), target: buf }
    }

    fn write_characteristic<'a>(
        &self,
        handle: &types::Handle,
        mode: types::WriteMode,
        offset: u16,
        buf: &'a [u8],
    ) -> <FuchsiaTypes as GattTypes>::WriteFut<'a> {
        let write_fut = self.proxy.write_characteristic(
            &to_fidl_handle(handle),
            buf,
            &fidl_gatt2::WriteOptions {
                write_mode: Some(to_fidl_writemode(&mode)),
                offset: Some(offset),
                ..Default::default()
            },
        );
        WriteFuture::new(self.peer_id, WriteQueryFut::Char(write_fut))
    }

    fn read_descriptor<'a>(
        &self,
        handle: &types::Handle,
        offset: u16,
        buf: &'a mut [u8],
    ) -> <FuchsiaTypes as GattTypes>::ReadFut<'a> {
        let max_bytes = buf.len().try_into().unwrap_or(u16::MAX);
        let read_fut = self.proxy.read_descriptor(
            &to_fidl_handle(handle),
            &fidl_gatt2::ReadOptions::LongRead(fidl_gatt2::LongReadOptions {
                offset: Some(offset),
                max_bytes: Some(max_bytes),
                ..Default::default()
            }),
        );
        ReadFuture { peer_id: self.peer_id, read_fut: ReadQueryFut::Desc(read_fut), target: buf }
    }

    fn write_descriptor<'a>(
        &self,
        handle: &types::Handle,
        offset: u16,
        buf: &'a [u8],
    ) -> <FuchsiaTypes as GattTypes>::WriteFut<'a> {
        let write_fut = self.proxy.write_descriptor(
            &to_fidl_handle(handle),
            buf,
            &fidl_gatt2::WriteOptions { offset: Some(offset), ..Default::default() },
        );
        WriteFuture::new(self.peer_id, WriteQueryFut::Desc(write_fut))
    }

    fn subscribe(&self, handle: &types::Handle) -> <FuchsiaTypes as GattTypes>::NotificationStream {
        let (client, stream) =
            fidl::endpoints::create_request_stream::<fidl_gatt2::CharacteristicNotifierMarker>();
        let notifier_fut =
            self.proxy.register_characteristic_notifier(&to_fidl_handle(handle), client);
        CharacteristicNotificationStream {
            peer_id: self.peer_id,
            error: None,
            stream: Some(stream),
            result: Some(notifier_fut),
        }
    }
}

pub struct PeerServiceHandle {
    peer_id: bt_common::PeerId,
    uuid: Uuid,
    service_info: fidl_gatt2::ServiceInfo,
    handle: fidl_gatt2::ServiceHandle,
    proxy: fidl_gatt2::ClientProxy,
}

impl bt_gatt::client::PeerServiceHandle<FuchsiaTypes> for PeerServiceHandle {
    fn uuid(&self) -> Uuid {
        self.uuid
    }

    fn is_primary(&self) -> bool {
        self.service_info.kind.map_or(false, |k| k == fidl_gatt2::ServiceKind::Primary)
    }

    fn connect(&self) -> <FuchsiaTypes as GattTypes>::ServiceConnectFut {
        let (proxy, server_end) =
            fidl::endpoints::create_proxy::<fidl_gatt2::RemoteServiceMarker>();
        if let Err(e) = self.proxy.connect_to_service(&self.handle, server_end) {
            return futures::future::ready(Err(types::Error::Other(Box::new(e))));
        }
        futures::future::ready(Ok(PeerService { peer_id: self.peer_id, proxy }))
    }
}

#[derive(Clone)]
pub struct Client {
    peer_id: PeerId,
    _connection_proxy: fidl_le::ConnectionProxy,
    client_proxy: fidl_gatt2::ClientProxy,
    watched_uuid: Arc<Mutex<Option<fidl_bt::Uuid>>>,
    known_services: Arc<Mutex<HashMap<u64, fidl_gatt2::ServiceInfo>>>,
}

impl Client {
    fn new(
        peer_id: PeerId,
        connection_proxy: ConnectionProxy,
        client_proxy: fidl_gatt2::ClientProxy,
    ) -> Self {
        Client {
            peer_id,
            _connection_proxy: connection_proxy,
            client_proxy,
            watched_uuid: Default::default(),
            known_services: Default::default(),
        }
    }
}

/// Time to wait for services update from a peer on a hanging get.
const SERVICE_UPDATE_TIMEOUT: fasync::MonotonicDuration =
    fasync::MonotonicDuration::from_seconds(3);

impl bt_gatt::Client<FuchsiaTypes> for Client {
    fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    fn find_service(&self, uuid: Uuid) -> <FuchsiaTypes as GattTypes>::FindServicesFut {
        let fidl_uuid = to_fidl_uuid(&uuid);
        fasync::Task::spawn({
            let watched_uuid = self.watched_uuid.clone();
            let known_services = self.known_services.clone();
            let client_proxy = self.client_proxy.clone();
            let peer_id = self.peer_id;
            let timeout = fasync::MonotonicInstant::after(SERVICE_UPDATE_TIMEOUT);
            async move {
                let result = client_proxy
                    .watch_services(&[fidl_uuid])
                    .on_timeout(timeout, || Ok((Vec::new(), Vec::new())))
                    .await;
                let Ok((added, removed)) = result else {
                    return Err(types::Error::Other(Box::new(result.unwrap_err())));
                };
                let mut watched_uuid = watched_uuid.lock();
                let mut known_services = known_services.lock();
                match *watched_uuid {
                    Some(current) if current == fidl_uuid => {
                        removed
                            .into_iter()
                            .for_each(|handle| drop(known_services.remove(&handle.value)));
                    }
                    _ => {
                        known_services.clear();
                        *watched_uuid = Some(fidl_uuid);
                    }
                };
                for info in added {
                    // updating a known service is okay, new info will be the most up-to-date
                    let _ = known_services.insert(info.handle.unwrap().value, info);
                }
                let services = known_services
                    .iter()
                    .map(|(handle, service_info)| PeerServiceHandle {
                        peer_id,
                        uuid,
                        service_info: service_info.clone(),
                        handle: fidl_gatt2::ServiceHandle { value: *handle },
                        proxy: client_proxy.clone(),
                    })
                    .collect();
                Ok(services)
            }
        })
    }
}

pub struct Server {
    proxy: fidl_fuchsia_bluetooth_gatt2::Server_Proxy,
}

impl Server {
    pub fn new(proxy: fidl_gatt2::Server_Proxy) -> Self {
        Self { proxy }
    }
}

fn to_fidl_desc(gatt: &bt_gatt::types::Descriptor) -> fidl_gatt2::Descriptor {
    fidl_gatt2::Descriptor {
        handle: Some(to_fidl_handle(&gatt.handle)),
        type_: Some(to_fidl_uuid(&(&gatt.r#type).into())),
        permissions: Some(to_fidl_permissions(&gatt.permissions)),
        ..Default::default()
    }
}

fn to_fidl_levels(gatt: &bt_gatt::types::SecurityLevels) -> fidl_gatt2::SecurityRequirements {
    fidl_gatt2::SecurityRequirements {
        encryption_required: Some(gatt.encryption),
        authentication_required: Some(gatt.authentication),
        authorization_required: Some(gatt.authorization),
        ..Default::default()
    }
}

fn to_fidl_permissions(
    gatt: &bt_gatt::types::AttributePermissions,
) -> fidl_gatt2::AttributePermissions {
    fidl_gatt2::AttributePermissions {
        read: gatt.read.as_ref().map(to_fidl_levels),
        write: gatt.write.as_ref().map(to_fidl_levels),
        update: gatt.update.as_ref().map(to_fidl_levels),
        ..Default::default()
    }
}

fn to_fidl_char(gatt: &bt_gatt::types::Characteristic) -> fidl_gatt2::Characteristic {
    // Property bits match between bt_gatt and fidl_gatt2
    let properties = fidl_gatt2::CharacteristicPropertyBits::from_bits(
        gatt.properties.0.iter().fold(0, |acc, prop| acc | (*prop as u16)),
    );
    fidl_gatt2::Characteristic {
        handle: Some(to_fidl_handle(&gatt.handle)),
        type_: Some(to_fidl_uuid(&gatt.uuid)),
        properties,
        permissions: Some(to_fidl_permissions(&gatt.permissions)),
        descriptors: Some(gatt.descriptors().map(to_fidl_desc).collect()),
        ..Default::default()
    }
}

fn from_gatt_service_definition(gatt_def: server::ServiceDefinition) -> fidl_gatt2::ServiceInfo {
    let mut res = fidl_gatt2::ServiceInfo::default();
    let service_id: u64 = gatt_def.id().into();
    res.handle = Some(fidl_gatt2::ServiceHandle { value: service_id });
    let kind = match gatt_def.kind() {
        bt_gatt::types::ServiceKind::Primary => fidl_gatt2::ServiceKind::Primary,
        bt_gatt::types::ServiceKind::Secondary => fidl_gatt2::ServiceKind::Secondary,
    };
    res.kind = Some(kind);
    res.type_ = Some(to_fidl_uuid(&gatt_def.uuid()));
    res.characteristics = Some(gatt_def.characteristics().map(to_fidl_char).collect());
    res
}

impl bt_gatt::Server<FuchsiaTypes> for Server {
    fn prepare(
        &self,
        service: server::ServiceDefinition,
    ) -> <FuchsiaTypes as ServerTypes>::LocalServiceFut {
        let info = from_gatt_service_definition(service);
        let (client, request_stream) = fidl::endpoints::create_request_stream::<
            fidl_fuchsia_bluetooth_gatt2::LocalServiceMarker,
        >();
        LocalServiceFut {
            future: self.proxy.publish_service(&info, client),
            request_stream: Some(request_stream),
        }
    }
}

pub struct LocalServiceFut {
    future: QueryResponseFut<ServerPublishServiceResult>,
    request_stream: Option<LocalServiceRequestStream>,
}

impl Future for LocalServiceFut {
    type Output = Result<LocalService>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let result = futures::ready!(self.future.poll_unpin(cx));
        let stream = self.request_stream.take().expect("polled after terminated");
        match result {
            Ok(Ok(())) => Poll::Ready(Ok(stream.into())),
            Ok(Err(e)) => {
                use bt_gatt::types::Error;
                use fidl_fuchsia_bluetooth_gatt2::PublishServiceError::*;
                let gatt_err = match e {
                    InvalidServiceHandle => Error::from("Invalid service handle"),
                    InvalidUuid => Error::from("Invalid UUID"),
                    InvalidCharacteristics => Error::from("Invalid Characteristics"),
                    _ => Error::from("Sapphire stack error"),
                };
                Poll::Ready(Err(gatt_err))
            }
            Err(fidl_err) => Poll::Ready(Err(bt_gatt::types::Error::other(fidl_err))),
        }
    }
}

impl FusedFuture for LocalServiceFut {
    fn is_terminated(&self) -> bool {
        self.request_stream.is_none()
    }
}

enum WaitingSendItem {
    Notification(ValueChangedParameters),
    Indication(ValueChangedParameters, EventPair),
}

struct ServiceSender {
    credits: Arc<Mutex<u32>>,
    waiting: Arc<Mutex<VecDeque<WaitingSendItem>>>,
    control_handle: LocalServiceControlHandle,
}

impl ServiceSender {
    fn new(control_handle: LocalServiceControlHandle) -> Self {
        Self {
            credits: Arc::new(Mutex::new(fidl_gatt2::INITIAL_VALUE_CHANGED_CREDITS)),
            waiting: Default::default(),
            control_handle,
        }
    }

    fn defunct() -> Self {
        let (_, closed) = zx::Channel::create();
        let dead = fidl_gatt2::LocalServiceRequestStream::from_channel(
            fasync::Channel::from_channel(closed),
        );
        Self::new(dead.control_handle())
    }

    fn add_notification(&self, params: ValueChangedParameters) {
        self.waiting.lock().push_back(WaitingSendItem::Notification(params));
        self.try_send();
    }

    fn add_indication(&self, params: ValueChangedParameters, pair: EventPair) {
        self.waiting.lock().push_back(WaitingSendItem::Indication(params, pair));
        self.try_send();
    }

    fn add_credits(&self, additional: u32) {
        *self.credits.lock() += additional;
        self.try_send();
    }

    fn try_send(&self) {
        let mut credits_lock = self.credits.lock();
        loop {
            if *credits_lock == 0 {
                return;
            }
            let mut waiting_lock = self.waiting.lock();
            let Some(next) = waiting_lock.pop_front() else {
                return;
            };
            *credits_lock -= 1;
            let res = match next {
                WaitingSendItem::Notification(params) => {
                    self.control_handle.send_on_notify_value(&params)
                }
                WaitingSendItem::Indication(params, pair) => {
                    self.control_handle.send_on_indicate_value(&params, pair)
                }
            };
            if res.is_err() {
                return;
            }
        }
    }
}

pub struct LocalService {
    // The request stream. None if the service has been published.
    stream: Mutex<Option<LocalServiceRequestStream>>,
    sender: Arc<ServiceSender>,
}

impl From<LocalServiceRequestStream> for LocalService {
    fn from(value: LocalServiceRequestStream) -> Self {
        let sender = Arc::new(ServiceSender::new(value.control_handle()));
        Self { stream: Mutex::new(Some(value)), sender }
    }
}

impl bt_gatt::server::LocalService<FuchsiaTypes> for LocalService {
    fn publish(&self) -> <FuchsiaTypes as ServerTypes>::ServiceEventStream {
        match self.stream.lock().take() {
            None => LocalEventStream::error(bt_gatt::types::Error::from("already published")),
            Some(stream) => LocalEventStream::new(stream, self.sender.clone()),
        }
    }

    fn notify(&self, characteristic: &types::Handle, data: &[u8], peers: &[PeerId]) {
        self.sender.add_notification(ValueChangedParameters {
            handle: Some(to_fidl_handle(characteristic)),
            value: Some(data.into()),
            peer_ids: Some(peers.iter().map(to_fidl_peer_id).collect()),
            ..Default::default()
        });
    }

    fn indicate(
        &self,
        characteristic: &types::Handle,
        data: &[u8],
        peers: &[PeerId],
    ) -> <FuchsiaTypes as ServerTypes>::IndicateConfirmationStream {
        let (indication_stream, their_pair) = IndicateConfirmationStream::new(peers.into());

        self.sender.add_indication(
            ValueChangedParameters {
                handle: Some(to_fidl_handle(characteristic)),
                value: Some(data.into()),
                peer_ids: Some(peers.iter().map(to_fidl_peer_id).collect()),
                ..Default::default()
            },
            their_pair,
        );
        indication_stream
    }
}

pub struct LocalEventStream {
    stream: Option<Result<LocalServiceRequestStream>>,
    // Used to add credits and send waiting indications.
    sender: Arc<ServiceSender>,
}

impl LocalEventStream {
    /// Construct a stream that only contains an error.
    fn error(error: bt_gatt::types::Error) -> Self {
        Self { stream: Some(Err(error)), sender: Arc::new(ServiceSender::defunct()) }
    }

    fn new(stream: LocalServiceRequestStream, sender: Arc<ServiceSender>) -> Self {
        Self { stream: Some(Ok(stream)), sender }
    }
}

impl Stream for LocalEventStream {
    type Item = Result<server::ServiceEvent<FuchsiaTypes>>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let sender = self.sender.clone();
        let Some(result) = self.stream.as_mut() else {
            return Poll::Ready(None);
        };
        let Ok(stream) = result.as_mut() else {
            let result = self.stream.take();
            return Poll::Ready(Some(Err(result.unwrap().err().unwrap())));
        };
        loop {
            let Some(res) = futures::ready!(stream.poll_next_unpin(cx)) else {
                self.stream = None;
                return Poll::Ready(None);
            };
            let Ok(request) = res else {
                self.stream = None;
                return Poll::Ready(Some(Err(bt_gatt::types::Error::other(res.unwrap_err()))));
            };
            use bt_gatt::server::ServiceEvent;
            use fidl_fuchsia_bluetooth_gatt2::LocalServiceRequest::*;
            use fidl_fuchsia_bluetooth_gatt2::{
                LocalServicePeerUpdateRequest, LocalServiceWriteValueRequest,
            };
            match request {
                CharacteristicConfiguration { peer_id, handle, notify, indicate, responder } => {
                    let indicate_type = match (notify, indicate) {
                        (_, true) => bt_gatt::server::NotificationType::Indicate,
                        (true, false) => bt_gatt::server::NotificationType::Notify,
                        (false, false) => bt_gatt::server::NotificationType::Disable,
                    };
                    let _ = responder.send();
                    return Poll::Ready(Some(Ok(ServiceEvent::ClientConfiguration {
                        peer_id: to_gatt_peer_id(&peer_id),
                        handle: to_gatt_handle(&handle),
                        notification_type: indicate_type,
                    })));
                }
                ReadValue { peer_id, handle, offset, responder } => {
                    let responder = ReadResponder { responder };
                    return Poll::Ready(Some(Ok(ServiceEvent::Read {
                        peer_id: to_gatt_peer_id(&peer_id),
                        handle: to_gatt_handle(&handle),
                        offset: offset.try_into().unwrap(),
                        responder,
                    })));
                }
                WriteValue {
                    payload: LocalServiceWriteValueRequest { peer_id, handle, offset, value, .. },
                    responder,
                } => {
                    let responder = WriteResponder { responder };
                    return Poll::Ready(Some(Ok(ServiceEvent::Write {
                        peer_id: to_gatt_peer_id(&peer_id.unwrap()),
                        handle: to_gatt_handle(&handle.unwrap()),
                        offset: offset.unwrap().try_into().unwrap(),
                        value: value.unwrap(),
                        responder,
                    })));
                }
                PeerUpdate {
                    payload: LocalServicePeerUpdateRequest { peer_id, mtu, .. },
                    responder,
                } => {
                    let _ = responder.send();
                    return Poll::Ready(Some(Ok(ServiceEvent::peer_info(
                        to_gatt_peer_id(&peer_id.unwrap()),
                        mtu,
                        None,
                    ))));
                }
                ValueChangedCredit { additional_credit, control_handle: _ } => {
                    sender.add_credits(additional_credit as u32);
                }
            }
        }
    }
}

pub struct IndicateConfirmationStream {
    event: Option<Pin<Box<dyn Future<Output = std::result::Result<zx::Signals, zx::Status>>>>>,
    peers: Vec<PeerId>,
}

impl IndicateConfirmationStream {
    fn new(peers: Vec<PeerId>) -> (Self, EventPair) {
        let (ours, theirs) = fidl::EventPair::create();
        let signals = fuchsia_async::OnSignals::new(
            ours,
            zx::Signals::EVENTPAIR_SIGNALED | zx::Signals::EVENTPAIR_PEER_CLOSED,
        );
        (Self { event: Some(Box::pin(signals)), peers }, theirs)
    }
}

impl Stream for IndicateConfirmationStream {
    type Item = Result<bt_gatt::server::ConfirmationEvent>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        loop {
            let Some(signals) = self.event.as_mut() else {
                match self.peers.pop() {
                    None => return Poll::Ready(None),
                    Some(peer_id) => {
                        return Poll::Ready(Some(Ok(
                            bt_gatt::server::ConfirmationEvent::create_ack(peer_id),
                        )));
                    }
                }
            };
            let signal = futures::ready!(signals.as_mut().poll(cx));
            self.event = None;
            use bt_gatt::types::Error;
            match signal {
                // Continue to the top of the loop to start draining the ack queue
                Ok(zx::Signals::EVENTPAIR_SIGNALED) => continue,
                Ok(zx::Signals::EVENTPAIR_PEER_CLOSED) => {
                    self.peers.clear();
                    return Poll::Ready(Some(Err(Error::from("Peer not subscribed or timed out"))));
                }
                Ok(sig) => {
                    self.peers.clear();
                    return Poll::Ready(Some(Err(Error::from(format!(
                        "Unexpected signal: {sig:?}",
                    )))));
                }
                Err(e) => {
                    self.peers.clear();
                    return Poll::Ready(Some(Err(Error::from(format!(
                        "Error on pair wait: {e:?}",
                    )))));
                }
            }
        }
    }
}

pub struct ReadResponder {
    responder: fidl_gatt2::LocalServiceReadValueResponder,
}

impl server::ReadResponder for ReadResponder {
    fn respond(self, value: &[u8]) {
        let _ = self.responder.send(Ok(value.into()));
    }

    fn error(self, error: types::GattError) {
        let _ = self.responder.send(Err(to_fidl_gatt_error(&error)));
    }
}

pub struct WriteResponder {
    responder: fidl_gatt2::LocalServiceWriteValueResponder,
}

impl server::WriteResponder for WriteResponder {
    fn acknowledge(self) {
        let _ = self.responder.send(Ok(()));
    }

    fn error(self, error: types::GattError) {
        let _ = self.responder.send(Err(to_fidl_gatt_error(&error)));
    }
}
