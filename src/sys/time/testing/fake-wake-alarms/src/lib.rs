// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A fake server library for the protocol `fidl.fuchsia.time.alarms/WakeAlarms`, for reuse in the
//! unit tests of clients of `WakeAlarm`.
//!
//! It serves a simplified but somewhat functional flavor of the `WakeAlarms` FIDL API, which
//! covers the needs of unit tests we identified so far. This is not a fully-fledged
//! implementation, so be sure to verify that what it does is enough for your purposes.
//!
//! If you need a fully capable server component for tests, see
//! `//src/sys/time/testing/wake-alarms` instead.
//!
//! Include the library as follows. In `BUILD.gn`:
//! ```ignore
//! deps = [
//!     "//src/sys/time/testing/fake-wake-alarms:lib",
//! ]
//! ```
//!
//! In rust code:
//! ```ignore
//! use fake_wake_alarms::*; // Or use items by name.
//! ```

use futures::stream::StreamExt;
use scopeguard::defer;
use std::collections::HashMap;
use {fidl_fuchsia_time_alarms as fta, fuchsia_async as fasync, fuchsia_runtime as fxr};

/// Configure the response of [serve_fake_wake_alarms].
#[derive(Debug, Copy, Clone)]
pub enum Response {
    /// Respond immediately, without waiting.
    Immediate,
    /// Delay a response, sometimes controlled by additional values.
    Delayed,
    /// Respond with an error.
    Error,
}

/// Scheduling a hrtimer with this deadline will expire it immediately.
pub const MAGIC_EXPIRE_DEADLINE: i64 = 424242;

/// Makes sure that a dropped responder is properly responded to.
#[derive(Debug)]
pub struct ResponderCleanup {
    alarm_id: String,
    responder: Option<fta::WakeAlarmsSetAndWaitResponder>,
    responder_utc: Option<fta::WakeAlarmsSetAndWaitUtcResponder>,
    notifier: Option<fidl::endpoints::ClientEnd<fta::NotifierMarker>>,
}

impl Drop for ResponderCleanup {
    fn drop(&mut self) {
        if let Some(responder) = self.responder.take() {
            log::debug!("dropping responder: {responder:?}");
            responder
                .send(Err(fta::WakeAlarmsError::Dropped))
                .expect("should be able to respond to a FIDL message")
        }
        if let Some(responder) = self.responder_utc.take() {
            log::debug!("dropping responder: {responder:?}");
            responder
                .send(Err(fta::WakeAlarmsError::Dropped))
                .expect("should be able to respond to a FIDL message")
        }
        if let Some(notifier) = self.notifier.take() {
            log::debug!("dropping notifier: {notifier:?}");
            notifier
                .into_proxy()
                .notify_error(&self.alarm_id, fta::WakeAlarmsError::Dropped)
                .expect("should be able to respond to a FIDL message")
        }
    }
}

fn signal_event(
    event: &zx::Event,
    clear_mask: zx::Signals,
    set_mask: zx::Signals,
) -> Result<(), zx::Status> {
    event
        .signal(clear_mask, set_mask)
        .inspect_err(|err| log::error!(err:?, clear_mask:?, set_mask:?; "while signaling event"))
}

/// A representation of deadlines with different reference points.
#[derive(Debug, Clone, Copy)]
enum Deadline {
    Boot(fasync::BootInstant),
    Utc(fxr::UtcInstant),
}

impl Deadline {
    /// Converts to boot instant.  The results are faked for UTC timestamps.
    fn into_boot(self) -> fasync::BootInstant {
        match self {
            Deadline::Boot(deadline) => deadline,
            // This is wrong, but good enough for tests.
            Deadline::Utc(deadline) => fasync::BootInstant::from_nanos(deadline.into_nanos()),
        }
    }
}

// Describes specific handler variants. Variants are only slightly different, which makes it
// sensible to unify.
enum HandlerVariant {
    SetAndWait {
        responder: fta::WakeAlarmsSetAndWaitResponder,
    },
    SetAndWaitUtc {
        responder: fta::WakeAlarmsSetAndWaitUtcResponder,
    },
    Set {
        responder: fta::WakeAlarmsSetResponder,
        notifier: fidl::endpoints::ClientEnd<fta::NotifierMarker>,
    },
}

