// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42066994): Remove.
#![allow(unused_variables, unused_imports, dead_code)]

use crate::guest_config;
use anyhow::{Error, anyhow};
use fidl::endpoints::{Proxy, ServerEnd, create_proxy};
use fidl_fuchsia_hardware_network;
use fidl_fuchsia_net::MacAddress;
use fidl_fuchsia_net_interfaces as ninterfaces;
use fidl_fuchsia_virtualization::{
    GuestConfig, GuestDescriptor, GuestError, GuestInfo, GuestLifecycleMarker, GuestLifecycleProxy,
    GuestManagerConnectResponder, GuestManagerError, GuestManagerForceShutdownResponder,
    GuestManagerGetInfoResponder, GuestManagerLaunchResponder, GuestManagerRequest,
    GuestManagerRequestStream, GuestMarker, GuestStatus, NetSpec,
};
use fuchsia_async as fasync;
use fuchsia_component::client::{connect_channel_to_protocol, connect_to_protocol};
use futures::stream::{FuturesUnordered, SelectAll, try_unfold};
use futures::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt, future, select_biased};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::{fmt, fs};

// This is a locally administered MAC address (first byte 0x02) mixed with the
// Google Organizationally Unique Identifier (00:1a:11). The host gets ff:ff:ff
// and the guest gets 00:00:00 for the last three octets.
const DEFAULT_GUEST_MAC_ADDRESS: MacAddress =
    MacAddress { octets: [0x02u8, 0x1a, 0x11, 0x00, 0x01, 0x00] };

fn get_default_guest_memory() -> u64 {
    // There are no assumptions made by this unsafe block; it is only unsafe due to FFI.
    let host_memory = unsafe { zx::sys::zx_system_get_physmem() };
    let max_reserved_host_memory = 3 * (1u64 << 30); // 3 GiB.

    // Reserve half the host memory up to 3 GiB, and allow the rest to be used by the guest.
    host_memory - std::cmp::min(host_memory / 2, max_reserved_host_memory)
}

fn get_default_num_cpus() -> u8 {
    // There are no assumptions made by this unsafe block; it is only unsafe due to FFI.
    let num_system_cpus: u32 = unsafe { zx::sys::zx_system_get_num_cpus() };
    std::cmp::min(num_system_cpus, u8::MAX.into())
        .try_into()
        .expect("this value is known to be no larger than u8::MAX")
}

macro_rules! send_checked {
    ($responder:ident $(, $res:expr)?) => {
        if let Err(err) = $responder.send($($res)?) {
            log::warn!(err:%; "Could not send reply")
        };
    }
}

#[derive(Debug, PartialEq)]
enum GuestNetworkState {
    // There are at least enough virtual device interfaces to match the guest configuration, and
    // if there is a bridged configuration, there's at least one bridged interface. This doesn't
    // guarantee working networking, but means that the system state is likely correct.
    Ok = 0,

    // This guest wasn't started with a network device, so no networking is expected.
    NoNetworkDevice = 1,

    // Failed to query network interfaces. Check component routing if this is unexpected.
    FailedToQuery = 2,

    // Host doesn't have a WLAN or ethernet interface, so there's probably no guest networking.
    NoHostNetworking = 3,

    // There's at least one missing virtual interface that was expected to be present. Check
    // virtio-net device logs for a failure.
    MissingVirtualInterfaces = 4,

    // An interface is bridged, there's an ethernet interface to bridge against, but the
    // bridge hasn't been created yet. This might be a transient issue while the bridge is created.
    NoBridgeCreated = 5,

    // An interface is bridged, and there's no ethernet to bridge against, but the host is
    // connected to WLAN. This is a common user misconfiguration.
    AttemptedToBridgeWithWlan = 6,
}

impl fmt::Display for GuestNetworkState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                GuestNetworkState::Ok =>
                    "Guest network likely configured correctly; \
                     check host connectivity if suspected network failure",
                GuestNetworkState::NoNetworkDevice =>
                    "Guest not configured with a network device; \
                     check guest configuration if networking is required",
                GuestNetworkState::FailedToQuery => "Failed to query guest network status",
                GuestNetworkState::NoHostNetworking =>
                    "Host has no network interfaces; guest networking will not work",
                GuestNetworkState::MissingVirtualInterfaces =>
                    "Fewer than expected virtual interfaces; guest failed network device startup",
                GuestNetworkState::NoBridgeCreated =>
                    "No bridge between guest and host network interaces; \
                     this may be transient so retrying is recommended",
                GuestNetworkState::AttemptedToBridgeWithWlan =>
                    "Cannot create bridged guest network when host is using WiFi; \
                     disconnect WiFi and connect via ethernet",
            }
        )
    }
}

#[derive(Default)]
struct HostNetworkState {
    has_bridge: bool,
    has_ethernet: bool,
    has_wlan: bool,
    num_virtual: u32,
}

pub struct GuestManager {
    // Path under which guest files are served.
    config_dir: PathBuf,

    // Path to the default guest config.
    config_path: String,

    // The current state of the VMM managed by this guest manager.
    status: GuestStatus,

    // Cached error reported by the VMM upon stopping, if not stopped due to a clean shutdown.
    last_error: Option<GuestError>,

    // Snapshot of some of the configuration settings used to start this guest. This is
    // informational only, and sent in response to a get_info call.
    guest_descriptor: GuestDescriptor,

    // Start and stop time for a guest, used to calculate the guest's uptime.
    start_time: zx::MonotonicInstant,
    stop_time: zx::MonotonicInstant,
}

impl GuestManager {
    pub fn new_with_defaults() -> Self {
        GuestManager::new(PathBuf::from("/guest_pkg/"), "data/guest.cfg".to_string())
    }

