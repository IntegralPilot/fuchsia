// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};

use fidl_fuchsia_update::{ListenerRequest, ListenerRequestStream, NotifierProxy};
use fidl_fuchsia_update_ext::State;
use futures::channel::mpsc;
use futures::{SinkExt as _, StreamExt as _, TryStreamExt as _};
use log::warn;

pub(crate) struct CompletionResponder {
    state: CompletionResponderState,
    receiver: mpsc::UnboundedReceiver<Message>,
}

enum CompletionResponderState {
    Waiting { notifiers: Vec<NotifierProxy> },
    Satisfied,
}

pub(crate) struct CompletionResponderStateReactor(mpsc::UnboundedSender<Message>);

#[derive(Clone)]
pub(crate) struct CompletionResponderFidlServer(mpsc::UnboundedSender<Message>);

enum Message {
    Completed,
    NewNotifier(NotifierProxy),
}

impl CompletionResponderFidlServer {
    pub async fn serve_completion_responses(
        mut self,
        mut request_stream: ListenerRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) =
            request_stream.try_next().await.context("extracting request from stream")?
        {
            match request {
                ListenerRequest::NotifyOnFirstUpdateCheck { payload, control_handle } => {
                    let notifier = match payload.notifier {
                        Some(notifier) => notifier,
                        None => {
                            // This is a required field.
                            control_handle.shutdown_with_epitaph(zx::Status::INVALID_ARGS);
                            return Ok(());
                        }
                    };
                    if let Err(e) = self.0.send(Message::NewNotifier(notifier.into_proxy())).await {
                        warn!(e:?; "Internal bug; this send should always succeed");
                    }
                }
                ListenerRequest::_UnknownMethod { .. } => {}
            }
        }
        Ok(())
    }
}

impl CompletionResponderStateReactor {
    pub async fn react_to(&mut self, state: &State) {
        // If a software update has completed, with or without error, and no reboot is needed,
        // then send a Completed message to the CompletionResponder message-handler.
        // Otherwise, just return.
        match state {
            State::ErrorCheckingForUpdate
            | State::NoUpdateAvailable
            | State::InstallationDeferredByPolicy(_) => {
                // Internal stream - discard theoretical errors
                if let Err(e) = self.0.send(Message::Completed).await {
                    warn!(e:?; "Internal bug; this send should always succeed");
                }
            }
            State::InstallingUpdate(_)
            | State::WaitingForReboot(_)
            | State::InstallationError(_)
            | State::CheckingForUpdates => {}
        }
    }

    // A non-functional struct for use in tests that don't care about completion responding.
    #[cfg(test)]
    pub fn test_stub() -> Self {
        let (send, _) = mpsc::unbounded();
        Self(send)
    }
}

impl CompletionResponder {
    pub fn build() -> (CompletionResponderStateReactor, CompletionResponderFidlServer, Self) {
        let (sender, receiver) = mpsc::unbounded();
        (
            CompletionResponderStateReactor(sender.clone()),
            CompletionResponderFidlServer(sender),
            Self { state: CompletionResponderState::Waiting { notifiers: Vec::new() }, receiver },
        )
    }

    pub async fn wait_for_messages(mut self) {
        while let Some(message) = self.receiver.next().await {
            match message {
                Message::Completed => self.state.become_satisfied(),
                Message::NewNotifier(notifier) => self.state.notify_when_appropriate(notifier),
            }
        }
        warn!("CompletionResponder stream shouldn't close");
    }
}

impl CompletionResponderState {
    fn become_satisfied(&mut self) {
        if let CompletionResponderState::Waiting { notifiers } = self {
            for notifier in notifiers.drain(..) {
                if let Err(e) = notifier.notify() {
                    warn!(
                        "Received FIDL error notifying client of software update completion: {e:?}"
                    );
                }
            }
            *self = CompletionResponderState::Satisfied;
        }
    }

