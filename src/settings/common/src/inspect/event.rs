// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::listener_logger::ListenerInspectLogger;
use crate::service_context::ExternalServiceEvent;
use anyhow::{Error, anyhow};
use futures::channel::mpsc::UnboundedSender;
use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

#[derive(Clone)]
pub enum Direction {
    Request(String),
    Response(String, ResponseType),
}

#[derive(Clone)]
pub struct UsageEvent {
    pub setting: &'static str,
    pub request_type: RequestType,
    pub direction: Direction,
    pub id: u64,
}

pub trait Nameable {
    const NAME: &str;
}

#[derive(Clone)]
pub struct UsagePublisher<T> {
    id_gen: Rc<Cell<u64>>,
    inspect_tx: UnboundedSender<UsageEvent>,
    listener_logger: Rc<ListenerInspectLogger>,
    _phantom: PhantomData<T>,
}

impl<T> UsagePublisher<T> {
    pub fn new(
        inspect_tx: UnboundedSender<UsageEvent>,
        listener_logger: Rc<ListenerInspectLogger>,
    ) -> Self {
        Self { id_gen: Rc::new(Cell::new(0)), inspect_tx, listener_logger, _phantom: PhantomData }
    }
}

impl<T> UsagePublisher<T>
where
    T: Nameable,
{
    pub fn request(&self, request: String, request_type: RequestType) -> UsageResponsePublisher<T> {
        let id = self.id_gen.get();
        self.id_gen.set(id.wrapping_add(1));
        let _ = self.inspect_tx.unbounded_send(UsageEvent {
            setting: T::NAME,
            request_type,
            direction: Direction::Request(request),
            id,
        });
        if let RequestType::Get = request_type {
            self.listener_logger.add_listener(T::NAME.into());
        }

        UsageResponsePublisher {
            id,
            request_type,
            inspect_tx: self.inspect_tx.clone(),
            listener_logger: Rc::clone(&self.listener_logger),
            sent: false,
            _phantom: PhantomData,
        }
    }
}

#[derive(Clone)]
pub struct UsageResponsePublisher<T> {
    id: u64,
    request_type: RequestType,
    inspect_tx: UnboundedSender<UsageEvent>,
    listener_logger: Rc<ListenerInspectLogger>,
    sent: bool,
    _phantom: PhantomData<T>,
}

impl<T> UsageResponsePublisher<T>
where
    T: Nameable,
{
    pub fn respond(mut self, response: String, response_type: ResponseType) {
        let _ = self.inspect_tx.unbounded_send(UsageEvent {
            setting: T::NAME,
            request_type: self.request_type,
            direction: Direction::Response(response, response_type),
            id: self.id,
        });
        if let RequestType::Get = self.request_type {
            self.listener_logger.remove_listener(T::NAME.into());
        }
        self.sent = true;
    }
}

impl<T> Drop for UsageResponsePublisher<T> {
    fn drop(&mut self) {
        if !self.sent {
            log::error!("UsageResponsePublisher dropped without sending response");
        }
    }
}

/// A wrapper for hanging get observers that ensures the `UsageResponsePublisher`
/// is responded to even if the observer is dropped (e.g., due to client disconnect).
pub struct HangingGetObserver<T, R>
where
    T: Nameable,
{
    inner: Option<(UsageResponsePublisher<T>, R)>,
}

impl<T, R> HangingGetObserver<T, R>
where
    T: Nameable,
{
    pub fn new(publisher: UsageResponsePublisher<T>, responder: R) -> Self {
        Self { inner: Some((publisher, responder)) }
    }

    /// Deconstruct the observer to retrieve the publisher and responder.
    /// This consumes the wrapper and prevents the automatic "Cancelled" response on drop.
    pub fn into_parts(mut self) -> (UsageResponsePublisher<T>, R) {
        self.inner.take().expect("inner components present")
    }
}

