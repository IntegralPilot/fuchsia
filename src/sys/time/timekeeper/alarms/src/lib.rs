// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Alarm management subsystem.
//!
//! This subsystem serves the FIDL API `fuchsia.time.alarms/Wake`. To instantiate,
//! you can use the following approach:
//!
//! ```ignore
//! let proxy = client::connect_to_protocol::<ffhh::DeviceMarker>().map_err(
//!    |e| error!("error: {}", e)).expect("add proper error handling");
//!    let timer_loop = alarms::Handle::new(proxy);
//! ```
//!
//! From here, use the standard approach with [ServiceFs::new] to expose the
//! discoverable FIDL endpoint and call:
//!
//! ```ignore
//! let stream: fidl_fuchsia_time_alarms::WakeRequestStream = ... ;
//! alarms::serve(timer_loop, stream).await;
//! // ...
//! ```
//!
//! Of course, for everything to work well, your component will need appropriate
//! capability routing.  Refer to capability routing docs for those details.

mod emu;
mod timers;

use crate::emu::EmulationTimerOps;
use anyhow::{Context, Result};
use async_trait::async_trait;
use fidl::encoding::ProxyChannelBox;
use fidl::endpoints::RequestStream;
use fidl_fuchsia_driver_token as fdt;
use fidl_fuchsia_hardware_hrtimer as ffhh;
use fidl_fuchsia_time_alarms as fta;
use fuchsia_async as fasync;
use fuchsia_component::client::Service;
use fuchsia_inspect as finspect;
use fuchsia_inspect::{IntProperty, NumericProperty, Property};
use fuchsia_runtime as fxr;
use fuchsia_trace as trace;
use futures::StreamExt;
use futures::channel::mpsc;
use futures::sink::SinkExt;
use log::{debug, error, warn};
use scopeguard::defer;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::LazyLock;
use time_pretty::{MSEC_IN_NANOS, format_duration, format_timer};
use zx::AsHandleRef;

static DEBUG_STACK_TRACE_TOKEN: std::sync::OnceLock<zx::Event> = std::sync::OnceLock::new();
static I64_MAX_AS_U64: LazyLock<u64> = LazyLock::new(|| i64::MAX.try_into().expect("infallible"));
static I32_MAX_AS_U64: LazyLock<u64> = LazyLock::new(|| i32::MAX.try_into().expect("infallible"));

/// The largest value of timer "ticks" that is still considered useful.
static MAX_USEFUL_TICKS: LazyLock<u64> = LazyLock::new(|| *I32_MAX_AS_U64);

/// The smallest value of "ticks" that we can program into the driver. To wit,
/// driver will reject "0" ticks, even though it probably shouldn't. See
/// for details: b/437177931.
static MIN_USEFUL_TICKS: u64 = 1;

/// The hrtimer ID used for scheduling wake alarms.  This ID is reused from
/// Starnix, and should eventually no longer be critical.
const MAIN_TIMER_ID: usize = 6;

/// This is what we consider a "long" delay in alarm operations.
const LONG_DELAY_NANOS: i64 = 2000 * MSEC_IN_NANOS;

const TIMEOUT_SECONDS: i64 = 40;

async fn request_stack_trace() {
    if let Some(ev) = DEBUG_STACK_TRACE_TOKEN.get() {
        log::warn!("*** DRIVER STACK TRACE REQUESTED: expect a driver stack trace below.");
        let ev_dup = ev.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let debug_proxy = fuchsia_component::client::connect_to_protocol::<fdt::DebugMarker>();
        match debug_proxy {
            Ok(proxy) => {
                if let Err(e) = proxy.log_stack_trace(ev_dup).await {
                    log::warn!("failed to log stack trace: {:?}", e);
                }
            }
            Err(e) => {
                log::warn!("failed to connect to Debug protocol: {:?}", e);
            }
        }
    } else {
        log::warn!("DEBUG_STACK_TRACE_TOKEN not initialized, cannot log stack trace");
    }
}

/// Waits on a future, but if the future takes longer than `TIMEOUT_SECONDS`, we log a warning and
/// a stack trace. A stack trace is requested at most once once for each call.
macro_rules! log_long_op {
    ($fut:expr) => {{
        use futures::FutureExt;
        let fut = $fut;
        futures::pin_mut!(fut);
        let mut logged = false;
        loop {
            let timeout = fasync::Timer::new(zx::MonotonicDuration::from_seconds(TIMEOUT_SECONDS));
            futures::select! {
                res = fut.as_mut().fuse() => {
                    if logged {
                        log::warn!("unexpected blocking is now resolved: long-running async operation at {}:{}.",
                            file!(), line!());
                    }
                    break res;
                }
                _ = timeout.fuse() => {
                    // Check logs for a `kBadState` status reported from the hrtimer driver.
                    log::warn!("unexpected blocking: long-running async op at {}:{}. Report to `componentId:1408151`",
                        file!(), line!());
                    if !logged {
                        #[cfg(all(target_os = "fuchsia", not(doc)))]
                        ::debug::backtrace_request_all_threads();
                        fasync::Task::local(request_stack_trace()).detach();
                    }
                    logged = true;
                }
            }
        }
    }};
}

/// Increments the value of an underlying inspect property during its lifetime.
struct ScopedInc<'a> {
    property: &'a IntProperty,
}

impl<'a> ScopedInc<'a> {
    fn new(property: &'a IntProperty) -> Self {
        property.add(1);
        Self { property }
    }
}

impl<'a> Drop for ScopedInc<'a> {
    fn drop(&mut self) {
        self.property.add(-1);
    }
}

/// Compares two optional deadlines and returns true if the `before is different from `after.
/// Nones compare as equal.
fn is_deadline_changed(
    before: Option<fasync::BootInstant>,
    after: Option<fasync::BootInstant>,
) -> bool {
    match (before, after) {
        (None, None) => false,
        (None, Some(_)) | (Some(_), None) => true,
        (Some(before), Some(after)) => before != after,
    }
}

// Errors returnable from [TimerOps] calls.
#[derive(Debug, Clone)]
pub(crate) enum TimerOpsError {
    /// The driver reported an error.
    Driver(ffhh::DriverError),
    /// FIDL-specific RPC error.
    Fidl(fidl::Error),
}

impl Into<fta::WakeAlarmsError> for TimerOpsError {
    /// Compute the error that gets propagated to callers, depending on messages
    /// from the driver.
    fn into(self) -> fta::WakeAlarmsError {
        match self {
            TimerOpsError::Fidl(fidl::Error::ClientChannelClosed { .. }) => {
                fta::WakeAlarmsError::DriverConnection
            }
            TimerOpsError::Driver(ffhh::DriverError::InternalError) => fta::WakeAlarmsError::Driver,
            _ => fta::WakeAlarmsError::Internal,
        }
    }
}

impl TimerOpsError {
    fn is_canceled(&self) -> bool {
        match self {
            TimerOpsError::Driver(ffhh::DriverError::Canceled) => true,
            _ => false,
        }
    }
}

trait SawResponseFut: std::future::Future<Output = Result<zx::EventPair, TimerOpsError>> {
    // nop
}

/// Abstracts away timer operations.
#[async_trait(?Send)]
pub(crate) trait TimerOps {
    /// Stop the timer with the specified ID.
    async fn stop(&self, id: u64);

    /// Examine the timer's properties, such as supported resolutions and tick
    /// counts.
    async fn get_timer_properties(&self) -> TimerConfig;

    /// This method must return an actual future, to handle the borrow checker:
    /// making this async will assume that `self` remains borrowed, which will
    /// thwart attempts to move the return value of this call into a separate
    /// closure.
    fn start_and_wait(
        &self,
        id: u64,
        resolution: &ffhh::Resolution,
        ticks: u64,
        setup_event: zx::Event,
    ) -> std::pin::Pin<Box<dyn SawResponseFut>>;
}

/// TimerOps backed by an actual hardware timer.
struct HardwareTimerOps {
    proxy: ffhh::DeviceProxy,
}

impl HardwareTimerOps {
    fn new(proxy: ffhh::DeviceProxy) -> Box<Self> {
        Box::new(Self { proxy })
    }
}

#[async_trait(?Send)]
impl TimerOps for HardwareTimerOps {
    async fn stop(&self, id: u64) {
        let _ = self
            .proxy
            .stop(id)
            .await
            .map(|result| {
                let _ = result.map_err(|e| warn!("stop_hrtimer: driver error: {:?}", e));
            })
            .map_err(|e| warn!("stop_hrtimer: could not stop prior timer: {}", e));
    }

    async fn get_timer_properties(&self) -> TimerConfig {
        match log_long_op!(self.proxy.get_properties()) {
            Ok(p) => {
                if let Some(token) = p.driver_node_token {
                    let _ = DEBUG_STACK_TRACE_TOKEN.set(token);
                }
                let timers_properties = &p.timers_properties.expect("timers_properties must exist");
                debug!("get_timer_properties: got: {:?}", timers_properties);

                // Pick the correct hrtimer to use for wakes.
                let timer_index = if timers_properties.len() > MAIN_TIMER_ID {
                    // Mostly vim3, where we have pre-existing timer allocations
                    // that we don't need to change.
                    MAIN_TIMER_ID
                } else if timers_properties.len() > 0 {
                    // Newer devices that don't need to allocate timer IDs, and/or
                    // may not even have as many timers as vim3 does. But, at least
                    // one timer is needed.
                    0
                } else {
                    // Give up.
                    return TimerConfig::new_empty();
                };
                let main_timer_properties = &timers_properties[timer_index];
                debug!("alarms: main_timer_properties: {:?}", main_timer_properties);
                // Not sure whether it is useful to have more ticks than this, so limit it.
                let max_ticks: u64 = std::cmp::min(
                    main_timer_properties.max_ticks.unwrap_or(*MAX_USEFUL_TICKS),
                    *MAX_USEFUL_TICKS,
                );
                let resolutions = &main_timer_properties
                    .supported_resolutions
                    .as_ref()
                    .expect("supported_resolutions is populated")
                    .iter()
                    .last() //  Limits the resolution to the coarsest available.
                    .map(|r| match *r {
                        ffhh::Resolution::Duration(d) => d,
                        _ => {
                            error!(
                            "get_timer_properties: Unknown resolution type, returning millisecond."
                        );
                            MSEC_IN_NANOS
                        }
                    })
                    .map(|d| zx::BootDuration::from_nanos(d))
                    .into_iter() // Used with .last() above.
                    .collect::<Vec<_>>();
                let timer_id = main_timer_properties.id.expect("timer ID is always present");
                TimerConfig::new_from_data(timer_id, resolutions, max_ticks)
            }
            Err(e) => {
                error!("could not get timer properties: {:?}", e);
                TimerConfig::new_empty()
            }
        }
    }

    fn start_and_wait(
        &self,
        id: u64,
        resolution: &ffhh::Resolution,
        ticks: u64,
        setup_event: zx::Event,
    ) -> std::pin::Pin<Box<dyn SawResponseFut>> {
        let inner = self.proxy.start_and_wait(id, resolution, ticks, setup_event);
        Box::pin(HwResponseFut { pinner: Box::pin(inner) })
    }
}

// Untangles the borrow checker issues that otherwise result from making
// TimerOps::start_and_wait an async function.
struct HwResponseFut {
    pinner: std::pin::Pin<
        Box<
            fidl::client::QueryResponseFut<
                ffhh::DeviceStartAndWaitResult,
                fidl::encoding::DefaultFuchsiaResourceDialect,
            >,
        >,
    >,
}

