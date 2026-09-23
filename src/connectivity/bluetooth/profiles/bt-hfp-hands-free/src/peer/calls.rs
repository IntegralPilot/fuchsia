// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! At a high level, the primary element of this module is the `Calls` struct.
//! This is a container for a number of `Call` structs.
//!
//! Each `Call` is a `Stream` of `CallOutput` which are used to drive the
//! current running procedure, initiate a new procedure, or do other things like
//! tear down SCO.  Internally, the `Call` wraps a request stream for the `Call`
//! protocol which allows FIDL clients to control the call.
//!
//! For example, a headset or carkit may send a FIDL request to hang up an in
//! progress call, and this would eventually cause the corresponding `Call`
//! stream to yield the appropriate `CallOutput`,
//! `CallOutput::ProcedureInput(ProcedureInput::CommandFromHf(CommandFromHf::HangupCall))`.
//! Additionally `Call` has several methods on it that allow the
//! `ProcedureManager` to manipulate it, such as by setting the phone number
//! associated with the call.
//!
//! However, the `Call` struct is not public--instead it is accessed through the
//! `Calls` struct, which contains zero or more `Call` structs. The `Calls`
//! struct also is a `Stream`.  It multiplexes the `Stream`s of the internal
//! `Call` structs.  Similarly, the `Calls` struct has several public methods to
//! set `Call` state that are routed to the proper `Call`.

use anyhow::{Error, bail, format_err};
use async_helpers::maybe_stream::MaybeStream;
use bt_hfp::call::list::{Idx as CallIndex, List as CallList};
use bt_hfp::call::{Direction, Number, indicators as call_indicators};
use fidl_fuchsia_bluetooth_hfp::{
    CallDirection, CallMarker, CallRequest, CallRequestStream, CallState, CallWatchStateResponder,
    NextCall, PeerHandlerWatchNextCallResponder,
};
use fuchsia_bluetooth::types::PeerId;
use fuchsia_sync::Mutex;
use futures::stream::FusedStream;
use futures::{Stream, StreamExt};
use log::{debug, error, info, warn};
use std::collections::VecDeque;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use crate::one_to_one::OneToOneMatcher;
use crate::peer::ag_indicators::CallIndicator;
use crate::peer::procedure::{CommandFromHf, ProcedureInput};

use shutdown_state::ShutdownState;

// Maximum call count to prevent malicious peers from adding arbitrarily many calls.
static MAX_NUM_CALLS: usize = 20;

/// Information sent to the responder method for the WatchNextCall hanging get matcher.
struct WatchCallResponderInfo {
    #[allow(unused)]
    peer_id: PeerId, // For logging
    #[allow(unused)]
    call_index: Option<CallIndex>, // For logging
    call_state: CallState,                     // Sent to the hanging get
    shutdown_state: Arc<Mutex<ShutdownState>>, // For updating whether the Terminated state has been sent
}

impl std::fmt::Debug for WatchCallResponderInfo {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("WatchCallResponderInfo")
            .field("peer_id", &self.peer_id)
            .field("call_index", &self.call_index)
            .field("call_state", &self.call_state)
            .field("shutdown_state", &"<lock>")
            .finish()
    }
}

type ResponderResult = Result<(), fidl::Error>;
type WatchStateMatcher =
    OneToOneMatcher<WatchCallResponderInfo, CallWatchStateResponder, ResponderResult>;

mod shutdown_state {
    /// Records the state of various events that need to occur before we can stop awaiting on a
    /// Call's Stream implementation. We want to guarantee that we've both received a
    /// +CIEV(call = 0) from the peer and either sent a State::Terminated to the FIDL client or
    /// gotten an error because the client has already closed the channel.
    ///
    /// Additionally we need to guarantee that we send the peer AT+CHUP at most once, even in the
    /// case of an error.
    #[derive(Debug, Default)]
    pub struct ShutdownState {
        received_call_none: bool,
        sent_terminated: bool,
        received_fidl_error: bool,

        // Records whether we have sent an AT+CHUP to the peer.  This prevents an issue where we have
        // already sent the hang up command and the FIDL client closes the channel.  In that case,
        // we would naively send the hang up command on a FIDL error.  Recording that we have already
        // done so will prevent us from doing it twice.
        sent_hang_up: bool,
    }

    impl ShutdownState {
        /// Can we stop awaiting on this Call's Stream.
        pub fn is_complete(&self) -> bool {
            self.received_call_none && (self.sent_terminated || self.received_fidl_error)
        }

        /// Should we send a AT+CHUP when requested to by the client.
        pub fn can_send_hang_up(&self) -> bool {
            !self.sent_hang_up && !self.received_call_none
        }

        pub fn received_call_none(&mut self) {
            self.received_call_none = true;
        }

        pub fn sent_terminated(&mut self) {
            self.sent_terminated = true;
        }

