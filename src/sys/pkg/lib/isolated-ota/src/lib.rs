// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]

use isolated_swd::omaha;
use isolated_swd::updater::Updater;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("error launching pkg-cache")]
    PkgCacheLaunchError(#[source] anyhow::Error),

    #[error("error launching pkg-resolver")]
    PkgResolverLaunchError(#[source] anyhow::Error),

    #[error("error launching system-updater and installing update")]
    InstallError(#[source] anyhow::Error),

    #[error("error setting up resources")]
    FidlError(#[source] fidl::Error),

    #[error("IO error occurred")]
    IoError(#[source] std::io::Error),

    #[error("error connecting to system-updater")]
    UpdaterConnectError(#[source] anyhow::Error),
}

pub struct OmahaConfig {
    /// The app_id to use for Omaha.
    pub app_id: String,
    /// The URL of the Omaha server.
    pub server_url: String,
}

/// How to obtain the URL for the update package.
pub enum UpdateUrlSource {
    /// Obtain the update package URL from an Omaha server.
    OmahaConfig(OmahaConfig),

    /// Use this URL.
    UpdateUrl(http::Uri),

    /// Use the default update package URL "fuchsia-pkg://fuchsia.com/update".
    UseDefault,
}

/// Installs all packages and writes the Fuchsia ZBI from the latest build on the given channel.
///
/// Has the same arguments as `download_and_apply_update`, but allows passing in
/// pre-configured components for testing.
pub async fn download_and_apply_update_with_updater(
    mut updater: Updater,
    channel_name: &str,
    version: &str,
    update_url_source: UpdateUrlSource,
) -> Result<(), UpdateError> {
    match update_url_source {
        UpdateUrlSource::OmahaConfig(cfg) => {
            let () = omaha::install_update(
                updater,
                cfg.app_id,
                cfg.server_url,
                version.to_owned(),
                channel_name.to_owned(),
            )
            .await
            .map_err(UpdateError::InstallError)?;
        }
        UpdateUrlSource::UpdateUrl(url) => {
            let () = updater.install_update(Some(&url)).await.map_err(UpdateError::InstallError)?;
        }
        UpdateUrlSource::UseDefault => {
            let () = updater.install_update(None).await.map_err(UpdateError::InstallError)?;
        }
    }
    Ok(())
}

/// Installs all packages and writes the Fuchsia ZBI from the latest build on the given channel.
///
/// The following conditions are expected to be met:
/// * Network services (fuchsia.net.name.Lookup and fuchsia.posix.socket.Provider) are available in
///   the /svc/ directory.
/// * `pkg-recovery.cml` should be a child of this component, and all
///   dependencies specified in its 'offer' section should be available in the
///   out directory of the component running this code prior to this function
///   being called.
///
/// If successful, a reboot should be the only thing necessary to boot Fuchsia.
///
/// # Arguments
/// * `channel_name` - The channel to update from.
/// * `version` - Version to report as the current installed version.
/// * `omaha_cfg` - The |OmahaConfig| to use for Omaha. If None, the update will not use Omaha to
///   determine the updater URL.
pub async fn download_and_apply_update(
    channel_name: &str,
    version: &str,
    omaha_cfg: Option<OmahaConfig>,
) -> Result<(), UpdateError> {
    let updater = Updater::new().map_err(UpdateError::UpdaterConnectError)?;
    let update_url_source = if let Some(omaha_cfg) = omaha_cfg {
        UpdateUrlSource::OmahaConfig(omaha_cfg)
    } else {
        UpdateUrlSource::UseDefault
    };
    download_and_apply_update_with_updater(updater, channel_name, version, update_url_source).await
}
