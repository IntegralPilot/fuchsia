// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod args;
mod format;

use anyhow::{Context as _, Result};
use component_debug::dirs::{OpenDirType, connect_to_instance_protocol};
use fidl_fuchsia_developer_remotecontrol as rcs;
use fidl_fuchsia_sys2 as fsys;
use moniker::Moniker;
use std::io::Write;

pub async fn connect_to_rcs() -> Result<rcs::RemoteControlProxy> {
    // 1. Try connecting via RealmQuery (works when executed from console-launcher / serial shell)
    for realm_query_path in &["/svc/fuchsia.sys2.RealmQuery.root", "/svc/fuchsia.sys2.RealmQuery"] {
        if let Ok(realm_query) = fuchsia_component::client::connect_to_protocol_at_path::<
            fsys::RealmQueryMarker,
        >(realm_query_path)
        {
            for moniker_str in &["./core/remote-control", "./bootstrap/remote-control"] {
                if let Ok(moniker) = Moniker::try_from(*moniker_str) {
                    if let Ok(proxy) = connect_to_instance_protocol::<rcs::RemoteControlMarker>(
                        &moniker,
                        OpenDirType::Exposed,
                        &realm_query,
                    )
                    .await
                    {
                        return Ok(proxy);
                    }
                }
            }
        }
    }

    // 2. Fall back to standard /svc namespace connection (when component capability is routed)
    fuchsia_component::client::connect_to_protocol::<rcs::RemoteControlMarker>()
        .context("failed to connect to fuchsia.developer.remotecontrol.RemoteControl")
}

pub async fn run_identify_host<W: Write>(
    proxy: &rcs::RemoteControlProxy,
    args: &args::IdentifyHostArgs,
    mut writer: W,
) -> Result<()> {
    match proxy.identify_host().await.context("FIDL call to IdentifyHost failed")? {
        Ok(response) => {
            let output = match args.format {
                args::OutputFormat::Text => format::format_text(&response),
                args::OutputFormat::Json => format::format_json(&response)
                    .context("failed to serialize response to JSON")?,
                args::OutputFormat::PrettyJson => format::format_pretty_json(&response)
                    .context("failed to serialize response to pretty JSON")?,
            };
            writer.write_all(output.as_bytes()).context("failed to write formatted output")?;
            if !output.ends_with('\n') {
                writer.write_all(b"\n").context("failed to write trailing newline")?;
            }
            Ok(())
        }
        Err(err) => {
            anyhow::bail!("IdentifyHost service returned error: {:?}", err);
        }
    }
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let args: args::IdentifyHostArgs = argh::from_env();
    let proxy = connect_to_rcs().await?;
    let stdout = std::io::stdout();
    run_identify_host(&proxy, &args, stdout.lock()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async as fasync;
    use futures::TryStreamExt as _;

    fn spawn_mock_rcs(
        result: Result<rcs::IdentifyHostResponse, rcs::IdentifyHostError>,
    ) -> (rcs::RemoteControlProxy, fasync::Task<()>) {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<rcs::RemoteControlMarker>();
        let task = fasync::Task::spawn(async move {
            while let Ok(Some(req)) = stream.try_next().await {
                if let rcs::RemoteControlRequest::IdentifyHost { responder } = req {
                    let _ = responder.send(result.as_ref().map_err(|e| *e));
                }
            }
        });
        (proxy, task)
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_run_identify_host_text_success() {
        let response = rcs::IdentifyHostResponse {
            nodename: Some("mock-nodename".to_string()),
            serial_number: Some("mock-serial-001".to_string()),
            product_config: Some("workbench_eng".to_string()),
            board_config: Some("x64".to_string()),
            boot_id: Some(123456),
            boot_timestamp_nanos: Some(1700000000000000000),
            ..Default::default()
        };

        let (proxy, _task) = spawn_mock_rcs(Ok(response));
        let args = args::IdentifyHostArgs { format: args::OutputFormat::Text };

        let mut buf = Vec::new();
        run_identify_host(&proxy, &args, &mut buf)
            .await
            .expect("run_identify_host text should succeed");

        let output = String::from_utf8(buf).expect("utf8 string");
        assert!(output.contains("Nodename:            mock-nodename"));
        assert!(output.contains("Serial Number:       mock-serial-001"));
        assert!(output.contains("Product Config:      workbench_eng"));
        assert!(output.contains("Board Config:        x64"));
        assert!(output.contains("Boot ID:             123456"));
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_run_identify_host_json_success() {
        let response = rcs::IdentifyHostResponse {
            nodename: Some("mock-json-nodename".to_string()),
            serial_number: Some("mock-json-serial".to_string()),
            boot_id: Some(987654),
            ..Default::default()
        };

        let (proxy, _task) = spawn_mock_rcs(Ok(response));
        let args = args::IdentifyHostArgs { format: args::OutputFormat::Json };

        let mut buf = Vec::new();
        run_identify_host(&proxy, &args, &mut buf)
            .await
            .expect("run_identify_host json should succeed");

        let output = String::from_utf8(buf).expect("utf8 string");
        assert_eq!(output.matches('\n').count(), 1, "compact json has only the trailing newline");
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid json output");
        assert_eq!(parsed["nodename"], "mock-json-nodename");
        assert_eq!(parsed["serial_number"], "mock-json-serial");
        assert_eq!(parsed["boot_id"], 987654);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_run_identify_host_pretty_json_success() {
        let response = rcs::IdentifyHostResponse {
            nodename: Some("mock-json-nodename".to_string()),
            serial_number: Some("mock-json-serial".to_string()),
            boot_id: Some(987654),
            ..Default::default()
        };

        let (proxy, _task) = spawn_mock_rcs(Ok(response));
        let args = args::IdentifyHostArgs { format: args::OutputFormat::PrettyJson };

        let mut buf = Vec::new();
        run_identify_host(&proxy, &args, &mut buf)
            .await
            .expect("run_identify_host pretty json should succeed");

        let output = String::from_utf8(buf).expect("utf8 string");
        assert!(output.matches('\n').count() > 1, "pretty json has multiple formatted newlines");
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid json output");
        assert_eq!(parsed["nodename"], "mock-json-nodename");
        assert_eq!(parsed["serial_number"], "mock-json-serial");
        assert_eq!(parsed["boot_id"], 987654);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_run_identify_host_error_response() {
        let (proxy, _task) = spawn_mock_rcs(Err(rcs::IdentifyHostError::GetDeviceNameFailed));
        let args = args::IdentifyHostArgs { format: args::OutputFormat::Text };

        let mut buf = Vec::new();
        let err = run_identify_host(&proxy, &args, &mut buf)
            .await
            .expect_err("should fail when service returns GetDeviceNameFailed");

        assert!(err.to_string().contains("GetDeviceNameFailed"), "Error was: {}", err);
    }
}
