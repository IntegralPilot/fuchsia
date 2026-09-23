// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_lock::Mutex;
use async_trait::async_trait;
use fidl_fuchsia_developer_ffx as ffx;
use fidl_fuchsia_net::{IpAddress, Ipv4Address, Ipv6Address};
use fuchsia_async::{Task, Timer};
use futures::FutureExt;
use mdns::protocol as dns;
use netext::{IsLocalAddr, get_mcast_interfaces};
use packet::{InnerPacketBuilder, ParseBuffer};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::unix::prelude::AsRawFd;
use std::sync::{Arc, LazyLock, Weak};
use std::time::Duration;
use timeout::timeout;
use tokio::net::UdpSocket;
use zerocopy::SplitByteSlice;

#[derive(thiserror::Error, Debug)]
pub enum MdnsDiscoveryError {
    #[error("Timeout waiting for target")]
    Timeout,

    #[error("Discovery loop exited early")]
    DiscoveryLoopExited,

    #[error("Failed to receive mDNS event: {0}")]
    ReceiveEvent(#[from] async_channel::RecvError),

    #[error("Failed to parse UTF-8 string: {0}")]
    Utf8Parse(#[from] std::str::Utf8Error),

    #[error("Failed to bind socket to {0}: {1}")]
    BindSocket(std::net::SocketAddr, #[source] std::io::Error),

    #[error("Failed to join multicast group: {0}")]
    JoinMulticast(#[source] std::io::Error),

    #[error("Failed to set socket option: {0}")]
    SetSocketOption(#[source] std::io::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Default mDNS port
pub const MDNS_PORT: u16 = 5353;

pub const MDNS_BROADCAST_INTERVAL: Duration = Duration::from_secs(10);
pub const MDNS_INTERFACE_DISCOVERY_INTERVAL: Duration = Duration::from_secs(1);
pub const MDNS_TTL: u32 = 255;

const MDNS_MCAST_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_MCAST_V6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0x00fb);

#[derive(Debug)]
pub struct CachedTarget {
    target: ffx::TargetInfo,
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    eviction_task: Option<Task<()>>,
}

impl CachedTarget {
    fn new(target: ffx::TargetInfo) -> Self {
        Self { target, eviction_task: None }
    }

    fn new_with_task(target: ffx::TargetInfo, eviction_task: Task<()>) -> Self {
        Self { target, eviction_task: Some(eviction_task) }
    }
}

impl Hash for CachedTarget {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.target.nodename.as_deref().unwrap_or(target_errors::UNKNOWN_TARGET_NAME).hash(state);
    }
}

impl PartialEq for CachedTarget {
    fn eq(&self, other: &CachedTarget) -> bool {
        self.target.nodename.eq(&other.target.nodename)
    }
}

impl Eq for CachedTarget {}

pub struct MdnsProtocol {
    pub events_out: async_channel::Sender<ffx::MdnsEventType>,
    pub target_cache: Mutex<HashSet<CachedTarget>>,
}

impl MdnsProtocol {
    pub async fn handle_target(self: &Arc<Self>, t: ffx::TargetInfo, ttl: u32) {
        let weak = Arc::downgrade(self);
        let t_clone = t.clone();
        let eviction_task = Task::spawn(async move {
            fuchsia_async::Timer::new(Duration::from_secs(ttl.into())).await;
            if let Some(this) = weak.upgrade() {
                this.evict_target(t_clone).await;
            }
        });

        if self
            .target_cache
            .lock()
            .await
            .replace(CachedTarget::new_with_task(t.clone(), eviction_task))
            .is_none()
        {
            self.publish_event(ffx::MdnsEventType::TargetFound(t)).await;
        } else {
            self.publish_event(ffx::MdnsEventType::TargetRediscovered(t)).await
        }
    }

    async fn evict_target(&self, t: ffx::TargetInfo) {
        if self.target_cache.lock().await.remove(&CachedTarget::new(t.clone())) {
            self.publish_event(ffx::MdnsEventType::TargetExpired(t)).await
        }
    }

    async fn publish_event(&self, event: ffx::MdnsEventType) {
        let _ = self.events_out.send(event).await;
    }

    pub async fn target_cache(&self) -> Vec<ffx::TargetInfo> {
        self.target_cache.lock().await.iter().map(|c| c.target.clone()).collect()
    }
}

pub struct DiscoveryConfig {
    pub socket_tasks: Arc<Mutex<HashMap<IpAddr, Task<()>>>>,
    pub mdns_protocol: Weak<MdnsProtocol>,
    pub discovery_interval: Duration,
    pub query_interval: Duration,
    pub ttl: u32,
    pub mdns_port: u16,
}

async fn propagate_bind_event(sock: &UdpSocket, svc: &Weak<MdnsProtocol>) -> u16 {
    let port = match sock.local_addr().unwrap() {
        SocketAddr::V4(s) => s.port(),
        SocketAddr::V6(s) => s.port(),
    };
    if let Some(svc) = svc.upgrade() {
        svc.publish_event(ffx::MdnsEventType::SocketBound(ffx::MdnsBindEvent {
            port: Some(port),
            ..Default::default()
        }))
        .await;
    }
    port
}

#[async_trait]
pub trait MdnsEnabledChecker: Send + Sync {
    async fn enabled(&self) -> bool;
}

struct MdnsEnabled;

#[async_trait]
impl MdnsEnabledChecker for MdnsEnabled {
    async fn enabled(&self) -> bool {
        true
    }
}

/// Returns a TargetInfo of a Fuchsia target discovered via mDNS during the given `duration`
pub async fn discover_target(
    target_name: String,
    listen_duration: Duration,
    mdns_port: u16,
) -> std::result::Result<ffx::TargetInfo, MdnsDiscoveryError> {
    discover_target_by(listen_duration, mdns_port, move |t| {
        t.nodename.as_ref() == Some(&target_name)
    })
    .await
}

pub async fn discover_target_by<F>(
    listen_duration: Duration,
    mdns_port: u16,
    filter: F,
) -> std::result::Result<ffx::TargetInfo, MdnsDiscoveryError>
where
    F: Fn(&ffx::TargetInfo) -> bool + Send + 'static,
{
    let (sender, receiver) = async_channel::bounded::<ffx::MdnsEventType>(1);
    let inner = Arc::new(MdnsProtocol { events_out: sender, target_cache: Default::default() });

    let inner_mv = Arc::downgrade(&inner);

    let discover_task = discovery_loop(
        DiscoveryConfig {
            socket_tasks: Default::default(),
            mdns_protocol: inner_mv,
            discovery_interval: MDNS_INTERFACE_DISCOVERY_INTERVAL,
            query_interval: MDNS_BROADCAST_INTERVAL,
            ttl: MDNS_TTL,
            mdns_port,
        },
        MdnsEnabled {},
    )
    .fuse();
    let discover_task = Box::pin(discover_task);

    //  Okay have a loop that will pull from the receiver until it has a target that matches the
    //  given one
    let loop_task = timeout(listen_duration, loop_for_target(receiver, filter)).fuse();
    let loop_task = Box::pin(loop_task);

    // Wait on either the discovery or the timeout
    match futures::future::select(loop_task, discover_task).await {
        futures::future::Either::Left((loop_res, _)) => match loop_res {
            Ok(Ok(ok)) => Ok(ok),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(MdnsDiscoveryError::Timeout),
        },
        futures::future::Either::Right((_, _)) => {
            return Err(MdnsDiscoveryError::DiscoveryLoopExited);
        }
    }
}

async fn loop_for_target<F>(
    receiver: async_channel::Receiver<ffx::MdnsEventType>,
    filter: F,
) -> std::result::Result<ffx::TargetInfo, MdnsDiscoveryError>
where
    F: Fn(&ffx::TargetInfo) -> bool + Send + 'static,
{
    loop {
        let mdns_event = receiver.recv().await?;
        match mdns_event {
            ffx::MdnsEventType::TargetFound(target_info) => {
                if filter(&target_info) {
                    return Ok(target_info);
                }
            }
            _ => {
                log::warn!("Got an mdns event, but it wasnt a TargetFound so skipping it");
            }
        }
    }
}

/// Returns a Vec<TargetInfo> of Fuchsia targets discovered via mDNS during the given `duration`
pub async fn discover_targets(
    listen_duration: Duration,
    mdns_port: u16,
) -> std::result::Result<Vec<ffx::TargetInfo>, MdnsDiscoveryError> {
    let (sender, receiver) = async_channel::bounded::<ffx::MdnsEventType>(1);
    let inner = Arc::new(MdnsProtocol { events_out: sender, target_cache: Default::default() });

    let inner_mv = Arc::downgrade(&inner);

    let discover_task = timeout(
        listen_duration,
        discovery_loop(
            DiscoveryConfig {
                socket_tasks: Default::default(),
                mdns_protocol: inner_mv,
                discovery_interval: MDNS_INTERFACE_DISCOVERY_INTERVAL,
                query_interval: MDNS_BROADCAST_INTERVAL,
                ttl: MDNS_TTL,
                mdns_port,
            },
            MdnsEnabled {},
        ),
    )
    .fuse();
    let discover_task = Box::pin(discover_task);

    let drain_task = Task::spawn(drain_reciever(receiver)).fuse();
    let drain_task = Box::pin(drain_task);

    // Wait on either the discovery or the timeout
    futures::future::select(discover_task, drain_task).await;

    Ok(inner.as_ref().target_cache().await)
}

async fn drain_reciever(
    receiver: async_channel::Receiver<ffx::MdnsEventType>,
) -> std::result::Result<(), MdnsDiscoveryError> {
    loop {
        match receiver.recv().await {
            Ok(_) => {}
            Err(_) => {
                return Ok(());
            }
        }
    }
}

pub struct MdnsWatcher {
    // Task for the discovery loop
    discovery_task: Option<Task<()>>,
    // Task for the drain loop
    drain_task: Option<Task<()>>,
    // Inner
    inner: Option<Arc<MdnsProtocol>>,
}

pub trait MdnsEventHandler: Send + 'static {
    /// Handles an event.
    fn handle_event(&mut self, event: ffx::MdnsEventType);
}

impl<F> MdnsEventHandler for F
where
    F: FnMut(ffx::MdnsEventType) -> () + Send + 'static,
{
    fn handle_event(&mut self, x: ffx::MdnsEventType) -> () {
        self(x)
    }
}

pub fn recommended_watcher<F>(event_handler: F) -> MdnsWatcher
where
    F: MdnsEventHandler,
{
    MdnsWatcher::new(
        event_handler,
        MDNS_PORT,
        MDNS_INTERFACE_DISCOVERY_INTERVAL,
        MDNS_BROADCAST_INTERVAL,
        MDNS_TTL,
    )
}

impl MdnsWatcher {
    fn new<F>(
        events_out: F,
        mdns_port: u16,
        discovery_interval: Duration,
        query_interval: Duration,
        ttl: u32,
    ) -> Self
    where
        F: MdnsEventHandler,
    {
        let mut res = Self { discovery_task: None, inner: None, drain_task: None };

        let (sender, receiver) = async_channel::bounded::<ffx::MdnsEventType>(1);

        let inner = Arc::new(MdnsProtocol { events_out: sender, target_cache: Default::default() });
        res.inner.replace(inner.clone());

        let inner = Arc::downgrade(&inner);
        res.discovery_task.replace(Task::spawn(discovery_loop(
            DiscoveryConfig {
                socket_tasks: Default::default(),
                mdns_protocol: inner,
                discovery_interval,
                query_interval,
                ttl,
                mdns_port,
            },
            MdnsEnabled {},
        )));

        res.drain_task.replace(Task::spawn(handle_events_loop(receiver, events_out)));

        res
    }
}

async fn handle_events_loop<F>(
    receiver: async_channel::Receiver<ffx::MdnsEventType>,
    mut handler: F,
) where
    F: MdnsEventHandler,
{
    loop {
        let event = receiver.recv().await.expect("MdnsEvent stream closed?");
        handler.handle_event(event);
    }
}

// discovery_loop iterates over all multicast interfaces and adds them to
// the socket_tasks if there is not already a task for that interface.
pub async fn discovery_loop(config: DiscoveryConfig, checker: impl MdnsEnabledChecker + 'static) {
    let DiscoveryConfig {
        socket_tasks,
        mdns_protocol,
        discovery_interval,
        query_interval,
        ttl,
        mdns_port,
    } = config;
    // See https://fxbug.dev/42141030#c10 for details. A macOS system can end up in
    // a situation where the default routes for protocols are on
    // non-functional interfaces, and under such conditions the wildcard
    // listen socket binds will fail. We will repeat attempting to bind
    // them, as newly added interfaces later may unstick the issue, if
    // they introduce new routes. These boolean flags are used to
    // suppress the production of a log output in every interface
    // iteration.
    // In order to manually reproduce these conditions on a macOS
    // system, open Network.prefpane, and for each connection in the
    // list select Advanced... > TCP/IP > Configure IPv6 > Link-local
    // only. Click apply, then restart ffx.
    let mut should_log_v4_listen_error = true;
    let mut should_log_v6_listen_error = true;

    let mut v4_listen_socket: Weak<UdpSocket> = Weak::new();
    let mut v6_listen_socket: Weak<UdpSocket> = Weak::new();

    let checker_strong = Arc::new(checker);
    let checker = Arc::downgrade(&checker_strong);

    let mut recv_ipv4_task: Option<Task<()>> = None;
    let mut recv_ipv6_task: Option<Task<()>> = None;

    loop {
        let should_wait = match checker.upgrade() {
            Some(c) => !c.enabled().await,
            None => false,
        };

        if should_wait {
            Timer::new(discovery_interval).await;
            continue;
        }

        if v4_listen_socket.upgrade().is_none() {
            match make_listen_socket((MDNS_MCAST_V4, mdns_port).into()) {
                Ok(sock) => {
                    // TODO(awdavies): Networking tests appear to fail when
                    // using IPv6. Only propagates the port binding event for
                    // IPv4.
                    let _ = propagate_bind_event(&sock, &mdns_protocol).await;
                    let sock = Arc::new(sock);
                    v4_listen_socket = Arc::downgrade(&sock);
                    let rcv_task =
                        Task::spawn(recv_loop(sock, mdns_protocol.clone(), checker.clone()));
                    if recv_ipv4_task.is_some() {
                        log::warn!(
                            "the IpV4 listen socket was none but we had a task receiving data on it. Replacing the old task"
                        )
                    }
                    recv_ipv4_task.replace(rcv_task);
                    should_log_v4_listen_error = true;
                }
                Err(err) => {
                    if should_log_v4_listen_error {
                        log::error!(
                            "unable to bind IPv4 listen socket: {}. Discovery may fail.",
                            err
                        );
                        should_log_v4_listen_error = false;
                    }
                }
            }
        }

        if v6_listen_socket.upgrade().is_none() && recv_ipv6_task.is_none() {
            match make_listen_socket((MDNS_MCAST_V6, mdns_port).into()) {
                Ok(sock) => {
                    let sock = Arc::new(sock);
                    v6_listen_socket = Arc::downgrade(&sock);
                    let rcv_task =
                        Task::spawn(recv_loop(sock, mdns_protocol.clone(), checker.clone()));
                    if recv_ipv6_task.is_some() {
                        log::warn!(
                            "the IpV6 listen socket was none but we had a task receiving data on it. Replacing the old task"
                        )
                    }
                    recv_ipv6_task.replace(rcv_task);
                    should_log_v6_listen_error = true;
                }
                Err(err) => {
                    if should_log_v6_listen_error {
                        log::error!(
                            "unable to bind IPv6 listen socket: {}. Discovery may fail.",
                            err
                        );
                        should_log_v6_listen_error = false;
                    }
                }
            }
        }

        // As some operating systems will not error sendmsg/recvmsg for UDP
        // sockets bound to addresses that no longer exist, they must be removed
        // by ensuring that they still exist, otherwise we may be sending out
        // unanswerable queries.
        let mut to_delete = HashSet::<IpAddr>::new();
        for ip in socket_tasks.lock().await.keys() {
            to_delete.insert(*ip);
        }

        for iface in get_mcast_interfaces().unwrap_or_default() {
            match iface.id() {
                Ok(id) => {
                    if let Some(sock) = v6_listen_socket.upgrade() {
                        let _ = sock.join_multicast_v6(&MDNS_MCAST_V6, id);
                    }
                }
                Err(err) => {
                    log::warn!("{}", err);
                }
            }

            for addr in iface.addrs.iter() {
                to_delete.remove(&addr.ip());

                let mut addr = *addr;
                addr.set_port(0);

                // TODO(raggi): remove duplicate joins, log unexpected errors
                if let SocketAddr::V4(addr) = addr {
                    if let Some(sock) = v4_listen_socket.upgrade() {
                        let _ = sock.join_multicast_v4(MDNS_MCAST_V4, *addr.ip());
                    }
                }

                if socket_tasks.lock().await.get(&addr.ip()).is_some() {
                    continue;
                }

                let sock = iface
                    .id()
                    .map(|id| match make_sender_socket(id, addr, ttl) {
                        Ok(sock) => Some(sock),
                        Err(err) => {
                            // Moving this to debug from error because there is nothing actionable
                            // for the user.
                            //
                            // On Linux, we see this error most prominently during suspend/resume
                            // cycle of the host. Looking at `journalctl -u avahi-service`, we see
                            // the daemon withdrawing the address, registering a new one, and
                            // invalidating the old address. These messages are classified as info
                            // in the system journal and thus is a normal part of the operation.
                            //
                            // In rust, we get this error when we try to bind the UDP socket.
                            // Because this function is a discovery loop, we retry automatically
                            // and we eventually bind once the avahi service successfully registers
                            // the address.
                            log::debug!("mdns: failed to bind {}: {}", addr, err);
                            None
                        }
                    })
                    .ok()
                    .flatten();

                if sock.is_some() {
                    socket_tasks.lock().await.insert(
                        addr.ip(),
                        Task::spawn(query_recv_loop(
                            Arc::new(sock.unwrap()),
                            addr,
                            mdns_protocol.clone(),
                            query_interval,
                            Arc::downgrade(&socket_tasks),
                            checker.clone(),
                            mdns_port,
                        )),
                    );
                }
            }
        }

        // Drop tasks for IP addresses no longer found on the system.
        {
            let mut tasks = socket_tasks.lock().await;
            for ip in to_delete {
                tasks.remove(&ip);
            }
        }
        Timer::new(discovery_interval).await;
    }
}

fn make_target<B: SplitByteSlice + Copy>(
    src: SocketAddr,
    msg: dns::Message<B>,
) -> Option<(ffx::TargetInfo, u32)> {
    let mut nodename = String::new();
    let mut serial = None;
    let mut ttl = 0u32;
    let mut ssh_port: u16 = 0;
    let mut ssh_address = None;
    let src_info = ffx::TargetAddrInfo::Ip(ffx::TargetIp {
        ip: match &src {
            SocketAddr::V6(s) => IpAddress::Ipv6(Ipv6Address { addr: s.ip().octets() }),
            SocketAddr::V4(s) => IpAddress::Ipv4(Ipv4Address { addr: s.ip().octets() }),
        },
        scope_id: if let SocketAddr::V6(s) = &src { s.scope_id() } else { 0 },
    });
    let mut discovered_addresses = Vec::new();
    if !src.ip().is_multicast() {
        discovered_addresses.push(src_info.clone());
    }
    let fastboot_interface = is_fastboot_response(&msg);

    for record in msg.answers.iter().chain(msg.additional.iter()) {
        match record.rtype {
            // Emulator adds Txt records to share the user mode networking configuration. This information
            // should override any A record information.
            dns::Type::Txt => {
                if let Some(data) = record.rdata.bytes() {
                    let txt_lines: Vec<String> = decode_txt_rdata(data).unwrap_or_default();
                    log::debug!("found text lines: {:#?}", txt_lines);
                    let mut ip_addr: Option<IpAddress> = None;
                    for txt in &txt_lines {
                        if let Some((name, value)) = txt.split_once(':') {
                            match name {
                                "host" => {
                                    if let Ok(addr) = value.parse::<Ipv4Addr>() {
                                        let ip =
                                            IpAddress::Ipv4(Ipv4Address { addr: addr.octets() });
                                        ip_addr = Some(ip);
                                    }
                                }
                                "ssh" => {
                                    ssh_port = value.parse().unwrap_or(22);
                                }
                                _ => {}
                            };
                        }
                        if let Some((name, value)) = txt.split_once('=') {
                            match name {
                                "serial" => {
                                    serial = Some(value.to_string());
                                }
                                _ => {}
                            }
                        }
                    }
                    if let Some(ip) = ip_addr {
                        ssh_address = Some(ffx::TargetIpAddrInfo::IpPort(ffx::TargetIpPort {
                            ip,
                            scope_id: 0,
                            port: ssh_port,
                        }));
                        discovered_addresses.push(ffx::TargetAddrInfo::IpPort(ffx::TargetIpPort {
                            ip,
                            scope_id: 0,
                            port: ssh_port,
                        }));
                    }
                    log::debug!("emulator mdns txt {:?} {:?}", txt_lines, record.domain);
                } else {
                    log::debug!("no data in txt record {:?}", record.domain);
                }
            }
            dns::Type::A => {
                if nodename.is_empty() {
                    write!(nodename, "{}", record.domain).unwrap();
                    nodename = nodename.trim_end_matches(".local").into();
                }
                if ttl == 0 {
                    ttl = record.ttl;
                }
                if let Some(IpAddr::V4(v4)) = record.rdata.ip_addr() {
                    let ip = IpAddress::Ipv4(Ipv4Address { addr: v4.octets() });
                    let target_addr = ffx::TargetAddrInfo::Ip(ffx::TargetIp { ip, scope_id: 0 });
                    if !discovered_addresses.contains(&target_addr) {
                        discovered_addresses.push(target_addr);
                    }
                }
            }
            dns::Type::Aaaa => {
                if nodename.is_empty() {
                    write!(nodename, "{}", record.domain).unwrap();
                    nodename = nodename.trim_end_matches(".local").into();
                }
                if ttl == 0 {
                    ttl = record.ttl;
                }
                if let Some(IpAddr::V6(v6)) = record.rdata.ip_addr() {
                    let ip = IpAddress::Ipv6(Ipv6Address { addr: v6.octets() });
                    let scope_id = if v6.is_link_local_addr()
                        && let ffx::TargetAddrInfo::Ip(ref sip) = src_info
                    {
                        sip.scope_id
                    } else {
                        0
                    };

                    // Only add link-local addresses if we have a valid scope ID.
                    if !v6.is_link_local_addr() || scope_id != 0 {
                        let target_addr = ffx::TargetAddrInfo::Ip(ffx::TargetIp { ip, scope_id });
                        if !discovered_addresses.contains(&target_addr) {
                            discovered_addresses.push(target_addr);
                        }
                    }
                }
            }
            _ => {}
        };
    }

    log::debug!(
        "Making target from message. nodename: {} address: {:#?} fastboot_interface: {:#?} serial: {:#?}",
        nodename,
        discovered_addresses,
        fastboot_interface,
        serial,
    );

    if nodename.is_empty() || ttl == 0 {
        return None;
    }
    Some((
        ffx::TargetInfo {
            nodename: Some(nodename),
            addresses: Some(discovered_addresses),
            serial_number: serial,
            target_state: fastboot_interface.map(|_| ffx::TargetState::Fastboot),
            fastboot_interface,
            ssh_address,
            ..Default::default()
        },
        ttl,
    ))
}

/// Read the bytes from the txt record. These are encoded
/// as <len><string> where len is u8. multiple strings
/// can be in encoded.
fn decode_txt_rdata(data: &[u8]) -> std::result::Result<Vec<String>, MdnsDiscoveryError> {
    // Each text element is preceded by the length
    let mut ret: Vec<String> = vec![];
    let mut pos = 0;
    while pos < data.len() {
        let l: usize = data[pos].into();
        if l == 0 {
            break;
        }
        let s = std::str::from_utf8(&data[pos + 1..pos + l + 1])?;
        ret.push(s.to_string());
        pos = pos + l + 1;
    }
    Ok(ret)
}

// recv_loop reads packets from sock. If the packet is a Fuchsia mdns packet, a
// corresponding mdns event is published to the queue. All other packets are
// silently discarded.
async fn recv_loop(
    sock: Arc<UdpSocket>,
    mdns_protocol: Weak<MdnsProtocol>,
    checker: Weak<impl MdnsEnabledChecker>,
) {
    let should_break = match checker.upgrade() {
        Some(check) => !check.enabled().await,
        None => true,
    };
    if should_break {
        return;
    }

    loop {
        let mut buf = &mut [0u8; 1500][..];
        let addr = match sock.recv_from(buf).await {
            Ok((sz, addr)) => {
                buf = &mut buf[..sz];
                addr
            }
            Err(err) => {
                log::error!("listen socket recv error: {}, mdns listener closed", err);
                return;
            }
        };

        // Note: important, otherwise non-local responders could add themselves.
        if !addr.ip().is_local_addr() {
            continue;
        }

        let msg = match buf.parse::<dns::Message<_>>() {
            Ok(msg) => msg,
            Err(e) => {
                log::trace!(
                    "unable to parse message received on {} from {}: {:?}",
                    sock.local_addr()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|_| format!("fd:{}", sock.as_raw_fd())),
                    addr,
                    e
                );
                continue;
            }
        };

        log::trace!("Socket: {:#?} received message", sock);

        // Only interested in fuchsia services or fastboot.
        if !is_fuchsia_response(&msg) && is_fastboot_response(&msg).is_none() {
            log::trace!("Socket: {:#?} skipping message: as it is not fuchsia or fastboot", sock);
            continue;
        }
        // Source addresses need to be present in the response, or be a TXT record which
        // contains address information about user mode networking being used by an emulator
        // instance.
        if !contains_source_address(&addr, &msg) && !contains_txt_response(&msg) {
            log::debug!(
                "Socket: {:#?} skipping message as it does not contain source address {} or does not contain txt resposne",
                sock,
                addr
            );
            continue;
        }

        if let Some(mdns_protocol) = mdns_protocol.upgrade() {
            if let Some((t, ttl)) = make_target(addr, msg) {
                log::trace!(
                    "packet from {} ({}) on {}",
                    addr,
                    t.nodename.as_deref().unwrap_or(target_errors::UNKNOWN_TARGET_NAME),
                    sock.local_addr().unwrap()
                );
                mdns_protocol.handle_target(t, ttl).await;
            }
        } else {
            return;
        }
    }
}

fn construct_query_buf(service: &str) -> Box<[u8]> {
    let question = dns::QuestionBuilder::new(
        dns::DomainBuilder::from_str(service).unwrap(),
        dns::Type::Ptr,
        dns::Class::In,
        true,
    );

    let mut message = dns::MessageBuilder::new(0, true);
    message.add_question(question);

    let mut buf = vec![0; message.bytes_len()];
    message.serialize(buf.as_mut_slice());
    buf.into_boxed_slice()
}

static QUERY_BUF: LazyLock<[Box<[u8]>; 2]> = LazyLock::new(|| {
    [(construct_query_buf("_fuchsia._udp.local")), (construct_query_buf("_fastboot._tcp.local"))]
});

// query_loop broadcasts an mdns query on sock every interval.
async fn query_loop(sock: Arc<UdpSocket>, interval: Duration, mdns_port: u16) {
    let to_addr: SocketAddr = match sock.local_addr() {
        Ok(SocketAddr::V4(_)) => (MDNS_MCAST_V4, mdns_port).into(),
        Ok(SocketAddr::V6(_)) => (MDNS_MCAST_V6, mdns_port).into(),
        Err(err) => {
            log::error!("resolving local socket addr failed with: {}", err);
            return;
        }
    };

    loop {
        for query_buf in QUERY_BUF.iter() {
            if let Err(err) = sock.send_to(query_buf, to_addr).await {
                // Moving this to debug from error as there is nothing actionable for the user.
                // See the corresponding explanation in discovery_loop fn above.
                //
                // But the premise is that we see this during suspend / resume cycle of the host
                // because the mDNS services for the relevant address and interface are not ready
                // yet.
                log::debug!(
                    "mdns query failed from {}: {}",
                    sock.local_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_else(|_| "unknown".to_string()),
                    err
                );
                return;
            }
        }
        Timer::new(interval).await;
    }
}

// sock is dispatched with a recv_loop, as well as broadcasting an
// mdns query to discover Fuchsia devices every interval.
async fn query_recv_loop(
    sock: Arc<UdpSocket>,
    addr: SocketAddr,
    mdns_protocol: Weak<MdnsProtocol>,
    interval: Duration,
    tasks: Weak<Mutex<HashMap<IpAddr, Task<()>>>>,
    checker: Weak<impl MdnsEnabledChecker>,
    mdns_port: u16,
) {
    let mut recv = recv_loop(sock.clone(), mdns_protocol, checker).boxed().fuse();
    let mut query = query_loop(sock.clone(), interval, mdns_port).boxed().fuse();

    log::debug!("mdns: started query socket {}", addr);

    futures::select!(
        _ = recv => {
            log::trace!("query_recv_loop finished recv for addr: {}", addr);
        },
        _ = query => {
            log::trace!("query_recv_loop finished query loop for addr: {}", addr);
        },
    );

    drop(recv);
    drop(query);

    if let Some(tasks) = tasks.upgrade() {
        let mut guard = tasks.lock().await;
        if let Some(a) = guard.remove(&addr.ip()) {
            drop(a)
        }
    }
    log::debug!("mdns: shut down query socket {}", addr);
}

// Exclude any mdns packets received where the source address of the packet does not appear in any
// of the answers in the advert/response, as this likely means the target was NAT'd in some way, and
// the return path is likely not viable. In particular this filters out multicast that QEMU SLIRP
// has invalidly pumped onto the network that would cause us to attempt to connect to the host
// machine as if it was a Fuchsia target.
fn contains_source_address<B: zerocopy::SplitByteSlice + Copy>(
    addr: &SocketAddr,
    msg: &dns::Message<B>,
) -> bool {
    for answer in msg.answers.iter().chain(msg.additional.iter()) {
        if answer.rtype != dns::Type::A && answer.rtype != dns::Type::Aaaa {
            continue;
        }

        if answer.rdata.ip_addr() == Some(addr.ip()) {
            return true;
        }
    }
    // This message was a warning. Changed to debug as this is internal logic.
    // This is also expected as part of this function.
    // There is nothing actionable for the user.
    // This warning is most obvious when launching an emulator in user mode (which is the default)
    // and causes a lot of noise in the logs.
    log::debug!(
        "Dubious mdns from: {:?} does not contain an answer that includes the source address, therefore it is ignored.",
        addr
    );
    false
}

fn contains_txt_response<B: zerocopy::SplitByteSlice + Copy>(m: &dns::Message<B>) -> bool {
    m.answers.iter().any(|a| a.rtype == dns::Type::Txt)
}
fn is_fuchsia_response<B: zerocopy::SplitByteSlice + Copy>(m: &dns::Message<B>) -> bool {
    m.answers.iter().any(|a| a.domain == "_fuchsia._udp.local")
}

fn is_fastboot_response<B: zerocopy::SplitByteSlice + Copy>(
    m: &dns::Message<B>,
) -> Option<ffx::FastbootInterface> {
    if m.answers.is_empty() {
        None
    } else if m.answers.iter().any(|a| a.domain == "_fastboot._udp.local") {
        Some(ffx::FastbootInterface::Udp)
    } else if m.answers.iter().any(|a| a.domain == "_fastboot._tcp.local") {
        Some(ffx::FastbootInterface::Tcp)
    } else {
        None
    }
}

fn make_listen_socket(
    listen_addr: SocketAddr,
) -> std::result::Result<UdpSocket, MdnsDiscoveryError> {
    let socket: std::net::UdpSocket = match listen_addr {
        SocketAddr::V4(_) => {
            let socket = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            )
            .map_err(MdnsDiscoveryError::Io)?;
            socket.set_multicast_loop_v4(false).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket.set_reuse_address(true).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket.set_reuse_port(true).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket
                .bind(
                    &SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listen_addr.port()).into(),
                )
                .map_err(|e| {
                    MdnsDiscoveryError::BindSocket(
                        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listen_addr.port()),
                        e,
                    )
                })?;
            socket
                .join_multicast_v4(&MDNS_MCAST_V4, &Ipv4Addr::UNSPECIFIED)
                .map_err(MdnsDiscoveryError::JoinMulticast)?;
            socket
        }
        SocketAddr::V6(_) => {
            let socket = socket2::Socket::new(
                socket2::Domain::IPV6,
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            )
            .map_err(MdnsDiscoveryError::Io)?;
            socket.set_only_v6(true).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket.set_multicast_loop_v6(false).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket.set_reuse_address(true).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket.set_reuse_port(true).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket
                .bind(
                    &SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), listen_addr.port()).into(),
                )
                .map_err(|e| {
                    MdnsDiscoveryError::BindSocket(
                        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), listen_addr.port()),
                        e,
                    )
                })?;
            // For some reason this often fails to bind on Mac, so avoid it and
            // use the interface binding loop to get multicast group joining to
            // work.
            #[cfg(not(target_os = "macos"))]
            socket
                .join_multicast_v6(&MDNS_MCAST_V6, 0)
                .map_err(MdnsDiscoveryError::JoinMulticast)?;
            socket
        }
    }
    .into();
    socket.set_nonblocking(true).map_err(MdnsDiscoveryError::Io)?;
    Ok(UdpSocket::from_std(socket).map_err(MdnsDiscoveryError::Io)?)
}