impl<T, R> Drop for HangingGetObserver<T, R>
where
    T: Nameable,
{
    fn drop(&mut self) {
        if let Some((publisher, _)) = self.inner.take() {
            publisher.respond("Cancelled".to_string(), ResponseType::Cancelled);
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub enum RequestType {
    Get,
    Set,
    Camera,
    MediaButtons,
}

#[derive(Debug, Copy, Clone)]
/// Response type to a request to a setting. Used for accumulating response type
/// counts for inspect. This should be updated to have a matching error for each
/// of the controller error variants.
pub enum ResponseType {
    OkSome,
    OkNone,
    UnimplementedRequest,
    StorageFailure,
    InitFailure,
    RestoreFailure,
    InvalidArgument,
    IncompatibleArguments,
    ExternalFailure,
    UnhandledType,
    DeliveryError,
    UnexpectedError,
    UndeliverableError,
    UnsupportedError,
    CommunicationError,
    IrrecoverableError,
    TimeoutError,
    AlreadySubscribed,
    MaximumInputDevicesReached,
    Cancelled,
}

#[derive(Copy, Clone, Debug)]
pub struct MediaButtons {
    pub mic_mute: Option<bool>,
    pub camera_disable: Option<bool>,
}

#[derive(Clone)]
pub struct SettingValuePublisher<T> {
    tx: UnboundedSender<(&'static str, String)>,
    _phantom: PhantomData<T>,
}

impl<T> SettingValuePublisher<T> {
    pub fn new(tx: UnboundedSender<(&'static str, String)>) -> Self {
        Self { tx, _phantom: PhantomData }
    }
}

impl<T> SettingValuePublisher<T>
where
    T: Nameable + std::fmt::Debug,
{
    pub fn publish(&self, value: &T) -> Result<(), Error> {
        self.tx
            .unbounded_send((T::NAME, format!("{value:?}")))
            .map_err(|e| anyhow!("Unable to send setting_value update: {e:?}"))
    }
}

#[derive(Clone)]
pub struct ExternalEventPublisher {
    tx: UnboundedSender<ExternalServiceEvent>,
}

impl ExternalEventPublisher {
    pub fn new(tx: UnboundedSender<ExternalServiceEvent>) -> Self {
        Self { tx }
    }

    pub fn publish(&self, event: ExternalServiceEvent) -> Result<(), Error> {
        self.tx
            .unbounded_send(event)
            .map_err(|e| anyhow!("Unable to send external event update: {e:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspect::listener_logger::ListenerInspectLogger;
    use futures::channel::mpsc;

    struct TestSetting;
    impl Nameable for TestSetting {
        const NAME: &'static str = "TestSetting";
    }

    #[test]
    fn test_observer_into_parts() {
        let (tx, _rx) = mpsc::unbounded();
        let logger = Rc::new(ListenerInspectLogger::new());
        let publisher = UsagePublisher::<TestSetting>::new(tx, logger);
        let response_publisher = publisher.request("Watch".to_string(), RequestType::Get);
        let dummy_responder = 42;

        let observer = HangingGetObserver::new(response_publisher, dummy_responder);
        let (pub_out, resp_out) = observer.into_parts();
        assert_eq!(resp_out, 42);

        // Mark sent so drop doesn't log error
        pub_out.respond("Ok".to_string(), ResponseType::OkSome);
    }

    #[test]
    fn test_observer_drop_cancels() {
        let (tx, mut rx) = mpsc::unbounded();
        let logger = Rc::new(ListenerInspectLogger::new());
        let publisher = UsagePublisher::<TestSetting>::new(tx, logger);
        let response_publisher = publisher.request("Watch".to_string(), RequestType::Get);

        let observer = HangingGetObserver::new(response_publisher, ());
        drop(observer);

        // Verify request event was sent
        let req_event = rx.try_next().unwrap().unwrap();
        assert_eq!(req_event.setting, "TestSetting");
        assert!(matches!(req_event.direction, Direction::Request(_)));

        // Verify response event was sent on drop with Cancelled status
        let resp_event = rx.try_next().unwrap().unwrap();
        assert_eq!(resp_event.setting, "TestSetting");
        if let Direction::Response(msg, status) = resp_event.direction {
            assert_eq!(msg, "Cancelled");
            assert!(matches!(status, ResponseType::Cancelled));
        } else {
            panic!("Expected Direction::Response");
        }
    }
}
