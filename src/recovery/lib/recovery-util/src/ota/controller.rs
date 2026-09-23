// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ota::state_machine::{Event, EventProcessor, StateHandler};
use fuchsia_async::{self as fasync, Task};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
#[cfg(test)]
use mockall::automock;

#[derive(Clone)]
pub struct EventSender {
    sender: mpsc::Sender<Event>,
}

impl EventSender {
    pub fn new(sender: mpsc::Sender<Event>) -> Self {
        Self { sender }
    }
}

#[cfg_attr(test, automock)]
pub trait SendEvent {
    fn send(&mut self, event: Event);
}

impl SendEvent for EventSender {
    fn send(&mut self, event: Event) {
        let mut sender = self.sender.clone();
        let event_clone = event.clone();
        fasync::Task::local(async move {
            if let Err(error) = sender.send(event).await {
                eprintln!("Failed to send event {:?}: {}", event_clone, error);
            }
        })
        .detach();
    }
    // TODO(b/255587508): we have some idea to improve this, such as using a general function
    // send_metric. It might require a mapping a metric type to a logging function.
}

#[cfg_attr(test, automock)]
pub trait Controller {
    fn add_event_observer(&mut self, sender: mpsc::Sender<Event>);
    fn add_state_handler(&mut self, handler: Box<dyn StateHandler>);
    fn get_event_sender(&self) -> EventSender;
    fn start(&mut self, state_machine: Box<dyn EventProcessor>);
}

pub struct ControllerImpl {
    sender: mpsc::Sender<Event>,
    receiver: Option<mpsc::Receiver<Event>>,
    state_handlers: Option<Vec<Box<dyn StateHandler>>>,
    event_observers: Option<Vec<mpsc::Sender<Event>>>,
}

impl ControllerImpl {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel::<Event>(10);
        Self {
            sender,
            receiver: Some(receiver),
            state_handlers: Some(Vec::new()),
            event_observers: Some(Vec::new()),
        }
    }
}

impl Controller for ControllerImpl {
    fn add_event_observer(&mut self, observer: mpsc::Sender<Event>) {
        if let Some(ref mut event_observers) = self.event_observers {
            event_observers.push(observer);
        }
    }

    fn add_state_handler(&mut self, handler: Box<dyn StateHandler>) {
        if let Some(ref mut state_handlers) = self.state_handlers {
            state_handlers.push(handler);
        }
    }

    fn get_event_sender(&self) -> EventSender {
        EventSender::new(self.sender.clone())
    }

    fn start(&mut self, mut state_machine: Box<dyn EventProcessor>) {
        let mut receiver = self.receiver.take().unwrap();
        let mut state_handlers = self.state_handlers.take().unwrap();
        let mut event_observers = self.event_observers.take().unwrap();
        let main_loop = async move {
            loop {
                match receiver.next().await {
                    Some(event) => {
                        let event_clone = event.clone();
                        for sender in event_observers.iter_mut() {
                            if let Err(e) = sender.send(event.clone()).await {
                                eprintln!("Error sending observer event: {:#?}", e);
                            }
                        }
                        let state = state_machine.process_event(event);
                        println!("Controller: event {:?} -> state {:?}", event_clone, state);
                        if let Some(state) = &state {
                            for state_handler in state_handlers.iter_mut() {
                                state_handler.handle_state(state.clone());
                            }
                        }
                    }
                    None => {
                        eprintln!("event reader returned: None")
                    }
                }
            }
        };
        Task::local(main_loop).detach();
    }
}

#[cfg(test)]
mod test {
    use crate::ota::controller::{Controller, ControllerImpl, SendEvent};
    use crate::ota::state_machine::{
        Event, MockEventProcessor, MockStateHandler, State, StateHandler,
    };
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use futures::channel::mpsc;
    use mockall::predicate::*;

    #[test]
    fn send_states() {
        let mut exec = fasync::TestExecutor::new();
        let mut state_machine = MockEventProcessor::new();
        state_machine
            .expect_process_event()
            .with(eq(Event::Cancel))
            .return_const(Some(State::Home));

        let mut state_handler = MockStateHandler::new();
        state_handler.expect_handle_state().with(eq(State::Home)).times(1).return_const(());
        let state_handler: Box<dyn StateHandler> = Box::new(state_handler);

        let mut controller = ControllerImpl::new();
        controller.add_state_handler(state_handler);
        let mut event_sender: Box<dyn SendEvent> = Box::new(controller.get_event_sender());
        controller.start(Box::new(state_machine));
        event_sender.send(Event::Cancel);

        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
    }

    #[fuchsia::test]
    async fn test_observe_events() {
        let (sender, mut receiver) = mpsc::channel::<Event>(5);
        let mut state_machine = MockEventProcessor::new();
        state_machine.expect_process_event().times(3).return_const(Some(State::Home));
        let mut controller = ControllerImpl::new();
        controller.add_event_observer(sender);
        let mut event_sender: Box<dyn SendEvent> = Box::new(controller.get_event_sender());

        controller.start(Box::new(state_machine));
        event_sender.send(Event::Error("123".to_string()));
        event_sender.send(Event::DebugLog("test".to_string()));
        event_sender.send(Event::WiFiConnected);

        assert_matches!(receiver.next().await.unwrap(), Event::Error(_));
        assert_matches!(receiver.next().await.unwrap(), Event::DebugLog(_));
        assert_matches!(receiver.next().await.unwrap(), Event::WiFiConnected);
    }
}
