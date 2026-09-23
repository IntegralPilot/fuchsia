// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(dead_code)]
#![allow(unused_imports)]
use anyhow::{Context, Error};
use fuchsia_component::client::connect_to_protocol_at_path;
use fuchsia_fs::directory::{DirentKind, open_in_namespace, readdir};

#[fuchsia::test]
async fn enumerate_exposed_tas() -> Result<(), Error> {
    const NOOP_UUID: &str = "185d0391-bb47-495a-ba57-d6c6b808bfae";
    const PANIC_UUID: &str = "7672c06d-f8b3-482b-b8e2-f88fcc8604d7";
    let ta_dir = open_in_namespace("/ta", fuchsia_fs::PERM_READABLE)
        .context("Failed to open /ta directory")?;
    let entries = readdir(&ta_dir).await?;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, NOOP_UUID);
    assert_eq!(entries[0].kind, DirentKind::Directory);
    assert_eq!(entries[1].name, PANIC_UUID);
    assert_eq!(entries[1].kind, DirentKind::Directory);

    let noop_dir = open_in_namespace(&("/ta/".to_owned() + NOOP_UUID), fuchsia_fs::PERM_READABLE)?;
    let noop_entries = readdir(&noop_dir).await?;
    assert_eq!(noop_entries.len(), 1);
    assert_eq!(noop_entries[0].name, "fuchsia.tee.Application");
    assert_eq!(noop_entries[0].kind, DirentKind::Service);

    let panic_dir =
        open_in_namespace(&("/ta/".to_owned() + PANIC_UUID), fuchsia_fs::PERM_READABLE)?;
    let panic_entries = readdir(&panic_dir).await?;
    assert_eq!(panic_entries.len(), 1);
    assert_eq!(panic_entries[0].name, "fuchsia.tee.Application");
    assert_eq!(panic_entries[0].kind, DirentKind::Service);

    Ok(())
}
