// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

use anyhow::Error;
use argh::FromArgs;
use fidl::Error as FidlError;
use fidl_fuchsia_bluetooth_snoop::{
    CaptureError, DevicePackets, SnoopPacket as FidlSnoopPacket, SnoopRequest, SnoopRequestStream,
    SnoopStartRequest, UnrecognizedDeviceName,
};
use fidl_fuchsia_feedback::CrashReporterMarker;
use fidl_fuchsia_firmware_crash as fcrash;

use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect as inspect;
use fuchsia_trace as trace;
use futures::future::{Join, Ready, join, ready};
use futures::select;
use futures::stream::{FusedStream, FuturesUnordered, Stream, StreamExt, StreamFuture};
use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use crate::packet_logs::PacketLogs;
use crate::snooper::{SnoopPacket, Snooper};
use crate::subscription_manager::SubscriptionManager;

mod bounded_queue;
mod core_dump;
use crate::core_dump::{CoreDumpCollector, CrashEventStatus, CrashState};
mod packet_logs;
mod snooper;
mod subscription_manager;
#[cfg(test)]
mod tests;

/// Size of the standard HCI event header (Event Code + Length).
pub(crate) const HCI_EVENT_HEADER_SIZE: usize = 2;

/// A `DeviceId` represents the name of a device (such as a service instance name).
pub(crate) type DeviceId = String;

/// A request is a tuple of the client id, and the next request or error from the stream, or None
/// if the stream has closed.
type ClientRequest = (ClientId, Option<Result<SnoopRequest, FidlError>>);

/// A `Stream` that holds a collection of client request streams and will return the item from the
/// next ready stream.
type ConcurrentClientRequestFutures =
    FuturesUnordered<Join<Ready<ClientId>, StreamFuture<SnoopRequestStream>>>;

/// A `Stream` that holds a collection of snooper streams and will return the item from the
/// next ready stream.
type ConcurrentSnooperPacketFutures = FuturesUnordered<StreamFuture<Snooper>>;

/// A `ClientId` represents the unique identifier for a client that has connected to the bt-snoop
/// service.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ClientId(u64);

impl fmt::Display for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Generates 64-bit ids in increasing order with wrap around behavior at `u64::MAX`
/// Ids will be unique, as long as there is not a client that lives longer than the
/// next 2^63-1 clients.
struct IdGenerator(ClientId);

impl IdGenerator {
    fn new() -> IdGenerator {
        IdGenerator(ClientId(0))
    }
    fn next(&mut self) -> ClientId {
        let id = self.0;
        (self.0).0 = (self.0).0.wrapping_add(1);
        id
    }
}

async fn process_vendor_connection(
    path: &str,
    vendor: &fidl_fuchsia_hardware_bluetooth::VendorProxy,
    snoopers: &mut ConcurrentSnooperPacketFutures,
    crash_states: &mut HashMap<DeviceId, CrashState>,
    packet_logs: &mut PacketLogs,
    subscribers: &mut SubscriptionManager,
) {
    let crash_params = match vendor.get_crash_parameters().await {
        Ok(Ok(params)) => Some(params),
        Ok(Err(e)) => {
            debug!("Device {} does not support crash reporting: {:?}", path, e);
            None
        }
        Err(e) => {
            warn!("FIDL error getting crash parameters for {}: {:?}", path, e);
            None
        }
    };

    match Snooper::from_vendor(vendor, path).await {
        Ok(snooper) => {
            snoopers.push(snooper.into_future());
            let removed_device = packet_logs.add_device(path.to_string());
            if let Some(device) = removed_device {
                subscribers.remove_device(&device);
                let _ = crash_states.remove(&device);
            }
            if let Some(params) = crash_params {
                let last_report_local_time =
                    crash_states.get(path).and_then(|s| s.last_report_local_time);
                let _ = crash_states.insert(
                    path.to_string(),
                    CrashState {
                        parameters: params,
                        last_report_local_time,
                        collector: None,
                        tentative_report_file_time: None,
                    },
                );
            }
        }
        Err(e) => {
            warn!("Failed to open snoop channel for \"{path}\": {e:?}");
        }
    }
}

fn spawn_crash_timer(
    crash_timers: &mut FuturesUnordered<fasync::Task<DeviceId>>,
    device_id: DeviceId,
    target: fuchsia_async::MonotonicInstant,
) {
    crash_timers.push(fasync::Task::spawn(async move {
        fasync::Timer::new(target).await;
        device_id
    }));
}

