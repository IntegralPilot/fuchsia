// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of the [`fuchsia.input.report.InputDevice`] FIDL server
//! protocol.
//!
//! The code in this module is useful, but does not meet the team's quality
//! bar. See go/fuchsia-display-rough

// TODO(https://fxbug.dev/559080325): Replace with shared input-report-reader crate once available.

use crate::data_types::reports as registers;
use crate::hardware_integration::fidl_input_report_reader::V2ReaderEntry;
use crate::hardware_integration::input_reports::make_input_report;
use fidl_next;
use fidl_next_fuchsia_input_report as fidl_input_report;
use fuchsia_sync::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Max unacknowledged report count allowed for 1/2 second.
///
/// GT6853 touchscreens are interrupt-driven, and the report rate is configurable
/// between 50 and 200 Hz based on the datasheet.
/// Assuming a standard max event rate of 120 Hz yields 60 reports per 1/2 second
/// (120 Hz / 2).
pub const MAX_REPORTS_PER_HALF_SECOND: u16 = 60;

struct FidlInputDeviceInner {
    descriptor: fidl_input_report::DeviceDescriptor,
    v2_readers: Mutex<Vec<V2ReaderEntry>>,
    next_reader_id: AtomicUsize,
}

impl FidlInputDeviceInner {
    /// Creates an `InputReportsReaderV2` reader and registers it.
    ///
    /// Returns the negotiated max unacknowledged reports limit clamped to the
    /// device limit.
    fn create_reader_v2(
        self: &Arc<Self>,
        server_end: fidl_next::ServerEnd<fidl_input_report::InputReportsReaderV2>,
        max_unacknowledged_reports_limit: u16,
    ) -> u16 {
        let max_unacknowledged_reports =
            max_unacknowledged_reports_limit.clamp(1, MAX_REPORTS_PER_HALF_SECOND);
        if max_unacknowledged_reports_limit != max_unacknowledged_reports {
            log::info!(
                "get_input_reports_reader_v2: requested limit {} clamped to {}",
                max_unacknowledged_reports_limit,
                max_unacknowledged_reports
            );
        }

        let mut v2_readers = self.v2_readers.lock();
        let id = self.next_reader_id.fetch_add(1, Ordering::Relaxed);
        let weak_inner = Arc::downgrade(self);
        let entry = V2ReaderEntry::new(id, server_end, max_unacknowledged_reports, move || {
            if let Some(inner) = weak_inner.upgrade() {
                let mut v2_readers = inner.v2_readers.lock();
                v2_readers.retain(|entry| entry.id() != id);
                log::info!("V2 Reader disconnected. Remaining readers: {}", v2_readers.len());
            }
        });

        assert!(v2_readers.iter().all(|r| r.id() != id), "Duplicate V2 reader ID {id}");
        v2_readers.push(entry);
        log::info!("V2 reader connected. Total readers: {}", v2_readers.len());

        max_unacknowledged_reports
    }
}

/// Serves the `fuchsia.input.report.InputDevice` protocol.
#[derive(Clone)]
pub struct FidlInputDevice {
    inner: Arc<FidlInputDeviceInner>,
}

impl FidlInputDevice {
    /// Creates a new [`FidlInputDevice`] with the given [`fidl_input_report::DeviceDescriptor`].
    pub fn new(descriptor: fidl_input_report::DeviceDescriptor) -> Self {
        Self {
            inner: Arc::new(FidlInputDeviceInner {
                descriptor,
                v2_readers: Mutex::new(Vec::new()),
                next_reader_id: AtomicUsize::new(0),
            }),
        }
    }

    /// Dispatches active touch contacts to all registered V2 readers.
    pub fn handle_touch_report(
        &self,
        contacts: &[registers::TouchContact],
        event_time: zx::MonotonicInstant,
    ) {
        let v2_readers = self.inner.v2_readers.lock();
        log::debug!(
            "FidlInputDevice: dispatching touch report across {} V2 readers",
            v2_readers.len()
        );
        for reader in v2_readers.iter() {
            reader.push_report(make_input_report(event_time, contacts));
        }
    }
}

impl fidl_input_report::InputDeviceServerHandler for FidlInputDevice {
    async fn get_descriptor(
        &mut self,
        responder: fidl_next::Responder<fidl_input_report::input_device::GetDescriptor>,
    ) {
        let _ = responder.respond(&self.inner.descriptor).await;
    }

