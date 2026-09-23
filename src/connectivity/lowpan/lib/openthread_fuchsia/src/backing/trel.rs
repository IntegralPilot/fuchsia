// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use crate::to_escaped_string::*;
use anyhow::Context as _;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_net_mdns::*;
use fuchsia_async::Task;
use futures::stream::FusedStream;
use openthread_sys::*;
use ot::{PlatTrel as _, TrelCounters};
use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::task::{Context, Poll};

pub(crate) struct TrelInstance {
    socket: fasync::net::UdpSocket,
    publication_responder: Option<Task<Result<(), anyhow::Error>>>,
    instance_name: String,
    peer_instance_sockaddr_map: HashMap<String, ot::SockAddr>,

    #[allow(dead_code)] // This field must be kept around for <https://fxbug.dev/42182233>
    subscriber: ServiceSubscriber2Proxy,

    subscriber_request_stream: ServiceSubscriptionListenerRequestStream,

    counters: RefCell<TrelCounters>,
}

// Converts an optional vector of strings to a single DNS-compatible string.
fn flatten_txt(txt: Option<Vec<Vec<u8>>>) -> Vec<u8> {
    let mut ret = vec![];

    for mut txt in txt.iter().flat_map(|x| x.iter()).map(Vec::as_slice) {
        if txt.len() > u8::MAX as usize {
            // Limit the size of the records to 255 characters.
            txt = &txt[0..(u8::MAX as usize) + 1];
        }
        ret.push(u8::try_from(txt.len()).unwrap());
        ret.extend_from_slice(txt);
    }

    ret
}

/// Converts an iterator over [`fidl_fuchsia_net::SocketAddress`]es to a vector of
/// [`ot::Ip6Address`]es and a port.
fn process_addresses_from_socket_addresses<
    T: IntoIterator<Item = fidl_fuchsia_net::SocketAddress>,
>(
    addresses: T,
) -> (Vec<ot::Ip6Address>, Option<u16>) {
    let mut ret_port: Option<u16> = None;
    let mut addresses =
        addresses
            .into_iter()
            .flat_map(|x| {
                if let fidl_fuchsia_net::SocketAddress::Ipv6(
                    fidl_fuchsia_net::Ipv6SocketAddress { address, port, .. },
                ) = x
                {
                    let addr = ot::Ip6Address::from(address.addr);
                    if ret_port.is_none() {
                        ret_port = Some(port);
                    } else if ret_port != Some(port) {
                        warn!(
                            tag = "trel";
                            "mDNS service has multiple ports for the same service, {:?} != {:?}",
                            ret_port.unwrap(),
                            port
                        );
                    }
                    // Require link-local. Thread requires TREL peers to advertise link-local via mDNS.
                    if ipv6addr_is_unicast_link_local(&addr) {
                        return Some(addr);
                    }
                }
                None
            })
            .collect::<Vec<_>>();
    addresses.sort();
    (addresses, ret_port)
}

/// Returns `true` if the address is a unicast address with link-local scope.
///
/// The official equivalent of this method is [`std::net::Ipv6Addr::is_unicast_link_local()`],
/// however that method is [still experimental](https://github.com/rust-lang/rust/issues/27709).
fn ipv6addr_is_unicast_link_local(addr: &std::net::Ipv6Addr) -> bool {
    (addr.segments()[0] & 0xffc0) == 0xfe80
}

// Splits the TXT record into individual values.
fn split_txt(txt: &[u8]) -> Vec<Vec<u8>> {
    info!(tag = "trel"; "trel:split_txt: Splitting TXT record: {:?}", hex::encode(txt));
    let txt =
        ot::DnsTxtEntryIterator::try_new(txt).expect("can't parse TXT records from OpenThread");
    txt.map(|x| x.expect("can't parse TXT records from OpenThread").to_vec()).collect::<Vec<_>>()
}

