// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod fake_cqhci;

pub use fake_cqhci::TestCommandQueueHost;

use crate::CqhciDriver;
use block_client::{
    BlockClient, BufferSlice, InlineCryptoOptions, MutableBufferSlice, ReadOptions,
    RemoteBlockClient, WriteOptions,
};
use block_server::callback_interface::SessionManager;
use fdf_component::testing::harness::DriverUnderTest;
use fdf_power::SuspendableDriver;
use fidl_fuchsia_hardware_block_volume::{self as fvolume};
use fidl_fuchsia_storage_block as fblock;
use fidl_next_fuchsia_hardware_cqhci::EmmcPartitionId;
use fidl_next_fuchsia_hardware_rpmb as rpmb;
use fidl_next_fuchsia_mem as fmem;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use futures::{FutureExt as _, StreamExt as _};
use sdmmc_spec::{CQHCI_TASK_DESCRIPTOR_LIST_DCMD_SLOT, SdhciInterruptStatusRegister};
use std::pin::pin;
use std::sync::{Arc, Weak};
use test_case::test_case;
use zx;

use fake_cqhci::{Blocker, FakeCqhci};

// With the fake, there are a limited number of reads that can be performed before pinning will
// start failing.  This is related to the number of addresses we provide to our fake BTI
// implementation, so if that changes, this might need to change.
const MAX_PIN_OPS: usize = 1750;

fn connect_block_proxy(
    started_driver: &DriverUnderTest<'_, CqhciDriver>,
    partition_name: &str,
) -> fblock::BlockProxy {
    let incoming = started_driver.driver_outgoing();
    let service = incoming
        .service::<fidl_fuchsia_hardware_block_volume::ServiceProxy>()
        .instance(partition_name)
        .connect()
        .expect("failed to connect to service");

    service.connect_to_volume().expect("failed to connect to volume")
}

async fn connect_block_client(
    started_driver: &DriverUnderTest<'_, CqhciDriver>,
    partition_name: &str,
) -> RemoteBlockClient {
    RemoteBlockClient::new(connect_block_proxy(started_driver, partition_name))
        .await
        .expect("failed to create block client")
}

fn connect_rpmb_client(
    started_driver: &DriverUnderTest<'_, CqhciDriver>,
    scope: &fasync::Scope,
) -> fidl_next::Client<rpmb::Rpmb> {
    let rpmb_service: fdf_component::ServiceInstance<rpmb::Service> =
        started_driver.driver_outgoing().service().connect_next().unwrap();

    let (client_end, server_end) = fidl_next::fuchsia::create_channel();
    rpmb_service.device(server_end).unwrap();
    client_end.spawn_on(scope)
}

fn connect_inline_encryption_proxy(
    started_driver: &DriverUnderTest<'_, CqhciDriver>,
    partition_name: &str,
) -> fidl_fuchsia_hardware_inlineencryption::DeviceProxy {
    let incoming = started_driver.driver_outgoing();
    let service = incoming
        .service::<fvolume::ServiceProxy>()
        .instance(partition_name)
        .connect()
        .expect("failed to connect to service");

    service.connect_to_inline_encryption().expect("failed to connect to inline encryption")
}

