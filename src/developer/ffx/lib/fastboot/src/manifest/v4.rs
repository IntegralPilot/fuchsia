// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::FfxFastbootError;
use crate::file_resolver::FileResolver;
use crate::manifest::{Boot, Flash, Unlock};
use crate::util::Event;

type Result<T> = std::result::Result<T, FfxFastbootError>;
use assembly_partitions_config::UploadMethod;
use async_trait::async_trait;
use ffx_fastboot_interface::fastboot_interface::FastbootInterface;
use ffx_flash_manifest::ManifestParams;
use ffx_flash_manifest::v2::FlashManifest as FlashManifestV2;
use ffx_flash_manifest::v4::FlashManifest;
use tokio::sync::mpsc::Sender;

#[async_trait]
impl Flash for FlashManifest {
    async fn flash<F, T>(
        &self,
        messenger: &Sender<Event>,
        file_resolver: &mut F,
        fastboot_interface: &mut T,
        cmd: ManifestParams,
        ssh_key_upload_method: Option<&UploadMethod>,
    ) -> Result<()>
    where
        F: FileResolver + Sync + Send,
        T: FastbootInterface,
    {
        let v2: FlashManifestV2 = self.into();
        // If the caller explicitly asked for an SSH upload method it takes priority, otherwise use
        // the method from our manifest.
        let method = ssh_key_upload_method.or(self.ssh_key_upload_method.as_ref());
        v2.flash(messenger, file_resolver, fastboot_interface, cmd, method).await
    }
}

#[async_trait]
impl Unlock for FlashManifest {
    async fn unlock<F, T>(
        &self,
        messenger: &Sender<Event>,
        file_resolver: &mut F,
        fastboot_interface: &mut T,
    ) -> Result<()>
    where
        F: FileResolver + Sync + Send,
        T: FastbootInterface,
    {
        let v2: FlashManifestV2 = self.into();
        v2.unlock(messenger, file_resolver, fastboot_interface).await
    }
}

#[async_trait]
impl Boot for FlashManifest {
    async fn boot<F, T>(
        &self,
        messenger: Sender<Event>,
        file_resolver: &mut F,
        slot: String,
        fastboot_interface: &mut T,
        cmd: ManifestParams,
    ) -> Result<()>
    where
        F: FileResolver + Sync + Send,
        T: FastbootInterface,
    {
        let v2: FlashManifestV2 = self.into();
        v2.boot(messenger, file_resolver, slot, fastboot_interface, cmd).await
    }
}

#[cfg(test)]
mod test {
    use super::*;
    type Result<T> = std::result::Result<T, anyhow::Error>;
    use crate::common::vars::{IS_USERSPACE_VAR, MAX_DOWNLOAD_SIZE_VAR, REVISION_VAR};
    use crate::file_resolver::test::TestResolver;
    use ffx_fastboot_interface::test::setup;
    use serde_json::{from_str, json};
    use std::path::PathBuf;
    use tempfile::NamedTempFile;
    use tokio::sync::mpsc;

    #[fuchsia::test]
    async fn v4_manifest_ssh_key_upload_none_defaults_to_staged() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        let ssh_file = NamedTempFile::new().expect("tmp access failed");
        let ssh_file_name = ssh_file.path().to_string_lossy().to_string();

        let manifest = json!({
            "hw_revision": "rev_test",
            "products": [
                {
                    "name": "zedboot",
                    "partitions": []
                }
            ]
        });

        let v: FlashManifest = from_str(&manifest.to_string())?;
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(REVISION_VAR.to_string(), "rev_test-b4".to_string());
            state.set_var(IS_USERSPACE_VAR.to_string(), "no".to_string());
            state.set_var(MAX_DOWNLOAD_SIZE_VAR.to_string(), "8192".to_string());
        }
        let (client, _server) = mpsc::channel(100);
        v.flash(
            &client,
            &mut TestResolver::new(),
            &mut proxy,
            ManifestParams {
                manifest: Some(PathBuf::from(tmp_file_name)),
                product: "zedboot".to_string(),
                ssh_key: Some(ssh_file_name),
                ..Default::default()
            },
            None,
        )
        .await?;

        let state = state.lock().unwrap();
        assert_eq!(state.oem_commands, vec!["oem add-staged-bootloader-file ssh.authorized_keys"]);
        Ok(())
    }

    #[fuchsia::test]
    async fn v4_manifest_ssh_key_upload_some_inline() -> Result<()> {
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        let ssh_file = NamedTempFile::new().expect("tmp access failed");
        std::fs::write(ssh_file.path(), "ssh-ed25519 ABCDEF...").unwrap();
        let ssh_file_name = ssh_file.path().to_string_lossy().to_string();

        let manifest = json!({
            "hw_revision": "rev_test",
            "products": [
                {
                    "name": "zedboot",
                    "partitions": []
                }
            ],
            "ssh_key_upload_method": {
                "type": "inline",
                "command_prefix": "add-key=",
                "command_max_length": 64,
                "init_command": "init-key"
            }
        });

        let v: FlashManifest = from_str(&manifest.to_string())?;
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            state.set_var(REVISION_VAR.to_string(), "rev_test-b4".to_string());
            state.set_var(IS_USERSPACE_VAR.to_string(), "no".to_string());
            state.set_var(MAX_DOWNLOAD_SIZE_VAR.to_string(), "8192".to_string());
        }
        let (client, _server) = mpsc::channel(100);
        v.flash(
            &client,
            &mut TestResolver::new(),
            &mut proxy,
            ManifestParams {
                manifest: Some(PathBuf::from(tmp_file_name)),
                product: "zedboot".to_string(),
                ssh_key: Some(ssh_file_name),
                ..Default::default()
            },
            None,
        )
        .await?;

        let state = state.lock().unwrap();
        assert!(state.oem_commands.len() >= 2);
        assert_eq!(state.oem_commands[0], "oem init-key");
        assert!(state.oem_commands[1].starts_with("oem add-key="));
        Ok(())
    }
}
