// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::intl_controller::{IntlController, IntlError, Request};
use super::types::IntlInfo;
use async_utils::hanging_get::server;
use fidl_fuchsia_settings::{
    Error as SettingsError, IntlRequest, IntlRequestStream, IntlSettings, IntlWatchResponder,
};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;
use settings_common::inspect::event::{
    HangingGetObserver, RequestType, ResponseType, UsagePublisher,
};

pub(super) type SubscriberObject = HangingGetObserver<IntlInfo, IntlWatchResponder>;
type HangingGetFn = fn(&IntlInfo, SubscriberObject) -> bool;
pub(super) type HangingGet = server::HangingGet<IntlInfo, SubscriberObject, HangingGetFn>;
pub(super) type Publisher = server::Publisher<IntlInfo, SubscriberObject, HangingGetFn>;
pub(super) type Subscriber = server::Subscriber<IntlInfo, SubscriberObject, HangingGetFn>;

pub struct IntlFidlHandler {
    hanging_get: HangingGet,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<IntlInfo>,
}

impl IntlFidlHandler {
    pub(crate) fn new(
        intl_controller: &mut IntlController,
        usage_publisher: UsagePublisher<IntlInfo>,
        initial_value: IntlInfo,
    ) -> (Self, UnboundedReceiver<Request>) {
        let hanging_get = HangingGet::new(initial_value, Self::hanging_get);
        intl_controller.register_publisher(hanging_get.new_publisher());
        let (controller_tx, controller_rx) = mpsc::unbounded();
        (Self { hanging_get, controller_tx, usage_publisher }, controller_rx)
    }

    fn hanging_get(info: &IntlInfo, observer: SubscriberObject) -> bool {
        let (usage_responder, responder) = observer.into_parts();
        usage_responder.respond(format!("{info:?}"), ResponseType::OkSome);
        if let Err(e) = responder.send(&IntlSettings::from(info.clone())) {
            log::warn!("Failed to respond to watch request: {e:?}");
            return false;
        }
        true
    }

    pub fn handle_stream(&mut self, mut stream: IntlRequestStream) {
        let request_handler = RequestHandler {
            subscriber: self.hanging_get.new_subscriber(),
            controller_tx: self.controller_tx.clone(),
            usage_publisher: self.usage_publisher.clone(),
        };
        fasync::Task::local(async move {
            while let Some(Ok(request)) = stream.next().await {
                request_handler.handle_request(request).await;
            }
        })
        .detach();
    }
}

#[derive(Debug)]
enum HandlerError {
    AlreadySubscribed,
    ControllerStopped,
    Controller(IntlError),
}

impl From<&HandlerError> for ResponseType {
    fn from(error: &HandlerError) -> Self {
        match error {
            HandlerError::AlreadySubscribed => ResponseType::AlreadySubscribed,
            HandlerError::ControllerStopped => ResponseType::UnexpectedError,
            HandlerError::Controller(e) => ResponseType::from(e),
        }
    }
}

struct RequestHandler {
    subscriber: Subscriber,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<IntlInfo>,
}

impl RequestHandler {
    async fn handle_request(&self, request: IntlRequest) {
        match request {
            IntlRequest::Watch { responder } => {
                let usage_res = self.usage_publisher.request("Watch".to_string(), RequestType::Get);
                let observer = HangingGetObserver::new(usage_res, responder);
                if let Err(observer) = self.subscriber.register2(observer) {
                    let e = HandlerError::AlreadySubscribed;
                    let (usage_res, responder) = observer.into_parts();
                    usage_res.respond(format!("Err({e:?})"), ResponseType::from(&e));
                    drop(responder);
                }
            }
            IntlRequest::Set { settings, responder } => {
                let usage_res = self
                    .usage_publisher
                    .request(format!("Set{{settings:{settings:?}}}"), RequestType::Set);
                if let Err(e) = self.set(settings).await {
                    usage_res.respond(format!("Err({e:?}"), ResponseType::from(&e));
                    let _ = responder.send(Err(SettingsError::Failed));
                } else {
                    usage_res.respond("Ok(())".to_string(), ResponseType::OkNone);
                    let _ = responder.send(Ok(()));
                }
            }
        }
    }

    async fn set(&self, settings: IntlSettings) -> Result<(), HandlerError> {
        let (set_tx, set_rx) = oneshot::channel();
        self.controller_tx
            .unbounded_send(Request::Set(IntlInfo::from(settings), set_tx))
            .map_err(|_| HandlerError::ControllerStopped)?;
        set_rx
            .await
            .map_err(|_| HandlerError::ControllerStopped)
            .and_then(|res| res.map_err(HandlerError::Controller))
    }
}
