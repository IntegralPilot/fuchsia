// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_testing::{
    DeadlineEventType, FakeClockControlRequest, FakeClockControlRequestStream, FakeClockRequest,
    FakeClockRequestStream, Increment,
};
use fidl_fuchsia_testing_deadline::DeadlineId;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::stream::{StreamExt, TryStreamExt};
use log::{debug, error, trace, warn};
use zx::Peered;

use fuchsia_sync::Mutex;
use std::collections::{BinaryHeap, HashMap, HashSet, hash_map};
use std::ops;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const DEFAULT_INCREMENTS_MS: i64 = 10;

#[derive(Debug, PartialEq)]
struct ErrorKoidNotRegistered;

impl From<ErrorKoidNotRegistered> for zx::Status {
    fn from(ErrorKoidNotRegistered: ErrorKoidNotRegistered) -> Self {
        zx::Status::BAD_HANDLE
    }
}

// Relationships and properties for Boot and Mono clocks.
trait ClockTraits:
    Ord
    + Copy
    + std::fmt::Debug
    + ops::Sub<Self, Output = Self::Duration>
    + ops::Add<Self::Duration, Output = Self>
    + ops::Sub<Self::Duration, Output = Self>
{
    type Duration: Copy + std::fmt::Debug + Ord;
    const ID: TimelineId;
}

impl ClockTraits for zx::BootInstant {
    type Duration = zx::BootDuration;
    const ID: TimelineId = TimelineId::Boot;
}

impl ClockTraits for zx::MonotonicInstant {
    type Duration = zx::MonotonicDuration;
    const ID: TimelineId = TimelineId::Monotonic;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimelineId {
    Boot,
    Monotonic,
}

#[derive(Debug)]
struct PendingEvent<T, E = zx::Koid> {
    time: T,
    event: E,
}

struct RegisteredEvent {
    event: Rc<zx::EventPair>,
    pending: bool,
    clock: TimelineId,
}

// Ord and Eq implementations provided for use with BinaryHeap.
impl<T: Eq, E> Eq for PendingEvent<T, E> {}
impl<T: PartialEq, E> PartialEq for PendingEvent<T, E> {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time
    }
}

impl<T: PartialOrd + Ord, E> PartialOrd for PendingEvent<T, E> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Ord, E> Ord for PendingEvent<T, E> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.time.cmp(&self.time)
    }
}

impl RegisteredEvent {
    fn signal(&mut self) {
        self.pending = false;
        match self.event.signal_peer(zx::Signals::NONE, zx::Signals::EVENT_SIGNALED) {
            Ok(()) => (),
            Err(zx::Status::PEER_CLOSED) => debug!("Got PEER_CLOSED while signaling an event"),
            Err(e) => error!("Got an unexpected error while signaling an event: {:?}", e),
        }
    }

    fn clear(&mut self) {
        self.pending = false;
        match self.event.signal_peer(zx::Signals::EVENT_SIGNALED, zx::Signals::NONE) {
            Ok(()) => (),
            Err(zx::Status::PEER_CLOSED) => debug!("Got PEER_CLOSED while clearing an event"),
            Err(e) => error!("Got an unexpected error while clearing an event: {:?}", e),
        }
    }
}

#[derive(Eq, PartialEq, Hash, Debug)]
struct StopPoint {
    deadline_id: DeadlineId,
    event_type: DeadlineEventType,
}

#[derive(Debug)]
struct PendingDeadlineExpireEvent<T> {
    deadline_id: DeadlineId,
    deadline: T,
}

// Ord and Eq implementations provided for use with BinaryHeap.
impl<T: Eq> Eq for PendingDeadlineExpireEvent<T> {}
impl<T: PartialEq> PartialEq for PendingDeadlineExpireEvent<T> {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline
    }
}

impl<T: PartialOrd + Ord> PartialOrd for PendingDeadlineExpireEvent<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Ord> Ord for PendingDeadlineExpireEvent<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.deadline.cmp(&self.deadline)
    }
}

struct TaskWithId {
    _task: fasync::Task<()>,
    id: usize,
}

impl Drop for TaskWithId {
    fn drop(&mut self) {
        trace!("dropping task: id={}", self.id);
    }
}

#[derive(Debug)]
struct ClockState<T: ClockTraits> {
    time: T,
    pending_events: BinaryHeap<PendingEvent<T>>,
    pending_deadlines: BinaryHeap<PendingDeadlineExpireEvent<T>>,
}

impl Default for ClockState<zx::MonotonicInstant> {
    fn default() -> Self {
        Self {
            time: zx::MonotonicInstant::from_nanos(1),
            pending_events: BinaryHeap::new(),
            pending_deadlines: BinaryHeap::new(),
        }
    }
}

impl Default for ClockState<zx::BootInstant> {
    fn default() -> Self {
        Self {
            time: zx::BootInstant::from_nanos(1),
            pending_events: BinaryHeap::new(),
            pending_deadlines: BinaryHeap::new(),
        }
    }
}

/// The fake clock implementation.
/// Type parameter `T` is used to observe events during testing.
/// The empty tuple `()` implements `FakeClockObserver` and is meant to be used
/// for production instances.
struct FakeClock<T> {
    boot_clock: ClockState<zx::BootInstant>,
    mono_clock: ClockState<zx::MonotonicInstant>,

    // Shared state that is not clock-specific
    free_running: Option<TaskWithId>,
    registered_events: HashMap<zx::Koid, RegisteredEvent>,
    ignored_deadline_ids: HashSet<DeadlineId>,
    registered_stop_points: HashMap<StopPoint, zx::EventPair>,
    observer: T,
}

trait FakeClockObserver: 'static {
    fn new() -> Self;
    fn event_removed(&mut self, koid: zx::Koid);
}

impl FakeClockObserver for () {
    fn new() -> () {
        ()
    }
    fn event_removed(&mut self, _koid: zx::Koid) {
        /* do nothing, the trait is just used for testing */
    }
}

