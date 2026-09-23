// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_starnix_psi::{
    PsiProviderGetMemoryPressureStatsResponse, PsiProviderRequest,
    PsiProviderWatchMemoryStallResponse, PsiStats,
};
use std::sync::Arc;
use test_case::test_case;

mod event_waiter;
use event_waiter::{EventWaiter, make_epoll_waiter, make_poll_waiter};
mod fake_psi_provider;
use fake_psi_provider::*;
mod puppet;
use puppet::*;

async fn start_puppet_with_fake_psi_provider() -> (PuppetInstance, Arc<FakePsiProvider>) {
    // Start Starnix with our fake PsiProvider and expect its initial probe.
    let fake_psi_provider = Arc::new(FakePsiProvider::new());
    let puppet = fake_psi_provider
        .clone()
        .with_expected_request(
            |request| {
                // Before the puppet starts to run, the Starnix kernel will make an initial request
                // to probe if the PsiProvider is actually connected. As it doesn't use the returned
                // data, we can just reply with zeros.
                let PsiProviderRequest::GetMemoryPressureStats { responder } = request else {
                    panic!("Unexpected request received")
                };
                let zeros = PsiStats {
                    avg10: Some(0.0),
                    avg60: Some(0.0),
                    avg300: Some(0.0),
                    total: Some(0),
                    ..Default::default()
                };
                let response = PsiProviderGetMemoryPressureStatsResponse {
                    some: Some(zeros.clone()),
                    full: Some(zeros.clone()),
                    ..Default::default()
                };
                responder.send(Ok(&response)).unwrap();
            },
            async { PuppetInstance::new(Some(fake_psi_provider.clone())).await },
        )
        .await;

    (puppet, fake_psi_provider)
}

#[fuchsia::test]
async fn test_read_psi_memory_stats() {
    let (mut puppet, fake_psi_provider) = start_puppet_with_fake_psi_provider().await;

    fake_psi_provider
        .with_expected_request(
            |request| {
                let PsiProviderRequest::GetMemoryPressureStats { responder } = request else {
                    panic!("Unexpected request received")
                };
                let some = PsiStats {
                    avg10: Some(0.08),
                    avg60: Some(0.9),
                    avg300: Some(1.0),
                    total: Some(5678 * 1000),
                    ..Default::default()
                };
                let full = PsiStats {
                    avg10: Some(0.5),
                    avg60: Some(0.6),
                    avg300: Some(0.77),
                    total: Some(1234 * 1000),
                    ..Default::default()
                };
                let response = PsiProviderGetMemoryPressureStatsResponse {
                    some: Some(some),
                    full: Some(full),
                    ..Default::default()
                };
                responder.send(Ok(&response)).unwrap();
            },
            async {
                let fd = puppet.open("/proc/pressure/memory").await;
                let contents = puppet.read_to_end(fd).await;
                assert_eq!(
                    contents,
                    "some avg10=0.08 avg60=0.90 avg300=1.00 total=5678\n\
                     full avg10=0.50 avg60=0.60 avg300=0.77 total=1234\n"
                );
                puppet.close(fd).await;
            },
        )
        .await;

    puppet.check_exit_clean().await;
}

// Test files that are just stubs too.
#[test_case("cpu")]
#[test_case("io")]
#[fuchsia::test]
async fn test_read_psi_stub_stats(kind: &str) {
    let (mut puppet, _fake_psi_provider) = start_puppet_with_fake_psi_provider().await;

    let fd = puppet.open(&format!("/proc/pressure/{kind}")).await;
    let contents = puppet.read_to_end(fd).await;
    assert_eq!(
        contents,
        "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n\
         full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n"
    );
    puppet.close(fd).await;

    puppet.check_exit_clean().await;
}

// Verify that the pressure directory is not created if no PsiProvider is given.
#[fuchsia::test]
async fn test_psi_unavailable() {
    // Start Starnix without a PsiProvider (which is optional in its manifest).
    let mut puppet = PuppetInstance::new(None).await;

    assert!(puppet.check_exists("/proc/pressure").await == false);

    puppet.check_exit_clean().await;
}

/// Given a function that builds an EventWaiter from a PuppetInstance and an
/// open PSI file descriptor in it, tests that said EventWaiter delivers the
/// expected PSI events.
#[test_case(make_poll_waiter => ignore; "poll")]
#[test_case(make_epoll_waiter; "epoll")]
#[fuchsia::test]
async fn test_wait<T>(make_waiter: T)
where
    for<'p> T:
        AsyncFn(&'p mut PuppetInstance, PuppetFileDescriptor) -> Box<dyn EventWaiter<'p> + 'p>,
{
    let (mut puppet, fake_psi_provider) = start_puppet_with_fake_psi_provider().await;

    let event = zx::Event::create();
    let event_dup = event
        .duplicate_handle(zx::Rights::WAIT | zx::Rights::DUPLICATE | zx::Rights::TRANSFER)
        .unwrap();

    let fd = fake_psi_provider
        .with_expected_request(
            |request| {
                let PsiProviderRequest::WatchMemoryStall { payload, responder } = request else {
                    panic!("Unexpected request received")
                };
                assert_eq!(payload.kind.unwrap(), zx::sys::ZX_SYSTEM_MEMORY_STALL_SOME);
                assert_eq!(payload.threshold.unwrap(), 1234 * 1000);
                assert_eq!(payload.window.unwrap(), 1000000 * 1000);
                responder
                    .send(Ok(PsiProviderWatchMemoryStallResponse {
                        event: Some(event_dup),
                        ..Default::default()
                    }))
                    .unwrap();
            },
            async {
                let fd = puppet.open("/proc/pressure/memory").await;
                puppet.write_all(fd, "some 1234 1000000").await;
                fd
            },
        )
        .await;

    let mut waiter = make_waiter(&mut puppet, fd).await;

    // The event is not asserted: it should report no events.
    assert_eq!(waiter.wait(1000).await, false);

    // Pulse the event and sleep for longer than the window. A PSI event should be latched and
    // delivered, even if the wait starts after the sleep.
    event.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();
    event.signal(zx::Signals::EVENT_SIGNALED, zx::Signals::empty()).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert_eq!(waiter.wait(0).await, true);

    // Expect no events to be further delivered if waited again, as the event is no longer signaled.
    assert_eq!(waiter.wait(100).await, false);

    // Signal it again and expect the wait result to reflect it.
    event.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();
    assert_eq!(waiter.wait(5000).await, true);

    // The next wait, however, should not report any event until the window has elapsed.
    assert_eq!(waiter.wait(100).await, false);
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert_eq!(waiter.wait(0).await, true);

    waiter.destroy().await;

    puppet.close(fd).await;
    puppet.check_exit_clean().await;
}