fn make_sender_socket(
    interface_id: u32,
    addr: SocketAddr,
    ttl: u32,
) -> std::result::Result<UdpSocket, MdnsDiscoveryError> {
    let socket: std::net::UdpSocket = match addr {
        SocketAddr::V4(ref saddr) => {
            let socket = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            )
            .map_err(MdnsDiscoveryError::Io)?;
            socket.set_ttl_v4(ttl).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket.set_multicast_if_v4(saddr.ip()).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket.set_multicast_ttl_v4(ttl).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket.bind(&addr.into()).map_err(|e| MdnsDiscoveryError::BindSocket(addr, e))?;
            socket
        }
        SocketAddr::V6(ref _saddr) => {
            let socket = socket2::Socket::new(
                socket2::Domain::IPV6,
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            )
            .map_err(MdnsDiscoveryError::Io)?;
            socket.set_only_v6(true).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket
                .set_multicast_if_v6(interface_id)
                .map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket.set_unicast_hops_v6(ttl).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket.set_multicast_hops_v6(ttl).map_err(MdnsDiscoveryError::SetSocketOption)?;
            socket.bind(&addr.into()).map_err(|e| MdnsDiscoveryError::BindSocket(addr, e))?;
            socket
        }
    }
    .into();
    socket.set_nonblocking(true).map_err(MdnsDiscoveryError::Io)?;
    Ok(UdpSocket::from_std(socket).map_err(MdnsDiscoveryError::Io)?)
}

