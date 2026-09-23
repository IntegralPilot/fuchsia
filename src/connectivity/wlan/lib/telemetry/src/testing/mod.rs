// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fidl::endpoints::create_proxy_and_stream;
use fidl_fuchsia_metrics::{MetricEvent, MetricEventLoggerRequest, MetricEventPayload};
use fuchsia_async as fasync;
use fuchsia_inspect::reader::{
    DiagnosticsHierarchy, {self as reader},
};
use fuchsia_inspect::{Inspector, Node as InspectNode};
use futures::TryStreamExt;
use futures::stream::FusedStream;
use futures::task::Poll;
use std::pin::pin;
use windowed_stats::experimental::testing::MockTimeMatrixClient;

trait CobaltExt {
    // Respond to MetricEventLoggerRequest and extract its MetricEvent
    fn respond_to_metric_req(
        self,
        result: Result<(), fidl_fuchsia_metrics::Error>,
    ) -> Vec<fidl_fuchsia_metrics::MetricEvent>;
}

impl CobaltExt for MetricEventLoggerRequest {
    fn respond_to_metric_req(
        self,
        result: Result<(), fidl_fuchsia_metrics::Error>,
    ) -> Vec<fidl_fuchsia_metrics::MetricEvent> {
        match self {
            Self::LogOccurrence { metric_id, count, event_codes, responder } => {
                assert!(responder.send(result).is_ok());
                vec![MetricEvent {
                    metric_id,
                    event_codes,
                    payload: MetricEventPayload::Count(count),
                }]
            }
            Self::LogInteger { metric_id, value, event_codes, responder } => {
                assert!(responder.send(result).is_ok());
                vec![MetricEvent {
                    metric_id,
                    event_codes,
                    payload: MetricEventPayload::IntegerValue(value),
                }]
            }
            Self::LogIntegerHistogram { metric_id, histogram, event_codes, responder } => {
                assert!(responder.send(result).is_ok());
                vec![MetricEvent {
                    metric_id,
                    event_codes,
                    payload: MetricEventPayload::Histogram(histogram),
                }]
            }
            Self::LogString { metric_id, string_value, event_codes, responder } => {
                assert!(responder.send(result).is_ok());
                vec![MetricEvent {
                    metric_id,
                    event_codes,
                    payload: MetricEventPayload::StringValue(string_value),
                }]
            }
            Self::LogMetricEvents { events, responder } => {
                assert!(responder.send(result).is_ok());
                events
            }
        }
    }
}

pub struct TestHelper {
    pub inspector: Inspector,
    pub inspect_node: InspectNode,
    pub inspect_metadata_node: InspectNode,
    pub inspect_metadata_path: String,

    pub cobalt_proxy: fidl_fuchsia_metrics::MetricEventLoggerProxy,
    cobalt_stream: fidl_fuchsia_metrics::MetricEventLoggerRequestStream,
    /// As requests to Cobalt are responded to via `self.drain_cobalt_events()`,
    /// their payloads are drained to this HashMap
    pub cobalt_events: Vec<MetricEvent>,

    pub monitor_svc_proxy: fidl_fuchsia_wlan_device_service::DeviceMonitorProxy,
    monitor_svc_stream: fidl_fuchsia_wlan_device_service::DeviceMonitorRequestStream,
    pub telemetry_svc_stream: Option<fidl_fuchsia_wlan_sme::TelemetryRequestStream>,

    pub mock_time_matrix_client: MockTimeMatrixClient,

    // Note: keep the executor field last in the struct so it gets dropped last.
    pub exec: fasync::TestExecutor,
}