impl<T: FakeClockObserver> FakeClock<T> {
    fn new() -> Self {
        FakeClock {
            boot_clock: ClockState::default(),
            mono_clock: ClockState::default(),
            free_running: None,
            registered_events: HashMap::new(),
            ignored_deadline_ids: HashSet::new(),
            registered_stop_points: HashMap::new(),
            observer: T::new(),
        }
    }

    fn is_free_running(&self) -> bool {
        self.free_running.is_some()
    }

    fn check_events_common<C: ClockTraits>(
        clock_state: &mut ClockState<C>,
        registered_events: &mut HashMap<zx::Koid, RegisteredEvent>,
    ) {
        while let Some(e) = clock_state.pending_events.peek() {
            if e.time <= clock_state.time {
                let koid = clock_state.pending_events.pop().unwrap().event;
                registered_events.get_mut(&koid).unwrap().signal();
            } else {
                debug!("Next event for clock {:?} in {:?} ns", C::ID, e.time - clock_state.time);
                break;
            }
        }
    }

    fn check_events(&mut self) {
        Self::check_events_common(&mut self.mono_clock, &mut self.registered_events);
        Self::check_events_common(&mut self.boot_clock, &mut self.registered_events);
    }

    /// Check if a matching stop point is registered and attempts to signal the matching eventpair
    /// if one is registered. Returns true iff a match exists and signaling the event pair succeeds.
    fn check_stop_point(
        stop_point: &StopPoint,
        registered_stop_points: &mut HashMap<StopPoint, zx::EventPair>,
    ) -> bool {
        if let Some(stop_point_eventpair) = registered_stop_points.remove(&stop_point) {
            match stop_point_eventpair.signal_peer(zx::Signals::NONE, zx::Signals::EVENT_SIGNALED) {
                Ok(()) => true,
                Err(zx::Status::PEER_CLOSED) => {
                    debug!("Got PEER_CLOSED while signaling a named event");
                    false
                }
                Err(e) => {
                    error!("Failed to signal named event: {:?}", e);
                    false
                }
            }
        } else {
            false
        }
    }

    /// Check if any expired stop points are registered and signal any that exist. Returns true iff
    /// at least one is expired and has been successfully signaled.
    fn check_stop_points_common<C: ClockTraits>(
        clock_state: &mut ClockState<C>,
        registered_stop_points: &mut HashMap<StopPoint, zx::EventPair>,
    ) -> bool {
        let mut stop_time = false;
        while let Some(e) = clock_state.pending_deadlines.peek() {
            if e.deadline <= clock_state.time {
                let stop_point = StopPoint {
                    deadline_id: clock_state.pending_deadlines.pop().unwrap().deadline_id,
                    event_type: DeadlineEventType::Expired,
                };
                if Self::check_stop_point(&stop_point, registered_stop_points) {
                    stop_time = true;
                }
            } else {
                break;
            }
        }
        stop_time
    }

    fn check_stop_points(&mut self) -> bool {
        let mono_stopped =
            Self::check_stop_points_common(&mut self.mono_clock, &mut self.registered_stop_points);
        let boot_stopped =
            Self::check_stop_points_common(&mut self.boot_clock, &mut self.registered_stop_points);
        mono_stopped || boot_stopped
    }

    fn install_event_common<C: ClockTraits>(
        arc_self: FakeClockHandle<T>,
        time: C,
        event: zx::EventPair,
        clock_state: &mut ClockState<C>,
        registered_events: &mut HashMap<zx::Koid, RegisteredEvent>,
    ) {
        let koid = if let Ok(koid) = event.basic_info().map(|i| i.related_koid) {
            koid
        } else {
            return;
        };
        // avoid installing duplicate events if user is calling the API by
        // mistake, but warn in log.
        if registered_events.contains_key(&koid) {
            warn!("RegisterEvent called with already known event, rescheduling instead.");
            Self::reschedule_event_common(time, koid, clock_state, registered_events).unwrap();
            return;
        }

        let event = Rc::new(event);

        let pending = PendingEvent { time, event: koid };
        let mut registered = RegisteredEvent {
            pending: pending.time > clock_state.time,
            event: event.clone(),
            clock: C::ID,
        };

        if registered.pending {
            debug!("Registering event at {:?} -> {:?}", time, time - clock_state.time);
            clock_state.pending_events.push(pending);
        } else {
            // signal immediately if the deadline is in the past.
            registered.signal();
        };

        registered_events.insert(koid, registered);
        fasync::Task::local(async move {
            if let Ok(_) = fasync::OnSignals::new(&*event, zx::Signals::EVENTPAIR_PEER_CLOSED).await
            {
                let mut mc = arc_self.lock();
                mc.cancel_event(koid);
                mc.registered_events.remove(&koid).expect("Registered event disappeared");
                mc.observer.event_removed(koid);
            }
        })
        .detach();
    }

    fn install_event_in_mono(
        &mut self,
        arc_self: FakeClockHandle<T>,
        time: zx::MonotonicInstant,
        event: zx::EventPair,
    ) {
        Self::install_event_common(
            arc_self,
            time,
            event,
            &mut self.mono_clock,
            &mut self.registered_events,
        );
    }

    fn install_event_in_boot(
        &mut self,
        arc_self: FakeClockHandle<T>,
        time: zx::BootInstant,
        event: zx::EventPair,
    ) {
        Self::install_event_common(
            arc_self,
            time,
            event,
            &mut self.boot_clock,
            &mut self.registered_events,
        );
    }

