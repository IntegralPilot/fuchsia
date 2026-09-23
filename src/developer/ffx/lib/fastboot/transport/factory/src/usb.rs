// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style licence that can be
// found in the LICENSE file.

use crate::analytics::PointOfFailure;
use anyhow::Result;
use async_trait::async_trait;
use fastboot::command::{ClientVariable, Command};
use fastboot::reply::Reply;
use fastboot::{FastbootContext, send};
use ffx_diagnostics_analytics::ResultExt;
use ffx_fastboot_interface::interface_factory::{
    InterfaceFactory, InterfaceFactoryBase, InterfaceFactoryError,
};
use fuchsia_async::{TimeoutExt, Timer};
use futures::channel::oneshot::{Sender, channel};
use std::time::Duration;
use usb_fastboot_discovery::{
    DefaultSerialFinder, FastbootEvent, FastbootEventHandler, FastbootUsbLiveTester,
    FastbootUsbTester, FastbootUsbWatcher, Interface as AsyncInterface, SerialNumberFinder,
    UnversionedFastbootUsbTester, open_interface_with_serial, wait_for_live,
};

///////////////////////////////////////////////////////////////////////////////
// UsbFactory
//

#[derive(Default, Debug, Clone)]
pub struct UsbFactory {
    serial: String,
}

/// Interval between polling passes while waiting for the target to disconnect from USB.
const DEFAULT_DISCONNECT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Maximum time to wait for the target to physically drop from the USB bus during a reboot.
const DEFAULT_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Discovery polling interval for FastbootUsbWatcher.
const DEFAULT_DISCOVERY_INTERVAL: Duration = Duration::from_secs(1);

/// Sleep duration between consecutive getvar liveness checks on the rediscovered target.
const DEFAULT_LIVE_SLEEP_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RediscoveryConfig {
    pub disconnect_poll_interval: Duration,
    pub disconnect_timeout: Duration,
    pub discovery_interval: Duration,
    pub live_sleep_interval: Duration,
}

impl Default for RediscoveryConfig {
    fn default() -> Self {
        Self {
            disconnect_poll_interval: DEFAULT_DISCONNECT_POLL_INTERVAL,
            disconnect_timeout: DEFAULT_DISCONNECT_TIMEOUT,
            discovery_interval: DEFAULT_DISCOVERY_INTERVAL,
            live_sleep_interval: DEFAULT_LIVE_SLEEP_INTERVAL,
        }
    }
}

impl UsbFactory {
    pub fn new(serial: String) -> Self {
        Self { serial }
    }

    pub(crate) async fn rediscover_impl<W, O, T>(
        &mut self,
        mut finder: W,
        usb_tester: O,
        mut live_tester: T,
        config: RediscoveryConfig,
    ) -> Result<(), InterfaceFactoryError>
    where
        W: SerialNumberFinder,
        O: FastbootUsbTester,
        T: FastbootUsbLiveTester,
    {
        log::debug!("Rediscovering devices for serial: {}", self.serial);

        wait_for_disconnect(
            self.serial.as_str(),
            &mut finder,
            config.disconnect_poll_interval,
            config.disconnect_timeout,
        )
        .await;

        let (tx, rx) = channel::<()>();
        // Handler will handle the found usb devices.
        // Will filter to only usb devices that match the given serial number
        // Will send a signal once both the serial number is found _and_
        // is readily accepting fastboot commands
        let handler = UsbTargetHandler { tx: Some(tx), target_serial: self.serial.clone() };

        // This is usb therefore we only need to find usb targets
        let watcher =
            FastbootUsbWatcher::new(handler, finder, usb_tester, config.discovery_interval);

        rx.await
            .map_err(|e| {
                InterfaceFactoryError::from(anyhow::anyhow!(
                    "error awaiting oneshot channel rediscovering target: {}",
                    e
                ))
            })
            .or_else_analytics(|e| PointOfFailure::FactoryRediscoveryError("usb".into(), e).into())
            .await?;

        // Explicitly drop the watcher now that discovery has succeeded to terminate its
        // background discovery loop and release any USB resources/claims before testing
        // liveness or issuing fastboot commands to the rediscovered target.
        drop(watcher);

        log::debug!("Rediscovered device with serial {}. Waiting for it to be live", self.serial);
        wait_for_live(self.serial.as_str(), &mut live_tester, config.live_sleep_interval).await;

        Ok(())
    }
}

