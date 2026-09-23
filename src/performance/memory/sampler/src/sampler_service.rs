// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This modules contains most of the top-level logic of the sampler
//! service. It defines functions to process a stream of profiling
//! requests and produce a complete profile, as well as utilities to
//! persist it.
use crate::crash_reporter::ProfileReport;
use crate::profile_builder::ProfileBuilder;

use anyhow::Error;
use fidl_fuchsia_memory_sampler::{
    RecordAllocationEvent, RecordDeallocationEvent, SamplerRequest, SamplerRequestStream,
    SamplerSetProcessInfoRequest,
};
use fuchsia_async::Task;
use fuchsia_component::server::ServiceFs;
use futures::channel::mpsc;
use futures::prelude::*;
use futures::stream::SelectAll;
use std::time::{Duration, Instant};

/// The threshold of recorded stack traces to trigger a partial
/// report. The overhead for each stack trace is of the order of 1
/// KiB; keeping this below 1000 should keep the residual memory
/// *for a single profiled process* roughly under ~1 MiB.
const RECLAIMABLE_STACK_TRACES_PROFILE_THRESHOLD: usize = 1000;

/// Upper bound on the number of concurrent connections served.
const MAX_CONCURRENT_REQUESTS: usize = 10;

/// Upper bound on the elapsed time between producing two partial
/// profiles.
const MAX_DURATION_BETWEEN_PARTIAL_PROFILES: Duration = Duration::from_secs(12 * 60 * 60);

/// Marks `builder` lossy if the client has signalled that it dropped datagrams
/// because the shared socket's buffer was saturated. The signal latches on the
/// socket, so polling it at profile emission time is sufficient and keeps the
/// syscall off the per-datagram path.
fn check_lossy_signal(socket: &Option<zx::Socket>, builder: &mut ProfileBuilder) {
    if builder.is_lossy() {
        return;
    }
    let Some(socket) = socket else {
        return;
    };
    if let zx::WaitResult::Ok(signals) =
        socket.wait_one(zx::Signals::USER_0, zx::MonotonicInstant::ZERO)
        && signals.contains(zx::Signals::USER_0)
    {
        builder.set_lossy(true);
    }
}

async fn maybe_emit_partial_profile<'a>(
    builder: &'a mut ProfileBuilder,
    tx: &'a mut mpsc::Sender<ProfileReport>,
    index: usize,
    mut time_of_last_profile: Instant,
    shared_socket: &Option<zx::Socket>,
) -> Result<Option<(&'a mut ProfileBuilder, &'a mut mpsc::Sender<ProfileReport>, Instant)>, Error> {
    let now = Instant::now();
    if (now - time_of_last_profile >= MAX_DURATION_BETWEEN_PARTIAL_PROFILES)
        || (builder.get_approximate_reclaimable_stack_traces_count()
            >= RECLAIMABLE_STACK_TRACES_PROFILE_THRESHOLD)
    {
        check_lossy_signal(shared_socket, builder);
        let profile = builder.build_partial_profile(index)?;
        let res = tx.try_send(profile);
        if let Err(ref e) = res
            && e.is_full()
        {
            log::warn!(
                "{}: Failed to send partial profile, channel is full",
                builder.process_name()
            );
            return Ok(None);
        };
        res?;
        time_of_last_profile = now;
    }
    Ok(Some((builder, tx, time_of_last_profile)))
}

enum ClientEvent {
    RecordAllocation(RecordAllocationEvent),
    RecordDeallocation(RecordDeallocationEvent),
    SetProcessInfo(SamplerSetProcessInfoRequest),
    SetSharedSocket(zx::Socket),
    Ignored,
    PeerClosed,
}

