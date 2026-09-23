// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::factory_reset_controller::{FactoryResetController, FactoryResetError, Request};
use super::types::FactoryResetInfo;
use async_utils::hanging_get::server;
use fidl_fuchsia_settings::{
    Error as SettingsError, FactoryResetRequest, FactoryResetRequestStream, FactoryResetSettings,
    FactoryResetWatchResponder,
};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;
use settings_common::inspect::event::{
    HangingGetObserver, RequestType, ResponseType, UsagePublisher,
};

impl From<FactoryResetInfo> for FactoryResetSettings {
    fn from(info: FactoryResetInfo) -> Self {
        FactoryResetSettings {
            is_local_reset_allowed: Some(info.is_local_reset_allowed),
            ..Default::default()
        }
    }
}

pub(crate) type SubscriberObject = HangingGetObserver<FactoryResetInfo, FactoryResetWatchResponder>;
type HangingGetFn = fn(&FactoryResetInfo, SubscriberObject) -> bool;
pub(crate) type HangingGet = server::HangingGet<FactoryResetInfo, SubscriberObject, HangingGetFn>;
pub(crate) type Publisher = server::Publisher<FactoryResetInfo, SubscriberObject, HangingGetFn>;
pub(crate) type Subscriber = server::Subscriber<FactoryResetInfo, SubscriberObject, HangingGetFn>;

pub struct FactoryResetFidlHandler {
    hanging_get: HangingGet,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<FactoryResetInfo>,
}

impl FactoryResetFidlHandler {
    pub(crate) fn new(
        factory_reset_controller: &mut FactoryResetController,
        usage_publisher: UsagePublisher<FactoryResetInfo>,
        initial_value: FactoryResetInfo,
    ) -> (Self, UnboundedReceiver<Request>) {
        let hanging_get = HangingGet::new(initial_value, Self::hanging_get);
        factory_reset_controller.register_publisher(hanging_get.new_publisher());
        let (controller_tx, controller_rx) = mpsc::unbounded();
        (Self { hanging_get, controller_tx, usage_publisher }, controller_rx)
    }

    fn hanging_get(info: &FactoryResetInfo, observer: SubscriberObject) -> bool {
        let (usage_responder, responder) = observer.into_parts();
        usage_responder.respond(format!("{info:?}"), ResponseType::OkSome);
        if let Err(e) = responder.send(&FactoryResetSettings::from(*info)) {
            log::warn!("Failed to respond to watch request: {e:?}");
            return false;
        }
        true
    }

    pub fn handle_stream(&mut self, mut stream: FactoryResetRequestStream) {
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
    MissingArg,
    ControllerStopped,
    Controller(FactoryResetError),
}

impl From<&HandlerError> for ResponseType {
    fn from(error: &HandlerError) -> Self {
        match error {
            HandlerError::AlreadySubscribed => ResponseType::AlreadySubscribed,
            HandlerError::MissingArg => ResponseType::InvalidArgument,
            HandlerError::ControllerStopped => ResponseType::UnexpectedError,
            HandlerError::Controller(e) => ResponseType::from(e),
        }
    }
}

struct RequestHandler {
    subscriber: Subscriber,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<FactoryResetInfo>,
}

impl RequestHandler {
    async fn handle_request(&self, request: FactoryResetRequest) {
        match request {
            FactoryResetRequest::Watch { responder } => {
                let usage_res = self.usage_publisher.request("Watch".to_string(), RequestType::Get);
                let observer = HangingGetObserver::new(usage_res, responder);
                if let Err(observer) = self.subscriber.register2(observer) {
                    let e = HandlerError::AlreadySubscribed;
                    let (usage_res, responder) = observer.into_parts();
                    usage_res.respond(format!("Err({e:?})"), ResponseType::from(&e));
                    drop(responder);
                }
            }
            FactoryResetRequest::Set { settings, responder } => {
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

    async fn set(&self, settings: FactoryResetSettings) -> Result<(), HandlerError> {
        let (set_tx, set_rx) = oneshot::channel();
        let local_reset_allowed =
            settings.is_local_reset_allowed.ok_or(HandlerError::MissingArg)?;
        self.controller_tx
            .unbounded_send(Request::Set(FactoryResetInfo::new(local_reset_allowed), set_tx))
            .map_err(|_| HandlerError::ControllerStopped)?;
        set_rx
            .await
            .map_err(|_| HandlerError::ControllerStopped)
            .and_then(|res| res.map_err(HandlerError::Controller))
    }
}
