// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ptysvc::{Pty, run_server};
use assert_matches::assert_matches;
use fidl::endpoints::Proxy;
use fidl_fuchsia_device::DeviceSignal;
use fidl_fuchsia_hardware_pty as fpty;
use fidl_fuchsia_hardware_pty::{DeviceMarker, DeviceProxy, WindowSize};
use fuchsia_async as fasync;
use std::cell::RefCell;
use std::rc::Rc;
use test_util::assert_gt;

fn setup() -> DeviceProxy {
    let pty = Rc::new(RefCell::new(Pty::new()));
    let (client_end, server_end) = fidl::endpoints::create_proxy::<DeviceMarker>();
    fasync::Task::local(async move {
        run_server(pty, server_end.into_stream()).await;
    })
    .detach();
    client_end
}

async fn open_client(conn: &DeviceProxy, id: u32) -> Result<DeviceProxy, zx::Status> {
    let (client_end, server_end) = fidl::endpoints::create_endpoints::<DeviceMarker>();
    let status = conn.open_client(id, server_end).await.map_err(|_| zx::Status::PEER_CLOSED)?;
    zx::Status::ok(status)?;
    Ok(client_end.into_proxy())
}

async fn get_event(conn: &DeviceProxy) -> Result<zx::EventPair, zx::Status> {
    let info = conn.describe().await.map_err(|_| zx::Status::PEER_CLOSED)?;
    info.event.ok_or(zx::Status::INVALID_ARGS)
}

#[fuchsia::test]
async fn server_describe() {
    let server = setup();
    let event = get_event(&server).await.expect("failed to get event");
    assert!(!event.is_invalid());
}

#[fuchsia::test]
async fn server_set_window_size() {
    let server = setup();
    let status =
        server.set_window_size(&WindowSize { width: 80, height: 24 }).await.expect("fidl failed");
    assert_eq!(zx::Status::ok(status), Ok(()));
}

#[fuchsia::test]
async fn server_clr_set_feature() {
    let server = setup();
    let (status, _) = server.clr_set_feature(0, 0).await.expect("fidl failed");
    assert_eq!(zx::Status::ok(status), Err(zx::Status::NOT_SUPPORTED));
}

#[fuchsia::test]
async fn server_get_window_size() {
    let server = setup();
    let (status, _) = server.get_window_size().await.expect("fidl failed");
    assert_eq!(zx::Status::ok(status), Err(zx::Status::NOT_SUPPORTED));
}

#[fuchsia::test]
async fn server_make_active() {
    let server = setup();
    let status = server.make_active(0).await.expect("fidl failed");
    assert_eq!(zx::Status::ok(status), Err(zx::Status::NOT_SUPPORTED));
}

#[fuchsia::test]
async fn server_read_events() {
    let server = setup();
    let (status, _) = server.read_events().await.expect("fidl failed");
    assert_eq!(zx::Status::ok(status), Err(zx::Status::NOT_SUPPORTED));
}

#[fuchsia::test]
async fn server_basic_open_client() {
    let server = setup();
    let client = open_client(&server, 0).await.expect("failed to open client");

    // Check client is valid (not closed).
    let channel = client.into_channel().unwrap().into_zx_channel();
    let signals =
        channel.wait_one(zx::Signals::CHANNEL_PEER_CLOSED, zx::MonotonicInstant::INFINITE_PAST);
    assert_matches!(signals, zx::WaitResult::TimedOut(_));
}

#[fuchsia::test]
async fn server_open_client_twice() {
    let server = setup();
    let _client = open_client(&server, 0).await.expect("failed to open client");
    assert_eq!(open_client(&server, 0).await.err(), Some(zx::Status::INVALID_ARGS));
}

#[fuchsia::test]
async fn server_open_client_two_different() {
    let server = setup();
    open_client(&server, 1).await.unwrap();
    open_client(&server, 0).await.unwrap();
}

