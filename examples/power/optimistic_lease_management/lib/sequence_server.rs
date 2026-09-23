// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use async_trait::async_trait;
use futures::StreamExt;
use futures::lock::Mutex;
use log::warn;
use std::sync::Arc;
use {
    fidl_fuchsia_example_power as fexample, fidl_fuchsia_power_system as fsag,
    fuchsia_trace as ftrace,
};

const TRACE_NAME_FLUSH: &std::ffi::CStr = c"flush-count-dirty";

/// You probably don't want to create one of these directly, but instead use
/// |SequenceServer| to create one for you and then get a reference to it from
/// |SequenceServer.get_message_tracker|.
///
/// Server components can use this to help manage sending batons to clients
/// at appropriate times. Server components should call |message_sent| when
/// they send a message to a client. Servers should call |set_requester|
/// whenever server receives a hanging-GET for a baton from a client.
pub struct MessageSendTracker {
    sent_messages: u64,
    // The value of |sent_messages| when we last passed a baton
    last_baton_pass: u64,
    baton_request: Option<fexample::MessageSourceReceiveBatonResponder>,
    sag: fsag::ActivityGovernorProxy,
    baton: Option<zx::EventPair>,
    resumed: bool,
    terminated: bool,
}

#[derive(Debug)]
pub enum MessageTrackerError {
    Terminated,
}

impl MessageSendTracker {
    pub async fn new(sag: fsag::ActivityGovernorProxy) -> Self {
        let me = Self {
            sent_messages: 0,
            last_baton_pass: 0,
            baton_request: None,
            sag,
            baton: None,
            resumed: false,
            terminated: false,
        };
        me
    }

    /// Inform the tracker a message was sent, equivalent to |messages_sent(1)|.
    /// If the system is suspended, a wake lease is taken to guarantee the
    /// message can be processed. This is the behavior we probably want if we
    /// this message came from a waking interrupt. This means after this
    /// function returns, the caller can safely ack an interrupt related to
    /// this event.
    pub async fn message_sent(&mut self) -> Result<(), MessageTrackerError> {
        self.messages_sent(1).await
    }

    /// Inform the tracker that |message_count| messages were sent. If the
    /// system is suspended, a wake lease is taken to guarantee the message
    /// can be processed. This is the behavior we probably want if we this
    /// message came from a waking interrupt. This means after this function
    /// returns, the caller can safely ack an interrupt related to this event.
    pub async fn messages_sent(&mut self, message_count: u64) -> Result<(), MessageTrackerError> {
        ftrace::instant!(
            crate::TRACE_CATEGORY,
            c"messages_sent",
            ftrace::Scope::Process,
            "count" => message_count
        );

        if self.terminated {
            return Err(MessageTrackerError::Terminated);
        }

        self.sent_messages += message_count;

        // If we're not resumed we want to resume the system so we can make
        // sure the message will be processed.
        if !self.resumed {
            ftrace::instant!(
                crate::TRACE_CATEGORY,
                c"send-during-suspension",
                ftrace::Scope::Process
            );
            self.flush().await;
            // Flush acquires a wake lease, which guarantees we are or will be
            // resumed at some point, mark the state as such.
            self.resumed();
        }
        Ok(())
    }

    /// Deposits a request for a baton that a client made. This is a
    /// hanging-GET-style communication pattern and so the client receives a
    /// response the next time a flush is triggered. If a flush was triggered
    /// before |set_requester| was called, the baton is sent now based on the
    /// current message index.
    pub fn set_requester(&mut self, requester: fexample::MessageSourceReceiveBatonResponder) {
        {
            // Drop any previous requester
            let _previous_requester = self.baton_request.take();
        }

        if let Some(baton) = self.baton.take() {
            let id = self.sent_messages;
            self.send_baton(id, requester, baton);
        } else {
            self.baton_request = Some(requester);
        }
    }

    pub fn suspended(&mut self) {
        self.resumed = false;
    }

    pub fn resumed(&mut self) {
        self.resumed = true;
    }

    fn terminated(&mut self) {
        self.terminated = true;
    }

    pub fn get_message_count(&self) -> u64 {
        self.sent_messages
    }

    fn send_baton(
        &mut self,
        msg_id: u64,
        request: fexample::MessageSourceReceiveBatonResponder,
        baton: zx::EventPair,
    ) {
        request
            .send(fexample::LeaseBaton {
                lease: Some(baton),
                msg_index: Some(msg_id),
                ..Default::default()
            })
            .expect("send failed");

        ftrace::duration_end!(crate::TRACE_CATEGORY, TRACE_NAME_FLUSH, "msg_id" => msg_id);
        self.last_baton_pass = msg_id;
    }

