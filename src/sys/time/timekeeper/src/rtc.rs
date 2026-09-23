// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::RtcInitializationPolicy;
use anyhow::{Error, Result, anyhow};
use async_trait::async_trait;
use chrono::LocalResult;
use chrono::prelude::*;
use fdio as _;
use fidl_fuchsia_hardware_hrtimer as ffhh;
use fidl_fuchsia_hardware_rtc as frtc;
use fuchsia_async::{self as fasync, TimeoutExt};
use fuchsia_component::client::Service;
use fuchsia_runtime::{UtcDuration, UtcInstant};
use futures::{StreamExt, TryFutureExt, select};
use log::{debug, error, warn};
use std::cell::RefCell;
use std::pin::pin;
use std::rc::Rc;
use thiserror::Error;
use time_persistence::State;
use time_pretty::format_duration;
#[cfg(test)]
use {fuchsia_sync::Mutex, std::sync::Arc};

/// Time to wait before declaring a FIDL call to be failed.
const FIDL_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(2);

// The minimum error at which to begin an async wait for top of second while setting RTC.
const WAIT_THRESHOLD: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(1);

// If a RTC device op lasts more than this, we declare a timeout.
const RTC_DEVICE_OPEN_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(5);

const NANOS_PER_SECOND: i64 = 1_000_000_000;

#[derive(Error, Debug)]
pub enum RtcCreationError {
    #[error("Could not find any RTC devices")]
    NoDevices,
    #[error("Could not connect to RTC device: {0}")]
    ConnectionFailed(Error),
    #[error("Not configured to use RTC")]
    NotConfigured,
}

/// Interface to interact with a real-time clock. Note that the RTC hardware interface is limited
/// to a resolution of one second; times returned by the RTC will always be a whole number of
/// seconds and times sent to the RTC will discard any fractional second.
#[async_trait(?Send)]
pub trait Rtc {
    /// Returns the current time reported by the realtime clock.
    async fn get(&self) -> Result<UtcInstant>;
    /// Sets the time of the realtime clock to `value`.
    async fn set(&self, value: UtcInstant) -> Result<()>;
}

fn get_service() -> Result<Service<frtc::ServiceMarker>> {
    Service::open(frtc::ServiceMarker).map_err(Error::from)
}

/// An implementation of the `Rtc` trait that uses persistent storage to emulate
/// writable RTC nonvolatile memory.
///
/// Requires an operational boot clock, since it relies on boot clock readings to
/// reasonably advance while the device is in low power states. This makes
/// this particular RTC implementation unusable if an operational boot clock
/// is not available.
pub struct ReadOnlyRtcImpl<F, C>
where
    F: Fn(&State) -> Result<()>,
    C: Fn() -> zx::BootInstant,
{
    state: Rc<RefCell<State>>,
    // Used to query for current time from a persistent timer.
    proxy: Option<ffhh::DeviceProxy>,
    // Overridable for tests.
    writer_fn: F,
    clock_fn: C,
    rtc_initialization_policy: RtcInitializationPolicy,
}

/// Create a new `ReadOnlyRtcImpl`, using the provided `state` for UTC reference
/// storage.
pub fn new_read_only_rtc(
    state: Rc<RefCell<State>>,
    proxy: Option<ffhh::DeviceProxy>,
    rtc_initialization_policy: RtcInitializationPolicy,
) -> ReadOnlyRtcImpl<impl Fn(&State) -> Result<()>, impl Fn() -> zx::BootInstant> {
    let func = |s: &State| State::write(s);
    let now_fn = || fasync::BootInstant::now().into();
    new_read_only_rtc_with_dependencies(state, proxy, func, now_fn, rtc_initialization_policy)
}

// A factory method with an option to inject state. Intended to be called
// directly in tests, and its non-test counterpart above.
fn new_read_only_rtc_with_dependencies<F, C>(
    state: Rc<RefCell<State>>,
    proxy: Option<ffhh::DeviceProxy>,
    writer_fn: F,
    clock_fn: C,
    rtc_initialization_policy: RtcInitializationPolicy,
) -> ReadOnlyRtcImpl<F, C>
where
    F: Fn(&State) -> Result<()>,
    C: Fn() -> zx::BootInstant,
{
    ReadOnlyRtcImpl { state, proxy, writer_fn, clock_fn, rtc_initialization_policy }
}

