// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fidl::endpoints::{RequestStream, create_endpoints};

const TEST_EP_IN_ADDR: u8 = 0x81;
const TEST_EP_OUT_ADDR: u8 = 0x01;

const USB_DIR_OUT: u8 = 0x00;
const USB_DIR_IN: u8 = 0x80;
const USB_RECIP_DEVICE: u8 = 0x00;

const USB_TYPE_VENDOR_OUT: u8 = USB_DIR_OUT | USB_TYPE_VENDOR | USB_RECIP_DEVICE;
const USB_TYPE_VENDOR_IN: u8 = USB_DIR_IN | USB_TYPE_VENDOR | USB_RECIP_DEVICE;

const USB_REQ_STANDARD_DEVICE_OUT: u8 = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE;
const USB_REQ_STANDARD_DEVICE_IN: u8 = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE;
const USB_REQ_STANDARD_INTERFACE_IN: u8 = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_INTERFACE;
const USB_REQ_STANDARD_ENDPOINT_IN: u8 = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT;
const USB_REQ_STANDARD_ENDPOINT_OUT: u8 = USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_ENDPOINT;

use futures::channel::mpsc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, PartialEq)]
enum MockEvent {
    VmoRegistered,
    RequestQueued,
}

#[derive(Default)]
struct MockEndpointState {
    requests: Vec<fusb_request::Request>,
    vmos: HashMap<u64, zx::Vmo>,
}

async fn run_mock_endpoint(
    mut stream: fusb_endpoint::EndpointRequestStream,
    state: Arc<Mutex<MockEndpointState>>,
    mut completion_rx: mpsc::UnboundedReceiver<Vec<fusb_endpoint::Completion>>,
    event_tx: mpsc::UnboundedSender<MockEvent>,
    scope: Arc<fasync::Scope>,
) {
    let control_handle = stream.control_handle();

    // Spawn task to handle completions
    let ch = control_handle.clone();
    scope.spawn_local(async move {
        while let Some(completion) = completion_rx.next().await {
            let _ = ch.send_on_completion(completion);
        }
    });

    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fusb_endpoint::EndpointRequest::RegisterVmos { vmo_ids, responder } => {
                let mut vmos = vec![];
                let mut state_lock = state.lock().unwrap();
                for info in vmo_ids {
                    let id = info.id.unwrap();
                    let size = info.size.unwrap();
                    let vmo = zx::Vmo::create(size).unwrap();
                    let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
                    state_lock.vmos.insert(id, vmo);
                    vmos.push(fusb_endpoint::VmoHandle {
                        id: Some(id),
                        vmo: Some(dup),
                        ..Default::default()
                    });
                }
                let _ = responder.send(vmos);
                let _ = event_tx.unbounded_send(MockEvent::VmoRegistered);
            }
            fusb_endpoint::EndpointRequest::QueueRequests { req, control_handle: _ } => {
                let mut state_lock = state.lock().unwrap();
                state_lock.requests.extend(req);
                let _ = event_tx.unbounded_send(MockEvent::RequestQueued);
            }
            fusb_endpoint::EndpointRequest::UnregisterVmos { vmo_ids, responder } => {
                let mut state_lock = state.lock().unwrap();
                for id in vmo_ids {
                    state_lock.vmos.remove(&id);
                }
                let _ = responder.send(&[], &[]);
            }
            _ => {}
        }
    }
}

async fn run_mock_function(mut stream: fusb_function::UsbFunctionRequestStream) {
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fusb_function::UsbFunctionRequest::ConfigureEndpoint { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            fusb_function::UsbFunctionRequest::DisableEndpoint { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            fusb_function::UsbFunctionRequest::EndpointSetStall { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            fusb_function::UsbFunctionRequest::EndpointClearStall { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            fusb_function::UsbFunctionRequest::ConnectToEndpoint { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            fusb_function::UsbFunctionRequest::Deconfigure { responder } => {
                let _ = responder.send(Ok(()));
            }
            _ => {}
        }
    }
}
#[fuchsia::test]
async fn test_loopback() {
    let (ep_in_client, ep_in_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let (ep_out_client, ep_out_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();

    let state_in =
        Arc::new(Mutex::new(MockEndpointState { requests: vec![], vmos: HashMap::new() }));
    let state_out =
        Arc::new(Mutex::new(MockEndpointState { requests: vec![], vmos: HashMap::new() }));

    let (comp_in_tx, comp_in_rx) = mpsc::unbounded();
    let (comp_out_tx, comp_out_rx) = mpsc::unbounded();
    let (event_in_tx, mut event_in_rx) = mpsc::unbounded();
    let (event_out_tx, mut event_out_rx) = mpsc::unbounded();

    let scope = Arc::new(fasync::Scope::new_with_name("test"));

    scope.spawn_local(run_mock_endpoint(
        ep_in_server.into_stream(),
        state_in.clone(),
        comp_in_rx,
        event_in_tx,
        scope.clone(),
    ));
    scope.spawn_local(run_mock_endpoint(
        ep_out_server.into_stream(),
        state_out.clone(),
        comp_out_rx,
        event_out_tx,
        scope.clone(),
    ));

    let ep_in_proxy = ep_in_client.into_proxy();
    let ep_out_proxy = ep_out_client.into_proxy();

    let mut vmos_registered = false;
    let _tasks = run_loopback(
        ep_in_proxy,
        ep_out_proxy,
        &mut vmos_registered,
        u64::from(USB_MAX_PACKET_SIZE_HIGH_SPEED),
    )
    .await
    .unwrap();

    // Await setup events: ep_out registered (1), ep_in registered (1),
    // and ep_out queued read request (1).
    assert_eq!(event_out_rx.next().await, Some(MockEvent::VmoRegistered));
    assert_eq!(event_out_rx.next().await, Some(MockEvent::RequestQueued));
    assert_eq!(event_in_rx.next().await, Some(MockEvent::VmoRegistered));

    // Verify OUT VMO was registered
    let vmo_out = {
        let state = state_out.lock().unwrap();
        state
            .vmos
            .get(&USB_ZERO_OUT_VMO_ID)
            .unwrap()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .unwrap()
    };

    // Verify IN VMO was registered
    let vmo_in = {
        let state = state_in.lock().unwrap();
        state
            .vmos
            .get(&USB_ZERO_IN_VMO_ID)
            .unwrap()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .unwrap()
    };

    // Verify read request was queued (request 0 maps to USB_ZERO_OUT_VMO_ID)
    let read_req = {
        let mut state = state_out.lock().unwrap();
        state.requests.remove(0)
    };

    // Fill VMO with some data
    let test_data = vec![1, 2, 3, 4, 5];
    vmo_out.write(&test_data, 0).unwrap();

    // Complete read
    comp_out_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(read_req),
            status: Some(zx::sys::ZX_OK),
            transfer_size: Some(test_data.len() as u64),
            ..Default::default()
        }])
        .unwrap();

    // Wait for loopback to process and queue write request on ep_in
    assert_eq!(event_in_rx.next().await, Some(MockEvent::RequestQueued));

    // Verify write request was queued on ep_in
    let write_req = {
        let mut state = state_in.lock().unwrap();
        state.requests.pop().unwrap()
    };

    // Complete write
    comp_in_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(write_req),
            status: Some(zx::sys::ZX_OK),
            transfer_size: Some(test_data.len() as u64),
            ..Default::default()
        }])
        .unwrap();

    // Verify data in VMO IN
    let mut read_back = vec![0; test_data.len()];
    vmo_in.read(&mut read_back, 0).unwrap();
    assert_eq!(read_back, test_data);

    // Verify Zero-Length Packet (ZLP) through loopback
    let zlp_read_req = {
        let mut state = state_out.lock().unwrap();
        state.requests.remove(0)
    };
    comp_out_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(zlp_read_req),
            status: Some(zx::sys::ZX_OK),
            transfer_size: Some(0),
            ..Default::default()
        }])
        .unwrap();

    assert_eq!(event_in_rx.next().await, Some(MockEvent::RequestQueued));
    let zlp_write_req = {
        let mut state = state_in.lock().unwrap();
        let req = state.requests.pop().unwrap();
        let len = req
            .data
            .as_ref()
            .and_then(|d| d.first())
            .and_then(|b| match b.buffer.as_ref() {
                Some(fusb_request::Buffer::VmoId(_)) => Some(b.size.unwrap_or(0)),
                _ => None,
            })
            .unwrap_or(0);
        assert_eq!(len, 0);
        req
    };
    comp_in_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(zlp_write_req),
            status: Some(zx::sys::ZX_OK),
            transfer_size: Some(0),
            ..Default::default()
        }])
        .unwrap();
}

