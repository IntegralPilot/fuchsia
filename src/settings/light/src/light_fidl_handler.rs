// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;

use async_utils::hanging_get::server;
use fidl_fuchsia_settings::{
    LightError, LightGroup as FidlLightGroup, LightRequest, LightRequestStream, LightState,
    LightWatchLightGroupResponder, LightWatchLightGroupsResponder,
};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;

use crate::light_controller::{
    ARG_NAME, LightController, LightError as ControllerLightError, Request,
};
use crate::types::{LightGroup, LightInfo};
use settings_common::inspect::event::{
    HangingGetObserver, RequestType, ResponseType, UsagePublisher,
};

pub(crate) type SubscriberObject<T> = HangingGetObserver<LightInfo, T>;
pub(crate) type InfoSubscriberObject = SubscriberObject<LightWatchLightGroupsResponder>;
pub(crate) type GroupSubscriberObject = SubscriberObject<LightWatchLightGroupResponder>;

type InfoHangingFn = Box<dyn Fn(&LightInfo, InfoSubscriberObject) -> bool>;
pub(super) type InfoHangingGet = server::HangingGet<LightInfo, InfoSubscriberObject, InfoHangingFn>;
pub(super) type InfoPublisher = server::Publisher<LightInfo, InfoSubscriberObject, InfoHangingFn>;
pub(super) type InfoSubscriber = server::Subscriber<LightInfo, InfoSubscriberObject, InfoHangingFn>;

type GroupHangingFn = Box<dyn Fn(&LightGroup, GroupSubscriberObject) -> bool>;
pub(super) type GroupHangingGet =
    server::HangingGet<LightGroup, GroupSubscriberObject, GroupHangingFn>;
pub(super) type GroupPublisher =
    server::Publisher<LightGroup, GroupSubscriberObject, GroupHangingFn>;
pub(super) type GroupSubscriber =
    server::Subscriber<LightGroup, GroupSubscriberObject, GroupHangingFn>;

pub struct LightFidlHandler {
    info_hanging_get: InfoHangingGet,
    group_hanging_gets: HashMap<String, GroupHangingGet>,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<LightInfo>,
}

impl LightFidlHandler {
    pub(crate) fn new(
        light_controller: &mut LightController,
        usage_publisher: UsagePublisher<LightInfo>,
        initial_value: LightInfo,
    ) -> (Self, UnboundedReceiver<Request>) {
        let (info_hanging_get, group_hanging_gets) = Self::build_hanging_gets(initial_value);
        light_controller.register_publishers(
            info_hanging_get.new_publisher(),
            group_hanging_gets
                .iter()
                .map(|(key, hanging_get)| (key.clone(), hanging_get.new_publisher()))
                .collect(),
        );
        let (controller_tx, controller_rx) = mpsc::unbounded();
        (
            Self { info_hanging_get, group_hanging_gets, controller_tx, usage_publisher },
            controller_rx,
        )
    }

    pub(crate) fn build_hanging_gets(
        info: LightInfo,
    ) -> (InfoHangingGet, HashMap<String, GroupHangingGet>) {
        let group_hanging_gets: HashMap<_, _> = info
            .light_groups
            .clone()
            .into_iter()
            .map(|(key, group)| {
                (
                    key,
                    GroupHangingGet::new(
                        group,
                        Box::new(|group, observer| {
                            let (usage_responder, responder) = observer.into_parts();
                            usage_responder.respond(format!("{group:?}"), ResponseType::OkSome);
                            if let Err(e) = responder.send(&FidlLightGroup::from(group.clone())) {
                                log::warn!("Failed to respond to watch light group request: {e:?}");
                                return false;
                            }
                            true
                        }),
                    ),
                )
            })
            .collect();
        let info_hanging_get: InfoHangingGet = InfoHangingGet::new(
            info,
            Box::new(|info, observer| {
                let (usage_responder, responder) = observer.into_parts();
                usage_responder.respond(format!("{info:?}"), ResponseType::OkSome);
                if let Err(e) = responder.send(&Vec::<FidlLightGroup>::from(info)) {
                    log::warn!("Failed to respond to watch light groups request: {e:?}");
                    return false;
                }
                true
            }),
        );
        (info_hanging_get, group_hanging_gets)
    }