#[async_trait(?Send)]
impl<F, C> Rtc for ReadOnlyRtcImpl<F, C>
where
    F: Fn(&State) -> Result<()>,
    C: Fn() -> zx::BootInstant,
{
    /// Returns a linear approximation of the UtcInstant based on a valid reading
    /// of the current boot clock, and assuming a valid persisted reference boot instant.
    async fn get(&self) -> Result<UtcInstant> {
        let boot_now = self.now().await;
        let (boot_reference, utc_reference) = self.state.borrow().get_rtc_reference();
        let diff = boot_now - boot_reference;
        if diff < zx::BootDuration::ZERO {
            match self.rtc_initialization_policy {
                RtcInitializationPolicy::ApplyMaybeStale => {
                    log::warn!(
                        concat!(
                            "negative RTC diff detected, but config allows applying past UTC. ",
                            "References:\n\tpersisted: {:?}\n\tnow:       {:?}\n\tutc:       {:?}"
                        ),
                        boot_reference,
                        boot_now,
                        utc_reference
                    );
                    Ok(utc_reference)
                }
                RtcInitializationPolicy::Default => {
                    // ReadOnlyRtc relies on the boot clock for RTC updates. This allows us to have
                    // correct time estimates during suspend.  However, on reboot, we typically
                    // restart the boot clock, leading to a negative offset adjustment, which is wrong.
                    // To avoid incorrect UTC adjustments, we disallow negative offsets.
                    log::warn!(
                        concat!(
                            "negative RTC diff detected. References:",
                            "\n\tpersisted: {:?}\n\tnow:       {:?}\n\tutc:       {:?}"
                        ),
                        boot_reference,
                        boot_now,
                        utc_reference
                    );
                    Err(anyhow!(
                        "negative offset adjustment for RTC is not allowed: {}",
                        format_duration(diff)
                    ))
                }
            }
        } else {
            let utc_now = utc_reference + UtcDuration::from_nanos(diff.into_nanos());
            Ok(utc_now)
        }
    }

    /// Sets a reference point based on the current reading of the boot clock,
    /// and a reference UTC instant.
    async fn set(&self, value: UtcInstant) -> Result<()> {
        let boot_now = self.now().await;
        self.state.borrow_mut().set_rtc_reference(boot_now.into(), value);
        (self.writer_fn)(&self.state.borrow())
    }
}
impl<F, C> ReadOnlyRtcImpl<F, C>
where
    F: Fn(&State) -> Result<()>,
    C: Fn() -> zx::BootInstant,
{
    async fn now(&self) -> zx::BootInstant {
        const CLOCK_ID: u64 = 1;
        let local_now = (self.clock_fn)();
        if let Some(ref proxy) = self.proxy {
            // This hard-coded resolution is appropriate for our device.
            let resolution = ffhh::Resolution::Duration(NANOS_PER_SECOND);
            proxy
                .read_clock(CLOCK_ID, &resolution)
                .await
                .inspect_err(|err| {
                    error!("error while reading persistent clock: {err:?}");
                })
                .expect("FIDL call to a driver does not fail")
                .map(|value| {
                    zx::BootInstant::ZERO
                        + zx::BootDuration::from_seconds(
                            value.try_into().expect("ticks is convertible to i64"),
                        )
                })
                .unwrap_or(local_now)
        } else {
            local_now
        }
    }
}

/// An implementation of the `Rtc` trait that connects to an RTC device in /dev/class/rtc.
#[derive(Debug)]
pub struct RtcImpl {
    proxy: frtc::DeviceProxy,
}

impl RtcImpl {
    /// Returns a new `RtcImpl` connected to the only available RTC device. Returns an Error if no
    /// devices were found, multiple devices were found, or the connection failed.
    ///
    /// Args:
    /// - `has_rtc`: set to true if the board is configured with an RTC.
    pub async fn only_device(has_rtc: bool) -> Result<RtcImpl, RtcCreationError> {
        Self::only_device_for_test(has_rtc, get_service).await
    }