impl TrelInstance {
    fn new(instance_name: String) -> Result<TrelInstance, anyhow::Error> {
        let (client, server) = create_endpoints::<ServiceSubscriptionListenerMarker>();

        let subscriber =
            fuchsia_component::client::connect_to_protocol::<ServiceSubscriber2Marker>().unwrap();

        subscriber
            .subscribe_to_service(
                ot::TREL_DNSSD_SERVICE_NAME_WITH_DOT,
                &ServiceSubscriptionOptions { exclude_local: Some(true), ..Default::default() },
                client,
            )
            .context("Unable to subscribe to TREL services")?;

        Ok(TrelInstance {
            socket: fasync::net::UdpSocket::bind(&SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::UNSPECIFIED,
                0,
                0,
                0,
            )))
            .context("Unable to open TREL UDP socket")?,
            publication_responder: None,
            instance_name,
            peer_instance_sockaddr_map: HashMap::default(),
            subscriber,
            subscriber_request_stream: server.into_stream(),
            counters: RefCell::new(TrelCounters::default()),
        })
    }

    fn port(&self) -> u16 {
        self.socket.local_addr().unwrap().port()
    }

    fn register_service(&mut self, port: u16, txt: &[u8]) {
        let txt = split_txt(txt);

        let (client, server) = create_endpoints::<ServiceInstancePublicationResponder_Marker>();

        let publisher =
            fuchsia_component::client::connect_to_protocol::<ServiceInstancePublisherMarker>()
                .unwrap();

        let publish_init_future = publisher
            .publish_service_instance(
                ot::TREL_DNSSD_SERVICE_NAME_WITH_DOT,
                self.instance_name.as_str(),
                &ServiceInstancePublicationOptions::default(),
                client,
            )
            .map(|x| -> Result<(), anyhow::Error> {
                match x {
                    Ok(Ok(x)) => Ok(x),
                    Ok(Err(err)) => Err(anyhow::format_err!("{:?}", err)),
                    Err(zx_err) => Err(zx_err.into()),
                }
            });

        let publish_responder_future = server.into_stream().map_err(Into::into).try_for_each(
            move |ServiceInstancePublicationResponder_Request::OnPublication {
                      responder, ..
                  }| {
                let txt = txt.clone();
                let _publisher = publisher.clone();
                async move {
                    responder
                        .send(Ok(&ServiceInstancePublication {
                            port: Some(port),
                            text: Some(txt),
                            ..Default::default()
                        }))
                        .map_err(Into::into)
                }
            },
        );

        let future =
            futures::future::try_join(publish_init_future, publish_responder_future).map_ok(|_| ());

        self.publication_responder = Some(fuchsia_async::Task::spawn(future));
    }

    pub fn handle_service_subscriber_request(
        &mut self,
        ot_instance: &ot::Instance,
        service_subscriber_request: ServiceSubscriptionListenerRequest,
    ) -> Result<(), anyhow::Error> {
        match service_subscriber_request {
            // A DNS-SD IPv6 service instance has been discovered.
            ServiceSubscriptionListenerRequest::OnInstanceDiscovered {
                instance:
                    ServiceInstance {
                        instance: Some(instance_name),
                        addresses: Some(addresses),
                        text_strings,
                        ..
                    },
                responder,
            } => {
                let txt = flatten_txt(text_strings);

                let (addresses, port) = process_addresses_from_socket_addresses(addresses);

                info!(
                    tag = "trel";
                    "ServiceSubscriptionListenerRequest::OnInstanceDiscovered: [PII]({instance_name:?}) port:{port:?} addresses:{addresses:?}"
                );

                // Pick the smallest link-local to be robust to reorderings.
                if let Some(address) = addresses.first() {
                    let sockaddr = ot::SockAddr::new(*address, port.unwrap());

                    self.peer_instance_sockaddr_map.insert(instance_name, sockaddr);

                    let info = ot::PlatTrelPeerInfo::new(false, &txt, sockaddr);
                    info!(tag = "trel"; "otPlatTrelHandleDiscoveredPeerInfo: Adding {:?}", info);
                    ot_instance.plat_trel_handle_discovered_peer_info(&info);
                } else {
                    warn!(
                        tag = "trel";
                        "Peer {instance_name:?} does not have any IPv6 link-local address, ignored"
                    );
                }

                responder.send().context("Unable to respond to OnInstanceDiscovered")?;
            }

            // A DNS-SD IPv6 service instance has changed.
            ServiceSubscriptionListenerRequest::OnInstanceChanged {
                instance:
                    ServiceInstance {
                        instance: Some(instance_name),
                        addresses: Some(addresses),
                        text_strings,
                        ..
                    },
                responder,
            } => {
                let txt = flatten_txt(text_strings);
                let (addresses, port) = process_addresses_from_socket_addresses(addresses);

                info!(
                    tag = "trel";
                    "ServiceSubscriptionListenerRequest::OnInstanceChanged: [PII]({instance_name:?}) port:{port:?} addresses:{addresses:?}"
                );

                // Pick the smallest link-local to be robust to reorderings.
                if let Some(address) = addresses.first() {
                    let sockaddr = ot::SockAddr::new(*address, port.unwrap());

                    if let Some(old_sockaddr) =
                        self.peer_instance_sockaddr_map.insert(instance_name, sockaddr)
                    {
                        if old_sockaddr != sockaddr {
                            // Remove old sockaddr with the same instance name
                            let info_old = ot::PlatTrelPeerInfo::new(true, &[], old_sockaddr);
                            info!(
                                tag = "trel";
                                "otPlatTrelHandleDiscoveredPeerInfo: Removing {:?}", info_old
                            );
                            ot_instance.plat_trel_handle_discovered_peer_info(&info_old);
                        }

                        let info = ot::PlatTrelPeerInfo::new(false, &txt, sockaddr);
                        info!(
                            tag = "trel";
                            "otPlatTrelHandleDiscoveredPeerInfo: Updating {:?}", info
                        );
                        ot_instance.plat_trel_handle_discovered_peer_info(&info);
                    }
                } else {
                    warn!(
                        tag = "trel";
                        "Peer {instance_name:?} does not have any IPv6 link-local address, ignored"
                    );
                }

                responder.send().context("Unable to respond to OnInstanceChanged")?;
            }

            // A DNS-SD IPv6 service instance has been lost.
            ServiceSubscriptionListenerRequest::OnInstanceLost { instance, responder, .. } => {
                info!(
                    tag = "trel";
                    "ServiceSubscriptionListenerRequest::OnInstanceLost [PII]({instance:?})"
                );
                if let Some(sockaddr) = self.peer_instance_sockaddr_map.remove(&instance) {
                    let info = ot::PlatTrelPeerInfo::new(true, &[], sockaddr);
                    info!(tag = "trel"; "otPlatTrelHandleDiscoveredPeerInfo: Removing {:?}", info);
                    ot_instance.plat_trel_handle_discovered_peer_info(&info);
                }

                responder.send().context("Unable to respond to OnInstanceLost")?;
            }

            ServiceSubscriptionListenerRequest::OnInstanceChanged { instance, responder } => {
                warn!(
                    tag = "trel";
                    "ServiceSubscriptionListenerRequest::OnInstanceChanged: [PII]({instance:?})"
                );
                // Skip changes without an IPv6 address.
                responder.send().context("Unable to respond to OnInstanceChanged")?;
            }

            ServiceSubscriptionListenerRequest::OnInstanceDiscovered {
                instance,
                responder,
                ..
            } => {
                warn!(
                    tag = "trel";
                    "ServiceSubscriptionListenerRequest::OnInstanceDiscovered: [PII]({instance:?})"
                );
                // Skip discoveries without an IPv6 address.
                responder.send().context("Unable to respond to OnInstanceDiscovered")?;
            }

            ServiceSubscriptionListenerRequest::OnQuery { resource_type, responder, .. } => {
                info!(
                    tag = "trel";
                    "ServiceSubscriptionListenerRequest::OnQuery: {resource_type:?}"
                );

                // We don't care about queries.
                responder.send().context("Unable to respond to OnQuery")?;
            }
        }
        Ok(())
    }

    pub fn get_trel_counters(&self) -> *const otPlatTrelCounters {
        self.counters.borrow().as_ot_ptr()
    }

    pub fn reset_trel_counters(&self) {
        self.counters.borrow_mut().reset_counters()
    }

    /// Async entrypoint for I/O.
    ///
    /// This is explicitly not `mut` so that `on_trel_send` can be called reentrantly from here.
    pub fn poll_io(&self, instance: &ot::Instance, cx: &mut Context<'_>) {
        let mut buffer = [0u8; crate::UDP_PACKET_MAX_LENGTH];
        loop {
            match self.socket.async_recv_from(&mut buffer, cx) {
                Poll::Ready(Ok((len, sockaddr))) => {
                    let sockaddr: ot::SockAddr = sockaddr.as_socket_ipv6().unwrap().into();
                    debug!(tag = "trel"; "Incoming {} byte TREL packet from {:?}", len, sockaddr);
                    {
                        let mut counters = self.counters.borrow_mut();
                        counters.update_rx_bytes(len.try_into().unwrap());
                        counters.update_rx_packets(1);
                    }
                    instance.plat_trel_handle_received(&buffer[..len], &sockaddr)
                }
                Poll::Ready(Err(err)) => {
                    warn!(tag = "trel"; "Error receiving packet: {:?}", err);
                    break;
                }
                _ => {
                    break;
                }
            }
        }
    }

    /// Async entrypoint for non-I/O
    pub fn poll(&mut self, instance: &ot::Instance, cx: &mut Context<'_>) {
        if let Some(task) = &mut self.publication_responder {
            if let Poll::Ready(x) = task.poll_unpin(cx) {
                warn!(
                    tag = "trel";
                    "TrelInstance: publication_responder finished unexpectedly: {:?}", x
                );
                self.publication_responder = None;
            }
        }

        if !self.subscriber_request_stream.is_terminated() {
            while let Poll::Ready(Some(event)) = self.subscriber_request_stream.poll_next_unpin(cx)
            {
                match event {
                    Ok(event) => {
                        if let Err(err) = self.handle_service_subscriber_request(instance, event) {
                            error!(
                                tag = "trel";
                                "Error handling service subscriber request: {err:?}"
                            );
                        }
                    }
                    Err(err) => {
                        error!(tag = "trel"; "subscriber_request_stream FIDL error: {:?}", err);
                    }
                }
            }
        }
    }
}

