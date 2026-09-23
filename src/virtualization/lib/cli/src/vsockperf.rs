// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::platform::PlatformServices;
use anyhow::{Error, anyhow};
use fidl::endpoints::{create_proxy, create_request_stream};
use fidl_fuchsia_virtualization::{
    GuestManagerProxy, GuestMarker, GuestStatus, HostVsockAcceptorMarker, HostVsockEndpointMarker,
};
use fuchsia_async as fasync;
use futures::{AsyncReadExt, AsyncWriteExt, FutureExt, TryStreamExt, select, try_join};
use guest_cli_args as arguments;
use prettytable::format::consts::FORMAT_CLEAN;
use prettytable::{Table, row};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Write;

const LATENCY_CHECK_SIZE_BYTES: usize = 4096;
const THROUGHPUT_SIZE_MEBIBYTES: usize = 128;
const THROUGHPUT_SIZE_BYTES: usize = (1 << 20) * THROUGHPUT_SIZE_MEBIBYTES;

const HOST_PORT: u32 = 8500;
const CONTROL_STREAM: u32 = 8501;
const LATENCY_CHECK_STREAM: u32 = 8502;

const SINGLE_STREAM_THROUGHPUT: u32 = 8503;
const SINGLE_STREAM_MAGIC_NUM: u8 = 123;

const MULTI_STREAM_THROUGHPUT1: u32 = 8504;
const MULTI_STREAM_MAGIC_NUM1: u8 = 124;
const MULTI_STREAM_THROUGHPUT2: u32 = 8505;
const MULTI_STREAM_MAGIC_NUM2: u8 = 125;
const MULTI_STREAM_THROUGHPUT3: u32 = 8506;
const MULTI_STREAM_MAGIC_NUM3: u8 = 126;
const MULTI_STREAM_THROUGHPUT4: u32 = 8507;
const MULTI_STREAM_MAGIC_NUM4: u8 = 127;
const MULTI_STREAM_THROUGHPUT5: u32 = 8508;
const MULTI_STREAM_MAGIC_NUM5: u8 = 128;

const SINGLE_STREAM_BIDIRECTIONAL: u32 = 8509;
#[allow(dead_code)]
const SINGLE_STREAM_BIDIRECTIONAL_MAGIC_NUM: u8 = 129;

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
enum PercentileUnit {
    Nanoseconds,
    MebibytesPerSecond,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Percentiles {
    min: f64,
    p_25th: f64,
    p_50th: f64,
    p_75th: f64,
    p_99th: f64,
    max: f64,
    unit: PercentileUnit,
}

impl fmt::Display for Percentiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let get_units = |val: f64, unit: PercentileUnit| -> String {
            match unit {
                PercentileUnit::Nanoseconds => {
                    format!("{}ns ({:.3}ms)", val as u64, val / 1_000_000.0)
                }
                PercentileUnit::MebibytesPerSecond => {
                    format!("{:.2}MiB/s", val)
                }
            }
        };

        let mut table = Table::new();
        table.set_format(*FORMAT_CLEAN);

        table.add_row(row!["\tMin:", get_units(self.min, self.unit)]);
        table.add_row(row!["\t25th percentile:", get_units(self.p_25th, self.unit)]);
        table.add_row(row!["\t50th percentile:", get_units(self.p_50th, self.unit)]);
        table.add_row(row!["\t75th percentile:", get_units(self.p_75th, self.unit)]);
        table.add_row(row!["\t99th percentile:", get_units(self.p_99th, self.unit)]);
        table.add_row(row!["\tMax:", get_units(self.max, self.unit)]);

        write!(f, "\n{}", table)
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Measurements {
    data_corruption: Option<bool>,
    round_trip_page: Option<Percentiles>,
    tx_throughput: Option<Percentiles>,
    rx_throughput: Option<Percentiles>,
    single_stream_unidirectional: Option<Percentiles>,
    multi_stream_unidirectional: Option<Percentiles>,
}

impl fmt::Display for Measurements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let format_percentiles = |percentiles: &Option<Percentiles>| -> String {
            match percentiles {
                None => " NOT RUN".to_owned(),
                Some(percentile) => percentile.to_string(),
            }
        };

        writeln!(f, "\n\nMicrobenchmark Results\n------------------------")?;