        pub fn received_fidl_error(&mut self) {
            self.received_fidl_error = true;
        }

        pub fn sent_hang_up(&mut self) {
            self.sent_hang_up = true;
        }
    }
}

#[derive(Debug)]
pub enum CallOutput {
    ProcedureInput(ProcedureInput),
    TransferCallToAg,
    QueryCalls,
}

/// This struct contains information about individual calls and methods to
/// manipulate that state.  Additionally, it acts as a stream of `CallOutput`
/// which translate incoming FIDL requests to change the call's state.
struct Call {
    peer_id: PeerId,               // Used for logging
    call_index: Option<CallIndex>, // Set once the call is inserted in a CallList

    state: Option<CallState>, // Set to Some when we get any +CIEVs for this call.
    // For incoming calls, set to Some when we get a +CLIP.  For incoming calls,
    // set to Some initially.
    number: Option<Number>,
    direction: Option<Direction>,

    // This is set to None when creating a Call, and then set to Some when a
    // request stream is created when creating the NextCall to yield to a
    // hanging get call to WatchNextCall.
    request_stream: MaybeStream<CallRequestStream>,

    watch_state_hanging_get_matcher: WatchStateMatcher,
    waker: Option<Waker>,

    shutdown_state: Arc<Mutex<ShutdownState>>,
}

fn respond_to_watch_call_state(
    info: WatchCallResponderInfo,
    responder: CallWatchStateResponder,
) -> Result<(), fidl::Error> {
    if info.call_state == CallState::Terminated {
        info.shutdown_state.lock().sent_terminated();
    }
    debug!("Sending {:?}, shutdown_state {:?}", info, info.shutdown_state.lock());
    responder.send(info.call_state)
}

impl Call {
    pub fn new(peer_id: PeerId) -> Self {
        let watch_state_hanging_get_matcher = OneToOneMatcher::new(respond_to_watch_call_state);

        Self {
            peer_id,
            call_index: None,
            state: None,
            number: None,
            direction: None,
            request_stream: MaybeStream::default(),
            watch_state_hanging_get_matcher,
            waker: None,
            shutdown_state: Default::default(),
        }
    }

