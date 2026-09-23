// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use argh::FromArgs as _;
use fuchsia_async as fasync;
use netemul::TestSandbox;
use netstack_testing_common::realms::{Netstack3, TestSandboxExt as _};
use test_case::test_case;

use netstack_testing_common::pcap as pcap_helper;

struct SharedBuffer {
    buffer: Arc<Mutex<Option<Vec<u8>>>>,
}

impl std::io::Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut guard = self.buffer.lock().unwrap();
        guard.as_mut().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct CaptureTestDeps {
    output_buffer: Arc<Mutex<Option<Vec<u8>>>>,
    written_path: Arc<Mutex<Option<PathBuf>>>,
}

impl net_cli::CaptureDeps for CaptureTestDeps {
    type OutputWriter = SharedBuffer;

    fn create_output_writer(
        &self,
        path: &std::path::Path,
    ) -> Result<Self::OutputWriter, anyhow::Error> {
        let mut guard = self.output_buffer.lock().unwrap();
        if guard.is_some() {
            panic!("create_output_writer called more than once");
        }
        *guard = Some(Vec::new());
        *self.written_path.lock().unwrap() = Some(path.to_path_buf());
        Ok(SharedBuffer { buffer: self.output_buffer.clone() })
    }
}

struct TestParams {
    start_name: Option<&'static str>,
    stop_name: Option<&'static str>,
    interface: &'static str,
    output: Option<&'static str>,
    skip_download: bool,
    expected_stop_error: Option<&'static str>,
    payloads: Vec<Vec<u8>>,
}

async fn run_lifecycle_test(params: TestParams) {
    let TestParams {
        start_name,
        stop_name,
        interface,
        output,
        skip_download,
        expected_stop_error,
        payloads,
    } = params;

    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox
        .create_netstack_realm::<Netstack3, _>("net-cli-capture-test")
        .expect("failed to create realm");

    let connector = crate::TestRealmConnector { realm: &realm };
    let deps = CaptureTestDeps {
        output_buffer: Arc::new(Mutex::new(None)),
        written_path: Arc::new(Mutex::new(None)),
    };

    // 1. Start the rolling capture
    let start_args = ["capture", "start-rolling"]
        .into_iter()
        .chain(start_name.into_iter().flat_map(|name| ["--name", name]))
        .chain(std::iter::once(interface))
        .collect::<Vec<_>>();

    let start_cmd =
        net_cli::Command::from_args(&["net"], &start_args).expect("failed to parse start command");
    let buffers = writer::TestBuffers::default();
    net_cli::do_root(writer::JsonWriter::new_test(None, &buffers), start_cmd, &connector, &deps)
        .await
        .expect("start capture failed");

    // Generate some traffic to capture.
    const PORT: u16 = 12345;
    let bind_addr =
        std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), PORT);
    for payload in &payloads {
        pcap_helper::send_udp_to_self_and_recv(&realm, bind_addr, payload).await;
    }

    // 2. Stop the rolling capture
    let stop_args = ["capture", "stop-rolling"]
        .into_iter()
        .chain(stop_name.into_iter().flat_map(|name| ["--name", name]))
        .chain(output.into_iter().flat_map(|out| ["--output", out]))
        .chain(skip_download.then_some("--skip-download"))
        .collect::<Vec<_>>();

    let stop_cmd =
        net_cli::Command::from_args(&["net"], &stop_args).expect("failed to parse stop command");
    let stop_res =
        net_cli::do_root(writer::JsonWriter::new_test(None, &buffers), stop_cmd, &connector, &deps)
            .await;

    if let Some(expected_err) = expected_stop_error {
        let err = stop_res.expect_err("expected stop to fail");
        assert!(
            err.root_cause().to_string().contains(expected_err),
            "expected error containing {:?}, got {:?}",
            expected_err,
            err
        );
        return;
    }

    stop_res.expect("stop capture failed");

    let expected_active_name = start_name.unwrap_or(net_cli::DEFAULT_CAPTURE_NAME);

    if skip_download {
        assert_eq!(*deps.output_buffer.lock().unwrap(), None);
        assert_eq!(*deps.written_path.lock().unwrap(), None);
    } else {
        let path = deps.written_path.lock().unwrap().clone().expect("expected path");
        let expected_path = if let Some(out_path) = output {
            PathBuf::from(out_path)
        } else {
            std::env::temp_dir().join("pcap").join(format!("{expected_active_name}.pcapng"))
        };
        assert_eq!(path, expected_path);

        let pcap_data = deps.output_buffer.lock().unwrap().clone().expect("expected data");
        assert!(!pcap_data.is_empty());

        let cap = pcap::parse_pcapng(&pcap_data).expect("failed to parse pcapng");
        let expected_packets = payloads
            .iter()
            .map(|payload| pcap_helper::ExpectedUdpPacket {
                src_ip: net_declare::net_ip_v4!("127.0.0.1"),
                dst_ip: net_declare::net_ip_v4!("127.0.0.1"),
                src_port: PORT,
                dst_port: PORT,
                payload: payload.as_slice(),
            })
            .collect::<Vec<_>>();
        pcap_helper::assert_udp_packets(
            cap.packet_blocks(),
            &expected_packets,
            true, /* force_skip_checksum_validation */
        );
    }
}

