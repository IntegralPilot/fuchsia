// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_controller::{InputController, InputError, Request};
use crate::types::{DeviceStateSource, InputDevice, InputDeviceType, InputInfo};
use anyhow::{Error, anyhow};
use async_utils::hanging_get::server;
use fidl_fuchsia_settings::{
    Error as SettingsError, InputRequest, InputRequestStream, InputSettings, InputState,
    InputWatchResponder,
};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;
use settings_common::inspect::event::{
    HangingGetObserver, RequestType, ResponseType, UsagePublisher,
};

fn to_request(fidl_input_states: Vec<InputState>) -> Result<Vec<InputDevice>, Error> {
    // Every device requires at least a device type and state flags.
    let mut input_states_invalid_args = fidl_input_states
        .iter()
        .filter(|input_state| input_state.device_type.is_none() || input_state.state.is_none());

    // If any devices were filtered out, the args were invalid, so exit.
    if input_states_invalid_args.next().is_some() {
        return Err(anyhow!("Failed to parse input request: missing args"));
    }

    let input_states = fidl_input_states
        .iter()
        .map(|input_state| {
            let device_type: InputDeviceType = input_state.device_type.unwrap().into();
            let device_state = input_state.state.clone().unwrap().into();
            let device_name = input_state.name.clone().unwrap_or_else(|| device_type.to_string());
            let source_states = [(DeviceStateSource::SOFTWARE, device_state)].into();
            InputDevice { name: device_name, device_type, state: device_state, source_states }
        })
        .collect();

    Ok(input_states)
}

pub(super) type SubscriberObject = HangingGetObserver<InputInfo, InputWatchResponder>;
type HangingGetFn = fn(&InputInfo, SubscriberObject) -> bool;
pub(super) type HangingGet = server::HangingGet<InputInfo, SubscriberObject, HangingGetFn>;
pub(super) type Publisher = server::Publisher<InputInfo, SubscriberObject, HangingGetFn>;
pub(super) type Subscriber = server::Subscriber<InputInfo, SubscriberObject, HangingGetFn>;

pub struct InputFidlHandler {
    hanging_get: HangingGet,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<InputInfo>,
}

impl InputFidlHandler {
    pub(crate) fn new(
        input_controller: &mut InputController,
        usage_publisher: UsagePublisher<InputInfo>,
        initial_value: InputInfo,
    ) -> (Self, UnboundedReceiver<Request>) {
        let hanging_get = HangingGet::new(initial_value, Self::hanging_get);
        input_controller.register_publisher(hanging_get.new_publisher());
        let (controller_tx, controller_rx) = mpsc::unbounded();
        (Self { hanging_get, controller_tx, usage_publisher }, controller_rx)
    }

    fn hanging_get(info: &InputInfo, observer: SubscriberObject) -> bool {
        let (usage_responder, responder) = observer.into_parts();
        usage_responder.respond(format!("{info:?}"), ResponseType::OkSome);
        if let Err(e) = responder.send(&InputSettings::from(info)) {
            log::warn!("Failed to respond to watch request: {e:?}");
            return false;
        }
        true
    }

    pub fn handle_stream(&mut self, mut stream: InputRequestStream) {
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
    InvalidArgument(
        // Error used by Debug impl for inspect logs.
        #[allow(dead_code)] Error,
    ),
    ControllerStopped,
    Controller(InputError),
}

impl From<&HandlerError> for ResponseType {
    fn from(error: &HandlerError) -> Self {
        match error {
            HandlerError::AlreadySubscribed => ResponseType::AlreadySubscribed,
            HandlerError::InvalidArgument(_) => ResponseType::InvalidArgument,
            HandlerError::ControllerStopped => ResponseType::UnexpectedError,
            HandlerError::Controller(e) => ResponseType::from(e),
        }
    }
}

struct RequestHandler {
    subscriber: Subscriber,
    controller_tx: UnboundedSender<Request>,
    usage_publisher: UsagePublisher<InputInfo>,
}

impl RequestHandler {
    async fn handle_request(&self, request: InputRequest) {
        match request {
            InputRequest::Watch { responder } => {
                let usage_res = self.usage_publisher.request("Watch".to_string(), RequestType::Get);
                let observer = HangingGetObserver::new(usage_res, responder);
                if let Err(observer) = self.subscriber.register2(observer) {
                    let e = HandlerError::AlreadySubscribed;
                    let (usage_res, responder) = observer.into_parts();
                    usage_res.respond(format!("Err({e:?})"), ResponseType::from(&e));
                    drop(responder);
                }
            }
            InputRequest::Set { input_states, responder } => {
                let usage_res = self
                    .usage_publisher
                    .request(format!("Set{{input_states:{input_states:?}}}"), RequestType::Set);
                if let Err(e) = self.set(input_states).await {
                    usage_res.respond(format!("Err({e:?}"), ResponseType::from(&e));
                    let _ = responder.send(Err(SettingsError::Failed));
                } else {
                    usage_res.respond("Ok(())".to_string(), ResponseType::OkNone);
                    let _ = responder.send(Ok(()));
                }
            }
        }
    }

    async fn set(&self, input_states: Vec<InputState>) -> Result<(), HandlerError> {
        let (set_tx, set_rx) = oneshot::channel();
        let info = to_request(input_states).map_err(HandlerError::InvalidArgument)?;
        self.controller_tx
            .unbounded_send(Request::Set(info, set_tx))
            .map_err(|_| HandlerError::ControllerStopped)?;
        set_rx
            .await
            .map_err(|_| HandlerError::ControllerStopped)
            .and_then(|res| res.map_err(HandlerError::Controller))
    }
}