/// Polls until the USB target with the specified `serial` is no longer enumerated,
/// or until `timeout` has elapsed.
pub(crate) async fn wait_for_disconnect<F>(
    serial: &str,
    finder: &mut F,
    poll_interval: Duration,
    timeout: Duration,
) where
    F: SerialNumberFinder,
{
    // Check if the target is already disconnected before arming a timeout.
    let serials = finder.find_serial_numbers().await;
    if !serials.iter().any(|s| s == serial) {
        log::debug!("USB target {serial} disconnected");
        return;
    }

    let poll_fut = async {
        loop {
            Timer::new(poll_interval).await;
            let serials = finder.find_serial_numbers().await;
            if !serials.iter().any(|s| s == serial) {
                log::debug!("USB target {serial} disconnected");
                return true;
            }
        }
    };
    if !poll_fut.on_timeout(timeout, || false).await {
        log::debug!(
            "Timed out waiting for USB target {serial} to disconnect; proceeding to discovery"
        );
    }
}

struct UsbTargetHandler {
    // The serial number we are looking for
    target_serial: String,
    // Channel to send on once we find the usb device with the provided serial
    // number is up and ready to accept fastboot commands
    tx: Option<Sender<()>>,
}

impl FastbootEventHandler for UsbTargetHandler {
    async fn handle_event(&mut self, event: FastbootEvent) {
        if self.tx.is_none() {
            log::debug!("Handling event: {:?} but our sender is none. Returning early.", event);
            return;
        }
        match event {
            FastbootEvent::Discovered(s) if s == self.target_serial => {
                log::debug!(
                    "Discovered target with serial: {} we were looking for!",
                    self.target_serial
                );
                let _ = self.tx.take().unwrap().send(());
            }
            FastbootEvent::Discovered(s) => log::debug!(
                "Attempting to rediscover target with serial: {}. Found target: {}",
                self.target_serial,
                s,
            ),
            FastbootEvent::Lost(l) => log::debug!(
                "Attempting to rediscover target with serial: {}. Lost target: {}",
                self.target_serial,
                l,
            ),
        }
    }
}

pub struct StrictGetVarFastbootUsbLiveTester {
    serial: String,
}

impl FastbootUsbLiveTester for StrictGetVarFastbootUsbLiveTester {
    async fn is_fastboot_usb_live(&mut self, serial: &str) -> bool {
        if serial != self.serial {
            return false;
        }
        let Ok(mut interface) = open_interface_with_serial(serial) else {
            return false;
        };

        match send(FastbootContext::new(), Command::GetVar(ClientVariable::Version), &mut interface)
            .await
        {
            Ok(Reply::Okay(version)) => {
                log::debug!("USB serial {serial}: fastboot version: {version}");
                true
            }
            Ok(Reply::Fail(message)) => {
                log::warn!(
                    "Failed to get variable \"version\" with message: \"{message}\". but we communicated over fastboot protocol... continuing"
                );
                true
            }
            Err(e) => {
                log::debug!(
                    "USB serial {serial}: could not communicate over Fastboot protocol. Error: {e:#?}"
                );
                false
            }
            e => {
                log::debug!(
                    "USB serial {serial}: got unexpected response getting variable: {e:#?}"
                );
                false
            }
        }
    }
}

#[async_trait]
impl InterfaceFactoryBase<AsyncInterface> for UsbFactory {
    async fn open(&mut self) -> Result<AsyncInterface, InterfaceFactoryError> {
        let interface = open_interface_with_serial(&self.serial)
            .map_err(InterfaceFactoryError::Usb)
            .or_else_analytics(|e| PointOfFailure::FactoryOpenError("usb".to_owned(), e).into())
            .await?;
        log::debug!("serial now in use: {}", self.serial);
        Ok(interface)
    }

    async fn close(&self) {
        log::debug!("dropping UsbFactory for serial: {}", self.serial);
    }

    async fn rediscover(&mut self) -> Result<(), InterfaceFactoryError> {
        self.rediscover_impl(
            DefaultSerialFinder {},
            // This tester will not attempt to talk to the USB devices to extract version info, it
            // only inspects the USB interface
            UnversionedFastbootUsbTester {},
            StrictGetVarFastbootUsbLiveTester { serial: self.serial.clone() },
            RediscoveryConfig::default(),
        )
        .await
    }
}

impl Drop for UsbFactory {
    fn drop(&mut self) {
        futures::executor::block_on(async move {
            self.close().await;
        });
    }
}

impl InterfaceFactory<AsyncInterface> for UsbFactory {}

#[cfg(test)]
mod test {
    use super::*;

    ///////////////////////////////////////////////////////////////////////////////
    // UsbTargetHandler
    //

    #[fuchsia::test]
    async fn handle_target_test() -> Result<()> {
        let target_serial = "1234567890".to_string();

        let (tx, mut rx) = channel::<()>();
        let mut handler = UsbTargetHandler { tx: Some(tx), target_serial: target_serial.clone() };

        //Lost our serial
        handler.handle_event(FastbootEvent::Lost(target_serial.clone())).await;
        assert!(rx.try_recv().unwrap().is_none());
        // Lost a different serial
        handler.handle_event(FastbootEvent::Lost("1234asdf".to_string())).await;
        assert!(rx.try_recv().unwrap().is_none());
        // Found a new serial
        handler.handle_event(FastbootEvent::Discovered("1234asdf".to_string())).await;
        assert!(rx.try_recv().unwrap().is_none());
        // Found our serial
        handler.handle_event(FastbootEvent::Discovered(target_serial.clone())).await;
        assert!(rx.try_recv().unwrap().is_some());

        Ok(())
    }

