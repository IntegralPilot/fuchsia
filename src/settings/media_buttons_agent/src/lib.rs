// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl::endpoints::create_request_stream;
use fidl_fuchsia_ui_input::MediaButtonsEvent;
use fidl_fuchsia_ui_policy::{
    DeviceListenerRegistryMarker, MediaButtonsListenerMarker, MediaButtonsListenerRequest,
};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender;
use settings_common::call_async;
use settings_common::inspect::event::ExternalEventPublisher;
use settings_common::service_context::ServiceContext;
use settings_media_buttons::{self as media_buttons, MediaButtons};

/// Method for listening to media button changes. Changes will be reported back
/// on the supplied sender.
pub(crate) async fn monitor_media_buttons(
    service_context: &ServiceContext,
    sender: futures::channel::mpsc::UnboundedSender<MediaButtonsEvent>,
    external_publisher: ExternalEventPublisher,
) -> Result<(), Error> {
    let presenter_service = service_context
        .connect_with_publisher::<DeviceListenerRegistryMarker, _>(external_publisher)
        .await?;
    let (client_end, mut stream) = create_request_stream::<MediaButtonsListenerMarker>();

    // TODO(https://fxbug.dev/42058092) This independent spawn is necessary! For some reason removing this or
    // merging it with the spawn below causes devices to lock up on input button events. Figure out
    // whether this can be removed or left as-is as part of the linked bug.
    fasync::Task::local(async move {
        if let Err(error) = call_async!(presenter_service => register_listener(client_end)).await {
            log::error!(
                "Registering media button listener with presenter service failed {:?}",
                error
            );
        }
    })
    .detach();

    fasync::Task::local(async move {
        while let Some(Ok(media_request)) = stream.next().await {
            // Support future expansion of FIDL
            #[allow(clippy::single_match)]
            #[allow(unreachable_patterns)]
            match media_request {
                MediaButtonsListenerRequest::OnEvent { event, responder } => {
                    sender
                        .unbounded_send(event)
                        .expect("Media buttons sender failed to send event");
                    // Acknowledge the event.
                    responder
                        .send()
                        .unwrap_or_else(|_| log::error!("Failed to ack media buttons event"));
                }
                _ => {}
            }
        }
    })
    .detach();

    Ok(())
}

pub struct MediaButtonsAgent {
    event_txs: Vec<UnboundedSender<media_buttons::Event>>,
    external_publisher: ExternalEventPublisher,
}

impl MediaButtonsAgent {
    pub fn new(
        event_txs: Vec<UnboundedSender<media_buttons::Event>>,
        external_publisher: ExternalEventPublisher,
    ) -> Self {
        Self { event_txs, external_publisher }
    }

    pub async fn spawn(self, service_context: &ServiceContext) -> Result<(), Error> {
        let (input_tx, mut input_rx) = futures::channel::mpsc::unbounded::<MediaButtonsEvent>();
        monitor_media_buttons(service_context, input_tx, self.external_publisher.clone())
            .await
            .context("monitoring media buttons")?;

        let event_handler = EventHandler { event_txs: self.event_txs.clone() };
        fasync::Task::local(async move {
            while let Some(event) = input_rx.next().await {
                event_handler.handle_event(event);
            }
        })
        .detach();

        Ok(())
    }
}

struct EventHandler {
    event_txs: Vec<UnboundedSender<media_buttons::Event>>,
}

impl EventHandler {
    fn handle_event(&self, event: MediaButtonsEvent) {
        if event.mic_mute.is_some() || event.camera_disable.is_some() {
            let media_buttons: MediaButtons = event.into();
            self.send_event(media_buttons);
        }
    }