fn handle_crash_timer_fired(
    device_id: DeviceId,
    crash_states: &mut HashMap<DeviceId, CrashState>,
    reporting_tasks: &mut FuturesUnordered<fasync::Task<()>>,
    crash_timers: &mut FuturesUnordered<fasync::Task<DeviceId>>,
) {
    let Some(state) = crash_states.get_mut(&device_id) else {
        return;
    };

    let Some(target) = state.tentative_report_file_time else {
        return;
    };

    if fuchsia_async::MonotonicInstant::now() < target {
        spawn_crash_timer(crash_timers, device_id, target);
        return;
    }

    state.tentative_report_file_time = None;
    let Some(collector) = state.collector.take() else {
        return;
    };

    spawn_reporting_task(reporting_tasks, collector);
}

fn spawn_reporting_task(
    reporting_tasks: &mut FuturesUnordered<fasync::Task<()>>,
    collector: CoreDumpCollector,
) {
    // Spawn a task because file_report is a long-running operation.
    reporting_tasks.push(fasync::Task::spawn(async move {
        match fuchsia_component::client::connect_to_protocol::<fcrash::ReporterMarker>() {
            Ok(firmware_reporter) => {
                collector.file_firmware_report(&firmware_reporter);
            }
            Err(e) => {
                warn!("Failed to connect to fuchsia.firmware.crash.Reporter: {:?}", e);
            }
        }

        match fuchsia_component::client::connect_to_protocol::<CrashReporterMarker>() {
            Ok(crash_reporter) => {
                collector.file_report(&crash_reporter).await;
            }
            Err(e) => {
                warn!("Failed to connect to fuchsia.feedback.CrashReporter: {:?}", e);
            }
        }
    }));
}

/// Handle a new service instance in the bluetooth service directory.
async fn handle_service_instance(
    instance: fidl_fuchsia_hardware_bluetooth::ServiceProxy,
    snoopers: &mut ConcurrentSnooperPacketFutures,
    subscribers: &mut SubscriptionManager,
    packet_logs: &mut PacketLogs,
    crash_states: &mut HashMap<DeviceId, CrashState>,
) {
    let path = instance.instance_name().to_string();
    info!("Opening snoop channel via service for \"{path}\"");
    match instance.connect_to_vendor() {
        Ok(vendor) => {
            process_vendor_connection(
                &path,
                &vendor,
                snoopers,
                crash_states,
                packet_logs,
                subscribers,
            )
            .await;
        }
        Err(e) => warn!("Failed to connect to vendor on service instance {path}: {e:?}"),
    }
}

fn register_new_client(
    stream: SnoopRequestStream,
    client_stream: &mut ConcurrentClientRequestFutures,
    client_id: ClientId,
) {
    client_stream.push(join(ready(client_id), stream.into_future()));
}

/// Maximum serialized size in bytes of a single FIDL message sent to the client.
/// FIDL channel message limit is 64 KiB (65,536 bytes). We stay comfortably below this limit.
pub(crate) const MAX_BYTES_PER_MSG: usize = 60_000;

/// Maximum number of packets sent in a single FIDL message to the client.
pub(crate) const MAX_PACKETS_PER_MSG: usize = 100;

/// Estimated FIDL serialization overhead per packet in bytes (table and envelope headers).
pub(crate) const ESTIMATED_FIDL_OVERHEAD_BYTES: usize = 150;

/// Helper iterator that chunks snoop packets so that neither the packet count
/// (at most `MAX_PACKETS_PER_MSG`) nor the estimated FIDL serialized byte size
/// (at most `MAX_BYTES_PER_MSG`) exceeds message limits.
struct PacketChunker<I: Iterator<Item = FidlSnoopPacket>> {
    iter: I,
    buffered_packet: Option<FidlSnoopPacket>,
}

impl<I: Iterator<Item = FidlSnoopPacket>> Iterator for PacketChunker<I> {
    type Item = Vec<FidlSnoopPacket>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut current_chunk = Vec::with_capacity(MAX_PACKETS_PER_MSG);
        let mut current_bytes = 0;

        if let Some(packet) = self.buffered_packet.take() {
            let packet_bytes =
                ESTIMATED_FIDL_OVERHEAD_BYTES + packet.data.as_ref().map_or(0, |d| d.len());
            current_bytes += packet_bytes;
            current_chunk.push(packet);
        }

