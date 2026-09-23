// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_bluetooth_snoop::PacketFormat;
use fidl_fuchsia_feedback::{Attachment, CrashReport, CrashReporterProxy};
use fidl_fuchsia_firmware_crash as fcrash;
use fidl_fuchsia_hardware_bluetooth::VendorCrashParameters;
use fidl_fuchsia_mem as fmem;
use log::{error, info, warn};
use zx::Vmo;

use crate::packet_logs::{
    PCAP_GLOBAL_HEADER_SIZE, PCAP_PACKET_HEADER_SIZE, write_pcap_header, write_pcap_packet_header,
};
use crate::snooper::SnoopPacket;

/// This is slightly more than the largest Broadcom controller core dump we've seen.
const MAX_SNOOP_VMO_SIZE: u64 = 4 * 1024 * 1024; // 4MB

/// HCI Event Code for Vendor Specific Events.
const VENDOR_EVENT_CODE: u8 = 0xFF;

const CORE_DUMP_FILE: &str = "bluetooth_core_dump.pcap";
const DEFAULT_PROGRAM_NAME: &str = "bt-snoop";
const DEFAULT_CRASH_SIGNATURE: &str = "bluetooth-controller-crash";

/// The debounce duration is a guess, but the events should be sent rapid-fire with less than a
/// second between each event.
pub(crate) const CRASH_REPORT_DEBOUNCE_DURATION: zx::MonotonicDuration =
    zx::MonotonicDuration::from_seconds(5);

const CRASH_REPORT_RATE_LIMIT_DURATION: zx::MonotonicDuration =
    zx::MonotonicDuration::from_hours(1);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CrashEventStatus {
    FirstCrashEvent,
    ContinuingCrashEvent,
    NotCrashEvent,
}

pub(crate) struct CrashState {
    pub parameters: VendorCrashParameters,
    pub last_report_local_time: Option<fuchsia_async::MonotonicInstant>,
    pub collector: Option<CoreDumpCollector>,
    /// The target time for filing the crash report, used for debouncing.
    ///
    /// When a crash is detected, this is set to `now() + CRASH_REPORT_DEBOUNCE_DURATION`.
    /// Each subsequent packet received while collecting the core dump will push this time
    /// further into the future.
    ///
    /// Once the current time reaches `tentative_report_file_time` without being pushed back, the crash
    /// report is considered finished and is filed.
    pub tentative_report_file_time: Option<fuchsia_async::MonotonicInstant>,
}

impl CrashState {
    pub(crate) fn is_rate_limited(&self) -> bool {
        self.collector.is_none()
            && self.last_report_local_time.map_or(false, |t| {
                fuchsia_async::MonotonicInstant::now() - t < CRASH_REPORT_RATE_LIMIT_DURATION
            })
    }

    pub(crate) fn is_crash_event(&self, packet: &SnoopPacket) -> bool {
        packet.format == PacketFormat::Event
            && packet.payload.len() >= 2
            && packet.payload[0] == VENDOR_EVENT_CODE
            && self.parameters.crash_events.as_ref().map_or(false, |events| {
                events.iter().any(|prefix| packet.payload[2..].starts_with(prefix))
            })
    }

    fn maybe_start_new_crash(&mut self) -> bool {
        if self.collector.is_some() {
            return false;
        }
        let program_name = self
            .parameters
            .program_name
            .clone()
            .unwrap_or_else(|| DEFAULT_PROGRAM_NAME.to_string());
        let crash_signature = self
            .parameters
            .crash_signature
            .clone()
            .unwrap_or_else(|| DEFAULT_CRASH_SIGNATURE.to_string());
        match CoreDumpCollector::new(program_name, crash_signature) {
            Ok(c) => {
                self.collector = Some(c);
                self.last_report_local_time = Some(fuchsia_async::MonotonicInstant::now());
                true
            }
            Err(e) => {
                warn!("Failed to create CoreDumpCollector: {:?}", e);
                false
            }
        }
    }

    /// Returns a status indicating the type of the packet (i.e. whether it was a crash event).
    pub(crate) fn process_packet(&mut self, packet: &SnoopPacket) -> CrashEventStatus {
        if !self.is_crash_event(packet) {
            return CrashEventStatus::NotCrashEvent;
        }

        if self.is_rate_limited() {
            return CrashEventStatus::NotCrashEvent;
        }

        let started_new_crash = self.maybe_start_new_crash();

        if let Some(collector) = &mut self.collector {
            collector.on_packet(packet);
            self.tentative_report_file_time = Some(
                (fuchsia_async::MonotonicInstant::now() + CRASH_REPORT_DEBOUNCE_DURATION).into(),
            );
            if started_new_crash {
                CrashEventStatus::FirstCrashEvent
            } else {
                CrashEventStatus::ContinuingCrashEvent
            }
        } else {
            CrashEventStatus::NotCrashEvent
        }
    }
}