    fn new(config_dir: PathBuf, config_path: String) -> Self {
        GuestManager {
            config_dir,
            config_path,
            status: GuestStatus::NotStarted,
            last_error: None,
            guest_descriptor: GuestDescriptor::default(),
            start_time: zx::MonotonicInstant::INFINITE_PAST,
            stop_time: zx::MonotonicInstant::INFINITE_PAST,
        }
    }

    pub async fn run<St: Stream<Item = GuestManagerRequestStream> + Unpin>(
        &mut self,
        mut lifecycle: GuestLifecycleProxy,
        request_streams: St,
    ) -> Result<(), Error> {
        let mut on_closed = Box::pin(lifecycle.on_closed().fuse());

        let mut request_streams = request_streams.fuse();
        let mut connections = SelectAll::new();

        let mut run_futures: FuturesUnordered<future::LocalBoxFuture<'_, Result<(), GuestError>>> =
            FuturesUnordered::new();
        run_futures.push(Box::pin(future::pending::<Result<(), GuestError>>()));
        let mut pending_shutdowns = Vec::<GuestManagerForceShutdownResponder>::new();

        loop {
            select_biased! {
                result = on_closed => {
                    result.map_err(|err| anyhow!(
                        "failed to wait on guest lifecycle proxy closed: {}", err))?;
                    log::error!("VMM component has unexpectedly stopped");
                    self.status = GuestStatus::VmmUnexpectedTermination;

                    // The VMM component has terminated, create a new one by opening a new
                    // lifecycle channel.
                    drop(on_closed);
                    lifecycle = connect_to_protocol::<GuestLifecycleMarker>()?;
                    on_closed = Box::pin(lifecycle.on_closed().fuse());

                    // Any pending run future is now invalid.
                    run_futures.clear();
                    run_futures.push(Box::pin(future::pending::<Result<(), GuestError>>()));

                    // Respond to any pending shutdowns
                    pending_shutdowns.drain(..).for_each(|responder| {
                        send_checked!(responder);
                    });
                }
                run_result = run_futures.next() => {
                    let run_result = run_result.expect("Should never resolve to Poll::Ready(None)");
                    self.handle_guest_stopped(run_result);

                    pending_shutdowns.drain(..).for_each(|responder| {
                        send_checked!(responder);
                    });
                }
                stream = request_streams.next() => {
                    connections.push(stream.ok_or_else(|| anyhow!(
                        "unexpected end of stream of guest manager request streams"))?);
                }
                request = connections.next() => {
                    let request = match request {
                        None => {
                            // Clean end of this connection stream.
                            continue;
                        }
                        Some(result) => match result {
                            Ok(request) => request,
                            Err(err) => {
                                log::error!(err:%; "Connection stream ended with reason");
                                continue;
                            }
                        }
                    };

                    match request {
                        GuestManagerRequest::Launch { guest_config, controller, responder } => {
                            if self.is_guest_started() {
                                send_checked!(responder, Err(GuestManagerError::AlreadyRunning));
                                continue;
                            }

                            // Merge guest config.
                            let config = self.get_merged_config(guest_config);
                            if let Err(err) = config {
                                log::error!(err:%; "Could not create guest config");
                                send_checked!(responder, Err(GuestManagerError::BadConfig));
                                continue;
                            }
                            let config = config.unwrap();
                            self.handle_guest_started(&config);

                            // Create VMM.
                            let create =
                                GuestManager::send_create_request(lifecycle.clone(), config).await
                                .and_then(|_| {
                                    GuestManager::connect(&lifecycle, controller)
                                });

                            if let Err(err) = create {
                                log::error!(err:?; "Could not create VMM");
                                send_checked!(responder, Err(GuestManagerError::StartFailure));
                                self.handle_guest_stopped(create);
                                continue;
                            }

                            self.status = GuestStatus::Running;
                            send_checked!(responder, Ok(()));
                            // Run returns when the guest has stopped. Push this long running
                            // async call into a FuturesUnordered to be polled by the select.
                            assert!(run_futures.len() == 1);
                            run_futures.push(
                                Box::pin(GuestManager::send_run_request(lifecycle.clone())));
                        }
                        GuestManagerRequest::ForceShutdown { responder } => {
                            if !self.is_guest_started() {
                                // Respond immediately if guest isn't started.
                                send_checked!(responder);
                                continue;
                            }

                            self.status = GuestStatus::Stopping;
                            if let Err(err) = lifecycle.stop().await {
                                log::error!(err:%; "failed to send Stop FIDL call")
                            }
                            // Respond to this request when the shutdown completes.
                            pending_shutdowns.push(responder);

                        }
                        GuestManagerRequest::Connect { controller, responder } => {
                            if !self.is_guest_started() {
                                send_checked!(responder, Err(GuestManagerError::NotRunning));
                            } else {
                                send_checked!(responder, GuestManager::connect(&lifecycle, controller)
                                          .map_err(|_| GuestManagerError::NotRunning ));
                            }
                        }
                        GuestManagerRequest::GetInfo { responder } => {
                            let network_state = GuestManager::host_network_state().await
                                .map(|host_state| self.guest_network_state(host_state))
                                .unwrap_or_else(|err| {
                                    log::warn!(err:%; "Unable to query host network interface.");
                                    GuestNetworkState::FailedToQuery
                                });


                            send_checked!(responder, &self.guest_info(GuestManager::check_for_problems(network_state)));
                        }
                    }
                }
            }
        }
    }

    fn handle_guest_stopped(&mut self, reason: Result<(), GuestError>) {
        self.status = GuestStatus::Stopped;
        self.stop_time = fasync::MonotonicInstant::now().into();
        if let Err(e) = reason {
            self.last_error = Some(e);
        }
    }