    async fn get_input_reports_reader(
        &mut self,
        #[expect(unused)] request: fidl_next::Request<
            fidl_input_report::input_device::GetInputReportsReader,
        >,
    ) {
        // V1 InputReportsReader is deprecated in API 32 and unsupported in GT6853.
        // Dropping the request immediately closes the channel without allocating resources.
        log::warn!("GetInputReportsReader (V1) is deprecated and unsupported; dropping request");
    }

    async fn get_input_reports_reader_v2(
        &mut self,
        request: fidl_next::Request<fidl_input_report::input_device::GetInputReportsReaderV2>,
        responder: fidl_next::Responder<fidl_input_report::input_device::GetInputReportsReaderV2>,
    ) {
        let payload = request.payload();
        let max_unacknowledged_reports =
            self.inner.create_reader_v2(payload.reader, payload.max_unacknowledged_reports_limit);
        let _ = responder.respond(max_unacknowledged_reports).await;
    }

    async fn get_input_report(
        &mut self,
        #[expect(unused)] request: fidl_next::Request<
            fidl_input_report::input_device::GetInputReport,
        >,
        responder: fidl_next::Responder<fidl_input_report::input_device::GetInputReport>,
    ) {
        let _ = responder.respond_err(zx::Status::NOT_SUPPORTED).await;
    }

    async fn send_output_report(
        &mut self,
        #[expect(unused)] request: fidl_next::Request<
            fidl_input_report::input_device::SendOutputReport,
        >,
        responder: fidl_next::Responder<fidl_input_report::input_device::SendOutputReport>,
    ) {
        let _ = responder.respond_err(zx::Status::NOT_SUPPORTED).await;
    }

    async fn get_feature_report(
        &mut self,
        responder: fidl_next::Responder<fidl_input_report::input_device::GetFeatureReport>,
    ) {
        let _ = responder.respond_err(zx::Status::NOT_SUPPORTED).await;
    }

