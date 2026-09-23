// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use anyhow::Context as _;
use fidl_fuchsia_net_name::DnsServerWatcherMarker;
use fuchsia_sync::Mutex;
use openthread::ot::DnsUpstream;
use openthread_sys::*;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

const MAX_DNS_RESPONSE_SIZE: usize = 2048;

struct DnsUpstreamQueryRefWrapper(&'static ot::PlatDnsUpstreamQuery);

impl DnsUpstreamQueryRefWrapper {
    fn as_ptr(&self) -> *const ot::PlatDnsUpstreamQuery {
        std::ptr::from_ref(self.0)
    }
}

impl PartialEq for DnsUpstreamQueryRefWrapper {
    fn eq(&self, other: &Self) -> bool {
        self.as_ptr().eq(&other.as_ptr())
    }
}

impl Eq for DnsUpstreamQueryRefWrapper {}

impl Hash for DnsUpstreamQueryRefWrapper {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.as_ptr().hash(state);
    }
}

struct Transaction {
    // This field is set but never accessed directly, so we need to silence this warning
    // so that we can compile.
    #[allow(unused)]
    // The task that performs socket poll and forwards the DNS reply from socket.
    task: fasync::Task<Result<(), anyhow::Error>>,
    // Receive the DNS reply from the `task` which stores the corresponding sender.
    receiver: fmpsc::UnboundedReceiver<(DnsUpstreamQueryRefWrapper, Vec<u8>)>,
}

struct LocalDnsServerList {
    // A local copy of the DNS server list
    dns_server_list: Vec<fidl_fuchsia_net_name::DnsServer_>,
    // This field is set but never accessed directly, so we need to silence this warning
    // so that we can compile
    #[allow(unused)]
    // The task that awaits on the DNS server list change.
    task: fasync::Task<Result<(), anyhow::Error>>,
    // Receive the DNS server list from the `task` which stores the corresponding sender.
    receiver: fmpsc::UnboundedReceiver<Vec<fidl_fuchsia_net_name::DnsServer_>>,
}

pub(crate) struct Resolver {
    // The Map that uses the `DnsUpstreamQueryRefWrapper` as key to quickly locate the Transaction
    transactions_map: Arc<Mutex<HashMap<DnsUpstreamQueryRefWrapper, Transaction>>>,
    // Maintains a local DNS record for immediately sending out the DNS upstream query.
    local_dns_record: RefCell<Option<LocalDnsServerList>>,
    waker: Cell<Option<Waker>>,
}

impl Resolver {
    pub fn new() -> Resolver {
        if let Ok(proxy) =
            fuchsia_component::client::connect_to_protocol::<DnsServerWatcherMarker>()
        {
            let (mut sender, receiver) = fmpsc::unbounded();

            // Create a future that await for the latest DNS server list, and forward it to the
            // corresponding receiver. The future is executed in the task in `LocalDnsServerList`.
            let dns_list_watcher_fut = async move {
                loop {
                    let vec = proxy.watch_servers().await?;
                    info!(tag = "resolver"; "getting latest DNS server list: {:?}", vec);
                    if let Err(e) = sender.send(vec).await {
                        warn!(
                            tag = "resolver";
                            "error when sending out latest dns list to process_poll, {:?}", e
                        );
                    }
                }
            };
            Resolver {
                transactions_map: Default::default(),
                waker: Default::default(),
                local_dns_record: RefCell::new(Some(LocalDnsServerList {
                    dns_server_list: Vec::new(),
                    task: fuchsia_async::Task::spawn(dns_list_watcher_fut),
                    receiver,
                })),
            }
        } else {
            warn!(
                tag = "resolver";
                "failed to connect to `DnsServerWatcherMarker`, \
                         DNS upstream query will not be supported"
            );
            Resolver {
                transactions_map: Arc::new(Mutex::new(HashMap::new())),
                waker: Cell::new(None),
                local_dns_record: RefCell::new(None),
            }
        }
    }

    pub fn is_upstream_query_available(&self) -> bool {
        if let Some(local_dns_record) = self.local_dns_record.borrow().as_ref() {
            !local_dns_record.dns_server_list.is_empty()
        } else {
            false
        }
    }

