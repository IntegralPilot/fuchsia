// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::storage::{copy, delete, delete_all, list, make_directory};
use anyhow::{Result, format_err};
use moniker::Moniker;
use std::io::Write;

use flex_client::ProxyHasDomain;
use flex_fuchsia_sys2 as fsys;

async fn get_storage_admin(
    realm_query: fsys::RealmQueryProxy,
    storage_provider_moniker: String,
    storage_capability_name: String,
) -> Result<fsys::StorageAdminProxy> {
    let storage_provider_moniker = Moniker::parse_str(&storage_provider_moniker).map_err(|e| {
        format_err!("Error: {} is not a valid moniker ({})", storage_provider_moniker, e)
    })?;

    let (storage_admin, server_end) =
        realm_query.domain().create_proxy::<fsys::StorageAdminMarker>();

    realm_query
        .connect_to_storage_admin(
            storage_provider_moniker.as_ref(),
            &storage_capability_name,
            server_end,
        )
        .await?
        .map_err(|e| {
            format_err!(
                "Failed to get StorageAdmin proxy for capability '{}' of instance '{}': {:?}",
                storage_capability_name,
                storage_provider_moniker,
                e
            )
        })?;

    Ok(storage_admin)
}

pub async fn storage_copy_cmd(
    storage_provider_moniker: String,
    storage_capability_name: String,
    source_path: String,
    destination_path: String,
    realm_query: fsys::RealmQueryProxy,
) -> Result<()> {
    let storage_admin =
        get_storage_admin(realm_query, storage_provider_moniker, storage_capability_name).await?;
    copy(storage_admin, source_path, destination_path).await
}

pub async fn storage_list_cmd(
    storage_provider_moniker: String,
    storage_capability_name: String,
    path: String,
    realm_query: fsys::RealmQueryProxy,
) -> Result<Vec<String>> {
    let storage_admin =
        get_storage_admin(realm_query, storage_provider_moniker, storage_capability_name).await?;
    list(storage_admin, path).await
}

pub fn storage_list_cmd_write<W: Write>(entries: Vec<String>, mut writer: W) -> Result<()> {
    for entry in entries {
        writeln!(writer, "{entry}")?;
    }
    Ok(())
}

pub async fn storage_make_directory_cmd(
    storage_provider_moniker: String,
    storage_capability_name: String,
    path: String,
    realm_query: fsys::RealmQueryProxy,
) -> Result<()> {
    let storage_admin =
        get_storage_admin(realm_query, storage_provider_moniker, storage_capability_name).await?;
    make_directory(storage_admin, path).await
}

pub async fn storage_delete_cmd(
    storage_provider_moniker: String,
    storage_capability_name: String,
    path: String,
    realm_query: fsys::RealmQueryProxy,
) -> Result<()> {
    let storage_admin =
        get_storage_admin(realm_query, storage_provider_moniker, storage_capability_name).await?;
    delete(storage_admin, path).await
}

pub async fn storage_delete_all_cmd(
    storage_provider_moniker: String,
    storage_capability_name: String,
    moniker: String,
    realm_query: fsys::RealmQueryProxy,
) -> Result<()> {
    let storage_admin =
        get_storage_admin(realm_query, storage_provider_moniker, storage_capability_name).await?;
    delete_all(storage_admin, moniker).await
}