#[cfg(test)]
mod tests {

    use super::*;
    use ::mdns::protocol::{
        Class, DomainBuilder, EmbeddedPacketBuilder, Message, MessageBuilder, RecordBuilder,
    };
    use fidl_fuchsia_developer_ffx::TargetIpPort;
    use fidl_fuchsia_net::IpAddress::Ipv4;
    use packet::{InnerPacketBuilder, NoOpSerializationContext, ParseBuffer, Serializer};
    use std::io::Write;

    static_assertions::assert_impl_all!(MdnsProtocol: Send, Sync);

    #[test]
    fn test_make_target() {
        let nodename = DomainBuilder::from_str("foo._fuchsia._udp.local").unwrap();
        let record =
            RecordBuilder::new(nodename, dns::Type::A, Class::Any, true, 4500, &[8, 8, 8, 8]);
        let mut message = MessageBuilder::new(0, true);
        message.add_additional(record);
        let mut msg_bytes = message
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap_or_else(|_| panic!("failed to serialize"));
        let parsed = msg_bytes.parse::<Message<_>>().expect("failed to parse");
        let addr: SocketAddr = (Ipv4Addr::new(192, 168, 1, 1), 12).into();
        let (t, ttl) = make_target(addr, parsed).unwrap();
        assert_eq!(ttl, 4500);
        let addrs = t.addresses.as_ref().unwrap();
        assert_eq!(addrs.len(), 2);
        assert!(addrs.contains(&ffx::TargetAddrInfo::Ip(ffx::TargetIp {
            ip: IpAddress::Ipv4(Ipv4Address { addr: [192, 168, 1, 1] }),
            scope_id: 0
        })));
        assert!(addrs.contains(&ffx::TargetAddrInfo::Ip(ffx::TargetIp {
            ip: IpAddress::Ipv4(Ipv4Address { addr: [8, 8, 8, 8] }),
            scope_id: 0
        })));
        assert_eq!(t.nodename.unwrap(), "foo._fuchsia._udp");
    }