use std::task::Poll;
impl SawResponseFut for HwResponseFut {}
impl std::future::Future for HwResponseFut {
    type Output = Result<zx::EventPair, TimerOpsError>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let inner_poll = self.pinner.as_mut().poll(cx);
        match inner_poll {
            Poll::Ready(result) => Poll::Ready(match result {
                Ok(Ok(keep_alive)) => Ok(keep_alive),
                Ok(Err(e)) => Err(TimerOpsError::Driver(e)),
                Err(e) => Err(TimerOpsError::Fidl(e)),
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Stops a currently running hardware timer.
async fn stop_hrtimer(hrtimer: &Box<dyn TimerOps>, timer_config: &TimerConfig) {
    trace::duration!("alarms", "hrtimer:stop", "id" => timer_config.id);
    debug!("stop_hrtimer: stopping hardware timer: {}", timer_config.id);
    log_long_op!(hrtimer.stop(timer_config.id));
    debug!("stop_hrtimer: stopped  hardware timer: {}", timer_config.id);
}

// The default size of the channels created in this module.
// This is very unlikely to create bottlenecks.
const CHANNEL_SIZE: usize = 1000;

/// A type handed around between the concurrent loops run by this module.
#[derive(Debug)]
enum Cmd {
    /// Request a timer to be started.
    Start {
        /// The unique connection ID.
        conn_id: zx::Koid,
        /// A timestamp (presumably in the future), at which to expire the timer.
        deadline: timers::Deadline,
        // The API supports several modes. See fuchsia.time.alarms/Wake.fidl.
        //
        // Optional, because not always needed:
        //
        // * `mode` is required for hanging get API calls (e.g. `StartAndWait`), as we must signal
        //   when the alarm is scheduled.
        // * The calls such as `SetUtc` which return only upon scheduling do not need a `mode`, as
        //   the caller can wait for the call to return immediately.
        mode: Option<fta::SetMode>,
        /// An alarm identifier, chosen by the caller.
        alarm_id: String,
        /// A responder that will be called when the timer expires. The
        /// client end of the connection will block until we send something
        /// on this responder.
        ///
        /// This is packaged into a Rc... only because both the "happy path"
        /// and the error path must consume the responder.  This allows them
        /// to be consumed, without the responder needing to implement Default.
        responder: Rc<dyn timers::Responder>,
    },
    StopById {
        done: zx::Event,
        timer_id: timers::Id,
    },
    Alarm {
        expired_deadline: fasync::BootInstant,
        keep_alive: fidl::EventPair,
    },
    AlarmFidlError {
        expired_deadline: fasync::BootInstant,
        error: fidl::Error,
    },
    AlarmDriverError {
        expired_deadline: fasync::BootInstant,
        error: ffhh::DriverError,

        // Added these for debugging details, otherwise not necessary.
        timer_config_id: u64,
        resolution_nanos: i64,
        ticks: u64,
    },
    /// The UTC clock transformation has been updated.
    UtcUpdated {
        // The new boot-to-utc clock transformation.
        transform: fxr::UtcClockTransform,
    },
}

impl std::fmt::Display for Cmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cmd::Start { conn_id, deadline, alarm_id, .. } => {
                write!(
                    f,
                    "Start[alarm_id=\"{}\", conn_id={:?}, deadline={}]",
                    alarm_id, conn_id, deadline,
                )
            }
            Cmd::Alarm { expired_deadline, .. } => {
                write!(f, "Alarm[deadline={}]", format_timer((*expired_deadline).into()))
            }
            Cmd::AlarmFidlError { expired_deadline, error } => {
                write!(
                    f,
                    "FIDLError[deadline={}, err={}, NO_WAKE_LEASE!]",
                    format_timer((*expired_deadline).into()),
                    error
                )
            }
            Cmd::AlarmDriverError { expired_deadline, error, .. } => {
                write!(
                    f,
                    "DriverError[deadline={}, err={:?}, NO_WAKE_LEASE!]",
                    format_timer((*expired_deadline).into()),
                    error
                )
            }
            Cmd::StopById { timer_id, done: _ } => {
                write!(f, "StopById[timerId={}]", timer_id,)
            }
            Cmd::UtcUpdated { transform } => {
                write!(f, "UtcUpdated[timerId={transform:?}]")
            }
        }
    }
}

/// Extracts a KOID from the underlying channel of the provided stream.
///
/// This function deconstructs the provided stream to access the underlying
/// channel and extract its KOID. It then reconstructs the stream and returns
/// it to the caller along with the KOID.
///
/// # Args
/// - `stream`: The `fta::WakeAlarmsRequestStream` to extract the KOID from.
///
/// # Returns
/// A tuple containing the `zx::Koid` of the stream's channel and the
/// reconstructed `fta::WakeAlarmsRequestStream`.
pub fn get_stream_koid(
    stream: fta::WakeAlarmsRequestStream,
) -> (zx::Koid, fta::WakeAlarmsRequestStream) {
    let (inner, is_terminated) = stream.into_inner();
    let koid = inner.channel().as_channel().as_handle_ref().koid().expect("infallible");
    let stream = fta::WakeAlarmsRequestStream::from_inner(inner, is_terminated);
    (koid, stream)
}

/// Serves a single Wake API client.
///
/// This function processes incoming requests from a `fta::WakeAlarmsRequestStream`,
/// handling each request by calling `handle_request`. It continues to process
/// requests until the stream is exhausted.
///
/// # Args
/// - `timer_loop`: A reference-counted pointer to the `Loop` that manages timers.
/// - `requests`: The stream of incoming `fta::WakeAlarmsRequest` from a client.
pub async fn serve(timer_loop: Rc<Loop>, requests: fta::WakeAlarmsRequestStream) {
    let timer_loop = timer_loop.clone();
    let timer_loop_send = || timer_loop.get_sender();
    let (conn_id, mut requests) = get_stream_koid(requests);
    let mut request_count = 0;
    debug!("alarms::serve: opened connection: {:?}", conn_id);
    while let Some(maybe_request) = requests.next().await {
        request_count += 1;
        debug!("alarms::serve: conn_id: {:?} incoming request: {}", conn_id, request_count);
        match maybe_request {
            Ok(request) => {
                // Should return quickly.
                handle_request(conn_id, timer_loop_send(), request).await;
            }
            Err(e) => {
                warn!("alarms::serve: error in request: {:?}", e);
            }
        }
        debug!("alarms::serve: conn_id: {:?} done request: {}", conn_id, request_count);
    }
    // Check if connection closure was intentional. It is way too easy to close
    // a FIDL connection inadvertently if doing non-mainstream things with FIDL.
    warn!("alarms::serve: CLOSED CONNECTION: conn_id: {:?}", conn_id);
}

async fn handle_cancel(alarm_id: String, conn_id: zx::Koid, cmd: &mut mpsc::Sender<Cmd>) {
    let done = zx::Event::create();
    let timer_id = timers::Id::new(alarm_id.clone(), conn_id);
    if let Err(e) = log_long_op!(cmd.send(Cmd::StopById {
        timer_id,
        done: done.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("infallible"),
    })) {
        warn!("handle_request: error while trying to cancel: {}: {:?}", alarm_id, e);
    }
    log_long_op!(wait_signaled(&done));
}

/// Processes a single Wake API request from a single client.
/// This function is expected to return quickly.
///
/// # Args
/// - `conn_id`: the unique identifier of the connection producing these requests.
/// - `cmd`: the outbound queue of commands to deliver to the timer manager.
/// - `request`: a single inbound Wake FIDL API request.
async fn handle_request(
    conn_id: zx::Koid,
    mut cmd: mpsc::Sender<Cmd>,
    request: fta::WakeAlarmsRequest,
) {
    match request {
        fta::WakeAlarmsRequest::SetAndWait { deadline, mode, alarm_id, responder } => {
            // Since responder is consumed by the happy path and the error path, but not both,
            // and because the responder does not implement Default, this is a way to
            // send it in two mutually exclusive directions.  Each direction will reverse
            // this wrapping once the responder makes it to the other side.
            //
            // Rc required because of sharing a noncopyable struct; RefCell required because
            // borrow_mut() is needed to move out; and Option is required so we can
            // use take() to replace the struct with None so it does not need to leave
            // a Default in its place.
            let responder = Rc::new(RefCell::new(Some(responder)));

            // Alarm is not scheduled yet!
            debug!(
                "handle_request: scheduling alarm_id: \"{}\"\n\tconn_id: {:?}\n\tdeadline: {}",
                alarm_id,
                conn_id,
                format_timer(deadline.into())
            );
            // Expected to return quickly.
            let deadline = timers::Deadline::Boot(deadline.into());
            if let Err(e) = log_long_op!(cmd.send(Cmd::Start {
                conn_id,
                deadline,
                mode: Some(mode),
                alarm_id: alarm_id.clone(),
                responder: responder.clone(),
            })) {
                warn!("handle_request: error while trying to schedule `{}`: {:?}", alarm_id, e);
                responder
                    .borrow_mut()
                    .take()
                    .expect("always present if call fails")
                    .send(Err(fta::WakeAlarmsError::Internal))
                    .unwrap();
            }
        }
        fta::WakeAlarmsRequest::SetAndWaitUtc { deadline, mode, alarm_id, responder } => {
            // Quickly get rid of the custom wake alarms deadline type.
            let deadline =
                timers::Deadline::Utc(fxr::UtcInstant::from_nanos(deadline.timestamp_utc));

            // The rest of this match branch is the same as for `SetAndWait`. However, the handling
            // is for now simple enough that we don't need to explore factoring common actions out.
            let responder = Rc::new(RefCell::new(Some(responder)));
            debug!(
                "handle_request: scheduling alarm_id UTC: \"{alarm_id}\"\n\tconn_id: {conn_id:?}\n\tdeadline: {deadline}",
            );

            if let Err(e) = log_long_op!(cmd.send(Cmd::Start {
                conn_id,
                deadline,
                mode: Some(mode),
                alarm_id: alarm_id.clone(),
                responder: responder.clone(),
            })) {
                warn!("handle_request: error while trying to schedule `{}`: {:?}", alarm_id, e);
                responder
                    .borrow_mut()
                    .take()
                    .expect("always present if call fails")
                    .send(Err(fta::WakeAlarmsError::Internal))
                    .unwrap();
            }
        }
        fta::WakeAlarmsRequest::Cancel { alarm_id, .. } => {
            // TODO: b/383062441 - make this into an async task so that we wait
            // less to schedule the next alarm.
            handle_cancel(alarm_id, conn_id, &mut cmd).await;
        }
        fta::WakeAlarmsRequest::Set { notifier, deadline, mode, alarm_id, responder } => {
            // Alarm is not scheduled yet!
            debug!(
                "handle_request: scheduling alarm_id: \"{alarm_id}\"\n\tconn_id: {conn_id:?}\n\tdeadline: {}",
                format_timer(deadline.into())
            );
            // Expected to return quickly.
            if let Err(e) = log_long_op!(cmd.send(Cmd::Start {
                conn_id,
                deadline: timers::Deadline::Boot(deadline.into()),
                mode: Some(mode),
                alarm_id: alarm_id.clone(),
                responder: Rc::new(RefCell::new(Some(notifier))),
            })) {
                warn!("handle_request: error while trying to schedule `{}`: {:?}", alarm_id, e);
                responder.send(Err(fta::WakeAlarmsError::Internal)).unwrap();
            } else {
                // Successfully scheduled the alarm.
                responder.send(Ok(())).unwrap();
            }
        }
        fta::WakeAlarmsRequest::_UnknownMethod { .. } => {}
    };
}

/// Represents a single alarm event processing loop.
///
/// One instance is created per each alarm-capable low-level device. The `Loop`
/// is responsible for managing the lifecycle of wake alarms, including their
/// creation, scheduling, and cancellation. It interacts with the underlying
/// hardware timer through a `TimerOps` trait object.
pub struct Loop {
    // Given to any clients that need to send messages to `_task`
    // via [get_sender].
    snd: mpsc::Sender<Cmd>,
}

impl Loop {
    /// Creates a new instance of `Loop`.
    ///
    /// This function initializes a new `Loop` with a connection to a low-level
    /// hardware timer device. It spawns two background tasks: one for the main
    /// timer event loop and another for monitoring UTC clock changes.
    ///
    /// # Args
    /// - `scope`: The `fasync::ScopeHandle` to spawn background tasks in.
    /// - `device_proxy`: A `ffhh::DeviceProxy` for communicating with the hardware timer.
    /// - `inspect`: A `finspect::Node` for recording diagnostics.
    /// - `utc_clock`: A `fxr::UtcClock` for tracking UTC time.
    ///
    /// # Returns
    /// A new instance of `Loop`.
    pub fn new(
        scope: fasync::ScopeHandle,
        device_proxy: ffhh::DeviceProxy,
        inspect: finspect::Node,
        utc_clock: fxr::UtcClock,
    ) -> Self {
        let hw_device_timer_ops = HardwareTimerOps::new(device_proxy);
        Loop::new_internal(scope, hw_device_timer_ops, inspect, utc_clock)
    }

    /// Creates a new instance of `Loop` with emulated wake alarms.
    ///
    /// This function is similar to `new`, but it uses an emulated timer instead
    /// of a real hardware timer. This is useful for testing environments where
    /// a hardware timer may not be available.
    ///
    /// # Args
    /// - `scope`: The `fasync::ScopeHandle` to spawn background tasks in.
    /// - `inspect`: A `finspect::Node` for recording diagnostics.
    /// - `utc_clock`: A `fxr::UtcClock` for tracking UTC time.
    ///
    /// # Returns
    /// A new instance of `Loop` with an emulated timer.
    pub fn new_emulated(
        scope: fasync::ScopeHandle,
        inspect: finspect::Node,
        utc_clock: fxr::UtcClock,
    ) -> Self {
        let timer_ops = Box::new(EmulationTimerOps::new());
        Loop::new_internal(scope, timer_ops, inspect, utc_clock)
    }