async fn handle_set_like_method(
    method_name: &str,
    message_counter: &zx::Counter,
    responders: &mut HashMap<String, ResponderCleanup>,
    deadline: Deadline,
    mode: fta::SetMode,
    alarm_id: String,
    response_type: &Response,
    handler_variant: HandlerVariant,
) {
    log::debug!(
        "serve_fake_wake_alarms: {}: alarm_id: {:?}: deadline: {:?}",
        method_name,
        alarm_id,
        deadline
    );
    defer! {
        if let fta::SetMode::NotifySetupDone(setup_done) = mode {
            // Caller blocks until this event is signaled.
            signal_event(&setup_done, zx::Signals::NONE, zx::Signals::EVENT_SIGNALED).unwrap();
        }
    };
    match response_type {
        // Two possibilities: a "magic" expire deadline, which expires right away, or
        // a "never" expiring deadline, regardless of the actual specified deadline.
        Response::Delayed => {
            if deadline.into_boot().into_nanos() == MAGIC_EXPIRE_DEADLINE {
                log::debug!(
                    "serve_fake_wake_alarms: {method_name}: responding immediately to magic deadline"
                );
                // If any responders are removed, then add one return
                // message for each.
                let r_count_before = responders.len();
                responders.retain(|k, _| *k != alarm_id);
                let r_count_after = responders.len();

                message_counter
                    .add(
                        (r_count_before - r_count_after).try_into().expect("should be convertible"),
                    )
                    .expect("add to message_counter");
                let (_, peer) = zx::EventPair::create();
                message_counter.add(1).expect("add 1 to message counter");
                match handler_variant {
                    HandlerVariant::SetAndWait { responder } => {
                        responder.send(Ok(peer)).expect("send FIDL response");
                    }
                    HandlerVariant::SetAndWaitUtc { responder } => {
                        responder.send(Ok(peer)).expect("send FIDL response");
                    }
                    HandlerVariant::Set { responder, notifier } => {
                        responder.send(Ok(())).unwrap();
                        notifier
                            .into_proxy()
                            .notify(&alarm_id, peer)
                            .expect("send Notify FIDL request");
                    }
                }
            } else {
                log::debug!("serve_fake_wake_alarms: {method_name}: will not respond");
                let removed = match handler_variant {
                    HandlerVariant::SetAndWait { responder } => responders.insert(
                        alarm_id.clone(),
                        ResponderCleanup {
                            alarm_id,
                            responder: Some(responder),
                            responder_utc: None,
                            notifier: None,
                        },
                    ),
                    HandlerVariant::SetAndWaitUtc { responder } => responders.insert(
                        alarm_id.clone(),
                        ResponderCleanup {
                            alarm_id,
                            notifier: None,
                            responder: None,
                            responder_utc: Some(responder),
                        },
                    ),
                    HandlerVariant::Set { responder, notifier } => {
                        responder.send(Ok(())).unwrap();
                        responders.insert(
                            alarm_id.clone(),
                            ResponderCleanup {
                                alarm_id,
                                notifier: Some(notifier),
                                responder_utc: None,
                                responder: None,
                            },
                        )
                    }
                };
                // If some responder was removed, add a return message for it to the message
                // counter.
                if let Some(_) = removed {
                    message_counter.add(1).unwrap();
                }
            }
        }
        Response::Immediate => {
            // Manufacture a token to return, not relevant for the unit tests,
            // so no functionality attributed to it.
            let (_ignored, fake_lease) = zx::EventPair::create();
            message_counter.add(1).unwrap();

            match handler_variant {
                HandlerVariant::SetAndWait { responder } => {
                    responder.send(Ok(fake_lease)).expect("infallible");
                }
                HandlerVariant::SetAndWaitUtc { responder } => {
                    responder.send(Ok(fake_lease)).expect("infallible");
                }
                HandlerVariant::Set { responder, notifier } => {
                    responder.send(Ok(())).expect("send FIDL response");
                    notifier
                        .into_proxy()
                        .notify(&alarm_id, fake_lease)
                        .expect("send Notify FIDL request");
                }
            }
            log::debug!("serve_fake_wake_alarms: {method_name}: test fake responded immediately");
        }
        Response::Error => {
            message_counter.add(1).unwrap();
            match handler_variant {
                HandlerVariant::SetAndWait { responder } => {
                    responder.send(Err(fta::WakeAlarmsError::Unspecified)).expect("infallible");
                }
                HandlerVariant::SetAndWaitUtc { responder } => {
                    responder.send(Err(fta::WakeAlarmsError::Unspecified)).expect("infallible");
                }
                HandlerVariant::Set { responder, notifier } => {
                    // Even if the end result is an error, the responder gets an OK, but
                    // the notifier is told there was an error.
                    responder.send(Ok(())).expect("send FIDL response");
                    notifier
                        .into_proxy()
                        .notify_error(&alarm_id, fta::WakeAlarmsError::Unspecified)
                        .expect("infallible");
                }
            }
            log::debug!("serve_fake_wake_alarms: {method_name}: Responded with error");
        }
    }
}