    #[test]
    fn test_make_target_with_serial() -> anyhow::Result<()> {
        let nodename = DomainBuilder::from_str("foo._fuchsia._udp.local").unwrap();
        let mut txt_data: Vec<u8> = vec![];
        let txt_strings = ["foo=bar", "serial=1234990"];
        for d in txt_strings {
            txt_data.write_all(&[d.len() as u8])?;
            txt_data.write_all(d.as_bytes())?;
        }
        let record = RecordBuilder::new(
            nodename.clone(),
            dns::Type::A,
            Class::Any,
            true,
            4500,
            &[8, 8, 8, 8],
        );
        let text_record =
            RecordBuilder::new(nodename, dns::Type::Txt, Class::Any, true, 4500, &txt_data);
        let mut message = MessageBuilder::new(0, true);
        message.add_additional(record);
        message.add_additional(text_record);
        let mut msg_bytes = message
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap_or_else(|_| panic!("failed to serialize"));
        let parsed = msg_bytes.parse::<Message<_>>().expect("failed to parse");
        let addr: SocketAddr = (Ipv4Addr::new(192, 168, 1, 1), 12).into();
        let (t, ttl) = make_target(addr, parsed).unwrap();
        assert_eq!(ttl, 4500);
        let addrs = t.addresses.as_ref().unwrap();
        assert!(addrs.contains(&ffx::TargetAddrInfo::Ip(ffx::TargetIp {
            ip: IpAddress::Ipv4(Ipv4Address { addr: [192, 168, 1, 1] }),
            scope_id: 0
        })));
        assert!(addrs.contains(&ffx::TargetAddrInfo::Ip(ffx::TargetIp {
            ip: IpAddress::Ipv4(Ipv4Address { addr: [8, 8, 8, 8] }),
            scope_id: 0
        })));
        assert_eq!(t.nodename.unwrap(), "foo._fuchsia._udp");
        assert_eq!(t.serial_number.unwrap(), "1234990");
        Ok(())
    }