        while current_chunk.len() < MAX_PACKETS_PER_MSG {
            let Some(packet) = self.iter.next() else {
                break;
            };

            let packet_bytes =
                ESTIMATED_FIDL_OVERHEAD_BYTES + packet.data.as_ref().map_or(0, |d| d.len());

            if !current_chunk.is_empty() && (current_bytes + packet_bytes > MAX_BYTES_PER_MSG) {
                self.buffered_packet = Some(packet);
                break;
            }

            current_bytes += packet_bytes;
            current_chunk.push(packet);
        }

        if current_chunk.is_empty() { None } else { Some(current_chunk) }
    }
}

pub(crate) fn chunk_packets(
    packets: impl IntoIterator<Item = FidlSnoopPacket>,
) -> impl Iterator<Item = Vec<FidlSnoopPacket>> {
    PacketChunker { iter: packets.into_iter(), buffered_packet: None }
}

/// Handle a client request to dump the packet log, subscribe to future events or do both.
/// Returns an error if the client channel does not accept a response that it requested, or a
/// boolean indicating if the client should receive ongoing packets.
async fn handle_client_request(
    request: ClientRequest,
    subscribers: &mut SubscriptionManager,
    packet_logs: &PacketLogs,
) -> Result<bool, Error> {
    let (id, request) = request;
    info!("Request received from client {id}.");
    match request {
        Some(Ok(SnoopRequest::Start {
            payload: SnoopStartRequest { follow, host_device, client, .. },
            ..
        })) => {
            info!("Start request from client: {follow:?}, {host_device:?}");

            let Some(client) = client.map(|client| client.into_proxy()) else {
                warn!("No client delivered, skipping");
                return Ok(true);
            };

            let device_ids: Vec<String> = match &host_device {
                Some(device) => {
                    let Some(_log) = packet_logs.get(device) else {
                        warn!("Couldn't find device: {device}, sending error to client");
                        let _ = client.error(&CaptureError::UnrecognizedDeviceName(
                            UnrecognizedDeviceName::default(),
                        ));
                        drop(client);
                        return Ok(true);
                    };
                    vec![device.clone()]
                }
                None => packet_logs.device_ids().cloned().collect(),
            };

            let mut dev_packets: HashMap<_, _> = Default::default();
            for device in &device_ids {
                let log = packet_logs.get(device).unwrap();
                let packets: &mut Vec<FidlSnoopPacket> =
                    dev_packets.entry(device.clone()).or_insert_with(Vec::new);
                packets.extend(log.lock().iter_mut().map(|e| (&*e).to_fidl()));
            }

            for (device, packets) in dev_packets.into_iter() {
                info!("Dumping {} packets from {} to new client..", packets.len(), device);
                for chunk in chunk_packets(packets) {
                    if let Err(e) = client
                        .observe(&DevicePackets {
                            host_device: Some(device.clone()),
                            packets: Some(chunk),
                            ..Default::default()
                        })
                        .await
                    {
                        warn!("Failed to send a previously observed packet to client: {e:?}");
                        return Ok(true);
                    }
                }
            }

            if follow.is_some_and(|f| f) {
                if let Err(e) = subscribers.register(id, client, host_device) {
                    warn!("Failed to register new subscriber: {e:?}");
                }
            }
        }
        Some(Ok(_)) => {
            warn!("Unknown method called on Snoop from {id:?}, closing stream");
        }
        Some(Err(e)) => {
            warn!("Client returned error: {e:?}");
            subscribers.deregister(&id);
        }
        None => {
            debug!("Client disconnected");
            subscribers.deregister(&id);
        }
    }
    Ok(false)
}

/// The outcome of handling a possible incoming packet.
pub(crate) enum HandlePacketOutcome {
    /// The snoop channel for the device has closed.
    ChannelClosed,
    /// The snoop channel closed with an in-flight core dump ready to be reported.
    DumpReady(CoreDumpCollector),
    /// The packet was processed normally.
    Processed,
    /// A crash was detected and a new crash dump collection has started.
    CrashDetected(DeviceId),
}