    // Call directly only for tests.
    //
    // Args:
    // See `Self::only_device`.
    //
    // Generics:
    // - `F`: a closure that maybe provides a `Service<frtc::ServiceMarker>` to be used to
    //   enumerate service instances.  Normally this is only set to non-default
    //   value in tests.
    async fn only_device_for_test<F>(
        has_rtc: bool,
        rtc_service_source: F,
    ) -> Result<RtcImpl, RtcCreationError>
    where
        F: FnOnce() -> Result<Service<frtc::ServiceMarker>>,
    {
        debug!("has_rtc: {}", has_rtc);
        if has_rtc {
            let service = rtc_service_source().map_err(|err| {
                RtcCreationError::ConnectionFailed(anyhow!("could not open RTC service: {}", err))
            })?;

            let mut rtc_instances = match service.watch().await {
                Ok(instances) => instances,
                Err(_) => return Err(RtcCreationError::NoDevices),
            };
            let mut timeout = pin!(fasync::Timer::new(RTC_DEVICE_OPEN_TIMEOUT));
            select! {
                instance = rtc_instances.next() => {
                    match instance {
                        Some(Ok(instance)) => {
                            let device = instance.connect_to_device().map_err(|err| {
                                RtcCreationError::ConnectionFailed(anyhow!(
                                    "could not connect to RTC device: {}",
                                    err
                                ))
                            })?;
                            fasync::Task::local(async move {
                                while let Some(instance) = rtc_instances.next().await {
                                    // This warning occurs if the system has multiple RTC hardware devices (or drivers
                                    // presenting as such) that register service instances. Because Timekeeper binds
                                    // to the first RTC service instance it detects, the presence of multiple instances
                                    // means the selection of the active RTC is non-deterministic and could vary across boots.
                                    warn!("another RTC device appeared and was ignored: {:?}", instance.map(|_| ()))
                                }
                            })
                            .detach();
                            Ok(RtcImpl { proxy: device })
                        },
                        Some(Err(err)) => {
                             Err(RtcCreationError::ConnectionFailed(anyhow!(
                                "could not read any RTC device: {}",
                                err
                            )))
                        }
                        None => {
                            // While this should not happen in general, we may be better
                            // served by continuing without RTC if it does.
                            Err(RtcCreationError::NoDevices)
                        },
                    }
                },

                _ = timeout => {
                    Err(RtcCreationError::NoDevices)
                },
            }
        } else {
            debug!("no RTC was configured");
            Err(RtcCreationError::NotConfigured)
        }
    }
}

fn fidl_time_to_zx_time(fidl_time: frtc::Time) -> Result<UtcInstant> {
    let chrono = Utc.with_ymd_and_hms(
        fidl_time.year as i32,
        fidl_time.month as u32,
        fidl_time.day as u32,
        fidl_time.hours as u32,
        fidl_time.minutes as u32,
        fidl_time.seconds as u32,
    );
    match chrono {
        LocalResult::Single(t) => Ok(UtcInstant::from_nanos(t.timestamp_nanos_opt().unwrap())),
        _ => Err(anyhow!("Invalid RTC time: {:?}", fidl_time)),
    }
}

fn zx_time_to_fidl_time(zx_time: UtcInstant) -> frtc::Time {
    let nanos = UtcInstant::into_nanos(zx_time);
    let chrono = Utc.timestamp_opt(nanos / NANOS_PER_SECOND, 0).unwrap();
    frtc::Time {
        year: chrono.year() as u16,
        month: chrono.month() as u8,
        day: chrono.day() as u8,
        hours: chrono.hour() as u8,
        minutes: chrono.minute() as u8,
        seconds: chrono.second() as u8,
    }
}