    fn handle_guest_started(&mut self, config: &GuestConfig) {
        self.status = GuestStatus::Starting;
        self.start_time = fasync::MonotonicInstant::now().into();
        self.guest_descriptor = GuestManager::snapshot_config(config);
        self.last_error = None;
    }

    fn get_merged_config(&self, user_config: GuestConfig) -> Result<GuestConfig, Error> {
        let mut merged = guest_config::merge_configs(self.get_default_guest_config()?, user_config);

        // Set config defaults for mem, cpus, and net device.
        merged.guest_memory.get_or_insert_with(get_default_guest_memory);
        merged.cpus.get_or_insert_with(get_default_num_cpus);
        if merged.default_net.unwrap_or(false) {
            merged
                .net_devices
                .get_or_insert(Vec::new())
                .push(NetSpec { mac_address: DEFAULT_GUEST_MAC_ADDRESS, enable_bridge: true });
        }

        // Merge command-line additions into the main command-line field.
        merged.cmdline =
            merged.cmdline.into_iter().chain(merged.cmdline_add.into_iter().flatten()).reduce(
                |mut acc, a| {
                    acc.push(' ');
                    acc.push_str(&a);
                    acc
                },
            );
        merged.cmdline_add = None;

        // Initial vsock listeners must be bound to unique ports.
        if merged.vsock_listeners.is_some() {
            let listeners = merged.vsock_listeners.as_ref().unwrap();
            let ports: HashSet<_> = listeners.iter().map(|l| l.port).collect();
            if ports.len() != listeners.len() {
                return Err(anyhow!("Vsock listeners not bound to unique ports"));
            }
        }

        Ok(merged)
    }

    // Build a GuestInfo from the current configuration and the given list of problems.
    pub fn guest_info(&self, detected_problems: Vec<String>) -> GuestInfo {
        let mut info = GuestInfo {
            guest_status: Some(self.status),
            detected_problems: if detected_problems.len() > 0 {
                Some(detected_problems)
            } else {
                None
            },
            ..Default::default()
        };

        match self.status {
            _ if self.is_guest_started() => {
                info.uptime = Some(
                    (fasync::MonotonicInstant::now().into_zx() - self.start_time).into_nanos(),
                );
                info.guest_descriptor = Some(self.guest_descriptor.clone());
            }
            GuestStatus::Stopped => {
                info.uptime = Some((self.stop_time - self.start_time).into_nanos());
                info.stop_error = self.last_error;
            }
            _ => (),
        };
        info
    }