#[fuchsia::test]
async fn test_vendor_requests() {
    let (iface_client, iface_server) =
        create_endpoints::<fusb_function::UsbFunctionInterfaceMarker>();
    let (func_client, func_server) = create_endpoints::<fusb_function::UsbFunctionMarker>();
    let (ep_in_client, _ep_in_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let (ep_out_client, _ep_out_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();

    let scope = Arc::new(fasync::Scope::new_with_name("test_vendor"));
    scope.spawn_local(run_mock_function(func_server.into_stream()));
    let func_client_proxy = func_client.into_proxy();
    let ep_in_proxy = ep_in_client.into_proxy();
    let ep_out_proxy = ep_out_client.into_proxy();
    scope.spawn_local(async move {
        let mut zero_function = UsbZeroFunctionDevice::new(
            func_client_proxy,
            ep_in_proxy,
            TEST_EP_IN_ADDR,
            ep_out_proxy,
            TEST_EP_OUT_ADDR,
            0,
            TestMode::SourceSink,
        );
        zero_function.handle_requests(iface_server.into_stream()).await;
    });

    let proxy = iface_client.into_proxy();

    // Test VendorRequest::SetStall (0x50)
    let setup_set_stall = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::SetStall as u8,
        w_value: TEST_EP_IN_ADDR as u16,
        w_index: 0,
        w_length: 0,
    };
    let res = proxy.control(&setup_set_stall, &[]).await.unwrap();
    assert_eq!(res, Ok(vec![]));

    // Test VendorRequest::SetStall with invalid w_value (> 0xFF)
    let setup_invalid_w_value = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::SetStall as u8,
        w_value: 0x100,
        w_index: 0,
        w_length: 0,
    };
    let res = proxy.control(&setup_invalid_w_value, &[]).await.unwrap();
    assert_eq!(res, Err(Status::INVALID_ARGS.into_raw()));

    // Test VendorRequest::SetStall with invalid direction bit (0xC0)
    let setup_invalid_dir = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_IN,
        b_request: VendorRequest::SetStall as u8,
        w_value: TEST_EP_IN_ADDR as u16,
        w_index: 0,
        w_length: 0,
    };
    let res = proxy.control(&setup_invalid_dir, &[]).await.unwrap();
    assert_eq!(res, Err(Status::INVALID_ARGS.into_raw()));

    // Test VendorRequest::ClearStall (0x51)
    let setup_clear_stall = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::ClearStall as u8,
        w_value: TEST_EP_IN_ADDR as u16,
        w_index: 0,
        w_length: 0,
    };
    let res = proxy.control(&setup_clear_stall, &[]).await.unwrap();
    assert_eq!(res, Ok(vec![]));

    // Test VendorRequest::ConfigureEndpoint (0x52)
    let setup_config_ep = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::ConfigureEndpoint as u8,
        w_value: TEST_EP_IN_ADDR as u16,
        w_index: 0,
        w_length: 0,
    };
    let res = proxy.control(&setup_config_ep, &[]).await.unwrap();
    assert_eq!(res, Ok(vec![]));

    // Test VendorRequest::DisableEndpoint (0x53)
    let setup_disable_ep = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::DisableEndpoint as u8,
        w_value: TEST_EP_IN_ADDR as u16,
        w_index: 0,
        w_length: 0,
    };
    let res = proxy.control(&setup_disable_ep, &[]).await.unwrap();
    assert_eq!(res, Ok(vec![]));

    // Test VendorRequest::ConnectEndpoint (0x54)
    let setup_connect_ep = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::ConnectEndpoint as u8,
        w_value: TEST_EP_IN_ADDR as u16,
        w_index: 0,
        w_length: 0,
    };
    let res = proxy.control(&setup_connect_ep, &[]).await.unwrap();
    assert_eq!(res, Ok(vec![]));

    // Test VendorRequest::Deconfigure (0x55)
    let setup_deconfig = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::Deconfigure as u8,
        w_value: 0,
        w_index: 0,
        w_length: 0,
    };
    let res_deconfig = proxy.control(&setup_deconfig, &[]).await.unwrap();
    assert_eq!(res_deconfig, Ok(vec![]));

    // Test VendorRequest::WritePayload (0x56 - valid data)
    let setup_write = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::WritePayload as u8,
        w_value: 0,
        w_index: 0,
        w_length: USB_ZERO_WRITE_PAYLOAD.len() as u16,
    };
    let res_out = proxy.control(&setup_write, USB_ZERO_WRITE_PAYLOAD).await.unwrap();
    assert_eq!(res_out, Ok(vec![]));

    // Test VendorRequest::WritePayload (0x56 - invalid payload content)
    let res_err =
        proxy.control(&setup_write, &vec![0; USB_ZERO_WRITE_PAYLOAD.len()]).await.unwrap();
    assert_eq!(res_err, Err(Status::INVALID_ARGS.into_raw()));

    // Test VendorRequest::WritePayload (0x56 - w_length mismatch)
    let setup_write_mismatch = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::WritePayload as u8,
        w_value: 0,
        w_index: 0,
        w_length: (USB_ZERO_WRITE_PAYLOAD.len() + 1) as u16,
    };
    let res_mismatch = proxy.control(&setup_write_mismatch, USB_ZERO_WRITE_PAYLOAD).await.unwrap();
    assert_eq!(res_mismatch, Err(Status::INVALID_ARGS.into_raw()));

    // Test VendorRequest::ReadPayload (0x57 - valid)
    let setup_read = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_IN,
        b_request: VendorRequest::ReadPayload as u8,
        w_value: 0,
        w_index: 0,
        w_length: USB_ZERO_READ_PAYLOAD.len() as u16,
    };
    let res = proxy.control(&setup_read, &[]).await.unwrap();
    assert_eq!(res, Ok(USB_ZERO_READ_PAYLOAD.to_vec()));

    // Test VendorRequest::ReadPayload (0x57 - invalid non-empty write payload)
    let res_read_nonempty = proxy.control(&setup_read, &[0x01]).await.unwrap();
    assert_eq!(res_read_nonempty, Err(Status::INVALID_ARGS.into_raw()));

    // Test VendorRequest::SetTestMode (0x58 - rejected for now, mode is fixed per configuration)
    let setup_set_mode_loopback = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::SetTestMode as u8,
        w_value: TestMode::Loopback as u16,
        w_index: 0,
        w_length: 0,
    };
    let res_set_mode = proxy.control(&setup_set_mode_loopback, &[]).await.unwrap();
    assert_eq!(res_set_mode, Err(Status::NOT_SUPPORTED.into_raw()));

    // Test VendorRequest::ControlLoopbackOut (0x5c) and ControlLoopbackIn (0x5b)
    let payload = vec![1, 2, 3, 4, 5];
    let setup_control_loopback_out = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::ControlLoopbackOut as u8,
        w_value: 0,
        w_index: 0,
        w_length: payload.len() as u16,
    };
    let res_cl_out = proxy.control(&setup_control_loopback_out, &payload).await.unwrap();
    assert_eq!(res_cl_out, Ok(vec![]));

    let setup_control_loopback_in = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_IN,
        b_request: VendorRequest::ControlLoopbackIn as u8,
        w_value: 0,
        w_index: 0,
        w_length: payload.len() as u16,
    };
    let res_cl_in = proxy.control(&setup_control_loopback_in, &[]).await.unwrap();
    assert_eq!(res_cl_in, Ok(payload));

    // Test VendorRequest::GetTestMode (0x59)
    let setup_get_test_mode = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_IN,
        b_request: VendorRequest::GetTestMode as u8,
        w_value: 0,
        w_index: 0,
        w_length: 1,
    };
    let res_get_mode = proxy.control(&setup_get_test_mode, &[]).await.unwrap();
    assert_eq!(res_get_mode, Ok(vec![TestMode::default() as u8]));

    // Test invalid vendor request (opcode 0x00 with bm_request_type = 0x40)
    let setup_invalid_vendor = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: 0x00,
        w_value: 0,
        w_index: 0,
        w_length: 0,
    };
    let res_invalid_vendor = proxy.control(&setup_invalid_vendor, &[]).await.unwrap();
    assert_eq!(res_invalid_vendor, Err(Status::NOT_SUPPORTED.into_raw()));

    // Test unsupported request
    let setup_unsupported = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: 0xff,
        w_value: 0,
        w_index: 0,
        w_length: 0,
    };
    let res_unsupported = proxy.control(&setup_unsupported, &[]).await.unwrap();
    assert_eq!(res_unsupported, Err(Status::NOT_SUPPORTED.into_raw()));
}

