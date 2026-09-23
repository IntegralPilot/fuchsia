// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use futures::lock::Mutex;
use {fidl_fuchsia_example_power as fexample, fuchsia_trace as ftrace};

/// Client components should use |SequenceClient| to automatically manage
/// interaction with server components. Clients should use |process_message(s)|
/// to tell |SequenceClient| when they've processed a message.
/// |SequenceClient| will then automatically discard batons at the appropriate
/// times.
pub struct SequenceClient {
    baton_source: fexample::MessageSourceProxy,
    baton: Mutex<Option<LeaseBaton>>,
    msg_index: Mutex<u64>,
}

pub enum BatonError {
    InvalidBatonMessage,
    Internal,
}

pub struct LeaseBaton {
    pub msg_index: u64,
    pub lease: zx::EventPair,
}

impl TryFrom<fexample::LeaseBaton> for LeaseBaton {
    type Error = BatonError;

    fn try_from(value: fexample::LeaseBaton) -> Result<Self, Self::Error> {
        match (value.lease, value.msg_index) {
            (Some(lease), Some(msg_index)) => Ok(Self { msg_index, lease }),
            _ => Err(BatonError::InvalidBatonMessage),
        }
    }
}

impl SequenceClient {
    /// Creates a new |SequenceClient|, but does not start its management.
    /// Clients should call |run| to start baton management.
    pub fn new(message_source: fexample::MessageSourceProxy) -> Self {
        Self { baton_source: message_source, baton: Mutex::new(None), msg_index: Mutex::new(0) }
    }

    /// Start baton management. This function will not return until the
    /// |MessageSourceProxy| passed to |new| closes, returns an error when
    /// read, or receives an invalid baton message.
    pub async fn run(&self) -> Result<(), BatonError> {
        loop {
            let baton_result = self.baton_source.receive_baton().await;

            if let Err(e) = baton_result {
                if e.is_closed() {
                    break;
                } else {
                    return Err(BatonError::Internal);
                }
            }

            ftrace::instant!(crate::TRACE_CATEGORY, c"receive-baton", ftrace::Scope::Process);
            let baton = baton_result.unwrap();
            {
                let new_baton: LeaseBaton = baton.try_into()?;

                let current_index = self.msg_index.lock().await;
                if *current_index < new_baton.msg_index {
                    let mut current_baton = self.baton.lock().await;
                    *current_baton = Some(new_baton);
                }
            }
        }
        Ok(())
    }

    /// Tell |SequenceClient| that a message was processed. If a baton is held
    /// corresponding to this message, it is returned. Likely the only valid
    /// reason for the caller to hold on to the baton is to pass it to it's
    /// client(s).
    pub async fn process_message(&self) -> Option<LeaseBaton> {
        self.process_messages(1).await
    }

    /// Tell |SequenceClient| that |message_count| messages were processed. If
    /// a baton is held corresponding to one of the messages, it is returned.
    /// Likely the only valid reason for the caller to hold on to the baton is
    /// to pass the baton to it's client(s).
    pub async fn process_messages(&self, message_count: u64) -> Option<LeaseBaton> {
        ftrace::duration!(crate::TRACE_CATEGORY, c"process-message", "count" => message_count);
        let mut current_index = self.msg_index.lock().await;
        *current_index += message_count;
        let mut baton_ref = self.baton.lock().await;

        // If we have a baton check to see if we should return it to the caller
        if let Some(baton) = baton_ref.take() {
            let baton_index = baton.msg_index;

            // We've seen the message this baton corresponds to, so take it and
            // return to the caller so they can do with it as they will.
            if baton_index <= *current_index {
                ftrace::instant!(crate::TRACE_CATEGORY, c"dropping-baton", ftrace::Scope::Process);
                return Some(baton);
            }
            *baton_ref = Some(baton);
        }
        None
    }

    pub async fn get_receieved_count(&self) -> u64 {
        *self.msg_index.lock().await
    }
}
