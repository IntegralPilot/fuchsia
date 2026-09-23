// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod fake_serial;

use fdf_component::ServiceOffer;
use fdf_component::testing::harness::TestHarness;
use fidl_next_fuchsia_hardware_serial as serial;
use fidl_next_fuchsia_hardware_serialimpl as serialimpl;
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;
use std::sync::Arc;

use crate::BtTransportUart;
use fake_serial::{FAKE_SERIAL_PID, FakeSerialService, FakeSerialState};

fn setup_test_harness(state: Arc<Mutex<FakeSerialState>>) -> TestHarness<BtTransportUart> {
    let mut driver_incoming = ServiceFs::new();
    let harness = TestHarness::<BtTransportUart>::new();
    let offer = ServiceOffer::<serialimpl::Service>::new_next()
        .add_default_named_next(
            &mut driver_incoming,
            "default",
            FakeSerialService::new(harness.dispatcher(), state),
        )
        .build_driver_offer();

    harness.add_offer(offer).set_driver_incoming(driver_incoming)
}

#[fuchsia::test]
async fn test_driver_lifecycle_start_and_stop() {
    let state = Arc::new(Mutex::new(FakeSerialState::default()));
    let mut harness = setup_test_harness(state.clone());

    let started_driver = harness.start_driver().await.expect("driver should start successfully");

    {
        let state_guard = state.lock();
        assert!(state_guard.enabled, "device should be enabled");
        assert_eq!(state_guard.cancel_all_count, 0, "cancel_all should not be called yet");
    }

    let driver = started_driver.get_driver().expect("driver instance");
    assert_eq!(driver.serial_pid(), FAKE_SERIAL_PID);

    started_driver.stop_driver().await;

    {
        let state_guard = state.lock();
        assert_eq!(state_guard.cancel_all_count, 1, "stop should trigger cancel_all exactly once");
    }
}

#[fuchsia::test]
async fn test_driver_get_info_error() {
    let state = Arc::new(Mutex::new(FakeSerialState {
        info_result: Err(zx::Status::INTERNAL),
        ..Default::default()
    }));
    let mut harness = setup_test_harness(state);

    assert_eq!(harness.start_driver().await.err(), Some(zx::Status::INTERNAL));
}

#[fuchsia::test]
async fn test_driver_wrong_serial_class() {
    let state = Arc::new(Mutex::new(FakeSerialState {
        info_result: Ok(serial::SerialPortInfo {
            serial_class: serial::Class::Generic,
            serial_vid: 0,
            serial_pid: 0,
        }),
        ..Default::default()
    }));
    let mut harness = setup_test_harness(state);

    assert_eq!(harness.start_driver().await.err(), Some(zx::Status::INTERNAL));
}

#[fuchsia::test]
async fn test_driver_enable_error() {
    let state = Arc::new(Mutex::new(FakeSerialState {
        enable_result: Err(zx::Status::NOT_SUPPORTED),
        ..Default::default()
    }));
    let mut harness = setup_test_harness(state.clone());

    assert_eq!(harness.start_driver().await.err(), Some(zx::Status::NOT_SUPPORTED));
    assert!(!state.lock().enabled, "device should not be marked enabled if enable() failed");
}