#[fuchsia::test]
async fn server_with_no_clients_initial_conditions() {
    let server = setup();
    let event = get_event(&server).await.unwrap();

    let check_state = || async {
        let signals = event.wait_one(zx::Signals::USER_ALL, zx::MonotonicInstant::INFINITE_PAST);
        let signals = assert_matches!(signals, zx::WaitResult::Ok(s) => s);
        assert!(signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits())));
        assert!(signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::HANGUP.bits())));

        let result = server.read(10).await.unwrap();
        assert_eq!(result, Ok(vec![]));

        let data = vec![0; 16];
        let result = server.write(&data).await.unwrap();
        assert_eq!(result, Err(zx::Status::PEER_CLOSED.into_raw()));
    };

    check_state().await;

    // Create a client and close it
    {
        let _client = open_client(&server, 1).await.unwrap();
    }
    // Wait for hangup signal (client disconnect)
    let _ = fasync::OnSignals::new(
        &event,
        zx::Signals::from_bits_truncate(DeviceSignal::HANGUP.bits()),
    )
    .await
    .unwrap();

    check_state().await;
}

#[fuchsia::test]
async fn server_with_client_initial_conditions() {
    let server = setup();
    let client = open_client(&server, 0).await.unwrap();

    let server_event = get_event(&server).await.unwrap();
    let client_event = get_event(&client).await.unwrap();

    let signals = server_event.wait_one(zx::Signals::USER_ALL, zx::MonotonicInstant::INFINITE_PAST);
    let signals = assert_matches!(signals, zx::WaitResult::Ok(s) => s);
    assert!(signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::WRITABLE.bits())));

    let signals = client_event.wait_one(zx::Signals::USER_ALL, zx::MonotonicInstant::INFINITE_PAST);
    let signals = assert_matches!(signals, zx::WaitResult::Ok(s) => s);
    assert!(signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::WRITABLE.bits())));

    let result = server.read(10).await.unwrap();
    assert_eq!(result, Err(zx::Status::SHOULD_WAIT.into_raw()));

    let result = client.read(10).await.unwrap();
    assert_eq!(result, Err(zx::Status::SHOULD_WAIT.into_raw()));

    let (status, features) = client.clr_set_feature(0, 0).await.unwrap();
    assert_eq!(zx::Status::ok(status), Ok(()));
    assert_eq!(features, 0);
}

#[fuchsia::test]
async fn server_empty_0_byte_read() {
    let server = setup();
    let _client = open_client(&server, 1).await.unwrap();

    let result = server.read(0).await.unwrap();
    assert_eq!(result, Ok(vec![]));
}

#[fuchsia::test]
async fn client_full_0_byte_server_write() {
    let server = setup();
    let _client = open_client(&server, 1).await.unwrap();

    // Fill up FIFO.
    loop {
        let buf = vec![0; 256];
        let result = server.write(&buf).await.unwrap();
        match result {
            Ok(_) => continue,
            Err(e) => {
                assert_eq!(e, zx::Status::SHOULD_WAIT.into_raw());
                break;
            }
        }
    }

    let result = server.write(&[]).await.unwrap();
    assert_eq!(result, Ok(0));
}

#[fuchsia::test]
async fn client_inactive_0_byte_client_write() {
    let server = setup();
    let _client = open_client(&server, 1).await.unwrap();
    let inactive_client = open_client(&server, 0).await.unwrap();

    let result = inactive_client.write(&[]).await.unwrap();
    assert_eq!(result, Err(zx::Status::SHOULD_WAIT.into_raw()));
}

#[fuchsia::test]
async fn client_describe() {
    let server = setup();
    let client = open_client(&server, 0).await.unwrap();

    let info = client.describe().await.unwrap();
    let event = info.event.unwrap();
    assert!(!event.is_invalid());
}

#[fuchsia::test]
async fn client_window_size() {
    let server = setup();
    let client = open_client(&server, 0).await.unwrap();

    let window_size = WindowSize { width: 80, height: 24 };
    {
        let status = server.set_window_size(&window_size).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
    }
    {
        let (status, size) = client.get_window_size().await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(size, window_size);
    }
    let window_size = WindowSize { width: 5, height: 32 };
    {
        let status = client.set_window_size(&window_size).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
    }
    {
        let (status, size) = client.get_window_size().await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(size, window_size);
    }
}

