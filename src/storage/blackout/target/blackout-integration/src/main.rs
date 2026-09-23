// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use blackout_target::{Test, TestServer};
use fidl_fuchsia_io as fio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct IntegrationTest {
    setup_called: AtomicBool,
    test_called: AtomicBool,
}

impl IntegrationTest {
    fn new() -> Self {
        Self { setup_called: AtomicBool::new(false), test_called: AtomicBool::new(false) }
    }
}

#[async_trait]
impl Test for IntegrationTest {
    async fn setup(
        self: Arc<Self>,
        device_label: String,
        device_path: Option<String>,
        seed: u64,
    ) -> Result<()> {
        log::info!(device_label:%, device_path:?, seed; "setup called");

        if self.setup_called.swap(true, Ordering::Relaxed) {
            log::error!("setup should only be called once!");
            return Err(anyhow::anyhow!("setup should only be called once!"));
        }

        // Make sure we have access to /dev
        let proxy = fuchsia_fs::directory::open_in_namespace("/dev", fio::PERM_READABLE)
            .expect("failed to open /dev");
        let _: Vec<_> = proxy.query().await?;

        Ok(())
    }

    async fn test(
        self: Arc<Self>,
        device_label: String,
        device_path: Option<String>,
        seed: u64,
    ) -> Result<()> {
        log::info!(device_label:%, device_path:?, seed; "test called");

        if self.test_called.swap(true, Ordering::Relaxed) {
            log::error!("test should only be called once!");
            return Err(anyhow::anyhow!("test should only be called once!"));
        }

        // We use the block device path to pass an indicator to loop the test forever, to test
        // returning after the specified duration.
        if device_label == "loop" {
            assert_eq!(device_label, device_path.unwrap());
            futures::future::pending::<()>().await;
        }

        Ok(())
    }

    async fn verify(
        self: Arc<Self>,
        device_label: String,
        device_path: Option<String>,
        seed: u64,
    ) -> Result<()> {
        log::info!(device_label:%, device_path:?, seed; "verify called");

        // We use the block device path to pass an indicator to fail verification, to test the
        // error propagation.
        if device_label == "fail" {
            assert_eq!(device_label, device_path.unwrap());
            Err(anyhow::anyhow!("verification failure"))
        } else {
            Ok(())
        }
    }
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let server = TestServer::new(IntegrationTest::new())?;
    server.serve().await;

    Ok(())
}