impl From<&[u8]> for ClientEvent {
    fn from(datagram: &[u8]) -> ClientEvent {
        if datagram.is_empty() {
            return ClientEvent::PeerClosed;
        }
        match fidl::unpersist::<fidl_fuchsia_memory_sampler::SamplerDatagram>(&datagram) {
            Ok(fidl_fuchsia_memory_sampler::SamplerDatagram::RecordAllocation(alloc)) => {
                ClientEvent::RecordAllocation(alloc)
            }
            Ok(fidl_fuchsia_memory_sampler::SamplerDatagram::RecordDeallocation(dealloc)) => {
                ClientEvent::RecordDeallocation(dealloc)
            }
            Ok(fidl_fuchsia_memory_sampler::SamplerDatagramUnknown!()) => {
                log::warn!("Received unknown SamplerDatagram variant");
                ClientEvent::Ignored
            }
            Err(e) => {
                log::warn!("Failed to handle datagram: {:#}", e);
                ClientEvent::Ignored
            }
        }
    }
}

impl From<SamplerRequest> for ClientEvent {
    fn from(request: SamplerRequest) -> ClientEvent {
        match request {
            SamplerRequest::SetSharedSocket { socket, .. } => ClientEvent::SetSharedSocket(socket),
            SamplerRequest::RecordAllocation { payload, .. } => {
                ClientEvent::RecordAllocation(payload)
            }
            SamplerRequest::RecordDeallocation { payload, .. } => {
                ClientEvent::RecordDeallocation(payload)
            }
            SamplerRequest::SetProcessInfo { payload, .. } => ClientEvent::SetProcessInfo(payload),
            unknown_method => {
                log::debug!("Unknown, unhandled method: {:?}", unknown_method);
                ClientEvent::Ignored
            }
        }
    }
}

/// Build a profile from a stream of profiling requests and socket datagrams.
async fn process_sampler_requests(
    stream: SamplerRequestStream,
    tx: &mut mpsc::Sender<ProfileReport>,
) -> Result<ProfileReport, Error> {
    let mut profile_builder = ProfileBuilder::default();
    let mut time_of_last_profile = Instant::now();
    let mut request_index = 0;
    let mut is_enabled = true;
    let mut shared_socket: Option<zx::Socket> = None;

    let mut event_streams: SelectAll<
        futures::stream::BoxStream<'static, Result<ClientEvent, Error>>,
    > = SelectAll::new();

    let fidl_stream = stream
        .map(|res| match res {
            Ok(request) => Ok(ClientEvent::from(request)),
            Err(e) => Err(anyhow::Error::from(e).context("failed fidl request")),
        })
        .boxed();

    event_streams.push(fidl_stream);

    while let Some(event_res) = event_streams.next().await {
        let event = event_res?;
        if !is_enabled {
            continue;
        }

        match event {
            ClientEvent::SetSharedSocket(socket) => {
                // A client is expected to install its socket exactly once, but
                // do not depend on that: harvest any saturation signal latched
                // on the socket being replaced, since dropping its handle here
                // would lose the signal permanently.
                check_lossy_signal(&shared_socket, &mut profile_builder);
                shared_socket = match socket.duplicate_handle(zx::Rights::SAME_RIGHTS) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        log::warn!("Failed to duplicate shared socket handle: {:#}", e);
                        None
                    }
                };
                let socket_stream = fuchsia_async::Socket::from_socket(socket)
                    .into_datagram_stream()
                    .map(|res| match res {
                        Ok(datagram) => Ok(ClientEvent::from(&datagram[..])),
                        Err(e) => Err(anyhow::Error::from(e).context("failed socket read")),
                    })
                    .boxed();
                event_streams.push(socket_stream);
            }
            ClientEvent::RecordAllocation(RecordAllocationEvent {
                address,
                stack_trace,
                size,
                ..
            }) => {
                let address = address.ok_or_else(|| {
                    anyhow::anyhow!("Unsupported record allocation request: missing address")
                })?;
                let stack_frames = stack_trace
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Unsupported record allocation request: missing stack_trace"
                        )
                    })?
                    .stack_frames
                    .ok_or_else(|| {
                        anyhow::anyhow!("Unsupported stack trace: missing stack frames")
                    })?;
                let size = size.ok_or_else(|| {
                    anyhow::anyhow!("Unsupported record allocation request: missing size")
                })?;
                profile_builder.allocate(address, stack_frames, size);
            }
            ClientEvent::RecordDeallocation(RecordDeallocationEvent {
                address,
                stack_trace,
                ..
            }) => {
                let address = address.ok_or_else(|| {
                    anyhow::anyhow!("Unsupported deallocation request: missing address")
                })?;
                let stack_frames = stack_trace
                    .ok_or_else(|| {
                        anyhow::anyhow!("Unsupported deallocation request: missing stack trace")
                    })?
                    .stack_frames
                    .ok_or_else(|| {
                        anyhow::anyhow!("Unsupported stack_trace: missing stack frames")
                    })?;
                profile_builder.deallocate(address, stack_frames);
            }
            ClientEvent::SetProcessInfo(SamplerSetProcessInfoRequest {
                process_name,
                module_map,
                ..
            }) => {
                profile_builder.set_process_info(process_name, module_map.into_iter().flatten());
            }
            ClientEvent::PeerClosed => {
                log::info!("Socket connection closed by peer");
                break;
            }
            ClientEvent::Ignored => {}
        }

        request_index += 1;
        if let Some((_, _, new_time)) = maybe_emit_partial_profile(
            &mut profile_builder,
            tx,
            request_index,
            time_of_last_profile,
            &shared_socket,
        )
        .await?
        {
            time_of_last_profile = new_time;
        } else {
            is_enabled = false;
        }
    }

    check_lossy_signal(&shared_socket, &mut profile_builder);

    profile_builder.build()
}