    #[test]
    fn test_make_target_from_txt() -> anyhow::Result<()> {
        let nodename = DomainBuilder::from_str("foo._fuchsia._udp.local").unwrap();
        let mut emu_data: Vec<u8> = vec![];
        let emu_strings = ["host:123.11.22.33", "ssh:54321", "debug:1111"];
        for d in emu_strings {
            emu_data.write_all(&[d.len() as u8])?;
            emu_data.write_all(d.as_bytes())?;
        }
        let record = RecordBuilder::new(
            nodename.clone(),
            dns::Type::A,
            Class::Any,
            true,
            4500,
            &[8, 8, 8, 8],
        );
        let text_record =
            RecordBuilder::new(nodename, dns::Type::Txt, Class::Any, true, 4500, &emu_data);
        let mut message = MessageBuilder::new(0, true);
        message.add_additional(record);
        message.add_additional(text_record);
        let mut msg_bytes = message
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap_or_else(|_| panic!("failed to serialize"));
        let parsed = msg_bytes.parse::<Message<_>>().expect("failed to parse");
        let addr: SocketAddr = (Ipv4Addr::new(192, 168, 1, 1), 12).into();
        let (t, ttl) = make_target(addr, parsed).unwrap();
        assert_eq!(ttl, 4500);
        let addrs = t.addresses.as_ref().unwrap();
        assert!(addrs.contains(&ffx::TargetAddrInfo::Ip(ffx::TargetIp {
            ip: IpAddress::Ipv4(Ipv4Address { addr: [192, 168, 1, 1] }),
            scope_id: 0
        })));
        assert!(addrs.contains(&ffx::TargetAddrInfo::IpPort(ffx::TargetIpPort {
            ip: IpAddress::Ipv4(Ipv4Address { addr: [123, 11, 22, 33] }),
            scope_id: 0,
            port: 54321
        })));
        assert!(addrs.contains(&ffx::TargetAddrInfo::Ip(ffx::TargetIp {
            ip: IpAddress::Ipv4(Ipv4Address { addr: [8, 8, 8, 8] }),
            scope_id: 0
        })));
        assert_eq!(
            t.ssh_address,
            Some(ffx::TargetIpAddrInfo::IpPort(TargetIpPort {
                ip: Ipv4(Ipv4Address { addr: [123, 11, 22, 33] }),
                scope_id: 0,
                port: 54321
            }))
        );
        assert_eq!(t.nodename.unwrap(), "foo._fuchsia._udp");
        Ok(())
    }