        writeln!(
            f,
            "* Data corruption check: {}",
            match self.data_corruption {
                None => "NOT RUN",
                Some(result) =>
                    if result {
                        "PASSED"
                    } else {
                        "FAILED"
                    },
            }
        )?;

        writeln!(
            f,
            "* Round trip latency of {LATENCY_CHECK_SIZE_BYTES} bytes:{}",
            format_percentiles(&self.round_trip_page)
        )?;
        writeln!(
            f,
            "* TX (guest -> host, unreliable) throughput of {THROUGHPUT_SIZE_MEBIBYTES} MiB:{}",
            format_percentiles(&self.tx_throughput)
        )?;
        writeln!(
            f,
            "* RX (host -> guest, unreliable) throughput of {THROUGHPUT_SIZE_MEBIBYTES} MiB:{}",
            format_percentiles(&self.rx_throughput)
        )?;
        writeln!(
            f,
            "* Single stream unidirectional round trip throughput of {THROUGHPUT_SIZE_MEBIBYTES} MiB:{}",
            format_percentiles(&self.single_stream_unidirectional)
        )?;
        writeln!(
            f,
            "* Multistream (5 connections) unidirectional round trip throughput of {THROUGHPUT_SIZE_MEBIBYTES} MiB:{}",
            format_percentiles(&self.multi_stream_unidirectional)
        )
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum VsockPerfResult {
    BenchmarkComplete(Box<Measurements>),
    UnsupportedGuest(arguments::GuestType),
    Internal(String),
}

impl fmt::Display for VsockPerfResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VsockPerfResult::BenchmarkComplete(result) => write!(f, "{}", result),
            VsockPerfResult::UnsupportedGuest(guest) => {
                write!(f, "VsockPerf is not supported for '{}'. Only 'debian' is supported", guest)
            }
            VsockPerfResult::Internal(context) => {
                write!(f, "Internal error: {}", context)
            }
        }
    }
}

fn get_time_delta_nanos(before: fasync::MonotonicInstant, after: fasync::MonotonicInstant) -> i64 {
    #[cfg(target_os = "fuchsia")]
    {
        (after - before).into_nanos()
    }

    #[cfg(not(target_os = "fuchsia"))]
    {
        (after - before).as_nanos().try_into().unwrap()
    }
}

pub async fn handle_vsockperf<P: PlatformServices>(
    services: &P,
    args: &arguments::vsockperf_args::VsockPerfArgs,
) -> Result<VsockPerfResult, Error> {
    if args.guest_type != arguments::GuestType::Debian {
        return Ok(VsockPerfResult::UnsupportedGuest(args.guest_type));
    }

    let guest_manager = services.connect_to_manager(args.guest_type).await?;
    Ok(match run_micro_benchmark(guest_manager).await {
        Err(err) => VsockPerfResult::Internal(format!("{}", err)),
        Ok(result) => VsockPerfResult::BenchmarkComplete(Box::new(result)),
    })
}

fn percentile(durations: &[u64], percentile: u8) -> u64 {
    assert!(percentile <= 100 && !durations.is_empty());
    // Don't bother interpolating between two points if this isn't a whole number, just floor it.
    let location = (((percentile as f64) / 100.0) * ((durations.len() - 1) as f64)) as usize;
    durations[location]
}

fn latency_percentile(durations: &[u64]) -> Percentiles {
    Percentiles {
        min: percentile(&durations, 0) as f64,
        p_25th: percentile(&durations, 25) as f64,
        p_50th: percentile(&durations, 50) as f64,
        p_75th: percentile(&durations, 75) as f64,
        p_99th: percentile(&durations, 99) as f64,
        max: percentile(&durations, 100) as f64,
        unit: PercentileUnit::Nanoseconds,
    }
}

fn throughput_percentile(durations: &[u64], bytes: usize) -> Percentiles {
    let to_mebibytes_per_second = |nanos: u64| -> f64 {
        let seconds = nanos as f64 / (1000.0 * 1000.0 * 1000.0);
        let bytes_per_second = (bytes as f64) / seconds;
        bytes_per_second / (1 << 20) as f64
    };

    Percentiles {
        min: to_mebibytes_per_second(percentile(&durations, 0)),
        p_25th: to_mebibytes_per_second(percentile(&durations, 25)),
        p_50th: to_mebibytes_per_second(percentile(&durations, 50)),
        p_75th: to_mebibytes_per_second(percentile(&durations, 75)),
        p_99th: to_mebibytes_per_second(percentile(&durations, 99)),
        max: to_mebibytes_per_second(percentile(&durations, 100)),
        unit: PercentileUnit::MebibytesPerSecond,
    }
}