    async fn set_feature_report(
        &mut self,
        #[expect(unused)] request: fidl_next::Request<
            fidl_input_report::input_device::SetFeatureReport,
        >,
        responder: fidl_next::Responder<fidl_input_report::input_device::SetFeatureReport>,
    ) {
        let _ = responder.respond_err(zx::Status::NOT_SUPPORTED).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_types::hardware_limits::{
        MAX_CONTACTS, NELSON_MAX_CONTACT_X, NELSON_MAX_CONTACT_Y,
    };
    use crate::hardware_integration::descriptors;

    fn setup_test_device() -> (
        fidl_next::Client<fidl_input_report::InputDevice>,
        FidlInputDevice,
        fuchsia_async::CancelableJoinHandle<
            Result<FidlInputDevice, fidl_next::ProtocolError<zx::Status>>,
        >,
    ) {
        let (input_device_client_end, input_device_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_input_report::InputDevice>();
        let input_device_client = input_device_client_end.spawn();
        let fidl_input_device = FidlInputDevice::new(descriptors::make_device_descriptor());
        let input_device_server_task = input_device_server_end.spawn(fidl_input_device.clone());
        (input_device_client, fidl_input_device, input_device_server_task)
    }

    #[fuchsia::test]
    async fn test_get_descriptor() {
        let (input_device_client, _fidl_input_device, _input_device_server_task) =
            setup_test_device();

        let res = input_device_client.get_descriptor().await.unwrap();
        let touch_desc = res.descriptor.touch.unwrap();
        let input_desc = touch_desc.input.unwrap();
        assert_eq!(input_desc.max_contacts, Some(MAX_CONTACTS as u32));
        assert_eq!(input_desc.touch_type, Some(fidl_input_report::TouchType::Touchscreen));

        let contacts = input_desc.contacts.as_ref().unwrap();
        assert_eq!(contacts.len(), MAX_CONTACTS);
        let pos_x = contacts[0].position_x.as_ref().unwrap();
        let pos_y = contacts[0].position_y.as_ref().unwrap();

        assert_eq!(pos_x.range.min, 0);
        assert_eq!(pos_x.range.max, NELSON_MAX_CONTACT_X);
        assert_eq!(pos_y.range.min, 0);
        assert_eq!(pos_y.range.max, NELSON_MAX_CONTACT_Y);

        let dev_info = res.descriptor.device_information.as_ref().unwrap();
        assert_eq!(dev_info.vendor_id, Some(u32::from(fidl_input_report::VendorId::Google)));
        assert_eq!(
            dev_info.product_id,
            Some(u32::from(fidl_input_report::VendorGoogleProductId::GoodixTouchscreen))
        );
        assert_eq!(dev_info.manufacturer_name, Some("Goodix".to_string()));
        assert_eq!(dev_info.product_name, Some("GT6853 Touchscreen".to_string()));
    }

    #[fuchsia::test]
    async fn test_get_input_reports_reader_v2_limit_clamping() {
        let (input_device_client, fidl_input_device, _input_device_server_task) =
            setup_test_device();

        // 1. Lower-bound clamp: 0 -> 1
        let (_reader_client_end, reader_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_input_report::InputReportsReaderV2>();
        let res =
            input_device_client.get_input_reports_reader_v2(reader_server_end, 0).await.unwrap();
        assert_eq!(res.max_unacknowledged_reports, 1);

        // 2. In-range: 30 -> 30
        let (_reader_client_end, reader_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_input_report::InputReportsReaderV2>();
        let res =
            input_device_client.get_input_reports_reader_v2(reader_server_end, 30).await.unwrap();
        assert_eq!(res.max_unacknowledged_reports, 30);

        // 3. Upper-bound clamp: 200 -> MAX_REPORTS_PER_HALF_SECOND (60)
        let (_reader_client_end, reader_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_input_report::InputReportsReaderV2>();
        let res =
            input_device_client.get_input_reports_reader_v2(reader_server_end, 200).await.unwrap();
        assert_eq!(res.max_unacknowledged_reports, MAX_REPORTS_PER_HALF_SECOND);
        assert_eq!(fidl_input_device.inner.v2_readers.lock().len(), 3);
    }

    #[fuchsia::test]
    async fn test_v2_reader_pruning_on_disconnect() {
        let (input_device_client, fidl_input_device, _input_device_server_task) =
            setup_test_device();

        let (reader_client_end, reader_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_input_report::InputReportsReaderV2>();
        input_device_client.get_input_reports_reader_v2(reader_server_end, 60).await.unwrap();
        let reader_client = reader_client_end.spawn();

        let v2_readers = &fidl_input_device.inner.v2_readers;
        while v2_readers.lock().len() != 1 {
            fuchsia_async::yield_now().await;
        }

        assert_eq!(v2_readers.lock().len(), 1);

        drop(reader_client);
        drop(input_device_client);

        while !v2_readers.lock().is_empty() {
            fuchsia_async::yield_now().await;
        }

        assert_eq!(v2_readers.lock().len(), 0);
    }

    #[fuchsia::test]
    async fn test_handle_touch_report_dispatch() {
        let (input_device_client, fidl_input_device, _input_device_server_task) =
            setup_test_device();

        let (_reader_client_end, reader_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_input_report::InputReportsReaderV2>();
        input_device_client.get_input_reports_reader_v2(reader_server_end, 60).await.unwrap();

        let v2_readers = &fidl_input_device.inner.v2_readers;
        while v2_readers.lock().len() != 1 {
            fuchsia_async::yield_now().await;
        }

        let contact = registers::TouchContact {
            track_id_and_status: registers::TrackIdAndStatus(0x01),
            x7_0: 0x64,
            x15_8: 0x00, // x = 100
            y7_0: 0xc8,
            y15_8: 0x00, // y = 200
            pressure: 0x10,
            reserved: [0; 2],
        };

        const EXPECTED_TIME_NANOS: i64 = 555_666_777;
        fidl_input_device
            .handle_touch_report(&[contact], zx::MonotonicInstant::from_nanos(EXPECTED_TIME_NANOS));

        assert_eq!(v2_readers.lock().len(), 1);
    }

    #[fuchsia::test]
    async fn test_unsupported_v1_reader_dropped() {
        let (input_device_client, fidl_input_device, _input_device_server_task) =
            setup_test_device();

        let (reader_client_end, reader_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_input_report::InputReportsReader>();
        input_device_client.get_input_reports_reader(reader_server_end).await.unwrap();
        let reader_client = reader_client_end.spawn();

        let result = reader_client.read_input_reports().await;
        assert!(result.is_err());
        assert_eq!(fidl_input_device.inner.v2_readers.lock().len(), 0);
    }
}