#[async_trait(?Send)]
impl Rtc for RtcImpl {
    async fn get(&self) -> Result<UtcInstant> {
        self.proxy
            .get()
            .map_err(|err| anyhow!("FIDL error on Rtc::get: {}", err))
            .on_timeout(zx::MonotonicInstant::after(FIDL_TIMEOUT), || {
                Err(anyhow!("FIDL timeout on Rtc::get"))
            })
            .await?
            .map_err(|err| anyhow!("Driver error on Rtc::get: {}", err))
            .and_then(fidl_time_to_zx_time)
    }

    async fn set(&self, value: UtcInstant) -> Result<()> {
        let fractional_second =
            zx::MonotonicDuration::from_nanos(value.into_nanos() % NANOS_PER_SECOND);
        // The RTC API only accepts integer seconds but we really need higher accuracy, particularly
        // for the kernel clock set by the RTC driver...
        let fidl_time = if fractional_second < WAIT_THRESHOLD {
            // ...if we are being asked to set a time at or near the bottom of the second, truncate
            // the time and set the RTC immediately...
            zx_time_to_fidl_time(value)
        } else {
            // ...otherwise, wait until the top of the current second than set the RTC using the
            // following second.
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_seconds(1) - fractional_second,
            ))
            .await;
            zx_time_to_fidl_time(value + UtcDuration::from_seconds(1))
        };
        let result = self
            .proxy
            .set2(&fidl_time)
            .map_err(|err| anyhow!("FIDL error on Rtc::set: {}", err))
            .on_timeout(zx::MonotonicInstant::after(FIDL_TIMEOUT), || {
                Err(anyhow!("FIDL timeout on Rtc::set"))
            })
            .await?
            .map_err(zx::Status::err_from_raw);
        result.map_err(|stat| anyhow!("Bad status on Rtc::set: {:?}", stat))
    }
}

/// A Fake implementation of the Rtc trait for use in testing. The fake always returns a fixed
/// value set during construction and remembers the last value it was told to set (shared across
/// all clones of the `FakeRtc`).
#[cfg(test)]
#[derive(Clone)]
pub struct FakeRtc {
    /// The response used for get requests.
    value: Result<UtcInstant, String>,
    /// The most recent value received in a set request.
    last_set: Arc<Mutex<Option<UtcInstant>>>,
}

#[cfg(test)]
impl FakeRtc {
    /// Returns a new `FakeRtc` that always returns the supplied time.
    pub fn valid(time: UtcInstant) -> FakeRtc {
        FakeRtc { value: Ok(time), last_set: Arc::new(Mutex::new(None)) }
    }

    /// Returns a new `FakeRtc` that always returns the supplied error message.
    pub fn invalid(error: String) -> FakeRtc {
        FakeRtc { value: Err(error), last_set: Arc::new(Mutex::new(None)) }
    }

    /// Returns the last time set on this clock, or none if the clock has never been set.
    pub fn last_set(&self) -> Option<UtcInstant> {
        self.last_set.lock().clone()
    }

    /// Resets the last set value to None.
    pub fn reset_last_set(&self) {
        let mut last_set = self.last_set.lock();
        *last_set = None;
    }
}

#[cfg(test)]
#[async_trait(?Send)]
impl Rtc for FakeRtc {
    async fn get(&self) -> Result<UtcInstant> {
        self.value.as_ref().map(|time| time.clone()).map_err(|msg| Error::msg(msg.clone()))
    }