#[fuchsia::test]
async fn test_source_sink() {
    let (ep_in_client, ep_in_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let (ep_out_client, ep_out_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();

    let state_in =
        Arc::new(Mutex::new(MockEndpointState { requests: vec![], vmos: HashMap::new() }));
    let state_out =
        Arc::new(Mutex::new(MockEndpointState { requests: vec![], vmos: HashMap::new() }));

    let (comp_in_tx, comp_in_rx) = mpsc::unbounded();
    let (comp_out_tx, comp_out_rx) = mpsc::unbounded();
    let (event_tx, mut event_rx) = mpsc::unbounded();

    let scope = Arc::new(fasync::Scope::new_with_name("test_ss"));

    scope.spawn_local(run_mock_endpoint(
        ep_in_server.into_stream(),
        state_in.clone(),
        comp_in_rx,
        event_tx.clone(),
        scope.clone(),
    ));
    scope.spawn_local(run_mock_endpoint(
        ep_out_server.into_stream(),
        state_out.clone(),
        comp_out_rx,
        event_tx,
        scope.clone(),
    ));

    let ep_in_proxy = ep_in_client.into_proxy();
    let ep_out_proxy = ep_out_client.into_proxy();

    let mut vmos_registered = false;
    let _tasks = run_source_sink(
        ep_in_proxy,
        ep_out_proxy,
        &mut vmos_registered,
        USB_MAX_PACKET_SIZE_HIGH_SPEED.into(),
    )
    .await
    .unwrap();

    // Await setup events: ep_out registered (1), ep_in registered (1),
    // ep_out queued initial request (1), ep_in queued initial request (1)
    for _ in 0..4 {
        let _ = event_rx.next().await;
    }

    // Verify OUT VMO was registered
    let read_req = {
        let mut state = state_out.lock().unwrap();
        assert!(state.vmos.contains_key(&USB_ZERO_OUT_VMO_ID));
        state.requests.pop().unwrap()
    };

    // Verify IN VMO was registered
    let write_req = {
        let mut state = state_in.lock().unwrap();
        assert!(state.vmos.contains_key(&USB_ZERO_IN_VMO_ID));
        state.requests.pop().unwrap()
    };

    // Send mock completion on OUT to assert read loop re-queues
    comp_out_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(read_req),
            status: Some(zx::sys::ZX_OK),
            transfer_size: Some(0),
            ..Default::default()
        }])
        .unwrap();

    let event = event_rx.next().await;
    assert_eq!(event, Some(MockEvent::RequestQueued));
    {
        let state = state_out.lock().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH);
    }

    // Send mock completion on IN to assert write loop re-queues
    comp_in_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(write_req),
            status: Some(zx::sys::ZX_OK),
            transfer_size: Some(512),
            ..Default::default()
        }])
        .unwrap();

    let event = event_rx.next().await;
    assert_eq!(event, Some(MockEvent::RequestQueued));
    {
        let state = state_in.lock().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH);
    }
}