    #[test]
    fn test_make_target_link_local_no_scope() {
        let nodename = DomainBuilder::from_str("foo._fuchsia._udp.local").unwrap();
        let record = RecordBuilder::new(
            nodename,
            dns::Type::Aaaa,
            Class::Any,
            true,
            4500,
            &[0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], // fe80::1
        );
        let mut message = MessageBuilder::new(0, true);
        message.add_additional(record);
        let mut msg_bytes = message
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap_or_else(|_| panic!("failed to serialize"));
        let parsed = msg_bytes.parse::<Message<_>>().expect("failed to parse");
        let addr: SocketAddr = (Ipv4Addr::new(192, 168, 1, 1), 12).into();
        let (t, _ttl) = make_target(addr, parsed).unwrap();
        let addrs = t.addresses.as_ref().unwrap();

        // Should only contain the source address (IPv4), not the link-local IPv6 address.
        assert_eq!(addrs.len(), 1);
        assert!(addrs.contains(&ffx::TargetAddrInfo::Ip(ffx::TargetIp {
            ip: IpAddress::Ipv4(Ipv4Address { addr: [192, 168, 1, 1] }),
            scope_id: 0
        })));
    }

    #[test]
    fn test_make_target_link_local_with_scope() {
        let nodename = DomainBuilder::from_str("foo._fuchsia._udp.local").unwrap();
        let record = RecordBuilder::new(
            nodename,
            dns::Type::Aaaa,
            Class::Any,
            true,
            4500,
            &[0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2], // fe80::2
        );
        let mut message = MessageBuilder::new(0, true);
        message.add_additional(record);
        let mut msg_bytes = message
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap_or_else(|_| panic!("failed to serialize"));
        let parsed = msg_bytes.parse::<Message<_>>().expect("failed to parse");

        let src_ip = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1); // fe80::1
        let addr = SocketAddr::V6(std::net::SocketAddrV6::new(src_ip, 12, 0, 3)); // scope_id = 3

