// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_fuchsia_power_system::{self as fsag};
use fuchsia_component::server::{ServiceFs, ServiceObj};
use futures::channel::mpsc;
use futures::future::{self, Either};
use futures::lock::Mutex;
use futures::stream::StreamExt;
use lease_management::{MessageSendTracker, SequenceServer};
use log::{error, info, warn};
use std::boxed::Box;
use std::pin::Pin;
use std::sync::Arc;
use zx::{self, Peered};
use {fidl_fuchsia_example_power as fexample, fuchsia_async as fasync};

/// Sends messages while the system is resumed at multiples of a hard-coded rate
/// until |until|. We have two goals:
///   * have the server and client work at different rates so that we can see
///     what happens when there are messages "in flight" at system suspension
///   * in the case suspension is very late in the interval, make sure over the
///     total interval the server produces the same number of messages as the
///     client will consume so we can reach suspension within the time interval
///
/// The details are not particularly important and the current implementation
/// is less test code-driven than it should be, for example, to meet the above
/// goals the test code should always make |time_of_rate_change| half way
/// through the the interval because the sending rates of the server are not
/// configurable.
///
/// The server selects its send rate based on a few factors. If it is between
/// the starting time and |time_of_rate_change| and no suspend callback has
/// happened, it sends 1.5X the rate at which the receiver processes messages.
/// If it is after |time_of_rate_change| and no suspend callback has happened,
/// it sends at 0.5X the processing rate of the client. If at any time it
/// observes a suspend callback, the send rate is set to 0.2X the processing
/// rate of the client and remains that rate for the rest of the interval. No
/// messages are sent between a suspend and resume callback.
async fn send_messages(
    socket: zx::Socket,
    tracker: Arc<Mutex<MessageSendTracker>>,
    sag: fsag::ActivityGovernorProxy,
    until: zx::BootInstant,
    time_of_rate_change: zx::BootInstant,
) {
    if socket.is_closed().unwrap_or(false) {
        panic!("socket should not be closed!");
    }

    // Create a suspend blocker so we know when we suspend/resume
    let (client, server) = fidl::endpoints::create_endpoints::<fsag::SuspendBlockerMarker>();

    let registration_lease = sag
        .register_suspend_blocker(fsag::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(client),
            name: Some("olm_server".into()),
            ..Default::default()
        })
        .await
        .expect("error registering suspend blocker");
    drop(registration_lease);

    let msg = "hello, world";

    let mut send_timer = SendTimer {
        first_suspend: false,
        is_suspended: false,
        timer: None,
        until,
        time_of_rate_change,
    };

    let mut event_stream = Box::new(server.into_stream());

    // Send messages whenever the timer ticks until we run out of time.
    loop {
        let mut next_event = Box::pin(event_stream.next());
        // Wait for a timer ticket or the stream event to complete.
        let wait_result = wait_for_event(&mut send_timer, &mut next_event).await;

        // Only when a timer expires is it time to write a new message to the
        // socket.
        match wait_result {
            Some(EventType::TimerExpired) => {
                // Actually send the message!
                if let Err(e) = socket.write(msg.as_bytes()) {
                    warn!("write loop terminated: {:?}", e);
                    break;
                }
                info!("sent message");

                // Inform the send tracker that we sent a message.
                if let Err(e) = tracker.lock().await.message_sent().await {
                    warn!("Aborting message tracker returned an error:{:?}", e);
                    break;
                }
            }
            Some(EventType::StreamEvent) => {
                // The stream got a request, wait again on the next timer and event.
                continue;
            }
            None => {
                // Time must be up, exit.
                break;
            }
        }
    }
}

async fn serve_message_source_client(
    mut stream: fexample::MessageSourceRequestStream,
    sag: fsag::ActivityGovernorProxy,
    tracker: Arc<Mutex<MessageSendTracker>>,
    params: FrameParameters,
) {
    loop {
        let next_item = stream.next().await;

        if let None = next_item {
            return;
        }

        match next_item.unwrap() {
            Ok(fexample::MessageSourceRequest::ReceiveMessages { socket, responder }) => {
                if let Err(e) = responder.send() {
                    error!("Error sending result to client: {:?}", e);
                    return;
                }
                let tracker_copy = tracker.clone();
                let sag_copy = sag.clone();
                fasync::Task::local(async move {
                    send_messages(
                        socket,
                        tracker_copy,
                        sag_copy,
                        zx::BootInstant::after(zx::BootDuration::from_millis(
                            params.duration.into(),
                        )),
                        zx::BootInstant::after(zx::BootDuration::from_millis(
                            params.rate_change_offset.into(),
                        )),
                    )
                    .await;
                })
                .detach();
            }
            Ok(fexample::MessageSourceRequest::ReceiveBaton { responder }) => {
                tracker.lock().await.set_requester(responder);
            }
            Err(e) => {
                if e.is_closed() {
                    return;
                }
                warn!("Receive error treated as non-terminal: {:?}", e);
                continue;
            }
        }
    }
}