/// Handle a possible incoming packet. Returns the outcome of the processing.
pub(crate) fn handle_packet(
    device_name: &DeviceId,
    packet: Option<(DeviceId, SnoopPacket)>,
    subscribers: &mut SubscriptionManager,
    packet_logs: &mut PacketLogs,
    truncate_payload: Option<usize>,
    crash_states: &mut HashMap<DeviceId, CrashState>,
) -> HandlePacketOutcome {
    let Some((device, mut packet)) = packet else {
        info!("Snoop channel closed for device: {}", device_name);
        if let Some(state) = crash_states.get_mut(device_name) {
            state.tentative_report_file_time = None;
            if let Some(collector) = state.collector.take() {
                return HandlePacketOutcome::DumpReady(collector);
            }
        }
        return HandlePacketOutcome::ChannelClosed;
    };
    trace!("Received packet from {}.", device_name);

    let mut crash_status = CrashEventStatus::NotCrashEvent;
    if let Some(state) = crash_states.get_mut(&device) {
        crash_status = state.process_packet(&packet);
    }

    if let Some(len) = truncate_payload {
        packet.payload.truncate(len);
    }
    subscribers.notify(&device, &packet);

    if crash_status != CrashEventStatus::NotCrashEvent {
        packet.payload.truncate(HCI_EVENT_HEADER_SIZE);
    }
    packet_logs.log_packet(&device, packet);

    if crash_status == CrashEventStatus::FirstCrashEvent {
        HandlePacketOutcome::CrashDetected(device)
    } else {
        HandlePacketOutcome::Processed
    }
}

struct SnoopConfig {
    log_size_soft_max_bytes: usize,
    log_size_hard_max_bytes: usize,
    log_time: Duration,
    max_device_count: usize,
    truncate_payload: Option<usize>,

    // Inspect tree
    _config_inspect: inspect::Node,
    _log_size_soft_max_bytes_property: inspect::UintProperty,
    _log_size_hard_max_bytes_property: inspect::StringProperty,
    _log_time_property: inspect::UintProperty,
    _max_device_count_property: inspect::UintProperty,
    _truncate_payload_property: inspect::StringProperty,
}

impl SnoopConfig {
    /// Creates a strongly typed `SnoopConfig` out of primitives parsed from the command line
    fn from_args(args: Args, config_inspect: inspect::Node) -> SnoopConfig {
        let log_size_soft_max_bytes = args.log_size_soft_kib * 1024;
        let log_size_hard_max_bytes = args.log_size_hard_kib * 1024;
        let log_time = Duration::from_secs(args.log_time_seconds);
        let _log_size_soft_max_bytes_property =
            config_inspect.create_uint("log_size_soft_max_bytes", log_size_soft_max_bytes as u64);
        let hard_max = if log_size_hard_max_bytes == 0 {
            "No Hard Max".to_string()
        } else {
            log_size_hard_max_bytes.to_string()
        };
        let _log_size_hard_max_bytes_property =
            config_inspect.create_string("log_size_hard_max_bytes", &hard_max);
        let _log_time_property = config_inspect.create_uint("log_time", log_time.as_secs());
        let _max_device_count_property =
            config_inspect.create_uint("max_device_count", args.max_device_count as u64);
        let truncate = args
            .truncate_payload
            .as_ref()
            .map(|n| format!("{} bytes", n))
            .unwrap_or_else(|| "No Truncation".to_string());
        let _truncate_payload_property =
            config_inspect.create_string("truncate_payload", &truncate);

        SnoopConfig {
            log_size_soft_max_bytes,
            log_size_hard_max_bytes,
            log_time,
            max_device_count: args.max_device_count,
            truncate_payload: args.truncate_payload,
            _config_inspect: config_inspect,
            _log_size_soft_max_bytes_property,
            _log_size_hard_max_bytes_property,
            _log_time_property,
            _max_device_count_property,
            _truncate_payload_property,
        }
    }
}

#[derive(FromArgs)]
/// Log bluetooth snoop packets and provide them to clients.
struct Args {
    #[argh(option, default = "32")]
    /// packet storage buffer size after which packets will start aging off.
    log_size_soft_kib: usize,
    #[argh(option, default = "256")]
    /// hard maximum size in KiB of the buffer to store packets in.
    /// a value of "0" indicates no limit. Defaults to 0.
    log_size_hard_kib: usize,
    #[argh(option, default = "60")]
    /// minimum time to store packets in a snoop log in seconds.
    log_time_seconds: u64,
    #[argh(option, default = "8")]
    /// maximum number of devices for which to store logs.
    max_device_count: usize,
    #[argh(option)]
    /// maximum number of bytes to keep in the payload of incoming packets. Defaults to no limit.
    truncate_payload: Option<usize>,
}