    pub fn handle_stream(&mut self, mut stream: LightRequestStream) {
        let request_handler = RequestHandler {
            info_subscriber: self.info_hanging_get.new_subscriber(),
            group_subscribers: self
                .group_hanging_gets
                .iter_mut()
                .map(|(key, hanging_get)| (key.clone(), hanging_get.new_subscriber()))
                .collect::<HashMap<_, _>>(),
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
    NotFound,
    ControllerStopped,
    Controller(ControllerLightError),
}

impl From<&HandlerError> for ResponseType {
    fn from(error: &HandlerError) -> Self {
        match error {
            HandlerError::AlreadySubscribed => ResponseType::AlreadySubscribed,
            HandlerError::NotFound => ResponseType::InvalidArgument,
            HandlerError::ControllerStopped => ResponseType::UnexpectedError,
            HandlerError::Controller(e) => ResponseType::from(e),
        }
    }
}

impl From<HandlerError> for LightError {
    fn from(error: HandlerError) -> Self {
        if let HandlerError::Controller(ControllerLightError::InvalidArgument(argument, _)) = error
        {
            if ARG_NAME == argument { LightError::InvalidName } else { LightError::InvalidValue }
        } else {
            LightError::Failed
        }
    }
}

struct RequestHandler {
    info_subscriber: InfoSubscriber,
    group_subscribers: HashMap<String, GroupSubscriber>,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<LightInfo>,
}

impl RequestHandler {
    async fn handle_request(&self, request: LightRequest) {
        match request {
            LightRequest::WatchLightGroups { responder } => {
                let usage_res =
                    self.usage_publisher.request("WatchLightGroups".to_string(), RequestType::Get);
                let observer = HangingGetObserver::new(usage_res, responder);
                if let Err(observer) = self.info_subscriber.register2(observer) {
                    let e = HandlerError::AlreadySubscribed;
                    let (usage_res, responder) = observer.into_parts();
                    usage_res.respond(format!("Err({e:?})"), ResponseType::from(&e));
                    drop(responder);
                }
            }
            LightRequest::WatchLightGroup { name, responder } => {
                let usage_res = self
                    .usage_publisher
                    .request(format!("WatchLightGroup{{name:{name:?}}}"), RequestType::Get);
                let res = if let Some(subscriber) = self.group_subscribers.get(&name) {
                    let observer = HangingGetObserver::new(usage_res, responder);
                    subscriber.register2(observer).map_err(|observer| {
                        let (usage_res, responder) = observer.into_parts();
                        (HandlerError::AlreadySubscribed, usage_res, responder)
                    })
                } else {
                    Err((HandlerError::NotFound, usage_res, responder))
                };
                if let Err((e, usage_res, responder)) = res {
                    usage_res.respond(format!("Err({e:?})"), ResponseType::from(&e));
                    drop(responder);
                }
            }
            LightRequest::SetLightGroupValues { name, state, responder } => {
                let usage_res = self.usage_publisher.request(
                    format!("SetLightGroupValues{{name:{name:?},state:{state:?}}}"),
                    RequestType::Set,
                );
                if let Err(e) = self.set(name, state).await {
                    usage_res.respond(format!("Err({e:?}"), ResponseType::from(&e));
                    let _ = responder.send(Err(LightError::from(e)));
                } else {
                    usage_res.respond("Ok(())".to_string(), ResponseType::OkNone);
                    let _ = responder.send(Ok(()));
                }
            }
        }
    }

    async fn set(&self, name: String, state: Vec<LightState>) -> Result<(), HandlerError> {
        let (set_tx, set_rx) = oneshot::channel();
        self.controller_tx
            .unbounded_send(Request::SetLightGroupValue(
                name,
                state.into_iter().map(LightState::into).collect::<Vec<_>>(),
                set_tx,
            ))
            .map_err(|_| HandlerError::ControllerStopped)?;
        set_rx
            .await
            .map_err(|_| HandlerError::ControllerStopped)
            .and_then(|res| res.map_err(HandlerError::Controller))
    }
}

impl From<&LightInfo> for Vec<FidlLightGroup> {
    fn from(info: &LightInfo) -> Self {
        // Internally we store the data in a HashMap, need to flatten it out into a vector.
        info.light_groups.values().cloned().map(FidlLightGroup::from).collect::<Vec<_>>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LightState, LightType, LightValue};

    #[fuchsia::test]
    fn test_response_to_vector_empty() {
        let response: Vec<fidl_fuchsia_settings::LightGroup> =
            (&LightInfo { light_groups: Default::default() }).into();

        assert_eq!(response, vec![]);
    }

    #[fuchsia::test]
    fn test_response_to_vector() {
        let light_group_1 = LightGroup {
            name: "test".to_string(),
            enabled: true,
            light_type: LightType::Simple,
            lights: vec![LightState { value: Some(LightValue::Simple(true)) }],
            hardware_index: vec![],
            disable_conditions: vec![],
        };
        let light_group_2 = LightGroup {
            name: "test2".to_string(),
            enabled: false,
            light_type: LightType::Rgb,
            lights: vec![LightState { value: Some(LightValue::Brightness(0.42)) }],
            hardware_index: vec![],
            disable_conditions: vec![],
        };

        let light_groups: HashMap<_, _> = IntoIterator::into_iter([
            (String::from("test"), light_group_1.clone()),
            (String::from("test2"), light_group_2.clone()),
        ])
        .collect();

        let mut response: Vec<fidl_fuchsia_settings::LightGroup> =
            (&LightInfo { light_groups }).into();

        // Sort so light groups are in a predictable order.
        response.sort_by_key(|l| l.name.clone());

        assert_eq!(
            response,
            vec![FidlLightGroup::from(light_group_1), FidlLightGroup::from(light_group_2)]
        );
    }
}