    async fn set(&self, value: UtcInstant) -> Result<()> {
        let mut last_set = self.last_set.lock();
        last_set.replace(value);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use test_util::{assert_gt, assert_lt};

    const TEST_FIDL_TIME: frtc::Time =
        frtc::Time { year: 2020, month: 8, day: 14, hours: 0, minutes: 0, seconds: 0 };
    const INVALID_FIDL_TIME_1: frtc::Time =
        frtc::Time { year: 2020, month: 14, day: 0, hours: 0, minutes: 0, seconds: 0 };
    const INVALID_FIDL_TIME_2: frtc::Time =
        frtc::Time { year: 2020, month: 8, day: 14, hours: 99, minutes: 99, seconds: 99 };
    const TEST_OFFSET: UtcDuration = UtcDuration::from_millis(250);
    const TEST_ZX_TIME: UtcInstant = UtcInstant::from_nanos(1_597_363_200_000_000_000);
    const DIFFERENT_ZX_TIME: UtcInstant = UtcInstant::from_nanos(1_597_999_999_000_000_000);

    fn new_rw_rtc(proxy: frtc::DeviceProxy) -> RtcImpl {
        RtcImpl { proxy }
    }

    #[fuchsia::test]
    fn time_conversion() {
        let to_fidl = zx_time_to_fidl_time(TEST_ZX_TIME);
        assert_eq!(to_fidl, TEST_FIDL_TIME);
        // Times should be truncated to the previous second
        let to_fidl_2 = zx_time_to_fidl_time(TEST_ZX_TIME + UtcDuration::from_millis(999));
        assert_eq!(to_fidl_2, TEST_FIDL_TIME);

        let to_zx = fidl_time_to_zx_time(TEST_FIDL_TIME).unwrap();
        assert_eq!(to_zx, TEST_ZX_TIME);

        assert_eq!(fidl_time_to_zx_time(INVALID_FIDL_TIME_1).is_err(), true);
        assert_eq!(fidl_time_to_zx_time(INVALID_FIDL_TIME_2).is_err(), true);
    }

    #[fuchsia::test]
    async fn rtc_impl_get_valid() {
        let (proxy, mut stream) = create_proxy_and_stream::<frtc::DeviceMarker>();

        let rtc_impl = new_rw_rtc(proxy);
        let _responder = fasync::Task::spawn(async move {
            if let Some(Ok(frtc::DeviceRequest::Get { responder })) = stream.next().await {
                responder.send(Ok(&TEST_FIDL_TIME)).expect("Failed response");
            }
        });
        assert_eq!(rtc_impl.get().await.unwrap(), TEST_ZX_TIME);
    }

    #[fuchsia::test]
    async fn rtc_impl_get_invalid() {
        let (proxy, mut stream) = create_proxy_and_stream::<frtc::DeviceMarker>();

        let rtc_impl = new_rw_rtc(proxy);
        let _responder = fasync::Task::spawn(async move {
            if let Some(Ok(frtc::DeviceRequest::Get { responder })) = stream.next().await {
                responder.send(Ok(&INVALID_FIDL_TIME_1)).expect("Failed response");
            }
        });
        assert_eq!(rtc_impl.get().await.is_err(), true);
    }

    const RTC_SETUP_TIME: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(90);

    #[fuchsia::test]
    async fn rtc_impl_set_whole_second() {
        let (proxy, mut stream) = create_proxy_and_stream::<frtc::DeviceMarker>();

        let rtc_impl = new_rw_rtc(proxy);
        let _responder = fasync::Task::spawn(async move {
            if let Some(Ok(frtc::DeviceRequest::Set2 { rtc, responder })) = stream.next().await {
                let response =
                    if rtc == TEST_FIDL_TIME { Ok(()) } else { Err(zx::Status::INVALID_ARGS) };
                responder.send(response.map_err(zx::Status::into_raw)).expect("Failed response");
            }
        });
        let before = zx::MonotonicInstant::get();
        assert!(rtc_impl.set(TEST_ZX_TIME).await.is_ok());
        let span = zx::MonotonicInstant::get() - before;
        // Setting an integer second should not require any delay and therefore should complete
        // very fast - well under a millisecond typically. We did observe ~54ms very rarely.
        assert_lt!(span, RTC_SETUP_TIME);
    }

    #[fuchsia::test]
    async fn rtc_impl_set_partial_second() {
        let (proxy, mut stream) = create_proxy_and_stream::<frtc::DeviceMarker>();

        let rtc_impl = new_rw_rtc(proxy);
        let _responder = fasync::Task::spawn(async move {
            if let Some(Ok(frtc::DeviceRequest::Set2 { rtc, responder })) = stream.next().await {
                let response =
                    if rtc == TEST_FIDL_TIME { Ok(()) } else { Err(zx::Status::INVALID_ARGS) };
                responder.send(response.map_err(zx::Status::into_raw)).expect("Failed response");
            }
        });
        let before = zx::MonotonicInstant::get();
        assert!(rtc_impl.set(TEST_ZX_TIME - TEST_OFFSET).await.is_ok());
        let span = zx::MonotonicInstant::get() - before;
        // Setting a fractional second should cause a delay until the top of second before calling
        // the FIDL interface. We only verify half the expected time has passed to allow for some
        // slack in the timer calculation.
        assert_gt!(span, zx::MonotonicDuration::from_nanos(TEST_OFFSET.into_nanos() / 2));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn valid_fake() {
        let fake = FakeRtc::valid(TEST_ZX_TIME);
        assert_eq!(fake.get().await.unwrap(), TEST_ZX_TIME);
        assert_eq!(fake.last_set(), None);

        // Set a new time, this should be recorded but get should still return the original time.
        assert!(fake.set(DIFFERENT_ZX_TIME).await.is_ok());
        assert_eq!(fake.last_set(), Some(DIFFERENT_ZX_TIME));
        assert_eq!(fake.get().await.unwrap(), TEST_ZX_TIME);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn invalid_fake() {
        let message = "I'm designed to fail".to_string();
        let fake = FakeRtc::invalid(message.clone());
        assert_eq!(&fake.get().await.unwrap_err().to_string(), &message);
        assert_eq!(fake.last_set(), None);

        // Setting a new time should still succeed and be recorded but it won't make get valid.
        assert!(fake.set(DIFFERENT_ZX_TIME).await.is_ok());
        assert_eq!(fake.last_set(), Some(DIFFERENT_ZX_TIME));
        assert_eq!(&fake.get().await.unwrap_err().to_string(), &message);
    }

    use assert_matches::assert_matches;
    use vfs::pseudo_directory;

    #[fuchsia::test]
    async fn no_rtc_configured() {
        let dir = pseudo_directory! {};
        let dir_proxy =
            vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new());
        let service_source = || Service::open_from_dir(dir_proxy, frtc::ServiceMarker);
        let result = RtcImpl::only_device_for_test(/*has_rtc*/ false, service_source).await;
        assert_matches!(result, Err(RtcCreationError::NotConfigured))
    }

    #[fuchsia::test]
    async fn no_rtc_detected() {
        let dir = pseudo_directory! {};
        let dir_proxy =
            vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new());
        let service_source = || Service::open_from_dir(dir_proxy, frtc::ServiceMarker);
        let result = RtcImpl::only_device_for_test(/*has_rtc*/ true, service_source).await;
        assert_matches!(result, Err(RtcCreationError::NoDevices))
    }

    #[fuchsia::test]
    async fn rtc_configured_and_detected() {
        let dir = pseudo_directory! {
            "fuchsia.hardware.rtc.Service" => pseudo_directory! {
                "default" => pseudo_directory! {
                    "device" => vfs::service::host(move |mut _stream: frtc::DeviceRequestStream| {
                        async move {
                        }
                    }),
                },
            },
        };
        let dir_proxy =
            vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new());
        let service_source = || Service::open_from_dir(dir_proxy, frtc::ServiceMarker);
        let result = RtcImpl::only_device_for_test(/*has_rtc*/ true, service_source).await;
        assert!(result.is_ok());
    }