#[fuchsia::test]
async fn test_set_and_get_interface() {
    let (iface_c, iface_s) = create_endpoints::<fusb_function::UsbFunctionInterfaceMarker>();
    let (func_c, func_s) = create_endpoints::<fusb_function::UsbFunctionMarker>();
    let (ep_in_c, ep_in_s) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let (ep_out_c, ep_out_s) = create_endpoints::<fusb_endpoint::EndpointMarker>();

    let scope = Arc::new(fasync::Scope::new_with_name("test_set_get_iface"));
    scope.spawn_local(run_mock_function(func_s.into_stream()));
    scope.spawn_local(run_mock_endpoint(
        ep_in_s.into_stream(),
        Default::default(),
        mpsc::unbounded().1,
        mpsc::unbounded().0,
        scope.clone(),
    ));
    scope.spawn_local(run_mock_endpoint(
        ep_out_s.into_stream(),
        Default::default(),
        mpsc::unbounded().1,
        mpsc::unbounded().0,
        scope.clone(),
    ));

    let f_p = func_c.into_proxy();
    let ep_i = ep_in_c.into_proxy();
    let ep_o = ep_out_c.into_proxy();
    scope.spawn_local(async move {
        UsbZeroFunctionDevice::new(
            f_p,
            ep_i,
            TEST_EP_IN_ADDR,
            ep_o,
            TEST_EP_OUT_ADDR,
            0,
            TestMode::SourceSink,
        )
        .handle_requests(iface_s.into_stream())
        .await;
    });

    let proxy = iface_c.into_proxy();
    let setup = fusb_descriptor::UsbSetup {
        bm_request_type: 0x81,
        b_request: USB_SETUP_REQ_GET_INTERFACE,
        w_value: 0,
        w_index: 0,
        w_length: 1,
    };

    // GetInterface initially returns alt 0
    assert_eq!(proxy.control(&setup, &[]).await.unwrap(), Ok(vec![0x00]));

    // Alternate setting 0 succeeds and resets endpoints
    assert_eq!(proxy.set_interface(0, 0).await.unwrap(), Ok(()));
    assert_eq!(proxy.control(&setup, &[]).await.unwrap(), Ok(vec![0x00]));

    // Configure device, alternate setting 0 still succeeds
    assert_eq!(proxy.set_configured(true, fusb_descriptor::UsbSpeed::High).await.unwrap(), Ok(()));
    assert_eq!(proxy.set_interface(0, 0).await.unwrap(), Ok(()));
    assert_eq!(proxy.control(&setup, &[]).await.unwrap(), Ok(vec![0x00]));

    // Alternate settings > 0 are rejected.
    assert_eq!(proxy.set_interface(0, 1).await.unwrap(), Err(Status::NOT_SUPPORTED.into_raw()));
    assert_eq!(proxy.set_interface(0, 2).await.unwrap(), Err(Status::NOT_SUPPORTED.into_raw()));
    assert_eq!(proxy.set_interface(1, 0).await.unwrap(), Err(Status::NOT_SUPPORTED.into_raw()));
}