    async fn host_network_state() -> Result<HostNetworkState, Error> {
        // Connect to interface watcher.
        let state_proxy = connect_to_protocol::<ninterfaces::StateMarker>()?;
        let (watcher_proxy, watcher_server) = create_proxy::<ninterfaces::WatcherMarker>();
        state_proxy.get_watcher(&ninterfaces::WatcherOptions::default(), watcher_server)?;

        // Collect interface state events until Idle is received, indicating the end of current
        // events at the time of the query.
        let stream = try_unfold(watcher_proxy, |watcher| async move {
            match watcher.watch().await {
                Err(e) => Err(e),
                Ok(ninterfaces::Event::Idle(_)) => Ok(None),
                Ok(event) => Ok(Some((event, watcher))),
            }
        });

        stream
            .try_filter_map(|event| async move {
                // Only consider existing properties at the time of this query.
                if let ninterfaces::Event::Existing(properties) = event {
                    // Only consider active, non-loopback devices
                    if !properties.online.unwrap_or(false) {
                        return Ok(None);
                    }
                    match properties.port_class {
                        None => Ok(None),
                        Some(ninterfaces::PortClass::Loopback(_)) => Ok(None),
                        Some(ninterfaces::PortClass::Device(net_device)) => Ok(Some(net_device)),
                        Some(_) => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            })
            .try_fold(HostNetworkState::default(), |mut host_state, net_device| async move {
                match net_device {
                    fidl_fuchsia_hardware_network::PortClass::Virtual => {
                        host_state.num_virtual += 1
                    }
                    fidl_fuchsia_hardware_network::PortClass::Ethernet => {
                        host_state.has_ethernet = true
                    }
                    fidl_fuchsia_hardware_network::PortClass::WlanClient => {
                        host_state.has_wlan = true
                    }
                    fidl_fuchsia_hardware_network::PortClass::Bridge => {
                        host_state.has_bridge = true
                    }
                    _ => (),
                };
                Ok(host_state)
            })
            .err_into::<anyhow::Error>()
            .await
    }

    // Check the guest network settings against the state of the host network interfaces to
    // generate a guest network state.
    fn guest_network_state(&self, host: HostNetworkState) -> GuestNetworkState {
        let expected_bridge =
            self.guest_descriptor.networks.iter().flatten().any(|net_spec| net_spec.enable_bridge);
        let guest_num_networks =
            self.guest_descriptor.networks.as_ref().map(Vec::len).unwrap_or(0) as u32;

        if guest_num_networks == 0 {
            return GuestNetworkState::NoNetworkDevice;
        }
        if !host.has_ethernet && !host.has_wlan {
            return GuestNetworkState::NoHostNetworking;
        }
        if host.num_virtual < guest_num_networks {
            return GuestNetworkState::MissingVirtualInterfaces;
        }
        if expected_bridge && !host.has_bridge {
            if !host.has_ethernet {
                return GuestNetworkState::AttemptedToBridgeWithWlan;
            }
            return GuestNetworkState::NoBridgeCreated;
        }
        GuestNetworkState::Ok
    }

    fn check_for_problems(network_state: GuestNetworkState) -> Vec<String> {
        // TODO: Check if host is experiencing memory pressure.
        let mut problems = Vec::<String>::new();
        if matches!(
            network_state,
            GuestNetworkState::FailedToQuery
                | GuestNetworkState::NoHostNetworking
                | GuestNetworkState::MissingVirtualInterfaces
                | GuestNetworkState::NoBridgeCreated
                | GuestNetworkState::AttemptedToBridgeWithWlan
        ) {
            problems.push(network_state.to_string());
        }
        problems
    }

    fn get_default_guest_config(&self) -> Result<GuestConfig, Error> {
        let config_path = self.config_dir.join(&self.config_path);
        let config_path =
            config_path.to_str().ok_or_else(|| anyhow!("file path is not a valid UTF-8 string"))?;
        guest_config::parse_config(&fs::read_to_string(config_path)?, &self.config_dir)
    }

    fn snapshot_config(config: &GuestConfig) -> GuestDescriptor {
        GuestDescriptor {
            num_cpus: config.cpus,
            guest_memory: config.guest_memory,
            balloon: config.virtio_balloon,
            console: config.virtio_console,
            gpu: config.virtio_gpu,
            rng: config.virtio_rng,
            vsock: config.virtio_vsock,
            sound: config.virtio_sound,
            networks: config.net_devices.clone(),
            mem: config.virtio_mem,
            ..Default::default()
        }
    }

    fn is_guest_started(&self) -> bool {
        matches!(self.status, GuestStatus::Starting | GuestStatus::Running | GuestStatus::Stopping)
    }

    async fn send_create_request(
        lifecycle: GuestLifecycleProxy,
        config: GuestConfig,
    ) -> Result<(), GuestError> {
        let result = lifecycle.create(config).await;
        if let Err(err) = result {
            log::error!(err:%; "failed to send Create FIDL call");
            Err(GuestError::InternalError)
        } else {
            result.unwrap()
        }
    }

    async fn send_run_request(lifecycle: GuestLifecycleProxy) -> Result<(), GuestError> {
        let result = lifecycle.run().await;
        if let Err(err) = result {
            log::error!(err:%; "failed to send Run FIDL call");
            Err(GuestError::InternalError)
        } else {
            result.unwrap()
        }
    }

    fn connect(
        lifecycle: &GuestLifecycleProxy,
        controller: ServerEnd<GuestMarker>,
    ) -> Result<(), GuestError> {
        let result = lifecycle.bind(controller);
        if let Err(err) = result {
            log::error!(err:%; "failed to send Bind FIDL call");
            Err(GuestError::InternalError)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_utils::PollExt;
    use fidl::endpoints::{create_endpoints, create_proxy_and_stream};
    use fidl_fuchsia_virtualization::{
        GuestLifecycleRequest, GuestLifecycleRequestStream, GuestLifecycleRunResponder,
        GuestManagerMarker, GuestManagerProxy, GuestManagerRequestStream, HostVsockAcceptorMarker,
        Listener,
    };
    use fs::write;
    use fuchsia_fs::{OpenFlags, file};
    use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
    use futures::future::{self, Either, join};
    use futures::select;
    use std::cell::{Cell, Ref, RefCell};
    use std::io::Write;
    use tempfile::{NamedTempFile, TempPath};

    #[derive(PartialEq, Debug)]
    enum VmmState {
        NotCreated,
        Running,
        NotRunning,
    }
    struct MockVmm {
        state: VmmState,
        request_stream: GuestLifecycleRequestStream,
        // GuestManager expects to wait on this response whilst the VMM is running.
        running_vmm: Option<GuestLifecycleRunResponder>,
    }

    impl MockVmm {
        fn new(request_stream: GuestLifecycleRequestStream) -> MockVmm {
            MockVmm { state: VmmState::NotCreated, request_stream, running_vmm: None }
        }

        fn handle_request(&mut self, req: GuestLifecycleRequest) {
            match req {
                GuestLifecycleRequest::Create { guest_config, responder } => {
                    self.state = VmmState::NotRunning;
                    responder.send(Ok(())).expect("stream should not be closed");
                }
                GuestLifecycleRequest::Bind { guest, control_handle } => {
                    ();
                }
                GuestLifecycleRequest::Run { responder } => {
                    self.state = VmmState::Running;
                    self.running_vmm = Some(responder);
                }
                GuestLifecycleRequest::Stop { responder } => {
                    self.stop();
                    responder.send().expect("stream should not be closed");
                }
            }
        }

        // Simulate guest shutdown.
        fn stop(&mut self) {
            self.state = VmmState::NotRunning;
            self.running_vmm
                .take()
                .expect("VMM should be running")
                .send(Ok(()))
                .expect("stream should not be closed");
        }

        // Pull the next request off the queue to handle manually.
        async fn next(&mut self) -> GuestLifecycleRequest {
            self.request_stream.next().await.unwrap().unwrap()
        }

        // Successfully handle a single request.
        async fn run_once(&mut self) {
            let req = self.next().await;
            self.handle_request(req);
        }

        // Long running future successfully handling all VMM requests.
        async fn run(&mut self) {
            while let Some(req) = self.request_stream.next().await {
                self.handle_request(req.unwrap());
            }
        }

        // Successfully handle requests as part of a Launch request.
        async fn run_launch(&mut self) {
            // Successfully handle a single vmm request, asserting that the request type matches the
            // given pattern.
            macro_rules! run_expect {
                ($vmm:expr, $req_type:pat_param) => {
                    let req = $vmm.next().await;
                    assert!(matches!(req, $req_type));
                    $vmm.handle_request(req);
                };
            }

            run_expect!(self, GuestLifecycleRequest::Create { .. });
            run_expect!(self, GuestLifecycleRequest::Bind { .. });
            run_expect!(self, GuestLifecycleRequest::Run { .. });
        }
    }

    // A test fixture to manage the state for a running GuestManager.
    struct ManagerFixture {
        // manager and lifycycle_stream are behind RefCell as they create futures holding
        // mutable refs.
        manager: RefCell<GuestManager>,
        stream_tx: UnboundedSender<GuestManagerRequestStream>,
        state_rx: Cell<Option<UnboundedReceiver<GuestManagerRequestStream>>>,
        vmm: RefCell<MockVmm>,
        lifecycle_proxy: Cell<Option<GuestLifecycleProxy>>,
        config_path: Option<TempPath>,
    }

    impl ManagerFixture {
        fn new(config: &str) -> ManagerFixture {
            let mut tmpfile = NamedTempFile::new().expect("failed to create tempfile");
            write!(tmpfile, "{}", config).expect("failed to write to tempfile");

            let mut man = Self::new_with_config_file(tmpfile.path().to_str().unwrap().to_string());
            // Hold the TempPath to keep the file from being deleted.
            man.config_path = Some(tmpfile.into_temp_path());
            man
        }

        fn new_with_config_file(path: String) -> ManagerFixture {
            let (stream_tx, state_rx) = mpsc::unbounded::<GuestManagerRequestStream>();
            let (lifecycle_proxy, lifecycle_stream) =
                create_proxy_and_stream::<GuestLifecycleMarker>();

            ManagerFixture {
                manager: RefCell::new(GuestManager::new(PathBuf::from(""), path)),
                stream_tx,
                state_rx: Cell::new(Some(state_rx)),
                vmm: RefCell::new(MockVmm::new(lifecycle_stream)),
                lifecycle_proxy: Cell::new(Some(lifecycle_proxy)),
                config_path: None,
            }
        }

        // Create a new fixture with a minimal default config.
        fn new_with_defaults() -> ManagerFixture {
            Self::new(r#"{}"#)
        }

        async fn run(&self) -> Result<(), Error> {
            // run consumes lifecycle_proxy and state_rx, it will panic if called more than once.
            self.manager
                .borrow_mut()
                .run(
                    self.lifecycle_proxy.take().expect("manager already run"),
                    self.state_rx.take().expect("manager already run"),
                )
                .await
        }

        fn manager(&self) -> Ref<'_, GuestManager> {
            self.manager.borrow()
        }

        fn new_proxy(&self) -> GuestManagerProxy {
            let (manager_proxy, manager_server) = create_proxy_and_stream::<GuestManagerMarker>();
            self.stream_tx.unbounded_send(manager_server).expect("stream should not be closed");
            manager_proxy
        }

        async fn run_vmm(&self) -> () {
            self.vmm.borrow_mut().run().await;
        }
    }

    #[fuchsia::test]
    async fn config_applies_defaults() {
        let config = GuestConfig { default_net: Some(true), ..Default::default() };
        let manager = ManagerFixture::new_with_defaults();
        let merged = manager.manager().get_merged_config(config).unwrap();
        assert_eq!(merged.cpus, Some(get_default_num_cpus()));
        assert_eq!(merged.guest_memory, Some(get_default_guest_memory()));
        assert_eq!(merged.net_devices.as_ref().map(|nd| nd.len()), Some(1));
        assert_eq!(
            merged.net_devices.unwrap()[0],
            NetSpec { mac_address: DEFAULT_GUEST_MAC_ADDRESS, enable_bridge: true }
        );
    }

    #[fuchsia::test]
    async fn config_merges_cmdline() {
        let config = r#"{"cmdline": "firstarg"}"#;
        let userconfig = GuestConfig {
            cmdline_add: Some(Vec::from(["secondarg", "thirdarg"].map(String::from))),
            ..Default::default()
        };

        let manager = ManagerFixture::new(config);
        let merged = manager.manager().get_merged_config(userconfig).unwrap();
        assert_eq!(merged.cmdline, Some(String::from("firstarg secondarg thirdarg")));
    }

    #[fuchsia::test]
    async fn config_fails_due_to_duplicate_vsock_listeners() {
        let (listener_client, listener_server) = create_endpoints::<HostVsockAcceptorMarker>();
        let (listener_client2, listener_server2) = create_endpoints::<HostVsockAcceptorMarker>();
        let userconfig = GuestConfig {
            vsock_listeners: Some(vec![
                Listener { port: 2011, acceptor: listener_client },
                Listener { port: 2011, acceptor: listener_client2 },
            ]),
            ..Default::default()
        };

        let manager = ManagerFixture::new_with_defaults();
        let merged = manager.manager().get_merged_config(userconfig);
        assert!(merged.is_err());
    }

    #[fuchsia::test]
    async fn user_provided_initial_listeners() {
        let manager = ManagerFixture::new_with_defaults();
        let (listener_client, listener_server) = create_endpoints::<HostVsockAcceptorMarker>();
        let (listener_client2, listener_server2) = create_endpoints::<HostVsockAcceptorMarker>();
        let user_config = GuestConfig {
            virtio_vsock: Some(true),
            vsock_listeners: Some(vec![
                Listener { port: 2011, acceptor: listener_client },
                Listener { port: 2022, acceptor: listener_client2 },
            ]),
            ..Default::default()
        };

        let merged = manager.manager().get_merged_config(user_config).unwrap();
        assert_eq!(merged.vsock_listeners.unwrap().len(), 2);
        assert_eq!(merged.virtio_vsock, Some(true));
    }

    #[fuchsia::test]
    async fn vmm_component_crash() {
        let mut tmpfile = NamedTempFile::new().expect("failed to create tempfile");
        write!(tmpfile, "{{}}").expect("failed to write to tempfile");

        let mut manager =
            GuestManager::new(PathBuf::from(""), tmpfile.path().to_str().unwrap().to_string());
        let (stream_tx, state_rx) = mpsc::unbounded::<GuestManagerRequestStream>();

        let (proxy, server) = create_proxy_and_stream::<GuestLifecycleMarker>();

        let run_fut = manager.run(proxy, state_rx).fuse();
        futures::pin_mut!(run_fut);
        let mut vmm = MockVmm::new(server);

        let (manager_proxy, manager_server) = create_proxy_and_stream::<GuestManagerMarker>();

        // Launch Guest
        let (guest_client_end, guest_server_end) =
            fidl::endpoints::create_endpoints::<GuestMarker>();
        stream_tx.unbounded_send(manager_server).expect("stream should never close");

        select! {
            _ = run_fut => {
                panic!("run should not complete")
            }
            result = join(manager_proxy.launch(GuestConfig::default(), guest_server_end),
                          vmm.run_launch()).fuse()
                => {
                    assert!(result.0.is_ok());
                }
        }

        // Dropping the server closes the connection. The status should change, and a new
        // connection should be established.
        drop(vmm);
        let guest_info = select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            result = manager_proxy.get_info() => {
                result.unwrap()
            }
        };

        assert_eq!(guest_info.guest_status, Some(GuestStatus::VmmUnexpectedTermination));
    }

    #[fuchsia::test]
    async fn launch_fails_due_to_invalid_config_path() {
        let manager = ManagerFixture::new_with_config_file(String::from("foo"));
        let run_fut = manager.run().fuse();
        futures::pin_mut!(run_fut);

        let (guest_client_end, guest_server_end) =
            fidl::endpoints::create_endpoints::<GuestMarker>();
        let manager_proxy = manager.new_proxy();

        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            result = manager_proxy.launch(GuestConfig::default(), guest_server_end).fuse() => {
                assert_eq!(result.unwrap(), Err(GuestManagerError::BadConfig));
            }
        }
    }

    #[fuchsia::test]
    async fn launch_fails_due_to_bad_config_schema() {
        let config = r#"{"invalid": "field"}"#;
        let manager = ManagerFixture::new(config);
        let run_fut = manager.run().fuse();
        futures::pin_mut!(run_fut);

        let (guest_client_end, guest_server_end) =
            fidl::endpoints::create_endpoints::<GuestMarker>();

        let manager_proxy = manager.new_proxy();

        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            result = manager_proxy.launch(GuestConfig::default(), guest_server_end).fuse() => {
                assert_eq!(result.unwrap(), Err(GuestManagerError::BadConfig));
            }
        }
    }

    #[fuchsia::test]
    async fn double_launch_fails() {
        let manager = ManagerFixture::new_with_defaults();
        let run_fut = manager.run().fuse();
        futures::pin_mut!(run_fut);
        let manager_proxy = manager.new_proxy();

        let (guest_client_end1, guest_server_end1) =
            fidl::endpoints::create_endpoints::<GuestMarker>();
        let (guest_client_end2, guest_server_end2) =
            fidl::endpoints::create_endpoints::<GuestMarker>();

        // Lanuch Guest
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            _ = manager.run_vmm().fuse() => {
                panic!("vmm should not complete");
            }
            result = manager_proxy.launch(GuestConfig::default(), guest_server_end1).fuse() => {
                assert!(result.is_ok());
            }
        }

        // Try second launch and fail with AlreadyRunning.
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            _ = manager.run_vmm().fuse() => {
                panic!("vmm should not complete");
            }
            result = manager_proxy.launch(GuestConfig::default(), guest_server_end2).fuse() => {
                assert_eq!(result.unwrap(), Err(GuestManagerError::AlreadyRunning));
            }
        }
    }

    #[fuchsia::test]
    async fn failed_to_create_and_initialize_vmm_with_restart() {
        let manager = ManagerFixture::new_with_defaults();
        let run_fut = manager.run().fuse();
        futures::pin_mut!(run_fut);
        let manager_proxy = manager.new_proxy();
        let (guest_client_end1, guest_server_end1) =
            fidl::endpoints::create_endpoints::<GuestMarker>();
        let (guest_client_end2, guest_server_end2) =
            fidl::endpoints::create_endpoints::<GuestMarker>();

        // Launch Guest and fail to start the VMM
        let launch_fut =
            join(manager_proxy.launch(GuestConfig::default(), guest_server_end1), async {
                match manager.vmm.borrow_mut().next().await {
                    GuestLifecycleRequest::Create { guest_config, responder } => {
                        responder
                            .send(Err(GuestError::InternalError))
                            .expect("stream should not be closed");
                    }
                    req => panic!("vmm: expected create, got {:?}", req),
                }
            })
            .fuse();
        futures::pin_mut!(launch_fut);
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            result = launch_fut => {
                assert_eq!(result.0.unwrap(), Err(GuestManagerError::StartFailure));
            }
        }

        // Second launch succeeds.
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            _ = manager.run_vmm().fuse() => {
                panic!("vmm should not complete");
            }
            result = manager_proxy.launch(GuestConfig::default(), guest_server_end2).fuse() => {
                assert!(result.is_ok());
            }
        }
    }

    #[fuchsia::test]
    async fn get_guest_info_new_guest() {
        let manager = ManagerFixture::new_with_defaults();
        let problems = vec![String::from("test problem")];
        let info = manager.manager().guest_info(problems.clone());

        assert_eq!(info.guest_status, Some(GuestStatus::NotStarted));
        assert_eq!(info.detected_problems, Some(problems));
    }

    #[fuchsia::test]
    async fn launch_and_get_info() {
        let manager = ManagerFixture::new_with_defaults();

        let (guest_client_end, guest_server_end) =
            fidl::endpoints::create_endpoints::<GuestMarker>();

        let run_fut = manager.run().fuse();
        futures::pin_mut!(run_fut);

        let manager_proxy = manager.new_proxy();

        // Lanuch Guest
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            _ = manager.run_vmm().fuse() => {
                panic!("vmm should not complete");
            }
            result = manager_proxy.launch(GuestConfig::default(), guest_server_end).fuse() => {
                assert!(result.is_ok());
            }
        }

        // Get info and check that settings are correctly inherited or overridden.
        let guest_info = select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            result = manager_proxy.get_info().fuse() => {
                result.unwrap()
            }
        };

        assert_eq!(guest_info.guest_status, Some(GuestStatus::Running));
        assert!(guest_info.uptime.unwrap() > 0);
        let descriptor = GuestDescriptor {
            num_cpus: Some(get_default_num_cpus()),
            guest_memory: Some(get_default_guest_memory()),
            ..Default::default()
        };
        assert_eq!(guest_info.guest_descriptor, Some(descriptor));
        assert_eq!(
            guest_info.detected_problems,
            Some(vec![GuestNetworkState::FailedToQuery.to_string()])
        );
    }

    #[fuchsia::test]
    async fn launch_and_apply_user_guest_config() {
        let manager =
            ManagerFixture::new(r#"{"virtio-gpu": false, "virtio-balloon": false, "cpus": 4}"#);
        let user_config =
            GuestConfig { virtio_gpu: Some(true), cpus: Some(8), ..Default::default() };

        let (guest_client_end, guest_server_end) =
            fidl::endpoints::create_endpoints::<GuestMarker>();
        let manager_proxy = manager.new_proxy();

        let run_fut = manager.run().fuse();
        futures::pin_mut!(run_fut);

        // Lanuch Guest
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            _ = manager.run_vmm().fuse() => {
                panic!("vmm should not complete");
            }
            result = manager_proxy.launch(user_config, guest_server_end).fuse() => {
                assert!(result.is_ok());
            }
        }

        // Get info and check that settings are correctly inherited or overridden.
        let guest_descriptor = select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            result = manager_proxy.get_info() => {
                result.unwrap().guest_descriptor.unwrap()
            }
        };

        assert_eq!(guest_descriptor.num_cpus, Some(8));
        assert_eq!(guest_descriptor.balloon, Some(false));
        assert_eq!(guest_descriptor.gpu, Some(true));
    }

    #[fuchsia::test]
    async fn force_shutdown_non_running_guest() {
        let manager = ManagerFixture::new_with_defaults();
        let init_state = manager.manager().status;

        let manager_proxy = manager.new_proxy();
        let run_fut = manager.run().fuse();
        futures::pin_mut!(run_fut);

        // Run shutdown.
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            _ = manager.run_vmm().fuse() => {
                panic!("vmm should not complete");
            }
            result = manager_proxy.force_shutdown().fuse() => {
                assert!(result.is_ok());
            }
        };

        // Get info and check state hasn't changed
        let guest_info = select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            info = manager_proxy.get_info().fuse() => {
                info.unwrap()
            }
        };

        assert_eq!(init_state, guest_info.guest_status.unwrap());
    }

    #[fuchsia::test]
    async fn force_shutdown_guest() {
        let manager = ManagerFixture::new_with_defaults();

        let (guest_client_end, guest_server_end) =
            fidl::endpoints::create_endpoints::<GuestMarker>();

        let manager_proxy = manager.new_proxy();
        let run_fut = manager.run().fuse();
        let vmm_fut = manager.run_vmm().fuse();
        futures::pin_mut!(run_fut);
        futures::pin_mut!(vmm_fut);

        // Lanuch Guest
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            _ = vmm_fut => {
                panic!("vmm should not complete");
            }
            result = manager_proxy.launch(GuestConfig::default(), guest_server_end).fuse() => {
                assert!(result.is_ok());
            }
        }

        // Future returning guest info before and after a shutdown.
        let chain_fut = manager_proxy
            .get_info()
            .then(|pre_shutdown| async {
                assert!(manager_proxy.force_shutdown().await.is_ok());
                pre_shutdown
            })
            .then(|pre_shutdown| async { (pre_shutdown, manager_proxy.get_info().await) });
        futures::pin_mut!(chain_fut);

        let (pre_shutdown, post_shutdown) = select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            _ = vmm_fut => {
                panic!("vmm should not complete");
            }
            infos = chain_fut => {
                (infos.0.unwrap(), infos.1.unwrap())
            }
        };

        assert!(pre_shutdown.uptime.unwrap() < post_shutdown.uptime.unwrap());
        assert_eq!(pre_shutdown.guest_status, Some(GuestStatus::Running));
        assert_eq!(post_shutdown.guest_status, Some(GuestStatus::Stopped));
    }

    #[fuchsia::test]
    async fn guest_initiated_clean_shutdown() {
        let manager = ManagerFixture::new_with_defaults();

        let (guest_client_end, guest_server_end) =
            fidl::endpoints::create_endpoints::<GuestMarker>();

        let manager_proxy = manager.new_proxy();
        let run_fut = manager.run().fuse();
        let mut vmm = manager.vmm.borrow_mut();
        futures::pin_mut!(run_fut);

        // Lanuch Guest
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            result = join(manager_proxy.launch(GuestConfig::default(), guest_server_end),
                          vmm.run_launch()).fuse()
                => {
                    assert!(result.0.is_ok());
                }
        }

        // Check that guest is running, respond via lifecycle callback and check again to
        // confirm guest has stopped.
        let chain_fut = manager_proxy
            .get_info()
            .then(|pre_shutdown| async {
                assert_eq!(vmm.state, VmmState::Running);
                vmm.stop();
                pre_shutdown
            })
            .then(|pre_shutdown| async { (pre_shutdown, manager_proxy.get_info().await) });
        futures::pin_mut!(chain_fut);

        let (pre_shutdown, post_shutdown) = select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            infos = chain_fut => {
                (infos.0.unwrap(), infos.1.unwrap())
            }
        };

        assert_eq!(pre_shutdown.guest_status, Some(GuestStatus::Running));
        assert_eq!(post_shutdown.guest_status, Some(GuestStatus::Stopped));
    }

    #[fuchsia::test]
    async fn connect_to_vmm() {
        let manager = ManagerFixture::new_with_defaults();

        let (guest_client_end_connect_fail, guest_server_end_connect_fail) =
            fidl::endpoints::create_endpoints::<GuestMarker>();
        let (guest_client_end_launch, guest_server_end_launch) =
            fidl::endpoints::create_endpoints::<GuestMarker>();
        let (guest_client_end_connect_success, guest_server_end_connect_success) =
            fidl::endpoints::create_endpoints::<GuestMarker>();

        let manager_proxy = manager.new_proxy();
        let run_fut = manager.run().fuse();
        futures::pin_mut!(run_fut);

        // Try connect when guest isn't running and fail.
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            connection = manager_proxy.connect(guest_server_end_connect_fail).fuse() => {
                assert_eq!(connection.unwrap(), Err(GuestManagerError::NotRunning));
            }
        }

        // Launch guest.
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            _ = manager.run_vmm().fuse() => {
                panic!("vmm should not complete");
            }
            result = manager_proxy.launch(GuestConfig::default(), guest_server_end_launch).fuse() => {
                assert!(result.is_ok());
            }
        }

        // Try connect again and succeed.
        select! {
            _ = run_fut => {
                panic!("run should not complete");
            }
            connection = manager_proxy.connect(guest_server_end_connect_success).fuse() => {
                assert!(connection.unwrap().is_ok());
            }
        }
    }

    const DEFAULT_NET_DEVICE: NetSpec =
        NetSpec { mac_address: DEFAULT_GUEST_MAC_ADDRESS, enable_bridge: true };

    fn guest_network_test(
        host_state: HostNetworkState,
        guest_networks: Option<Vec<NetSpec>>,
    ) -> GuestNetworkState {
        let manager = ManagerFixture::new_with_defaults();
        let mut guest_manager = manager.manager.borrow_mut();
        guest_manager.guest_descriptor =
            GuestDescriptor { networks: guest_networks, ..Default::default() };

        guest_manager.guest_network_state(host_state)
    }

    #[fuchsia::test]
    async fn guest_probably_has_networking() {
        let guest_state = guest_network_test(
            HostNetworkState {
                has_ethernet: true,
                num_virtual: 1,
                has_bridge: true,
                ..HostNetworkState::default()
            },
            Some(vec![DEFAULT_NET_DEVICE]),
        );

        assert_eq!(guest_state, GuestNetworkState::Ok);
    }

    #[fuchsia::test]
    async fn guest_no_network_devices() {
        let guest_state = guest_network_test(HostNetworkState::default(), None);
        assert_eq!(guest_state, GuestNetworkState::NoNetworkDevice);
    }

    #[fuchsia::test]
    async fn guest_bridging_required_but_host_on_wifi() {
        let guest_state = guest_network_test(
            HostNetworkState {
                has_wlan: true,
                has_bridge: false,
                num_virtual: 1,
                has_ethernet: false,
            },
            Some(vec![DEFAULT_NET_DEVICE]),
        );

        assert_eq!(guest_state, GuestNetworkState::AttemptedToBridgeWithWlan);
    }

    #[fuchsia::test]
    async fn guest_bridging_required_and_host_on_wifi_and_ethernet() {
        let guest_state = guest_network_test(
            HostNetworkState {
                has_wlan: true,
                has_bridge: false,
                num_virtual: 1,
                has_ethernet: true,
            },
            Some(vec![DEFAULT_NET_DEVICE]),
        );

        assert_eq!(guest_state, GuestNetworkState::NoBridgeCreated);
    }

    #[fuchsia::test]
    async fn guest_bridging_required_and_host_on_ethernet() {
        let guest_state = guest_network_test(
            HostNetworkState {
                has_wlan: false,
                has_bridge: false,
                num_virtual: 1,
                has_ethernet: true,
            },
            Some(vec![DEFAULT_NET_DEVICE]),
        );

        assert_eq!(guest_state, GuestNetworkState::NoBridgeCreated);
    }

    #[fuchsia::test]
    async fn not_enough_virtual_interfaces_for_guest() {
        let guest_state = guest_network_test(
            HostNetworkState {
                has_wlan: false,
                has_bridge: true,
                num_virtual: 0,
                has_ethernet: true,
            },
            Some(vec![DEFAULT_NET_DEVICE]),
        );

        assert_eq!(guest_state, GuestNetworkState::MissingVirtualInterfaces);
    }

    #[fuchsia::test]
    async fn guest_requires_networking_but_no_host_networking() {
        let guest_state = guest_network_test(
            HostNetworkState {
                has_wlan: false,
                has_ethernet: false,
                ..HostNetworkState::default()
            },
            Some(vec![DEFAULT_NET_DEVICE]),
        );

        assert_eq!(guest_state, GuestNetworkState::NoHostNetworking);
    }
}