#[fuchsia::test]
async fn client_clr_set_feature() {
    let server = setup();
    let client = open_client(&server, 0).await.unwrap();

    {
        let (status, features) = client.clr_set_feature(0, 0).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(features, 0);
    }

    // Make sure we can set bits.
    {
        let (status, features) =
            client.clr_set_feature(0, fpty::FEATURE_RAW).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(features, fpty::FEATURE_RAW);
    }

    // If we don't change any bits, we should see the new settings.
    {
        let (status, features) = client.clr_set_feature(0, 0).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(features, fpty::FEATURE_RAW);
    }

    // Make sure we can clear bits.
    {
        let (status, features) =
            client.clr_set_feature(fpty::FEATURE_RAW, 0).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(features, 0);
    }
}

#[fuchsia::test]
async fn client_clr_set_feature_invalid_bit() {
    let server = setup();
    let client = open_client(&server, 0).await.unwrap();

    {
        let (status, features) = client.clr_set_feature(0, 0x2).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Err(zx::Status::NOT_SUPPORTED));
        assert_eq!(features, 0);
    }

    {
        let (status, features) = client.clr_set_feature(0x2, 0).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Err(zx::Status::NOT_SUPPORTED));
        assert_eq!(features, 0);
    }
}

#[fuchsia::test]
async fn client_get_window_size_server_never_set() {
    let server = setup();
    let client = open_client(&server, 0).await.unwrap();

    let (status, size) = client.get_window_size().await.expect("fidl failed");
    assert_eq!(zx::Status::ok(status), Ok(()));
    assert_eq!(size, WindowSize { width: 0, height: 0 });
}

#[fuchsia::test]
async fn client_independent_feature_flags() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();
    let client2 = open_client(&server, 0).await.unwrap();

    {
        let (status, features) =
            client.clr_set_feature(0, fpty::FEATURE_RAW).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(features, fpty::FEATURE_RAW);
    }

    {
        // Client 2 shouldn't see the changes.
        let (status, features) = client2.clr_set_feature(0, 0).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(features, 0);
    }
}

#[fuchsia::test]
async fn client_make_active() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();
    let client2 = open_client(&server, 0).await.unwrap();

    {
        let status = client.make_active(0).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Err(zx::Status::ACCESS_DENIED));
    }
    {
        let status = client2.make_active(1).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
    }
    {
        let status = client2.make_active(1).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
    }
    {
        let status = client2.make_active(0).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
    }
    {
        let status = client2.make_active(2).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Err(zx::Status::NOT_FOUND));
    }
}

#[fuchsia::test]
async fn client_read_events() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();
    let client2 = open_client(&server, 0).await.unwrap();

    {
        let (status, _) = client.read_events().await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Err(zx::Status::ACCESS_DENIED));
    }

    {
        let (status, events) = client2.read_events().await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(events, 0);
    }
}

async fn write_ctrl_c(conn: &DeviceProxy) {
    let data = [0x03];
    let result = conn.write(&data).await.expect("fidl failed");
    assert_eq!(result, Ok(1));
}

#[fuchsia::test]
async fn client_read_events_clears() {
    let server = setup();
    let _active_client = open_client(&server, 1).await.unwrap();
    let control_client = open_client(&server, 0).await.unwrap();

    let control_event = get_event(&control_client).await.unwrap();

    // No events yet.
    let signals = control_event.wait_one(
        zx::Signals::from_bits_truncate(DeviceSignal::OOB.bits()),
        zx::MonotonicInstant::INFINITE_PAST,
    );
    assert_matches!(signals, zx::WaitResult::TimedOut(_));

    write_ctrl_c(&server).await;

    let _ = fasync::OnSignals::new(
        &control_event,
        zx::Signals::from_bits_truncate(DeviceSignal::OOB.bits()),
    )
    .await
    .unwrap();

    {
        let (status, events) = control_client.read_events().await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(events, fpty::EVENT_INTERRUPT);
    }

    // Signal should have cleared.
    let signals = control_event.wait_one(
        zx::Signals::from_bits_truncate(DeviceSignal::OOB.bits()),
        zx::MonotonicInstant::INFINITE_PAST,
    );
    assert_matches!(signals, zx::WaitResult::TimedOut(_));

    // Event should have cleared.
    {
        let (status, events) = control_client.read_events().await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(events, 0);
    }
}