    pub fn process_poll_resolver(&self, instance: &ot::Instance, cx: &mut Context<'_>) {
        // Update the waker so that we can later signal when we need to be polled again
        self.waker.replace(Some(cx.waker().clone()));

        // Poll the DNS server list task
        if let Some(local_dns_record) = self.local_dns_record.borrow_mut().as_mut() {
            while let Poll::Ready(Some(dns_server_list)) =
                local_dns_record.receiver.poll_next_unpin(cx)
            {
                // DNS server watcher proxy returns the new DNS server list when something changed
                // in netstack. The outdated list should be replaced by the new one.
                local_dns_record.dns_server_list = dns_server_list;
            }
        }

        let mut remove_key_vec = Vec::new();
        // Poll the socket in each transaction. If a response is ready, forward it to the OpenThread
        // and remove the corresponding transaction.
        for (_, transaction) in self.transactions_map.lock().iter_mut() {
            while let Poll::Ready(Some((context, message_vec))) =
                transaction.receiver.poll_next_unpin(cx)
            {
                if let Ok(mut message) =
                    ot::Message::udp_new(instance, None).context("cannot create UDP message")
                {
                    match message.append(&message_vec) {
                        Ok(_) => {
                            instance.plat_dns_upstream_query_done(context.0, message);
                        }
                        Err(e) => {
                            warn!(tag = "resolver"; "failed to append to `ot::Message`: {}", e);
                        }
                    }
                } else {
                    warn!(
                        tag = "resolver";
                        "failed to create `ot::Message`, drop the upstream DNS response"
                    );
                }
                remove_key_vec.push(context);
            }
        }

        // cancel the transaction
        for key in remove_key_vec {
            self.transactions_map.lock().remove(&key);
        }
    }