#[fuchsia::test]
async fn test_driver_lifecycle() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");
    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_node_token() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let token = zx::Event::create();
    harness = harness.set_node_token(token.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap());
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let incoming = started_driver.driver_outgoing();
    let service = incoming
        .service::<fidl_fuchsia_hardware_block_volume::ServiceProxy>()
        .instance("user")
        .connect()
        .expect("failed to connect to service");

    let token_proxy = service.connect_to_token().expect("failed to connect to token");
    let res = token_proxy.get().await.expect("get failed");
    assert!(res.is_ok());
    let token = res.unwrap();
    assert!(!token.is_invalid());

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_io() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let block_client = connect_block_client(&started_driver, "user").await;

    let buf = vec![11u8; 512];
    block_client.write_at(BufferSlice::from(&buf[..]), 0).await.expect("write failed");

    let mut read_buf = vec![0u8; 512];
    block_client
        .read_at(MutableBufferSlice::from(&mut read_buf[..]), 0)
        .await
        .expect("read failed");
    assert_eq!(read_buf, buf);

    block_client.flush().await.expect("flush failed");
    block_client.trim(0..1024).await.expect("trim failed");

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_trim() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let block_client = connect_block_client(&started_driver, "user").await;

    let buf = vec![0xAAu8; 512];
    block_client.write_at(BufferSlice::from(&buf[..]), 0).await.expect("write failed");

    block_client.trim(0..0).await.expect_err("zero-length trim should fail");

    // Read back and ensure it's still 0xAA (not trimmed)
    let mut read_buf = vec![0u8; 512];
    block_client
        .read_at(MutableBufferSlice::from(&mut read_buf[..]), 0)
        .await
        .expect("read failed");
    assert_eq!(read_buf, buf);

    // Now test a real trim.
    block_client.trim(0..512).await.expect("trim failed");
    let mut read_buf2 = vec![0u8; 512];
    block_client
        .read_at(MutableBufferSlice::from(&mut read_buf2[..]), 0)
        .await
        .expect("read failed");
    assert_eq!(read_buf2, vec![0xFFu8; 512]);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_switch_partition() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let user_client = connect_block_client(&started_driver, "user").await;
    let boot1_client = connect_block_client(&started_driver, "boot1").await;

    let user_pattern = vec![0xaa; 512];
    let boot1_pattern = vec![0xbb; 512];

    user_client.write_at(BufferSlice::from(&user_pattern[..]), 0).await.expect("user write failed");
    boot1_client
        .write_at(BufferSlice::from(&boot1_pattern[..]), 0)
        .await
        .expect("boot1 write failed");

    let user_fut = async {
        for _ in 0..10 {
            let mut buf = vec![0u8; 512];
            user_client
                .read_at(MutableBufferSlice::from(&mut buf[..]), 0)
                .await
                .expect("user read failed");
            assert_eq!(buf, user_pattern);
        }
    };

    let boot1_fut = async {
        for _ in 0..10 {
            let mut buf = vec![0u8; 512];
            boot1_client
                .read_at(MutableBufferSlice::from(&mut buf[..]), 0)
                .await
                .expect("boot1 read failed");
            assert_eq!(buf, boot1_pattern);
        }
    };

    futures::join!(user_fut, boot1_fut);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_forwards_non_cq_interrupts() {
    let (mut fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    // Generate a non-CQ interrupt
    fixture.set_sdhci_interrupt_status({
        let mut sdhci_interrupt_status = SdhciInterruptStatusRegister::from_raw(0);
        sdhci_interrupt_status.set_command_complete(true);
        sdhci_interrupt_status
    });
    fixture.handles.hardware_irq.trigger(zx::BootInstant::get()).unwrap();

    // Wait until the first non-CQ interrupt is delivered to the delegator.
    let irq =
        fixture.handles.non_cq_interrupt_receiver.next().await.expect("Failed to wait for irq");

    // Trigger again while it's not acked.
    fixture.handles.hardware_irq.trigger(zx::BootInstant::get()).unwrap();

    // Give time for it to be processed.
    fasync::Timer::new(std::time::Duration::from_millis(10)).await;

    // No new interrupt should have been triggered because we haven't acked the first one.
    assert!(fixture.handles.non_cq_interrupt_receiver.next().now_or_never().is_none());

    // Now ack the virtual interrupt.  The second IRQ should be delivered.
    let _ = irq.ack();

    let irq2 =
        fixture.handles.non_cq_interrupt_receiver.next().await.expect("Failed to wait for irq");

    fixture.set_sdhci_interrupt_status(SdhciInterruptStatusRegister::from_raw(0));
    let _ = irq2.ack();

    started_driver.stop_driver().await;
}

#[fuchsia::test(threads = 2)]
async fn test_error_recovery() {
    let (fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    struct TestState {
        on_error_event: event_listener::Event,
        num_errors: usize,
        abort: bool,
    }
    let state = Arc::new(Mutex::new(TestState {
        on_error_event: event_listener::Event::new(),
        num_errors: 0,
        abort: false,
    }));
    let on_error_listener = state.lock().on_error_event.listen();

    // Start two tasks to continuously submit requests.
    let block_client = {
        let proxy = connect_block_proxy(&started_driver, "user");
        RemoteBlockClient::new(proxy)
    }
    .await
    .expect("failed to create block client");
    let state_clone = state.clone();
    let reader = fixture.scope.spawn(async move {
        // Limit iterations to avoid exhausting the fake BTI's physical addresses.
        for _ in 0..1000 {
            let mut buf = vec![0u8; 512];
            let result = block_client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await;
            let mut state = state_clone.lock();
            if result.is_err() {
                state.num_errors += 1;
                state.on_error_event.notify(usize::MAX);
            }
            if state.abort {
                break;
            }
        }
    });
    let flusher = {
        let block_client = {
            let proxy = connect_block_proxy(&started_driver, "user");
            RemoteBlockClient::new(proxy)
        }
        .await
        .expect("failed to create block client");
        let state = state.clone();
        fixture.scope.spawn(async move {
            loop {
                let result = block_client.flush().await;
                let mut state = state.lock();
                if result.is_err() {
                    state.num_errors += 1;
                    state.on_error_event.notify(usize::MAX);
                }
                if state.abort {
                    break;
                }
            }
        })
    };

    // Fail a task.
    fixture.fail_next(1);
    on_error_listener.await;
    fasync::Timer::new(std::time::Duration::from_millis(100)).await;

    state.lock().abort = true;
    reader.await;
    flusher.await;

    // We should only see tasks which were currently in-flight failing. Since there are only two
    // clients (1 reader, 1 flusher), at most 2 tasks should fail (the one we explicitly failed,
    // and any which was already in-flight).
    assert!(state.lock().num_errors <= 2);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_crypto_icce_recovery() {
    let (fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    struct TestState {
        on_error_event: event_listener::Event,
        on_success_event: event_listener::Event,
        num_errors: usize,
        num_successes: usize,
        abort: bool,
    }
    let state = Arc::new(Mutex::new(TestState {
        on_error_event: event_listener::Event::new(),
        on_success_event: event_listener::Event::new(),
        num_errors: 0,
        num_successes: 0,
        abort: false,
    }));

    // Start two tasks to continuously submit requests.
    let reader = {
        let block_client = connect_block_client(&started_driver, "user").await;
        let state = state.clone();
        fixture.scope.spawn(async move {
            for _ in 0..MAX_PIN_OPS {
                let mut buf = vec![0u8; 512];
                let result = block_client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await;
                let mut state = state.lock();
                if result.is_ok() {
                    state.num_successes += 1;
                    state.on_success_event.notify(usize::MAX);
                } else {
                    state.num_errors += 1;
                    state.on_error_event.notify(usize::MAX);
                }
                if state.abort {
                    break;
                }
            }
        })
    };
    let flusher = {
        let block_client = connect_block_client(&started_driver, "user").await;
        let state = state.clone();
        fixture.scope.spawn(async move {
            loop {
                let result = block_client.flush().await;
                let mut state = state.lock();
                if result.is_ok() {
                    state.num_successes += 1;
                    state.on_success_event.notify(usize::MAX);
                } else {
                    state.num_errors += 1;
                    state.on_error_event.notify(usize::MAX);
                }
                if state.abort {
                    break;
                }
            }
        })
    };

    // Issue a task with invalid crypto slot. This should fail and trigger recovery.
    let bad_opts = ReadOptions { inline_crypto: InlineCryptoOptions::enabled(99, 100) };

    let bad_client = connect_block_client(&started_driver, "user").await;
    let mut buf = vec![0u8; 512];
    let result = bad_client
        .read_at_with_opts_traced(
            MutableBufferSlice::from(&mut buf[..]),
            0,
            bad_opts,
            block_client::NO_TRACE_ID,
        )
        .await;
    assert!(result.is_err());

    // Verify that background tasks are still succeeding (recovery worked).
    let on_success_listener = state.lock().on_success_event.listen();
    on_success_listener.await;

    state.lock().abort = true;
    reader.await;
    flusher.await;

    assert!(state.lock().num_errors <= 2);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_crypto_gce_recovery() {
    let (fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    struct TestState {
        on_error_event: event_listener::Event,
        on_success_event: event_listener::Event,
        num_errors: usize,
        num_successes: usize,
        abort: bool,
    }
    let state = Arc::new(Mutex::new(TestState {
        on_error_event: event_listener::Event::new(),
        on_success_event: event_listener::Event::new(),
        num_errors: 0,
        num_successes: 0,
        abort: false,
    }));
    let on_error_listener = state.lock().on_error_event.listen();

    // Start two tasks to continuously submit requests.
    let reader = {
        let block_client = connect_block_client(&started_driver, "user").await;
        let state = state.clone();
        fixture.scope.spawn(async move {
            for _ in 0..MAX_PIN_OPS {
                let mut buf = vec![0u8; 512];
                let result = block_client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await;
                let mut state = state.lock();
                if result.is_ok() {
                    state.num_successes += 1;
                    state.on_success_event.notify(usize::MAX);
                } else {
                    state.num_errors += 1;
                    state.on_error_event.notify(usize::MAX);
                }
                if state.abort {
                    break;
                }
            }
        })
    };
    let flusher = {
        let block_client = connect_block_client(&started_driver, "user").await;
        let state = state.clone();
        fixture.scope.spawn(async move {
            loop {
                let result = block_client.flush().await;
                let mut state = state.lock();
                if result.is_ok() {
                    state.num_successes += 1;
                    state.on_success_event.notify(usize::MAX);
                } else {
                    state.num_errors += 1;
                    state.on_error_event.notify(usize::MAX);
                }
                if state.abort {
                    break;
                }
            }
        })
    };

    // Fail a task with GCE. This should eventually trigger recovery.
    let on_success_listener = state.lock().on_success_event.listen();
    fixture.fail_next_crypto_gce(1);
    on_error_listener.await;
    on_success_listener.await;

    state.lock().abort = true;
    reader.await;
    flusher.await;

    assert!(state.lock().num_errors <= 2);

    started_driver.stop_driver().await;
}

#[fuchsia::test(threads = 4)]
async fn test_concurrency() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");
    let started_driver = Mutex::new(Some(started_driver));

    let proxy0 = connect_block_proxy((*started_driver.lock()).as_ref().unwrap(), "user");
    let proxy1 = connect_block_proxy((*started_driver.lock()).as_ref().unwrap(), "user");
    let proxy2 = connect_block_proxy((*started_driver.lock()).as_ref().unwrap(), "user");
    let proxy3 = connect_block_proxy((*started_driver.lock()).as_ref().unwrap(), "boot1");

    let user_client1 = RemoteBlockClient::new(proxy0).await.expect("failed to create block client");
    let user_client2 = RemoteBlockClient::new(proxy1).await.expect("failed to create block client");
    let user_client3 = RemoteBlockClient::new(proxy2).await.expect("failed to create block client");
    let boot1_client = RemoteBlockClient::new(proxy3).await.expect("failed to create block client");

    let mut pattern = vec![0x11; 512];
    pattern.extend(vec![0x22; 1024]);

    for i in 0..100 {
        user_client1
            .write_at(BufferSlice::from(&pattern[..]), i * 1536)
            .await
            .expect("read failed");
    }

    let pattern = vec![0x33; 512];
    for i in 0..100 {
        boot1_client
            .write_at(BufferSlice::from(&pattern[..]), i * 1536)
            .await
            .expect("read failed");
    }

    let do_io = |client: RemoteBlockClient, pattern: Vec<u8>, offset: u64| async move {
        for i in 0..100 {
            let mut buf = vec![0x00; pattern.len()];
            client
                .read_at(MutableBufferSlice::from(&mut buf[..]), offset + i * 1536)
                .await
                .expect("read failed");
            assert_eq!(buf, pattern);
        }
    };
    let do_flush = |client: RemoteBlockClient| async move {
        for _ in 0..50 {
            client.flush().await.expect("flush failed");
        }
    };

    futures::join!(
        do_io(user_client1, vec![0x11; 512], 0),
        do_io(user_client2, vec![0x22; 1024], 512),
        do_flush(user_client3),
        do_io(boot1_client, vec![0x33; 512], 0),
    );

    let driver = started_driver.lock().take().unwrap();
    driver.stop_driver().await;
}

#[fuchsia::test(threads = 4)]
async fn test_shutdown_with_active_clients() {
    let (fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");
    let started_driver = Mutex::new(Some(started_driver));

    let proxy0 = connect_block_proxy((*started_driver.lock()).as_ref().unwrap(), "user");
    let proxy1 = connect_block_proxy((*started_driver.lock()).as_ref().unwrap(), "user");
    let proxy2 = connect_block_proxy((*started_driver.lock()).as_ref().unwrap(), "boot1");

    let user_client0 = RemoteBlockClient::new(proxy0).await.expect("failed to create block client");
    let user_client1 = RemoteBlockClient::new(proxy1).await.expect("failed to create block client");
    let boot1_client = RemoteBlockClient::new(proxy2).await.expect("failed to create block client");

    let do_io = |client: RemoteBlockClient| async move {
        for _ in 0..100 {
            let mut buf = vec![0x00; 512];
            let _ = client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await;
        }
    };
    let do_flush = |client: RemoteBlockClient| async move {
        let _ = client.flush().await;
    };

    let io_fut = fixture.scope.spawn(do_io(user_client0));
    let flush_fut = fixture.scope.spawn(do_flush(user_client1));
    let io_fut2 = fixture.scope.spawn(do_io(boot1_client));

    fasync::Timer::new(std::time::Duration::from_millis(100)).await;

    let driver = started_driver.lock().take().unwrap();
    let stop_fut = driver.stop_driver();
    futures::join!(stop_fut, io_fut, flush_fut, io_fut2,);
}

#[fuchsia::test]
async fn test_rpmb() {
    let (mut fixture, mut harness) = FakeCqhci::new(None);

    let started_driver = harness.start_driver().await.expect("failed to start driver");
    let started_driver = Mutex::new(Some(started_driver));

    let proxy = connect_block_proxy(started_driver.lock().as_ref().unwrap(), "user");
    let block_client = RemoteBlockClient::new(proxy).await.expect("failed to create block client");

    let rpmb_client = connect_rpmb_client(started_driver.lock().as_ref().unwrap(), &fixture.scope);

    let rpmb_svc_fut = async {
        while let Some(responder) = fixture.handles.rpmb_request_receiver.next().await {
            assert_eq!(fixture.in_progress_tasks(), 0);
            let completed_tasks = fixture.completed_tasks();
            fasync::Timer::new(std::time::Duration::from_micros(500)).await;
            assert_eq!(fixture.in_progress_tasks(), 0);
            assert_eq!(fixture.completed_tasks(), completed_tasks);
            let _ = responder.send(());
        }
    };

    let read_fut = async move {
        for i in 0..100 {
            let mut buf = vec![0x00; 512];
            block_client
                .read_at(MutableBufferSlice::from(&mut buf[..]), 0)
                .await
                .expect("read failed");
            if i % 10 == 0 {
                block_client.flush().await.expect("flush failed");
            }
        }
    };

    let rpmb_fut = async move {
        let vmo = zx::Vmo::create(1024).expect("failed to create vmo");
        for _ in 0..100 {
            let request = rpmb::Request {
                tx_frames: fmem::Range {
                    vmo: vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                    offset: 0,
                    size: 0,
                },
                rx_frames: None,
            };
            rpmb_client.request(request).await.expect("FIDL error").expect("rpmb request failed");
            // Do a brief sleep to avoid completely starving the reader.  This increases the
            // chances of interesting interleavings.
            fasync::Timer::new(std::time::Duration::from_micros(500)).await;
        }
    };

    let mut read_futs = pin!(futures::future::join(read_fut, rpmb_fut).fuse());
    let mut rpmb_svc_fut = pin!(rpmb_svc_fut.fuse());
    futures::select! {
        _ = read_futs => {}
        _ = rpmb_svc_fut => unreachable!(),
    }

    let driver = started_driver.lock().take().unwrap();
    driver.stop_driver().await;
}

#[test_case("user", 0xaa ; "user")]
#[test_case("boot1", 0x11 ; "boot1")]
#[fuchsia::test]
async fn test_rpmb_partition_interleave(partition_name: &str, pattern: u8) {
    let (mut fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let client = connect_block_client(&started_driver, partition_name).await;
    let boot1_client = connect_block_client(&started_driver, "boot1").await;
    let rpmb_client = connect_rpmb_client(&started_driver, &fixture.scope);

    let pattern_vec = vec![pattern; 512];
    let boot1_pattern = vec![0x11; 512];

    // Initialize partitions.
    client.write_at(BufferSlice::from(&pattern_vec[..]), 0).await.expect("write failed");
    boot1_client
        .write_at(BufferSlice::from(&boot1_pattern[..]), 0)
        .await
        .expect("boot1 write failed");

    // 1. Issue a request on the boot partition.
    let mut buf = vec![0u8; 512];
    boot1_client
        .read_at(MutableBufferSlice::from(&mut buf[..]), 0)
        .await
        .expect("boot1 read failed");
    assert_eq!(buf, boot1_pattern);

    // 2. Issue an RPMB request.
    let rpmb_svc_fut = async {
        let responder = fixture.handles.rpmb_request_receiver.next().await.unwrap();
        responder.send(()).unwrap();
    };

    let rpmb_request_fut = async {
        let vmo = zx::Vmo::create(1024).expect("failed to create vmo");
        let request = rpmb::Request {
            tx_frames: fmem::Range {
                vmo: vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                offset: 0,
                size: 0,
            },
            rx_frames: None,
        };
        rpmb_client.request(request).await.expect("FIDL error").expect("rpmb request failed");
    };

    futures::join!(rpmb_svc_fut, rpmb_request_fut);

    // 3. Issue a request on the target partition.
    let mut buf = vec![0u8; 512];
    client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await.expect("read failed");
    assert_eq!(buf, pattern_vec);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_shutdown_with_active_dcmd() {
    let mut blocker = Blocker::default();
    let (fixture, mut harness) = FakeCqhci::new(Some(blocker.hook()));
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let block_client0 = connect_block_client(&started_driver, "user").await;
    let block_client1 = connect_block_client(&started_driver, "user").await;

    blocker.block(1 << CQHCI_TASK_DESCRIPTOR_LIST_DCMD_SLOT);

    let flush0_task = fixture.scope.spawn(async move {
        // The first flush might or might not succeed.
        let _ = block_client0.flush().await;
    });

    // Wait for the DCMD to start.
    let unblock = blocker.next().await;

    // Issue another flush which should fail.
    let flush1_task = fixture.scope.spawn(async move {
        block_client1.flush().await.expect_err("flush should fail");
    });

    // Make sure no future DCMDs block.
    blocker.block(0);

    crate::SHUTTING_DOWN_FLAG.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut stop_driver = started_driver.stop_driver().boxed().fuse();
    futures::select! {
        _ = stop_driver => {}
        default => {
            // Drive shutdown concurrently with unblocking to ensure progress.
            // Since stop_driver() blocks on active tasks (which requires dropping unblock),
            // we loop-yield until SHUTTING_DOWN_FLAG is set to guarantee that the driver task
            // has entered stop() and set shutting_down=true. Only then do we drop unblock
            // to fail the enqueued flush1 task instead of letting it succeed.
            while !crate::SHUTTING_DOWN_FLAG.load(std::sync::atomic::Ordering::Relaxed) {
                fasync::yield_now().await;
            }
            drop(unblock);
        }
    }
    stop_driver.await;

    futures::join!(flush0_task, flush1_task);
}

#[fuchsia::test]
async fn test_shutdown_with_blocked_transfers() {
    let blocker = Blocker::default();
    let (fixture, mut harness) = FakeCqhci::new(Some(blocker.hook()));
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    // Issue one read to switch the partition.
    let block_client = connect_block_client(&started_driver, "user").await;
    let mut buf = vec![0x00; 512];
    let _ = block_client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await;

    // Block all requests.
    blocker.block(!(1 << CQHCI_TASK_DESCRIPTOR_LIST_DCMD_SLOT));

    let read_scope = fixture.scope.new_child();
    // Queue up 32 requests, which is one more than the available number of slots.
    for _ in 0..32 {
        let block_client = connect_block_client(&started_driver, "user").await;
        read_scope.spawn(async move {
            let mut buf = vec![0x00; 512];
            let _ = block_client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await;
        });
    }

    // Wait for 31 requests to be enqueued and submitted to the hardware.
    let unblock: Vec<_> = blocker.take(31).collect().await;

    crate::SHUTTING_DOWN_FLAG.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut stop_driver = started_driver.stop_driver().boxed().fuse();
    futures::select! {
        _ = stop_driver => {}
        default => {
            // Drive shutdown concurrently with unblocking to ensure progress.
            // Since stop_driver() blocks on active tasks (which requires dropping unblock),
            // we loop-yield until SHUTTING_DOWN_FLAG is set to guarantee that the driver task
            // has entered stop() and set shutting_down=true. Only then do we drop unblock
            // to fail the enqueued transfers instead of letting them succeed.
            while !crate::SHUTTING_DOWN_FLAG.load(std::sync::atomic::Ordering::Relaxed) {
                fasync::yield_now().await;
            }
            drop(unblock);
        }
    }
    stop_driver.await;

    read_scope.await;
}

#[fuchsia::test]
async fn test_flush_while_queue_not_empty() {
    let mut blocker = Blocker::default();
    let (fixture, mut harness) = FakeCqhci::new(Some(blocker.hook()));
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let block_client = connect_block_client(&started_driver, "user").await;

    // Perform a read to ensure the partition is switched and avoid DCMDs during the test.
    {
        let mut buf = vec![0u8; 512];
        block_client
            .read_at(MutableBufferSlice::from(&mut buf[..]), 0)
            .await
            .expect("placeholder read failed");
    }

    // Now enable blocking for everything except dcmds for the actual test.
    blocker.block(!(1 << CQHCI_TASK_DESCRIPTOR_LIST_DCMD_SLOT));

    // Submit a read request.
    let read_fut = fixture.scope.spawn(async move {
        let mut buf = vec![0u8; 512];
        block_client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await.expect("read failed");
    });

    // Wait for the transfer to be submitted to hardware.
    let unblock = blocker.next().await;

    // Unblock all future requests.
    blocker.block(0);

    // Submit a flush request.
    let block_client_clone = connect_block_client(&started_driver, "user").await;
    let mut flush_fut = fixture.scope.spawn(async move {
        block_client_clone.flush().await.expect("flush failed");
    });

    // The flush should be blocked and not yet submitted as a DCMD.  We wait a bit to give it a
    // chance to be submitted if it were not blocked.
    fasync::Timer::new(std::time::Duration::from_millis(100)).await;

    // Check if anything else was submitted (it shouldn't be).
    assert!(blocker.next().now_or_never().is_none());

    // And double check the flush request didn't finish.
    assert!((&mut flush_fut).now_or_never().is_none());

    // Unblock the read request.
    drop(unblock);

    futures::join!(read_fut, flush_fut);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_inline_crypto() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let proxy = connect_inline_encryption_proxy(&started_driver, "user");

    let response = proxy
        .program_key(b"my-wrapped-key", 4096)
        .await
        .expect("FIDL error")
        .expect("program_key failed");
    assert_eq!(response, 0);

    let response = proxy
        .derive_raw_secret(b"my-wrapped-key")
        .await
        .expect("FIDL error")
        .expect("derive_raw_secret failed");
    assert_eq!(response, vec![1, 2, 3, 4]);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_crypto_data_integrity() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let block_client = connect_block_client(&started_driver, "user").await;
    let crypto_proxy = connect_inline_encryption_proxy(&started_driver, "user");

    // Program two keys to get valid slots.
    let slot1 = crypto_proxy.program_key(b"key1", 4096).await.unwrap().unwrap();
    let slot2 = crypto_proxy.program_key(b"key2", 4096).await.unwrap().unwrap();
    assert_eq!(slot1, 0);
    assert_eq!(slot2, 1);

    let write_buf = vec![0xAAu8; 512];
    let mut read_buf = vec![0u8; 512];

    // Write with crypto enabled, slot1 (5), dun 100.
    let write_opts = WriteOptions {
        inline_crypto: InlineCryptoOptions::enabled(slot1, 100),
        ..Default::default()
    };

    block_client
        .write_at_with_opts_traced(
            BufferSlice::from(&write_buf[..]),
            0,
            write_opts,
            block_client::NO_TRACE_ID,
        )
        .await
        .expect("write failed");

    // Read with same parameters, should match.
    let read_opts = ReadOptions { inline_crypto: InlineCryptoOptions::enabled(slot1, 100) };

    block_client
        .read_at_with_opts_traced(
            MutableBufferSlice::from(&mut read_buf[..]),
            0,
            read_opts,
            block_client::NO_TRACE_ID,
        )
        .await
        .expect("read failed");

    assert_eq!(read_buf, write_buf);

    // Read with different slot, should NOT match (returns garbage).
    let bad_read_opts = ReadOptions { inline_crypto: InlineCryptoOptions::enabled(slot2, 100) };

    block_client
        .read_at_with_opts_traced(
            MutableBufferSlice::from(&mut read_buf[..]),
            0,
            bad_read_opts,
            block_client::NO_TRACE_ID,
        )
        .await
        .expect("read failed");

    assert_ne!(read_buf, write_buf);

    // Read with different dun, should NOT match.
    let bad_read_opts2 = ReadOptions { inline_crypto: InlineCryptoOptions::enabled(slot1, 101) };

    block_client
        .read_at_with_opts_traced(
            MutableBufferSlice::from(&mut read_buf[..]),
            0,
            bad_read_opts2,
            block_client::NO_TRACE_ID,
        )
        .await
        .expect("read failed");

    assert_ne!(read_buf, write_buf);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_suspend_resume() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let block_client = connect_block_client(&started_driver, "user").await;

    // Verify initial I/O.
    let buf = vec![0x12u8; 512];
    block_client.write_at(BufferSlice::from(&buf[..]), 0).await.expect("write failed");

    let driver = started_driver.get_driver().expect("failed to get driver");

    // Suspend.
    driver.suspend().await;

    // Submit a read request.  This should be blocked.
    let mut read_buf = vec![0u8; 512];
    let mut read_fut = block_client.read_at(MutableBufferSlice::from(&mut read_buf[..]), 0).boxed();

    // Give it a bit of time and make sure it hasn't completed.
    fasync::Timer::new(std::time::Duration::from_millis(100)).await;
    assert!(read_fut.as_mut().now_or_never().is_none());

    // Resume.
    driver.resume().await;

    // Now it should complete.
    read_fut.await.expect("read failed");
    assert_eq!(read_buf, buf);

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_suspend_wrong_state() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");
    let driver = started_driver.get_driver().expect("failed to get driver");
    {
        let command_queue = driver.command_queue.lock().as_ref().cloned().unwrap();

        // Shut down the command queue so its state becomes State::Disabled.
        command_queue.shutdown().await;

        // Suspending while in State::Disabled should return BAD_STATE instead of panicking.
        assert_eq!(command_queue.suspend().await.err(), Some(zx::Status::BAD_STATE));
    }

    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_immediate_suspend_after_resume() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");
    let driver = started_driver.get_driver().expect("failed to get driver");

    // Suspend.
    driver.suspend().await;

    // Resume (which now awaits completion of the resume hook).
    driver.resume().await;

    // Immediately suspending again should succeed without race or panic.
    driver.suspend().await;

    driver.resume().await;
    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_suspend_with_active_io() {
    let mut blocker = Blocker::default();
    let (fixture, mut harness) = FakeCqhci::new(Some(blocker.hook()));
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let block_client = connect_block_client(&started_driver, "user").await;

    // Perform a read to ensure the partition is switched and avoid DCMDs during the test.
    {
        let mut buf = vec![0u8; 512];
        block_client
            .read_at(MutableBufferSlice::from(&mut buf[..]), 0)
            .await
            .expect("placeholder read failed");
    }

    // Block all requests.
    blocker.block(!(1 << CQHCI_TASK_DESCRIPTOR_LIST_DCMD_SLOT));

    // Submit a read request.
    let (tx, rx) = oneshot::channel();
    fixture.scope.spawn(async move {
        let mut read_buf = vec![0u8; 512];
        block_client
            .read_at(MutableBufferSlice::from(&mut read_buf[..]), 0)
            .await
            .expect("read failed");
        let _ = tx.send(read_buf);
    });

    // Wait for the transfer to be submitted to hardware and blocked.
    let unblock = blocker.next().await;

    let driver = started_driver.get_driver().expect("failed to get driver");

    // Suspend. This should wait for the active task to finish, but since we're blocking it in the
    // hardware, suspend will block too.
    let mut suspend_fut = driver.suspend().boxed();

    // Verify suspend is blocked.
    fasync::Timer::new(std::time::Duration::from_millis(100)).await;
    assert!(suspend_fut.as_mut().now_or_never().is_none());

    // Unblock the read request.
    drop(unblock);

    // Now suspend should finish.
    suspend_fut.await;

    // Verify read finished.
    let read_buf = rx.await.expect("failed to receive read buf");
    assert_eq!(read_buf, vec![0u8; 512]);

    // Resume.
    driver.resume().await;

    // And make sure another read request completes.
    blocker.block(0);
    let mut buf = vec![0u8; 512];
    let block_client = connect_block_client(&started_driver, "user").await;
    block_client
        .read_at(MutableBufferSlice::from(&mut buf[..]), 0)
        .await
        .expect("placeholder read failed");
    assert_eq!(read_buf, vec![0u8; 512]);

    started_driver.stop_driver().await;
}

#[fuchsia::test(threads = 4)]
async fn test_no_wakeup_on_prepare_transfer_failure() {
    let mut blocker = Blocker::default();
    let (fixture, mut harness) = FakeCqhci::new(Some(blocker.hook()));
    let started_driver = harness.start_driver().await.expect("failed to start driver");
    let started_driver = Mutex::new(Some(started_driver));

    // Perform a read to ensure the partition is switched and avoid DCMDs during the test.
    let block_client = {
        let proxy = connect_block_proxy(started_driver.lock().as_ref().unwrap(), "user");
        RemoteBlockClient::new(proxy).await.expect("failed to create block client")
    };
    {
        let mut buf = vec![0u8; 512];
        block_client
            .read_at(MutableBufferSlice::from(&mut buf[..]), 0)
            .await
            .expect("placeholder read failed");
    }

    // Block all requests in hardware.
    blocker.block(!(1 << CQHCI_TASK_DESCRIPTOR_LIST_DCMD_SLOT));

    // Queue up 31 requests, which should fill all transfer slots (0..30) and leave one blocked
    // waiting for a slot.
    let read_scope = fixture.scope.new_child();
    for _ in 0..31 {
        let proxy = connect_block_proxy(started_driver.lock().as_ref().unwrap(), "user");
        read_scope.spawn(async move {
            let block_client =
                RemoteBlockClient::new(proxy).await.expect("failed to create block client");
            let mut buf = vec![0u8; 512];
            let _ = block_client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await;
        });
    }

    // Wait for 31 requests to be enqueued and submitted to the hardware.
    let mut unblock: Vec<_> = blocker.by_ref().take(31).collect().await;
    assert_eq!(unblock.len(), 31);

    // Spawn Task A: Submit an invalid request. It should block because queue is full.
    // When it wakes up, it will fail in `prepare_transfer` and drop the slot without notifying.
    let proxy_invalid = connect_block_proxy(started_driver.lock().as_ref().unwrap(), "user");
    let (tx_invalid, rx_invalid) = oneshot::channel();
    read_scope.spawn(async move {
        let block_client =
            RemoteBlockClient::new(proxy_invalid).await.expect("failed to create block client");
        let mut buf = vec![];
        let res = block_client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await;
        assert_eq!(res, Err(zx::Status::INVALID_ARGS));
        let _ = tx_invalid.send(());
    });

    // Spawn Task B: Submit a valid request. It should block because queue is full.
    // It should be woken up when a slot is freed.
    let proxy_valid = connect_block_proxy(started_driver.lock().as_ref().unwrap(), "user");
    let (tx_valid, rx_valid) = oneshot::channel();
    read_scope.spawn(async move {
        let block_client =
            RemoteBlockClient::new(proxy_valid).await.expect("failed to create block client");
        let mut buf = vec![0u8; 512];
        let res = block_client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await;
        assert_eq!(res, Ok(()));
        let _ = tx_valid.send(());
    });

    // Give them a moment to block.
    fasync::Timer::new(std::time::Duration::from_millis(100)).await;

    // Now, unblock one request in hardware.
    // This will free one slot.
    drop(unblock.pop().unwrap());

    let mut rx_invalid = rx_invalid.fuse();
    let mut next_blocked = blocker.next().fuse();
    let mut timeout = pin!(fasync::Timer::new(std::time::Duration::from_secs(5)).fuse());

    let (thread_a_finished, thread_b_unblocker) = futures::select! {
        _ = rx_invalid => {
            // Thread A finished first. Thread B must submit because Thread A freed the slot.
            let res = futures::select! {
                res = next_blocked => res,
                _ = timeout => {
                    panic!("Thread A finished but Thread B did not submit request! Lost wakeup?");
                }
            };
            println!("Thread A won the race, both finished/submitted as expected.");
            (true, res)
        }
        res = next_blocked => {
            // Thread B submitted first. Thread A should still be blocked.
            if (&mut rx_invalid).now_or_never().is_some() {
                println!("Both finished/submitted (Thread B polled first), as expected.");
                (true, res)
            } else {
                println!("Thread B won the race, Thread A is waiting.");
                (false, res)
            }
        }
        _ = timeout => {
            panic!("Timed out waiting for either thread to finish/submit!");
        }
    };

    // Clean up remaining blocked tasks so driver can progress and Thread A (if waiting) can finish.
    blocker.block(0);
    for u in unblock {
        drop(u);
    }
    if let Some(u) = thread_b_unblocker {
        drop(u);
    }

    // Ensure Thread A is finished.
    if !thread_a_finished {
        rx_invalid.await.expect("Thread A failed to run");
    }

    // Ensure Thread B completes successfully.
    let mut rx_valid = rx_valid.fuse();
    let mut cleanup_timeout = pin!(fasync::Timer::new(std::time::Duration::from_secs(5)).fuse());
    futures::select! {
        res = rx_valid => {
            res.expect("Thread B failed to complete");
        }
        _ = cleanup_timeout => {
            panic!("Timed out waiting for Thread B to complete!");
        }
    }

    let driver = started_driver.lock().take().unwrap();
    driver.stop_driver().await;
}

#[fuchsia::test(threads = 4)]
async fn test_async_task_wakeup_with_active_transfers() {
    let mut blocker = Blocker::default();
    let (fixture, mut harness) = FakeCqhci::new(Some(blocker.hook()));
    let started_driver = harness.start_driver().await.expect("failed to start driver");
    let started_driver = Mutex::new(Some(started_driver));

    // Perform a read to ensure the partition is switched to UserDataPartition.
    let block_client = {
        let proxy = connect_block_proxy(started_driver.lock().as_ref().unwrap(), "user");
        RemoteBlockClient::new(proxy).await.expect("failed to create block client")
    };
    {
        let mut buf = vec![0u8; 512];
        block_client
            .read_at(MutableBufferSlice::from(&mut buf[..]), 0)
            .await
            .expect("placeholder read failed");
    }

    // Block all requests in hardware.
    blocker.block(!(1 << CQHCI_TASK_DESCRIPTOR_LIST_DCMD_SLOT));

    // Submit 1 placeholder request to UserDataPartition. It will be fast-path and block in
    // hardware.
    let placeholder_scope = fixture.scope.new_child();
    let (tx_placeholder, rx_placeholder) = oneshot::channel();
    {
        let proxy = connect_block_proxy(started_driver.lock().as_ref().unwrap(), "user");
        placeholder_scope.spawn(async move {
            let block_client =
                RemoteBlockClient::new(proxy).await.expect("failed to create block client");
            let mut buf = vec![0u8; 512];
            let _ = block_client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await;
            let _ = tx_placeholder.send(());
        });
    }

    // Wait for placeholder request to be enqueued and submitted to hardware.
    let unblock = blocker.next().await;

    // Submit a slow-path request to BootPartition2.
    // It should acquire a slot, but block in async_task_queue because placeholder task is active.
    let proxy_slow = connect_block_proxy(started_driver.lock().as_ref().unwrap(), "boot1");
    let (tx_slow, rx_slow) = oneshot::channel();
    placeholder_scope.spawn(async move {
        let block_client =
            RemoteBlockClient::new(proxy_slow).await.expect("failed to create block client");
        let mut buf = vec![0u8; 512];
        let res = block_client.read_at(MutableBufferSlice::from(&mut buf[..]), 0).await;
        assert_eq!(res, Ok(()));
        let _ = tx_slow.send(());
    });

    // Give it a moment to block.
    fasync::Timer::new(std::time::Duration::from_millis(100)).await;

    // Now, unblock the placeholder request in hardware.
    // This should free the placeholder slot and wake up get_next_task to run the slow-path task.
    drop(unblock);

    // Wait for the placeholder task to finish.
    rx_placeholder.await.expect("placeholder task failed to finish");

    // The slow-path task should now be able to run.
    // We wait for the read to be submitted (after partition switch DCMD which is not blocked).
    let unblock_slow = blocker.next().await;
    drop(unblock_slow);

    // Now the slow-path task should complete.
    rx_slow.await.expect("slow task failed to run");

    let driver = started_driver.lock().take().unwrap();
    driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_submit_read_direct() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let block_client = connect_block_client(&started_driver, "user").await;

    // Write known data to block 0 of user partition.
    let mut write_buf = vec![0u8; 512];
    for (i, byte) in write_buf.iter_mut().enumerate() {
        *byte = (i as u8).wrapping_mul(7);
    }
    block_client.write_at(BufferSlice::from(&write_buf[..]), 0).await.expect("write failed");

    let driver = started_driver.get_driver().expect("failed to get driver");
    let cq = driver.command_queue.lock().as_ref().cloned().unwrap();

    assert_eq!(cq.minimum_contiguity(), zx::system_get_page_size() as u64);

    // Allocate a pre-pinned buffer.
    let source = storage_device::buffer_allocator::BufferSource::new(4096);
    let allocator = Arc::new(storage_device::pinned_buffer_allocator::PinnedBufferAllocator::new(
        512,
        source,
        cq.bti().duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        cq.minimum_contiguity(),
    ));
    let buffer = allocator.allocate_buffer_sync_owned(512);

    let (tx, rx) = oneshot::channel();
    cq.submit_read_direct(
        EmmcPartitionId::UserDataPartition,
        0,
        buffer,
        Box::new(move |res| {
            let _ = tx.send(res);
        }),
    )
    .expect("submit_read_direct failed");

    let buffer = rx.await.expect("channel dropped").expect("read failed");

    let mut read_buf = vec![0u8; 512];
    buffer.copy_to_slice(&mut read_buf);
    assert_eq!(read_buf, write_buf);

    // Verify invalid buffer length (unaligned) fails early with INVALID_ARGS:
    let unaligned_allocator =
        Arc::new(storage_device::pinned_buffer_allocator::PinnedBufferAllocator::new(
            256,
            storage_device::buffer_allocator::BufferSource::new(4096),
            cq.bti().duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            cq.minimum_contiguity(),
        ));
    let unaligned_buffer = unaligned_allocator.allocate_buffer_sync_owned(256);
    assert_eq!(
        cq.submit_read_direct(
            EmmcPartitionId::UserDataPartition,
            0,
            unaligned_buffer,
            Box::new(|_| {}),
        ),
        Err(zx::Status::INVALID_ARGS)
    );

    drop(cq);
    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_into_block_service_success() {
    let (_fixture, mut harness) = FakeCqhci::new(None);
    let started_driver = harness.start_driver().await.expect("failed to start driver");

    let block_client = connect_block_client(&started_driver, "user").await;

    // Write known data to block 0 of user partition.
    let mut write_buf = vec![0u8; 512];
    for (i, byte) in write_buf.iter_mut().enumerate() {
        *byte = (i as u8).wrapping_mul(11);
    }
    block_client.write_at(BufferSlice::from(&write_buf[..]), 0).await.expect("write failed");

    let driver = started_driver.get_driver().expect("failed to get driver");
    let cq = driver.command_queue.lock().as_ref().cloned().unwrap();
    let partition = Arc::new(crate::partition::EmmcPartition::new(
        EmmcPartitionId::UserDataPartition,
        Arc::downgrade(&cq),
        fblock::BlockInfo {
            block_count: 1024,
            block_size: 512,
            max_transfer_size: 0,
            flags: fblock::DeviceFlag::empty(),
        },
    ));
    let session_manager = Arc::new(SessionManager::new(partition, 512));
    let block_service = session_manager.into_block_service(&session_manager);

    let dest_buffer = block_service.allocate_buffer(4096);
    assert!(dest_buffer.len() >= 512);
    assert!(dest_buffer.paddrs().is_some());

    let (tx, rx) = oneshot::channel();
    block_service
        .read_blocks(
            0,
            dest_buffer,
            Box::new(move |res| {
                let _ = tx.send(res);
            }),
        )
        .expect("read_blocks failed");

    let res = rx.await.expect("channel dropped").expect("read failed");
    let mut read_buf = vec![0u8; 512];
    res.as_ref().subslice(..512).copy_to_slice(&mut read_buf);
    assert_eq!(read_buf, write_buf);

    drop(res);
    drop(block_service);
    drop(session_manager);
    drop(cq);
    drop(block_client);
    started_driver.stop_driver().await;
}

#[fuchsia::test]
async fn test_into_block_service_fallback() {
    let partition = Arc::new(crate::partition::EmmcPartition::new(
        EmmcPartitionId::UserDataPartition,
        Weak::new(),
        fblock::BlockInfo {
            block_count: 1024,
            block_size: 512,
            max_transfer_size: 0,
            flags: fblock::DeviceFlag::empty(),
        },
    ));
    let session_manager = Arc::new(SessionManager::new(partition, 512));
    let block_service = session_manager.into_block_service(&session_manager);

    let dest_buffer = block_service.allocate_buffer(4096);
    // The fallback DefaultCallbackBlockService allocates unpinned buffers.
    assert!(dest_buffer.paddrs().is_none());
}