#[fuchsia::test]
async fn test_endpoint_stall_state() {
    let (func_client, func_server) = create_endpoints::<fusb_function::UsbFunctionMarker>();
    let (ep_in_client, _ep_in_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let (ep_out_client, _ep_out_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let scope = Arc::new(fasync::Scope::new_with_name("test_stall"));
    scope.spawn_local(run_mock_function(func_server.into_stream()));

    let mut device = UsbZeroFunctionDevice::new(
        func_client.into_proxy(),
        ep_in_client.into_proxy(),
        TEST_EP_IN_ADDR,
        ep_out_client.into_proxy(),
        TEST_EP_OUT_ADDR,
        0,
        TestMode::SourceSink,
    );

    // Initial state: no stalled endpoints
    assert!(device.stalled_endpoints.is_empty());

    // Stall IN endpoint
    device.set_endpoint_stall(TEST_EP_IN_ADDR).await.unwrap();
    assert!(device.stalled_endpoints.contains(&TEST_EP_IN_ADDR));
    assert_eq!(device.stalled_endpoints.len(), 1);

    // Stall OUT endpoint
    device.set_endpoint_stall(TEST_EP_OUT_ADDR).await.unwrap();
    assert!(device.stalled_endpoints.contains(&TEST_EP_IN_ADDR));
    assert!(device.stalled_endpoints.contains(&TEST_EP_OUT_ADDR));
    assert_eq!(device.stalled_endpoints.len(), 2);

    // Stall EP0 (invalid)
    assert_eq!(device.set_endpoint_stall(0).await, Err(Status::INVALID_ARGS));
    assert_eq!(device.set_endpoint_stall(0x80).await, Err(Status::INVALID_ARGS));

    // Clear IN endpoint stall
    device.clear_endpoint_stall(TEST_EP_IN_ADDR).await.unwrap();
    assert!(!device.stalled_endpoints.contains(&TEST_EP_IN_ADDR));
    assert!(device.stalled_endpoints.contains(&TEST_EP_OUT_ADDR));
    assert_eq!(device.stalled_endpoints.len(), 1);

    // Clear EP0 stall (should be no-op and succeed)
    device.clear_endpoint_stall(0).await.unwrap();
    device.clear_endpoint_stall(0x80).await.unwrap();
    assert!(device.stalled_endpoints.contains(&TEST_EP_OUT_ADDR));
    assert_eq!(device.stalled_endpoints.len(), 1);

    // Clear OUT endpoint stall
    device.clear_endpoint_stall(TEST_EP_OUT_ADDR).await.unwrap();
    assert!(device.stalled_endpoints.is_empty());
}

#[fuchsia::test]
async fn test_standard_endpoint_halt() {
    let in_ep = TEST_EP_IN_ADDR as u16;
    let (iface_c, iface_s) = create_endpoints::<fusb_function::UsbFunctionInterfaceMarker>();
    let (func_c, func_s) = create_endpoints::<fusb_function::UsbFunctionMarker>();
    let (ep_in_c, _) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let (ep_out_c, _) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let scope = Arc::new(fasync::Scope::new_with_name("test_halt"));
    scope.spawn_local(run_mock_function(func_s.into_stream()));
    let mut dev = UsbZeroFunctionDevice::new(
        func_c.into_proxy(),
        ep_in_c.into_proxy(),
        TEST_EP_IN_ADDR,
        ep_out_c.into_proxy(),
        TEST_EP_OUT_ADDR,
        0, // interface_num
        TestMode::SourceSink,
    );

    // EP0 (Control Endpoint) stall management is handled by hardware / driver stack
    // and cannot be stalled via this vendor request. Return INVALID_ARGS for EP0.
    assert_eq!(dev.set_endpoint_stall(0).await, Err(Status::INVALID_ARGS));
    // Clearing stall on EP0 is a no-op because EP0 stall status automatically resets upon the next setup packet.
    assert_eq!(dev.clear_endpoint_stall(0).await, Ok(()));

    scope.spawn_local(async move {
        dev.handle_requests(iface_s.into_stream()).await;
    });

    let proxy = iface_c.into_proxy();
    macro_rules! control {
        ($setup:expr) => {
            proxy.control(&$setup, &[]).await.unwrap()
        };
    }

    let get_status = |bm, idx| fusb_descriptor::UsbSetup {
        bm_request_type: bm,
        b_request: USB_SETUP_REQ_GET_STATUS,
        w_value: 0,
        w_index: idx,
        w_length: 2,
    };
    let set_halt = |bm, val, idx| fusb_descriptor::UsbSetup {
        bm_request_type: bm,
        b_request: USB_SETUP_REQ_SET_FEATURE,
        w_value: val,
        w_index: idx,
        w_length: 0,
    };
    let clear_halt = |bm, val, idx| fusb_descriptor::UsbSetup {
        bm_request_type: bm,
        b_request: USB_SETUP_REQ_CLEAR_FEATURE,
        w_value: val,
        w_index: idx,
        w_length: 0,
    };

    // GET_STATUS on device (0), interface (0), and unhalted endpoint (0)
    assert_eq!(control!(get_status(USB_REQ_STANDARD_DEVICE_IN, 0)), Ok(vec![0, 0]));
    assert_eq!(control!(get_status(USB_REQ_STANDARD_INTERFACE_IN, 0)), Ok(vec![0, 0]));
    assert_eq!(
        control!(get_status(USB_REQ_STANDARD_INTERFACE_IN, 1)),
        Err(Status::NOT_SUPPORTED.into_raw())
    );
    assert_eq!(control!(get_status(USB_REQ_STANDARD_ENDPOINT_IN, in_ep)), Ok(vec![0, 0]));

    // SET_FEATURE(ENDPOINT_HALT) -> GET_STATUS (1)
    assert_eq!(
        control!(set_halt(USB_REQ_STANDARD_ENDPOINT_OUT, USB_FEATURE_ENDPOINT_HALT, in_ep)),
        Ok(vec![])
    );
    assert_eq!(control!(get_status(USB_REQ_STANDARD_ENDPOINT_IN, in_ep)), Ok(vec![1, 0]));

    // CLEAR_FEATURE(ENDPOINT_HALT) -> GET_STATUS (0)
    assert_eq!(
        control!(clear_halt(USB_REQ_STANDARD_ENDPOINT_OUT, USB_FEATURE_ENDPOINT_HALT, in_ep)),
        Ok(vec![])
    );
    assert_eq!(control!(get_status(USB_REQ_STANDARD_ENDPOINT_IN, in_ep)), Ok(vec![0, 0]));

    // Negative validations: invalid feature, recipient, endpoint address, direction
    assert_eq!(
        control!(set_halt(USB_REQ_STANDARD_ENDPOINT_OUT, 1, in_ep)),
        Err(Status::NOT_SUPPORTED.into_raw())
    );
    assert_eq!(
        control!(set_halt(USB_REQ_STANDARD_DEVICE_OUT, USB_FEATURE_ENDPOINT_HALT, in_ep)),
        Err(Status::NOT_SUPPORTED.into_raw())
    );
    assert_eq!(
        control!(set_halt(USB_REQ_STANDARD_ENDPOINT_OUT, USB_FEATURE_ENDPOINT_HALT, 99)),
        Err(Status::NOT_SUPPORTED.into_raw())
    );
    assert_eq!(
        control!(set_halt(USB_REQ_STANDARD_ENDPOINT_IN, USB_FEATURE_ENDPOINT_HALT, in_ep)),
        Err(Status::NOT_SUPPORTED.into_raw())
    );
}

#[fuchsia::test]
fn test_get_usb_protocol_parsing() {
    use fidl_fuchsia_driver_framework as fdf;

    let start_args_sourcesink = fdf::DriverStartArgs {
        node_properties_2: Some(vec![fdf::NodePropertyEntry2 {
            name: "default".to_string(),
            properties: vec![fdf::NodeProperty2 {
                key: super::BIND_USB_PROTOCOL_KEY.to_string(),
                value: fdf::NodePropertyValue::IntValue(1),
            }],
        }]),
        ..Default::default()
    };
    assert_eq!(get_usb_protocol(&start_args_sourcesink), Some(1));

    let start_args_loopback = fdf::DriverStartArgs {
        node_properties_2: Some(vec![fdf::NodePropertyEntry2 {
            name: "default".to_string(),
            properties: vec![fdf::NodeProperty2 {
                key: super::BIND_USB_PROTOCOL_KEY.to_string(),
                value: fdf::NodePropertyValue::IntValue(2),
            }],
        }]),
        ..Default::default()
    };
    assert_eq!(get_usb_protocol(&start_args_loopback), Some(2));

    let start_args_empty = fdf::DriverStartArgs::default();
    assert_eq!(get_usb_protocol(&start_args_empty), None);
}

#[fuchsia::test]
async fn test_loopback_mode_and_set_interface() {
    let (iface_c, iface_s) = create_endpoints::<fusb_function::UsbFunctionInterfaceMarker>();
    let (func_c, func_s) = create_endpoints::<fusb_function::UsbFunctionMarker>();
    let (ep_in_c, ep_in_s) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let (ep_out_c, ep_out_s) = create_endpoints::<fusb_endpoint::EndpointMarker>();

    let scope = Arc::new(fasync::Scope::new_with_name("test_loopback_mode"));
    scope.spawn_local(run_mock_function(func_s.into_stream()));
    scope.spawn_local(run_mock_endpoint(
        ep_in_s.into_stream(),
        Default::default(),
        mpsc::unbounded().1,
        mpsc::unbounded().0,
        scope.clone(),
    ));
    scope.spawn_local(run_mock_endpoint(
        ep_out_s.into_stream(),
        Default::default(),
        mpsc::unbounded().1,
        mpsc::unbounded().0,
        scope.clone(),
    ));

    let (f_p, ep_i, ep_o) = (func_c.into_proxy(), ep_in_c.into_proxy(), ep_out_c.into_proxy());
    scope.spawn_local(async move {
        UsbZeroFunctionDevice::new(
            f_p,
            ep_i,
            TEST_EP_IN_ADDR,
            ep_o,
            TEST_EP_OUT_ADDR,
            0,
            TestMode::Loopback,
        )
        .handle_requests(iface_s.into_stream())
        .await;
    });

    let proxy = iface_c.into_proxy();
    let setup = fusb_descriptor::UsbSetup {
        bm_request_type: 0x81,
        b_request: USB_SETUP_REQ_GET_INTERFACE,
        w_value: 0,
        w_index: 0,
        w_length: 1,
    };

    // GetInterface returns alt 0
    assert_eq!(proxy.control(&setup, &[]).await.unwrap(), Ok(vec![0x00]));

    // FIDL SetInterface(0, 0) succeeds
    assert_eq!(proxy.set_interface(0, 0).await.unwrap(), Ok(()));
    assert_eq!(proxy.control(&setup, &[]).await.unwrap(), Ok(vec![0x00]));

    // FIDL SetInterface(0, 1) fails (only alt 0 is supported)
    assert_eq!(proxy.set_interface(0, 1).await.unwrap(), Err(Status::NOT_SUPPORTED.into_raw()));

    // Configure device
    assert_eq!(proxy.set_configured(true, fusb_descriptor::UsbSpeed::High).await.unwrap(), Ok(()));
    assert_eq!(proxy.control(&setup, &[]).await.unwrap(), Ok(vec![0x00]));

    // SetInterface(0, 0) while configured succeeds and resets endpoints
    assert_eq!(proxy.set_interface(0, 0).await.unwrap(), Ok(()));
    assert_eq!(proxy.control(&setup, &[]).await.unwrap(), Ok(vec![0x00]));

    // SetInterface(0, 1) while configured fails
    assert_eq!(proxy.set_interface(0, 1).await.unwrap(), Err(Status::NOT_SUPPORTED.into_raw()));

    // Test Chapter 9 SET_INTERFACE control request (alt 0 succeeds)
    let setup_set_interface_0 = fusb_descriptor::UsbSetup {
        bm_request_type: 0x01,
        b_request: USB_SETUP_REQ_SET_INTERFACE,
        w_value: 0,
        w_index: 0,
        w_length: 0,
    };
    assert_eq!(proxy.control(&setup_set_interface_0, &[]).await.unwrap(), Ok(vec![]));
    assert_eq!(proxy.control(&setup, &[]).await.unwrap(), Ok(vec![0x00]));

    // USB 2.0 §9.4.10: SET_INTERFACE resets halt state on all interface endpoints
    let set_halt = |ep_addr: u8| fusb_descriptor::UsbSetup {
        bm_request_type: 0x02, // OUT, Standard, Endpoint
        b_request: USB_SETUP_REQ_SET_FEATURE,
        w_value: USB_FEATURE_ENDPOINT_HALT,
        w_index: ep_addr as u16,
        w_length: 0,
    };
    let get_ep_status = |ep_addr: u8| fusb_descriptor::UsbSetup {
        bm_request_type: 0x82, // IN, Standard, Endpoint
        b_request: USB_SETUP_REQ_GET_STATUS,
        w_value: 0,
        w_index: ep_addr as u16,
        w_length: 2,
    };

    // Stall endpoints and verify stalled
    assert_eq!(proxy.control(&set_halt(TEST_EP_IN_ADDR), &[]).await.unwrap(), Ok(vec![]));
    assert_eq!(proxy.control(&set_halt(TEST_EP_OUT_ADDR), &[]).await.unwrap(), Ok(vec![]));
    assert_eq!(
        proxy.control(&get_ep_status(TEST_EP_IN_ADDR), &[]).await.unwrap(),
        Ok(vec![0x01, 0x00])
    );
    assert_eq!(
        proxy.control(&get_ep_status(TEST_EP_OUT_ADDR), &[]).await.unwrap(),
        Ok(vec![0x01, 0x00])
    );

    // Chapter 9 SET_INTERFACE(0, 0) resets halt state on all interface endpoints
    assert_eq!(proxy.control(&setup_set_interface_0, &[]).await.unwrap(), Ok(vec![]));
    assert_eq!(
        proxy.control(&get_ep_status(TEST_EP_IN_ADDR), &[]).await.unwrap(),
        Ok(vec![0x00, 0x00])
    );
    assert_eq!(
        proxy.control(&get_ep_status(TEST_EP_OUT_ADDR), &[]).await.unwrap(),
        Ok(vec![0x00, 0x00])
    );

    // Re-stall and verify FIDL SetInterface(0, 0) also resets halt state
    assert_eq!(proxy.control(&set_halt(TEST_EP_IN_ADDR), &[]).await.unwrap(), Ok(vec![]));
    assert_eq!(proxy.control(&set_halt(TEST_EP_OUT_ADDR), &[]).await.unwrap(), Ok(vec![]));
    assert_eq!(proxy.set_interface(0, 0).await.unwrap(), Ok(()));
    assert_eq!(
        proxy.control(&get_ep_status(TEST_EP_IN_ADDR), &[]).await.unwrap(),
        Ok(vec![0x00, 0x00])
    );
    assert_eq!(
        proxy.control(&get_ep_status(TEST_EP_OUT_ADDR), &[]).await.unwrap(),
        Ok(vec![0x00, 0x00])
    );

    // Test Chapter 9 SET_INTERFACE control request (alt 1 fails)
    let setup_set_interface_1 = fusb_descriptor::UsbSetup {
        bm_request_type: 0x01,
        b_request: USB_SETUP_REQ_SET_INTERFACE,
        w_value: 1,
        w_index: 0,
        w_length: 0,
    };
    assert_eq!(
        proxy.control(&setup_set_interface_1, &[]).await.unwrap(),
        Err(Status::NOT_SUPPORTED.into_raw())
    );

    // Verify integer truncation protection (> 255 does not truncate to 0)
    let setup_trunc_val = fusb_descriptor::UsbSetup {
        bm_request_type: 0x01,
        b_request: USB_SETUP_REQ_SET_INTERFACE,
        w_value: 0x0100, // 256
        w_index: 0,
        w_length: 0,
    };
    assert_eq!(
        proxy.control(&setup_trunc_val, &[]).await.unwrap(),
        Err(Status::NOT_SUPPORTED.into_raw())
    );

    let setup_trunc_idx = fusb_descriptor::UsbSetup {
        bm_request_type: 0x01,
        b_request: USB_SETUP_REQ_SET_INTERFACE,
        w_value: 0,
        w_index: 0x0100, // 256
        w_length: 0,
    };
    assert_eq!(
        proxy.control(&setup_trunc_idx, &[]).await.unwrap(),
        Err(Status::NOT_SUPPORTED.into_raw())
    );

    // Test VendorRequest::SetTestMode is rejected
    let setup_set_mode = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_OUT,
        b_request: VendorRequest::SetTestMode as u8,
        w_value: TestMode::SourceSink as u16,
        w_index: 0,
        w_length: 0,
    };
    assert_eq!(
        proxy.control(&setup_set_mode, &[]).await.unwrap(),
        Err(Status::NOT_SUPPORTED.into_raw())
    );

    // Test VendorRequest::GetTestMode returns Loopback
    let setup_get_mode = fusb_descriptor::UsbSetup {
        bm_request_type: USB_TYPE_VENDOR_IN,
        b_request: VendorRequest::GetTestMode as u8,
        w_value: 0,
        w_index: 0,
        w_length: 1,
    };
    assert_eq!(
        proxy.control(&setup_get_mode, &[]).await.unwrap(),
        Ok(vec![TestMode::Loopback as u8])
    );
}

#[fuchsia::test]
async fn test_source_sink_cancellation() {
    let (ep_in_client, ep_in_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let (ep_out_client, ep_out_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();

    let state_in =
        Arc::new(Mutex::new(MockEndpointState { requests: vec![], vmos: HashMap::new() }));
    let state_out =
        Arc::new(Mutex::new(MockEndpointState { requests: vec![], vmos: HashMap::new() }));

    let (comp_in_tx, comp_in_rx) = mpsc::unbounded();
    let (comp_out_tx, comp_out_rx) = mpsc::unbounded();
    let (event_in_tx, mut event_in_rx) = mpsc::unbounded();
    let (event_out_tx, mut event_out_rx) = mpsc::unbounded();

    let scope = Arc::new(fasync::Scope::new_with_name("test_ss_cancel"));

    scope.spawn_local(run_mock_endpoint(
        ep_in_server.into_stream(),
        state_in.clone(),
        comp_in_rx,
        event_in_tx,
        scope.clone(),
    ));
    scope.spawn_local(run_mock_endpoint(
        ep_out_server.into_stream(),
        state_out.clone(),
        comp_out_rx,
        event_out_tx,
        scope.clone(),
    ));

    let ep_in_proxy = ep_in_client.into_proxy();
    let ep_out_proxy = ep_out_client.into_proxy();

    let mut vmos_registered = false;
    let (sink_task, source_task) = run_source_sink(
        ep_in_proxy,
        ep_out_proxy,
        &mut vmos_registered,
        USB_MAX_PACKET_SIZE_HIGH_SPEED.into(),
    )
    .await
    .unwrap();

    assert_eq!(event_out_rx.next().await, Some(MockEvent::VmoRegistered));
    assert_eq!(event_out_rx.next().await, Some(MockEvent::RequestQueued));
    assert_eq!(event_in_rx.next().await, Some(MockEvent::VmoRegistered));
    assert_eq!(event_in_rx.next().await, Some(MockEvent::RequestQueued));

    // Verify cancellation in a mixed batch on OUT endpoint
    let (canceled_out_req, valid_out_req) = {
        let mut state = state_out.lock().unwrap();
        let c_req = state.requests.pop().unwrap();
        let v_req = state.requests.pop().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
        (c_req, v_req)
    };

    comp_out_tx
        .unbounded_send(vec![
            fusb_endpoint::Completion {
                request: Some(canceled_out_req),
                status: Some(zx::sys::ZX_ERR_CANCELED),
                transfer_size: Some(0),
                ..Default::default()
            },
            fusb_endpoint::Completion {
                request: Some(valid_out_req),
                status: Some(zx::sys::ZX_OK),
                transfer_size: Some(0),
                ..Default::default()
            },
        ])
        .unwrap();

    let event = event_out_rx.next().await;
    assert_eq!(event, Some(MockEvent::RequestQueued));
    {
        let state = state_out.lock().unwrap();
        // Canceled transfer dropped; only valid transfer re-queued.
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 1);
    }

    // Verify IO_REFUSED re-queues transfer to maintain ring depth on OUT endpoint
    let io_refused_out_req = {
        let mut state = state_out.lock().unwrap();
        let req = state.requests.pop().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
        req
    };
    comp_out_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(io_refused_out_req),
            status: Some(zx::sys::ZX_ERR_IO_REFUSED),
            transfer_size: Some(0),
            ..Default::default()
        }])
        .unwrap();

    let event = event_out_rx.next().await;
    assert_eq!(event, Some(MockEvent::RequestQueued));
    {
        let state = state_out.lock().unwrap();
        // IO_REFUSED transfer re-queued to maintain ring depth.
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 1);
    }

    // Verify IO_NOT_PRESENT terminates pump task without re-queuing on OUT endpoint
    let io_not_present_out_req = {
        let mut state = state_out.lock().unwrap();
        let req = state.requests.pop().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
        req
    };
    comp_out_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(io_not_present_out_req),
            status: Some(zx::sys::ZX_ERR_IO_NOT_PRESENT),
            transfer_size: Some(0),
            ..Default::default()
        }])
        .unwrap();

    sink_task.await;
    {
        let state = state_out.lock().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
    }

    // Verify cancellation in a mixed batch on IN endpoint
    let (canceled_in_req, valid_in_req) = {
        let mut state = state_in.lock().unwrap();
        let c_req = state.requests.pop().unwrap();
        let v_req = state.requests.pop().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
        (c_req, v_req)
    };

    comp_in_tx
        .unbounded_send(vec![
            fusb_endpoint::Completion {
                request: Some(canceled_in_req),
                status: Some(zx::sys::ZX_ERR_CANCELED),
                transfer_size: Some(0),
                ..Default::default()
            },
            fusb_endpoint::Completion {
                request: Some(valid_in_req),
                status: Some(zx::sys::ZX_OK),
                transfer_size: Some(512),
                ..Default::default()
            },
        ])
        .unwrap();

    let event = event_in_rx.next().await;
    assert_eq!(event, Some(MockEvent::RequestQueued));
    {
        let state = state_in.lock().unwrap();
        // Canceled transfer dropped; only valid transfer re-queued.
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 1);
    }

    // Verify IO_REFUSED re-queues transfer to maintain ring depth on IN endpoint
    let io_refused_in_req = {
        let mut state = state_in.lock().unwrap();
        let req = state.requests.pop().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
        req
    };
    comp_in_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(io_refused_in_req),
            status: Some(zx::sys::ZX_ERR_IO_REFUSED),
            transfer_size: Some(0),
            ..Default::default()
        }])
        .unwrap();

    let event = event_in_rx.next().await;
    assert_eq!(event, Some(MockEvent::RequestQueued));
    {
        let state = state_in.lock().unwrap();
        // IO_REFUSED transfer re-queued to maintain ring depth.
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 1);
    }

    // Verify IO_NOT_PRESENT terminates pump task without re-queuing on IN endpoint
    let io_not_present_in_req = {
        let mut state = state_in.lock().unwrap();
        let req = state.requests.pop().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
        req
    };
    comp_in_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(io_not_present_in_req),
            status: Some(zx::sys::ZX_ERR_IO_NOT_PRESENT),
            transfer_size: Some(0),
            ..Default::default()
        }])
        .unwrap();

    source_task.await;
    {
        let state = state_in.lock().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
    }
}