#[test_case("name:lo" ; "by_name")]
#[test_case("id:1" ; "by_id")]
#[test_case("ip:127.0.0.1" ; "by_ip")]
#[fasync::run_singlethreaded(test)]
async fn test_interface_resolution(interface: &'static str) {
    run_lifecycle_test(TestParams {
        start_name: None,
        stop_name: None,
        interface,
        output: None,
        skip_download: false,
        expected_stop_error: None,
        payloads: vec![vec![1, 2, 3, 4]],
    })
    .await;
}

#[fasync::run_singlethreaded(test)]
async fn test_skip_download() {
    run_lifecycle_test(TestParams {
        start_name: None,
        stop_name: None,
        interface: "name:lo",
        output: None,
        skip_download: true,
        expected_stop_error: None,
        payloads: vec![vec![1, 2, 3, 4]],
    })
    .await;
}

#[fasync::run_singlethreaded(test)]
async fn test_skip_download_with_output_error() {
    run_lifecycle_test(TestParams {
        start_name: None,
        stop_name: None,
        interface: "name:lo",
        output: Some("custom_path"),
        skip_download: true,
        expected_stop_error: Some("Cannot specify both"),
        payloads: vec![vec![1, 2, 3, 4]],
    })
    .await;
}

#[test_case(None, None ; "default_name_default_path")]
#[test_case(Some("test_cap"), None ; "custom_name_default_path")]
#[test_case(Some("test_cap"), Some("custom_path") ; "custom_name_custom_path")]
#[fasync::run_singlethreaded(test)]
async fn test_name_and_output_path(
    capture_name: Option<&'static str>,
    output: Option<&'static str>,
) {
    run_lifecycle_test(TestParams {
        start_name: capture_name,
        stop_name: capture_name,
        interface: "name:lo",
        output,
        skip_download: false,
        expected_stop_error: None,
        payloads: vec![vec![1, 2, 3, 4]],
    })
    .await;
}

#[fasync::run_singlethreaded(test)]
async fn test_large_packet_capture() {
    const PAYLOAD_SIZE: usize = 65000;
    // Generate 3 large packets with distinct repeating byte sequences to ensure
    // that the capture file exceeds both the 8 KiB FIDL read buffer (fio::MAX_BUF)
    // and the 16 concurrent in-flight reads queue (16 * 8 KiB = 128 KiB).
    let payloads = (0..3)
        .map(|seed| {
            std::iter::once(seed).chain((0..=255u8).cycle()).take(PAYLOAD_SIZE).collect::<Vec<u8>>()
        })
        .collect();

    run_lifecycle_test(TestParams {
        start_name: None,
        stop_name: None,
        interface: "name:lo",
        output: None,
        skip_download: false,
        expected_stop_error: None,
        payloads,
    })
    .await;
}