impl TestHelper {
    /// Execute the future and respond to GetSmeTelemetry request from DeviceMonitor
    pub fn run_and_handle_get_sme_telemetry<T>(
        &mut self,
        test_fut: &mut (impl Future<Output = T> + Unpin),
    ) -> Poll<T> {
        let result = self.exec.run_until_stalled(test_fut);
        if let Poll::Ready(Some(Ok(req))) =
            self.exec.run_until_stalled(&mut self.monitor_svc_stream.next())
        {
            match req {
                fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetSmeTelemetry {
                    telemetry_server,
                    responder,
                    ..
                } => {
                    let telemetry_stream = telemetry_server.into_stream();
                    responder.send(Ok(())).expect("Failed to respond to telemetry request");
                    self.telemetry_svc_stream = Some(telemetry_stream);
                    self.exec.run_until_stalled(test_fut)
                }
                _ => panic!("Unexpected device monitor request: {req:?}"),
            }
        } else {
            result
        }
    }

    /// Continually execute the future and respond to any incoming Cobalt request with Ok.
    /// Append each metric request payload into `self.cobalt_events`.
    pub fn run_until_stalled_drain_cobalt_events<F>(&mut self, test_fut: &mut F) -> Poll<F::Output>
    where
        F: Future + Unpin,
    {
        let mut made_progress = true;
        let mut result = Poll::Pending;
        while made_progress {
            result = self.exec.run_until_stalled(test_fut);
            made_progress = false;
            while let Poll::Ready(Some(Ok(req))) =
                self.exec.run_until_stalled(&mut self.cobalt_stream.next())
            {
                self.cobalt_events.append(&mut req.respond_to_metric_req(Ok(())));
                made_progress = true;
            }
        }
        result
    }

    pub fn run_and_respond_query_telemetry_support<T>(
        &mut self,
        test_fut: &mut (impl Future<Output = T> + Unpin),
        telemetry_support: Result<&fidl_fuchsia_wlan_stats::TelemetrySupport, i32>,
    ) -> Poll<T> {
        let result = self.exec.run_until_stalled(test_fut);
        let telemetry_svc_stream = match &mut self.telemetry_svc_stream {
            Some(telemetry_svc_stream) if !telemetry_svc_stream.is_terminated() => {
                telemetry_svc_stream
            }
            _ => return result,
        };

        let mut telemetry_svc_req_fut = pin!(telemetry_svc_stream.try_next());
        let request = match self.exec.run_until_stalled(&mut telemetry_svc_req_fut) {
            Poll::Ready(Ok(Some(request))) => request,
            _ => return result,
        };

        match request {
            fidl_fuchsia_wlan_sme::TelemetryRequest::QueryTelemetrySupport { responder } => {
                responder
                    .send(telemetry_support)
                    .expect("expect sending QueryTelemetrySupport response to succeed");
            }
            _ => {
                panic!("unexpected request: {request:?}");
            }
        }
        self.exec.run_until_stalled(test_fut)
    }

    pub fn run_and_respond_iface_stats_req<T>(
        &mut self,
        test_fut: &mut (impl Future<Output = T> + Unpin),
        iface_stats_resp: Result<&fidl_fuchsia_wlan_stats::IfaceStats, i32>,
    ) -> Poll<T> {
        let result = self.exec.run_until_stalled(test_fut);
        let telemetry_svc_stream = match &mut self.telemetry_svc_stream {
            Some(telemetry_svc_stream) if !telemetry_svc_stream.is_terminated() => {
                telemetry_svc_stream
            }
            _ => return result,
        };

        let mut telemetry_svc_req_fut = pin!(telemetry_svc_stream.try_next());
        let request = match self.exec.run_until_stalled(&mut telemetry_svc_req_fut) {
            Poll::Ready(Ok(Some(request))) => request,
            _ => return result,
        };

        match request {
            fidl_fuchsia_wlan_sme::TelemetryRequest::GetIfaceStats { responder } => {
                responder
                    .send(iface_stats_resp)
                    .expect("expect sending GetIfaceStats response to succeed");
            }
            _ => {
                panic!("unexpected request: {request:?}");
            }
        }
        self.exec.run_until_stalled(test_fut)
    }

