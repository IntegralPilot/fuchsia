// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! High-level touch controller state management for the Goodix GT6853 Touch IC.

use crate::data_types::hardware_limits::MAX_CONTACTS;
use crate::data_types::reports::{
    EventStatus, InitialTouchReport, RemainingTouchContacts, TouchContact,
};
use crate::hardware_units::i2c::MessageInterfaceUnitI2c;
use fidl_next_fuchsia_hardware_gpio as fidl_gpio;
use fuchsia_async::OnInterrupt;
use futures::StreamExt;
use std::pin::Pin;
use std::sync::Arc;

/// Type alias for the injected touch report callback handler.
pub type TouchReportHandler = Arc<dyn Fn(&[TouchContact], zx::MonotonicInstant) + Send + Sync>;

/// Touch controller managing the GT6853 touch IC state and coordinate reporting.
pub struct Controller<K: zx::InterruptKind = zx::RealInterruptKind> {
    i2c: MessageInterfaceUnitI2c,
    attention_interrupt: Pin<Box<OnInterrupt<K, zx::MonotonicTimeline>>>,
    /// Duplicated handle of [`Self::attention_interrupt`] used to acknowledge
    /// the interrupt.
    attention_interrupt_ack: Arc<zx::Interrupt<K, zx::MonotonicTimeline>>,
    #[expect(unused)]
    interrupt_gpio: Option<fidl_next::Client<fidl_gpio::Gpio>>,
    touch_report_handler: Option<TouchReportHandler>,
}

impl Controller<zx::RealInterruptKind> {
    /// Creates a new [`Controller`] using real hardware interrupts from the platform GPIO.
    pub async fn new(
        i2c: MessageInterfaceUnitI2c,
        interrupt_gpio: fidl_next::Client<fidl_gpio::Gpio>,
    ) -> Result<Self, zx::Status> {
        map_fidl_result(
            interrupt_gpio.set_buffer_mode(fidl_gpio::BufferMode::Input).await,
            "setting interrupt GPIO buffer mode to input",
        )?;

        let initial_pin =
            map_fidl_result(interrupt_gpio.read().await, "reading initial interrupt GPIO level")?;
        log::debug!("Initial touch interrupt GPIO level: {:?}", initial_pin.value);

        map_fidl_result(
            interrupt_gpio
                .configure_interrupt(&fidl_gpio::InterruptConfiguration {
                    mode: Some(fidl_gpio::InterruptMode::EdgeLow),
                    ..Default::default()
                })
                .await,
            "configuring interrupt GPIO to EdgeLow",
        )?;

        let interrupt_result = map_fidl_result(
            interrupt_gpio.get_interrupt(fidl_gpio::InterruptOptions::TIMESTAMP_MONO).await,
            "getting interrupt from GPIO",
        )?;

        let interrupt_handle: zx::NullableHandle = interrupt_result.interrupt.into();
        let interrupt =
            zx::Interrupt::<zx::RealInterruptKind, zx::MonotonicTimeline>::from(interrupt_handle);

        // Clear any leftover event status from boot to release the active-low INT line
        // back to HIGH.
        i2c.write_reg(&EventStatus::CLEAR).await.map_err(|status| {
            log::warn!("Failed to clear initial EventStatus: {status:?}");
            status
        })?;
        log::debug!("GT6853 initial buffer cleared and INT line released HIGH");

        Ok(Self::new_with_interrupt(i2c, interrupt, Some(interrupt_gpio)))
    }
}

impl<K: zx::InterruptKind> Controller<K> {
    /// Creates a new [`Controller`] wrapping the given [`i2c`] interface unit and interrupt handle.
    pub fn new_with_interrupt(
        i2c: MessageInterfaceUnitI2c,
        attention_interrupt: zx::Interrupt<K, zx::MonotonicTimeline>,
        interrupt_gpio: Option<fidl_next::Client<fidl_gpio::Gpio>>,
    ) -> Self {
        let attention_interrupt_ack = Arc::new(
            attention_interrupt
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .expect("Failed to duplicate attention interrupt for ACK Arc"),
        );
        let attention_interrupt = Box::pin(OnInterrupt::new(attention_interrupt));
        Self {
            i2c,
            attention_interrupt,
            attention_interrupt_ack,
            interrupt_gpio,
            touch_report_handler: None,
        }
    }

    /// Sets the callback handler invoked when touch reports are processed.
    pub fn set_touch_report_handler(&mut self, handler: TouchReportHandler) {
        self.touch_report_handler = Some(handler);
    }