async fn warmup_and_data_corruption_check(socket: &mut fasync::Socket) -> Result<bool, Error> {
    // Send and receive 100 messages, checking for a known but changing pattern.
    let mut buffer = vec![0u8; LATENCY_CHECK_SIZE_BYTES];
    for i in 0..100 {
        let pattern = format!("DAVID{:0>3}", i).repeat(512);
        let packet = pattern.as_bytes();
        assert_eq!(packet.len(), buffer.len());

        if packet.len() != socket.as_ref().write(&packet)? {
            return Err(anyhow!("failed to write full packet"));
        }

        let timeout =
            fasync::MonotonicInstant::now() + std::time::Duration::from_millis(100).into();
        select! {
            () = fasync::Timer::new(timeout).fuse() => {
                return Err(anyhow!("warmup timed out waiting 100ms for a packet echoed"));
            }
            result = socket.read_exact(&mut buffer).fuse() => {
                result.map_err(|err| anyhow!("failed to read from socket during warmup: {}", err))?;
            }
        }

        if buffer != packet {
            return Ok(false);
        }
    }

    Ok(true)
}

// Get the magic numbers for a test case from the guest to know that it's ready.
async fn wait_for_magic_numbers(
    mut numbers: HashSet<u8>,
    control_socket: &mut fasync::Socket,
) -> Result<(), Error> {
    let timeout = fasync::MonotonicInstant::now() + std::time::Duration::from_secs(5).into();
    let mut magic_buf = [0u8];
    loop {
        select! {
            () = fasync::Timer::new(timeout).fuse() => {
                return Err(anyhow!("timeout waiting 5s to get the test ready"));
            }
            result = control_socket.read_exact(&mut magic_buf).fuse() => {
                result.map_err(|err| anyhow!("failed to read magic value from socket: {}", err))?;
                match numbers.contains(&magic_buf[0]) {
                    false => Err(anyhow!("unexpected magic number from guest: {}", magic_buf[0])),
                    true => {
                        numbers.remove(&magic_buf[0]);
                        Ok(())
                    }
                }?;

                if numbers.is_empty() {
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn read_single_stream(
    total_size: usize,
    socket: &mut fasync::Socket,
) -> Result<fasync::MonotonicInstant, Error> {
    let timeout = fasync::MonotonicInstant::now() + std::time::Duration::from_secs(10).into();
    let mut buffer = vec![0u8; LATENCY_CHECK_SIZE_BYTES]; // 4 KiB
    let segments = total_size / buffer.len();

    for _ in 0..segments {
        select! {
            () = fasync::Timer::new(timeout).fuse() => {
                return Err(anyhow!("timeout waiting 10s for test iteration read to finish"));
            }
            result = socket.read_exact(&mut buffer).fuse() => {
                result.map_err(|err| anyhow!("failed to read segment from socket: {}", err))?;
            }
        }
    }

    Ok(fasync::MonotonicInstant::now())
}

async fn write_single_stream(
    total_size: usize,
    socket: &mut fasync::Socket,
) -> Result<fasync::MonotonicInstant, Error> {
    let timeout = fasync::MonotonicInstant::now() + std::time::Duration::from_secs(10).into();
    let buffer = vec![0u8; LATENCY_CHECK_SIZE_BYTES]; // 4 KiB
    let segments = total_size / buffer.len();

    for _ in 0..segments {
        select! {
            () = fasync::Timer::new(timeout).fuse() => {
                return Err(anyhow!("timeout waiting 10s for test iteration write to finish"));
            }
            result = socket.write_all(&buffer).fuse() => {
                result.map_err(
                    |err| anyhow!("failed to write segment to socket: {}", err))?;
            }
        }
    }

    Ok(fasync::MonotonicInstant::now())
}

async fn write_read_high_throughput(
    total_size: usize,
    socket: &mut fasync::Socket,
) -> Result<(), Error> {
    // This is intentionally sequential to measure roundtrip throughput from the perspective of
    // the host.
    write_single_stream(total_size, socket).await?;
    read_single_stream(total_size, socket).await?;
    Ok(())
}

#[cfg(target_os = "fuchsia")]
async fn run_single_stream_bidirectional_test(
    mut read_socket: fasync::Socket,
    control_socket: &mut fasync::Socket,
    measurements: &mut Measurements,
) -> Result<(), Error> {
    println!("Starting single stream bidirectional round trip throughput test...");

    let mut write_socket = fasync::Socket::from_socket(
        read_socket.as_ref().duplicate_handle(fidl::Rights::SAME_RIGHTS)?,
    );

    wait_for_magic_numbers(HashSet::from([SINGLE_STREAM_BIDIRECTIONAL_MAGIC_NUM]), control_socket)
        .await?;

    let total_size = THROUGHPUT_SIZE_BYTES;
    let mut rx_durations: Vec<u64> = Vec::new();
    let mut tx_durations: Vec<u64> = Vec::new();

    for i in 0..100 {
        let before = fasync::MonotonicInstant::now();

        let (write_finish, read_finish) = try_join!(
            write_single_stream(total_size, &mut write_socket),
            read_single_stream(total_size, &mut read_socket)
        )?;

        rx_durations.push(
            get_time_delta_nanos(before, write_finish)
                .try_into()
                .expect("durations measured by the same thread must be greater than zero"),
        );

        tx_durations.push(
            get_time_delta_nanos(before, read_finish)
                .try_into()
                .expect("durations measured by the same thread must be greater than zero"),
        );

        print!("\rFinished {} bidirectional throughput measurements", i + 1);
        std::io::stdout().flush().expect("failed to flush stdout");
    }

    rx_durations.sort();
    rx_durations.reverse();

    tx_durations.sort();
    tx_durations.reverse();

    assert_eq!(rx_durations.len(), tx_durations.len());
    println!("\rFinished {} bidirectional throughput measurements", rx_durations.len());

    measurements.tx_throughput = Some(throughput_percentile(&tx_durations, total_size));
    measurements.rx_throughput = Some(throughput_percentile(&rx_durations, total_size));

    Ok(())
}

async fn run_single_stream_unidirectional_round_trip_test(
    mut data_socket: fasync::Socket,
    control_socket: &mut fasync::Socket,
    measurements: &mut Measurements,
) -> Result<(), Error> {
    println!("Starting single stream unidirectional round trip throughput test...");

    wait_for_magic_numbers(HashSet::from([SINGLE_STREAM_MAGIC_NUM]), control_socket).await?;

    let total_size = THROUGHPUT_SIZE_BYTES;
    let mut durations: Vec<u64> = Vec::new();

    for i in 0..100 {
        let before = fasync::MonotonicInstant::now();

        write_read_high_throughput(total_size, &mut data_socket).await?;

        let after = fasync::MonotonicInstant::now();
        durations.push(
            get_time_delta_nanos(before, after)
                .try_into()
                .expect("durations measured by the same thread must be greater than zero"),
        );

        print!("\rFinished {} round trip throughput measurements", i + 1);
        std::io::stdout().flush().expect("failed to flush stdout");
    }

    durations.sort();
    durations.reverse();
    println!("\rFinished {} single stream round trip throughput measurements", durations.len());

    measurements.single_stream_unidirectional =
        Some(throughput_percentile(&durations, total_size * 2));

    Ok(())
}

async fn run_multi_stream_unidirectional_round_trip_test(
    mut data_socket1: fasync::Socket,
    mut data_socket2: fasync::Socket,
    mut data_socket3: fasync::Socket,
    mut data_socket4: fasync::Socket,
    mut data_socket5: fasync::Socket,
    control_socket: &mut fasync::Socket,
    measurements: &mut Measurements,
) -> Result<(), Error> {
    println!("Starting multistream unidirectional round trip throughput test...");

    wait_for_magic_numbers(
        HashSet::from([
            MULTI_STREAM_MAGIC_NUM1,
            MULTI_STREAM_MAGIC_NUM2,
            MULTI_STREAM_MAGIC_NUM3,
            MULTI_STREAM_MAGIC_NUM4,
            MULTI_STREAM_MAGIC_NUM5,
        ]),
        control_socket,
    )
    .await?;

    let total_size = THROUGHPUT_SIZE_BYTES;
    let mut durations: Vec<u64> = Vec::new();

    for i in 0..50 {
        let before = fasync::MonotonicInstant::now();

        try_join!(
            write_read_high_throughput(total_size, &mut data_socket1),
            write_read_high_throughput(total_size, &mut data_socket2),
            write_read_high_throughput(total_size, &mut data_socket3),
            write_read_high_throughput(total_size, &mut data_socket4),
            write_read_high_throughput(total_size, &mut data_socket5)
        )?;

        let after = fasync::MonotonicInstant::now();
        durations.push(
            get_time_delta_nanos(before, after)
                .try_into()
                .expect("durations measured by the same thread must be greater than zero"),
        );

        print!("\rFinished {} multistream round trip throughput measurements", i + 1);
        std::io::stdout().flush().expect("failed to flush stdout");
    }

    durations.sort();
    durations.reverse();
    println!("\rFinished {} multistream round trip throughput measurements", durations.len());

    measurements.multi_stream_unidirectional =
        Some(throughput_percentile(&durations, total_size * 2));

    Ok(())
}

async fn run_latency_test(
    mut socket: fasync::Socket,
    measurements: &mut Measurements,
) -> Result<(), Error> {
    println!("Checking for data corruption...");
    measurements.data_corruption = Some(warmup_and_data_corruption_check(&mut socket).await?);
    println!("Finished data corruption check");

    let packet = [42u8; LATENCY_CHECK_SIZE_BYTES];
    let mut buffer = vec![0u8; packet.len()];
    let mut latencies: Vec<u64> = Vec::new();

    println!("Starting latency test...");
    for i in 0..10000 {
        let before = fasync::MonotonicInstant::now();
        let timeout = before + std::time::Duration::from_millis(100).into();

        if packet.len() != socket.as_ref().write(&packet)? {
            return Err(anyhow!("failed to write full packet"));
        }

        select! {
            () = fasync::Timer::new(timeout).fuse() => {
                return Err(anyhow!("latency test timed out waiting 100ms for a packet echoed"));
            }
            result = socket.read_exact(&mut buffer).fuse() => {
                result.map_err(
                    |err| anyhow!("failed to read from socket during latency test: {}", err))?;
            }
        }

        let after = fasync::MonotonicInstant::now();
        latencies.push(
            get_time_delta_nanos(before, after)
                .try_into()
                .expect("durations measured by the same thread must be greater than zero"),
        );

        if (i + 1) % 50 == 0 {
            print!("\rFinished measuring round trip latency for {} packets", i + 1);
            std::io::stdout().flush().expect("failed to flush stdout");
        }
    }

    latencies.sort();
    println!("\rFinished measuring round trip latency for {} packets", latencies.len());

    measurements.round_trip_page = Some(latency_percentile(&latencies));

    Ok(())
}

async fn run_micro_benchmark(guest_manager: GuestManagerProxy) -> Result<Measurements, Error> {
    let guest_info = guest_manager.get_info().await?;
    if guest_info.guest_status.unwrap() != GuestStatus::Running {
        return Err(anyhow!(zx_status::Status::NOT_CONNECTED));
    }

    let (guest_endpoint, guest_server_end) = create_proxy::<GuestMarker>();
    guest_manager
        .connect(guest_server_end)
        .await
        .map_err(|err| anyhow!("failed to get a connect response: {}", err))?
        .map_err(|err| anyhow!("connect failed with: {:?}", err))?;

    let (vsock_endpoint, vsock_server_end) = create_proxy::<HostVsockEndpointMarker>();
    guest_endpoint
        .get_host_vsock_endpoint(vsock_server_end)
        .await?
        .map_err(|err| anyhow!("failed to get HostVsockEndpoint: {:?}", err))?;

    let (acceptor, mut client_stream) = create_request_stream::<HostVsockAcceptorMarker>();
    vsock_endpoint
        .listen(HOST_PORT, acceptor)
        .await
        .map_err(|err| anyhow!("failed to get a listen response: {}", err))?
        .map_err(|err| anyhow!("listen failed with: {}", zx_status::Status::err_from_raw(err)))?;

    let socket = guest_endpoint
        .get_console()
        .await
        .map_err(|err| anyhow!("failed to get a get_console response: {}", err))?
        .map_err(|err| anyhow!("get_console failed with: {:?}", err))?;

    // Start the micro benchmark utility on the guest which will begin by opening the necessary
    // connections.
    let command = b"../test_utils/virtio_vsock_test_util micro_benchmark\n";
    let bytes_written = socket
        .write(command)
        .map_err(|err| anyhow!("failed to write command to socket: {}", err))?;
    if bytes_written != command.len() {
        return Err(anyhow!(
            "attempted to send command '{}', but only managed to write '{}'",
            std::str::from_utf8(command).expect("failed to parse as utf-8"),
            std::str::from_utf8(&command[0..bytes_written]).expect("failed to parse as utf-8")
        ));
    }

    let mut expected_connections = HashSet::from([
        CONTROL_STREAM,
        LATENCY_CHECK_STREAM,
        SINGLE_STREAM_THROUGHPUT,
        MULTI_STREAM_THROUGHPUT1,
        MULTI_STREAM_THROUGHPUT2,
        MULTI_STREAM_THROUGHPUT3,
        MULTI_STREAM_THROUGHPUT4,
        MULTI_STREAM_THROUGHPUT5,
        SINGLE_STREAM_BIDIRECTIONAL,
    ]);
    let mut active_connections = HashMap::new();

    // Give the utility 15s to open all the expected connections.
    let timeout = fasync::MonotonicInstant::now() + std::time::Duration::from_secs(15).into();
    loop {
        select! {
            () = fasync::Timer::new(timeout).fuse() => {
                return Err(anyhow!("vsockperf timed out waiting 15s for vsock connections"));
            }
            request = client_stream.try_next() => {
                let request = request
                    .map_err(|err| anyhow!("failed to get acceptor request: {}", err))?
                    .ok_or_else(|| anyhow!("unexpected end of Listener stream"))?;
                let (_src_cid, src_port, _port, responder) = request
                    .into_accept().ok_or_else(|| anyhow!("failed to parse message as Accept"))?;

                match expected_connections.contains(&src_port) {
                    false => Err(anyhow!("unexpected connection from guest port: {}", src_port)),
                    true => {
                        expected_connections.remove(&src_port);
                        Ok(())
                    }
                }?;

                let (client_socket, device_socket) = fidl::Socket::create_stream();
                let client_socket = fasync::Socket::from_socket(client_socket);

                responder.send(Ok(device_socket))
                    .map_err(|err| anyhow!("failed to send response to device: {}", err))?;

                if let Some(_) = active_connections.insert(src_port, client_socket) {
                    panic!("Connections must be unique");
                }

                if expected_connections.is_empty() {
                    break;
                }
            }
        }
    }

    let mut measurements = Measurements::default();

    run_latency_test(
        active_connections.remove(&LATENCY_CHECK_STREAM).expect("socket should exist"),
        &mut measurements,
    )
    .await?;

    // TODO(https://fxbug.dev/42068091): Re-enable when overnet supports duplicated socket handles.
    #[cfg(target_os = "fuchsia")]
    run_single_stream_bidirectional_test(
        active_connections.remove(&SINGLE_STREAM_BIDIRECTIONAL).expect("socket should exist"),
        active_connections.get_mut(&CONTROL_STREAM).expect("socket should exist"),
        &mut measurements,
    )
    .await?;

    run_single_stream_unidirectional_round_trip_test(
        active_connections.remove(&SINGLE_STREAM_THROUGHPUT).expect("socket should exist"),
        active_connections.get_mut(&CONTROL_STREAM).expect("socket should exist"),
        &mut measurements,
    )
    .await?;

    run_multi_stream_unidirectional_round_trip_test(
        active_connections.remove(&MULTI_STREAM_THROUGHPUT1).expect("socket should exist"),
        active_connections.remove(&MULTI_STREAM_THROUGHPUT2).expect("socket should exist"),
        active_connections.remove(&MULTI_STREAM_THROUGHPUT3).expect("socket should exist"),
        active_connections.remove(&MULTI_STREAM_THROUGHPUT4).expect("socket should exist"),
        active_connections.remove(&MULTI_STREAM_THROUGHPUT5).expect("socket should exist"),
        active_connections.get_mut(&CONTROL_STREAM).expect("socket should exist"),
        &mut measurements,
    )
    .await?;

    return Ok(measurements);
}