    fn new_internal(
        scope: fasync::ScopeHandle,
        timer_ops: Box<dyn TimerOps>,
        inspect: finspect::Node,
        utc_clock: fxr::UtcClock,
    ) -> Self {
        let utc_transform = Rc::new(RefCell::new(
            utc_clock.get_details().expect("has UTC clock READ capability").reference_to_synthetic,
        ));

        let (snd, rcv) = mpsc::channel(CHANNEL_SIZE);
        let loop_scope = scope.clone();

        scope.spawn_local(wake_timer_loop(
            loop_scope,
            snd.clone(),
            rcv,
            timer_ops,
            inspect,
            utc_transform,
        ));
        scope.spawn_local(monitor_utc_clock_changes(utc_clock, snd.clone()));
        Self { snd }
    }

    /// Gets a copy of a channel through which async commands may be sent to
    /// the [Loop].
    fn get_sender(&self) -> mpsc::Sender<Cmd> {
        self.snd.clone()
    }
}

// Forwards the clock transformation of an updated clock into the alarm manager, to allow
// correcting the boot time deadlines of clocks on the UTC timeline.
async fn monitor_utc_clock_changes(utc_clock: fxr::UtcClock, mut cmd: mpsc::Sender<Cmd>) {
    let koid = utc_clock.as_handle_ref().koid();
    log::info!("monitor_utc_clock_changes: entry");
    loop {
        // CLOCK_UPDATED signal is self-clearing.
        fasync::OnSignals::new(utc_clock.as_handle_ref(), zx::Signals::CLOCK_UPDATED)
            .await
            .expect("UTC clock is readable");

        let transform =
            utc_clock.get_details().expect("UTC clock details are readable").reference_to_synthetic;
        log::debug!("Received a UTC update: koid={koid:?}: {transform:?}");
        if let Err(err) = cmd.send(Cmd::UtcUpdated { transform }).await {
            // This is OK in tests.
            log::warn!("monitor_utc_clock_changes: exit: {err:?}");
            break;
        }
    }
}

/// Clones a handle infallibly with `zx::Rights::SAME_RIGHTS`.
///
/// This function duplicates a handle, preserving its rights. It will panic if
/// the handle duplication fails, which is not expected to happen under normal
/// circumstances.
///
/// # Args
/// - `handle`: A reference to a handle-based object to be cloned.
///
/// # Returns
/// A new handle with the same rights as the original.

async fn wait_signaled<H: fidl::AsHandleRef>(handle: &H) {
    fasync::OnSignals::new(&handle.as_handle_ref(), zx::Signals::EVENT_SIGNALED)
        .await
        .expect("infallible");
}

pub(crate) fn signal(event: &zx::Event) {
    event.signal(zx::Signals::NONE, zx::Signals::EVENT_SIGNALED).expect("infallible");
}

/// A [TimerDuration] represents a duration of time that can be expressed by
/// a discrete timer register.
///
/// This is a low-level representation of time duration, used in interaction with
/// hardware devices. It is therefore necessarily discretized, with adaptive
/// resolution, depending on the physical characteristics of the underlying
/// hardware timer that it models.
#[derive(Debug, Clone, Copy)]
struct TimerDuration {
    // The resolution of each one of the `ticks` below.
    resolution: zx::BootDuration,
    // The number of ticks that encodes time duration. Each "tick" represents
    // one unit of `resolution` above.
    ticks: u64,
}

/// This and the comparison traits below are used to allow TimerDuration
/// calculations in a compact form.
impl Eq for TimerDuration {}

impl std::cmp::PartialOrd for TimerDuration {
    fn partial_cmp(&self, other: &TimerDuration) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::PartialEq for TimerDuration {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl std::cmp::Ord for TimerDuration {
    /// Two [TimerDuration]s compare equal if they model exactly the same duration of time,
    /// no matter the resolutions.
    fn cmp(&self, other: &TimerDuration) -> std::cmp::Ordering {
        let self_ticks_128: i128 = self.ticks as i128;
        let self_resolution: i128 = self.resolution_as_nanos() as i128;
        let self_nanos = self_resolution * self_ticks_128;

        let other_ticks_128: i128 = other.ticks as i128;
        let other_resolution: i128 = other.resolution_as_nanos() as i128;
        let other_nanos = other_resolution * other_ticks_128;

        self_nanos.cmp(&other_nanos)
    }
}

impl std::fmt::Display for TimerDuration {
    /// Human readable TimerDuration exposes both the tick count and the resolution,
    /// in the format of "ticks x resolution", with an end result of
    /// `10x5ms` for example.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ticks = self.ticks;
        let resolution = self.resolution();
        // Example: 10x1ms
        write!(f, "{}x{}", ticks, format_duration(resolution),)
    }
}

impl TimerDuration {
    /// The maximum representable TimerDuration that we allow.
    fn max() -> Self {
        TimerDuration::new(zx::BootDuration::from_nanos(1), *I64_MAX_AS_U64)
    }

    /// The zero [TimerDuration].
    fn zero() -> Self {
        TimerDuration::new(zx::BootDuration::from_nanos(1), 0)
    }

    /// Creates a new timer duration with the given parameters.
    fn new(resolution: zx::BootDuration, ticks: u64) -> Self {
        Self { resolution, ticks }
    }

    /// Creates a new timer duration using the resolution from `res_source` and
    /// a specified number of ticks.
    fn new_with_resolution(res_source: &TimerDuration, ticks: u64) -> Self {
        Self::new(res_source.resolution, ticks)
    }

    /// Returns the time duration represented by this TimerDuration.
    ///
    /// Due to the way duration is expressed, the same time duration
    /// can be represented in multiple ways.
    fn duration(&self) -> zx::BootDuration {
        let duration_as_nanos = self.resolution_as_nanos() * self.ticks;
        let clamp_duration = std::cmp::min(*I32_MAX_AS_U64, duration_as_nanos);
        zx::BootDuration::from_nanos(clamp_duration.try_into().expect("result was clamped"))
    }

    /// The resolution of this TimerDuration
    fn resolution(&self) -> zx::BootDuration {
        self.resolution
    }

    fn resolution_as_nanos(&self) -> u64 {
        self.resolution().into_nanos().try_into().expect("resolution is never negative")
    }

    /// The number of ticks of this [TimerDuration].
    fn ticks(&self) -> u64 {
        self.ticks
    }
}

impl From<zx::BootDuration> for TimerDuration {
    fn from(d: zx::BootDuration) -> TimerDuration {
        let nanos = d.into_nanos();
        assert!(nanos >= 0);
        let nanos_u64 = nanos.try_into().expect("guarded by assert");
        TimerDuration::new(zx::BootDuration::from_nanos(1), nanos_u64)
    }
}

impl std::ops::Div for TimerDuration {
    type Output = u64;
    fn div(self, rhs: Self) -> Self::Output {
        let self_nanos = self.resolution_as_nanos() * self.ticks;
        let rhs_nanos = rhs.resolution_as_nanos() * rhs.ticks;
        self_nanos / rhs_nanos
    }
}

impl std::ops::Mul<u64> for TimerDuration {
    type Output = Self;
    fn mul(self, rhs: u64) -> Self::Output {
        Self::new(self.resolution, self.ticks * rhs)
    }
}

/// Contains the configuration of a specific timer.
#[derive(Debug)]
pub(crate) struct TimerConfig {
    /// The resolutions supported by this timer. Each entry is one possible
    /// duration for on timer "tick".  The resolution is picked when a timer
    /// request is sent.
    ///
    /// The resolutions MUST be sorted from finest (index 0) to coarsest.
    ///
    /// There MUST be at least one resolution.
    resolutions: Vec<zx::BootDuration>,
    /// The maximum count of "ticks" that the timer supports. The timer usually
    /// has a register that counts up or down based on a clock signal with
    /// the period specified by `resolutions`.  This is the maximum value that
    /// the counter can count to without overflowing.
    max_ticks: u64,
    /// The stable ID of the timer with the above configuration.
    id: u64,
}

impl TimerConfig {
    /// Creates a new timer config with supported timer resolutions and the max
    /// ticks value for the timer's counter.
    fn new_from_data(timer_id: u64, resolutions: &[zx::BootDuration], max_ticks: u64) -> Self {
        debug!(
            "TimerConfig: resolutions: {:?}, max_ticks: {}, timer_id: {}",
            resolutions.iter().map(|r| format_duration(*r)).collect::<Vec<_>>(),
            max_ticks,
            timer_id
        );
        let resolutions = resolutions.iter().map(|d| *d).collect::<Vec<zx::BootDuration>>();
        TimerConfig { resolutions, max_ticks, id: timer_id }
    }

    fn new_empty() -> Self {
        error!("TimerConfig::new_empty() called, this is not OK.");
        TimerConfig { resolutions: vec![], max_ticks: 0, id: 0 }
    }

