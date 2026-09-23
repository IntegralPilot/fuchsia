// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::platform::PlatformServices;
use anyhow::{Error, anyhow};
use fidl_fuchsia_virtualization::{
    GuestMarker, GuestStatus, MemControllerMarker, MemControllerProxy,
};
use guest_cli_args as arguments;
use std::fmt;
use zx_status;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RequestSizeResult {
    size: u64,
}

impl fmt::Display for RequestSizeResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Resizing dynamically plugged memory to {} bytes!\n", self.size)
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
pub struct VirtioMemStats {
    block_size: u64,
    region_size: u64,
    usable_region_size: u64,
    plugged_size: u64,
    requested_size: u64,
}

impl fmt::Display for VirtioMemStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Dynamically plugged memory stats:\n")?;
        write!(f, "    block_size:          {}\n", self.block_size)?;
        write!(f, "    region_size:         {}\n", self.region_size)?;
        write!(f, "    usable_region_size:  {}\n", self.usable_region_size)?;
        write!(f, "    plugged_size:        {}\n", self.plugged_size)?;
        write!(f, "    requested_size:      {}\n", self.requested_size)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum GuestMemResult {
    RequestSize(RequestSizeResult),
    MemStats(VirtioMemStats),
}

impl fmt::Display for GuestMemResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GuestMemResult::RequestSize(res) => write!(f, "{}", res),
            GuestMemResult::MemStats(stats) => write!(f, "{}", stats),
        }
    }
}

async fn connect_to_mem_controller<P: PlatformServices>(
    services: &P,
    guest_type: arguments::GuestType,
) -> Result<MemControllerProxy, Error> {
    let manager = services.connect_to_manager(guest_type).await?;

    let guest_info = manager.get_info().await?;
    if guest_info.guest_status.expect("guest status should be set") == GuestStatus::Running {
        let (guest_endpoint, guest_server_end) = fidl::endpoints::create_proxy::<GuestMarker>();
        manager
            .connect(guest_server_end)
            .await
            .map_err(|err| anyhow!("failed to get a connect response: {}", err))?
            .map_err(|err| anyhow!("connect failed with: {:?}", err))?;

        let (controller, server_end) = fidl::endpoints::create_proxy::<MemControllerMarker>();

        guest_endpoint
            .get_mem_controller(server_end)
            .await?
            .map_err(|err| anyhow!("failed to get MemController: {:?}", err))?;

        Ok(controller)
    } else {
        Err(anyhow!(zx_status::Status::NOT_CONNECTED))
    }
}

pub async fn handle_mem<P: PlatformServices>(
    services: &P,
    args: arguments::mem_args::MemArgs,
) -> Result<GuestMemResult, Error> {
    Ok(match args.mem_cmd {
        arguments::mem_args::MemCommands::RequestPluggedMem(args) => {
            GuestMemResult::RequestSize(do_request_size(
                connect_to_mem_controller(services, args.guest_type).await?,
                args.size,
            )?)
        }
        arguments::mem_args::MemCommands::StatsMem(args) => GuestMemResult::MemStats(
            do_stats(connect_to_mem_controller(services, args.guest_type).await?).await?,
        ),
    })
}

fn do_request_size(controller: MemControllerProxy, size: u64) -> Result<RequestSizeResult, Error> {
    controller.request_size(size)?;
    Ok(RequestSizeResult { size })
}

async fn do_stats(controller: MemControllerProxy) -> Result<VirtioMemStats, Error> {
    let (block_size, region_size, usable_region_size, plugged_size, requested_size) =
        controller.get_mem_size().await?;
    Ok(VirtioMemStats { block_size, region_size, usable_region_size, plugged_size, requested_size })
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fuchsia_async as fasync;
    use futures::StreamExt;

    #[fuchsia::test(allow_stalls = false)]
    async fn mem_valid_request_plugged_returns_ok() {
        let (proxy, mut stream) = create_proxy_and_stream::<MemControllerMarker>();
        let size = 12345;
        let expected_string = format!("Resizing dynamically plugged memory to {} bytes!\n", size);

        let res = do_request_size(proxy, size).unwrap();
        let _ = stream
            .next()
            .await
            .expect("Failed to read from stream")
            .expect("Failed to parse request")
            .into_request_size()
            .expect("Unexpected call to Mem Controller");

        assert_eq!(res.size, size);
        assert_eq!(format!("{}", res), expected_string);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn mem_valid_stats_returns_ok() {
        let (proxy, mut stream) = create_proxy_and_stream::<MemControllerMarker>();
        let (block_size, region_size, usable_region_size, plugged_size, requested_size) =
            (1, 2, 3, 4, 5);

        let _task = fasync::Task::spawn(async move {
            let get_mem_size_responder = stream
                .next()
                .await
                .expect("Failed to read from stream")
                .expect("Failed to parse request")
                .into_get_mem_size()
                .expect("Unexpected call to Mem Controller");
            get_mem_size_responder
                .send(block_size, region_size, usable_region_size, plugged_size, requested_size)
                .expect("Failed to send request to proxy");
        });
        let res = do_stats(proxy).await.unwrap();
        assert_eq!(
            res,
            VirtioMemStats {
                block_size,
                region_size,
                usable_region_size,
                plugged_size,
                requested_size
            }
        );

        assert_eq!(
            format!("{}", res),
            concat!(
                "Dynamically plugged memory stats:\n",
                "    block_size:          1\n",
                "    region_size:         2\n",
                "    usable_region_size:  3\n",
                "    plugged_size:        4\n",
                "    requested_size:      5\n",
            )
        );
    }
}