    fn awaken(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    pub fn set_call_index(&mut self, call_index: CallIndex) {
        self.call_index = Some(call_index);
    }

    // TODO(https://fxbug.dev/135158) Handle setting phone numbers.
    #[allow(unused)]
    pub fn set_number(&mut self, number: Number) {
        info!("Setting {:?} for peer {:} for call {:?}.", number, self.peer_id, self.call_index);
        self.number = Some(number);
    }

    // Returns true iff the state is set to Terminated.
    pub fn set_state_by_indicator(
        &mut self,
        indicator: CallIndicator,
        sco_connected: bool,
    ) -> bool {
        info!(
            "Setting call state by indicator {:?}, sco connected {} for peer {} for call {:?}.",
            indicator, sco_connected, self.peer_id, self
        );
        let call_state_option = match indicator {
            CallIndicator::Call(call_indicators::Call::None) => Some(CallState::Terminated),
            CallIndicator::Call(call_indicators::Call::Some) if sco_connected => {
                Some(CallState::OngoingActive)
            }
            CallIndicator::Call(call_indicators::Call::Some) =>
            /* if !sco_connected */
            {
                Some(CallState::TransferredToAg)
            }
            // TODO(https://fxbug.dev/135119) Handle multiple calls.
            CallIndicator::CallHeld(_) => {
                error!(
                    "Received indicator {:?} for peer {:} but call holding unimplemented.",
                    indicator, self.peer_id
                );
                None
            }
            // CallSetup::None indicates the end of the call setup and not a new state, which
            // should be set by a Call or CallHeld indicator.
            CallIndicator::CallSetup(call_indicators::CallSetup::None) => None,
            CallIndicator::CallSetup(call_indicators::CallSetup::Incoming) => {
                Some(CallState::IncomingRinging)
            }
            CallIndicator::CallSetup(call_indicators::CallSetup::OutgoingDialing) => {
                Some(CallState::OutgoingDialing)
            }
            CallIndicator::CallSetup(call_indicators::CallSetup::OutgoingAlerting) => {
                Some(CallState::OutgoingAlerting)
            }
        };

        if let Some(call_state) = call_state_option {
            self.set_and_report_state(call_state);
        }

        return call_state_option == Some(CallState::Terminated);
    }

    pub fn set_sco_connected(&mut self, sco_connected: bool) {
        info!(
            "Toggling sco connection call state {:?}, sco connected {} for peer {} for call {:?}.",
            self.state, sco_connected, self.peer_id, self
        );
        let new_state = match self.state {
            Some(CallState::OngoingActive) if !sco_connected => CallState::TransferredToAg,
            Some(CallState::TransferredToAg) if sco_connected => CallState::OngoingActive,
            Some(CallState::OngoingActive) | Some(CallState::TransferredToAg) => {
                // This is probably a bug in the peer task.
                error!(
                    "Toggling call sco connection state for peer {}, call {:?}, \
                            when new sco state and old call state are inconsistent: \
                            old call state: {:?}, \
                            new sco connection state: {}",
                    self.peer_id, self.call_index, self.state, sco_connected
                );
                return;
            }
            _ => {
                // Nothing to do
                return;
            }
        };

        self.set_and_report_state(new_state);
    }

    fn set_and_report_state(&mut self, new_state: CallState) {
        info!("Setting call state {:?} for peer {:} for call {:?}.", new_state, self.peer_id, self);

        self.state = Some(new_state);
        let watch_call_responder_info = WatchCallResponderInfo {
            peer_id: self.peer_id,
            call_index: self.call_index,
            call_state: new_state,
            shutdown_state: self.shutdown_state.clone(),
        };
        self.watch_state_hanging_get_matcher.enqueue_left(watch_call_responder_info);

        // Make sure the stream is pumped to deliver this new state to a WatchState responder.
        self.awaken();
    }

    pub fn set_queried_call_info(
        &mut self,
        direction: Direction,
        state: CallState,
        // TODO(https://fxbug.dev/135119) Handle multiple calls
        _multiparty: bool,
        number: Option<Number>,
    ) {
        if Some(direction) != self.direction && self.direction.is_some() {
            error!("Queried call info for call {:?} has different direction {:?}", self, direction);
        }
        self.direction = Some(direction);

        if Some(state) != self.state && self.state.is_some() {
            error!("Queried call info for call {:?} has different state {:?}", self, state);
        }
        self.set_and_report_state(state);

        if number != self.number && self.number.is_some() && number.is_some() {
            // Number is some
            error!(
                "Queried call info for call {:?} has different number {:?}",
                self,
                number.as_ref().unwrap()
            );
        }
        self.number = number;

        // This is a hack.  If +CLCC doesn't contain the number for this call,
        // we probably won't get it and so set it to the empty string so as to
        // be able to respond to WatchNextCall.
        if self.number.is_none() {
            self.number = Some(Number::from_non_at_string("").expect("empty number is valid"));
        }
    }

    // The watch_state_hanging_get_matcher stream should be drained affer calling this method, as
    // it does not set store the waker so no client will poll the stream and do so for us.
    fn handle_watch_state(&mut self, responder: CallWatchStateResponder) {
        info!("Handling Call::WatchState for peer {:} for call {:?}.", self.peer_id, self);

        // Enqueue the WatchState responder.  This will match with any current or future enqueued
        // call states changes and respond to the hanging get.
        self.watch_state_hanging_get_matcher.enqueue_right(responder);

        // We don't need to wake the waker here because this is called from the
        // Stream impl poll_next which immediately pumps the matcher.
    }

    fn maybe_hang_up(&mut self) -> Poll<Option<CallOutput>> {
        let mut shutdown_state = self.shutdown_state.lock();
        if !shutdown_state.can_send_hang_up() {
            debug!(
                "Not sending hang up for peer {:} for call {:?}, shutdown state {:?}",
                self.peer_id, self, shutdown_state
            );
            Poll::Pending
        } else {
            info!(
                "Sending hang up for peer {:} for call {:?}, shutdown state {:?}.",
                self.peer_id, self, shutdown_state
            );
            shutdown_state.sent_hang_up();
            Poll::Ready(Some(CallOutput::ProcedureInput(ProcedureInput::CommandFromHf(
                CommandFromHf::HangUpCall,
            ))))
        }
    }

    // Must not be called with a WatchState or a SendDtmfCode request.
    fn call_request_to_call_output(
        &mut self,
        call_request: CallRequest,
    ) -> Poll<Option<CallOutput>> {
        match call_request {
            // TODO(https://fxbug.dev/135119) Handle multiple calls
            CallRequest::RequestHold { control_handle: _ } => Poll::Pending,
            // TODO(https://fxbug.dev/135119) Handle multiple calls
            CallRequest::RequestActive { control_handle: _ } => match self.state {
                Some(CallState::IncomingRinging) => Poll::Ready(Some(CallOutput::ProcedureInput(
                    ProcedureInput::CommandFromHf(CommandFromHf::AnswerIncoming),
                ))),
                Some(CallState::TransferredToAg) => Poll::Ready(Some(CallOutput::ProcedureInput(
                    ProcedureInput::CommandFromHf(CommandFromHf::StartAudioConnection),
                ))),
                _ => {
                    warn!(
                        "Got Call Request {:?} for call in inappropriate state: {:?}",
                        call_request, self
                    );
                    Poll::Pending
                }
            },
            CallRequest::RequestTerminate { control_handle: _ } => self.maybe_hang_up(),
            CallRequest::RequestTransferAudio { control_handle: _ } => {
                Poll::Ready(Some(CallOutput::TransferCallToAg))
            }
            _ => panic!("Unexpected Call request {:?}", call_request),
        }
    }

    /// Generate a NextCall if possible. This is possible if the number, direction and
    /// state fields have been set and the request_stream field has not yet been
    /// set when creating a previous NextCall.
    ///
    /// Failure to generate a NextCall is indicated by the Err variant of a
    /// Result but is not a true error; it just means the information needed to
    /// generate a NextCall hasn't been provided yet or that a NextCall has
    /// already been crated for this Call.
    pub fn possibly_generate_next_call(&mut self) -> Result<NextCall, Error> {
        // TODO (https://fxbug.dev/135158) It's not clear we will always have a number for all calls, so handle,
        // that case.
        let result = match (
            self.state.is_some(),
            self.number.is_some(),
            self.direction.is_some(),
            !self.request_stream.is_some(),
        ) {
            (false, _, _, _) => Err(format_err!("Call {:?} does not yet have a state.", self)),
            (true, false, _, _) => Err(format_err!("Call {:?} does not yet have a number.", self)),
            (true, true, false, _) => {
                Err(format_err!("Call {:?} does not yet have a direction.", self))
            }
            (true, true, true, false) => Err(format_err!(
                "(Not an error) Call {:?} already has a request stream, indicating that this call has already been converted to a NextCall",
                self
            )),
            (true, true, true, true) => {
                let (client_end, server_end) = fidl::endpoints::create_endpoints::<CallMarker>();
                self.request_stream.set(server_end.into_stream());

                let number =
                    self.number.as_ref().expect("Number should be set.").to_non_at_string();
                let state = self.state.expect("State should be set.");
                let direction =
                    CallDirection::from(self.direction.expect("Direction should be set"));
                Ok(NextCall {
                    call: Some(client_end),
                    remote: Some(number),
                    state: Some(state),
                    direction: Some(direction),
                    ..Default::default()
                })
            }
        };

        result
    }
}

impl fmt::Debug for Call {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Call")
            .field("peer_id", &format!("{:}", self.peer_id))
            .field("call_index", &self.call_index)
            .field("state", &self.state)
            .field("number", &self.number)
            .field("direction", &self.direction)
            .field("request_stream.is_some()", &self.request_stream.is_some())
            .field("shutdown_state", &"<lock>")
            .finish_non_exhaustive()
    }
}