        let (t, _ttl) = make_target(addr, parsed).unwrap();
        let addrs = t.addresses.as_ref().unwrap();

        assert_eq!(addrs.len(), 2);
        assert!(addrs.contains(&ffx::TargetAddrInfo::Ip(ffx::TargetIp {
            ip: IpAddress::Ipv6(Ipv6Address { addr: src_ip.octets() }),
            scope_id: 3
        })));
        assert!(addrs.contains(&ffx::TargetAddrInfo::Ip(ffx::TargetIp {
            ip: IpAddress::Ipv6(Ipv6Address {
                addr: [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]
            }),
            scope_id: 3
        })));
    }

    #[test]
    fn test_make_target_no_valid_record() {
        let nodename = DomainBuilder::from_str("foo._fuchsia._udp.local").unwrap();
        let record = RecordBuilder::new(
            nodename,
            dns::Type::Ptr,
            Class::Any,
            true,
            4500,
            &[0x03, b'f', b'o', b'o', 0],
        );
        let mut message = MessageBuilder::new(0, true);
        message.add_additional(record);
        let mut msg_bytes = message
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap_or_else(|_| panic!("failed to serialize"));
        let parsed = msg_bytes.parse::<Message<_>>().expect("failed to parse");
        let addr: SocketAddr = (MDNS_MCAST_V4, 12).into();
        assert!(make_target(addr, parsed).is_none());
    }

    /// Create an mdns advertisement packet as network bytes
    fn create_mdns_advert(nodename: &str, address: IpAddr) -> Vec<u8> {
        let domain = DomainBuilder::from_str(&format!("{}._fuchsia._udp.local", nodename)).unwrap();
        let rdata = DomainBuilder::from_str("_fuchsia._udp.local").unwrap().bytes();
        let record = RecordBuilder::new(domain, dns::Type::Ptr, Class::Any, true, 1, &rdata);
        let mut message = MessageBuilder::new(0, true);
        message.add_additional(record);

        let domain = DomainBuilder::from_str(&format!("{}.local", nodename)).unwrap();
        let rdata = match &address {
            IpAddr::V4(addr) => Vec::from(addr.octets()),
            IpAddr::V6(addr) => Vec::from(addr.octets()),
        };
        let record = RecordBuilder::new(
            domain,
            match address {
                IpAddr::V4(_) => dns::Type::A,
                IpAddr::V6(_) => dns::Type::Aaaa,
            },
            Class::Any,
            true,
            1,
            &rdata,
        );
        message.add_additional(record);

        message
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap_or_else(|_| panic!("failed to serialize"))
            .unwrap_b()
            .as_ref()
            .to_vec()
    }

    #[test]
    fn test_contains_source_address() {
        assert!(contains_source_address(
            &"127.0.0.1:0".parse().unwrap(),
            &create_mdns_advert("fuchsia-foo-1234", IpAddr::from([127, 0, 0, 1]))
                .as_slice()
                .parse::<Message<_>>()
                .unwrap(),
        ));

        assert!(!contains_source_address(
            &"127.0.0.1:0".parse().unwrap(),
            &create_mdns_advert("fuchsia-foo-1234", IpAddr::from([127, 0, 0, 2]))
                .as_slice()
                .parse::<Message<_>>()
                .unwrap(),
        ));
    }

    fn build_fastboot_mdns_response(ty: &'static str) -> Vec<u8> {
        let domain = dns::DomainBuilder::from_str(&format!("_fastboot.{ty}.local")).unwrap();
        let fastboot_record = dns::RecordBuilder::new(
            domain,
            dns::Type::Ptr,
            dns::Class::Any,
            true,
            4500,
            // Must contain length (3 chars) and a null terminator. Usually this is an IP address,
            // but we don't care.
            &[0x03, 'f' as u8, 'o' as u8, 'o' as u8, 0],
        );
        let nonsense_record = dns::RecordBuilder::new(
            dns::DomainBuilder::from_str("foo._fuchsia.tcp.local").unwrap(),
            dns::Type::Ptr,
            dns::Class::Any,
            true,
            4500,
            &[0x03, 'f' as u8, 'o' as u8, 'o' as u8, 0],
        );
        let mut message = MessageBuilder::new(0, true);
        message.add_answer(nonsense_record);
        message.add_answer(fastboot_record);
        message
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap_or_else(|_| panic!("couldn't serialize mdns message"))
            .unwrap_b()
            .as_ref()
            .to_vec()
    }

    #[test]
    fn test_is_fastboot_response() {
        assert_eq!(
            is_fastboot_response(
                &build_fastboot_mdns_response("_tcp").as_slice().parse::<Message<_>>().unwrap(),
            ),
            Some(ffx::FastbootInterface::Tcp)
        );
        assert_eq!(
            is_fastboot_response(
                &build_fastboot_mdns_response("_udp").as_slice().parse::<Message<_>>().unwrap(),
            ),
            Some(ffx::FastbootInterface::Udp)
        );
        assert_eq!(
            is_fastboot_response(
                &build_fastboot_mdns_response("ffx").as_slice().parse::<Message<_>>().unwrap(),
            ),
            None
        );
        let message = MessageBuilder::new(0, true);
        let mut bytes = message
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap_or_else(|_| panic!("couldn't serialize mdns message"));
        let message =
            bytes.parse::<dns::Message<_>>().unwrap_or_else(|_| panic!("couldn't parse mdns"));
        assert_eq!(is_fastboot_response(&message), None);
    }
}
