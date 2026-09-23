// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bootfs::BootfsSvc;
use anyhow::{Context, Error, anyhow};
use fidl_fuchsia_time as ftime;
use fuchsia_fs::{PERM_READABLE, file};
use fuchsia_runtime::{UtcClock, UtcInstant};
use futures::prelude::*;
use std::sync::Arc;
use zx::{ClockOpts, Rights};

/// An implementation of the `fuchsia.time.Maintenance` protocol, which
/// maintains a UTC clock, vending out handles with write access.
/// Consumers of this protocol are meant to keep the clock synchronized
/// with external time sources.
pub struct UtcInstantMaintainer {
    utc_clock: Arc<UtcClock>,
}

impl UtcInstantMaintainer {
    pub fn new(utc_clock: Arc<UtcClock>) -> Self {
        UtcInstantMaintainer { utc_clock }
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: ftime::MaintenanceRequestStream,
    ) -> Result<(), Error> {
        while let Some(ftime::MaintenanceRequest::GetWritableUtcClock { responder }) =
            stream.try_next().await?
        {
            responder.send(self.utc_clock.duplicate_handle(Rights::SAME_RIGHTS)?.downcast())?;
        }
        Ok(())
    }
}

async fn read_utc_backstop(path: &str, bootfs: &Option<BootfsSvc>) -> Result<UtcInstant, Error> {
    let file_contents: String;
    if bootfs.is_none() {
        let file_proxy = file::open_in_namespace(path, PERM_READABLE)
            .context("failed to open backstop time file from disk")?;
        file_contents = file::read_to_string(&file_proxy)
            .await
            .context("failed to read backstop time from disk")?;
    } else {
        let canonicalized = if path.starts_with("/boot/") { &path[6..] } else { &path };
        let file_bytes =
            match bootfs.as_ref().unwrap().read_config_from_uninitialized_vfs(canonicalized) {
                Ok(file) => file,
                Err(error) => {
                    return Err(anyhow!(
                        "Failed to read file from uninitialized vfs with error {}.",
                        error
                    ));
                }
            };
        file_contents = String::from_utf8(file_bytes)?;
    }
    let parsed_time =
        file_contents.trim().parse::<i64>().context("failed to parse backstop time")?;
    Ok(UtcInstant::from_nanos(
        parsed_time
            .checked_mul(1_000_000_000)
            .ok_or_else(|| anyhow!("backstop time is too large"))?,
    ))
}

/// Creates a UTC kernel clock with a backstop time configured by /boot.
pub async fn create_utc_clock(bootfs: &Option<BootfsSvc>) -> Result<UtcClock, Error> {
    let backstop = read_utc_backstop("/boot/config/build_info/minimum_utc_stamp", &bootfs).await?;
    // Allows the UTC clock to be converted to a memory-mapped clock.
    let clock_opts = ClockOpts::MAPPABLE;
    let clock = UtcClock::create(clock_opts, Some(backstop))
        .map_err(|s| anyhow!("failed to create UTC clock: {}", s))?;
    clock.set_name(&zx::Name::new_lossy("utc_from_cm")).context("while setting UTC name")?;
    Ok(clock)
}