/// Stream of procedure inputs generated by converting the underlying Call protocol
/// FIDL request stream into the procedure input needed to start the procedure
/// that was requested.  This will also drive reporting state changes for a given
/// call on a WatchNextCall hanging get request.
impl Stream for Call {
    type Item = CallOutput;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let call_request = self.request_stream.poll_next_unpin(context);
        if let Poll::Ready(_) = call_request {
            info!(
                "Received call request {:?} for peer {:} for call {:?}.",
                call_request, self.peer_id, self.call_index
            );
        }

        let poll = match call_request {
            Poll::Pending => Poll::Pending, // Stream contained nothing, but has registered waker
            Poll::Ready(None) => Poll::Ready(None), // Stream was terminated
            Poll::Ready(Some(Err(error))) =>
            // FIDL error indicates sending a hang up command.
            {
                info!(
                    "FIDL error on Call request stream for call {:?} with peer {:}: {:?}",
                    self, self.peer_id, error
                );
                self.shutdown_state.lock().received_fidl_error();
                self.maybe_hang_up()
            }
            Poll::Ready(Some(Ok(CallRequest::WatchState { responder }))) => {
                self.handle_watch_state(responder);
                Poll::Pending
            }
            Poll::Ready(Some(Ok(CallRequest::SendDtmfCode { code: fidl_code, responder }))) => {
                let _result = responder.send(Ok(()));
                Poll::Ready(Some(CallOutput::ProcedureInput(ProcedureInput::CommandFromHf(
                    CommandFromHf::SendDtmfCode { code: fidl_code.into() },
                ))))
            }
            Poll::Ready(Some(Ok(state_update_request))) => {
                self.call_request_to_call_output(state_update_request)
            }
        };

        if poll.is_pending() {
            self.waker = Some(context.waker().clone());
        }

        // Respond to any outstanding WatchState requests if we can. This must
        // happen after the call to `handle_watch_state`, which enqueues the
        // responder.
        while let Poll::Ready(Some(result)) =
            self.watch_state_hanging_get_matcher.poll_next_unpin(context)
        {
            if let Err(err) = result {
                info!(
                    "FIDL error responding to WatchState for call {:?} with peer {:}: {:?}",
                    self.call_index, self.peer_id, err
                );
                return self.maybe_hang_up();
            }
        }

        poll
    }
}