#[fuchsia::test]
async fn test_loopback_cancellation() {
    let (ep_in_client, ep_in_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let (ep_out_client, ep_out_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();

    let state_in =
        Arc::new(Mutex::new(MockEndpointState { requests: vec![], vmos: HashMap::new() }));
    let state_out =
        Arc::new(Mutex::new(MockEndpointState { requests: vec![], vmos: HashMap::new() }));

    let (_comp_in_tx, comp_in_rx) = mpsc::unbounded();
    let (comp_out_tx, comp_out_rx) = mpsc::unbounded();
    let (event_in_tx, _event_in_rx) = mpsc::unbounded();
    let (event_out_tx, mut event_out_rx) = mpsc::unbounded();

    let scope = Arc::new(fasync::Scope::new_with_name("test_loopback_cancellation"));

    scope.spawn_local(run_mock_endpoint(
        ep_in_server.into_stream(),
        state_in.clone(),
        comp_in_rx,
        event_in_tx,
        scope.clone(),
    ));
    scope.spawn_local(run_mock_endpoint(
        ep_out_server.into_stream(),
        state_out.clone(),
        comp_out_rx,
        event_out_tx,
        scope.clone(),
    ));

    let ep_in_proxy = ep_in_client.into_proxy();
    let ep_out_proxy = ep_out_client.into_proxy();

    let mut vmos_registered = false;
    let (read_task, _write_task) = run_loopback(
        ep_in_proxy,
        ep_out_proxy,
        &mut vmos_registered,
        u64::from(USB_MAX_PACKET_SIZE_HIGH_SPEED),
    )
    .await
    .unwrap();

    assert_eq!(event_out_rx.next().await, Some(MockEvent::VmoRegistered));
    assert_eq!(event_out_rx.next().await, Some(MockEvent::RequestQueued));

    // Verify cancellation in a mixed batch on OUT endpoint
    let (canceled_out_req, valid_out_req) = {
        let mut state = state_out.lock().unwrap();
        let c_req = state.requests.pop().unwrap();
        let v_req = state.requests.pop().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
        (c_req, v_req)
    };

    comp_out_tx
        .unbounded_send(vec![
            fusb_endpoint::Completion {
                request: Some(canceled_out_req),
                status: Some(zx::sys::ZX_ERR_CANCELED),
                transfer_size: Some(0),
                ..Default::default()
            },
            fusb_endpoint::Completion {
                request: Some(valid_out_req),
                status: Some(zx::sys::ZX_OK),
                transfer_size: Some(0),
                ..Default::default()
            },
        ])
        .unwrap();

    let event = event_out_rx.next().await;
    assert_eq!(event, Some(MockEvent::RequestQueued));
    {
        let state = state_out.lock().unwrap();
        // Canceled transfer dropped; only valid transfer re-queued.
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 1);
    }

    // Verify IO_REFUSED re-queues buffer to maintain ring depth (endpoint halt / abort)
    let io_refused_out_req = {
        let mut state = state_out.lock().unwrap();
        let req = state.requests.pop().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
        req
    };
    comp_out_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(io_refused_out_req),
            status: Some(zx::sys::ZX_ERR_IO_REFUSED),
            transfer_size: Some(0),
            ..Default::default()
        }])
        .unwrap();

    let event = event_out_rx.next().await;
    assert_eq!(event, Some(MockEvent::RequestQueued));
    {
        let state = state_out.lock().unwrap();
        // IO_REFUSED transfer re-queued to maintain ring depth.
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 1);
    }

    // Verify IO_NOT_PRESENT terminates loopback read task without re-queuing (disconnect)
    let io_not_present_out_req = {
        let mut state = state_out.lock().unwrap();
        let req = state.requests.pop().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
        req
    };
    comp_out_tx
        .unbounded_send(vec![fusb_endpoint::Completion {
            request: Some(io_not_present_out_req),
            status: Some(zx::sys::ZX_ERR_IO_NOT_PRESENT),
            transfer_size: Some(0),
            ..Default::default()
        }])
        .unwrap();

    read_task.await;
    {
        let state = state_out.lock().unwrap();
        assert_eq!(state.requests.len(), QUEUE_DEPTH - 2);
    }
}