    pub fn run_and_respond_get_signal_report<T>(
        &mut self,
        test_fut: &mut (impl Future<Output = T> + Unpin),
        signal_report: Result<&fidl_fuchsia_wlan_stats::SignalReport, i32>,
    ) -> Poll<T> {
        let result = self.exec.run_until_stalled(test_fut);
        let telemetry_svc_stream = match &mut self.telemetry_svc_stream {
            Some(telemetry_svc_stream) if !telemetry_svc_stream.is_terminated() => {
                telemetry_svc_stream
            }
            _ => return result,
        };

        let mut telemetry_svc_req_fut = pin!(telemetry_svc_stream.try_next());
        let request = match self.exec.run_until_stalled(&mut telemetry_svc_req_fut) {
            Poll::Ready(Ok(Some(request))) => request,
            _ => return result,
        };

        match request {
            fidl_fuchsia_wlan_sme::TelemetryRequest::GetSignalReport { responder } => {
                responder
                    .send(signal_report)
                    .expect("expect sending GetSignalReport response to succeed");
            }
            _ => {
                panic!("unexpected request: {request:?}");
            }
        }
        self.exec.run_until_stalled(test_fut)
    }

    pub fn get_logged_metrics(&self, metric_id: u32) -> Vec<MetricEvent> {
        self.cobalt_events.iter().filter(|ev| ev.metric_id == metric_id).cloned().collect()
    }

    /// Empty the cobalt metrics can be stored so that future checks on cobalt metrics can
    /// ignore previous values.
    pub fn clear_cobalt_events(&mut self) {
        self.cobalt_events = Vec::new();
    }

    pub fn get_inspect_data_tree(&mut self) -> DiagnosticsHierarchy {
        let read_fut = reader::read(&self.inspector);
        let mut read_fut = pin!(read_fut);
        match self.exec.run_until_stalled(&mut read_fut) {
            Poll::Pending => {
                panic!("Unexpected pending state");
            }
            Poll::Ready(result) => result.expect("failed to get hierarchy"),
        }
    }

    pub fn create_inspect_node(&mut self, name: &str) -> InspectNode {
        self.inspector.root().create_child(name)
    }

    pub fn filtered_cobalt_logger(&self) -> Arc<crate::util::cobalt_logger::FilteredCobaltLogger> {
        Arc::new(crate::util::cobalt_logger::FilteredCobaltLogger::new(
            self.cobalt_proxy.clone(),
            crate::config::CobaltAllowlist::All,
        ))
    }

    pub fn filtered_cobalt_logger_with_allowlist(
        &self,
        allowlist: crate::config::CobaltAllowlist,
    ) -> Arc<crate::util::cobalt_logger::FilteredCobaltLogger> {
        Arc::new(crate::util::cobalt_logger::FilteredCobaltLogger::new(
            self.cobalt_proxy.clone(),
            allowlist,
        ))
    }
}

pub fn setup_test() -> TestHelper {
    let exec = fasync::TestExecutor::new_with_fake_time();
    exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

    let (cobalt_proxy, cobalt_stream) =
        create_proxy_and_stream::<fidl_fuchsia_metrics::MetricEventLoggerMarker>();

    let (monitor_svc_proxy, monitor_svc_stream) =
        create_proxy_and_stream::<fidl_fuchsia_wlan_device_service::DeviceMonitorMarker>();

    let inspector = Inspector::default();
    let inspect_node = inspector.root().create_child("test_stats");
    let inspect_metadata_node = inspect_node.create_child("metadata");
    let inspect_metadata_path = "root/test_stats/metadata".to_string();

    let mock_time_matrix_client = MockTimeMatrixClient::new();

    TestHelper {
        inspector,
        inspect_node,
        inspect_metadata_node,
        inspect_metadata_path,
        cobalt_stream,
        cobalt_proxy,
        cobalt_events: vec![],
        monitor_svc_proxy,
        monitor_svc_stream,
        telemetry_svc_stream: None,
        mock_time_matrix_client,
        exec,
    }
}