type NextCallMatcher =
    OneToOneMatcher<NextCall, PeerHandlerWatchNextCallResponder, ResponderResult>;

/// This struct contains a list of `Call`s and methods to manipulate their
/// state.  Additionally, it acts as a stream of `CallOutput` by
/// multiplexing the underlying Call`s' streams.
pub struct Calls {
    peer_id: PeerId,
    call_list: CallList<Call>,
    sco_connected: bool,
    /// Calls for which a +CIEV(call = 0) has been received but which are still responding to a
    /// WatchCallState request.
    terminated_calls: VecDeque<Call>,
    /// Set when a new call is inserted to cause the `poll_next` implementation to yield QueryCalls next time it's called.
    query_calls: bool,
    watch_next_call_hanging_get_matcher: NextCallMatcher,
    waker: Option<Waker>,
}

fn respond_to_watch_next_call(
    call: NextCall,
    responder: PeerHandlerWatchNextCallResponder,
) -> Result<(), fidl::Error> {
    responder.send(call)
}

impl Calls {
    pub fn new(peer_id: PeerId) -> Self {
        let watch_next_call_hanging_get_matcher = OneToOneMatcher::new(respond_to_watch_next_call);
        Self {
            peer_id,
            call_list: CallList::default(),
            sco_connected: false,
            terminated_calls: VecDeque::new(),
            watch_next_call_hanging_get_matcher,
            query_calls: false,
            waker: None,
        }
    }