    fn notify_when_appropriate(&mut self, notifier: NotifierProxy) {
        match self {
            CompletionResponderState::Waiting { notifiers } => notifiers.push(notifier),
            CompletionResponderState::Satisfied => {
                if let Err(e) = notifier.notify() {
                    warn!(
                        "Received FIDL error notifying client of software update completion: {e:?}"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CompletionResponder;
    use assert_matches::assert_matches;
    use core::task::Poll;
    use fidl_fuchsia_update::{
        ListenerMarker, ListenerNotifyOnFirstUpdateCheckRequest, ListenerProxy, NotifierMarker,
        NotifierRequest,
    };
    use fidl_fuchsia_update_ext::{
        InstallationDeferredData, InstallationErrorData, InstallingData,
    };
    use fuchsia_async::{self as fasync, TestExecutor};
    use futures::pin_mut;
    use test_case::test_case;

    // Don't drop tasks until the end of the test.
    fn setup_responder()
    -> (CompletionResponderStateReactor, CompletionResponderFidlServer, fasync::Task<()>) {
        let (reactor, server, waiter) = CompletionResponder::build();
        let task = fasync::Task::local(waiter.wait_for_messages());
        (reactor, server, task)
    }

    fn spawn_server(server: CompletionResponderFidlServer) -> ListenerProxy {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<ListenerMarker>();
        fasync::Task::local(async move {
            server.serve_completion_responses(stream).await.unwrap();
        })
        .detach();
        proxy
    }

    enum TestExpect {
        /// Listener is waiting for the correct state.
        Wait,
        /// Listener has notified clients.
        Notify,
    }

    #[test_case(
        State::InstallationDeferredByPolicy(InstallationDeferredData::default()),
        TestExpect::Notify
    )]
    #[test_case(State::NoUpdateAvailable, TestExpect::Notify)]
    #[test_case(State::ErrorCheckingForUpdate, TestExpect::Notify)]
    #[test_case(State::CheckingForUpdates, TestExpect::Wait)]
    #[test_case(State::InstallingUpdate(InstallingData::default()), TestExpect::Wait)]
    #[test_case(State::WaitingForReboot(InstallingData::default()), TestExpect::Wait)]
    #[test_case(State::InstallationError(InstallationErrorData::default()), TestExpect::Wait)]
    #[fuchsia::test(allow_stalls = false)]
    async fn test_whether_state_satisfies(state: State, expect: TestExpect) {
        let (mut reactor, server, _task) = setup_responder();
        reactor.react_to(&state).await;
        let proxy = spawn_server(server);

        let (notifier_client, notifier_server) =
            fidl::endpoints::create_endpoints::<NotifierMarker>();
        assert_matches!(
            proxy.notify_on_first_update_check(ListenerNotifyOnFirstUpdateCheckRequest {
                notifier: Some(notifier_client),
                ..Default::default()
            }),
            Ok(())
        );

        let mut notifier_stream = notifier_server.into_stream();
        let notify_fut = notifier_stream.try_next();
        pin_mut!(notify_fut);
        let notify_result = TestExecutor::poll_until_stalled(&mut notify_fut).await;

        match expect {
            TestExpect::Notify => {
                assert_matches!(
                    notify_result,
                    Poll::Ready(Ok(Some(NotifierRequest::Notify { .. }))),
                    "Notifier didn't receive request"
                );
            }
            TestExpect::Wait => {
                assert_matches!(
                    notify_result,
                    Poll::Pending,
                    "Notifier received unexpected request"
                );
            }
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn correct_response_sequencing() {
        let (mut reactor, server, _task) = setup_responder();
        let proxy1 = spawn_server(server.clone());
        let proxy2 = spawn_server(server.clone());

        let (notifier_client_1, notifier_server_1) =
            fidl::endpoints::create_endpoints::<NotifierMarker>();
        assert_matches!(
            proxy1.notify_on_first_update_check(ListenerNotifyOnFirstUpdateCheckRequest {
                notifier: Some(notifier_client_1),
                ..Default::default()
            }),
            Ok(())
        );
        let mut notifier_stream_1 = notifier_server_1.into_stream();
        let notifier1 = notifier_stream_1.try_next();
        pin_mut!(notifier1);

        let (notifier_client_2, notifier_server_2) =
            fidl::endpoints::create_endpoints::<NotifierMarker>();
        assert_matches!(
            proxy2.notify_on_first_update_check(ListenerNotifyOnFirstUpdateCheckRequest {
                notifier: Some(notifier_client_2),
                ..Default::default()
            }),
            Ok(())
        );
        let mut notifier_stream_2 = notifier_server_2.into_stream();
        let notifier2 = notifier_stream_2.try_next();
        pin_mut!(notifier2);

        reactor.react_to(&State::CheckingForUpdates).await;
        assert_matches!(TestExecutor::poll_until_stalled(&mut notifier1).await, Poll::Pending);
        assert_matches!(TestExecutor::poll_until_stalled(&mut notifier2).await, Poll::Pending);

        reactor.react_to(&State::NoUpdateAvailable).await;
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut notifier1).await,
            Poll::Ready(Ok(Some(NotifierRequest::Notify { .. })))
        );
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut notifier2).await,
            Poll::Ready(Ok(Some(NotifierRequest::Notify { .. })))
        );

        // Clients that start to wait after the happy state is received should return immediately.
        let proxy3 = spawn_server(server);
        let (notifier_client_3, notifier_server_3) =
            fidl::endpoints::create_endpoints::<NotifierMarker>();
        assert_matches!(
            proxy3.notify_on_first_update_check(ListenerNotifyOnFirstUpdateCheckRequest {
                notifier: Some(notifier_client_3),
                ..Default::default()
            }),
            Ok(())
        );
        let mut notifier_stream_3 = notifier_server_3.into_stream();
        assert_matches!(
            notifier_stream_3.try_next().await,
            Ok(Some(NotifierRequest::Notify { .. }))
        );
    }
}