    fn reschedule_event_common<C: ClockTraits>(
        time: C,
        koid: zx::Koid,
        clock_state: &mut ClockState<C>,
        registered_events: &mut HashMap<zx::Koid, RegisteredEvent>,
    ) -> Result<(), ErrorKoidNotRegistered> {
        // First, remove the old event if it was pending.
        let entry = if let Some(e) = registered_events.get_mut(&koid) {
            e
        } else {
            return Err(ErrorKoidNotRegistered);
        };

        if entry.pending {
            clock_state.pending_events = clock_state
                .pending_events
                .drain()
                .filter(|e| e.event != koid)
                .collect::<Vec<_>>()
                .into();
        }
        entry.clear(); // Clear any existing signal.

        // Now, add the new event.
        if time <= clock_state.time {
            debug!("Immediately signaling reschedule to {:?}", time);
            entry.signal();
        } else {
            debug!(
                "Rescheduling event for clock {:?} at {:?} -> {:?}",
                C::ID,
                time,
                time - clock_state.time
            );
            entry.pending = true;
            clock_state.pending_events.push(PendingEvent { time, event: koid });
        }
        Ok(())
    }

    fn reschedule_event_in_mono(
        &mut self,
        time: zx::MonotonicInstant,
        koid: zx::Koid,
    ) -> Result<(), ErrorKoidNotRegistered> {
        Self::reschedule_event_common(time, koid, &mut self.mono_clock, &mut self.registered_events)
    }

    fn reschedule_event_in_boot(
        &mut self,
        time: zx::BootInstant,
        koid: zx::Koid,
    ) -> Result<(), ErrorKoidNotRegistered> {
        Self::reschedule_event_common(time, koid, &mut self.boot_clock, &mut self.registered_events)
    }

    fn cancel_event_common<C: ClockTraits>(clock_state: &mut ClockState<C>, koid: zx::Koid) {
        clock_state.pending_events = clock_state
            .pending_events
            .drain()
            .filter(|e| {
                if e.event != koid {
                    true
                } else {
                    // clear any signals in the event if we're cancelling it
                    debug!("Cancelling event registered at {:?} {:?}", C::ID, e.time);
                    false
                }
            })
            .collect::<Vec<_>>()
            .into();
    }

    fn cancel_event(&mut self, koid: zx::Koid) {
        let entry = if let Some(e) = self.registered_events.get_mut(&koid) {
            e
        } else {
            warn!("Unrecognized event in cancel call");
            return;
        };
        if entry.pending {
            match entry.clock {
                TimelineId::Monotonic => {
                    Self::cancel_event_common(&mut self.mono_clock, koid);
                }
                TimelineId::Boot => {
                    Self::cancel_event_common(&mut self.boot_clock, koid);
                }
            }
        }
        // always clear signals (even if entry was not pending)
        entry.clear();
    }

    /// Set a stop point at which to stop time and signal the provided `eventpair`.
    /// Returns `ZX_ALREADY_BOUND` if an identical stop point is already registered.
    fn set_stop_point(
        &mut self,
        stop_point: StopPoint,
        eventpair: zx::EventPair,
    ) -> Result<(), zx::Status> {
        trace!("setting stop point: {:?}", stop_point);
        match self.registered_stop_points.entry(stop_point) {
            hash_map::Entry::Occupied(mut occupied) => {
                match occupied
                    .get()
                    .wait_one(zx::Signals::EVENTPAIR_PEER_CLOSED, zx::MonotonicInstant::ZERO)
                    .to_result()
                {
                    Ok(_) => {
                        // Okay to replace an eventpair if the other end is already closed.
                        let _previous = occupied.insert(eventpair);
                        Ok(())
                    }
                    Err(zx::Status::TIMED_OUT) => {
                        warn!("Received duplicate interest in stop point {:?}.", occupied.key());
                        Err(zx::Status::ALREADY_BOUND)
                    }
                    Err(e) => {
                        error!("Got an error while checking signals on an eventpair: {:?}", e);
                        Err(zx::Status::ALREADY_BOUND)
                    }
                }
            }
            hash_map::Entry::Vacant(vacant) => {
                let _value: &mut zx::EventPair = vacant.insert(eventpair);
                Ok(())
            }
        }
    }

    fn add_named_deadline(
        &mut self,
        pending_deadline: PendingDeadlineExpireEvent<zx::MonotonicInstant>,
    ) {
        trace!("adding pending deadline: {:?}", pending_deadline);
        let () = self.mono_clock.pending_deadlines.push(pending_deadline);
    }

    fn add_named_boot_deadline(
        &mut self,
        pending_deadline: PendingDeadlineExpireEvent<zx::BootInstant>,
    ) {
        trace!("adding pending deadline: {:?}", pending_deadline);
        let () = self.boot_clock.pending_deadlines.push(pending_deadline);
    }

    fn add_ignored_deadline(&mut self, ignored_deadline: DeadlineId) {
        trace!("adding ignored deadline: {:?}", ignored_deadline);
        let _ = self.ignored_deadline_ids.insert(ignored_deadline);
    }

    fn increment(&mut self, increment: &Increment) {
        let nanos = match increment {
            Increment::Determined(d) => *d,
            Increment::Random(rr) => {
                if let Ok(v) = u64::try_from(rr.min_rand).and_then(|min| {
                    u64::try_from(rr.max_rand)
                        .map(|max| min + (rand::random::<u64>() % (max - min)))
                        .and_then(i64::try_from)
                }) {
                    v
                } else {
                    DEFAULT_INCREMENTS_MS
                }
            }
        };
        self.boot_clock.time += zx::BootDuration::from_nanos(nanos);
        self.mono_clock.time += zx::MonotonicDuration::from_nanos(nanos);
        trace!(
            "incrementing mock clock {:?} => {:?}; time now: monotonic={:?} boot={:?}",
            increment, nanos, self.mono_clock.time, self.boot_clock.time
        );
        let () = self.check_events();
        if self.check_stop_points() {
            let () = self.stop_free_running();
        }
    }

    fn increment_mono_to_boot_offset(&mut self, offset_nanos: i64) {
        self.boot_clock.time += zx::BootDuration::from_nanos(offset_nanos);
        trace!(
            "incrementing mono-to-boot offset by {:?}; time now: monotonic={:?} boot={:?}",
            offset_nanos, self.mono_clock.time, self.boot_clock.time
        );
        let () = self.check_events();
        if self.check_stop_points() {
            let () = self.stop_free_running();
        }
    }