#[fuchsia::test]
async fn events_sent_with_no_controlling_client() {
    let server = setup();
    let _active_client = open_client(&server, 1).await.unwrap();

    write_ctrl_c(&server).await;

    let control_client = open_client(&server, 0).await.unwrap();
    let control_event = get_event(&control_client).await.unwrap();

    let _ = fasync::OnSignals::new(
        &control_event,
        zx::Signals::from_bits_truncate(DeviceSignal::OOB.bits()),
    )
    .await
    .unwrap();

    {
        let (status, events) = control_client.read_events().await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(events, fpty::EVENT_INTERRUPT);
    }
}

#[fuchsia::test]
async fn set_window_size_sends_event() {
    let server = setup();
    let control_client = open_client(&server, 0).await.unwrap();

    let control_event = get_event(&control_client).await.unwrap();

    let signals = control_event.wait_one(
        zx::Signals::from_bits_truncate(DeviceSignal::OOB.bits()),
        zx::MonotonicInstant::INFINITE_PAST,
    );
    assert_matches!(signals, zx::WaitResult::TimedOut(_));

    {
        let (status, events) = control_client.read_events().await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(events, 0);
    }

    {
        let status = server
            .set_window_size(&WindowSize { width: 123, height: 45 })
            .await
            .expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
    }

    let _ = fasync::OnSignals::new(
        &control_event,
        zx::Signals::from_bits_truncate(DeviceSignal::OOB.bits()),
    )
    .await
    .unwrap();

    {
        let (status, events) = control_client.read_events().await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(events, fpty::EVENT_WINDOW_SIZE);
    }
}

#[fuchsia::test]
async fn non_controlling_client_open_client() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();

    let result = open_client(&client, 2).await;
    assert_eq!(result.err(), Some(zx::Status::ACCESS_DENIED));
}

#[fuchsia::test]
async fn controlling_client_open_client() {
    let server = setup();
    let client = open_client(&server, 0).await.unwrap();
    open_client(&client, 1).await.unwrap();
}

#[fuchsia::test]
async fn active_client_closes() {
    let server = setup();
    let control_client = open_client(&server, 0).await.unwrap();
    {
        let _active_client = open_client(&server, 1).await.unwrap();
        let status = control_client.make_active(1).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
    }

    let control_event = get_event(&control_client).await.unwrap();
    let _ = fasync::OnSignals::new(
        &control_event,
        zx::Signals::from_bits_truncate(DeviceSignal::OOB.bits()),
    )
    .await
    .unwrap();

    let signals = control_event
        .wait_one(
            zx::Signals::from_bits_truncate(DeviceSignal::HANGUP.bits()),
            zx::MonotonicInstant::INFINITE_PAST,
        )
        .unwrap();
    assert!(signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::OOB.bits())));
    assert!(signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::HANGUP.bits())));

    let (status, events) = control_client.read_events().await.expect("fidl failed");
    assert_eq!(zx::Status::ok(status), Ok(()));
    assert_eq!(events, fpty::EVENT_HANGUP);
}

#[fuchsia::test]
async fn active_client_closes_when_control() {
    let server = setup();
    {
        let _control_client = open_client(&server, 0).await.unwrap();
    }
    let event = get_event(&server).await.unwrap();
    let _ = fasync::OnSignals::new(
        &event,
        zx::Signals::from_bits_truncate(DeviceSignal::HANGUP.bits()),
    )
    .await
    .unwrap();
}

#[fuchsia::test]
async fn server_closes_when_client_present() {
    let server = setup();
    let client = open_client(&server, 0).await.unwrap();

    let test_data = Vec::from(b"hello world");
    {
        let result = server.write(&test_data).await.expect("fidl failed");
        assert_eq!(result, Ok(test_data.len().try_into().unwrap()));
    }

    drop(server);

    let event = get_event(&client).await.unwrap();
    let _ = fasync::OnSignals::new(
        &event,
        zx::Signals::from_bits_truncate(DeviceSignal::HANGUP.bits()),
    )
    .await
    .unwrap();
    let signals = event
        .wait_one(
            zx::Signals::from_bits_truncate(DeviceSignal::HANGUP.bits()),
            zx::MonotonicInstant::INFINITE_PAST,
        )
        .unwrap();
    assert!(signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::HANGUP.bits())));
    assert!(signals.contains(zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits())));

    {
        let (status, events) = client.read_events().await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(events, fpty::EVENT_HANGUP);
    }

    {
        let result = client.read((test_data.len() + 10) as u64).await.unwrap();
        assert_eq!(result, Ok(test_data));
    }

    {
        let result = client.read(10).await.unwrap();
        assert_eq!(result, Err(zx::Status::PEER_CLOSED.into_raw()));
    }

    {
        let result = client.write(&[0; 16]).await.unwrap();
        assert_eq!(result, Err(zx::Status::PEER_CLOSED.into_raw()));
    }
}

