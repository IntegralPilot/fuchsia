// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use brightness_manager_config::Config;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceObjLocal;
use fuchsia_inspect::component::inspector;
use futures::lock::Mutex;
use futures::prelude::*;
use std::sync::Arc;
// Include Brightness Control and Reader FIDL bindings
use control::{
    Control, ControlTrait, WatcherAdjustmentResponder, WatcherAutoResponder,
    WatcherCurrentResponder,
};
use fidl_fuchsia_ui_brightness::{ControlRequestStream, ReaderRequestStream};
use fuchsia_component::server::ServiceFs;
use futures::channel::mpsc::UnboundedReceiver;
use futures::future::{AbortHandle, Abortable};
use lib::backlight::Backlight;
use lib::sensor::Sensor;
use sender_channel::SenderChannel;
use watch_handler::WatchHandler;

mod control;
mod sender_channel;

const ADJUSTMENT_DELTA: f32 = 0.1;

enum IncomingRequest {
    Control(ControlRequestStream),
    Reader(ReaderRequestStream),
}

struct ServerWatchHandlers {
    watch_current_handler: Arc<Mutex<WatchHandler<f32, WatcherCurrentResponder>>>,
    watch_auto_handler: Arc<Mutex<WatchHandler<bool, WatcherAutoResponder>>>,
    watch_adjustment_handler: Arc<Mutex<WatchHandler<f32, WatcherAdjustmentResponder>>>,
    listen_tasks: [AbortHandle; 3],
}

impl ServerWatchHandlers {
    async fn new(control: Arc<Mutex<dyn ControlTrait>>) -> Result<Self, Error> {
        let (initial_current, initial_auto) = get_initial_value(control.clone()).await?;

        let watch_auto_handler: Arc<Mutex<WatchHandler<bool, WatcherAutoResponder>>> =
            Arc::new(Mutex::new(WatchHandler::create(Some(initial_auto))));

        let (auto_channel_sender, auto_channel_receiver) =
            futures::channel::mpsc::unbounded::<bool>();

        let watch_current_handler: Arc<Mutex<WatchHandler<f32, WatcherCurrentResponder>>> =
            Arc::new(Mutex::new(WatchHandler::create(Some(initial_current))));
        let (current_channel_sender, current_channel_receiver) =
            futures::channel::mpsc::unbounded::<f32>();

        let watch_adjustment_handler: Arc<Mutex<WatchHandler<f32, WatcherAdjustmentResponder>>> =
            Arc::new(Mutex::new(WatchHandler::create_with_change_fn(
                Box::new(move |old_data: &f32, new_data: &f32| {
                    (*new_data - *old_data).abs() >= ADJUSTMENT_DELTA
                }),
                Some(0.0),
            )));
        let (adjustment_channel_sender, adjustment_channel_receiver) =
            futures::channel::mpsc::unbounded::<f32>();

        {
            let mut control = control.lock().await;
            control.add_current_sender_channel(current_channel_sender).await;
            control.add_auto_sender_channel(auto_channel_sender).await;
            control.add_adjustment_sender_channel(adjustment_channel_sender).await;
        }

        let listen_current_task_abort_handle = start_listen_task(
            watch_current_handler.clone(),
            Arc::new(Mutex::new(current_channel_receiver)),
        );

        let listen_auto_task_abort_handle = start_listen_task(
            watch_auto_handler.clone(),
            Arc::new(Mutex::new(auto_channel_receiver)),
        );

        let listen_adjustment_task_abort_handle = start_listen_task(
            watch_adjustment_handler.clone(),
            Arc::new(Mutex::new(adjustment_channel_receiver)),
        );

        Ok(Self {
            watch_current_handler,
            watch_auto_handler,
            watch_adjustment_handler,
            listen_tasks: [
                listen_current_task_abort_handle,
                listen_auto_task_abort_handle,
                listen_adjustment_task_abort_handle,
            ],
        })
    }
}

impl Drop for ServerWatchHandlers {
    fn drop(&mut self) {
        for handle in &self.listen_tasks {
            handle.abort();
        }
    }
}