    fn stop_free_running(&mut self) {
        // Does this get canceled then?
        drop(self.free_running.take().map(|t| {
            debug!(
                "stop free running at: monotonic={:?} boot={:?}, no more ticks allowed. id={}",
                self.mono_clock.time, self.boot_clock.time, t.id
            );
            t
        }));
    }
}

type FakeClockHandle<T> = Arc<Mutex<FakeClock<T>>>;

// Starts the free-running clock. If you intend to start fake clock multiple
// times, make sure to have an intervening [stop_free_running] call between
// each two successive calls.
fn start_free_running<T: FakeClockObserver>(
    mock_clock: &FakeClockHandle<T>,
    real_increment: zx::MonotonicDuration,
    increment: Increment,
) {
    static INSTANCE_ID: AtomicUsize = AtomicUsize::new(0);

    let mock_clock_clone = Arc::clone(&mock_clock);

    debug!(
        "start free running mock clock: real_increment={:?} increment={:?}",
        real_increment, increment
    );
    let fr = &mut mock_clock.lock().free_running;
    assert!(
        fr.is_none(),
        "start_free_running called again without an intervening stop_free_running"
    );
    INSTANCE_ID.fetch_add(1, Ordering::SeqCst);
    let id = INSTANCE_ID.load(Ordering::SeqCst);

    debug!("creating task: id={}", id);
    let task = fasync::Task::local(async move {
        debug!("started task: id={}", id);
        loop {
            // This must produce at most one .await yield. This way, when
            // the task gets canceled, we are sure not to increment the
            // stopped counter anymore.  If we used streams from fasync::Interval,
            // we could not make that guarantee.
            // See: b/369671675 for details.
            fasync::Timer::new(real_increment).await;
            debug!("tick: id={}", id);
            mock_clock_clone.lock().increment(&increment);
        }
    });
    *fr = Some(TaskWithId { _task: task, id });
}

fn stop_free_running<T: FakeClockObserver>(mock_clock: &FakeClockHandle<T>) {
    mock_clock.lock().stop_free_running();
}

fn check_valid_increment(increment: &Increment) -> bool {
    match increment {
        Increment::Determined(_) => true,
        Increment::Random(rr) => rr.min_rand >= 0 && rr.max_rand >= 0 && rr.max_rand > rr.min_rand,
    }
}

async fn handle_control_events<T: FakeClockObserver>(
    mock_clock: FakeClockHandle<T>,
    rs: FakeClockControlRequestStream,
) -> Result<(), fidl::Error> {
    rs.try_for_each(|req| async {
        match req {
            FakeClockControlRequest::Advance { increment, responder } => {
                if check_valid_increment(&increment) {
                    let mut mc = mock_clock.lock();
                    if mc.is_free_running() {
                        responder.send(Err(zx::Status::ACCESS_DENIED.into_raw()))
                    } else {
                        mc.increment(&increment);
                        responder.send(Ok(()))
                    }
                } else {
                    responder.send(Err(zx::Status::INVALID_ARGS.into_raw()))
                }
            }
            FakeClockControlRequest::IncrementMonoToBootOffsetBy { increment, responder } => {
                if increment > 0 {
                    let mut mc = mock_clock.lock();
                    if mc.is_free_running() {
                        responder.send(Err(zx::Status::ACCESS_DENIED.into_raw()))
                    } else {
                        mc.increment_mono_to_boot_offset(increment);
                        responder.send(Ok(()))
                    }
                } else {
                    responder.send(Err(zx::Status::INVALID_ARGS.into_raw()))
                }
            }
            FakeClockControlRequest::Pause { responder } => {
                stop_free_running(&mock_clock);
                responder.send()
            }
            FakeClockControlRequest::ResumeWithIncrements { real, increment, responder } => {
                if real <= 0 || !check_valid_increment(&increment) {
                    responder.send(Err(zx::Status::INVALID_ARGS.into_raw()))
                } else {
                    // stop free running if we are
                    stop_free_running(&mock_clock);
                    start_free_running(
                        &mock_clock,
                        zx::MonotonicDuration::from_nanos(real),
                        increment,
                    );
                    responder.send(Ok(()))
                }
            }
            FakeClockControlRequest::AddStopPoint {
                deadline_id,
                event_type,
                on_stop,
                responder,
            } => {
                debug!("stop point of type {:?} registered", event_type);
                let mut mc = mock_clock.lock();
                if mc.is_free_running() {
                    responder.send(Err(zx::Status::ACCESS_DENIED.into_raw()))
                } else {
                    responder.send(
                        mc.set_stop_point(StopPoint { deadline_id, event_type }, on_stop)
                            .map_err(zx::Status::into_raw),
                    )
                }
            }
            FakeClockControlRequest::IgnoreNamedDeadline { deadline_id, responder } => {
                debug!("Ignoring named deadline with id {:?}", deadline_id);
                let mut mc = mock_clock.lock();
                mc.add_ignored_deadline(deadline_id);
                responder.send()
            }
        }
    })
    .await
}