#[fuchsia::test]
async fn test_transfer_size_exceeds_vmo_size() {
    let (ep_in_client, _ep_in_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let (ep_out_client, _ep_out_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let mut vmos_registered = false;
    let res = run_source_sink(
        ep_in_client.into_proxy(),
        ep_out_client.into_proxy(),
        &mut vmos_registered,
        DEFAULT_VMO_SIZE + 1,
    )
    .await;
    assert_eq!(res.err(), Some(Status::INVALID_ARGS));

    let (ep_in_client, _ep_in_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let (ep_out_client, _ep_out_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let mut vmos_registered = false;
    let res = run_loopback(
        ep_in_client.into_proxy(),
        ep_out_client.into_proxy(),
        &mut vmos_registered,
        DEFAULT_VMO_SIZE + 1,
    )
    .await;
    assert_eq!(res.err(), Some(Status::INVALID_ARGS));
}

#[fuchsia::test]
async fn test_register_vmos_out_of_order_and_mismatch() {
    let (ep_client, ep_server) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let mut ep_stream = ep_server.into_stream();
    let scope = Arc::new(fasync::Scope::new());
    let s = scope.clone();
    s.spawn_local(async move {
        while let Ok(Some(req)) = ep_stream.try_next().await {
            match req {
                fusb_endpoint::EndpointRequest::UnregisterVmos { responder, .. } => {
                    let _ = responder.send(&[], &[]);
                }
                fusb_endpoint::EndpointRequest::RegisterVmos { vmo_ids, responder } => {
                    // Return VMOs in reverse order to test sorting
                    let mut vmos = vec![];
                    for info in vmo_ids.into_iter().rev() {
                        let id = info.id.unwrap();
                        let size = info.size.unwrap();
                        let vmo = zx::Vmo::create(size).unwrap();
                        let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
                        vmos.push(fusb_endpoint::VmoHandle {
                            id: Some(id),
                            vmo: Some(dup),
                            ..Default::default()
                        });
                    }
                    let _ = responder.send(vmos);
                }
                _ => {}
            }
        }
    });

    let ep_proxy = ep_client.into_proxy();
    let vmos = register_vmos(&ep_proxy, 100, 4, 4096).await.expect("register_vmos should sort");
    assert_eq!(vmos.len(), 4);

    // Test mismatched ID
    let (ep_client2, ep_server2) = create_endpoints::<fusb_endpoint::EndpointMarker>();
    let mut ep_stream2 = ep_server2.into_stream();
    let s2 = scope.clone();
    s2.spawn_local(async move {
        while let Ok(Some(req)) = ep_stream2.try_next().await {
            match req {
                fusb_endpoint::EndpointRequest::UnregisterVmos { responder, .. } => {
                    let _ = responder.send(&[], &[]);
                }
                fusb_endpoint::EndpointRequest::RegisterVmos { responder, .. } => {
                    let vmo = zx::Vmo::create(4096).unwrap();
                    let dup = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
                    let vmos = vec![fusb_endpoint::VmoHandle {
                        id: Some(999), // Mismatched! Expected 100
                        vmo: Some(dup),
                        ..Default::default()
                    }];
                    let _ = responder.send(vmos);
                }
                _ => {}
            }
        }
    });

    let ep_proxy2 = ep_client2.into_proxy();
    let res = register_vmos(&ep_proxy2, 100, 1, 4096).await;
    assert_eq!(res.err(), Some(Status::INTERNAL));
}