#[fuchsia::test]
async fn server_read_client_cooked() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();

    let test_data = b"hello\x03 world\ntest message\n";
    let expected_readback = Vec::from(b"hello\x03 world\r\ntest message\r\n");

    {
        let result = client.write(test_data).await.expect("fidl failed");
        assert_eq!(result, Ok(test_data.len().try_into().unwrap()));
    }

    let event = get_event(&server).await.unwrap();
    let _ = fasync::OnSignals::new(
        &event,
        zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
    )
    .await
    .unwrap();

    {
        let result = server.read((expected_readback.len() + 10) as u64).await.unwrap();
        assert_eq!(result, Ok(expected_readback));
    }

    let signals = event.wait_one(
        zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
        zx::MonotonicInstant::INFINITE_PAST,
    );
    assert_matches!(signals, zx::WaitResult::TimedOut(_));
}

#[fuchsia::test]
async fn server_write_client_cooked() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();

    let test_data = b"hello world\ntest\x03 message\n";
    let expected_readback = Vec::from(b"hello world\ntest");

    {
        let result = server.write(test_data).await.expect("fidl failed");
        // +1 for ^C.
        assert_eq!(result, Ok((expected_readback.len() + 1).try_into().unwrap()));
    }

    let event = get_event(&client).await.unwrap();
    let _ = fasync::OnSignals::new(
        &event,
        zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
    )
    .await
    .unwrap();

    {
        let result = client.read((expected_readback.len() + 10) as u64).await.unwrap();
        assert_eq!(result, Ok(expected_readback));
    }

    let signals = event.wait_one(
        zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
        zx::MonotonicInstant::INFINITE_PAST,
    );
    assert_matches!(signals, zx::WaitResult::TimedOut(_));
}

#[fuchsia::test]
async fn server_read_client_raw() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();

    {
        let (status, _) = client.clr_set_feature(0, fpty::FEATURE_RAW).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
    }

    let test_data = Vec::from(b"hello\x03 world\ntest message\n");

    {
        let result = client.write(&test_data).await.expect("fidl failed");
        assert_eq!(result, Ok(test_data.len().try_into().unwrap()));
    }

    let event = get_event(&server).await.unwrap();
    let _ = fasync::OnSignals::new(
        &event,
        zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
    )
    .await
    .unwrap();

    {
        let result = server.read((test_data.len() + 10) as u64).await.unwrap();
        assert_eq!(result, Ok(test_data));
    }

    let signals = event.wait_one(
        zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
        zx::MonotonicInstant::INFINITE_PAST,
    );
    assert_matches!(signals, zx::WaitResult::TimedOut(_));
}

#[fuchsia::test]
async fn server_write_client_raw() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();
    let control_client = open_client(&server, 0).await.unwrap();

    {
        let (status, _) = client.clr_set_feature(0, fpty::FEATURE_RAW).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
    }

    let test_data = Vec::from(b"hello world\ntest\x03 message\n");

    {
        let result = server.write(&test_data).await.expect("fidl failed");
        assert_eq!(result, Ok(test_data.len().try_into().unwrap()));
    }

    let event = get_event(&client).await.unwrap();
    let _ = fasync::OnSignals::new(
        &event,
        zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
    )
    .await
    .unwrap();

    {
        let result = client.read((test_data.len() + 10) as u64).await.unwrap();
        assert_eq!(result, Ok(test_data));
    }

    let signals = event.wait_one(
        zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
        zx::MonotonicInstant::INFINITE_PAST,
    );
    assert_matches!(signals, zx::WaitResult::TimedOut(_));

    {
        let (status, events) = control_client.read_events().await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
        assert_eq!(events, 0);
    }
}