    #[fuchsia::test]
    async fn rtc_read_only() {
        let d = tempfile::TempDir::new().expect("tempdir created");
        let p = d.path().join("file.json");
        let state = Rc::new(RefCell::new(State::new(false)));

        let fake_boot_now = Rc::new(RefCell::new(zx::BootInstant::from_nanos(100)));

        {
            // Try initializing and moving the reference point as fake time passes.
            let p_clone = p.clone();
            let rtc = new_read_only_rtc_with_dependencies(
                state,
                /* proxy= */ None,
                // Fake persistent state is stored in a tempfile.
                |s| State::write_internal(&p_clone, s),
                // Fake "now".
                || fake_boot_now.borrow().clone(),
                RtcInitializationPolicy::Default,
            );
            let utc_reference = UtcInstant::from_nanos(42000);
            rtc.set(utc_reference).await.unwrap();

            // Advance the boot clock a bit. Verify that the UTC moved too.
            (*fake_boot_now.borrow_mut()) += zx::BootDuration::from_nanos(100);
            let utc = rtc.get().await.unwrap();
            assert_eq!(utc, UtcInstant::from_nanos(42100));

            // Again, please.
            (*fake_boot_now.borrow_mut()) += zx::BootDuration::from_nanos(100);
            let utc = rtc.get().await.unwrap();
            assert_eq!(utc, UtcInstant::from_nanos(42200));

            // Time travel is allowed in fake-land. Rewind time a bit, check it.
            // However, see (*) below.
            (*fake_boot_now.borrow_mut()) -= zx::BootDuration::from_nanos(100);
            let utc = rtc.get().await.unwrap();
            assert_eq!(utc, UtcInstant::from_nanos(42100));
        }

        {
            // Now try reading the contents of the persisted file, and verify that
            // we're reading what we persisted. But forward time a bit too.
            let p_clone = p.clone();
            let state = Rc::new(RefCell::new(State::read_and_update_internal(p_clone).unwrap()));
            let rtc = new_read_only_rtc_with_dependencies(
                state,
                /* proxy= */ None,
                |s| State::write_internal(&p, s),
                || fake_boot_now.borrow().clone(),
                RtcInitializationPolicy::Default,
            );

            // Some time passed since we last wrote the above file.
            (*fake_boot_now.borrow_mut()) += zx::BootDuration::from_nanos(300);
            let utc = rtc.get().await.unwrap();
            assert_eq!(utc, UtcInstant::from_nanos(42400));

            // Time travel is allowed in fake-land.
            // (*) But don't overdo it! If we rewind beyond the reference, then the RTC read should
            // be rejected.
            (*fake_boot_now.borrow_mut()) -= zx::BootDuration::from_nanos(500);
            assert_matches!(rtc.get().await, Err(_));
        }

        {
            // Now try with ApplyMaybeStale.
            let p_clone = p.clone();
            let state = Rc::new(RefCell::new(State::read_and_update_internal(p_clone).unwrap()));
            let rtc = new_read_only_rtc_with_dependencies(
                state,
                /* proxy= */ None,
                |s| State::write_internal(&p, s),
                || fake_boot_now.borrow().clone(),
                RtcInitializationPolicy::ApplyMaybeStale,
            );

            // Time travel is allowed in fake-land.
            // If we rewind beyond the reference, but we have ApplyMaybeStale, then
            // we should get the last known UTC reference.
            (*fake_boot_now.borrow_mut()) -= zx::BootDuration::from_nanos(500);
            let utc = rtc.get().await.unwrap();
            // The last set UTC reference was 42000.
            assert_eq!(utc, UtcInstant::from_nanos(42000));
        }
    }

    #[fuchsia::test]
    async fn read_time_from_proxy() {
        let d = tempfile::TempDir::new().expect("tempdir created");
        let p = d.path().join("file.json");
        let state = Rc::new(RefCell::new(State::new(false)));

        let fake_boot_now = Rc::new(RefCell::new(zx::BootInstant::from_nanos(100)));

        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<ffhh::DeviceMarker>();

        let _task = fasync::Task::local(async move {
            while let Some(request) = stream.next().await {
                match request {
                    Ok(ffhh::DeviceRequest::ReadClock { responder, .. }) => {
                        responder.send(Ok(42)).unwrap();
                    }
                    _ => unreachable!(),
                }
            }
        });

        // Try initializing and moving the reference point as fake time passes.
        let rtc = new_read_only_rtc_with_dependencies(
            state,
            Some(proxy),
            |s| State::write_internal(&p, s),
            // Fake "now".
            || fake_boot_now.borrow().clone(),
            RtcInitializationPolicy::Default,
        );

        assert_eq!(
            rtc.now().await,
            zx::BootInstant::ZERO + zx::BootDuration::from_nanos(42 * NANOS_PER_SECOND)
        );
    }
}