async fn run_brightness_server(
    mut stream: ControlRequestStream,
    control: Arc<Mutex<dyn ControlTrait>>,
) -> Result<(), Error> {
    let handlers = ServerWatchHandlers::new(control.clone()).await?;

    while let Some(request) = stream.try_next().await.context("error running brightness server")? {
        let mut control = control.lock().await;
        control
            .handle_control_request(
                request,
                handlers.watch_current_handler.clone(),
                handlers.watch_auto_handler.clone(),
                handlers.watch_adjustment_handler.clone(),
            )
            .await;
    }
    Ok(())
}

async fn run_reader_server(
    mut stream: ReaderRequestStream,
    control: Arc<Mutex<dyn ControlTrait>>,
) -> Result<(), Error> {
    let handlers = ServerWatchHandlers::new(control.clone()).await?;

    while let Some(request) = stream.try_next().await.context("error running reader server")? {
        let mut control = control.lock().await;
        control
            .handle_reader_request(
                request,
                handlers.watch_current_handler.clone(),
                handlers.watch_auto_handler.clone(),
                handlers.watch_adjustment_handler.clone(),
            )
            .await;
    }
    Ok(())
}

fn start_listen_task<T: std::marker::Send, ST: std::marker::Send>(
    watch_handler: Arc<Mutex<WatchHandler<T, ST>>>,
    receiver: Arc<Mutex<UnboundedReceiver<T>>>,
) -> AbortHandle
where
    T: std::clone::Clone + 'static,
    ST: watch_handler::Sender<T> + 'static,
{
    let (abort_handle, abort_registration) = AbortHandle::new_pair();
    let receiver = receiver;
    fasync::Task::spawn(
        Abortable::new(
            async move {
                while let Some(value) = receiver.lock().await.next().await {
                    let mut handler_lock = watch_handler.lock().await;
                    handler_lock.set_value(value);
                }
            },
            abort_registration,
        )
        .unwrap_or_else(|_| ()),
    )
    .detach();
    abort_handle
}

async fn get_initial_value(control: Arc<Mutex<dyn ControlTrait>>) -> Result<(f32, bool), Error> {
    let mut control = control.lock().await;
    let (backlight, auto_brightness_on) = control.get_backlight_and_auto_brightness_on();
    let initial_brightness = backlight.get_brightness().await.unwrap_or_else(|e| {
        log::warn!("Didn't get the initial brightness in watch due to err {}, assuming 1.0.", e);
        1.0
    });
    Ok((initial_brightness as f32, auto_brightness_on))
}

async fn run_brightness_service(
    fs: ServiceFs<ServiceObjLocal<'static, IncomingRequest>>,
    control: Arc<Mutex<dyn ControlTrait>>,
) -> Result<(), Error> {
    const MAX_CONCURRENT: usize = 10_000;
    let fut = fs.for_each_concurrent(MAX_CONCURRENT, |request| {
        let control = control.clone();
        async move {
            match request {
                IncomingRequest::Control(stream) => {
                    run_brightness_server(stream, control)
                        .await
                        .unwrap_or_else(|e| log::info!("{:?}", e));
                }
                IncomingRequest::Reader(stream) => {
                    run_reader_server(stream, control)
                        .await
                        .unwrap_or_else(|e| log::info!("{:?}", e));
                }
            }
        }
    });
    fut.await;
    Ok(())
}

#[fuchsia::main(logging_tags = ["auto-brightness"])]
async fn main() -> Result<(), Error> {
    log::info!("Started");
    let config = Config::take_from_startup_handle();
    inspector().root().record_child("config", |config_node| config.record_inspect(config_node));

    let mut fs = ServiceFs::new_local();
    fs.dir("svc")
        .add_fidl_service(IncomingRequest::Control)
        .add_fidl_service(IncomingRequest::Reader);
    fs.take_and_serve_directory_handle()?;

    let inspector = inspector();
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default());

    let backlight = if config.manage_display_power {
        Backlight::with_display_power(config.power_off_delay_millis, config.power_on_delay_millis)
            .await?
    } else {
        Backlight::without_display_power().await?
    };
    let backlight = Arc::new(backlight);

    let sensor = Sensor::new().await;
    let sensor = Arc::new(Mutex::new(sensor));

    let current_sender_channel: SenderChannel<f32> = SenderChannel::new();
    let current_sender_channel = Arc::new(Mutex::new(current_sender_channel));

    let auto_sender_channel: SenderChannel<bool> = SenderChannel::new();
    let auto_sender_channel = Arc::new(Mutex::new(auto_sender_channel));

    let adjustment_sender_channel: SenderChannel<f32> = SenderChannel::new();
    let adjustment_sender_channel = Arc::new(Mutex::new(adjustment_sender_channel));

    let control = Control::new(
        sensor,
        backlight,
        current_sender_channel,
        auto_sender_channel,
        adjustment_sender_channel,
    )
    .await;
    let control = Arc::new(Mutex::new(control));

    run_brightness_service(fs, control).await?;

    Ok(())
}