#[fuchsia::test]
async fn server_fills_client_fifo() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();

    let server_event = get_event(&server).await.unwrap();
    let client_event = get_event(&client).await.unwrap();

    let test_string = b"abcdefghijklmnopqrstuvwxyz";
    let mut total_written = 0;

    while let zx::WaitResult::Ok(_) = server_event.wait_one(
        zx::Signals::from_bits_truncate(DeviceSignal::WRITABLE.bits()),
        zx::MonotonicInstant::INFINITE_PAST,
    ) {
        let result =
            server.write(&test_string[..test_string.len() - 1]).await.expect("fidl failed");
        let count = result.unwrap();
        assert_gt!(count, 0);
        total_written += count;
    }

    {
        let result = server.write(&test_string[..test_string.len() - 1]).await.unwrap();
        assert_eq!(result, Err(zx::Status::SHOULD_WAIT.into_raw()));
    }

    let mut total_read = 0;
    while total_read < total_written {
        let _ = fasync::OnSignals::new(
            &client_event,
            zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
        )
        .await
        .unwrap();
        let result = client.read((test_string.len() - 1) as u64).await.unwrap();
        let data = result.unwrap();
        let expected_len =
            std::cmp::min((test_string.len() - 1) as u64, total_written - total_read);
        assert_eq!(data.len() as u64, expected_len);
        assert_eq!(data, &test_string[..data.len()]);
        total_read += data.len() as u64;
    }

    let signals = client_event.wait_one(
        zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
        zx::MonotonicInstant::INFINITE_PAST,
    );
    assert_matches!(signals, zx::WaitResult::TimedOut(_));
}

#[fuchsia::test]
async fn client_fills_server_fifo() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();

    let server_event = get_event(&server).await.unwrap();
    let client_event = get_event(&client).await.unwrap();

    let test_string = b"abcdefghijklmnopqrstuvwxyz";
    let mut total_written = 0;

    while let zx::WaitResult::Ok(_) = client_event.wait_one(
        zx::Signals::from_bits_truncate(DeviceSignal::WRITABLE.bits()),
        zx::MonotonicInstant::INFINITE_PAST,
    ) {
        let result =
            client.write(&test_string[..test_string.len() - 1]).await.expect("fidl failed");
        let count = result.unwrap();
        assert_gt!(count, 0);
        total_written += count;
    }

    {
        let result = client.write(&test_string[..test_string.len() - 1]).await.unwrap();
        assert_eq!(result, Err(zx::Status::SHOULD_WAIT.into_raw()));
    }

    let mut total_read = 0;
    while total_read < total_written {
        let _ = fasync::OnSignals::new(
            &server_event,
            zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
        )
        .await
        .unwrap();
        let result = server.read((test_string.len() - 1) as u64).await.unwrap();
        let data = result.unwrap();
        let expected_len =
            std::cmp::min((test_string.len() - 1) as u64, total_written - total_read);
        assert_eq!(data.len() as u64, expected_len);
        assert_eq!(data, &test_string[..data.len()]);
        total_read += data.len() as u64;
    }

    let signals = server_event.wait_one(
        zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
        zx::MonotonicInstant::INFINITE_PAST,
    );
    assert_matches!(signals, zx::WaitResult::TimedOut(_));
}

#[fuchsia::test]
async fn non_active_clients_cant_write() {
    let server = setup();
    let _control_client = open_client(&server, 0).await.unwrap();
    let other_client = open_client(&server, 1).await.unwrap();

    let event = get_event(&other_client).await.unwrap();
    let signals = event.wait_one(zx::Signals::USER_ALL, zx::MonotonicInstant::INFINITE_PAST);
    match signals {
        zx::WaitResult::Ok(s) => {
            assert!(s.contains(zx::Signals::from_bits_truncate(DeviceSignal::WRITABLE.bits())))
        }
        zx::WaitResult::TimedOut(_) => {}
        _ => panic!("wait failed"),
    }

    {
        let result = other_client.write(&[0]).await.unwrap();
        assert_eq!(result, Err(zx::Status::SHOULD_WAIT.into_raw()));
    }
}