    async fn read_remaining_contacts(
        &self,
        remaining_count: usize,
    ) -> Result<Vec<TouchContact>, zx::Status> {
        // TODO(https://fxbug.dev/558557195): Investigate alternative designs for reading variable
        // remaining contacts.
        macro_rules! match_remaining {
            ($($n:literal),*) => {
                match remaining_count {
                    $(
                        $n => self
                            .i2c
                            .read_reg::<RemainingTouchContacts<$n>>()
                            .await
                            .map(|r| r.contacts.to_vec()),
                    )*
                    _ => {
                        log::warn!("Invalid remaining contact count: {remaining_count}");
                        Err(zx::Status::BAD_STATE)
                    }
                }
            };
        }

        match_remaining!(1, 2, 3, 4, 5, 6, 7, 8, 9)
    }

    /// Awaits a single hardware attention interrupt and processes the touch report.
    ///
    /// Always acknowledges the interrupt before returning, ensuring the interrupt line
    /// is unmasked even if touch report processing encounters errors.
    pub async fn process_interrupt(&mut self) -> Result<bool, zx::Status> {
        let event_time = match self.attention_interrupt.next().await {
            Some(Ok(timestamp)) => timestamp,
            Some(Err(error)) => {
                log::warn!("Error waiting for touch interrupt: {error:?}");
                return Err(error);
            }
            None => {
                log::debug!("Touch interrupt stream closed");
                return Err(zx::Status::CANCELED);
            }
        };

        log::debug!("Touch interrupt triggered");

        let attention_interrupt_ack = Arc::clone(&self.attention_interrupt_ack);
        scopeguard::defer! {
            if let Err(error) = attention_interrupt_ack.ack() {
                log::warn!("Failed to acknowledge touch interrupt: {error:?}");
            }
        }

        self.process_touch_report(event_time).await
    }

    /// Reads and processes coordinate data from the hardware touch buffer, dispatching active
    /// contacts and release events to [`Self::touch_report_handler`].
    pub async fn process_touch_report(
        &mut self,
        event_time: zx::MonotonicInstant,
    ) -> Result<bool, zx::Status> {
        let report = self.i2c.read_reg::<InitialTouchReport>().await?;

        if !report.status.coordinates_ready() {
            return Ok(false);
        }

        let count = report.contact_count.count();
        if count > MAX_CONTACTS {
            log::warn!("Touch event with too many contacts: {count}");
            // Release the buffer lock to allow hardware recovery.
            self.i2c.write_reg(&EventStatus::CLEAR).await?;
            return Err(zx::Status::BAD_STATE);
        }

        if count > 0 {
            log::debug!("Touch event: count={count}");
            // TODO(https://fxbug.dev/558521383): Validate the coordinate checksum.
            let remaining =
                if count > 1 { self.read_remaining_contacts(count - 1).await? } else { Vec::new() };

            let mut contacts = Vec::with_capacity(count);
            contacts.push(report.first_contact);
            contacts.extend(remaining);

            for (index, contact) in contacts.iter().enumerate() {
                log::debug!(
                    "  contact[{index}]: id={}, x={}, y={}, pressure={}",
                    contact.id(),
                    contact.x(),
                    contact.y(),
                    contact.pressure()
                );
            }

            if let Some(handler) = &self.touch_report_handler {
                handler(&contacts, event_time);
            }
        } else {
            log::debug!("Touch release (all contacts lifted)");
            if let Some(handler) = &self.touch_report_handler {
                handler(&[], event_time);
            }
        }

        // Acknowledge the touch report and release the coordinate buffer lock.
        self.i2c.write_reg(&EventStatus::CLEAR).await?;

        Ok(true)
    }

    /// Runs the interrupt-driven touch event loop indefinitely.
    pub async fn run(mut self) {
        loop {
            match self.process_interrupt().await {
                Ok(_) => {}
                Err(zx::Status::CANCELED) => {
                    log::info!("Touch interrupt stream closed, exiting controller run loop");
                    break;
                }
                Err(error) => {
                    log::warn!("Error during touch interrupt handling: {error:?}");
                }
            }
        }
    }
}

trait IntoStatus {
    fn into_status(self) -> zx::Status;
}

impl IntoStatus for zx::Status {
    fn into_status(self) -> zx::Status {
        self
    }
}

impl IntoStatus for Result<(), zx::Status> {
    fn into_status(self) -> zx::Status {
        self.err().unwrap_or(zx::Status::INTERNAL)
    }
}