/// Serves a fake `fuchsia.time.alarms/Wake` API. The behavior is simplistic when compared to
/// the "real" implementation in that it never actually expires alarms on its own, and has
/// fixed behavior for each scheduled alarm, which is selected at the beginning of the test.
///
/// Despite this, we can use it to check a number of correctness scenarios with unit tests.
///
/// This allows us to remove the flakiness that may arise from the use of real time, and also
/// avoid the complications of fake time.  If you want an alarm that expires, schedule it with
/// a deadline of `MAGIC_EXPIRE_DEADLINE` above, and call this with `response_type ==
/// Response::Delayed`.
pub async fn serve_fake_wake_alarms(
    message_counter: zx::Counter,
    response_type: Response,
    mut stream: fta::WakeAlarmsRequestStream,
    once: bool,
) {
    log::warn!("serve_fake_wake_alarms: serving loop entry. response_type={:?}", response_type);
    let mut responders: HashMap<String, ResponderCleanup> = HashMap::new();
    if once {
        return;
    }

    while let Some(maybe_request) = stream.next().await {
        match maybe_request {
            Ok(request) => {
                log::debug!(
                    "serve_fake_wake_alarms: request: {:?}; response_type: {:?}",
                    request,
                    response_type
                );

                match request {
                    fta::WakeAlarmsRequest::SetAndWaitUtc {
                        mode,
                        responder,
                        alarm_id,
                        deadline,
                    } => {
                        handle_set_like_method(
                            "set_and_wait",
                            &message_counter,
                            &mut responders,
                            Deadline::Utc(fxr::UtcInstant::from_nanos(deadline.timestamp_utc)),
                            mode,
                            alarm_id,
                            &response_type,
                            HandlerVariant::SetAndWaitUtc { responder },
                        )
                        .await;
                    }
                    fta::WakeAlarmsRequest::SetAndWait { mode, responder, alarm_id, deadline } => {
                        handle_set_like_method(
                            "set_and_wait",
                            &message_counter,
                            &mut responders,
                            Deadline::Boot(deadline.into()),
                            mode,
                            alarm_id,
                            &response_type,
                            HandlerVariant::SetAndWait { responder },
                        )
                        .await;
                    }
                    fta::WakeAlarmsRequest::Set {
                        notifier,
                        deadline,
                        mode,
                        alarm_id,
                        responder,
                    } => {
                        handle_set_like_method(
                            "set",
                            &message_counter,
                            &mut responders,
                            Deadline::Boot(deadline.into()),
                            mode,
                            alarm_id.clone(),
                            &response_type,
                            HandlerVariant::Set { responder, notifier },
                        )
                        .await;
                    }
                    fta::WakeAlarmsRequest::Cancel { alarm_id, .. } => {
                        let r_count_before = responders.len();
                        responders.retain(|k, _| *k != alarm_id);
                        let r_count_after = responders.len();
                        message_counter
                            .add((r_count_before - r_count_after).try_into().unwrap())
                            .unwrap();

                        log::debug!("serve_fake_wake_alarms: Cancel: {}", alarm_id);
                    }
                    fta::WakeAlarmsRequest::_UnknownMethod { .. } => unreachable!(),
                }
            }
            Err(e) => {
                // This may or may not be an error, depending on what you wanted
                // to test.
                log::warn!("alarms::serve: error in request: {:?}", e);
            }
        }
    }
    log::warn!("serve_fake_wake_alarms: exiting");
}