#[fuchsia::test]
async fn clients_have_independent_fifos() {
    let server = setup();
    let control_client = open_client(&server, 0).await.unwrap();
    let other_client = open_client(&server, 1).await.unwrap();

    let control_client_byte = 1;
    let other_client_byte = 2;

    {
        let result = server.write(&[control_client_byte]).await.expect("fidl failed");
        assert_eq!(result, Ok(1));
    }

    {
        let status = control_client.make_active(1).await.expect("fidl failed");
        assert_eq!(zx::Status::ok(status), Ok(()));
    }

    {
        let result = server.write(&[other_client_byte]).await.expect("fidl failed");
        assert_eq!(result, Ok(1));
    }

    let check_client = |client: DeviceProxy, expected_value: u8| async move {
        let event = get_event(&client).await.unwrap();
        let _ = fasync::OnSignals::new(
            &event,
            zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
        )
        .await
        .unwrap();

        let result = client.read(10).await.unwrap();
        assert_eq!(result, Ok(vec![expected_value]));

        let signals = event.wait_one(
            zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
            zx::MonotonicInstant::INFINITE_PAST,
        );
        assert_matches!(signals, zx::WaitResult::TimedOut(_));
    };

    check_client(other_client, other_client_byte).await;
    check_client(control_client, control_client_byte).await;
}

#[fuchsia::test]
async fn client_write_cooked_partial_when_fifo_full() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();

    // When data fills the FIFO such that lines fit up to the FIFO boundary,
    // client.write must return Ok(total_written) for the lines written rather than
    // returning Err(SHOULD_WAIT) and discarding the already-buffered bytes.
    let line = b"123456789012345678901234567890\n"; // 31 bytes, expands to 32 bytes
    let mut data = Vec::new();
    for _ in 0..130 {
        // 128 lines fit (4096 bytes), line 129 cannot fit.
        data.extend_from_slice(line);
    }

    let result = client.write(&data).await.expect("fidl failed");
    let written =
        result.expect("client.write should return Ok(written) when lines fit up to FIFO boundary");
    assert_eq!(written, 128 * 31);
}

#[fuchsia::test]
async fn client_write_cooked_multi_chunk_spanning_fifo() {
    let server = setup();
    let client = open_client(&server, 1).await.unwrap();
    let server_event = get_event(&server).await.unwrap();
    let client_event = get_event(&client).await.unwrap();

    let line = b"123456789012345678901234567890\n"; // 31 bytes, expands to 32 bytes (\r\n)
    let total_lines = 300; // 300 lines = 9300 bytes of input, 9600 bytes in FIFO (> 2 full FIFOs)
    let mut data = Vec::new();
    for _ in 0..total_lines {
        data.extend_from_slice(line);
    }

    // Client task writes all data in chunks, waiting on WRITABLE when needed.
    let client_task = fasync::Task::local(async move {
        let mut total_written = 0;
        while total_written < data.len() {
            let chunk_end = std::cmp::min(data.len(), total_written + 4096);
            match client.write(&data[total_written..chunk_end]).await.unwrap() {
                Ok(n) => {
                    assert_gt!(n, 0);
                    total_written += n as usize;
                }
                Err(e) => {
                    assert_eq!(e, zx::Status::SHOULD_WAIT.into_raw());
                    let _ = fasync::OnSignals::new(
                        &client_event,
                        zx::Signals::from_bits_truncate(DeviceSignal::WRITABLE.bits()),
                    )
                    .await
                    .unwrap();
                }
            }
        }
    });

    // Server reads back all data.
    let expected_output: Vec<u8> = (0..total_lines)
        .flat_map(|_| b"123456789012345678901234567890\r\n".iter().copied())
        .collect();

    let mut received = Vec::new();
    while received.len() < expected_output.len() {
        let _ = fasync::OnSignals::new(
            &server_event,
            zx::Signals::from_bits_truncate(DeviceSignal::READABLE.bits()),
        )
        .await
        .unwrap();
        let chunk = server.read(1024).await.unwrap().unwrap();
        received.extend_from_slice(&chunk);
    }

    client_task.await;
    assert_eq!(received, expected_output);
}