/// Converts a `fidl_next` IPC result into a flat `Result<T, zx::Status>`.
fn map_fidl_result<T, E: IntoStatus>(
    result: Result<Result<T, E>, fidl_next::Error>,
    operation_name: &str,
) -> Result<T, zx::Status> {
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            let status = error.into_status();
            log::warn!("{operation_name} failed: {status}");
            Err(status)
        }
        Err(error) => {
            log::error!("FIDL error calling {operation_name}: {error:?}");
            let status = match error {
                fidl_next::Error::Protocol(fidl_next::ProtocolError::TransportError(status)) => {
                    status
                }
                fidl_next::Error::Protocol(fidl_next::ProtocolError::PeerClosed) => {
                    zx::Status::PEER_CLOSED
                }
                fidl_next::Error::Protocol(fidl_next::ProtocolError::PeerClosedWithEpitaph(
                    status,
                )) => zx::Status::err_from_raw(status),
                _ => zx::Status::IO,
            };
            Err(status)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_types::traits::AddressableRegister;
    use crate::testing::mock_i2c::MockI2cDevice;
    use fidl_next_fuchsia_hardware_i2c as fidl_i2c;
    use fuchsia_async as fasync;

    /// Runs a future to completion on a single-threaded executor whose port supports
    /// binding interrupts (`BIND_TO_INTERRUPT`).
    // TODO(https://fxbug.dev/539658337): Use #[fuchsia::test] directly once it supports enabling
    // BIND_TO_INTERRUPT on the underlying executor port.
    fn run_test_with_interrupts<F: std::future::Future<Output = ()>>(fut: F) {
        let port = zx::Port::create_with_opts(zx::PortOptions::BIND_TO_INTERRUPT);
        let mut exec = fasync::TestExecutor::builder().port(port).build();
        exec.run_singlethreaded(fut);
    }

    struct TestFixture {
        mock: MockI2cDevice,
        controller: Controller<zx::VirtualInterruptKind>,
        interrupt: zx::Interrupt<zx::VirtualInterruptKind, zx::MonotonicTimeline>,
        // The background server task must be kept alive for the lifetime of the test fixture.
        #[expect(dead_code)]
        i2c_server_task: fasync::Task<()>,
    }

    impl TestFixture {
        fn new() -> Self {
            let mock = MockI2cDevice::new();
            let (i2c_client_end, i2c_server_end) =
                fidl_next::fuchsia::create_channel::<fidl_i2c::Device>();
            let client = i2c_client_end.spawn();
            let server_task = i2c_server_end.spawn(mock.clone());
            let i2c_server_task = fasync::Task::spawn(async move {
                let _ = server_task.await;
            });
            let unit = MessageInterfaceUnitI2c::new(client);
            let interrupt =
                zx::Interrupt::<zx::VirtualInterruptKind, zx::MonotonicTimeline>::create_virtual()
                    .expect("Failed to create virtual interrupt");
            let interrupt_dup = interrupt
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .expect("Failed to duplicate virtual interrupt");
            let controller = Controller::new_with_interrupt(unit, interrupt_dup, None);
            Self { mock, controller, interrupt, i2c_server_task }
        }
    }

    #[fuchsia::test]
    async fn test_process_touch_report_idle() {
        let mut fixture = TestFixture::new();
        let idle_report = vec![0x00; 10];
        fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(idle_report));

        let event_time = zx::MonotonicInstant::from_nanos(123_456_789);
        let has_event = fixture
            .controller
            .process_touch_report(event_time)
            .await
            .expect("process_touch_report failed");
        assert!(!has_event);
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_process_touch_report_single_touch() {
        let mut fixture = TestFixture::new();
        // EventStatus: coordinates_ready = true (0x80), ContactCount = 1 (0x01)
        let mut report_bytes = vec![0x80, 0x01];
        // contact0: id=0, x=530 (0x0212), y=940 (0x03AC), pressure=40 (0x28)
        let contact0 = [0x00, 0x12, 0x02, 0xAC, 0x03, 0x28, 0x00, 0x00];
        report_bytes.extend_from_slice(&contact0);

        fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(report_bytes));
        fixture.mock.expect_write(EventStatus::ADDRESS, &[0x00], Ok(()));

        let event_time = zx::MonotonicInstant::from_nanos(123_456_789);
        let has_event = fixture
            .controller
            .process_touch_report(event_time)
            .await
            .expect("process_touch_report failed");
        assert!(has_event);
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_process_touch_report_two_contacts() {
        let mut fixture = TestFixture::new();
        // EventStatus: coordinates_ready = true (0x80), ContactCount = 2 (0x02)
        let mut initial_report_bytes = vec![0x80, 0x02];
        // contact0: id=0, x=300 (0x012C), y=400 (0x0190), pressure=50 (0x32)
        let contact0 = [0x00, 0x2C, 0x01, 0x90, 0x01, 0x32, 0x00, 0x00];
        initial_report_bytes.extend_from_slice(&contact0);

        // Remaining 1 contact (contact1): id=1, x=500 (0x01F4), y=600 (0x0258), pressure=60 (0x3C)
        let contact1 = [0x01, 0xF4, 0x01, 0x58, 0x02, 0x3C, 0x00, 0x00];

        fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(initial_report_bytes));
        fixture.mock.expect_read(RemainingTouchContacts::<1>::ADDRESS, 8, Ok(contact1.to_vec()));
        fixture.mock.expect_write(EventStatus::ADDRESS, &[0x00], Ok(()));

        let event_time = zx::MonotonicInstant::from_nanos(123_456_789);
        let has_event = fixture
            .controller
            .process_touch_report(event_time)
            .await
            .expect("process_touch_report failed");
        assert!(has_event);
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_process_touch_report_three_contacts() {
        let mut fixture = TestFixture::new();
        // EventStatus: coordinates_ready = true (0x80), ContactCount = 3 (0x03)
        let mut initial_report_bytes = vec![0x80, 0x03];
        // contact0: id=0, x=100 (0x0064), y=200 (0x00C8), pressure=30 (0x1E)
        let contact0 = [0x00, 0x64, 0x00, 0xC8, 0x00, 0x1E, 0x00, 0x00];
        initial_report_bytes.extend_from_slice(&contact0);

        // Remaining 2 contacts (contact1 and contact2) read together in a single pass (16 bytes).
        // contact1: id=1, x=300 (0x012C), y=400 (0x0190), pressure=40 (0x28)
        let contact1 = [0x01, 0x2C, 0x01, 0x90, 0x01, 0x28, 0x00, 0x00];
        // contact2: id=2, x=500 (0x01F4), y=600 (0x0258), pressure=50 (0x32)
        let contact2 = [0x02, 0xF4, 0x01, 0x58, 0x02, 0x32, 0x00, 0x00];
        let mut remaining_bytes = Vec::new();
        remaining_bytes.extend_from_slice(&contact1);
        remaining_bytes.extend_from_slice(&contact2);

        fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(initial_report_bytes));
        fixture.mock.expect_read(RemainingTouchContacts::<2>::ADDRESS, 16, Ok(remaining_bytes));
        fixture.mock.expect_write(EventStatus::ADDRESS, &[0x00], Ok(()));

        let event_time = zx::MonotonicInstant::from_nanos(123_456_789);
        let has_event = fixture
            .controller
            .process_touch_report(event_time)
            .await
            .expect("process_touch_report failed");
        assert!(has_event);
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_process_touch_report_zero_contacts() {
        let mut fixture = TestFixture::new();

        // Hardware reports coordinates_ready = true (0x80), but contact_count = 0 (0x00).
        let report_bytes = vec![0x80, 0x00, 0, 0, 0, 0, 0, 0, 0, 0];
        fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(report_bytes));
        fixture.mock.expect_write(EventStatus::ADDRESS, &[0x00], Ok(()));

        let event_time = zx::MonotonicInstant::from_nanos(123_456_789);
        let has_event = fixture
            .controller
            .process_touch_report(event_time)
            .await
            .expect("process_touch_report failed");
        assert!(has_event);
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_process_touch_report_invalid_contact_count() {
        let mut fixture = TestFixture::new();

        // Hardware reports coordinates_ready = true (0x80), but contact_count = 15
        // (0x0F > MAX_CONTACTS).
        let mut report_bytes = vec![0x80, 0x0F];
        report_bytes.extend_from_slice(&[0u8; 8]);

        fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(report_bytes));
        fixture.mock.expect_write(EventStatus::ADDRESS, &[0x00], Ok(()));

        let event_time = zx::MonotonicInstant::from_nanos(123_456_789);
        let result = fixture.controller.process_touch_report(event_time).await;
        assert_eq!(result.err(), Some(zx::Status::BAD_STATE));
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_process_touch_report_read_error_propagates() {
        let mut fixture = TestFixture::new();
        fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Err(zx::Status::PEER_CLOSED));

        let event_time = zx::MonotonicInstant::from_nanos(123_456_789);
        let result = fixture.controller.process_touch_report(event_time).await;
        assert_eq!(result.err(), Some(zx::Status::PEER_CLOSED));
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_process_touch_report_write_error_propagates() {
        let mut fixture = TestFixture::new();
        // EventStatus: coordinates_ready = true (0x80), ContactCount = 1 (0x01)
        let mut report_bytes = vec![0x80, 0x01];
        // contact0: id=0, x=530 (0x0212), y=940 (0x03AC), pressure=40 (0x28)
        let contact0 = [0x00, 0x12, 0x02, 0xAC, 0x03, 0x28, 0x00, 0x00];
        report_bytes.extend_from_slice(&contact0);

        fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(report_bytes));
        fixture.mock.expect_write(EventStatus::ADDRESS, &[0x00], Err(zx::Status::IO_REFUSED));

        let event_time = zx::MonotonicInstant::from_nanos(123_456_789);
        let result = fixture.controller.process_touch_report(event_time).await;
        assert_eq!(result.err(), Some(zx::Status::IO_REFUSED));
        fixture.mock.check_all_expectations_replayed();
    }

    #[test]
    fn test_process_interrupt_idle() {
        run_test_with_interrupts(async {
            let mut fixture = TestFixture::new();
            let idle_report = vec![0x00; 10];
            fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(idle_report));

            let interrupt_trigger =
                fixture.interrupt.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();

            interrupt_trigger.trigger(zx::MonotonicInstant::get()).unwrap();

            let has_event =
                fixture.controller.process_interrupt().await.expect("process_interrupt failed");
            assert!(!has_event);

            // Trigger again to verify interrupt was acknowledged and remains functional.
            let idle_report2 = vec![0x00; 10];
            fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(idle_report2));
            interrupt_trigger.trigger(zx::MonotonicInstant::get()).unwrap();
            let has_event2 = fixture
                .controller
                .process_interrupt()
                .await
                .expect("second process_interrupt failed");
            assert!(!has_event2);

            fixture.mock.check_all_expectations_replayed();
        });
    }

    #[test]
    fn test_process_interrupt_single_touch() {
        run_test_with_interrupts(async {
            let mut fixture = TestFixture::new();

            // EventStatus: coordinates_ready = true (0x80), ContactCount = 1 (0x01)
            let mut report_bytes = vec![0x80, 0x01];
            // contact0: id=0, x=530 (0x0212), y=940 (0x03AC), pressure=40 (0x28)
            let contact0 = [0x00, 0x12, 0x02, 0xAC, 0x03, 0x28, 0x00, 0x00];
            report_bytes.extend_from_slice(&contact0);

            fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(report_bytes));
            fixture.mock.expect_write(EventStatus::ADDRESS, &[0x00], Ok(()));

            let interrupt_trigger =
                fixture.interrupt.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();

            interrupt_trigger.trigger(zx::MonotonicInstant::get()).unwrap();

            let has_event =
                fixture.controller.process_interrupt().await.expect("process_interrupt failed");
            assert!(has_event);

            fixture.mock.check_all_expectations_replayed();
        });
    }

    #[test]
    fn test_process_interrupt_ack_on_i2c_error() {
        run_test_with_interrupts(async {
            let mut fixture = TestFixture::new();

            // The first read returns an `IO_REFUSED` error.
            fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Err(zx::Status::IO_REFUSED));

            // Second read returns a valid touch report:
            // EventStatus: coordinates_ready = true (0x80), ContactCount = 1 (0x01)
            let mut report_bytes = vec![0x80, 0x01];
            // contact0: id=0, x=530 (0x0212), y=940 (0x03AC), pressure=40 (0x28)
            let contact0 = [0x00, 0x12, 0x02, 0xAC, 0x03, 0x28, 0x00, 0x00];
            report_bytes.extend_from_slice(&contact0);
            fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(report_bytes));
            fixture.mock.expect_write(EventStatus::ADDRESS, &[0x00], Ok(()));

            let interrupt_trigger =
                fixture.interrupt.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();

            interrupt_trigger.trigger(zx::MonotonicInstant::get()).unwrap();
            let err = fixture.controller.process_interrupt().await.unwrap_err();
            assert_eq!(err, zx::Status::IO_REFUSED);

            interrupt_trigger.trigger(zx::MonotonicInstant::get()).unwrap();
            let has_event =
                fixture.controller.process_interrupt().await.expect("second event failed");
            assert!(has_event);

            fixture.mock.check_all_expectations_replayed();
        });
    }

    #[test]
    fn test_run_interrupt_stream_cancellation() {
        run_test_with_interrupts(async {
            let fixture = TestFixture::new();

            // Destroying the interrupt handle cancels pending waits and closes the
            // OnInterrupt stream (yielding zx::Status::CANCELED), simulating driver
            // unbind/shutdown.
            fixture.interrupt.destroy().unwrap();

            fixture.controller.run().await;
            fixture.mock.check_all_expectations_replayed();
        });
    }

    #[fuchsia::test]
    async fn test_process_touch_report_dispatches_to_handler() {
        let mut fixture = TestFixture::new();

        let received_report = Arc::new(std::sync::Mutex::new(None));
        let received_report_clone = Arc::clone(&received_report);
        fixture.controller.set_touch_report_handler(Arc::new(move |contacts, event_time| {
            *received_report_clone.lock().unwrap() = Some((contacts.to_vec(), event_time));
        }));

        // EventStatus: coordinates_ready = true (0x80), ContactCount = 2 (0x02)
        let mut initial_report_bytes = vec![0x80, 0x02];
        // contact0: id=0, x=90 (0x005A), y=953 (0x03B9), pressure=174 (0xAE)
        let contact0 = [0x00, 0x5A, 0x00, 0xB9, 0x03, 0xAE, 0x00, 0x00];
        initial_report_bytes.extend_from_slice(&contact0);

        // contact1: id=2, x=498 (0x01F2), y=68 (0x0044), pressure=108 (0x6C)
        let contact1 = [0x02, 0xF2, 0x01, 0x44, 0x00, 0x6C, 0x00, 0x00];

        fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(initial_report_bytes));
        fixture.mock.expect_read(RemainingTouchContacts::<1>::ADDRESS, 8, Ok(contact1.to_vec()));
        fixture.mock.expect_write(EventStatus::ADDRESS, &[0x00], Ok(()));

        const EVENT_TIME_NANOS: i64 = 888_777_666;
        let event_time = zx::MonotonicInstant::from_nanos(EVENT_TIME_NANOS);

        let has_event = fixture
            .controller
            .process_touch_report(event_time)
            .await
            .expect("process_touch_report failed");
        assert!(has_event);
        fixture.mock.check_all_expectations_replayed();

        let (contacts, reported_time) =
            received_report.lock().unwrap().take().expect("missing dispatched touch report");
        assert_eq!(reported_time, event_time);
        assert_eq!(contacts.len(), 2);

        assert_eq!(contacts[0].id(), 0);
        assert_eq!(contacts[0].x(), 90);
        assert_eq!(contacts[0].y(), 953);
        assert_eq!(contacts[0].pressure(), 174);

        assert_eq!(contacts[1].id(), 2);
        assert_eq!(contacts[1].x(), 498);
        assert_eq!(contacts[1].y(), 68);
        assert_eq!(contacts[1].pressure(), 108);
    }

    #[fuchsia::test]
    async fn test_process_touch_report_dispatches_release_to_handler() {
        let mut fixture = TestFixture::new();

        let received_report = Arc::new(std::sync::Mutex::new(None));
        let received_report_clone = Arc::clone(&received_report);
        fixture.controller.set_touch_report_handler(Arc::new(move |contacts, event_time| {
            *received_report_clone.lock().unwrap() = Some((contacts.to_vec(), event_time));
        }));

        // Hardware reports coordinates_ready = true (0x80), but contact_count = 0 (0x00).
        let report_bytes = vec![0x80, 0x00, 0, 0, 0, 0, 0, 0, 0, 0];
        fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(report_bytes));
        fixture.mock.expect_write(EventStatus::ADDRESS, &[0x00], Ok(()));

        const RELEASE_TIME_NANOS: i64 = 999_888_777;
        let release_time = zx::MonotonicInstant::from_nanos(RELEASE_TIME_NANOS);

        let has_event = fixture
            .controller
            .process_touch_report(release_time)
            .await
            .expect("process_touch_report failed");
        assert!(has_event);
        fixture.mock.check_all_expectations_replayed();

        let (contacts, reported_time) =
            received_report.lock().unwrap().take().expect("missing dispatched release report");
        assert_eq!(reported_time, release_time);
        assert!(contacts.is_empty(), "touch release should dispatch empty contacts slice");
    }
}