    fn send_event(&self, event: MediaButtons) {
        for tx in &self.event_txs {
            let _ = tx.unbounded_send(media_buttons::Event::from(event));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_ui_input::MediaButtonsEvent;
    use futures::channel::mpsc;
    use futures::lock::Mutex;
    use settings_media_buttons::{Event, MediaButtonsEventBuilder};
    use settings_test_common::fakes::input_device_registry_service::InputDeviceRegistryService;
    use settings_test_common::fakes::service::ServiceRegistry;
    use settings_test_common::helpers::clone_media_buttons_event_without_wake_lease;
    use std::rc::Rc;

    // Tests that the agent cannot start without a media buttons service.
    #[fuchsia::test(allow_stalls = false)]
    async fn when_media_buttons_inaccessible_returns_err() {
        // Setup messengers needed to construct the agent.
        let (event_tx, _) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(event_tx);

        // Construct the agent.
        let agent = MediaButtonsAgent { event_txs: vec![], external_publisher };
        let service_context = ServiceContext::new(
            // Create a service registry without a media buttons interface.
            Some(ServiceRegistry::serve(ServiceRegistry::create())),
        );

        // Try to spawn the agent without a media buttons interface.
        let result = agent.spawn(&service_context).await;
        assert!(matches!(result, Err(_)));
    }

    // Tests that events can be sent to the intended recipients.
    #[fuchsia::test(allow_stalls = false)]
    async fn event_handler_proxies_event() {
        let (tx1, rx1) = mpsc::unbounded();
        let (tx2, rx2) = mpsc::unbounded();

        // Make all setting types available.
        let event_handler = EventHandler { event_txs: vec![tx1, tx2] };

        // Send the events.
        event_handler.handle_event(
            MediaButtonsEventBuilder::new().set_mic_mute(true).set_camera_disable(true).build(),
        );

        let mut received_channel_events: usize = 0;

        let fused_rx1 = rx1.fuse();
        let fused_rx2 = rx2.fuse();
        futures::pin_mut!(fused_rx1, fused_rx2);

        drop(event_handler);
        // Loop over the select so we can handle the messages as they come in. When all messages
        // have been handled, due to the messengers being deleted above, the complete branch should
        // be hit to break out of the loop.
        loop {
            futures::select! {
                message = fused_rx1.select_next_some() => {
                    match message {
                        settings_media_buttons::Event::OnButton(
                            MediaButtons{..}
                        ) => {
                            received_channel_events += 1;
                        }
                    }
                }
                message = fused_rx2.select_next_some() => {
                    match message {
                        settings_media_buttons::Event::OnButton(
                            MediaButtons{..}
                        ) => {
                            received_channel_events += 1;
                        }
                    }
                }
                complete => break,
            }
        }

        // channels should have received one event each for both mic and camera.
        assert_eq!(received_channel_events, 2);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_media_buttons() {
        let service_registry = ServiceRegistry::create();
        let input_device_registry_service = Rc::new(Mutex::new(InputDeviceRegistryService::new()));

        let initial_event = MediaButtonsEventBuilder::new().set_mic_mute(true).build();
        input_device_registry_service
            .lock()
            .await
            .send_media_button_event(clone_media_buttons_event_without_wake_lease(&initial_event))
            .await;

        service_registry.lock().await.register_service(input_device_registry_service.clone());

        let service_context =
            ServiceContext::new(Some(ServiceRegistry::serve(service_registry.clone())));
        let (event_tx, _) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(event_tx);

        let (input_tx, mut input_rx) = futures::channel::mpsc::unbounded::<MediaButtonsEvent>();
        assert!(
            monitor_media_buttons(&service_context, input_tx, external_publisher).await.is_ok()
        );

        // Listener receives an event immediately upon listening.
        if let Some(event) = input_rx.next().await {
            assert_eq!(initial_event, event);
        }

        // Disable the camera.
        let second_event = MediaButtonsEventBuilder::new().set_camera_disable(true).build();
        input_device_registry_service
            .lock()
            .await
            .send_media_button_event(clone_media_buttons_event_without_wake_lease(&second_event))
            .await;

        // Listener receives the camera disable event.
        if let Some(event) = input_rx.next().await {
            assert_eq!(second_event, event);
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_device_listener_failure() {
        let service_registry = ServiceRegistry::create();
        let input_device_registry_service = Rc::new(Mutex::new(InputDeviceRegistryService::new()));
        input_device_registry_service.lock().await.set_fail(true);

        let initial_event = MediaButtonsEventBuilder::new().set_mic_mute(true).build();

        input_device_registry_service
            .lock()
            .await
            .send_media_button_event(clone_media_buttons_event_without_wake_lease(&initial_event))
            .await;

        service_registry.lock().await.register_service(input_device_registry_service.clone());

        let service_context =
            &ServiceContext::new(Some(ServiceRegistry::serve(service_registry.clone())));
        let (event_tx, _) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(event_tx);

        let (input_tx, _input_rx) = futures::channel::mpsc::unbounded::<MediaButtonsEvent>();
        #[allow(clippy::bool_assert_comparison)]
        {
            assert_eq!(
                monitor_media_buttons(service_context, input_tx, external_publisher).await.is_ok(),
                false
            );
        }
    }

    struct FakeServices {
        input_device_registry: Rc<Mutex<InputDeviceRegistryService>>,
    }

    // Returns a registry and input related services with which it is populated.
    async fn create_services() -> (Rc<Mutex<ServiceRegistry>>, FakeServices) {
        let service_registry = ServiceRegistry::create();

        let input_device_registry_service_handle =
            Rc::new(Mutex::new(InputDeviceRegistryService::new()));
        service_registry
            .lock()
            .await
            .register_service(input_device_registry_service_handle.clone());

        (
            service_registry,
            FakeServices { input_device_registry: input_device_registry_service_handle },
        )
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_media_buttons_proxied() {
        let (event_tx, _) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(event_tx);
        let (tx, mut rx) = mpsc::unbounded();
        let agent = MediaButtonsAgent::new(vec![tx], external_publisher);

        // Setup the fake services.
        let (service_registry, fake_services) = create_services().await;
        let service_context = ServiceContext::new(Some(ServiceRegistry::serve(service_registry)));

        let res = agent.spawn(&service_context).await;
        // Validate that the setup is complete.
        assert!(matches!(res, Ok(())), "spawn failed");

        // The agent should now be initialized. Send a media button event.
        fake_services
            .input_device_registry
            .lock()
            .await
            .send_media_button_event(MediaButtonsEvent {
                volume: Some(1),
                mic_mute: Some(true),
                pause: None,
                camera_disable: None,
                ..Default::default()
            })
            .await;

        let mic_mute = rx.next().await;
        assert_eq!(
            mic_mute,
            Some(Event::OnButton(MediaButtons { mic_mute: Some(true), camera_disable: None }))
        );
    }
}