pub enum ExposedCapabilities {
    StartFrame(fexample::FrameControlRequestStream),
    MessageSource(fexample::MessageSourceRequestStream),
    CountCheck(fexample::CounterRequestStream),
}

struct FrameParameters {
    duration: u16,
    rate_change_offset: u16,
}

#[fuchsia::main]
async fn main() {
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    let scope = fasync::Scope::new();

    let mut svc_fs: ServiceFs<ServiceObj<'_, ExposedCapabilities>> = ServiceFs::new();
    svc_fs.dir("svc").add_fidl_service(ExposedCapabilities::StartFrame);
    svc_fs.dir("svc").add_fidl_service(ExposedCapabilities::MessageSource);
    svc_fs.dir("svc").add_fidl_service(ExposedCapabilities::CountCheck);
    svc_fs.take_and_serve_directory_handle().expect("failed to serve namespace");

    // Create a barrier so we don't start serving message requests until we
    // get our configuration
    let (mut frame_params_sender, mut frame_params_recv) = {
        let channel = mpsc::channel::<FrameParameters>(1);
        (Some(channel.0), Some(channel.1))
    };

    // Connect to SAG and create the SequenceServer we'll use to manage batons.
    let sag = fuchsia_component::client::connect_to_protocol::<fsag::ActivityGovernorMarker>()
        .expect("Couldn't connect to SAG");
    let baton_manager = SequenceServer::new(sag.clone()).await;
    let (tracker, sequence_server) = baton_manager.manage();
    scope.spawn_local(async move {
        let _ = sequence_server
            .await
            .expect("Failure running sequence server, maybe SAG is unavailable?");
    });

    // Note that we can really only support one call to MessageSource
    // because the test code wants to ask how many messages we sent to *the*
    // client.
    while let Some(capability_request) = svc_fs.next().await {
        let scope_copy = scope.clone();
        match capability_request {
            ExposedCapabilities::MessageSource(stream) => {
                // Take the barrier which we'll move into the closure used to
                // serve this request stream.
                let params_receiver = frame_params_recv.take();
                let tracker_copy = tracker.clone();
                let sag_cpy = sag.clone();
                scope_copy.spawn(async move {
                    if let Some(mut receiver) = params_receiver {
                        let frame_parameters = receiver.next().await.unwrap();
                        serve_message_source_client(
                            stream,
                            sag_cpy,
                            tracker_copy,
                            frame_parameters,
                        )
                        .await;
                    }
                });
            }
            ExposedCapabilities::StartFrame(mut stream) => {
                let mut frame_params_sender_cpy = frame_params_sender.take();
                scope_copy.spawn(async move {
                    while let Some(Ok(request)) = stream.next().await {
                        match request {
                            fexample::FrameControlRequest::StartFrame {
                                duration_ms,
                                rate_change_offset_ms,
                                responder,
                            } => {
                                if let Some(mut frame_params_sender) =
                                    frame_params_sender_cpy.take()
                                {
                                    // Now we have the frame parameters, put
                                    // into the channel for the message sender
                                    // to pick up
                                    let _ = frame_params_sender.start_send(FrameParameters {
                                        duration: duration_ms,
                                        rate_change_offset: rate_change_offset_ms,
                                    });
                                    let _ = responder.send();
                                }
                            }
                        }
                    }
                });
            }
            // The test code uses this to check how many messages the server
            // reports it sent.
            ExposedCapabilities::CountCheck(mut stream) => {
                // Make a copy of the reference to message tracker so we can
                // use it to respond to count requests.
                let message_tracker = tracker.clone();
                scope_copy.spawn(async move {
                    while let Some(Ok(req)) = stream.next().await {
                        match req {
                            fidl_fuchsia_example_power::CounterRequest::Get { responder } => {
                                responder
                                    .send(message_tracker.lock().await.get_message_count())
                                    .expect("failed to send response");
                            }
                        }
                    }
                });
            }
        }
    }
    scope.join().await;
}

//===========BELOW HERE IS CODE RELATED TO RUNNING THIS EXAMPLE AS AN INTEGRATION TEST===========//