    // Picks the most appropriate timer setting for it to fire as close as possible
    // when `duration` expires.
    //
    // If duration is too far in the future for what the timer supports,
    // return a smaller value, to allow the timer to be reprogrammed multiple
    // times.
    //
    // If the available menu of resolutions is such that we can wake only after
    // the intended deadline, begrudgingly return that option.
    fn pick_setting(&self, duration: zx::BootDuration) -> TimerDuration {
        assert!(self.resolutions.len() > 0, "there must be at least one supported resolution");

        // Driver does not support zero ticks, so we must accept the finest resolution duration
        // instead.
        if duration <= zx::BootDuration::ZERO {
            return TimerDuration::new(self.resolutions[0], 1);
        }

        //  0         |-------------->|<---------------|
        //  |---------+---------------+----------------+---->
        //  |---------^               |                |
        //  | best positive slack     |                |
        //  |-------------------------^ duration       |
        //  |------------------------------------------^ best negative slack.
        let mut best_positive_slack = TimerDuration::zero();
        let mut best_negative_slack = TimerDuration::max();

        if self.max_ticks == 0 {
            return TimerDuration::new(zx::BootDuration::from_millis(1), 0);
        }
        let duration_slack: TimerDuration = duration.into();

        for res1 in self.resolutions.iter() {
            let smallest_unit = TimerDuration::new(*res1, 1);
            let max_tick_at_res = TimerDuration::new(*res1, self.max_ticks);

            let smallest_slack_larger_than_duration = smallest_unit > duration_slack;
            let largest_slack_smaller_than_duration = max_tick_at_res < duration_slack;

            if smallest_slack_larger_than_duration {
                if duration_slack == TimerDuration::zero() {
                    best_negative_slack = TimerDuration::zero();
                } else if smallest_unit < best_negative_slack {
                    best_negative_slack = smallest_unit;
                }
            }
            if largest_slack_smaller_than_duration {
                if max_tick_at_res > best_positive_slack
                    || best_positive_slack == TimerDuration::zero()
                {
                    best_positive_slack = max_tick_at_res;
                }
            }

            // "Regular" case.
            if !smallest_slack_larger_than_duration && !largest_slack_smaller_than_duration {
                // Check whether duration divides evenly into the available slack options
                // for this resolution.  If it does, then that is the slack we're looking for.
                let q = duration_slack / smallest_unit;
                let d = smallest_unit * q;
                if d == duration_slack {
                    // Exact match, we can return right now.
                    return d;
                } else {
                    // Not an exact match, so q ticks is before, but q+1 is after.
                    if d > best_positive_slack {
                        best_positive_slack = TimerDuration::new_with_resolution(&smallest_unit, q);
                    }
                    let d_plus = TimerDuration::new_with_resolution(&smallest_unit, q + 1);
                    if d_plus < best_negative_slack {
                        best_negative_slack = d_plus;
                    }
                }
            }
        }

        let p_slack = duration - best_positive_slack.duration();
        let n_slack = best_negative_slack.duration() - duration;

        // If the closest approximation is 0ns, then we can not advance time, so we reject it.
        // Otherwise pick the smallest slack.  Note that when we pick the best positive slack,
        // we will wake *before* the actual deadline.  In multi-resolution counters, this enables
        // us to pick a finer count in the next go.
        let ret = if p_slack < n_slack && best_positive_slack.duration().into_nanos() > 0 {
            best_positive_slack
        } else {
            best_negative_slack
        };
        debug!("TimerConfig: picked slack: {} for duration: {}", ret, format_duration(duration));
        assert!(
            ret.duration().into_nanos() >= 0,
            "ret: {}, p_slack: {}, n_slack: {}, orig.duration: {}\n\tbest_p_slack: {}\n\tbest_n_slack: {}\n\ttarget: {}\n\t 1: {} 2: {:?}, 3: {:?}",
            ret,
            format_duration(p_slack),
            format_duration(n_slack),
            format_duration(duration),
            best_positive_slack,
            best_negative_slack,
            duration_slack,
            p_slack != zx::BootDuration::ZERO,
            p_slack,
            zx::BootDuration::ZERO,
        );
        ret
    }
}

async fn get_timer_properties(hrtimer: &Box<dyn TimerOps>) -> TimerConfig {
    debug!("get_timer_properties: requesting timer properties.");
    hrtimer.get_timer_properties().await
}

/// The state of a single hardware timer that we must bookkeep.
struct TimerState {
    // The task waiting for the proximate timer to expire.
    task: fasync::Task<()>,
    // The deadline that the above task is waiting for.
    deadline: fasync::BootInstant,
}

/// The command loop for timer interaction.  All changes to the wake alarm device programming
/// come in form of commands through `cmd`.
///
/// Args:
/// - `snd`: the send end of `cmd` below, a clone is given to each spawned sub-task.
/// - `cmds``: the input queue of alarm related commands.
/// - `timer_proxy`: the FIDL API proxy for interacting with the hardware device.
/// - `inspect`: the inspect node to record loop info into.
async fn wake_timer_loop(
    scope: fasync::ScopeHandle,
    snd: mpsc::Sender<Cmd>,
    mut cmds: mpsc::Receiver<Cmd>,
    timer_proxy: Box<dyn TimerOps>,
    inspect: finspect::Node,
    utc_transform: Rc<RefCell<fxr::UtcClockTransform>>,
) {
    debug!("wake_timer_loop: started");

    let mut timers = timers::Heap::new(utc_transform.clone());
    let timer_config = get_timer_properties(&timer_proxy).await;

    // Keeps the currently executing HrTimer closure.  This is not read from, but
    // keeps the timer task active.
    #[allow(clippy::collection_is_never_read)]
    let mut hrtimer_status: Option<TimerState> = None;

    // Initialize inspect properties. This must be done only once.
    //
    // Take note that these properties are updated when the `cmds` loop runs.
    // This means that repeated reads while no `cmds` activity occurs will return
    // old readings.  This is to ensure a consistent ability to replay the last
    // loop run if needed.
    let now_prop = inspect.create_int("now_ns", 0);
    let now_formatted_prop = inspect.create_string("now_formatted", "");
    let pending_timers_count_prop = inspect.create_uint("pending_timers_count", 0);
    let pending_timers_prop = inspect.create_string("pending_timers", "");
    let _deadline_histogram_prop = inspect.create_int_exponential_histogram(
        "requested_deadlines_ns",
        finspect::ExponentialHistogramParams {
            floor: 0,
            initial_step: zx::BootDuration::from_micros(1).into_nanos(),
            // Allows capturing deadlines up to dozens of days.
            step_multiplier: 10,
            buckets: 16,
        },
    );
    let slack_histogram_prop = inspect.create_int_exponential_histogram(
        "slack_ns",
        finspect::ExponentialHistogramParams {
            floor: 0,
            initial_step: zx::BootDuration::from_micros(1).into_nanos(),
            step_multiplier: 10,
            buckets: 16,
        },
    );
    let schedule_delay_prop = inspect.create_int_exponential_histogram(
        "schedule_delay_ns",
        finspect::ExponentialHistogramParams {
            floor: 0,
            initial_step: zx::BootDuration::from_micros(1).into_nanos(),
            step_multiplier: 10,
            buckets: 16,
        },
    );
    let boot_deadlines_count_prop = inspect.create_uint("boot_deadlines_count", 0);
    let utc_deadlines_count_prop = inspect.create_uint("utc_deadlines_count", 0);
    // Internals of what was programmed into the wake alarms hardware.
    let hw_node = inspect.create_child("hardware");
    let current_hw_deadline_prop = hw_node.create_string("current_deadline", "");
    let remaining_until_alarm_prop = hw_node.create_string("remaining_until_alarm", "");

    // Debug nodes for b/454085350.
    let debug_node = inspect.create_child("debug_node");
    let start_notify_setup_count = debug_node.create_int("start_notify_setup", 0);
    let start_count = debug_node.create_int("start_count", 0);
    let responder_count = debug_node.create_int("responder_count", 0);
    let stop_count = debug_node.create_int("stop", 0);
    let stop_responder_count = debug_node.create_int("stop_responder", 0);
    let stop_hrtimer_count = debug_node.create_int("stop_hrtimer", 0);
    let schedule_hrtimer_count = debug_node.create_int("schedule_hrtimer", 0);
    let alarm_count = debug_node.create_int("alarm", 0);
    let alarm_fidl_count = debug_node.create_int("alarm_fidl", 0);
    let alarm_driver_count = debug_node.create_int("alarm_driver", 0);
    let utc_update_count = debug_node.create_int("utc_update", 0);
    let status_count = debug_node.create_int("status", 0);
    let loop_count = debug_node.create_int("loop_count", 0);

    let hrtimer_node = debug_node.create_child("hrtimer");

    const LRU_CACHE_CAPACITY: usize = 100;
    let mut error_cache = lru_cache::LruCache::new(LRU_CACHE_CAPACITY);

    while let Some(cmd) = cmds.next().await {
        let _i = ScopedInc::new(&loop_count);
        trace::duration!("alarms", "Cmd");
        // Use a consistent notion of "now" across commands.
        let now = fasync::BootInstant::now();
        now_prop.set(now.into_nanos());
        trace::instant!("alarms", "wake_timer_loop", trace::Scope::Process, "now" => now.into_nanos());
        match cmd {
            Cmd::Start { conn_id, deadline, mode, alarm_id, responder } => {
                let _i = ScopedInc::new(&start_count);
                trace::duration!("alarms", "Cmd::Start");
                fuchsia_trace::flow_step!(
                    "alarms",
                    "hrtimer_lifecycle",
                    timers::get_trace_id(&alarm_id)
                );
                // NOTE: hold keep_alive until all work is done.
                debug!(
                    "wake_timer_loop: START alarm_id: \"{}\", conn_id: {:?}\n\tdeadline: {}\n\tnow:      {}",
                    alarm_id,
                    conn_id,
                    deadline,
                    format_timer(now.into()),
                );

                defer! {
                    let _i = ScopedInc::new(&start_notify_setup_count);
                    // This is the only option that requires further action.
                    if let Some(mode) = mode {
                        if let fta::SetMode::NotifySetupDone(setup_done) = mode {
                            // Must signal once the setup is completed.
                            signal(&setup_done);
                            debug!("wake_timer_loop: START: setup_done signaled");
                        };
                    }
                }
                let deadline_boot = deadline.as_boot(&*utc_transform.borrow());

                // TODO: b/444236931: re-enable.
                //// Bookkeeping, record the incidence of deadline types.
                //deadline_histogram_prop.insert((deadline_boot - now).into_nanos());
                match deadline {
                    timers::Deadline::Boot(_) => boot_deadlines_count_prop.add(1),
                    timers::Deadline::Utc(_) => utc_deadlines_count_prop.add(1),
                };

                if timers::Heap::expired(now, deadline_boot) {
                    trace::duration!("alarms", "Cmd::Start:immediate");
                    fuchsia_trace::flow_step!(
                        "alarms",
                        "hrtimer_lifecycle",
                        timers::get_trace_id(&alarm_id)
                    );
                    // A timer set into now or the past expires right away.
                    let (_lease, keep_alive) = zx::EventPair::create();
                    debug!(
                        "[{}] wake_timer_loop: bogus lease {:?}",
                        line!(),
                        keep_alive.koid().unwrap()
                    );

                    {
                        let _i1 = ScopedInc::new(&responder_count);
                        if let Err(e) = responder
                            .send(&alarm_id, Ok(keep_alive))
                            .expect("responder is always present")
                        {
                            error!(
                                "wake_timer_loop: conn_id: {conn_id:?}, alarm: {alarm_id}: could not notify, dropping: {e}",
                            );
                        } else {
                            debug!(
                                "wake_timer_loop: conn_id: {conn_id:?}, alarm: {alarm_id}: EXPIRED IMMEDIATELY\n\tdeadline({}) <= now({})\n\tfull deadline: {}",
                                format_timer(deadline_boot.into()),
                                format_timer(now.into()),
                                deadline,
                            )
                        }
                    }
                } else {
                    trace::duration!("alarms", "Cmd::Start:regular");
                    fuchsia_trace::flow_step!(
                        "alarms",
                        "hrtimer_lifecycle",
                        timers::get_trace_id(&alarm_id)
                    );
                    // A timer scheduled for the future gets inserted into the timer heap.
                    let was_empty = timers.is_empty();

                    let deadline_before = timers.peek_deadline_as_boot();
                    let node = match deadline {
                        timers::Deadline::Boot(_) => {
                            timers.new_node_boot(deadline_boot, alarm_id, conn_id, responder)
                        }
                        timers::Deadline::Utc(d) => {
                            timers.new_node_utc(d, alarm_id, conn_id, responder)
                        }
                    };
                    timers.push(node);
                    let deadline_after = timers.peek_deadline_as_boot();

                    let deadline_changed = is_deadline_changed(deadline_before, deadline_after);
                    let needs_cancel = !was_empty && deadline_changed;
                    let needs_reschedule = was_empty || deadline_changed;

                    if needs_reschedule {
                        // Always schedule the proximate deadline.
                        let schedulable_deadline = deadline_after.unwrap_or(deadline_boot);
                        if needs_cancel {
                            log_long_op!(stop_hrtimer(&timer_proxy, &timer_config));
                        }
                        hrtimer_status = Some(
                            schedule_hrtimer(
                                scope.clone(),
                                now,
                                &timer_proxy,
                                schedulable_deadline,
                                snd.clone(),
                                &timer_config,
                                &schedule_delay_prop,
                                &hrtimer_node,
                            )
                            .await,
                        );
                    }
                }
            }
            Cmd::StopById { timer_id, done } => {
                let _i = ScopedInc::new(&stop_count);
                defer! {
                    signal(&done);
                }
                trace::duration!("alarms", "Cmd::StopById", "alarm_id" => timer_id.alarm());
                fuchsia_trace::flow_step!(
                    "alarms",
                    "hrtimer_lifecycle",
                    timers::get_trace_id(&timer_id.alarm())
                );
                debug!("wake_timer_loop: STOP timer: {}", timer_id);
                let deadline_before = timers.peek_deadline_as_boot();

                if let Some(timer_node) = timers.remove_by_id(&timer_id) {
                    let deadline_after = timers.peek_deadline_as_boot();

                    {
                        let _i = ScopedInc::new(&stop_responder_count);
                        if let Some(res) = timer_node
                            .get_responder()
                            .send(timer_node.id().alarm(), Err(fta::WakeAlarmsError::Dropped))
                        {
                            // We must reply to the responder to keep the connection open.
                            res.expect("infallible");
                        }
                    }
                    if is_deadline_changed(deadline_before, deadline_after) {
                        let _i = ScopedInc::new(&stop_hrtimer_count);
                        log_long_op!(stop_hrtimer(&timer_proxy, &timer_config));
                    }
                    if let Some(deadline) = deadline_after {
                        let _i = ScopedInc::new(&schedule_hrtimer_count);
                        // Reschedule the hardware timer if the removed timer is the earliest one,
                        // and another one exists.
                        let new_timer_state = schedule_hrtimer(
                            scope.clone(),
                            now,
                            &timer_proxy,
                            deadline,
                            snd.clone(),
                            &timer_config,
                            &schedule_delay_prop,
                            &hrtimer_node,
                        )
                        .await;
                        let old_hrtimer_status = hrtimer_status.replace(new_timer_state);
                        if let Some(task) = old_hrtimer_status.map(|ev| ev.task) {
                            // Allow the task to complete. Since this task should have been
                            // canceled or completed already, this call should not block for
                            // a long time.
                            log_long_op!(task);
                        }
                    } else {
                        // No next timer, clean up the hrtimer status.
                        hrtimer_status = None;
                    }
                } else {
                    // Imminent: the soonest to trigger, based on its timeline.
                    debug!("wake_timer_loop: STOP: removed non-imminent timer: {}", timer_id);
                }
            }
            Cmd::Alarm { expired_deadline, keep_alive } => {
                let _i = ScopedInc::new(&alarm_count);

                trace::duration!("alarms", "Cmd::Alarm");
                // Expire all eligible timers, based on "now".  This is because
                // we may have woken up earlier than the actual deadline. This
                // happens for example if the timer can not make the actual
                // deadline and needs to be re-programmed.
                debug!(
                    "wake_timer_loop: ALARM!!! reached deadline: {}, wakey-wakey! {:?}",
                    format_timer(expired_deadline.into()),
                    keep_alive.koid().unwrap(),
                );
                let expired_count =
                    notify_all(&mut timers, &keep_alive, now, None, &slack_histogram_prop)
                        .expect("notification succeeds");
                if expired_count == 0 {
                    // This could be a resolution switch, or a straggler notification.
                    // Either way, the hardware timer is still ticking, cancel it.
                    debug!("wake_timer_loop: no expired alarms, reset hrtimer state");
                    log_long_op!(stop_hrtimer(&timer_proxy, &timer_config));
                }
                // There is a timer to reschedule, do that now.
                hrtimer_status = match timers.peek_deadline_as_boot() {
                    None => None,
                    Some(deadline) => Some(
                        schedule_hrtimer(
                            scope.clone(),
                            now,
                            &timer_proxy,
                            deadline,
                            snd.clone(),
                            &timer_config,
                            &schedule_delay_prop,
                            &hrtimer_node,
                        )
                        .await,
                    ),
                }
            }
            Cmd::AlarmFidlError { expired_deadline, error } => {
                let _i = ScopedInc::new(&alarm_fidl_count);

                trace::duration!("alarms", "Cmd::AlarmFidlError");
                // We do not have a wake lease, so the system may sleep before
                // we get to schedule a new timer. We have no way to avoid it
                // today.
                let error_string = format!("{}", error);
                if !error_cache.contains_key(&error_string) {
                    warn!(
                        "wake_timer_loop: FIDL error: {}, deadline: {}, now: {}",
                        error,
                        format_timer(expired_deadline.into()),
                        format_timer(now.into()),
                    );
                    error_cache.insert(error_string, ());
                }
                // Manufacture a fake lease to make the code below work.
                // Maybe use Option instead?
                let (_dummy_lease, peer) = zx::EventPair::create();
                debug!(
                    "bogus lease: {:?} fidl error [{}:{}]",
                    peer.koid().unwrap(),
                    file!(),
                    line!()
                );
                notify_all(
                    &mut timers,
                    &peer,
                    now,
                    Some(TimerOpsError::Fidl(error)),
                    &slack_histogram_prop,
                )
                .expect("notification succeeds");
                hrtimer_status = match timers.peek_deadline_as_boot() {
                    None => None, // No remaining timers, nothing to schedule.
                    Some(deadline) => Some(
                        schedule_hrtimer(
                            scope.clone(),
                            now,
                            &timer_proxy,
                            deadline,
                            snd.clone(),
                            &timer_config,
                            &schedule_delay_prop,
                            &hrtimer_node,
                        )
                        .await,
                    ),
                }
            }
            Cmd::AlarmDriverError {
                expired_deadline,
                error,
                timer_config_id,
                resolution_nanos,
                ticks,
            } => {
                let _i = ScopedInc::new(&alarm_driver_count);

                trace::duration!("alarms", "Cmd::AlarmDriverError");
                let (_dummy_lease, peer) = zx::EventPair::create();
                debug!(
                    "bogus lease: {:?} driver error. [{}:{}]",
                    peer.koid().unwrap(),
                    file!(),
                    line!()
                );
                notify_all(
                    &mut timers,
                    &peer,
                    now,
                    Some(TimerOpsError::Driver(error)),
                    &slack_histogram_prop,
                )
                .expect("notification succeeds");
                match error {
                    fidl_fuchsia_hardware_hrtimer::DriverError::Canceled => {
                        // Nothing to do here, cancelation is handled in Stop code.
                        debug!(
                            "wake_timer_loop: CANCELED timer at deadline: {}",
                            format_timer(expired_deadline.into())
                        );
                    }
                    _ => {
                        error!(
                            "wake_timer_loop: DRIVER SAYS: {:?}, deadline: {}, now: {}\n\ttimer_id={}\n\tresolution={}\n\tticks={}",
                            error,
                            format_timer(expired_deadline.into()),
                            format_timer(now.into()),
                            timer_config_id,
                            resolution_nanos,
                            ticks,
                        );
                        // We do not have a wake lease, so the system may sleep before
                        // we get to schedule a new timer. We have no way to avoid it
                        // today.
                        hrtimer_status = match timers.peek_deadline_as_boot() {
                            None => None,
                            Some(deadline) => Some(
                                schedule_hrtimer(
                                    scope.clone(),
                                    now,
                                    &timer_proxy,
                                    deadline,
                                    snd.clone(),
                                    &timer_config,
                                    &schedule_delay_prop,
                                    &hrtimer_node,
                                )
                                .await,
                            ),
                        }
                    }
                }
            }
            Cmd::UtcUpdated { transform } => {
                let _i = ScopedInc::new(&utc_update_count);

                trace::duration!("alarms", "Cmd::UtcUpdated");
                debug!("wake_timer_loop: applying new clock transform: {transform:?}");

                // Assigning to this shared reference updates the deadlines of all
                // UTC timers.
                *utc_transform.borrow_mut() = transform;

                // Reschedule the hardware timer with the now-current deadline if there is an
                // active timer.
                if hrtimer_status.is_some() {
                    log_long_op!(stop_hrtimer(&timer_proxy, &timer_config));
                    // Should we request a wake lock here?
                    hrtimer_status = match timers.peek_deadline_as_boot() {
                        None => None,
                        Some(deadline) => Some(
                            schedule_hrtimer(
                                scope.clone(),
                                now,
                                &timer_proxy,
                                deadline,
                                snd.clone(),
                                &timer_config,
                                &schedule_delay_prop,
                                &hrtimer_node,
                            )
                            .await,
                        ),
                    }
                }
            }
        }

        {
            let _i = ScopedInc::new(&status_count);

            // Print and record diagnostics after each iteration, record the
            // duration for performance awareness.  Note that iterations happen
            // only occasionally, so these stats can remain unchanged for a long
            // time.
            trace::duration!("timekeeper", "inspect");
            let now_formatted = format_timer(now.into());
            debug!("wake_timer_loop: now:                             {}", now_formatted);
            now_formatted_prop.set(&now_formatted);

            let pending_timers_count: u64 =
                timers.timer_count().try_into().expect("always convertible");
            debug!("wake_timer_loop: currently pending timer count:   {}", pending_timers_count);
            pending_timers_count_prop.set(pending_timers_count);

            let pending_timers = format!("{}", timers);
            debug!("wake_timer_loop: currently pending timers:        \n\t{}", timers);
            pending_timers_prop.set(&pending_timers);

            let current_deadline: String = hrtimer_status
                .as_ref()
                .map(|s| format!("{}", format_timer(s.deadline.into())))
                .unwrap_or_else(|| "(none)".into());
            debug!("wake_timer_loop: current hardware timer deadline: {:?}", current_deadline);
            current_hw_deadline_prop.set(&current_deadline);

            let remaining_duration_until_alarm = hrtimer_status
                .as_ref()
                .map(|s| format!("{}", format_duration((s.deadline - now).into())))
                .unwrap_or_else(|| "(none)".into());
            debug!(
                "wake_timer_loop: remaining duration until alarm:  {}",
                remaining_duration_until_alarm
            );
            remaining_until_alarm_prop.set(&remaining_duration_until_alarm);
            debug!("---");
        }
    }

    // Prod code should not see this loop ever exiting. the wake alarm manager
    // should run forever.
    log::info!("wake_timer_loop: exiting. This is only correct in test code.");
}

/// Schedules a wake alarm.
///
/// # Args:
///
/// - `scope`: used to spawn async tasks.
/// - `now`: the time instant used as the value of current instant.
/// - `hrtimer`: the proxy for the hrtimer device driver.
/// - `deadline`: the time instant in the future at which the alarm should fire.
/// - `command_send`: the sender channel to use when the timer expires.
/// - `timer_config`: a configuration of the hardware timer showing supported resolutions and
///   max tick value.
/// - `schedule_delay_histogram`: inspect instrumentation.
/// - `debug_node`: used for keeping debug counters.
async fn schedule_hrtimer(
    scope: fasync::ScopeHandle,
    now: fasync::BootInstant,
    hrtimer: &Box<dyn TimerOps>,
    deadline: fasync::BootInstant,
    mut command_send: mpsc::Sender<Cmd>,
    timer_config: &TimerConfig,
    _schedule_delay_histogram: &finspect::IntExponentialHistogramProperty,
    debug_node: &finspect::Node,
) -> TimerState {
    let timeout = std::cmp::max(zx::BootDuration::ZERO, deadline - now);
    trace::duration!("alarms", "schedule_hrtimer", "timeout" => timeout.into_nanos());
    // When signaled, the hrtimer has been scheduled.
    let hrtimer_scheduled = zx::Event::create();

    let schedule_count = debug_node.create_int("schedule", 0);
    let hrtimer_wait_count = debug_node.create_int("hrtimer_wait", 0);
    let wait_signaled_count = debug_node.create_int("wait_signaled", 0);

    let _sc = ScopedInc::new(&schedule_count);

    debug!(
        "schedule_hrtimer:\n\tnow: {}\n\tdeadline: {}\n\ttimeout: {}",
        format_timer(now.into()),
        format_timer(deadline.into()),
        format_duration(timeout),
    );

    let slack = timer_config.pick_setting(timeout);
    let resolution_nanos = slack.resolution.into_nanos();
    // The driver will reject "0" ticks, even though it probably shouldn't. See for details:
    // b/437177931.
    let useful_ticks = std::cmp::max(MIN_USEFUL_TICKS, slack.ticks());

    trace::instant!("alarms", "hrtimer:programmed",
        trace::Scope::Process,
        "resolution_ns" => resolution_nanos,
        "ticks" => useful_ticks
    );
    let timer_config_id = timer_config.id;
    let start_and_wait_fut = {
        let _sc = ScopedInc::new(&hrtimer_wait_count);
        hrtimer.start_and_wait(
            timer_config.id,
            &ffhh::Resolution::Duration(resolution_nanos),
            useful_ticks,
            hrtimer_scheduled.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("infallible"),
        )
    };

    let hrtimer_scheduled_if_error =
        hrtimer_scheduled.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("infallible");
    let hrtimer_task = scope.spawn_local(async move {
        debug!("hrtimer_task: waiting for hrtimer driver response");
        trace::instant!("alarms", "hrtimer:started", trace::Scope::Process);
        let response = start_and_wait_fut.await;
        trace::instant!("alarms", "hrtimer:response", trace::Scope::Process);
        match response {
            Err(TimerOpsError::Fidl(e)) => {
                defer! {
                    // Allow hrtimer_scheduled to proceed anyways.
                    signal(&hrtimer_scheduled_if_error);
                }
                trace::instant!("alarms", "hrtimer:response:fidl_error", trace::Scope::Process);
                command_send
                    .start_send(Cmd::AlarmFidlError { expired_deadline: now, error: e })
                    .unwrap();
                // BAD: no way to keep alive.
            }
            Err(TimerOpsError::Driver(e)) => {
                defer! {
                    // This should be idempotent if the error occurs after
                    // the timer was scheduled.
                    signal(&hrtimer_scheduled_if_error);
                }
                let driver_error_str = format!("{:?}", e);
                trace::instant!("alarms", "hrtimer:response:driver_error", trace::Scope::Process, "error" => &driver_error_str[..]);
                // This is very common. For example, a "timer canceled" event
                // will result in this code path being hit.
                debug!("schedule_hrtimer: hrtimer driver error: {:?}", e);
                command_send
                    .start_send(Cmd::AlarmDriverError {
                        expired_deadline: now,
                        error: e,
                        timer_config_id,
                        resolution_nanos,
                        ticks: useful_ticks,
                    })
                    .unwrap();
                // BAD: no way to keep alive.
            }
            Ok(keep_alive) => {
                trace::instant!("alarms", "hrtimer:response:alarm", trace::Scope::Process);
                debug!("hrtimer: got alarm response: {:?}", keep_alive);
                // May trigger sooner than the deadline.
                command_send
                    .start_send(Cmd::Alarm { expired_deadline: deadline, keep_alive })
                    .unwrap();
            }
        }
        debug!("hrtimer_task: exiting task.");
        trace::instant!("alarms", "hrtimer:task_exit", trace::Scope::Process);
    }).into();
    debug!("schedule_hrtimer: waiting for event to be signaled");

    {
        let _i = ScopedInc::new(&wait_signaled_count);
        // We must wait here to ensure that the wake alarm has been scheduled.
        log_long_op!(wait_signaled(&hrtimer_scheduled));
    }

    let now_after_signaled = fasync::BootInstant::now();
    let duration_until_scheduled: zx::BootDuration = (now_after_signaled - now).into();
    if duration_until_scheduled > zx::BootDuration::from_nanos(LONG_DELAY_NANOS) {
        trace::duration!("alarms", "schedule_hrtimer:unusual_duration",
            "duration" => duration_until_scheduled.into_nanos());
        warn!(
            "unusual duration until hrtimer scheduled: {}",
            format_duration(duration_until_scheduled)
        );
    }
    // TODO: b/444236931: re-enable.
    //schedule_delay_histogram.insert(duration_until_scheduled.into_nanos());
    debug!("schedule_hrtimer: hrtimer wake alarm has been scheduled.");
    TimerState { task: hrtimer_task, deadline }
}

/// Notify all `timers` that `reference_instant` has been reached.
///
/// The notified `timers` are removed from the list of timers to notify.
///
/// Args:
/// - `timers`: the collection of currently available timers.
/// - `lease_prototype`: an EventPair used as a wake lease.
/// - `reference_instant`: the time instant used as a reference for alarm notification.
/// - `timer_ops_error`: if set, this is the error that happened while attempting to
///   schedule or trigger a timer in hardware.
fn notify_all(
    timers: &mut timers::Heap,
    lease_prototype: &zx::EventPair,
    reference_instant: fasync::BootInstant,
    timer_ops_error: Option<TimerOpsError>,
    _unusual_slack_histogram: &finspect::IntExponentialHistogramProperty,
) -> Result<usize> {
    trace::duration!("alarms", "notify_all");
    let now = fasync::BootInstant::now();
    let mut expired = 0;
    while let Some(timer_node) = timers.maybe_expire_earliest(reference_instant) {
        expired += 1;
        // How much later than requested did the notification happen.
        let deadline = timer_node.get_boot_deadline();
        let alarm = timer_node.id().alarm();
        let alarm_id = alarm.to_string();
        trace::duration!("alarms", "notify_all:notified", "alarm_id" => &*alarm_id);
        fuchsia_trace::flow_step!("alarms", "hrtimer_lifecycle", timers::get_trace_id(&alarm_id));
        let conn_id = timer_node.id().conn.clone();
        let slack: zx::BootDuration = deadline - now;
        if slack < zx::BootDuration::from_nanos(-LONG_DELAY_NANOS) {
            trace::duration!("alarms", "schedule_hrtimer:unusual_slack", "slack" => slack.into_nanos());
            // This alarm triggered noticeably later than it should have.
            warn!(
                "alarm id: {} had an unusually large slack: {}",
                alarm_id,
                format_duration(slack)
            );
        }
        if slack < zx::BootDuration::ZERO {
            // TODO: b/444236931: re-enable.
            //unusual_slack_histogram.insert(-slack.into_nanos());
        }
        if let Some(ref err) = timer_ops_error {
            // Canceled timers are getting notified with alarm, but not other
            // errors.
            if !err.is_canceled() {
                timer_node.get_responder().send(alarm, Err(err.clone().into()));
                continue;
            }
        }
        debug!(
            concat!(
                "wake_alarm_loop: ALARM alarm_id: \"{}\"\n\tdeadline: {},\n\tconn_id: {:?},\n\t",
                "reference_instant: {},\n\tnow: {},\n\tslack: {}",
            ),
            alarm_id,
            format_timer(deadline.into()),
            conn_id,
            format_timer(reference_instant.into()),
            format_timer(now.into()),
            format_duration(slack),
        );
        let lease = lease_prototype.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("infallible");
        trace::instant!("alarms", "notify", trace::Scope::Process, "alarm_id" => &alarm_id[..], "conn_id" => conn_id);
        if let Some(Err(e)) = timer_node.get_responder().send(alarm, Ok(lease)) {
            error!("could not signal responder: {:?}", e);
        }
        trace::instant!("alarms", "notified", trace::Scope::Process);
    }
    trace::instant!("alarms", "notify", trace::Scope::Process, "expired_count" => expired);
    debug!("notify_all: expired count: {}", expired);
    Ok(expired)
    // A new timer is not scheduled yet here.
}

/// Connects to the high resolution timer device driver.
///
/// This function watches the hrtimer service and connects to the first
/// available hrtimer device.
///
/// # Returns
/// A `Result` containing a `ffhh::DeviceProxy` on success, or an error if
/// the connection fails.
pub async fn connect_to_hrtimer_async() -> Result<ffhh::DeviceProxy> {
    debug!("connect_to_hrtimer: trying service");
    let service = Service::open(ffhh::ServiceMarker).context("failed to open hrtimer service")?;
    let instance = service.watch_for_any().await.context("no hrtimer devices found")?;
    instance.connect_to_device().context("failed to connect to hrtimer device")
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fuchsia_async::TestExecutor;
    use futures::select;
    use std::pin::pin;
    use test_case::test_case;
    use test_util::{assert_gt, assert_lt};

    fn fake_wake_lease() -> fidl_fuchsia_power_system::LeaseToken {
        let (_lease, peer) = zx::EventPair::create();
        peer
    }

    #[test]
    fn timer_duration_no_overflow() {
        let duration1 = TimerDuration {
            resolution: zx::BootDuration::from_seconds(100_000_000),
            ticks: u64::MAX,
        };
        let duration2 = TimerDuration {
            resolution: zx::BootDuration::from_seconds(110_000_000),
            ticks: u64::MAX,
        };
        assert_eq!(duration1, duration1);
        assert_eq!(duration2, duration2);

        assert_lt!(duration1, duration2);
        assert_gt!(duration2, duration1);
    }

    #[test_case(
        TimerDuration::new(zx::BootDuration::from_nanos(1), 1),
        TimerDuration::new(zx::BootDuration::from_nanos(1), 1)
    )]
    #[test_case(
        TimerDuration::new(zx::BootDuration::from_nanos(1), 10),
        TimerDuration::new(zx::BootDuration::from_nanos(10), 1)
    )]
    #[test_case(
        TimerDuration::new(zx::BootDuration::from_nanos(10), 1),
        TimerDuration::new(zx::BootDuration::from_nanos(1), 10)
    )]
    #[test_case(
        TimerDuration::new(zx::BootDuration::from_micros(1), 1),
        TimerDuration::new(zx::BootDuration::from_nanos(1), 1000)
    )]
    fn test_slack_eq(one: TimerDuration, other: TimerDuration) {
        assert_eq!(one, other);
    }

    #[test_case(
        TimerDuration::new(zx::BootDuration::from_nanos(1), 1),
        TimerDuration::new(zx::BootDuration::from_nanos(1), 2)
    )]
    #[test_case(
        TimerDuration::new(zx::BootDuration::from_nanos(1), 1),
        TimerDuration::new(zx::BootDuration::from_nanos(10), 1)
    )]
    fn test_slack_lt(one: TimerDuration, other: TimerDuration) {
        assert_lt!(one, other);
    }

    #[test_case(
        TimerDuration::new(zx::BootDuration::from_nanos(1), 2),
        TimerDuration::new(zx::BootDuration::from_nanos(1), 1)
    )]
    #[test_case(
        TimerDuration::new(zx::BootDuration::from_nanos(10), 1),
        TimerDuration::new(zx::BootDuration::from_nanos(1), 1)
    )]
    fn test_slack_gt(one: TimerDuration, other: TimerDuration) {
        assert_gt!(one, other);
    }

    #[test_case(
        vec![zx::BootDuration::from_nanos(1)],
        100,
        zx::BootDuration::from_nanos(0),
        TimerDuration::new(zx::BootDuration::from_nanos(1), 1) ; "0ns becomes 1ns"
    )]
    #[test_case(
        vec![zx::BootDuration::from_nanos(1)],
        100,
        zx::BootDuration::from_nanos(50),
        TimerDuration::new(zx::BootDuration::from_nanos(1), 50) ; "Exact at 50x1ns"
    )]
    #[test_case(
        vec![zx::BootDuration::from_nanos(2)],
        100,
        zx::BootDuration::from_nanos(50),
        TimerDuration::new(zx::BootDuration::from_nanos(2), 25) ; "Exact at 25x2ns"
    )]
    #[test_case(
        vec![zx::BootDuration::from_nanos(3)],
        100,
        zx::BootDuration::from_nanos(50),
        // The closest duration is 51ns.
        TimerDuration::new(zx::BootDuration::from_nanos(3), 17) ; "Inexact at 51ns"
    )]
    #[test_case(
        vec![
            zx::BootDuration::from_nanos(3),
            zx::BootDuration::from_nanos(4)
        ],
        100,
        zx::BootDuration::from_nanos(50),
        TimerDuration::new(zx::BootDuration::from_nanos(3), 17) ; "3ns is a better resolution"
    )]
    #[test_case(
        vec![
            zx::BootDuration::from_nanos(1000),
        ],
        100,
        zx::BootDuration::from_nanos(50),
        TimerDuration::new(zx::BootDuration::from_nanos(1000), 1) ;
        "950ns negative slack is the best we can do"
    )]
    #[test_case(
        vec![
            zx::BootDuration::from_nanos(1),
        ],
        10,
        zx::BootDuration::from_nanos(50),
        TimerDuration::new(zx::BootDuration::from_nanos(1), 10) ;
        "10ns positive slack is the best we can do"
    )]
    #[test_case(
        vec![
            zx::BootDuration::from_millis(1),
            zx::BootDuration::from_micros(100),
            zx::BootDuration::from_micros(10),
            zx::BootDuration::from_micros(1),
        ],
        20,  // Make only one of the resolutions above match.
        zx::BootDuration::from_micros(150),
        TimerDuration::new(zx::BootDuration::from_micros(10), 15) ;
        "Realistic case with resolutions from driver, should be 15us"
    )]
    #[test_case(
        vec![
            zx::BootDuration::from_millis(1),
            zx::BootDuration::from_micros(100),
            zx::BootDuration::from_micros(10),
            zx::BootDuration::from_micros(1),
        ],
        2000,  // Make only one of the resolutions above match.
        zx::BootDuration::from_micros(6000),
        TimerDuration::new(zx::BootDuration::from_millis(1), 6) ;
        "Coarser exact unit wins"
    )]
    #[test_case(
        vec![
            zx::BootDuration::from_millis(1),
            zx::BootDuration::from_millis(10),
            zx::BootDuration::from_millis(100),
        ],
        1000,
        zx::BootDuration::from_micros(-10),
        TimerDuration::new(zx::BootDuration::from_millis(1), 1) ;
        "Negative duration gets the smallest timer duration"
    )]
    #[test_case(
        vec![
            zx::BootDuration::from_millis(1),
            zx::BootDuration::from_millis(10),
            zx::BootDuration::from_millis(100),
        ],
        1000,
        zx::BootDuration::ZERO,
        TimerDuration::new(zx::BootDuration::from_millis(1), 1) ;
        "Zero duration gets the smallest timer duration"
    )]
    fn test_pick_setting(
        resolutions: Vec<zx::BootDuration>,
        max_ticks: u64,
        duration: zx::BootDuration,
        expected: TimerDuration,
    ) {
        let config = TimerConfig::new_from_data(MAIN_TIMER_ID as u64, &resolutions[..], max_ticks);
        let actual = config.pick_setting(duration);

        // .eq() does not work here, since we do not just require that the values
        // be equal, but also that the same resolution is used in both.
        assert_slack_eq(expected, actual);
    }

    // TimerDuration assertion with human-friendly output in case of an error.
    fn assert_slack_eq(expected: TimerDuration, actual: TimerDuration) {
        let slack = expected.duration() - actual.duration();
        assert_eq!(
            actual.resolution(),
            expected.resolution(),
            "\n\texpected: {} ({})\n\tactual  : {} ({})\n\tslack: expected-actual={}",
            expected,
            format_duration(expected.duration()),
            actual,
            format_duration(actual.duration()),
            format_duration(slack)
        );
        assert_eq!(
            actual.ticks(),
            expected.ticks(),
            "\n\texpected: {} ({})\n\tactual  : {} ({})\n\tslack: expected-actual={}",
            expected,
            format_duration(expected.duration()),
            actual,
            format_duration(actual.duration()),
            format_duration(slack)
        );
    }

    #[derive(Debug)]
    enum FakeCmd {
        SetProperties {
            resolutions: Vec<zx::BootDuration>,
            max_ticks: i64,
            keep_alive: zx::EventPair,
            done: zx::Event,
        },
    }

    use std::cell::RefCell;
    use std::rc::Rc;

    // A fake that emulates some aspects of the hrtimer driver.
    //
    // Specifically it can be configured with different resolutions, and will
    // bomb out if any waiting methods are called twice in a succession, without
    // canceling the timer in between.
    fn fake_hrtimer_connection(
        scope: fasync::ScopeHandle,
        rcv: mpsc::Receiver<FakeCmd>,
    ) -> ffhh::DeviceProxy {
        debug!("fake_hrtimer_connection: entry.");
        let (hrtimer, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<ffhh::DeviceMarker>();
        scope.clone().spawn_local(async move {
            let mut rcv = rcv.fuse();
            let timer_properties = Rc::new(RefCell::new(None));
            let wake_lease = Rc::new(RefCell::new(None));

            // Set to true when the hardware timer is supposed to be running.
            // Hardware timer may not be reprogrammed without canceling it first,
            // make sure the tests fail the same way as production would.
            let timer_running = Rc::new(RefCell::new(false));

            loop {
                let timer_properties = timer_properties.clone();
                let wake_lease = wake_lease.clone();
                select! {
                    cmd = rcv.next() => {
                        debug!("fake_hrtimer_connection: cmd: {:?}", cmd);
                        match cmd {
                            Some(FakeCmd::SetProperties{ resolutions, max_ticks, keep_alive, done}) => {
                                let mut timer_props = vec![];
                                for v in 0..10 {
                                    timer_props.push(ffhh::TimerProperties {
                                        supported_resolutions: Some(
                                            resolutions.iter()
                                                .map(|d| ffhh::Resolution::Duration(d.into_nanos())).collect()),
                                        max_ticks: Some(max_ticks.try_into().unwrap()),
                                        // start_and_wait method works.
                                        supports_wait: Some(true),
                                        id: Some(v),
                                        ..Default::default()
                                        },
                                    );
                                }
                                *timer_properties.borrow_mut() = Some(timer_props);
                                *wake_lease.borrow_mut() = Some(keep_alive);
                                debug!("set timer properties to: {:?}", timer_properties);
                                signal(&done);
                            }
                            e => {
                                panic!("unrecognized command: {:?}", e);
                            }
                        }
                        // Set some responses if we have them.
                    },
                    event = stream.next() => {
                        debug!("fake_hrtimer_connection: event: {:?}", event);
                        if let Some(Ok(event)) = event {
                            match event {
                                ffhh::DeviceRequest::Start { responder, .. } => {
                                    assert!(!*timer_running.borrow(), "invariant broken: timer may not be running here");
                                    *timer_running.borrow_mut() = true;
                                    responder.send(Ok(())).expect("");
                                }
                                ffhh::DeviceRequest::Stop { responder, .. } => {
                                    *timer_running.borrow_mut() = false;
                                    responder.send(Ok(())).expect("");
                                }
                                ffhh::DeviceRequest::GetTicksLeft { responder, .. } => {
                                    responder.send(Ok(1)).expect("");
                                }
                                ffhh::DeviceRequest::SetEvent { responder, .. } => {
                                    responder.send(Ok(())).expect("");
                                }
                                ffhh::DeviceRequest::StartAndWait { id, resolution, ticks, setup_event, responder, .. } => {
                                    assert!(!*timer_running.borrow(), "invariant broken: timer may not be running here");
                                    *timer_running.borrow_mut() = true;
                                    debug!("fake_hrtimer_connection: starting timer: \"{}\", resolution: {:?}, ticks: {}", id, resolution, ticks);
                                    let ticks: i64 = ticks.try_into().unwrap();
                                    let sleep_duration  = zx::BootDuration::from_nanos(ticks * match resolution {
                                        ffhh::Resolution::Duration(e) => e,
                                        _ => {
                                            error!("resolution has an unexpected value");
                                            1
                                        }
                                    });
                                    let timer_running_clone = timer_running.clone();
                                    scope.spawn_local(async move {
                                        // Signaling the setup event allows the client to proceed
                                        // with post-scheduling work.
                                        signal(&setup_event);

                                        // Respond after the requested sleep time. In tests this will
                                        // be sleeping in fake time.
                                        fasync::Timer::new(sleep_duration).await;
                                        *timer_running_clone.borrow_mut() = false;
                                        responder.send(Ok(wake_lease.borrow().as_ref().unwrap().duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())).unwrap();
                                        debug!("StartAndWait: hrtimer expired");
                                    });
                                }
                                ffhh::DeviceRequest::StartAndWait2 { responder, .. } => {
                                    assert!(!*timer_running.borrow(), "invariant broken: timer may not be running here");
                                    *timer_running.borrow_mut() = true;
                                    responder.send(Err(ffhh::DriverError::InternalError)).expect("");
                                }
                                ffhh::DeviceRequest::GetProperties { responder, .. } => {
                                    if (*timer_properties).borrow().is_none() {
                                        error!("timer_properties is empty, this is not what you want!");
                                    }
                                    responder
                                        .send(ffhh::Properties {
                                            timers_properties: (*timer_properties).borrow().clone(),
                                            ..Default::default()
                                        })
                                        .expect("");
                                }
                                ffhh::DeviceRequest::ReadTimer { responder, .. } => {
                                    responder.send(Err(ffhh::DriverError::NotSupported)).expect("");
                                }
                                ffhh::DeviceRequest::ReadClock { responder, .. } => {
                                    responder.send(Err(ffhh::DriverError::NotSupported)).expect("");
                                }
                                ffhh::DeviceRequest::_UnknownMethod { .. } => todo!(),
                            }
                        }
                    },
                }
            }
        });
        hrtimer
    }

    fn clone_utc_clock(orig: &fxr::UtcClock) -> fxr::UtcClock {
        orig.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()
    }

    struct TestContext {
        wake_proxy: fta::WakeAlarmsProxy,
        _scope: fasync::Scope,
        _cmd_tx: mpsc::Sender<FakeCmd>,
        // Use to manipulate the UTC clock from the test.
        utc_clock: fxr::UtcClock,
        utc_backstop: fxr::UtcInstant,
    }

    impl TestContext {
        async fn new() -> Self {
            TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(0)).await;

            let scope = fasync::Scope::new();
            let utc_backstop = fxr::UtcInstant::from_nanos(1000);
            let utc_clock =
                fxr::UtcClock::create(zx::ClockOpts::empty(), Some(utc_backstop)).unwrap();
            let utc_clone = clone_utc_clock(&utc_clock);
            let (mut cmd_tx, wake_proxy) = {
                let (tx, rx) = mpsc::channel::<FakeCmd>(0);
                let hrtimer_proxy = fake_hrtimer_connection(scope.to_handle(), rx);

                let inspector = finspect::component::inspector();
                let alarms = Rc::new(Loop::new(
                    scope.to_handle(),
                    hrtimer_proxy,
                    inspector.root().create_child("test"),
                    utc_clone,
                ));

                let (proxy, stream) =
                    fidl::endpoints::create_proxy_and_stream::<fta::WakeAlarmsMarker>();
                scope.spawn_local(async move {
                    serve(alarms, stream).await;
                });
                (tx, proxy)
            };

            let (_wake_lease, peer) = zx::EventPair::create();
            let done = zx::Event::create();
            cmd_tx
                .start_send(FakeCmd::SetProperties {
                    resolutions: vec![zx::Duration::from_nanos(1)],
                    max_ticks: 100,
                    keep_alive: peer,
                    done: done.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                })
                .unwrap();

            // Wait until hrtimer configuration has completed.
            assert_matches!(fasync::OnSignals::new(done, zx::Signals::EVENT_SIGNALED).await, Ok(_));

            Self { wake_proxy, _scope: scope, _cmd_tx: cmd_tx, utc_clock, utc_backstop }
        }
    }

    impl Drop for TestContext {
        fn drop(&mut self) {
            assert_matches!(TestExecutor::next_timer(), None, "Unexpected lingering timers");
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_basic_timed_wait() {
        let ctx = TestContext::new().await;

        let deadline = zx::BootInstant::from_nanos(100);
        let setup_done = zx::Event::create();
        let mut set_task = ctx.wake_proxy.set_and_wait(
            deadline.into(),
            fta::SetMode::NotifySetupDone(
                setup_done.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
            ),
            "Hello".into(),
        );

        assert_matches!(TestExecutor::poll_until_stalled(&mut set_task).await, Poll::Pending);

        let mut setup_done_task =
            pin!(fasync::OnSignals::new(setup_done, zx::Signals::EVENT_SIGNALED));
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut setup_done_task).await,
            Poll::Ready(Ok(_)),
            "Setup event not triggered after scheduling an alarm"
        );

        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(100)).await;
        assert_matches!(TestExecutor::poll_until_stalled(set_task).await, Poll::Ready(Ok(Ok(_))));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_basic_timed_wait_notify() {
        const ALARM_ID: &str = "Hello";
        let ctx = TestContext::new().await;

        let (notifier_client, mut notifier_stream) =
            fidl::endpoints::create_request_stream::<fta::NotifierMarker>();
        let setup_done = zx::Event::create();
        assert_matches!(
            ctx.wake_proxy
                .set(
                    notifier_client,
                    fidl::BootInstant::from_nanos(2),
                    fta::SetMode::NotifySetupDone(
                        setup_done.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()
                    ),
                    ALARM_ID,
                )
                .await,
            Ok(Ok(()))
        );

        let mut done_task = pin!(fasync::OnSignals::new(setup_done, zx::Signals::EVENT_SIGNALED));
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut done_task).await,
            Poll::Ready(Ok(_)),
            "Setup event not triggered after scheduling an alarm"
        );

        let mut next_task = notifier_stream.next();
        assert_matches!(TestExecutor::poll_until_stalled(&mut next_task).await, Poll::Pending);

        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(1)).await;
        assert_matches!(TestExecutor::poll_until_stalled(&mut next_task).await, Poll::Pending);

        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(2)).await;
        assert_matches!(
            TestExecutor::poll_until_stalled(next_task).await,
            Poll::Ready(Some(Ok(fta::NotifierRequest::Notify { alarm_id, .. }))) if alarm_id == ALARM_ID
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_two_alarms_same() {
        const DEADLINE_NANOS: i64 = 100;

        let ctx = TestContext::new().await;

        let mut set_task_1 = ctx.wake_proxy.set_and_wait(
            fidl::BootInstant::from_nanos(DEADLINE_NANOS),
            fta::SetMode::KeepAlive(fake_wake_lease()),
            "Hello1".into(),
        );
        let mut set_task_2 = ctx.wake_proxy.set_and_wait(
            fidl::BootInstant::from_nanos(DEADLINE_NANOS),
            fta::SetMode::KeepAlive(fake_wake_lease()),
            "Hello2".into(),
        );

        assert_matches!(TestExecutor::poll_until_stalled(&mut set_task_1).await, Poll::Pending);
        assert_matches!(TestExecutor::poll_until_stalled(&mut set_task_2).await, Poll::Pending);

        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(DEADLINE_NANOS)).await;

        assert_matches!(
            TestExecutor::poll_until_stalled(&mut set_task_1).await,
            Poll::Ready(Ok(Ok(_)))
        );
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut set_task_2).await,
            Poll::Ready(Ok(Ok(_)))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_two_alarms_same_notify() {
        const DEADLINE_NANOS: i64 = 100;
        const ALARM_ID_1: &str = "Hello1";
        const ALARM_ID_2: &str = "Hello2";

        let ctx = TestContext::new().await;

        let schedule = async |deadline_nanos: i64, alarm_id: &str| {
            let (notifier_client, notifier_stream) =
                fidl::endpoints::create_request_stream::<fta::NotifierMarker>();
            assert_matches!(
                ctx.wake_proxy
                    .set(
                        notifier_client,
                        fidl::BootInstant::from_nanos(deadline_nanos),
                        fta::SetMode::KeepAlive(fake_wake_lease()),
                        alarm_id,
                    )
                    .await,
                Ok(Ok(()))
            );
            notifier_stream
        };

        let mut notifier_1 = schedule(DEADLINE_NANOS, ALARM_ID_1).await;
        let mut notifier_2 = schedule(DEADLINE_NANOS, ALARM_ID_2).await;

        let mut next_task_1 = notifier_1.next();
        let mut next_task_2 = notifier_2.next();

        assert_matches!(TestExecutor::poll_until_stalled(&mut next_task_1).await, Poll::Pending);
        assert_matches!(TestExecutor::poll_until_stalled(&mut next_task_2).await, Poll::Pending);

        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(DEADLINE_NANOS)).await;

        assert_matches!(
            TestExecutor::poll_until_stalled(&mut next_task_1).await,
            Poll::Ready(Some(Ok(fta::NotifierRequest::Notify { alarm_id, .. }))) if alarm_id == ALARM_ID_1
        );
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut next_task_2).await,
            Poll::Ready(Some(Ok(fta::NotifierRequest::Notify { alarm_id, .. }))) if alarm_id == ALARM_ID_2
        );

        assert_matches!(
            TestExecutor::poll_until_stalled(notifier_1.next()).await,
            Poll::Ready(None)
        );
        assert_matches!(
            TestExecutor::poll_until_stalled(notifier_2.next()).await,
            Poll::Ready(None)
        );
    }

    #[test_case(100, 200 ; "push out")]
    #[test_case(200, 100 ; "pull in")]
    #[fuchsia::test(allow_stalls = false)]
    async fn test_two_alarms_different(
        // One timer scheduled at this instant (fake time starts from zero).
        first_deadline_nanos: i64,
        // Another timer scheduled at this instant.
        second_deadline_nanos: i64,
    ) {
        let ctx = TestContext::new().await;

        let mut set_task_1 = ctx.wake_proxy.set_and_wait(
            fidl::BootInstant::from_nanos(first_deadline_nanos),
            fta::SetMode::KeepAlive(fake_wake_lease()),
            "Hello1".into(),
        );
        let mut set_task_2 = ctx.wake_proxy.set_and_wait(
            fidl::BootInstant::from_nanos(second_deadline_nanos),
            fta::SetMode::KeepAlive(fake_wake_lease()),
            "Hello2".into(),
        );

        assert_matches!(TestExecutor::poll_until_stalled(&mut set_task_1).await, Poll::Pending);
        assert_matches!(TestExecutor::poll_until_stalled(&mut set_task_2).await, Poll::Pending);

        // Sort alarms by their deadlines.
        let mut tasks = [(first_deadline_nanos, set_task_1), (second_deadline_nanos, set_task_2)];
        tasks.sort_by(|a, b| a.0.cmp(&b.0));
        let [mut first_task, mut second_task] = tasks;

        // Alarms should fire in order of deadlines.
        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(first_task.0)).await;
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut first_task.1).await,
            Poll::Ready(Ok(Ok(_)))
        );
        assert_matches!(TestExecutor::poll_until_stalled(&mut second_task.1).await, Poll::Pending);

        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(second_task.0)).await;
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut second_task.1).await,
            Poll::Ready(Ok(Ok(_)))
        );
    }

    #[test_case(100, 200 ; "push out")]
    #[test_case(200, 100 ; "pull in")]
    #[fuchsia::test(allow_stalls = false)]
    async fn test_two_alarms_different_notify(
        // One timer scheduled at this instant (fake time starts from zero).
        first_deadline_nanos: i64,
        // Another timer scheduled at this instant.
        second_deadline_nanos: i64,
    ) {
        const ALARM_ID_1: &str = "Hello1";
        const ALARM_ID_2: &str = "Hello2";

        let ctx = TestContext::new().await;

        let schedule = async |deadline_nanos: i64, alarm_id: &str| {
            let (notifier_client, notifier_stream) =
                fidl::endpoints::create_request_stream::<fta::NotifierMarker>();
            assert_matches!(
                ctx.wake_proxy
                    .set(
                        notifier_client,
                        fidl::BootInstant::from_nanos(deadline_nanos),
                        fta::SetMode::KeepAlive(fake_wake_lease()),
                        alarm_id,
                    )
                    .await,
                Ok(Ok(()))
            );
            notifier_stream
        };

        // Sort alarms by their deadlines.
        let mut notifier_all = futures::stream::select_all([
            schedule(first_deadline_nanos, ALARM_ID_1).await,
            schedule(second_deadline_nanos, ALARM_ID_2).await,
        ]);
        let [(early_ns, early_alarm), (later_ns, later_alarm)] = {
            let mut tasks =
                [(first_deadline_nanos, ALARM_ID_1), (second_deadline_nanos, ALARM_ID_2)];
            tasks.sort_by(|a, b| a.0.cmp(&b.0));
            tasks
        };

        // Alarms should fire in order of deadlines.
        let mut next_task = notifier_all.next();
        assert_matches!(TestExecutor::poll_until_stalled(&mut next_task).await, Poll::Pending);

        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(early_ns)).await;
        assert_matches!(
            TestExecutor::poll_until_stalled(next_task).await,
            Poll::Ready(Some(Ok(fta::NotifierRequest::Notify { alarm_id, .. }))) if alarm_id == early_alarm
        );

        let mut next_task = notifier_all.next();
        assert_matches!(TestExecutor::poll_until_stalled(&mut next_task).await, Poll::Pending);

        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(later_ns)).await;
        assert_matches!(
            TestExecutor::poll_until_stalled(next_task).await,
            Poll::Ready(Some(Ok(fta::NotifierRequest::Notify { alarm_id, .. }))) if alarm_id == later_alarm
        );
        assert_matches!(
            TestExecutor::poll_until_stalled(notifier_all.next()).await,
            Poll::Ready(None)
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_alarm_immediate() {
        let ctx = TestContext::new().await;
        let mut set_task = ctx.wake_proxy.set_and_wait(
            fidl::BootInstant::INFINITE_PAST,
            fta::SetMode::KeepAlive(fake_wake_lease()),
            "Hello1".into(),
        );
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut set_task).await,
            Poll::Ready(Ok(Ok(_)))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_alarm_immediate_notify() {
        const ALARM_ID: &str = "Hello";
        let ctx = TestContext::new().await;

        let (notifier_client, mut notifier_stream) =
            fidl::endpoints::create_request_stream::<fta::NotifierMarker>();

        let mut set_task = ctx.wake_proxy.set(
            notifier_client,
            fidl::BootInstant::INFINITE_PAST,
            fta::SetMode::KeepAlive(fake_wake_lease()),
            ALARM_ID,
        );
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut set_task).await,
            Poll::Ready(Ok(Ok(_)))
        );
        assert_matches!(
            TestExecutor::poll_until_stalled(notifier_stream.next()).await,
            Poll::Ready(Some(Ok(fta::NotifierRequest::Notify { alarm_id, .. }))) if alarm_id == ALARM_ID
        );
    }

    // Rescheduling a timer will cancel the earlier call and use the new
    // deadline for the later call.
    #[test_case(200, 100 ; "pull in")]
    #[test_case(100, 200 ; "push out")]
    #[test_case(100, 100 ; "replace with the same deadline")]
    #[fuchsia::test(allow_stalls = false)]
    async fn test_reschedule(initial_deadline_nanos: i64, override_deadline_nanos: i64) {
        const ALARM_ID: &str = "Hello";

        let ctx = TestContext::new().await;

        let schedule = |deadline_nanos: i64| {
            let setup_done = zx::Event::create();
            let task = ctx.wake_proxy.set_and_wait(
                fidl::BootInstant::from_nanos(deadline_nanos),
                fta::SetMode::NotifySetupDone(
                    setup_done.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                ),
                ALARM_ID.into(),
            );
            (task, setup_done)
        };

        // Schedule timer with a long timeout first. Let it wait, then
        // try to reschedule the same timer
        let (mut set_task_1, setup_done_1) = schedule(initial_deadline_nanos);
        fasync::OnSignals::new(setup_done_1, zx::Signals::EVENT_SIGNALED).await.unwrap();
        assert_matches!(TestExecutor::poll_until_stalled(&mut set_task_1).await, Poll::Pending);

        // Schedule the same timer as above, but with a shorter deadline. This
        // should cancel the earlier call.
        let (mut set_task_2, setup_done_2) = schedule(override_deadline_nanos);
        fasync::OnSignals::new(setup_done_2, zx::Signals::EVENT_SIGNALED).await.unwrap();
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut set_task_1).await,
            Poll::Ready(Ok(Err(fta::WakeAlarmsError::Dropped)))
        );
        assert_matches!(TestExecutor::poll_until_stalled(&mut set_task_2).await, Poll::Pending);

        // The later call will be fired exactly on the new shorter deadline.
        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(override_deadline_nanos - 1))
            .await;
        assert_matches!(TestExecutor::poll_until_stalled(&mut set_task_2).await, Poll::Pending);
        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(override_deadline_nanos))
            .await;
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut set_task_2).await,
            Poll::Ready(Ok(Ok(_)))
        );

        // The values in the inspector tree are fixed because the test
        // runs fully deterministically in fake time.
        assert_data_tree!(finspect::component::inspector(), root: {
            test: {
                hardware: {
                    // All alarms fired, so this should be "none".
                    current_deadline: "(none)",
                    remaining_until_alarm: "(none)",
                },
                now_formatted: format!("{override_deadline_nanos}ns ({override_deadline_nanos})"),
                now_ns: override_deadline_nanos,
                pending_timers: "Boot:\n\t\n\tUTC:\n\t",
                pending_timers_count: 0u64,
                requested_deadlines_ns: AnyProperty,
                schedule_delay_ns: AnyProperty,
                slack_ns: AnyProperty,
                boot_deadlines_count: AnyProperty,
                utc_deadlines_count: AnyProperty,
                debug_node: contains {},
            },
        });
    }

    // Rescheduling a timer will send an error on the old notifier and use the
    // new notifier for the new deadline.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_reschedule_notify() {
        const ALARM_ID: &str = "Hello";
        const INITIAL_DEADLINE_NANOS: i64 = 100;
        const OVERRIDE_DEADLINE_NANOS: i64 = 200;

        let ctx = TestContext::new().await;

        let schedule = async |deadline_nanos: i64| {
            let (notifier_client, notifier_stream) =
                fidl::endpoints::create_request_stream::<fta::NotifierMarker>();
            assert_matches!(
                ctx.wake_proxy
                    .set(
                        notifier_client,
                        fidl::BootInstant::from_nanos(deadline_nanos),
                        fta::SetMode::KeepAlive(fake_wake_lease()),
                        ALARM_ID.into(),
                    )
                    .await,
                Ok(Ok(()))
            );
            notifier_stream
        };

        let mut notifier_1 = schedule(INITIAL_DEADLINE_NANOS).await;
        let mut next_task_1 = notifier_1.next();
        assert_matches!(TestExecutor::poll_until_stalled(&mut next_task_1).await, Poll::Pending);

        let mut notifier_2 = schedule(OVERRIDE_DEADLINE_NANOS).await;
        let mut next_task_2 = notifier_2.next();
        assert_matches!(TestExecutor::poll_until_stalled(&mut next_task_2).await, Poll::Pending);

        // First notifier is called with an error then closed.
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut next_task_1).await,
            Poll::Ready(Some(Ok(fta::NotifierRequest::NotifyError { alarm_id, error, .. }))) if alarm_id == ALARM_ID && error == fta::WakeAlarmsError::Dropped
        );
        assert_matches!(
            TestExecutor::poll_until_stalled(notifier_1.next()).await,
            Poll::Ready(None)
        );

        // Second notifier is called upon the new deadline then closed.
        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(OVERRIDE_DEADLINE_NANOS))
            .await;
        assert_matches!(
            TestExecutor::poll_until_stalled(next_task_2).await,
            Poll::Ready(Some(Ok(fta::NotifierRequest::Notify { alarm_id, .. }))) if alarm_id == ALARM_ID
        );
        assert_matches!(
            TestExecutor::poll_until_stalled(notifier_2.next()).await,
            Poll::Ready(None)
        );
    }

    // If we get two scheduling FIDL errors one after another, the wake alarm
    // manager must not lock up.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_fidl_error_on_reschedule() {
        const DEADLINE_NANOS: i64 = 100;

        let (wake_proxy, _stream) =
            fidl::endpoints::create_proxy_and_stream::<fta::WakeAlarmsMarker>();
        drop(_stream);

        assert_matches!(
            wake_proxy
                .set_and_wait(
                    zx::BootInstant::from_nanos(DEADLINE_NANOS).into(),
                    fta::SetMode::KeepAlive(fake_wake_lease()),
                    "hello1".into(),
                )
                .await,
            Err(fidl::Error::ClientChannelClosed { .. })
        );

        assert_matches!(
            wake_proxy
                .set_and_wait(
                    zx::BootInstant::from_nanos(DEADLINE_NANOS).into(),
                    fta::SetMode::KeepAlive(fake_wake_lease()),
                    "hello2".into(),
                )
                .await,
            Err(fidl::Error::ClientChannelClosed { .. })
        );
    }

    // Verify that if a UTC timer is scheduled in the future on the UTC timeline, then the
    // UTC clock is changed to move "now" beyond the timer's deadline, the timer fires.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_set_and_wait_utc() {
        const ALARM_ID: &str = "Hello_set_and_wait_utc";
        let ctx = TestContext::new().await;

        let now_boot = fasync::BootInstant::now();
        ctx.utc_clock
            .update(
                zx::ClockUpdate::builder()
                    .absolute_value(now_boot.into(), ctx.utc_backstop)
                    .build(),
            )
            .unwrap();

        let timestamp_utc = ctx.utc_backstop + fxr::UtcDuration::from_nanos(2);
        let mut wake_fut = ctx.wake_proxy.set_and_wait_utc(
            &fta::InstantUtc { timestamp_utc: timestamp_utc.into_nanos() },
            fta::SetMode::KeepAlive(fake_wake_lease()),
            ALARM_ID,
        );

        // Timer is not expired yet.
        assert_matches!(TestExecutor::poll_until_stalled(&mut wake_fut).await, Poll::Pending);

        // Move the UTC timeline.
        ctx.utc_clock
            .update(
                zx::ClockUpdate::builder()
                    .absolute_value(
                        now_boot.into(),
                        ctx.utc_backstop + fxr::UtcDuration::from_nanos(100),
                    )
                    .build(),
            )
            .unwrap();

        // See similar code in the test above.
        TestExecutor::advance_to(fasync::MonotonicInstant::from_nanos(1)).await;
        assert_matches!(TestExecutor::poll_until_stalled(wake_fut).await, Poll::Ready(_));
    }
}
