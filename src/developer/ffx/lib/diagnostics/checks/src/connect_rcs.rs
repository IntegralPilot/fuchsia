// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_client::fidl::DiscoverableProtocolMarker as _;
use fdomain_fuchsia_hwinfo::{ProductInfo, ProductMarker, ProductProxy};
use ffx_diagnostics::{Check, CheckFut, Notifier};
use ffx_diagnostics_analytics::{PointOfFailure, ResultExt};
use ffx_target::Connection;

pub struct ConnectRemoteControlProxy<N> {
    pub timeout: std::time::Duration,
    _w: std::marker::PhantomData<N>,
}

impl<N> ConnectRemoteControlProxy<N> {
    pub fn new(timeout: std::time::Duration) -> Self {
        Self { timeout, _w: Default::default() }
    }
}

impl<N> Check for ConnectRemoteControlProxy<N>
where
    N: Notifier + Sized,
{
    type Input = Connection;
    type Output = ProductInfo;
    type Notifier = N;

    fn write_preamble(
        &self,
        _input: &Self::Input,
        notifier: &mut Self::Notifier,
    ) -> anyhow::Result<()> {
        notifier.info("Attempting to connect remote control through ssh... ")
    }

    fn on_success(
        &self,
        _output: &Self::Output,
        notifier: &mut Self::Notifier,
    ) -> anyhow::Result<()> {
        notifier.on_success("Success")
    }

    fn check<'a>(
        &'a mut self,
        input: Self::Input,
        _notifier: &'a mut Self::Notifier,
    ) -> CheckFut<'a, Self::Output> {
        Box::pin(async move {
            let proxy = input
                .rcs_proxy_fdomain()
                .await
                .or_else_analytics(|e| {
                    ffx_target::analytics::PointOfFailure::FailedToConnectRCS { error: e }.into()
                })
                .await?;
            let moniker = "/core/hwinfo";
            let product_proxy: ProductProxy = target_holders::open_moniker(
                &proxy,
                rcs::OpenDirType::ExposedDir,
                moniker,
                self.timeout,
            )
            .await
            .or_analytics(PointOfFailure::FailedToOpenHWInfoComponent {
                moniker,
                protocol: ProductMarker::PROTOCOL_NAME,
            })
            .await?;
            let info = product_proxy
                .get_info()
                .await
                .or_else_analytics(|e| PointOfFailure::UnableToGetInfo { error: e }.into())
                .await?;
            Ok(info)
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_client::fidl::DiscoverableProtocolMarker;
    use fdomain_fuchsia_developer_remotecontrol::RemoteControlMarker;
    use fidl_fuchsia_developer_remotecontrol as rcs;
    use fidl_fuchsia_hwinfo::{ProductInfo, ProductMarker, ProductRequest};
    use fuchsia_async::Task;
    use futures_lite::stream::StreamExt;
    use std::sync::Arc;
    use std::time::Duration;

    fn handle_hwinfo(req: rcs::RemoteControlRequest) {
        let rcs::RemoteControlRequest::ConnectCapability {
            server_channel,
            responder,
            capability_name,
            ..
        } = req
        else {
            panic!("unexpected request: {req:?}");
        };
        let res = if capability_name.contains("hwinfo") {
            let server = fidl::endpoints::ServerEnd::<ProductMarker>::new(server_channel);
            Task::spawn(async move {
                let mut stream = server.into_stream();
                while let Ok(Some(req)) = stream.try_next().await {
                    match req {
                        ProductRequest::GetInfo { responder } => {
                            let res = ProductInfo {
                                name: Some("wubwubwub".to_owned()),
                                ..Default::default()
                            };
                            responder.send(&res).unwrap();
                        }
                    }
                }
            })
            .detach();
            Ok(())
        } else {
            Err(rcs::ConnectCapabilityError::NoMatchingCapabilities)
        };
        responder.send(res).unwrap();
    }

    // Creates a local FDomain client with a namespace that only supports the remote control.
    // This remote control also only supports opening the `hwinfo`
    fn fdomain_remote_control_server(
        handler: impl FnOnce(rcs::RemoteControlRequest) + Send + Copy + 'static,
    ) -> Arc<fdomain_client::Client> {
        fdomain_local::local_client(move || {
            let (client, server) =
                fidl::endpoints::create_endpoints::<fidl_fuchsia_io::DirectoryMarker>();
            Task::spawn(async move {
                let mut stream = server.into_stream();
                while let Ok(Some(req)) = stream.try_next().await {
                    if let fidl_fuchsia_io::DirectoryRequest::Open { path, object, .. } = req {
                        assert_eq!(path, RemoteControlMarker::PROTOCOL_NAME);
                        let server =
                            fidl::endpoints::ServerEnd::<rcs::RemoteControlMarker>::new(object);
                        Task::spawn(async move {
                            let mut stream = server.into_stream();
                            while let Ok(Some(req)) = stream.try_next().await {
                                (handler)(req);
                            }
                        })
                        .detach();
                    } else {
                        panic!("Unexpected request: {req:?}");
                    }
                }
            })
            .detach();
            Ok(client)
        })
    }

    #[fuchsia::test]
    async fn test_connect_remote_control_proxy_success() {
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let conn = Connection::from_fdomain_client(fdomain_remote_control_server(handle_hwinfo));
        let res = ConnectRemoteControlProxy::new(Duration::from_secs(1))
            .check_with_notifier(conn, &mut notifier)
            .await;
        assert!(res.is_ok(), "Got '{:?}'", res.unwrap_err());
        let (product_info, _) = res.unwrap();
        let output: String = notifier.into();
        assert!(output.contains("SUCCESS"), "Got '{output}'");
        assert_eq!(product_info.name, Some("wubwubwub".to_owned()));
    }
}
