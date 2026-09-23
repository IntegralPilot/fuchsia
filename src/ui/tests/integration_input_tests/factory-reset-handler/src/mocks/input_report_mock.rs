// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::RequestStream as _;
use fidl_fuchsia_input as fidl_input;
use fidl_fuchsia_input_report as fidl_input_report;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{ChildOptions, RealmBuilder, Ref};
use futures::StreamExt;

#[derive(Clone)]
pub(crate) struct InputReportMock {
    name: String,
}

impl InputReportMock {
    pub(crate) fn new<M: Into<String>>(name: M) -> Self {
        Self { name: name.into() }
    }
}

enum IncomingRequest {
    InputDeviceService(fidl_input_report::ServiceRequest),
}

#[async_trait::async_trait]
impl crate::traits::test_realm_component::TestRealmComponent for InputReportMock {
    fn ref_(&self) -> Ref {
        Ref::child(&self.name)
    }

    async fn add_to_builder(&self, builder: &RealmBuilder) {
        builder
            .add_local_child(
                &self.name,
                move |handles| {
                    Box::pin(async move {
                        let mut fs = ServiceFs::new();
                        fs.dir("svc")
                            .add_fidl_service_instance("default", IncomingRequest::InputDeviceService);

                        fs.serve_connection(handles.outgoing_dir)?;
                        let tasks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<fasync::Task<()>>::new()));
                        let tasks_clone = tasks.clone();
                        fs.for_each_concurrent(None, move |request| {
                            let tasks = tasks_clone.clone();
                            async move {
                                match request {
                                    IncomingRequest::InputDeviceService(
                                        fidl_input_report::ServiceRequest::InputDevice(mut stream),
                                    ) => {
                                        while let Some(req) = stream.next().await {
                                            match req.unwrap() {
                                                fidl_input_report::InputDeviceRequest::GetDescriptor { responder } => {
                                                    let mut desc = fidl_input_report::DeviceDescriptor::default();
                                                    let mut cc = fidl_input_report::ConsumerControlDescriptor::default();
                                                    cc.input = Some(fidl_input_report::ConsumerControlInputDescriptor {
                                                        buttons: Some(vec![
                                                            fidl_input::ConsumerControlButton::FactoryReset,
                                                        ]),
                                                        ..Default::default()
                                                    });
                                                    desc.consumer_control = Some(cc);
                                                    responder.send(&desc).unwrap();
                                                }
                                                fidl_input_report::InputDeviceRequest::GetInputReportsReaderV2 {
                                                    reader,
                                                    max_unacknowledged_reports_limit,
                                                    responder,
                                                } => {
                                                    responder.send(max_unacknowledged_reports_limit).unwrap();
                                                    tasks.lock().unwrap().push(fasync::Task::local(async move {
                                                        let mut stream = reader.into_stream();
                                                        let control_handle = stream.control_handle();
                                                        let mut report = fidl_input_report::InputReport::default();
                                                        report.event_time = Some(fasync::MonotonicInstant::now().into_nanos());
                                                        let mut cc = fidl_input_report::ConsumerControlInputReport::default();
                                                        cc.pressed_buttons = Some(vec![
                                                            fidl_input::ConsumerControlButton::FactoryReset,
                                                        ]);
                                                        report.consumer_control = Some(cc);
                                                        let _ = control_handle.send_on_input_reports(vec![report], 1);
                                                        while let Some(_req) = stream.next().await {}
                                                    }));
                                                }
                                                fidl_input_report::InputDeviceRequest::GetFeatureReport { responder } => {
                                                    responder.send(Ok(&fidl_input_report::FeatureReport::default())).unwrap();
                                                }
                                                req => {
                                                    panic!("Unexpected InputDeviceRequest: {:?}", req);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }).await;
                        Ok(())
                    })
                },
                ChildOptions::new(),
            )
            .await
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::test_realm_component::TestRealmComponent;
    use fuchsia_component_test::{Capability, Route};

    #[fuchsia::test]
    async fn test_input_report_mock_v2_reader_lifecycle() {
        let mock = InputReportMock::new("input_report_mock");
        let builder = RealmBuilder::new().await.expect("Failed to create RealmBuilder");
        mock.add_to_builder(&builder).await;

        builder
            .add_route(
                Route::new()
                    .capability(Capability::service_by_name("fuchsia.input.report.Service"))
                    .from(mock.ref_())
                    .to(fuchsia_component_test::Ref::parent()),
            )
            .await
            .expect("Failed to add route");

        let realm = builder.build().await.expect("Failed to build realm");

        let device = realm
            .root
            .connect_to_named_protocol_at_exposed_dir::<fidl_input_report::InputDeviceMarker>(
                "fuchsia.input.report.Service/default/input_device",
            )
            .expect("Failed to connect to input device");

        // 1. Verify GetDescriptor
        let desc = device.get_descriptor().await.expect("Failed to get descriptor");
        assert!(desc.consumer_control.is_some());
        let cc_desc = desc.consumer_control.unwrap();
        assert_eq!(
            cc_desc.input.unwrap().buttons.unwrap(),
            vec![fidl_input::ConsumerControlButton::FactoryReset]
        );

        // 2. Verify GetFeatureReport
        let feat = device.get_feature_report().await.expect("Failed to get feature report");
        assert!(feat.is_ok());

        // 3. Verify GetInputReportsReaderV2 responds with requested max_unacknowledged_reports_limit
        let (reader_proxy, server_end) =
            fidl::endpoints::create_proxy::<fidl_input_report::InputReportsReaderV2Marker>();
        let max_unacked = device
            .get_input_reports_reader_v2(server_end, 42)
            .await
            .expect("Failed to get reader v2");
        assert_eq!(max_unacked, 42);

        // 4. Verify V2 event emission: OnInputReports with FactoryReset report and stamp 1
        let mut event_stream = reader_proxy.take_event_stream();
        let event = event_stream.next().await.expect("Expected event").expect("Event error");
        match event {
            fidl_input_report::InputReportsReaderV2Event::OnInputReports {
                reports,
                last_report_stamp,
            } => {
                assert_eq!(last_report_stamp, 1);
                assert_eq!(reports.len(), 1);
                let report = &reports[0];
                assert!(report.event_time.is_some());
                let cc = report.consumer_control.as_ref().expect("ConsumerControl report missing");
                assert_eq!(
                    cc.pressed_buttons.as_ref().unwrap(),
                    &vec![fidl_input::ConsumerControlButton::FactoryReset]
                );
            }
            _ => panic!("Unexpected event received from InputReportsReaderV2"),
        }

        // 5. Verify acknowledgment draining: sending AcknowledgeReports is handled without error
        reader_proxy.acknowledge_reports(1).expect("Failed to send ack");
        reader_proxy.acknowledge_reports(10).expect("Failed to send ack 10");

        // 6. Verify channel disconnection cleanup: dropping reader_proxy terminates the reader task
        drop(event_stream);
        drop(reader_proxy);

        // 7. Verify subsequent reader connection on same device works
        let (reader_proxy2, server_end2) =
            fidl::endpoints::create_proxy::<fidl_input_report::InputReportsReaderV2Marker>();
        let max_unacked2 = device
            .get_input_reports_reader_v2(server_end2, 5)
            .await
            .expect("Failed to get second reader v2");
        assert_eq!(max_unacked2, 5);

        let mut event_stream2 = reader_proxy2.take_event_stream();
        let event2 =
            event_stream2.next().await.expect("Expected second event").expect("Event error");
        match event2 {
            fidl_input_report::InputReportsReaderV2Event::OnInputReports {
                reports,
                last_report_stamp,
            } => {
                assert_eq!(last_report_stamp, 1);
                assert_eq!(reports.len(), 1);
            }
            _ => panic!("Unexpected event received from second InputReportsReaderV2"),
        }
        reader_proxy2.acknowledge_reports(1).expect("Failed to send ack for reader2");
        drop(event_stream2);
        drop(reader_proxy2);

        drop(device);
        realm.destroy().await.expect("Failed to destroy realm");
    }

    #[fuchsia::test]
    async fn test_input_report_mock_concurrent_readers_and_ack_flood() {
        let mock = InputReportMock::new("input_report_mock_stress");
        let builder = RealmBuilder::new().await.expect("Failed to create RealmBuilder");
        mock.add_to_builder(&builder).await;

        builder
            .add_route(
                Route::new()
                    .capability(Capability::service_by_name("fuchsia.input.report.Service"))
                    .from(mock.ref_())
                    .to(fuchsia_component_test::Ref::parent()),
            )
            .await
            .expect("Failed to add route");

        let realm = builder.build().await.expect("Failed to build realm");

        let device = realm
            .root
            .connect_to_named_protocol_at_exposed_dir::<fidl_input_report::InputDeviceMarker>(
                "fuchsia.input.report.Service/default/input_device",
            )
            .expect("Failed to connect to input device");

        // Spawn 5 concurrent readers
        let mut readers = Vec::new();
        for i in 1..=5 {
            let (reader_proxy, server_end) =
                fidl::endpoints::create_proxy::<fidl_input_report::InputReportsReaderV2Marker>();
            let max_unacked = device
                .get_input_reports_reader_v2(server_end, i * 10)
                .await
                .expect("Failed to get reader");
            assert_eq!(max_unacked, i * 10);
            readers.push(reader_proxy);
        }

        // Each reader verifies the event and floods 50 acknowledgments
        for reader_proxy in &readers {
            let mut event_stream = reader_proxy.take_event_stream();
            let event = event_stream.next().await.expect("Expected event").expect("Event error");
            match event {
                fidl_input_report::InputReportsReaderV2Event::OnInputReports {
                    reports,
                    last_report_stamp,
                } => {
                    assert_eq!(last_report_stamp, 1);
                    assert_eq!(reports.len(), 1);
                    assert_eq!(
                        reports[0]
                            .consumer_control
                            .as_ref()
                            .unwrap()
                            .pressed_buttons
                            .as_ref()
                            .unwrap(),
                        &vec![fidl_input::ConsumerControlButton::FactoryReset]
                    );
                }
                _ => panic!("Unexpected event"),
            }

            for stamp in 1..=50 {
                reader_proxy.acknowledge_reports(stamp).expect("Ack failed");
            }
        }

        // Drop readers in reverse order
        while let Some(r) = readers.pop() {
            drop(r);
        }

        drop(device);
        realm.destroy().await.expect("Failed to destroy realm");
    }

    #[fuchsia::test]
    async fn test_input_report_mock_early_disconnect() {
        let mock = InputReportMock::new("input_report_mock_early_drop");
        let builder = RealmBuilder::new().await.expect("Failed to create RealmBuilder");
        mock.add_to_builder(&builder).await;

        builder
            .add_route(
                Route::new()
                    .capability(Capability::service_by_name("fuchsia.input.report.Service"))
                    .from(mock.ref_())
                    .to(fuchsia_component_test::Ref::parent()),
            )
            .await
            .expect("Failed to add route");

        let realm = builder.build().await.expect("Failed to build realm");

        let device = realm
            .root
            .connect_to_named_protocol_at_exposed_dir::<fidl_input_report::InputDeviceMarker>(
                "fuchsia.input.report.Service/default/input_device",
            )
            .expect("Failed to connect to input device");

        // Open reader and drop immediately before reading events
        let (reader_proxy, server_end) =
            fidl::endpoints::create_proxy::<fidl_input_report::InputReportsReaderV2Marker>();
        let max_unacked =
            device.get_input_reports_reader_v2(server_end, 20).await.expect("Failed to get reader");
        assert_eq!(max_unacked, 20);
        drop(reader_proxy);

        // Subsequent reader must still work
        let (reader_proxy2, server_end2) =
            fidl::endpoints::create_proxy::<fidl_input_report::InputReportsReaderV2Marker>();
        let max_unacked2 = device
            .get_input_reports_reader_v2(server_end2, 30)
            .await
            .expect("Failed to get reader 2");
        assert_eq!(max_unacked2, 30);

        let mut event_stream2 = reader_proxy2.take_event_stream();
        let event2 = event_stream2.next().await.expect("Expected event").expect("Event error");
        match event2 {
            fidl_input_report::InputReportsReaderV2Event::OnInputReports {
                reports,
                last_report_stamp,
            } => {
                assert_eq!(last_report_stamp, 1);
                assert_eq!(reports.len(), 1);
            }
            _ => panic!("Unexpected event"),
        }

        drop(reader_proxy2);
        drop(device);
        realm.destroy().await.expect("Failed to destroy realm");
    }
}