/// Setup the main loop of execution in a Task and run it.
async fn run(
    config: SnoopConfig,
    mut service_handler: impl Unpin + FusedStream + Stream<Item = SnoopRequestStream>,
    inspect: inspect::Node,
) -> Result<(), Error> {
    let mut id_gen = IdGenerator::new();
    let service_stream = match fuchsia_component::client::Service::open(
        fidl_fuchsia_hardware_bluetooth::ServiceMarker,
    ) {
        Ok(service) => match service.watch().await {
            Ok(watcher) => Some(watcher.boxed()),
            Err(e) => {
                warn!("Failed to watch bluetooth service: {:?}", e);
                None
            }
        },
        Err(e) => {
            warn!("Failed to open bluetooth service: {:?}", e);
            None
        }
    };
    let mut service_stream =
        service_stream.unwrap_or_else(|| futures::stream::empty().boxed()).fuse();

    let mut client_requests = ConcurrentClientRequestFutures::new();
    let mut subscribers = SubscriptionManager::new();
    let mut snoopers = ConcurrentSnooperPacketFutures::new();
    let mut reporting_tasks = FuturesUnordered::new();
    let mut crash_timers = FuturesUnordered::new();
    let mut packet_logs = PacketLogs::new(
        config.max_device_count,
        config.log_size_soft_max_bytes,
        config.log_size_hard_max_bytes,
        config.log_time,
        inspect,
    );

    let mut crash_states: HashMap<DeviceId, CrashState> = HashMap::new();

    debug!("Capturing snoop packets...");

    loop {
        select! {
            // A new client has connected to one of the exposed services.
            request_stream = service_handler.select_next_some() => {
                let client_id = id_gen.next();
                info!("New client connection: {client_id}");
                register_new_client(request_stream, &mut client_requests, client_id);
            },

            // A new service instance has appeared.
            instance = service_stream.select_next_some() => {
                match instance {
                    Ok(instance) => {
                        handle_service_instance(
                            instance,
                            &mut snoopers,
                            &mut subscribers,
                            &mut packet_logs,
                            &mut crash_states,
                        )
                        .await;
                    }
                    Err(e) => {
                        warn!("Error watching bluetooth service instances: {:?}", e);
                    }
                }
            },

            // A client has made a request to the server.
            request = client_requests.select_next_some() => {
                let (client_id, (request, client_stream)) = request;
                match handle_client_request((client_id, request),
                    &mut subscribers, &packet_logs).await {
                 Err(e) => {
                    warn!("Error handling client request: {e:?}");
                 },
                 Ok(true) => register_new_client(client_stream, &mut client_requests, client_id),
                 _ => {},
                }
            },

            // A new snoop packet has been received from an hci device.
            (packet, snooper) = snoopers.select_next_some() => {
                trace::duration!("bluetooth", "Snoop::ProcessPacket");
                let device_name = snooper.device_name.clone();
                match handle_packet(
                    &device_name,
                    packet,
                    &mut subscribers,
                    &mut packet_logs,
                    config.truncate_payload,
                    &mut crash_states,
                ) {
                    HandlePacketOutcome::CrashDetected(device_id) => {
                        if let Some(target) =
                            crash_states.get(&device_id).and_then(|s| s.tentative_report_file_time)
                        {
                            spawn_crash_timer(&mut crash_timers, device_id.clone(), target);
                        }
                        snoopers.push(snooper.into_future());
                    }
                    HandlePacketOutcome::Processed => snoopers.push(snooper.into_future()),
                    HandlePacketOutcome::DumpReady(collector) => {
                        spawn_reporting_task(&mut reporting_tasks, collector);
                    }
                    HandlePacketOutcome::ChannelClosed => {}
                }
            },

            // New crash timer fired
            device_id = crash_timers.select_next_some() => {
                handle_crash_timer_fired(
                    device_id,
                    &mut crash_states,
                    &mut reporting_tasks,
                    &mut crash_timers,
                );
            },

            // Reaping reporting tasks
            _ = reporting_tasks.select_next_some() => {},
        }
    }
}

/// Parse program arguments, call the main loop, and log any unrecoverable errors.
/// TODO(https://fxbug.dev/42076557): migrate runtime config to structured config.
#[fuchsia::main(logging_tags=["bt-snoop"])]
async fn main() {
    let args: Args = argh::from_env();

    let mut fs = ServiceFs::new();

    let inspector = inspect::Inspector::default();
    let _inspect_server_task =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());

    let config_inspect = inspector.root().create_child("configuration");
    let runtime_inspect = inspector.root().create_child("runtime_metrics");

    let config = SnoopConfig::from_args(args, config_inspect);

    let _ = fs.dir("svc").add_fidl_service(|stream: SnoopRequestStream| stream);

    let _ = fs.take_and_serve_directory_handle().expect("serve ServiceFS directory");

    match run(config, fs.fuse(), runtime_inspect).await {
        Err(err) => error!("Failed with critical error: {:?}", err),
        _ => {}
    };
}