    fn on_start_dns_upstream_query<'a>(
        &self,
        netif_index_backbone: Option<ot::NetifIndex>,
        _instance: &ot::Instance,
        thread_context: &'static ot::PlatDnsUpstreamQuery,
        dns_query: &ot::Message<'_>,
    ) {
        let sockaddr = SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 0 /* ephemeral port */);
        let socket = match fuchsia_async::net::UdpSocket::bind(&sockaddr) {
            Ok(socket) => socket,
            Err(_) => {
                warn!(
                    tag = "resolver";
                    "on_start_dns_upstream_query() failed to create UDP socket, ignoring the query"
                );
                return;
            }
        };

        let query_bytes = dns_query.to_vec();

        // Get the DNS server list, and send out the query to all the available DNS servers.
        if let Some(local_dns_record) = self.local_dns_record.borrow().as_ref() {
            for dns_server in &local_dns_record.dns_server_list {
                if let Some(address) = dns_server.address {
                    match address {
                        fidl_fuchsia_net::SocketAddress::Ipv4(ipv4_sock_addr) => {
                            let sock_addr = SocketAddr::new(
                                std::net::IpAddr::V4(std::net::Ipv4Addr::from(
                                    ipv4_sock_addr.address.addr,
                                )),
                                ipv4_sock_addr.port,
                            );
                            info!(
                                tag = "resolver";
                                "sending DNS query to IPv4 server {}", sock_addr
                            );
                            if let Some(Err(e)) =
                                socket.send_to(&query_bytes, sock_addr).now_or_never()
                            {
                                warn!(
                                    tag = "resolver";
                                    "Failed to send DNS query to IPv4 server {}: {}", sock_addr, e
                                );
                            }
                        }
                        fidl_fuchsia_net::SocketAddress::Ipv6(ipv6_sock_addr) => {
                            let dst_addr = std::net::Ipv6Addr::from(ipv6_sock_addr.address.addr);
                            let mut scope_id = ipv6_sock_addr.zone_index as u32;

                            // If the upstream DNS server uses an IPv6 link-local address without a
                            // scope ID, inject the backbone interface index.
                            if dst_addr.is_unicast_link_local() && scope_id == 0 {
                                if let Some(backbone_id) = netif_index_backbone {
                                    scope_id = backbone_id;
                                } else {
                                    warn!(
                                        tag = "resolver";
                                        "No scope_id present for link-local DNS server {} and \
                                        backbone interface index is unknown",
                                        dst_addr
                                    );
                                    return;
                                }
                            }

                            let sock_addr = std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
                                dst_addr,
                                ipv6_sock_addr.port,
                                0, // flowinfo
                                scope_id,
                            ));

                            info!(
                                tag = "resolver";
                                "sending DNS query to IPv6 server {}", sock_addr
                            );
                            if let Some(Err(e)) =
                                socket.send_to(&query_bytes, sock_addr).now_or_never()
                            {
                                warn!(
                                    tag = "resolver";
                                    "Failed to send DNS query to IPv6 server {}: {}", sock_addr, e
                                );
                            }
                        }
                    }
                }
            }

            let (mut sender, receiver) = fmpsc::unbounded();

            // Create a poll_fn for the socket that can be await on
            let receive_from_fut = futures::future::poll_fn(move |cx| {
                let mut buffer = [0u8; MAX_DNS_RESPONSE_SIZE];
                match socket.async_recv_from(&mut buffer, cx) {
                    Poll::Ready(Ok((len, sockaddr))) => {
                        let message = buffer[..len].to_vec();
                        Poll::Ready(Ok((message, sockaddr)))
                    }
                    Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
                    Poll::Pending => Poll::Pending,
                }
            });

            // Create a future that forward the DNS reply from socket to process_poll
            let fut = async move {
                let (message_vec, sockaddr) =
                    receive_from_fut.await.context("error receiving from dns upstream socket")?;

                let addr_string = sockaddr
                    .as_socket()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|| format!("{:?}", sockaddr));

                info!(
                    tag = "resolver";
                    "Incoming {} bytes DNS response from {:?}",
                    message_vec.len(),
                    addr_string
                );
                if let Err(e) =
                    sender.send((DnsUpstreamQueryRefWrapper(thread_context), message_vec)).await
                {
                    warn!(
                        tag = "resolver";
                        "error when sending out dns upstream reply to process_poll, {:?}", e
                    );
                }
                Ok(())
            };

            // Socket and the sender is owned by the task now
            let transaction = Transaction { task: fuchsia_async::Task::spawn(fut), receiver };

            self.transactions_map
                .lock()
                .insert(DnsUpstreamQueryRefWrapper(thread_context), transaction);
        } else {
            warn!(
                tag = "resolver";
                "on_start_dns_upstream_query() failed to get local_dns_record, ignoring the query"
            );
        }

        // Trigger the waker so that our poll method gets called by the executor
        self.waker.replace(None).and_then(|waker| {
            waker.wake();
            Some(())
        });
    }

    // Cancel the pending query
    fn on_cancel_dns_upstream_query(
        &self,
        _instance: &ot::Instance,
        thread_context: &'static ot::PlatDnsUpstreamQuery,
    ) {
        if let None =
            self.transactions_map.lock().remove(&DnsUpstreamQueryRefWrapper(thread_context))
        {
            warn!(
                tag = "resolver";
                "on_cancel_dns_upstream_query() target transaction not presented for remove, ignoring"
            );
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatDnsStartUpstreamQuery(
    a_instance: *mut otInstance,
    a_txn: *mut otPlatDnsUpstreamQuery,
    a_query: *const otMessage,
) {
    Resolver::on_start_dns_upstream_query(
        &unsafe { PlatformBacking::as_ref() }.resolver,
        unsafe { PlatformBacking::as_ref() }.netif_index_backbone,
        // SAFETY: `instance` must be a pointer to a valid `otInstance`,
        //         which is guaranteed by the caller.
        unsafe { ot::Instance::ref_from_ot_ptr(a_instance) }.unwrap(),
        // SAFETY: no dereference is happening in fuchsia platform side
        unsafe { ot::PlatDnsUpstreamQuery::mut_from_ot_mut_ptr(a_txn) }.unwrap(),
        // SAFETY: caller ensures the dns query is valid
        unsafe { ot::Message::ref_from_ot_ptr(a_query as *mut otMessage) }.unwrap(),
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatDnsIsUpstreamQueryAvailable(a_instance: *mut otInstance) -> bool {
    // The instance parameter is part of the function signature but unused in this implementation.
    // We still get a reference to it to match the pattern of other platform functions.
    let _ = unsafe { ot::Instance::ref_from_ot_ptr(a_instance) };
    unsafe { PlatformBacking::as_ref() }.resolver.is_upstream_query_available()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn otPlatDnsCancelUpstreamQuery(
    a_instance: *mut otInstance,
    a_txn: *mut otPlatDnsUpstreamQuery,
) {
    Resolver::on_cancel_dns_upstream_query(
        &unsafe { PlatformBacking::as_ref() }.resolver,
        // SAFETY: `instance` must be a pointer to a valid `otInstance`,
        //         which is guaranteed by the caller.
        unsafe { ot::Instance::ref_from_ot_ptr(a_instance) }.unwrap(),
        // SAFETY: no dereference is happening in fuchsia platform side
        unsafe { ot::PlatDnsUpstreamQuery::mut_from_ot_mut_ptr(a_txn) }.unwrap(),
    )
}
