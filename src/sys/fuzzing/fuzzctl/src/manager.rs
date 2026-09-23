// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result, anyhow, bail};
use flex_client::fidl::ProtocolMarker;
use flex_client::{self, ProxyHasDomain};
use flex_fuchsia_fuzzer::{self as fuzz};
use url::Url;
use zx_status as zx;

/// Represents the FIDL connection from the `ffx fuzz` plugin to the `fuzz-manager` component on a
/// target device.
pub struct Manager {
    proxy: fuzz::ManagerProxy,
}

impl Manager {
    /// Creates a new `Manager`.
    ///
    /// The created object maintains a FIDL `proxy` to the `fuzz-manager` component on a target
    /// device. Any output produced by this object will be written using the given `writer`.
    pub fn new(proxy: fuzz::ManagerProxy) -> Self {
        Self { proxy }
    }

    /// Requests that the `fuzz-manager` connect to a fuzzer instance.
    ///
    /// This will create and connect a `fuchsia.fuzzer.Controller` to the fuzzer on the target
    /// device given by the `url`. Any artifacts produced by the fuzzer will be saved to the
    /// `artifact_dir`, and outputs such as logs can optionally be saved to the `output_dir`.
    ///
    /// Returns an object representing the connected fuzzer, or an error.
    pub async fn connect(&self, url: &Url) -> Result<fuzz::ControllerProxy> {
        let dc = self.proxy.domain();
        let (proxy, server_end) = dc.create_proxy::<fuzz::ControllerMarker>();
        let result = self
            .proxy
            .connect(url.as_str(), server_end)
            .await
            .map_err(|e| anyhow!("{}/Connect: {}", fuzz::ManagerMarker::DEBUG_NAME, e))?;
        if let Err(e) = result {
            return Err(anyhow!(
                "{}/Connect returned ZX_ERR_{}",
                fuzz::ManagerMarker::DEBUG_NAME,
                zx::Status::err_from_raw(e)
            ));
        }
        Ok(proxy)
    }

    /// Returns a socket that provides the given type of fuzzer output.
    pub async fn get_output(
        &self,
        url: &Url,
        output: fuzz::TestOutput,
    ) -> Result<flex_client::Socket> {
        let dc = self.proxy.domain();
        let (rx, tx) = dc.create_stream_socket();
        let result = self
            .proxy
            .get_output(url.as_str(), output, tx)
            .await
            .context("failed to get output")?;
        if let Err(e) = result {
            bail!(
                "fuchsia.fuzzer/Manager.GetOutput returned ZX_ERR_{}",
                zx::Status::err_from_raw(e)
            );
        }
        Ok(rx)
    }

    /// Requests that the `fuzz-manager` stop a running fuzzer instance.
    ///
    /// As a result of this call, the fuzzer component will cease an ongoing workflow and exit.
    ///
    /// Returns whether a fuzzer was stopped.
    pub async fn stop(&self, url: &Url) -> Result<bool> {
        let result = self.proxy.stop(url.as_str()).await.context(fidl_name("Stop"))?;
        match result {
            Ok(()) => Ok(true),
            Err(e) if e == zx::Status::NOT_FOUND.into_raw() => Ok(false),
            Err(e) => {
                bail!("fuchsia.fuzzer/Manager.Stop returned ZX_ERR_{}", zx::Status::err_from_raw(e))
            }
        }
    }
}

fn fidl_name(method: &str) -> String {
    format!("{}/{}", fuzz::ManagerMarker::DEBUG_NAME, method)
}

#[cfg(test)]
mod tests {
    use super::Manager;
    use anyhow::Result;
    use flex_fuchsia_fuzzer as fuzz;
    use fuchsia_fuzzctl_test::{TEST_URL, Test, create_task, serve_manager};
    use url::Url;

    #[fuchsia::test]
    async fn test_connect() -> Result<()> {
        let test = Test::try_new()?;
        let (proxy, server_end) = test.create_proxy::<fuzz::ManagerMarker>();
        let _task = create_task(serve_manager(server_end, test.clone()), test.writer());
        let manager = Manager::new(proxy);

        let url = Url::parse(TEST_URL)?;
        manager.connect(&url).await?;

        let actual = test.url().borrow().as_ref().map(|url| url.to_string());
        let expected = Some(url.to_string());
        assert_eq!(actual, expected);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_stop() -> Result<()> {
        let test = Test::try_new()?;
        let (proxy, server_end) = test.create_proxy::<fuzz::ManagerMarker>();
        let _task = create_task(serve_manager(server_end, test.clone()), test.writer());
        let manager = Manager::new(proxy);

        let url = Url::parse(TEST_URL)?;
        manager.stop(&url).await?;

        let actual = test.url().borrow().as_ref().map(|url| url.to_string());
        let expected = Some(url.to_string());
        assert_eq!(actual, expected);
        Ok(())
    }
}