#[cfg(test)]

mod tests {
    use super::*;

    fn mock_sender_channel() -> SenderChannel<f64> {
        SenderChannel::new()
    }

    #[fuchsia::test]
    async fn test_send_value_in_channel_without_remove_any_sender() {
        let (channel_sender1, mut channel_receiver1) = futures::channel::mpsc::unbounded::<f64>();
        let (channel_sender2, mut channel_receiver2) = futures::channel::mpsc::unbounded::<f64>();
        let mut mock_sender_channel = mock_sender_channel();
        mock_sender_channel.add_sender_channel(channel_sender1).await;
        mock_sender_channel.add_sender_channel(channel_sender2).await;
        mock_sender_channel.send_value(12.0);
        assert_eq!(Some(12.0), channel_receiver1.next().await);
        assert_eq!(Some(12.0), channel_receiver2.next().await);
    }

    #[fuchsia::test]
    async fn test_send_value_in_channel_with_remove_a_sender() {
        let (channel_sender1, mut channel_receiver1) = futures::channel::mpsc::unbounded::<f64>();
        let (channel_sender2, mut channel_receiver2) = futures::channel::mpsc::unbounded::<f64>();
        let mut mock_sender_channel = mock_sender_channel();
        mock_sender_channel.add_sender_channel(channel_sender1).await;
        mock_sender_channel.add_sender_channel(channel_sender2).await;
        mock_sender_channel.sender_channel_vec.write()[0].close_channel();
        mock_sender_channel.send_value(12.0);
        assert_eq!(None, channel_receiver1.next().await);
        assert_eq!(Some(12.0), channel_receiver2.next().await);
    }

    #[fuchsia::test]
    async fn test_reader_server() {
        use control::tests::generate_control_struct;
        use fidl_fuchsia_ui_brightness::ReaderMarker;

        let control = generate_control_struct(400.0, 0.5).await;
        let control = Arc::new(Mutex::new(control));

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<ReaderMarker>();
        let server_task = fasync::Task::local(async move {
            run_reader_server(stream, control).await.unwrap();
        });

        let current = proxy.watch_current_brightness().await.unwrap();
        assert_eq!(current, 0.5);

        let auto = proxy.watch_auto_brightness().await.unwrap();
        assert_eq!(auto, false);

        let adjustment = proxy.watch_auto_brightness_adjustment().await.unwrap();
        assert_eq!(adjustment, 0.0);

        let max_bright = proxy.get_max_absolute_brightness().await.unwrap();
        assert_eq!(max_bright, Ok(250.0));

        drop(proxy);
        server_task.await;
    }

    #[fuchsia::test]
    async fn test_control_and_reader_interaction() {
        use control::tests::generate_control_struct;
        use fidl_fuchsia_ui_brightness::{ControlMarker, ReaderMarker};

        let control = generate_control_struct(400.0, 0.5).await;
        let control = Arc::new(Mutex::new(control));

        let (control_proxy, control_stream) =
            fidl::endpoints::create_proxy_and_stream::<ControlMarker>();
        let (reader_proxy, reader_stream) =
            fidl::endpoints::create_proxy_and_stream::<ReaderMarker>();

        let control_clone = control.clone();
        let brightness_server_task = fasync::Task::local(async move {
            run_brightness_server(control_stream, control_clone).await.unwrap();
        });

        let reader_server_task = fasync::Task::local(async move {
            run_reader_server(reader_stream, control).await.unwrap();
        });

        let initial_brightness = reader_proxy.watch_current_brightness().await.unwrap();
        assert_eq!(initial_brightness, 0.5);

        let watch_future = reader_proxy.watch_current_brightness();

        control_proxy.set_manual_brightness_smooth(0.8, 0).unwrap();

        let new_brightness = watch_future.await.unwrap();
        assert_eq!(new_brightness, 0.8);

        drop(control_proxy);
        drop(reader_proxy);
        brightness_server_task.await;
        reader_server_task.await;
    }
}