    #[derive(Clone)]
    struct MockFinder {
        results: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>,
        observed_disconnect: std::sync::Arc<std::sync::atomic::AtomicBool>,
        call_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        repeat_last: bool,
    }

    impl MockFinder {
        fn new(results: Vec<Vec<String>>) -> Self {
            Self {
                results: std::sync::Arc::new(std::sync::Mutex::new(results)),
                observed_disconnect: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                call_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                repeat_last: false,
            }
        }

        fn new_repeating(results: Vec<Vec<String>>) -> Self {
            Self {
                results: std::sync::Arc::new(std::sync::Mutex::new(results)),
                observed_disconnect: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                call_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                repeat_last: true,
            }
        }
    }

    impl SerialNumberFinder for MockFinder {
        async fn find_serial_numbers(&mut self) -> Vec<String> {
            self.call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut res = self.results.lock().unwrap();
            let next = if self.repeat_last && res.len() == 1 {
                res[0].clone()
            } else if !res.is_empty() {
                res.remove(0)
            } else {
                vec![]
            };
            if next.is_empty() {
                self.observed_disconnect.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            next
        }
    }

    struct MockUsbTester;

    impl FastbootUsbTester for MockUsbTester {
        async fn is_fastboot_usb(&mut self, _serial: &str) -> bool {
            true
        }
    }

    struct MockLiveTester {
        expected_serial: String,
        observed_disconnect: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl FastbootUsbLiveTester for MockLiveTester {
        async fn is_fastboot_usb_live(&mut self, serial: &str) -> bool {
            if serial != self.expected_serial {
                return false;
            }
            // In real hardware, querying a target before it physically disconnects
            // and re-enumerates fails with a protocol error (Os code 71).
            assert!(
                self.observed_disconnect.load(std::sync::atomic::Ordering::SeqCst),
                "is_fastboot_usb_live was called before the target disconnected!"
            );
            true
        }
    }

    #[fuchsia::test]
    async fn test_wait_for_disconnect_when_initially_connected() {
        let mut finder = MockFinder::new(vec![
            vec!["target123".to_string()],
            vec!["target123".to_string()],
            vec![],
        ]);
        wait_for_disconnect("target123", &mut finder, Duration::ZERO, Duration::from_secs(1)).await;
        assert!(finder.results.lock().unwrap().is_empty());
        assert!(finder.observed_disconnect.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[fuchsia::test]
    async fn test_wait_for_disconnect_already_disconnected() {
        let mut finder = MockFinder::new(vec![vec![]]);
        wait_for_disconnect("target123", &mut finder, Duration::ZERO, Duration::from_secs(1)).await;
        assert!(finder.results.lock().unwrap().is_empty());
        assert!(finder.observed_disconnect.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[fuchsia::test]
    async fn test_wait_for_disconnect_timeout() {
        let mut finder = MockFinder::new_repeating(vec![vec!["target123".to_string()]]);
        // Duration::ZERO timeout expires immediately on the first poll without waiting or sleeping.
        wait_for_disconnect("target123", &mut finder, Duration::from_secs(1), Duration::ZERO).await;
        // Times out gracefully without observing a disconnect.
        assert!(!finder.observed_disconnect.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(finder.call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[fuchsia::test]
    async fn test_rediscover_waits_for_disconnect_and_reconnects() -> Result<()> {
        let target_serial = "target123".to_string();
        let mut factory = UsbFactory::new(target_serial.clone());

        // First 2 calls to finder during wait_for_disconnect: device connected, then disconnected.
        // Third call during FastbootUsbWatcher: device discovered.
        let finder = MockFinder::new_repeating(vec![
            vec![target_serial.clone()],
            vec![],
            vec![target_serial.clone()],
        ]);
        let observed_disconnect = finder.observed_disconnect.clone();

        let tester = MockLiveTester { expected_serial: target_serial.clone(), observed_disconnect };

        factory
            .rediscover_impl(
                finder,
                MockUsbTester,
                tester,
                RediscoveryConfig {
                    disconnect_poll_interval: Duration::ZERO,
                    disconnect_timeout: Duration::from_secs(1),
                    discovery_interval: Duration::ZERO,
                    live_sleep_interval: Duration::ZERO,
                },
            )
            .await?;

        Ok(())
    }

    #[fuchsia::test]
    async fn test_strict_live_tester_serial_mismatch() {
        let mut tester = StrictGetVarFastbootUsbLiveTester { serial: "correct_serial".to_string() };
        assert!(!tester.is_fastboot_usb_live("wrong_serial").await);
    }
}