impl PlatformBacking {
    fn on_trel_enable(&self, instance: &ot::Instance) -> Result<u16, anyhow::Error> {
        let mut trel = self.trel.borrow_mut();
        if let Some(trel) = trel.as_ref() {
            Ok(trel.port())
        } else {
            let instance_name = hex::encode(instance.get_extended_address().as_slice());
            let trel_instance = TrelInstance::new(instance_name)?;
            let port = trel_instance.port();
            trel.replace(trel_instance);
            Ok(port)
        }
    }

    fn on_trel_disable(&self, _instance: &ot::Instance) {
        self.trel.replace(None);
    }

    fn on_trel_register_service(&self, _instance: &ot::Instance, port: u16, txt: &[u8]) {
        let mut trel = self.trel.borrow_mut();
        if let Some(trel) = trel.as_mut() {
            info!(
                tag = "trel";
                "otPlatTrelRegisterService: port:{} txt:{:?}",
                port,
                txt.to_escaped_string()
            );
            trel.register_service(port, txt);
        } else {
            debug!(tag = "trel"; "otPlatTrelRegisterService: TREL is disabled, cannot register.");
        }
    }

    fn on_trel_send(&self, _instance: &ot::Instance, payload: &[u8], sockaddr: &ot::SockAddr) {
        let trel = self.trel.borrow();
        if let Some(trel) = trel.as_ref() {
            let mut counters = trel.counters.borrow_mut();
            debug!(tag = "trel"; "otPlatTrelSend: {:?} -> {}", sockaddr, hex::encode(payload));
            let mut addr: SocketAddr = (*sockaddr).into();
            if let SocketAddr::V6(ref mut addr_v6) = addr {
                match self.netif_index_backbone {
                    Some(scope_id) => {
                        if addr_v6.scope_id() == 0 {
                            addr_v6.set_scope_id(scope_id);
                        } else if addr_v6.scope_id() != scope_id {
                            warn!(
                                tag = "trel";
                                "otPlatTrelSend: dest addr's scope_id {} does not match infra \
                                interface {}",
                                addr_v6.scope_id(),
                                scope_id
                            );
                        }
                    }
                    None => {
                        if addr_v6.scope_id() == 0 {
                            warn!(
                                tag = "trel";
                                "otPlatTrelSend: No scope_id present for infra interface, dropping"
                            );
                            return;
                        }
                    }
                }
            }
            match trel.socket.send_to(payload, addr).now_or_never() {
                Some(Ok(_)) => {
                    counters.update_tx_bytes(payload.len().try_into().unwrap());
                    counters.update_tx_packets(1);
                }
                Some(Err(err)) => {
                    counters.update_tx_failure(1);
                    warn!(tag = "trel"; "otPlatTrelSend: send_to failed: {:?}", err);
                }
                None => {
                    warn!(tag = "trel"; "otPlatTrelSend: send_to didn't finish immediately");
                }
            }
        } else {
            debug!(tag = "trel"; "otPlatTrelSend: TREL is disabled, cannot send.");
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatTrelEnable(instance: *mut otInstance, port_ptr: *mut u16) {
    match PlatformBacking::on_trel_enable(
        // SAFETY: Must only be called from OpenThread thread,
        unsafe { PlatformBacking::as_ref() },
        // SAFETY: `instance` must be a pointer to a valid `otInstance`,
        //         which is guaranteed by the caller.
        unsafe { ot::Instance::ref_from_ot_ptr(instance) }.unwrap(),
    ) {
        Ok(port) => {
            info!(tag = "trel"; "otPlatTrelEnable: Ready on port {}", port);
            unsafe { *port_ptr = port };
        }
        Err(err) => {
            warn!(tag = "trel"; "otPlatTrelEnable: Unable to start TREL: {:?}", err);
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatTrelDisable(instance: *mut otInstance) {
    PlatformBacking::on_trel_disable(
        // SAFETY: Must only be called from OpenThread thread,
        unsafe { PlatformBacking::as_ref() },
        // SAFETY: `instance` must be a pointer to a valid `otInstance`,
        //         which is guaranteed by the caller.
        unsafe { ot::Instance::ref_from_ot_ptr(instance) }.unwrap(),
    );
    info!(tag = "trel"; "otPlatTrelDisable: Closed.");
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatTrelRegisterService(
    instance: *mut otInstance,
    port: u16,
    txt_data: *const u8,
    txt_len: u8,
) {
    PlatformBacking::on_trel_register_service(
        // SAFETY: Must only be called from OpenThread thread,
        unsafe { PlatformBacking::as_ref() },
        // SAFETY: `instance` must be a pointer to a valid `otInstance`,
        //         which is guaranteed by the caller.
        unsafe { ot::Instance::ref_from_ot_ptr(instance) }.unwrap(),
        port,
        // SAFETY: Caller guarantees either txt_data is valid or txt_len is zero.
        unsafe { std::slice::from_raw_parts(txt_data, txt_len.into()) },
    );
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatTrelSend(
    instance: *mut otInstance,
    payload_data: *const u8,
    payload_len: u16,
    dest: *const otSockAddr,
) {
    PlatformBacking::on_trel_send(
        // SAFETY: Must only be called from OpenThread thread,
        unsafe { PlatformBacking::as_ref() },
        // SAFETY: `instance` must be a pointer to a valid `otInstance`,
        //         which is guaranteed by the caller.
        unsafe { ot::Instance::ref_from_ot_ptr(instance) }.unwrap(),
        // SAFETY: Caller guarantees either payload_data is valid or payload_len is zero.
        unsafe { std::slice::from_raw_parts(payload_data, payload_len.into()) },
        // SAFETY: Caller guarantees dest points to a valid otSockAddr.
        unsafe { ot::SockAddr::ref_from_ot_ptr(dest) }.unwrap(),
    );
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatTrelGetCounters(
    _instance: *mut otInstance,
) -> *const otPlatTrelCounters {
    if let Some(trel) = unsafe { PlatformBacking::as_ref() }.trel.borrow().as_ref() {
        trel.get_trel_counters()
    } else {
        std::ptr::null()
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatTrelNotifyPeerSocketAddressDifference(
    _instance: *mut otsys::otInstance,
    peer_sock_addr: &ot::SockAddr,
    rx_sock_addr: &ot::SockAddr,
) {
    info!(tag = "trel"; "otPlatTrelNotifyPeerSocketAddressDifference: Not Implemented. peer_sock_addr {}, rx_sock_addr {}", peer_sock_addr, rx_sock_addr);
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatTrelResetCounters(_instance: *mut otInstance) {
    if let Some(trel) = unsafe { PlatformBacking::as_ref() }.trel.borrow().as_ref() {
        trel.reset_trel_counters()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_split_txt() {
        assert_eq!(
            split_txt(b"\x13xa=a7bfc4981f4e4d22\x13xp=029c6f4dbae059cb"),
            vec![b"xa=a7bfc4981f4e4d22".to_vec(), b"xp=029c6f4dbae059cb".to_vec()]
        );
    }

    #[test]
    fn test_flatten_txt() {
        assert_eq!(flatten_txt(None), vec![]);
        assert_eq!(flatten_txt(Some(vec![])), vec![]);
        assert_eq!(
            flatten_txt(Some(vec![b"xa=a7bfc4981f4e4d22".to_vec()])),
            b"\x13xa=a7bfc4981f4e4d22".to_vec()
        );
        assert_eq!(
            flatten_txt(Some(vec![
                b"xa=a7bfc4981f4e4d22".to_vec(),
                b"xp=029c6f4dbae059cb".to_vec()
            ])),
            b"\x13xa=a7bfc4981f4e4d22\x13xp=029c6f4dbae059cb".to_vec()
        );
    }
}