pub(crate) struct CoreDumpCollector {
    vmo: Vmo,
    /// The current offset into the VMO where the next packet should be written.
    offset: u64,
    program_name: String,
    crash_signature: String,
    crash_timestamp: zx::BootInstant,
    logged_drop: bool,
}

impl CoreDumpCollector {
    pub fn new(program_name: String, crash_signature: String) -> Result<Self, Error> {
        let vmo = Vmo::create(MAX_SNOOP_VMO_SIZE).context("failed to create VMO for core dump")?;

        let mut header_buf = [0u8; PCAP_GLOBAL_HEADER_SIZE];
        write_pcap_header(&mut header_buf[..])?;
        vmo.write(&header_buf, 0).context("failed to write pcap header to VMO")?;

        Ok(Self {
            vmo,
            offset: header_buf.len() as u64,
            program_name,
            crash_signature,
            crash_timestamp: zx::BootInstant::get(),
            logged_drop: false,
        })
    }

    pub fn on_packet(&mut self, packet: &SnoopPacket) {
        let packet_len = PCAP_PACKET_HEADER_SIZE as u64 + packet.payload.len() as u64;
        let new_offset = self.offset + packet_len;

        if new_offset > MAX_SNOOP_VMO_SIZE {
            // Avoid log spam by only logging the first dropped packet.
            if !self.logged_drop {
                error!("Dropped packet because VMO limit exceeded");
                self.logged_drop = true;
            }
            return;
        }

        let mut header_buf = [0u8; PCAP_PACKET_HEADER_SIZE];
        if let Err(e) = write_pcap_packet_header(&mut header_buf[..], packet) {
            warn!("Failed to format pcap packet for core dump: {:?}", e);
            return;
        }

        if let Err(e) = self.vmo.write(&header_buf, self.offset) {
            warn!("Failed to write pcap packet header to core dump VMO: {:?}", e);
            return;
        }

        if let Err(e) =
            self.vmo.write(&packet.payload, self.offset + PCAP_PACKET_HEADER_SIZE as u64)
        {
            warn!("Failed to write pcap packet payload to core dump VMO: {:?}", e);
            return;
        }

        self.offset = new_offset;
    }

    pub fn file_firmware_report(&self, reporter: &fcrash::ReporterProxy) {
        if let Err(e) = self.vmo.set_content_size(&self.offset) {
            warn!("Failed to set VMO content size for firmware crash report: {:?}", e);
        }
        let rights =
            zx::Rights::BASIC | zx::Rights::READ | zx::Rights::MAP | zx::Rights::GET_PROPERTY;
        let vmo = match self.vmo.duplicate_handle(rights) {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to duplicate VMO for firmware crash report: {:?}", e);
                return;
            }
        };

        let trimmed = self.crash_signature.trim();
        let reason = if trimmed.is_empty() { DEFAULT_CRASH_SIGNATURE } else { trimmed };
        let reason = &reason[..reason.floor_char_boundary(fcrash::MAX_REASON_LENGTH as usize)];
        let crash = fcrash::Crash {
            subsystem_name: Some("bluetooth".to_string()),
            timestamp: Some(self.crash_timestamp),
            reason: Some(reason.to_string()),
            crash_dump: Some(vmo),
            ..Default::default()
        };