/// Helper to manage intervals between sending messages based on where we are
/// in our run interval and whether we've suspended before or not.
struct SendTimer {
    first_suspend: bool,
    is_suspended: bool,
    timer: Option<Pin<Box<fasync::Timer>>>,
    until: zx::BootInstant,
    time_of_rate_change: zx::BootInstant,
}

impl SendTimer {
    /// Set the next timer, if applicable. If we are beyond the time we are
    /// supposed to terminate, the timer is not created and the function
    /// returns |None|.
    fn set_next_timer(&mut self) -> Option<()> {
        let std_delay = 100i64;
        // Delay between messages initially.
        let initial_delay = (std_delay as f64 / 1.5) as i64;
        // Delay between messages after the rate change.
        let after_rate_change_delay = std_delay * 2;
        // Delay between messages after we observe the first request to suspend.
        let post_suspend_delay = std_delay * 5;

        if zx::BootInstant::get() >= self.until {
            info!("It is after our deadline, time to exit!");
            return None;
        }

        // Determine the next delay. If we are before the first suspend and
        // in our initial part of our run interval, use the initial delay.
        // If we are before the first suspend and after the initial part of
        // our run interval, slow down our send rate. No matter what, if we've
        // seen a suspend request, reduce to the very slow send rate.
        let mut delay = zx::BootDuration::from_millis(
            match (self.first_suspend, zx::BootInstant::get() < self.time_of_rate_change) {
                (false, true) => initial_delay,
                (false, false) => after_rate_change_delay,
                (true, _) => post_suspend_delay,
            },
        );

        // Adjust any delay not to exceed the end of our run interval.
        if zx::BootInstant::after(delay) > self.until {
            delay = zx::BootDuration::from_nanos(
                self.until.into_nanos() - zx::BootInstant::get().into_nanos(),
            )
        }

        self.timer = Some(Box::pin(fasync::Timer::new(delay)));
        Some(())
    }
}

pub enum EventType {
    TimerExpired,
    StreamEvent,
}

/// Waits for the next event to happen. This event might either be a new the
/// |next_request| future completing or the next timer tick happening.
///
/// If the |next_request| future completes, the function returns
/// |EventType::StreamEvent|. |next_request| is completed and therefore should not
/// be used for a new invocation.
///
/// If there is a timer tick the function returns |EventType::TimerExpired| and
/// |next_request| is still pending and therefore can be used for a future
/// invocation.
async fn wait_for_event<'b, 'c>(
    send_timer: &'b mut SendTimer,
    mut next_request: &'c mut Pin<
        Box<futures::stream::Next<'c, Box<fsag::SuspendBlockerRequestStream>>>,
    >,
) -> Option<EventType> {
    if send_timer.timer.is_none() {
        if let None = send_timer.set_next_timer() {
            return None;
        }
    }

    loop {
        match future::select(&mut *next_request, send_timer.timer.take().unwrap()).await {
            // If this is a suspend/resume callback, update our internal state
            // to control whether we send messages to the client or not.
            Either::Left((suspend_event, unexpired_timer)) => {
                send_timer.timer = Some(unexpired_timer);
                match suspend_event {
                    Some(Ok(fidl_fuchsia_power_system::SuspendBlockerRequest::AfterResume {
                        responder,
                    })) => {
                        info!("resumed!");
                        send_timer.is_suspended = false;
                        let _ = responder.send();
                    }
                    Some(Ok(fidl_fuchsia_power_system::SuspendBlockerRequest::BeforeSuspend {
                        responder,
                    })) => {
                        info!("suspended!");
                        send_timer.first_suspend = true;
                        send_timer.is_suspended = true;
                        let _ = responder.send();
                    }
                    Some(Ok(fsag::SuspendBlockerRequest::_UnknownMethod { .. })) => {
                        warn!("unknown method!");
                    }
                    Some(Err(e)) => {
                        if e.is_closed() {
                            warn!("suspend blocker channel closed, exiting");
                            return None;
                        }
                    }
                    None => {
                        warn!("suspend blocker channel closed, exiting");
                        return None;
                    }
                }

                return Some(EventType::StreamEvent);
            }
            Either::Right((_, suspend_blocker)) => {
                next_request = suspend_blocker;
                // Try setting the next timer.
                if let None = send_timer.set_next_timer() {
                    return None;
                }

                // If we're resumed, emit a timer tick, otherwise we'll just
                // start wiaintg on the next timer, effectively we miss the
                // tick if we're suspended.
                if !send_timer.is_suspended {
                    return Some(EventType::TimerExpired);
                }
            }
        }
    }
}