/// Serves the `Sampler` protocol for a given process. Once a client
/// closes their connection, enqueues a `pprof`-compatible profile to
/// the provided channel, for further processing. May also regularly
/// enqueue partial profiles, to offload the profiler's memory usage.
///
/// Note: this function does not retry pushing profiles through the
/// channel. Failure to handle profile reports in a timely manner will
/// cause this component to shut down.
async fn run_sampler_service(
    stream: SamplerRequestStream,
    mut tx: mpsc::Sender<ProfileReport>,
) -> Result<(), Error> {
    let profile = process_sampler_requests(stream, &mut tx).await?;
    let process_name = profile.get_process_name().to_string();
    log::debug!("Profiling for {} done, queuing final report", process_name);
    {
        let res = tx.try_send(profile);
        if let Err(ref e) = res
            && e.is_full()
        {
            log::warn!("{}: Failed to send final profile, channel is full", process_name);
        };
    }
    Ok(())
}

enum IncomingServiceRequest {
    Sampler(SamplerRequestStream),
}

/// Returns a task that serves the `fuchsia.memory.sampler/Sampler`
/// protocol.
///
/// Note: any error will cause the sampler service to shutdown.
pub fn setup_sampler_service(
    tx: mpsc::Sender<ProfileReport>,
) -> Result<Task<Result<(), Error>>, Error> {
    let mut service_fs = ServiceFs::new();
    service_fs.dir("svc").add_fidl_service(IncomingServiceRequest::Sampler);
    service_fs.take_and_serve_directory_handle()?;
    Ok(Task::local(
        service_fs
            .map(Ok)
            .try_for_each_concurrent(
                MAX_CONCURRENT_REQUESTS,
                move |IncomingServiceRequest::Sampler(stream)| {
                    run_sampler_service(stream, tx.clone())
                },
            )
            .inspect_err(|e| log::error!("fuchsia.memory.sampler/Sampler protocol: {}", e)),
    ))
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_memory_sampler::{ExecutableSegment, ModuleMap, SamplerMarker, StackTrace};
    use futures::{StreamExt, join};
    use itertools::{assert_equal, sorted};
    use prost::Message;
    use zx::{Peered, Vmo};

    use crate::crash_reporter::ProfileReport;
    use crate::pprof::pproto::{Location, Mapping, Profile};
    use crate::sampler_service::{
        MAX_DURATION_BETWEEN_PARTIAL_PROFILES, ProfileBuilder,
        RECLAIMABLE_STACK_TRACES_PROFILE_THRESHOLD, maybe_emit_partial_profile,
        process_sampler_requests,
    };

    fn deserialize_profile(profile: Vmo, size: u64) -> Profile {
        Profile::decode(&profile.read_to_vec(0, size).unwrap()[..]).unwrap()
    }

    #[fuchsia::test]
    async fn test_process_sampler_requests_full_profile() -> Result<(), Error> {
        let (client, request_stream) = create_proxy_and_stream::<SamplerMarker>();
        let (mut tx, _rx) = mpsc::channel(1);
        let profile_future = process_sampler_requests(request_stream, &mut tx);

        let module_map = vec![
            ModuleMap {
                build_id: Some(vec![1, 2, 3, 4]),
                executable_segments: Some(vec![
                    ExecutableSegment {
                        start_address: Some(2000),
                        relative_address: Some(0),
                        size: Some(1000),
                        ..Default::default()
                    },
                    ExecutableSegment {
                        start_address: Some(4000),
                        relative_address: Some(2000),
                        size: Some(2000),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            ModuleMap {
                build_id: Some(vec![3, 4, 5, 6]),
                executable_segments: Some(vec![ExecutableSegment {
                    start_address: Some(3000),
                    relative_address: Some(1000),
                    size: Some(100),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ];
        let allocation_stack_trace =
            StackTrace { stack_frames: Some(vec![1000, 1500]), ..Default::default() };
        let deallocation_stack_trace =
            StackTrace { stack_frames: Some(vec![3000, 3001]), ..Default::default() };

        client.set_process_info(&SamplerSetProcessInfoRequest {
            process_name: Some("test process".to_string()),
            module_map: Some(module_map),
            ..Default::default()
        })?;
        client.record_allocation(&RecordAllocationEvent {
            address: Some(0x100),
            stack_trace: Some(allocation_stack_trace.clone()),
            size: Some(100),
            ..Default::default()
        })?;
        client.record_allocation(&RecordAllocationEvent {
            address: Some(0x200),
            stack_trace: Some(allocation_stack_trace),
            size: Some(1000),
            ..Default::default()
        })?;
        client.record_deallocation(&RecordDeallocationEvent {
            address: Some(0x100),
            stack_trace: Some(deallocation_stack_trace),
            ..Default::default()
        })?;
        drop(client);

        if let ProfileReport::Final { process_name, size, profile } = profile_future.await? {
            assert_eq!("test process", process_name);
            let profile = deserialize_profile(profile, size);
            assert_eq!(3, profile.mapping.len());
            assert_eq!(4, profile.location.len());
            assert_eq!(3, profile.sample.len());
        } else {
            panic!("Expected complete report, got partial report instead.");
        };

        Ok(())
    }

    #[fuchsia::test]
    async fn test_process_sampler_request_partial_profile_on_size() -> Result<(), Error> {
        let (_client, request_stream) = create_proxy_and_stream::<SamplerMarker>();
        let (mut tx, mut rx) = mpsc::channel(1);
        const TEST_NAME: &str = "test process";
        let mut builder = ProfileBuilder::default();

        // Pre-fill `builder` with a large number of unique dead
        // allocations, to trigger a partial profile on the next
        // request.
        builder.set_process_info(Some(TEST_NAME.to_string()), vec![].into_iter());
        {
            (0..RECLAIMABLE_STACK_TRACES_PROFILE_THRESHOLD as u64).for_each(|i| {
                builder.allocate(i, (i..i + 4).collect(), 10);
                builder.deallocate(i, (i..i + 4).collect());
            });
        }

        let stack_trace = StackTrace { stack_frames: Some(vec![1000, 1500]), ..Default::default() };
        const TEST_INDEX: usize = 42;
        builder.allocate(
            RECLAIMABLE_STACK_TRACES_PROFILE_THRESHOLD as u64,
            stack_trace.stack_frames.unwrap(),
            10,
        );
        let profile_future =
            maybe_emit_partial_profile(&mut builder, &mut tx, TEST_INDEX, Instant::now(), &None);
        let _ = request_stream; // Silence unused variable warning
        let (_, report) = join!(profile_future, rx.next());
        let report = report.unwrap();
        match report {
            ProfileReport::Partial { process_name, iteration, profile, size } => {
                assert_eq!(process_name, TEST_NAME.to_string());
                assert_eq!(iteration, TEST_INDEX);
                // This test assumes that every single allocation ends
                // up as a sample in the produced profile.
                let profile = deserialize_profile(profile, size);
                assert!(profile.sample.len() > RECLAIMABLE_STACK_TRACES_PROFILE_THRESHOLD);
            }
            _ => assert!(false),
        };
        Ok(())
    }

    #[fuchsia::test]
    async fn test_process_sampler_request_partial_profile_on_time() -> Result<(), Error> {
        let (_client, request_stream) = create_proxy_and_stream::<SamplerMarker>();
        let (mut tx, mut rx) = mpsc::channel(1);
        const TEST_NAME: &str = "test process";
        let mut builder = ProfileBuilder::default();
        builder.set_process_info(Some(TEST_NAME.to_string()), vec![].into_iter());

        let stack_trace = StackTrace { stack_frames: Some(vec![1000, 1500]), ..Default::default() };
        const TEST_INDEX: usize = 42;
        builder.allocate(1, stack_trace.stack_frames.unwrap(), 10);
        let profile_future = maybe_emit_partial_profile(
            &mut builder,
            &mut tx,
            TEST_INDEX,
            Instant::now() - MAX_DURATION_BETWEEN_PARTIAL_PROFILES,
            &None,
        );
        let _ = request_stream; // Silence unused variable warning
        let (_, report) = join!(profile_future, rx.next());
        let report = report.unwrap();
        match report {
            ProfileReport::Partial { process_name, iteration, profile, size } => {
                assert_eq!(process_name, TEST_NAME.to_string());
                assert_eq!(iteration, TEST_INDEX);
                // This test assumes that every single allocation ends
                // up as a sample in the produced profile.
                let profile = deserialize_profile(profile, size);
                assert_eq!(1, profile.sample.len());
            }
            _ => assert!(false),
        };
        Ok(())
    }

    #[fuchsia::test]
    async fn test_process_sampler_requests_set_process_info() -> Result<(), Error> {
        let (client, request_stream) = create_proxy_and_stream::<SamplerMarker>();
        let (mut tx, _rx) = mpsc::channel(1);
        let profile_future = process_sampler_requests(request_stream, &mut tx);

        let module_map = vec![
            ModuleMap {
                build_id: Some(vec![1, 2, 3, 4]),
                executable_segments: Some(vec![
                    ExecutableSegment {
                        start_address: Some(2000),
                        relative_address: Some(0),
                        size: Some(1000),
                        ..Default::default()
                    },
                    ExecutableSegment {
                        start_address: Some(4000),
                        relative_address: Some(2000),
                        size: Some(2000),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            ModuleMap {
                build_id: Some(vec![3, 4, 5, 6]),
                executable_segments: Some(vec![ExecutableSegment {
                    start_address: Some(3000),
                    relative_address: Some(1000),
                    size: Some(100),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ];

        client.set_process_info(&SamplerSetProcessInfoRequest {
            process_name: Some("test process".to_string()),
            module_map: Some(module_map),
            ..Default::default()
        })?;
        drop(client);

        if let ProfileReport::Final { process_name, profile, size } = profile_future.await? {
            assert_eq!("test process", process_name);
            let profile = deserialize_profile(profile, size);
            assert_eq!(3, profile.mapping.len());
            assert_eq!(Vec::<Location>::new(), profile.location);
        } else {
            panic!("Expected complete report, got partial report instead.");
        };
        Ok(())
    }

    #[fuchsia::test]
    async fn test_process_sampler_requests_multiple_set_process_info() -> Result<(), Error> {
        let (client, request_stream) = create_proxy_and_stream::<SamplerMarker>();
        let (mut tx, _rx) = mpsc::channel(1);
        let profile_future = process_sampler_requests(request_stream, &mut tx);

        let module_map = vec![
            ModuleMap {
                build_id: Some(vec![1, 2, 3, 4]),
                executable_segments: Some(vec![
                    ExecutableSegment {
                        start_address: Some(2000),
                        relative_address: Some(0),
                        size: Some(1000),
                        ..Default::default()
                    },
                    ExecutableSegment {
                        start_address: Some(4000),
                        relative_address: Some(2000),
                        size: Some(2000),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            ModuleMap {
                build_id: Some(vec![3, 4, 5, 6]),
                executable_segments: Some(vec![ExecutableSegment {
                    start_address: Some(3000),
                    relative_address: Some(1000),
                    size: Some(100),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ];

        client.set_process_info(&SamplerSetProcessInfoRequest {
            process_name: Some("test process".to_string()),
            module_map: Some(module_map.clone()),
            ..Default::default()
        })?;
        client.set_process_info(&SamplerSetProcessInfoRequest {
            process_name: Some("other test process".to_string()),
            module_map: Some(module_map),
            ..Default::default()
        })?;
        drop(client);

        if let ProfileReport::Final { process_name, profile, size } = profile_future.await? {
            assert_eq!("other test process", process_name);
            let profile = deserialize_profile(profile, size);
            assert_eq!(6, profile.mapping.len());
            assert_eq!(Vec::<Location>::new(), profile.location);
        } else {
            panic!("Expected complete report, got partial report instead.");
        };

        Ok(())
    }

    #[fuchsia::test]
    async fn test_process_sampler_requests_allocate() -> Result<(), Error> {
        let (client, request_stream) = create_proxy_and_stream::<SamplerMarker>();
        let (mut tx, _rx) = mpsc::channel(1);
        let profile_future = process_sampler_requests(request_stream, &mut tx);

        let allocation_stack_trace =
            StackTrace { stack_frames: Some(vec![1000, 1500]), ..Default::default() };

        client.record_allocation(&RecordAllocationEvent {
            address: Some(0x100),
            stack_trace: Some(allocation_stack_trace),
            size: Some(100),
            ..Default::default()
        })?;
        drop(client);

        if let ProfileReport::Final { process_name, profile, size } = profile_future.await? {
            assert_eq!(String::default(), process_name);
            let profile = deserialize_profile(profile, size);
            let locations = profile.location.into_iter().map(|Location { address, .. }| address);
            assert_equal(vec![1000, 1500].into_iter(), sorted(locations));
        } else {
            panic!("Expected complete report, got partial report instead.");
        };
        Ok(())
    }

    #[fuchsia::test]
    async fn test_process_sampler_requests_deallocate() -> Result<(), Error> {
        let (client, request_stream) = create_proxy_and_stream::<SamplerMarker>();
        let (mut tx, _rx) = mpsc::channel(1);
        let profile_future = process_sampler_requests(request_stream, &mut tx);

        let stack_trace = StackTrace { stack_frames: Some(vec![3000, 3001]), ..Default::default() };

        client.record_deallocation(&RecordDeallocationEvent {
            address: Some(0x100),
            stack_trace: Some(stack_trace),
            ..Default::default()
        })?;
        drop(client);

        if let ProfileReport::Final { process_name, profile, size } = profile_future.await? {
            assert_eq!(String::default(), process_name);
            let profile = deserialize_profile(profile, size);
            assert_eq!(Vec::<Mapping>::new(), profile.mapping);
            assert_eq!(Vec::<Location>::new(), profile.location);
        } else {
            panic!("Expected complete report, got partial report instead.");
        };

        Ok(())
    }

    #[fuchsia::test]
    async fn test_process_sampler_requests_shared_socket() -> Result<(), Error> {
        let (client, request_stream) = create_proxy_and_stream::<SamplerMarker>();
        let (mut tx, _rx) = mpsc::channel(1);
        let profile_future = process_sampler_requests(request_stream, &mut tx);

        let (client_sock, server_sock) = zx::Socket::create_datagram();
        client.set_shared_socket(server_sock)?;

        client.set_process_info(&SamplerSetProcessInfoRequest {
            process_name: Some("socket test process".to_string()),
            module_map: Some(vec![]),
            ..Default::default()
        })?;

        // Send an allocation record over the socket datagram
        let alloc =
            fidl_fuchsia_memory_sampler::SamplerDatagram::RecordAllocation(RecordAllocationEvent {
                address: Some(0x100),
                size: Some(200),
                stack_trace: Some(StackTrace {
                    stack_frames: Some(vec![1000, 1500]),
                    ..Default::default()
                }),
                ..Default::default()
            });
        let alloc_bytes = fidl::persist(&alloc)?;
        client_sock.write(&alloc_bytes)?;

        // Send a deallocation record over the socket datagram
        let dealloc = fidl_fuchsia_memory_sampler::SamplerDatagram::RecordDeallocation(
            RecordDeallocationEvent {
                address: Some(0x100),
                stack_trace: Some(StackTrace {
                    stack_frames: Some(vec![3000]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let dealloc_bytes = fidl::persist(&dealloc)?;
        client_sock.write(&dealloc_bytes)?;

        drop(client_sock);
        drop(client);

        if let ProfileReport::Final { process_name, profile, size } = profile_future.await? {
            assert_eq!("socket test process", process_name);
            let profile = deserialize_profile(profile, size);
            let locations = profile.location.into_iter().map(|Location { address, .. }| address);
            assert_equal(vec![1000, 1500, 3000].into_iter(), sorted(locations));
        } else {
            panic!("Expected complete report, got partial report instead.");
        };

        Ok(())
    }

    #[fuchsia::test]
    async fn test_process_sampler_requests_shared_socket_lossy() -> Result<(), Error> {
        let (client, request_stream) = create_proxy_and_stream::<SamplerMarker>();
        let (mut tx, _rx) = mpsc::channel(1);
        let profile_future = process_sampler_requests(request_stream, &mut tx);

        let (client_sock, server_sock) = zx::Socket::create_datagram();
        client.set_shared_socket(server_sock)?;

        client.set_process_info(&SamplerSetProcessInfoRequest {
            process_name: Some("socket lossy process".to_string()),
            module_map: Some(vec![]),
            ..Default::default()
        })?;

        // Signal USER_0 from the client socket to indicate buffer saturation
        client_sock.signal_peer(zx::Signals::NONE, zx::Signals::USER_0)?;

        drop(client_sock);
        drop(client);

        if let ProfileReport::Final { process_name, profile, size } = profile_future.await? {
            assert_eq!("socket lossy process [TAINTED: BUFFER OVERFLOW]", process_name);
            let profile = deserialize_profile(profile, size);
            assert_eq!(1, profile.comment.len());
            let comment = &profile.string_table[profile.comment[0] as usize];
            assert!(comment.contains("WARNING: Socket buffer saturated"));
        } else {
            panic!("Expected complete report, got partial report instead.");
        };

        Ok(())
    }
}