async fn handle_events<T: FakeClockObserver>(
    mock_clock: FakeClockHandle<T>,
    rs: FakeClockRequestStream,
) -> Result<(), fidl::Error> {
    rs.try_for_each(|req| async {
        match req {
            FakeClockRequest::RegisterEventInMonotonic { time, event, control_handle: _ } => {
                mock_clock.lock().install_event_in_mono(
                    Arc::clone(&mock_clock),
                    time,
                    event.into(),
                );
                Ok(())
            }
            FakeClockRequest::RegisterEventInBoot { time, event, control_handle: _ } => {
                mock_clock.lock().install_event_in_boot(
                    Arc::clone(&mock_clock),
                    time,
                    event.into(),
                );
                Ok(())
            }
            FakeClockRequest::Get { responder } => {
                let clock = mock_clock.lock();
                responder.send(clock.boot_clock.time, clock.mono_clock.time)
            }
            FakeClockRequest::RescheduleEventInMonotonic { event, time, responder } => {
                let mut mc = mock_clock.lock();
                let result = event
                    .koid()
                    .and_then(|k| mc.reschedule_event_in_mono(time, k).map_err(zx::Status::from));
                responder.send(result.map_err(|status: zx::Status| {
                    warn!("error in reschedule call {:?}", status);
                    status.into_raw()
                }))
            }
            FakeClockRequest::RescheduleEventInBoot { event, time, responder } => {
                let mut mc = mock_clock.lock();
                let result = event
                    .koid()
                    .and_then(|k| mc.reschedule_event_in_boot(time, k).map_err(zx::Status::from));
                responder.send(result.map_err(|status: zx::Status| {
                    warn!("error in reschedule call {:?}", status);
                    status.into_raw()
                }))
            }
            FakeClockRequest::CancelEvent { event, responder } => {
                if let Ok(k) = event.koid() {
                    mock_clock.lock().cancel_event(k);
                }
                responder.send()
            }
            FakeClockRequest::CreateNamedDeadlineInMonotonic { id, duration, responder } => {
                debug!("Creating named deadline with id {:?}", id);
                let stop_point =
                    StopPoint { deadline_id: id.clone(), event_type: DeadlineEventType::Set };
                if FakeClock::<T>::check_stop_point(
                    &stop_point,
                    &mut mock_clock.lock().registered_stop_points,
                ) {
                    stop_free_running(&mock_clock);
                }

                let deadline = if mock_clock.lock().ignored_deadline_ids.contains(&id) {
                    zx::MonotonicInstant::INFINITE
                } else {
                    mock_clock.lock().mono_clock.time + zx::MonotonicDuration::from_nanos(duration)
                };

                let expiration_point =
                    PendingDeadlineExpireEvent { deadline_id: id, deadline: deadline };
                mock_clock.lock().add_named_deadline(expiration_point);

                responder.send(deadline)
            }
            FakeClockRequest::CreateNamedDeadlineInBoot { id, duration, responder } => {
                debug!("Creating named deadline with id {:?}", id);
                let stop_point =
                    StopPoint { deadline_id: id.clone(), event_type: DeadlineEventType::Set };
                if FakeClock::<T>::check_stop_point(
                    &stop_point,
                    &mut mock_clock.lock().registered_stop_points,
                ) {
                    stop_free_running(&mock_clock);
                }

                let deadline = if mock_clock.lock().ignored_deadline_ids.contains(&id) {
                    zx::BootInstant::INFINITE
                } else {
                    mock_clock.lock().boot_clock.time + zx::BootDuration::from_nanos(duration)
                };

                let expiration_point =
                    PendingDeadlineExpireEvent { deadline_id: id, deadline: deadline };
                mock_clock.lock().add_named_boot_deadline(expiration_point);

                responder.send(deadline)
            }
        }
    })
    .await
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    debug!("Starting mock clock service");

    let mock_clock = Arc::new(Mutex::new(FakeClock::<()>::new()));
    start_free_running(
        &mock_clock,
        zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS),
        Increment::Determined(
            zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS).into_nanos(),
        ),
    );
    let m1 = Arc::clone(&mock_clock);

    let mut fs = ServiceFs::new_local();
    fs.dir("svc")
        .add_fidl_service(move |rs: FakeClockControlRequestStream| {
            let cl = Arc::clone(&mock_clock);
            fasync::Task::local(async move {
                handle_control_events(cl, rs).await.unwrap_or_else(|e| {
                    error!("Got unexpected error while serving fake clock control: {:?}", e)
                });
            })
            .detach()
        })
        .add_fidl_service(move |rs: FakeClockRequestStream| {
            let cl = Arc::clone(&m1);
            fasync::Task::local(async move {
                handle_events(cl, rs).await.unwrap_or_else(|e| {
                    error!("Got unexpected error while serving fake clock: {:?}", e)
                });
            })
            .detach()
        });
    fs.take_and_serve_directory_handle()?;
    let () = fs.collect().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_testing::{FakeClockControlMarker, FakeClockMarker};
    use futures::channel::mpsc;
    use futures::pin_mut;
    use named_timer::DeadlineId;
    use zx::Koid;

    const DEADLINE_ID: DeadlineId<'static> = DeadlineId::new("component_1", "code_1");
    const DEADLINE_ID_2: DeadlineId<'static> = DeadlineId::new("component_1", "code_2");

    #[fuchsia::test]
    fn test_event_heap() {
        let time = zx::MonotonicInstant::get();
        let after = time + zx::MonotonicDuration::from_millis(10);
        let e1 = PendingEvent { time, event: 0 };
        let e2 = PendingEvent { time: after, event: 1 };
        let mut heap = BinaryHeap::new();
        heap.push(e2);
        heap.push(e1);
        assert_eq!(heap.pop().unwrap().time, time);
        assert_eq!(heap.pop().unwrap().time, after);
    }

    #[fuchsia::test]
    fn test_simple_increments() {
        let mut mock_clock = FakeClock::<()>::new();
        let begin = mock_clock.mono_clock.time;
        let skip = zx::MonotonicDuration::from_millis(10);
        let increment = Increment::Determined(skip.into_nanos());
        mock_clock.increment(&increment);
        assert_eq!(mock_clock.mono_clock.time, begin + skip);
    }

    #[fuchsia::test]
    fn test_random_increments() {
        let mut mock_clock = FakeClock::<()>::new();
        let min = zx::MonotonicDuration::from_nanos(10);
        let max = zx::MonotonicDuration::from_nanos(20);
        for _ in 0..200 {
            let begin = mock_clock.mono_clock.time;
            let allowed = (begin + min).into_nanos()..(begin + max).into_nanos();
            let increment = Increment::Random(fidl_fuchsia_testing::RandomRange {
                min_rand: min.into_nanos(),
                max_rand: max.into_nanos(),
            });
            mock_clock.increment(&increment);
            assert!(allowed.contains(&mock_clock.mono_clock.time.into_nanos()));
        }
    }

    #[fuchsia::test]
    fn test_add_ignored_deadline() {
        let mut mock_clock = FakeClock::<()>::new();
        mock_clock.add_ignored_deadline(DEADLINE_ID.into());
        assert_eq!(mock_clock.ignored_deadline_ids, HashSet::from([DEADLINE_ID.into()]));

        // Attempt to add the same deadline again, which should result in a no-op.
        mock_clock.add_ignored_deadline(DEADLINE_ID.into());
        assert_eq!(mock_clock.ignored_deadline_ids, HashSet::from([DEADLINE_ID.into()]));
    }

    fn check_signaled(e: &zx::EventPair) -> bool {
        e.wait_one(zx::Signals::EVENTPAIR_SIGNALED, zx::MonotonicInstant::from_nanos(0))
            .map(|s| s & zx::Signals::EVENTPAIR_SIGNALED != zx::Signals::NONE)
            .unwrap_or(false)
    }

    #[fuchsia::test]
    async fn test_event_signaling() {
        let clock_handle = Arc::new(Mutex::new(FakeClock::<()>::new()));
        let mut mock_clock = clock_handle.lock();
        let (e1, cli1) = zx::EventPair::create();
        let time = mock_clock.mono_clock.time;
        mock_clock.install_event_in_mono(
            Arc::clone(&clock_handle),
            time + zx::MonotonicDuration::from_millis(10),
            e1,
        );
        let (e2, cli2) = zx::EventPair::create();
        mock_clock.install_event_in_mono(
            Arc::clone(&clock_handle),
            time + zx::MonotonicDuration::from_millis(20),
            e2,
        );
        let (e3, cli3) = zx::EventPair::create();
        mock_clock.install_event_in_mono(Arc::clone(&clock_handle), time, e3);
        // only e3 should've signalled immediately:
        assert!(!check_signaled(&cli1));
        assert!(!check_signaled(&cli2));
        assert!(check_signaled(&cli3));
        // increment clock by 10 millis:
        let increment = Increment::Determined(zx::MonotonicDuration::from_millis(10).into_nanos());
        mock_clock.increment(&increment);
        assert!(check_signaled(&cli1));
        assert!(!check_signaled(&cli2));
        // increment clock by another 10 millis and check that e2 is signaled
        mock_clock.increment(&increment);
        assert!(check_signaled(&cli3));
    }

    #[fuchsia::test]
    async fn test_free_running() {
        let clock_handle = Arc::new(Mutex::new(FakeClock::<()>::new()));
        let event = {
            let mut mock_clock = clock_handle.lock();
            let (event, client) = zx::EventPair::create();
            let sched = mock_clock.mono_clock.time + zx::MonotonicDuration::from_millis(10);
            mock_clock.install_event_in_mono(Arc::clone(&clock_handle), sched, event);
            client
        };

        start_free_running(
            &clock_handle,
            zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS),
            Increment::Determined(
                zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS).into_nanos(),
            ),
        );
        let _ = fasync::OnSignals::new(&event, zx::Signals::EVENT_SIGNALED).await.unwrap();
        stop_free_running(&clock_handle);

        // after free running has ended, timer must not be updating anymore:
        let bef = clock_handle.lock().mono_clock.time;
        fasync::Timer::new(zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(30)))
            .await;
        assert_eq!(clock_handle.lock().mono_clock.time, bef);
    }

    struct RemovalObserver {
        sender: mpsc::UnboundedSender<zx::Koid>,
        receiver: Option<mpsc::UnboundedReceiver<zx::Koid>>,
    }

    impl FakeClockObserver for RemovalObserver {
        fn new() -> Self {
            let (sender, r) = mpsc::unbounded();
            Self { sender, receiver: Some(r) }
        }

        fn event_removed(&mut self, koid: Koid) {
            self.sender.unbounded_send(koid).unwrap();
        }
    }

    #[fuchsia::test]
    async fn test_observes_handle_closed() {
        let clock_handle = Arc::new(Mutex::new(FakeClock::<RemovalObserver>::new()));
        let event = {
            let mut mock_clock = clock_handle.lock();
            let (event, client) = zx::EventPair::create();
            let sched = mock_clock.mono_clock.time + zx::MonotonicDuration::from_millis(10);
            mock_clock.install_event_in_mono(Arc::clone(&clock_handle), sched, event);
            client
        };
        let mut recv = clock_handle.lock().observer.receiver.take().unwrap();
        // store the koid
        let koid = event.koid().unwrap();
        // dispose of the client side
        std::mem::drop(event);
        assert_eq!(recv.next().await.unwrap(), koid);
    }

    #[fuchsia::test]
    async fn test_reschedule() {
        const TEN_MILLIS: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(10);
        let clock_handle = Arc::new(Mutex::new(FakeClock::<RemovalObserver>::new()));
        let mut mock_clock = clock_handle.lock();
        let (event, client) = zx::EventPair::create();
        let sched = mock_clock.mono_clock.time + TEN_MILLIS;
        mock_clock.install_event_in_mono(Arc::clone(&clock_handle), sched, event);
        assert!(!check_signaled(&client));
        // now reschedule the same event:
        let sched = mock_clock.mono_clock.time + zx::MonotonicDuration::from_millis(20);
        let res = mock_clock.reschedule_event_in_mono(sched, client.koid().unwrap());
        assert_eq!(res, Ok(()));
        println!("{:?}", mock_clock.mono_clock.pending_events);
        assert!(!check_signaled(&client));
        // advance time and ensure that we don't fire the event
        let increment = Increment::Determined(TEN_MILLIS.into_nanos());
        mock_clock.increment(&increment);
        assert!(!check_signaled(&client));
        let increment = Increment::Determined(TEN_MILLIS.into_nanos());
        mock_clock.increment(&increment);
        assert!(check_signaled(&client));
        // clear the signal, reschedule once more and see that it gets hit again.
        client.signal(zx::Signals::EVENTPAIR_SIGNALED, zx::Signals::NONE).unwrap();
        assert!(!check_signaled(&client));
        let sched = mock_clock.mono_clock.time + TEN_MILLIS;
        let res = mock_clock.reschedule_event_in_mono(sched, client.koid().unwrap());
        assert_eq!(res, Ok(()));
        // not yet signaled...
        assert!(!check_signaled(&client));
        // increment once again and it should be signaled then:
        let increment = Increment::Determined(TEN_MILLIS.into_nanos());
        mock_clock.increment(&increment);
        assert!(check_signaled(&client));
    }

    #[fuchsia::test]
    async fn test_stop_points() {
        let clock_handle = Arc::new(Mutex::new(FakeClock::<RemovalObserver>::new()));
        let (client_event, server_event) = zx::EventPair::create();
        let () = clock_handle
            .lock()
            .set_stop_point(
                StopPoint { deadline_id: DEADLINE_ID.into(), event_type: DeadlineEventType::Set },
                server_event,
            )
            .expect("set stop point failed");
        let () = start_free_running(
            &clock_handle,
            zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS),
            Increment::Determined(
                zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS).into_nanos(),
            ),
        );
        // Checking for the stop point should signal the event pair.
        assert!(FakeClock::<RemovalObserver>::check_stop_point(
            &StopPoint { deadline_id: DEADLINE_ID.into(), event_type: DeadlineEventType::Set },
            &mut clock_handle.lock().registered_stop_points
        ));
        assert!(check_signaled(&client_event));
        let () = stop_free_running(&clock_handle);

        // A deadline set to expire in the future stops time when the deadline is reached.
        let future_deadline_timeout =
            clock_handle.lock().mono_clock.time + zx::MonotonicDuration::from_millis(10);
        let () = clock_handle.lock().add_named_deadline(PendingDeadlineExpireEvent {
            deadline_id: DEADLINE_ID.into(),
            deadline: future_deadline_timeout,
        });
        let (client_event, server_event) = zx::EventPair::create();
        let () = clock_handle
            .lock()
            .set_stop_point(
                StopPoint {
                    deadline_id: DEADLINE_ID.into(),
                    event_type: DeadlineEventType::Expired,
                },
                server_event,
            )
            .expect("set stop point failed");
        let () = start_free_running(
            &clock_handle,
            zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS),
            Increment::Determined(
                zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS).into_nanos(),
            ),
        );
        assert_eq!(
            fasync::OnSignals::new(&client_event, zx::Signals::EVENTPAIR_SIGNALED).await.unwrap()
                & !zx::Signals::EVENTPAIR_PEER_CLOSED,
            zx::Signals::EVENTPAIR_SIGNALED
        );
        assert!(!clock_handle.lock().is_free_running());
        assert_eq!(clock_handle.lock().mono_clock.time, future_deadline_timeout);
    }

    #[fuchsia::test]
    async fn test_ignored_stop_points() {
        let clock_handle = Arc::new(Mutex::new(FakeClock::<RemovalObserver>::new()));
        warn!("checkpoint 1: {:?}", clock_handle.lock().mono_clock.time);
        let () = start_free_running(
            &clock_handle,
            zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS),
            Increment::Determined(
                zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS).into_nanos(),
            ),
        );
        // Checking for an unregistered stop point should not stop time.
        assert!(!FakeClock::<RemovalObserver>::check_stop_point(
            &StopPoint { deadline_id: DEADLINE_ID.into(), event_type: DeadlineEventType::Set },
            &mut clock_handle.lock().registered_stop_points
        ));
        assert!(clock_handle.lock().is_free_running());
        warn!("checkpoint 2: {:?}", clock_handle.lock().mono_clock.time);

        // Time is not stopped if the other end of a registered event pair is dropped.
        let (client_event, server_event) = zx::EventPair::create();
        let () = clock_handle
            .lock()
            .set_stop_point(
                StopPoint { deadline_id: DEADLINE_ID.into(), event_type: DeadlineEventType::Set },
                server_event,
            )
            .expect("set stop point failed");
        warn!("checkpoint 3: {:?}", clock_handle.lock().mono_clock.time);

        stop_free_running(&clock_handle);
        let () = start_free_running(
            &clock_handle,
            zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS),
            Increment::Determined(
                zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS).into_nanos(),
            ),
        );
        drop(client_event);
        assert!(!FakeClock::<RemovalObserver>::check_stop_point(
            &StopPoint { deadline_id: DEADLINE_ID.into(), event_type: DeadlineEventType::Set },
            &mut clock_handle.lock().registered_stop_points
        ));
        warn!("checkpoint 3: {:?}", clock_handle.lock().mono_clock.time);
        assert!(clock_handle.lock().is_free_running());
        let () = stop_free_running(&clock_handle);

        warn!("checkpoint 4: {:?}", clock_handle.lock().mono_clock.time);
        // If we set two EXPIRED points and drop the handle of the earlier one, time should stop
        // on the later stop point.
        let future_deadline_timeout_1 =
            clock_handle.lock().mono_clock.time + zx::MonotonicDuration::from_millis(10);
        let future_deadline_timeout_2 =
            clock_handle.lock().mono_clock.time + zx::MonotonicDuration::from_millis(20);
        let () = clock_handle.lock().add_named_deadline(PendingDeadlineExpireEvent {
            deadline_id: DEADLINE_ID.into(),
            deadline: future_deadline_timeout_1,
        });
        let () = clock_handle.lock().add_named_deadline(PendingDeadlineExpireEvent {
            deadline_id: DEADLINE_ID_2.into(),
            deadline: future_deadline_timeout_2,
        });
        let (client_event_1, server_event_1) = zx::EventPair::create();
        let () = clock_handle
            .lock()
            .set_stop_point(
                StopPoint {
                    deadline_id: DEADLINE_ID.into(),
                    event_type: DeadlineEventType::Expired,
                },
                server_event_1,
            )
            .expect("set stop point failed");
        let (client_event_2, server_event_2) = zx::EventPair::create();
        let () = clock_handle
            .lock()
            .set_stop_point(
                StopPoint {
                    deadline_id: DEADLINE_ID_2.into(),
                    event_type: DeadlineEventType::Expired,
                },
                server_event_2,
            )
            .expect("set stop point failed");
        drop(client_event_1);

        warn!("checkpoint 5: {:?}", clock_handle.lock().mono_clock.time);
        let start_time = clock_handle.lock().mono_clock.time;
        warn!("about to start free running from: {:?}", start_time);

        let () = start_free_running(
            &clock_handle,
            zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS),
            Increment::Determined(
                zx::MonotonicDuration::from_millis(DEFAULT_INCREMENTS_MS).into_nanos(),
            ),
        );
        warn!("before signal");
        assert_eq!(
            fasync::OnSignals::new(&client_event_2, zx::Signals::EVENTPAIR_SIGNALED).await.unwrap()
                & !zx::Signals::EVENTPAIR_PEER_CLOSED,
            zx::Signals::EVENTPAIR_SIGNALED
        );
        warn!("after_signal");
        assert!(!clock_handle.lock().is_free_running());
        assert_eq!(
            clock_handle.lock().mono_clock.time,
            future_deadline_timeout_2,
            "left: actual; right: expected"
        );
    }

    #[fuchsia::test]
    fn duplicate_stop_points_rejected() {
        let mut clock = FakeClock::<()>::new();
        let (client_event_1, server_event_1) = zx::EventPair::create();
        assert!(
            clock
                .set_stop_point(
                    StopPoint {
                        deadline_id: DEADLINE_ID.into(),
                        event_type: DeadlineEventType::Expired
                    },
                    server_event_1
                )
                .is_ok()
        );

        let (client_event_2, server_event_2) = zx::EventPair::create();
        assert_eq!(
            clock.set_stop_point(
                StopPoint {
                    deadline_id: DEADLINE_ID.into(),
                    event_type: DeadlineEventType::Expired
                },
                server_event_2
            ),
            Err(zx::Status::ALREADY_BOUND)
        );

        // original can still be signaled.
        assert!(FakeClock::<()>::check_stop_point(
            &StopPoint { deadline_id: DEADLINE_ID.into(), event_type: DeadlineEventType::Expired },
            &mut clock.registered_stop_points
        ));
        assert!(check_signaled(&client_event_1));
        assert!(!check_signaled(&client_event_2));
    }

    #[fuchsia::test]
    fn duplicate_stop_point_accepted_if_initial_closed() {
        let mut clock = FakeClock::<()>::new();
        let (client_event_1, server_event_1) = zx::EventPair::create();
        assert!(
            clock
                .set_stop_point(
                    StopPoint {
                        deadline_id: DEADLINE_ID.into(),
                        event_type: DeadlineEventType::Expired
                    },
                    server_event_1
                )
                .is_ok()
        );

        drop(client_event_1);
        let (client_event_2, server_event_2) = zx::EventPair::create();
        assert!(
            clock
                .set_stop_point(
                    StopPoint {
                        deadline_id: DEADLINE_ID.into(),
                        event_type: DeadlineEventType::Expired
                    },
                    server_event_2
                )
                .is_ok()
        );

        // The later eventpair is signaled when checking a stop point.
        assert!(FakeClock::<()>::check_stop_point(
            &StopPoint { deadline_id: DEADLINE_ID.into(), event_type: DeadlineEventType::Expired },
            &mut clock.registered_stop_points
        ));
        assert!(check_signaled(&client_event_2));
    }

    #[fuchsia::test]
    async fn test_ignore_named_deadline() {
        let clock_handle = Arc::new(Mutex::new(FakeClock::<RemovalObserver>::new()));

        let (fake_clock_proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<FakeClockMarker>();
        let fake_clock_server_fut = handle_events(clock_handle.clone(), stream);
        pin_mut!(fake_clock_server_fut);

        let (fake_clock_control_proxy, control_stream) =
            fidl::endpoints::create_proxy_and_stream::<FakeClockControlMarker>();
        let fake_clock_control_server_fut =
            handle_control_events(clock_handle.clone(), control_stream);
        pin_mut!(fake_clock_control_server_fut);

        let server =
            futures::future::try_join(fake_clock_server_fut, fake_clock_control_server_fut);
        let client = async move {
            fake_clock_control_proxy.pause().await.expect("failed to pause the clock");

            fake_clock_control_proxy
                .ignore_named_deadline(&DEADLINE_ID.into())
                .await
                .expect("failed to ignore deadline");

            // Set an arbitrary time to see if it is replaced with zx::MonotonicInstant::INFINITE.
            let deadline_time_millis = 10;
            let deadline = fake_clock_proxy
                .create_named_deadline_in_monotonic(
                    &DEADLINE_ID.into(),
                    zx::MonotonicDuration::from_millis(deadline_time_millis).into_nanos(),
                )
                .await
                .expect("failed to create named deadline");

            assert_eq!(deadline, zx::MonotonicInstant::INFINITE);
            Ok(())
        };

        let (((), ()), ()) =
            futures::future::try_join(server, client).await.expect("client should finish first");

        // Confirm there is a deadline in the list and that the deadline is infinite.
        assert_eq!(clock_handle.lock().mono_clock.pending_deadlines.len(), 1);
        assert_eq!(
            clock_handle.lock().mono_clock.pending_deadlines.pop().unwrap(),
            PendingDeadlineExpireEvent {
                deadline_id: DEADLINE_ID.into(),
                deadline: zx::MonotonicInstant::INFINITE,
            }
        );
    }
}