        match reporter.report(crash) {
            Ok(()) => info!("Firmware crash report sent successfully for bluetooth"),
            Err(e) => warn!("FIDL error reporting firmware crash: {:?}", e),
        }
    }

    pub async fn file_report(self, crash_reporter: &CrashReporterProxy) {
        let rights =
            zx::Rights::BASIC | zx::Rights::READ | zx::Rights::MAP | zx::Rights::GET_PROPERTY;
        let vmo = match self.vmo.duplicate_handle(rights) {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to duplicate VMO for crash report: {:?}", e);
                return;
            }
        };

        let attachment = Attachment {
            key: CORE_DUMP_FILE.to_string(),
            value: fmem::Buffer { vmo, size: self.offset },
        };

        let program_name = self.program_name.clone();
        let crash_signature = self.crash_signature.clone();
        let report = CrashReport {
            program_name: Some(self.program_name),
            crash_signature: Some(self.crash_signature),
            attachments: Some(vec![attachment]),
            is_fatal: Some(false),
            ..Default::default()
        };

        match crash_reporter.file_report(report).await {
            Ok(Ok(_)) => info!(
                "Crash report filed successfully for program_name: {}, crash_signature: {}",
                program_name, crash_signature
            ),
            Ok(Err(e)) => warn!("Server returned error filing crash report: {:?}", e),
            Err(e) => warn!("FIDL error filing crash report: {:?}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[fuchsia::test]
    async fn test_core_dump_collector_initialization() {
        let collector = CoreDumpCollector::new("test_program".to_string(), "test_sig".to_string())
            .expect("Failed to create collector");

        assert_eq!(collector.program_name, "test_program");
        assert_eq!(collector.crash_signature, "test_sig");
        // Ensure some data was written to VMO (pcap header)
        assert!(collector.offset > 0);
        let size = collector.vmo.get_size().unwrap();
        assert_eq!(size, MAX_SNOOP_VMO_SIZE);
    }

    #[fuchsia::test]
    async fn test_core_dump_collector_on_packet() {
        let mut collector = CoreDumpCollector::new("test".to_string(), "sig".to_string()).unwrap();
        let packet = SnoopPacket::new(
            true,
            PacketFormat::Event,
            zx::MonotonicInstant::from_nanos(0),
            vec![0xFF, 0x01, 0x1B],
        );

        let initial_offset = collector.offset;
        collector.on_packet(&packet);
        assert!(collector.offset > initial_offset);
    }

    #[fuchsia::test]
    async fn test_core_dump_collector_file_report() {
        use fidl_fuchsia_feedback::{CrashReporterMarker, CrashReporterRequest};

        let mut collector =
            CoreDumpCollector::new("test_prog".to_string(), "test_sig".to_string()).unwrap();
        let packet = SnoopPacket::new(
            true,
            PacketFormat::Event,
            zx::MonotonicInstant::from_nanos(0),
            vec![0xFF, 0x01, 0x1B],
        );
        let expected_offset =
            collector.offset + PCAP_PACKET_HEADER_SIZE as u64 + packet.payload.len() as u64;
        collector.on_packet(&packet);

        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<CrashReporterMarker>();

        let server = async move {
            let req = stream.next().await.unwrap().unwrap();
            let CrashReporterRequest::FileReport { report, responder } = req;
            assert_eq!(report.program_name.as_deref(), Some("test_prog"));
            assert_eq!(report.crash_signature.as_deref(), Some("test_sig"));
            assert_eq!(report.is_fatal, Some(false));

            let attachments = report.attachments.unwrap();
            assert_eq!(attachments.len(), 1);
            assert_eq!(attachments[0].key, CORE_DUMP_FILE);
            assert_eq!(attachments[0].value.size, expected_offset);

            // Respond with success
            let _ = responder.send(Ok(&fidl_fuchsia_feedback::FileReportResults::default()));
        };

        futures::join!(server, collector.file_report(&proxy));
    }

    #[fuchsia::test]
    async fn test_crash_state_is_crash_event_with_multibyte_prefix() {
        let state = CrashState {
            parameters: fidl_fuchsia_hardware_bluetooth::VendorCrashParameters {
                crash_events: Some(vec![vec![0x1b, 0x03]]),
                ..Default::default()
            },
            last_report_local_time: None,
            collector: None,
            tentative_report_file_time: None,
        };

        let matching_packet = SnoopPacket::new(
            true,
            PacketFormat::Event,
            zx::MonotonicInstant::from_nanos(0),
            vec![0xFF, 0x02, 0x1B, 0x03],
        );
        assert!(state.is_crash_event(&matching_packet));

        let non_matching_packet = SnoopPacket::new(
            true,
            PacketFormat::Event,
            zx::MonotonicInstant::from_nanos(0),
            vec![0xFF, 0x02, 0x1B, 0x04],
        );
        assert!(!state.is_crash_event(&non_matching_packet));

        let short_packet = SnoopPacket::new(
            true,
            PacketFormat::Event,
            zx::MonotonicInstant::from_nanos(0),
            vec![0xFF, 0x01, 0x1B],
        );
        assert!(!state.is_crash_event(&short_packet));

        let non_vendor_event = SnoopPacket::new(
            true,
            PacketFormat::Event,
            zx::MonotonicInstant::from_nanos(0),
            vec![0x01, 0x02, 0x1B, 0x03],
        );
        assert!(!state.is_crash_event(&non_vendor_event));
    }

    #[fuchsia::test]
    async fn test_process_packet() {
        let mut state = CrashState {
            parameters: fidl_fuchsia_hardware_bluetooth::VendorCrashParameters {
                crash_events: Some(vec![vec![0x1b]]),
                ..Default::default()
            },
            last_report_local_time: None,
            collector: None,
            tentative_report_file_time: None,
        };

        let non_crash_packet = SnoopPacket::new(
            true,
            PacketFormat::Event,
            zx::MonotonicInstant::from_nanos(0),
            vec![0xFF, 0x01, 0x1A],
        );
        assert_eq!(state.process_packet(&non_crash_packet), CrashEventStatus::NotCrashEvent);

        let crash_packet = SnoopPacket::new(
            true,
            PacketFormat::Event,
            zx::MonotonicInstant::from_nanos(0),
            vec![0xFF, 0x01, 0x1B],
        );
        assert_eq!(state.process_packet(&crash_packet), CrashEventStatus::FirstCrashEvent);

        let continuing_packet = SnoopPacket::new(
            true,
            PacketFormat::Event,
            zx::MonotonicInstant::from_nanos(0),
            vec![0xFF, 0x01, 0x1B],
        );
        assert_eq!(
            state.process_packet(&continuing_packet),
            CrashEventStatus::ContinuingCrashEvent
        );
    }

    #[fuchsia::test]
    async fn test_core_dump_collector_file_firmware_report() {
        let mut collector =
            CoreDumpCollector::new("test_prog".to_string(), "test_sig".to_string()).unwrap();
        let packet = SnoopPacket::new(
            true,
            PacketFormat::Event,
            zx::MonotonicInstant::from_nanos(0),
            vec![0xFF, 0x01, 0x1B],
        );
        let expected_offset =
            collector.offset + PCAP_PACKET_HEADER_SIZE as u64 + packet.payload.len() as u64;
        collector.on_packet(&packet);

        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fcrash::ReporterMarker>();
        collector.file_firmware_report(&proxy);

        let req = stream.next().await.expect("received request").expect("valid request");
        let fcrash::ReporterRequest::Report { payload, .. } = req else {
            panic!("Unexpected request: {req:?}");
        };
        assert_eq!(payload.subsystem_name.as_deref(), Some("bluetooth"));
        assert_eq!(payload.timestamp, Some(collector.crash_timestamp));
        assert_eq!(payload.reason.as_deref(), Some("test_sig"));

        let vmo = payload.crash_dump.expect("crash dump present");
        let content_size = vmo.get_content_size().expect("content size");
        assert_eq!(content_size, expected_offset);

        let mut header = [0u8; PCAP_GLOBAL_HEADER_SIZE];
        vmo.read(&mut header, 0).expect("read header");
        assert_eq!(&header[0..4], &[0xa1, 0xb2, 0xc3, 0xd4]);
    }

    #[fuchsia::test]
    async fn test_core_dump_collector_file_firmware_report_reason_clamped() {
        let long_sig = "a".repeat(200);
        let collector = CoreDumpCollector::new("test_prog".to_string(), long_sig).unwrap();

        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fcrash::ReporterMarker>();
        collector.file_firmware_report(&proxy);

        let req = stream.next().await.expect("received request").expect("valid request");
        let fcrash::ReporterRequest::Report { payload, .. } = req else {
            panic!("Unexpected request: {req:?}");
        };
        assert_eq!(payload.reason.as_deref(), Some(&"a".repeat(128)[..]));
    }

    #[fuchsia::test]
    async fn test_core_dump_collector_file_firmware_report_reason_multibyte_utf8_clamped() {
        let multibyte_sig = format!("{}🦀", "a".repeat(127));
        let collector = CoreDumpCollector::new("test_prog".to_string(), multibyte_sig).unwrap();

        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fcrash::ReporterMarker>();
        collector.file_firmware_report(&proxy);

        let req = stream.next().await.expect("received request").expect("valid request");
        let fcrash::ReporterRequest::Report { payload, .. } = req else {
            panic!("Unexpected request: {req:?}");
        };
        assert_eq!(payload.reason.as_deref(), Some(&"a".repeat(127)[..]));
    }

    #[fuchsia::test]
    async fn test_core_dump_collector_file_firmware_report_whitespace_signature_fallback() {
        let collector = CoreDumpCollector::new("test_prog".to_string(), "   ".to_string()).unwrap();

        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fcrash::ReporterMarker>();
        collector.file_firmware_report(&proxy);

        let req = stream.next().await.expect("received request").expect("valid request");
        let fcrash::ReporterRequest::Report { payload, .. } = req else {
            panic!("Unexpected request: {req:?}");
        };
        assert_eq!(payload.reason.as_deref(), Some(DEFAULT_CRASH_SIGNATURE));
    }

    #[fuchsia::test]
    async fn test_core_dump_collector_file_firmware_report_disconnected() {
        let collector =
            CoreDumpCollector::new("test_prog".to_string(), "test_sig".to_string()).unwrap();

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fcrash::ReporterMarker>();
        drop(stream); // Server disconnected

        // Should log warning on FIDL error and not panic.
        collector.file_firmware_report(&proxy);
    }
}