    fn awaken(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    pub fn insert_new_call(&mut self) -> anyhow::Result<CallIndex> {
        // If a new call is added we want to query for its direction and number.
        self.insert_new_call_inner(/* query_calls_afterwards */ true)
    }

    fn insert_new_call_inner(&mut self, query_calls_afterwards: bool) -> anyhow::Result<CallIndex> {
        // TODO(https://fxbug.dev/135119) Handle multiparty calls
        if self.call_list.len() > 0 {
            bail!(
                "Inserting new call for peer {:} when calls currently exist: {:?}",
                self.peer_id,
                self.call_list
            );
        }

        let call = Call::new(self.peer_id);

        let call_list_size = self.call_list.len();
        if call_list_size == MAX_NUM_CALLS {
            warn!(
                "Inserting when there are already {:} calls which is too many calls.",
                call_list_size
            );
            return Err(format_err!(
                "Inserting call when there are already {:} calls which is too many calls.",
                call_list_size
            ));
        }

        let call_index = self.call_list.insert(call);

        let call = self.call_list.get_mut(call_index);
        let call = call.expect("Call was just inserted and so must be present.");
        call.set_call_index(call_index);

        info!("Inserted call {:?} for peer {:}.", call, self.peer_id);

        // We can't have the state, number or direction here, so no need to
        // possibly_respond_to_watch_next_call

        // Maybe cause the stream to yield an enhanced call status
        // fetch procedure input to get number and direction.
        self.query_calls |= query_calls_afterwards;

        Ok(call_index)
    }

    pub fn set_call_state_by_indicator(&mut self, indicator: CallIndicator) {
        debug!(
            "Setting call state for peer {:} with indicator {:?} for call list {:?}",
            self.peer_id, indicator, self.call_list
        );

        // TODO(https://fxbug.dev/135119) Handle multiple calls.
        // In the future, this will need to find the oldest call that this indicator could apply
        // to and compute the state update it causes for that call.  The calls module in the AG
        // component has several methods to help with with that; these should be factored out into
        // the bt_hfp crate.

        // Calls are 1-indexed
        let call_index = 1;

        debug!(
            "Setting call state for peer {:} for call {:?} with indicator {:?}",
            self.peer_id,
            self.call_list.get(call_index),
            indicator
        );

        let call = match self.call_list.get_mut(call_index) {
            Some(call) => call,
            None => {
                error!(
                    "No call found for for peer {:} at index {:} while setting state with indicators {:?}",
                    self.peer_id, call_index, indicator
                );
                return;
            }
        };

        let terminated = call.set_state_by_indicator(indicator, self.sco_connected);
        self.possibly_respond_to_watch_next_call(call_index);

        if terminated {
            self.handle_state_terminated(call_index);
        }
    }

    fn handle_state_terminated(&mut self, call_index: CallIndex) {
        debug!("Removing call {:?} for peer {:}.", call_index, self.peer_id);
        let removed = self.call_list.remove(call_index).expect("Removed call should exist.");
        removed.shutdown_state.lock().received_call_none();
        self.terminated_calls.push_back(removed);
    }

    // TODO(https://fxbug.dev/135158) Handle setting phone numbers.
    #[allow(unused)]
    pub fn set_number_for_current_call(&mut self, number: Number) {
        // TODO(https://fxbug.dev/135119) Handle multiple calls
        let call_index = 1; // Calls are 1-indexed
        let call_option = self.call_list.get_mut(call_index);
        match call_option {
            Some(call) => {
                call.set_number(number);
                self.possibly_respond_to_watch_next_call(call_index)
            }
            None => warn!(
                "No call found for for peer {:} at index {:} while setting number to {:?}",
                self.peer_id, call_index, number
            ),
        }
    }

    pub fn set_queried_call_info(
        &mut self,
        index: CallIndex,
        direction: Direction,
        state: CallState,
        multiparty: bool,
        number: Option<Number>,
    ) -> anyhow::Result<()> {
        // Calls take the lowest available index.  Thus, if we have a call with
        // a given index, in a conformant peer all lower indices should be used
        // by calls.  To prevent malicious peers from sending us very high
        // indices and forcing us to allocate calls for all lower indices,
        // insert_new_call only allows a certain number of calls,
        // MAX_NUM_CALLS, at a time.
        while let None = self.call_list.get(index) {
            // Will err if too many calls are added
            // We've already queried call info, so don't do that.
            let _index = self.insert_new_call_inner(/* query_calls_afterwards */ false)?;
        }

        let call =
            self.call_list.get_mut(index).expect("Call was just inserted and so must be present.");

        call.set_queried_call_info(direction, state, multiparty, number);
        self.possibly_respond_to_watch_next_call(index);

        Ok(())
    }

    #[allow(unused)]
    pub fn handle_watch_next_call(&mut self, responder: PeerHandlerWatchNextCallResponder) {
        self.watch_next_call_hanging_get_matcher.enqueue_right(responder);
        // Make sure the stream is pumped to deliver any new calls to this WatchState responder.
        self.awaken();
    }

    fn possibly_respond_to_watch_next_call(&mut self, call_index: CallIndex) {
        let call_option = self.call_list.get_mut(call_index);
        let Some(call) = call_option else {
            error!("Found no call at index {call_index} when responding to WatchNextCall.");
            return;
        };
        let next_call_result = call.possibly_generate_next_call();
        match next_call_result {
            Ok(next_call) => {
                debug!(
                    "Enqueueing WatchNextCall response {:?} for peer {:}",
                    next_call, self.peer_id
                );
                self.watch_next_call_hanging_get_matcher.enqueue_left(next_call);
                // Make sure the stream is pumped to deliver this calls to any WatchState responder.
                self.awaken();
            }
            Err(err) => {
                // This isn't a real error but just indicates we don't have all the information we
                // need for the NextCall yet, or we have already sent one.
                debug!(
                    "Unable to generate WatchNextCall response for peer {:}: {:?}",
                    self.peer_id, err
                );
            }
        }
    }

    pub fn set_sco_connected(&mut self, sco_connected: bool) {
        self.sco_connected = sco_connected;

        // TODO(https://fxbug.dev/135119) This already handles multiple calls, but remove this log
        if self.call_list.len() > 1 {
            error!(
                "Setting SCO connected to {} for peer {:} when more than one call exists: {:?}",
                sco_connected, self.peer_id, self.call_list
            );
        }

        for (_index, call) in self.call_list.calls_mut().into_iter() {
            call.set_sco_connected(sco_connected);
        }
    }
}

/// Produces a single stream by selecting over all the streams for each call.
impl Stream for Calls {
    type Item = CallOutput;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Respond to any outstanding WatchNextCall requests if we can.
        while let Poll::Ready(Some(result)) =
            self.watch_next_call_hanging_get_matcher.poll_next_unpin(context)
        {
            if let Err(err) = result {
                warn!("Error responding to WatchNextCall for peer {}: {:?}", self.peer_id, err);
            }
        }

        for call in &mut self.terminated_calls {
            // Pump the stream for the terminated call.  This will cause the Terminated state to be
            // sent to the FIDL client if there is a WatchNextState hanging get outstanding.  Since
            // the peer believes the call is terminated, we can ignore any CallOutput yielded
            // by this stream caused by incoming FIDL requests.
            while let Poll::Ready(Some(_)) = call.poll_next_unpin(context) {}
        }

        // Now that we've run every terminated call, we can filter out those that have successfully reported
        // their state to the FIDL client or had an error.
        self.terminated_calls.retain(|call| !call.shutdown_state.lock().is_complete());

        if self.query_calls {
            self.query_calls = false;
            return Poll::Ready(Some(CallOutput::QueryCalls));
        }

        // TODO(http://fxbug.dev/135119) Handle multiple calls.
        let call_index = 1; // Calls are 1-indexed
        let call_option = self.call_list.get_mut(call_index);
        let call = match call_option {
            Some(call) => call,
            None => {
                self.waker = Some(context.waker().clone());
                return Poll::Pending;
            }
        };

        let poll = call.poll_next_unpin(context);

        if poll.is_pending() {
            self.waker = Some(context.waker().clone());
        }

        poll
    }
}

