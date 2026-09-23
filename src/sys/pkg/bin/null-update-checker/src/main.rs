// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use fidl::endpoints::RequestStream as _;
use fidl_fuchsia_process_lifecycle as fprocess_lifecycle;
use fidl_fuchsia_update as fupdate;
use fidl_fuchsia_update_channel as fupdate_channel;
use fuchsia_component::escrow::EscrowOperation;
use fuchsia_component::server::ServiceFs;
use futures::future::FutureExt as _;
use futures::stream::{StreamExt as _, TryStreamExt as _};
use log::{info, warn};

#[fuchsia::main(logging_tags = ["null-update-checker"])]
pub async fn main() -> Result<(), anyhow::Error> {
    let null_update_checker_config::Config { current_ota_channel, stop_on_idle_timeout_millis } =
        null_update_checker_config::Config::take_from_startup_handle();
    let idle_timeout = if stop_on_idle_timeout_millis >= 0 {
        zx::MonotonicDuration::from_millis(stop_on_idle_timeout_millis)
    } else {
        zx::MonotonicDuration::INFINITE
    };

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(Services::ChannelProvider);
    fs.dir("svc").add_fidl_service(Services::Listener);
    fs.take_and_serve_directory_handle().context("taking directory handle")?;
    let fs = fs.until_stalled(idle_timeout);
    let mut service_fut = async move {
        let out_dir = fuchsia_sync::Mutex::new(None);
        let () = fs
            .for_each_concurrent(None, |item| async {
                use fuchsia_component::server::Item;
                match item {
                    Item::Request(Services::ChannelProvider(stream), _active_guard) => {
                        let () =
                            handle_channel_provider(&current_ota_channel, idle_timeout, stream)
                                .await
                                .unwrap_or_else(|e| {
                                    warn!("handling ChannelProvider stream {e:#}");
                                });
                    }
                    Item::Request(Services::Listener(stream), _active_guard) => {
                        let () = handle_listener(stream);
                    }
                    Item::Stalled(outgoing_dir) => {
                        *out_dir.lock() = Some(outgoing_dir);
                    }
                }
            })
            .await;
        out_dir.lock().take().expect("StallableServiceFs should return the out dir before ending")
    }
    .boxed_local()
    .fuse();

    let lifecycle = fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleInfo::new(
        fuchsia_runtime::HandleType::Lifecycle,
        0,
    ))
    .context("taking lifecycle handle")?;
    let lifecycle: fidl::endpoints::ServerEnd<fprocess_lifecycle::LifecycleMarker> =
        lifecycle.into();
    let (mut lifecycle_stream, lifecycle_controller) = lifecycle.into_stream_and_control_handle();
    let escrow_operation = EscrowOperation::new_with_control_handle(lifecycle_controller);

    futures::select! {
        out_dir = service_fut => {
            escrow_operation.run(out_dir.into()).context("failed to run escrow operation")?;
            Ok(())
        },
        req = lifecycle_stream.next() => {
            match req
                .ok_or_else(|| anyhow::anyhow!("LifecycleRequestStream closed unexpectedly"))?
                .context("error reading from LifecycleRequest stream")?
            {
                fprocess_lifecycle::LifecycleRequest::Stop{ control_handle} => {
                    // TODO(https://fxbug.dev/332341289) Exit cleanly by escrowing.
                    info!(
                        "received LifecycleRequest::Stop. Any client connections will be closed. \
                         This should only happen during shutdown."
                    );
                    // The shutdown request is acknowledged by closing the lifecycle channel which
                    // causes the ELF runner to kill the process. [0]
                    // Leak the channel (which is held by `escrow_operation`) so that it will be
                    // closed by the kernel after the process exits normally, allowing the rest of
                    // the process's own cleanup to occur.
                    // [0] https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/lib/elf_runner/src/component.rs;l=245;drc=82b695bd9ac772d898ef8f9525cba51c31040050

                    // Drop these so the Arc<Channel> in the request stream can be unwrapped.
                    drop((control_handle, escrow_operation));
                    // Leak the wrapped channel instead of the RequestStream because the Fuchsia
                    // executor will panic if it is dropped before all registered receivers.
                    let (inner, _terminated): (_, bool) = lifecycle_stream.into_inner();
                    let inner = std::sync::Arc::try_unwrap(inner).map_err(
                        |_: std::sync::Arc<_>| {
                            anyhow::anyhow!("failed to extract lifecycle channel from Arc")
                        },
                    )?;
                    let inner: zx::Channel = inner.into_channel().into_zx_channel();
                    std::mem::forget(inner);
                    Ok(())
                }
            }
        }
    }
}

enum Services {
    ChannelProvider(fupdate_channel::ProviderRequestStream),
    Listener(fupdate::ListenerRequestStream),
}

async fn handle_channel_provider(
    current_ota_channel: &str,
    idle_timeout: zx::MonotonicDuration,
    stream: fupdate_channel::ProviderRequestStream,
) -> Result<(), anyhow::Error> {
    let (stream, unbind_if_stalled) = detect_stall::until_stalled(stream, idle_timeout);
    let mut stream = std::pin::pin!(stream);
    while let Some(request) = stream.try_next().await.context("next ChannelProvider request")? {
        match request {
            fupdate_channel::ProviderRequest::GetCurrent { responder } => {
                let () = responder.send(current_ota_channel).context("sending response")?;
            }
        }
    }

    if let Ok(Some(server_end)) = unbind_if_stalled.await {
        fuchsia_component::client::connect_channel_to_protocol_at::<
            fupdate_channel::ProviderMarker,
        >(server_end, "/escrow")
        .context("escrowing stream")?;
    }

    Ok(())
}

fn handle_listener(stream: fupdate::ListenerRequestStream) {
    let () = stream.control_handle().shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
}
