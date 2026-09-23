// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, format_err};
use futures::stream::{BoxStream, StreamExt as _, TryStreamExt as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use crate::device::{PhyEvent, init_phy};

pub struct NewPhyDevice {
    pub id: u16,
    pub proxy: fidl_fuchsia_wlan_phy::WlanPhyProxy,
    pub event_stream: futures::stream::BoxStream<'static, Result<PhyEvent, anyhow::Error>>,
    pub power_dependency_token: Option<fidl_fuchsia_power_broker::DependencyToken>,
}

// Implement Debug manually because BoxStream doesn't implement Debug
impl std::fmt::Debug for NewPhyDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewPhyDevice").field("id", &self.id).field("proxy", &self.proxy).finish()
    }
}

pub async fn watch_phy_devices()
-> Result<BoxStream<'static, Result<NewPhyDevice, anyhow::Error>>, anyhow::Error> {
    let service =
        fuchsia_component::client::Service::open(fidl_fuchsia_wlan_phy::ServiceMarker).ok();
    watch_phy_devices_impl(service).await
}

async fn watch_phy_devices_impl(
    service: Option<fuchsia_component::client::Service<fidl_fuchsia_wlan_phy::ServiceMarker>>,
) -> Result<BoxStream<'static, Result<NewPhyDevice, anyhow::Error>>, anyhow::Error> {
    let service_stream = match service {
        Some(svc) => svc.watch().await.context("watch service"),
        None => Err(anyhow::anyhow!("service not available")),
    };

    let stream = match service_stream {
        Ok(stream) => stream,
        Err(e) => return Err(e),
    };

    let next_id = Arc::new(AtomicU16::new(0));

    let svc_stream = stream
        .then(move |result| {
            let next_id = Arc::clone(&next_id);
            async move {
                let instance_proxy = match result {
                    Err(e) => {
                        return Err(format_err!("Error in service instance stream {e:?}"));
                    }
                    Ok(x) => x,
                };

                let phy_proxy = match instance_proxy.connect_to_device() {
                    Err(e) => {
                        return Err(format_err!("Error connecting to PHY service instance: {}", e));
                    }
                    Ok(x) => x,
                };

                let id = next_id.fetch_add(1, Ordering::Relaxed);
                let (event_stream, power_dependency_token) = init_phy(&phy_proxy).await?;
                Ok(Some(NewPhyDevice {
                    id,
                    proxy: phy_proxy,
                    event_stream,
                    power_dependency_token,
                }))
            }
        })
        .try_filter_map(|x| futures::future::ready(Ok(x)))
        .boxed();

    Ok(svc_stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use futures::poll;
    use futures::task::Poll;
    use std::pin::pin;
    use std::sync::Arc;
    use vfs::pseudo_directory;

    fn serve_mock_wlan_phy() -> Arc<vfs::service::Service> {
        vfs::service::host(
            move |mut stream: fidl_fuchsia_wlan_phy::WlanPhyRequestStream| async move {
                use futures::StreamExt as _;
                while let Some(Ok(req)) = stream.next().await {
                    if let fidl_fuchsia_wlan_phy::WlanPhyRequest::Init { payload: _, responder } =
                        req
                    {
                        let _ = responder
                            .send(Ok(fidl_fuchsia_wlan_phy::WlanPhyInitResponse::default()));
                    }
                }
            },
        )
    }

    #[fuchsia::test]
    async fn test_watch_service_not_available() {
        let res = watch_phy_devices_impl(None).await;
        assert!(res.is_err());
    }

    #[fuchsia::test]
    async fn test_watch_service_available() {
        let fake_svc_dir = pseudo_directory! {
            "fuchsia.wlan.phy.Service" => pseudo_directory! {
                "default" => pseudo_directory! {
                    "device" => serve_mock_wlan_phy(),
                }
            }
        };
        let dir_proxy = vfs::directory::serve_read_only(
            fake_svc_dir,
            vfs::execution_scope::ExecutionScope::new(),
        );
        let service = fuchsia_component::client::Service::open_from_dir(
            dir_proxy,
            fidl_fuchsia_wlan_phy::ServiceMarker,
        )
        .expect("open_from_dir failed");

        let phy_watcher =
            watch_phy_devices_impl(Some(service)).await.expect("failed to start watcher");
        let mut phy_watcher = pin!(phy_watcher);

        let new_phy =
            phy_watcher.next().await.expect("stream ended").expect("watcher returned error");
        assert_eq!(new_phy.id, 0);

        assert_matches!(poll!(phy_watcher.next()), Poll::Pending);
    }
}