    /// Triggers a response the previous request passed to |set_requester|, if
    /// any, when |message(s)_sent| has been called since the previous time a
    /// response was set to a requester. If there is no waiting request for
    /// a baton, the baton is created, but not passed. It will be passed when
    /// |seq_requester| is called next.
    async fn flush(&mut self) {
        if self.last_baton_pass < self.sent_messages {
            let curr_offset = self.sent_messages;

            ftrace::duration_begin!(
                crate::TRACE_CATEGORY,
                TRACE_NAME_FLUSH,
                "msg_id" => curr_offset,
                "baton_delta" => self.sent_messages - self.last_baton_pass
            );

            let baton = self
                .sag
                .acquire_wake_lease("optimistic-lease-baton")
                .await
                .expect("FIDL failed")
                .expect("SAG returned error");

            // If there is a hanging-GET for a baton, return the send the baton
            // immediately, otherwise store it for later.
            if let Some(req) = self.baton_request.take() {
                self.send_baton(curr_offset, req, baton);
            } else {
                self.baton = Some(baton);
                return;
            }
        }
    }
}

/// Server components should use class to guarantee clients see all messages
/// sent by the server prior to system suspension. After creating a
/// |SequenceServer| server components should call |manage| to start this
/// management.
///
/// Then server components should use the |MessageSendTracker| returned from
/// |get_message_tracker| to record when messages are sent and deposit
/// hanging-GET requests for batons. Batons are sent to the client whenever the
/// system starts to suspend (as indicated by a SuspendStarted callback from
/// SystemActivityGovernor) *AND* messages count of sent messages is greater
/// than the last time a baton was sent to a the client *AND* a hanging-GET
/// request is pending.
pub struct SequenceServer {
    baton_sender: Arc<Mutex<MessageSendTracker>>,
    flusher: Option<crate::flush_trigger::FlushTrigger>,
    sag: fsag::ActivityGovernorProxy,
}

#[async_trait]
impl crate::flush_trigger::FlushListener for SequenceServer {
    async fn flush(&self) {
        self.baton_sender.lock().await.flush().await;
    }
}

impl SequenceServer {
    /// Creates a SequenceServer, but does *not* kick off its logic. |manage|
    /// MUST be called to start monitoring for suspend and managing baton hand-
    /// offs.
    pub async fn new(sag: fsag::ActivityGovernorProxy) -> Self {
        let flusher = Some(crate::flush_trigger::FlushTrigger::new(sag.clone()));
        let baton_sender = Arc::new(Mutex::new(MessageSendTracker::new(sag.clone()).await));
        Self { flusher, baton_sender, sag }
    }

    /// Returns a future which manages baton passing and a reference to the
    /// |MessageSendTracker| clients use to report message sends. The returned
    /// future *must* be polled for as long as batons need to be delivered. The
    /// future returns, yielding the |SequenceServer| if the channel to
    /// ActivityGovernor passed to |new| closes.
    ///
    /// To stop managing batons, simply drop the future. The
    /// |MessageSendTracker| should not be used after the future returns.
    pub fn manage(
        self,
    ) -> (Arc<Mutex<MessageSendTracker>>, impl futures::Future<Output = Result<Self, fidl::Error>>)
    {
        let tracker = self.baton_sender.clone();
        let fut = async move {
            let mut sequence_server = self;
            if let None = sequence_server.flusher {
                warn!("No flusher available, aborting manage");
                return Ok(sequence_server);
            }

            // Take the flush trigger because we can't borrow sequence_server
            // twice to run the two futures.
            let flusher = sequence_server.flusher.take().unwrap();

            // Await the two futures for what we expect to be effectively
            // forever.
            let results = futures::future::join(
                sequence_server.watch_system_state(),
                flusher.run(&sequence_server),
            )
            .await;

            // Both futures return errors only if we can't talk to SAG so probably if
            // one has an error the other will have to same or would have the same
            // error soon.
            if let Err(e) = results.1 {
                return Err(e);
            }

            if let Err(e) = results.0 {
                return Err(e);
            }

            sequence_server.flusher = Some(flusher);
            sequence_server.baton_sender.lock().await.terminated();
            Ok(sequence_server)
        };
        (tracker, fut)
    }

    /// Watch the suspend/resume state of the system and
    async fn watch_system_state(&self) -> Result<(), fidl::Error> {
        let (client, server) = fidl::endpoints::create_endpoints::<fsag::SuspendBlockerMarker>();

        let registration_lease = self
            .sag
            .register_suspend_blocker(fsag::ActivityGovernorRegisterSuspendBlockerRequest {
                suspend_blocker: Some(client),
                name: Some("suspend_watcher".into()),
                ..Default::default()
            })
            .await?
            .expect("error registering suspend blocker");
        drop(registration_lease);

        let mut request_stream = server.into_stream();
        while let Some(req) = request_stream.next().await {
            match req {
                Ok(fsag::SuspendBlockerRequest::BeforeSuspend { responder }) => {
                    ftrace::instant!(crate::TRACE_CATEGORY, c"suspended", ftrace::Scope::Process);
                    self.baton_sender.lock().await.suspended();
                    let _ = responder.send();
                }
                Ok(fsag::SuspendBlockerRequest::AfterResume { responder }) => {
                    ftrace::instant!(crate::TRACE_CATEGORY, c"resumed", ftrace::Scope::Process);
                    self.baton_sender.lock().await.resumed();
                    let _ = responder.send();
                }
                Ok(fsag::SuspendBlockerRequest::_UnknownMethod { .. }) => {
                    warn!("unrecognized listener method, ignoring");
                }
                Err(_) => {
                    warn!("Error receiving next items from stream");
                }
            }
        }
        Ok(())
    }
}