impl FusedStream for Calls {
    fn is_terminated(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod test {
    // TODO(https://https://fxbug.dev.dev/410610394) Add more tests.
    use super::*;

    use assert_matches::assert_matches;
    use fidl_fuchsia_bluetooth_hfp as fidl_hfp;
    use futures::future::{Either, FutureExt, select};

    static PEER_ID: PeerId = PeerId(1);

    #[fuchsia::test]
    async fn call_created_with_phone_number() {
        let mut calls = Calls::new(PEER_ID);
        calls.set_sco_connected(true);

        let (peer_handler_proxy, mut peer_handler_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_hfp::PeerHandlerMarker>();

        let watch_next_call_fut = peer_handler_proxy.watch_next_call();
        let watch_next_call_request_fut = peer_handler_request_stream.next();

        let (watch_next_call_request_result_option, watch_next_call_continue_fut) =
            match select(watch_next_call_fut, watch_next_call_request_fut)
                .now_or_never()
                .expect("Select hanging")
            {
                Either::Left(_) => panic!("WatchNextCall future terminated early."),
                Either::Right((req, wnc)) => (req, wnc),
            };
        let watch_next_call_request = watch_next_call_request_result_option
            .expect("Call request tream closed")
            .expect("FIDL error on CallRequestStream");

        let watch_next_call_responder = match watch_next_call_request {
            fidl_hfp::PeerHandlerRequest::WatchNextCall { responder } => responder,
            req => panic!("Unexpected PeerHandler request {req:?}."),
        };

        let call_index = calls.insert_new_call().expect("Insert new call");

        // Pump stream to respond to get the QueryCalls output
        let call_output_option = calls.next().now_or_never();
        // No calls have been returned to client yet with WatchNextCall
        assert_matches!(call_output_option, Some(Some(CallOutput::QueryCalls)));

        calls
            .set_queried_call_info(
                call_index,
                Direction::MobileOriginated,
                CallState::OutgoingAlerting,
                /* multiparty */ false,
                Some(Number::from_non_at_string("+1 212 555 0100").expect("valid test number")),
            )
            .expect("Set queried call info");

        calls.handle_watch_next_call(watch_next_call_responder);

        // Pump stream to respond to WatchNextCall
        let call_output_option = calls.next().now_or_never();
        // No new output
        assert_matches!(call_output_option, None);

        let next_call = watch_next_call_continue_fut
            .now_or_never()
            .expect("watch_next_call hanging")
            .expect("FIDL Error on watch_next_call");
        let call_proxy = next_call.call.expect("Missing client end").into_proxy();

        let watch_state_fut = call_proxy.watch_state();

        // Pump stream to respond to WatchState
        let call_output_option = calls.next().now_or_never();
        // No FIDL calls causing a procedure update have happened yet.
        assert_matches!(call_output_option, None);

        let state = watch_state_fut
            .now_or_never()
            .expect("watch_state hanging")
            .expect("FIDL error on watch_state");
        assert_eq!(state, CallState::OutgoingAlerting);

        // Tell Calls the call was was answered
        calls.set_call_state_by_indicator(CallIndicator::Call(call_indicators::Call::Some));

        let watch_state_fut = call_proxy.watch_state();
        // Pump stream to respond to WatchState
        let call_output_option = calls.next().now_or_never();
        // No FIDL calls causing a procedure update have happened yet.
        assert_matches!(call_output_option, None);

        let state = watch_state_fut
            .now_or_never()
            .expect("watch_state hanging")
            .expect("FIDL error on watch_state");
        assert_eq!(state, CallState::OngoingActive);

        // test transferring to AG
        call_proxy.request_transfer_audio().expect("Request transfer audio");

        let call_output_option = calls.next().now_or_never();
        assert_matches!(call_output_option, Some(Some(CallOutput::TransferCallToAg)));

        let watch_state_fut = call_proxy.watch_state();
        calls.set_sco_connected(false);

        // Pump stream to respond to WatchState
        let call_output_option = calls.next().now_or_never();
        // No FIDL calls causing a procedure update have happened yet.
        assert_matches!(call_output_option, None);

        let state = watch_state_fut
            .now_or_never()
            .expect("watch_state hanging")
            .expect("FIDL error on watch_state");
        assert_eq!(state, CallState::TransferredToAg);

        // Pass through hang up indicator
        call_proxy.request_terminate().expect("Request terminated");

        // Get hangup input
        let call_output_option = calls.next().now_or_never();
        assert_matches!(
            call_output_option,
            Some(Some(CallOutput::ProcedureInput(ProcedureInput::CommandFromHf(
                CommandFromHf::HangUpCall
            ))))
        );

        // Try again, but this time it shouldn't go through, since we've already requested a hangup
        call_proxy.request_terminate().expect("Request terminated");
        let call_output_option = calls.next().now_or_never();
        assert_matches!(call_output_option, None);

        // Hang up
        calls.set_call_state_by_indicator(CallIndicator::Call(call_indicators::Call::None));

        let watch_state_fut = call_proxy.watch_state();
        // Pump stream to respond to WatchState
        let _call_output_option = calls.next().now_or_never();
        let state = watch_state_fut
            .now_or_never()
            .expect("watch_state second call hanging")
            .expect("FIDL error on watch_state");
        assert_eq!(state, CallState::Terminated);
    }

    #[fuchsia::test]
    async fn call_created_without_phone_number_or_direction() {
        let mut calls = Calls::new(PEER_ID);

        let (peer_handler_proxy, mut peer_handler_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_hfp::PeerHandlerMarker>();

        let watch_next_call_fut = peer_handler_proxy.watch_next_call();
        let watch_next_call_request_fut = peer_handler_request_stream.next();

        let (watch_next_call_request_result_option, watch_next_call_continue_fut) =
            match select(watch_next_call_fut, watch_next_call_request_fut)
                .now_or_never()
                .expect("Select hanging")
            {
                Either::Left(_) => panic!("WatchNextCall future terminated early."),
                Either::Right((req, wnc)) => (req, wnc),
            };
        let watch_next_call_request = watch_next_call_request_result_option
            .expect("Call request tream closed")
            .expect("FIDL error on CallRequestStream");

        let watch_next_call_responder = match watch_next_call_request {
            fidl_hfp::PeerHandlerRequest::WatchNextCall { responder } => responder,
            req => panic!("Unexpected PeerHandler request {req:?}."),
        };

        let _call_index = calls.insert_new_call();

        calls.handle_watch_next_call(watch_next_call_responder);

        calls.set_call_state_by_indicator(CallIndicator::Call(call_indicators::Call::Some));

        // Pump stream to get the QueryCalls
        let call_output_option = calls.next().now_or_never();
        assert_matches!(call_output_option, Some(Some(CallOutput::QueryCalls)));

        // Pump stream to respond to WatchNextCall
        let call_output_option = calls.next().now_or_never();
        // No new call output
        assert_matches!(call_output_option, None);

        let next_call_hang = watch_next_call_continue_fut.now_or_never();
        // The NextcCall is never ready to be sent to clients.
        assert_matches!(next_call_hang, None);
    }

    #[fuchsia::test]
    async fn terminated_call_stream_none_does_not_infinite_loop() {
        let mut calls = Calls::new(PEER_ID);

        // Insert a new call and populate its state and number.
        let call_index = calls.insert_new_call().expect("Insert new call");
        calls
            .set_queried_call_info(
                call_index,
                Direction::MobileOriginated,
                CallState::OutgoingAlerting,
                /* multiparty */ false,
                Some(Number::from_non_at_string("+1 212 555 0100").expect("valid test number")),
            )
            .expect("Set queried call info");

        // Establish a hanging get request for the next call from the
        // peer handler client.
        let (peer_handler_proxy, mut peer_handler_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_hfp::PeerHandlerMarker>();
        let watch_next_call_fut = peer_handler_proxy.watch_next_call();
        let watch_next_call_request_fut = peer_handler_request_stream.next();
        let (watch_next_call_request_result_option, watch_next_call_continue_fut) =
            match select(watch_next_call_fut, watch_next_call_request_fut)
                .now_or_never()
                .expect("Select hanging")
            {
                Either::Left(_) => panic!("WatchNextCall future terminated early."),
                Either::Right((req, wnc)) => (req, wnc),
            };
        let watch_next_call_request = watch_next_call_request_result_option
            .expect("Call request stream closed")
            .expect("FIDL error on CallRequestStream");
        let watch_next_call_responder = match watch_next_call_request {
            fidl_hfp::PeerHandlerRequest::WatchNextCall { responder } => responder,
            req => panic!("Unexpected PeerHandler request {req:?}."),
        };
        calls.handle_watch_next_call(watch_next_call_responder);

        // Pump the calls stream once to fulfill the hanging get request
        // and attach the request stream to the call.
        let _ = calls.next().now_or_never();

        let next_call = watch_next_call_continue_fut
            .now_or_never()
            .expect("watch_next_call hanging")
            .expect("FIDL Error on watch_next_call");
        let call_proxy = next_call.call.expect("Missing client end").into_proxy();

        // Drop the client proxy handle so the underlying request stream
        // closes and stops yielding items.
        drop(call_proxy);

        // Transition the call state to terminated so it is moved to the
        // terminated calls queue.
        calls.set_call_state_by_indicator(CallIndicator::Call(call_indicators::Call::None));

        // Polling the calls stream should not enter an infinite loop, and
        // no stream items should be yielded.
        let call_output = calls.next().now_or_never();
        assert_matches!(call_output, None);
    }
}
