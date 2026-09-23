// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, anyhow};
use fidl_fuchsia_mem as fmem;
use fidl_fuchsia_paver as fpaver;
use log::{info, warn};

mod configuration;
pub use configuration::{CurrentConfiguration, NonCurrentConfiguration, TargetConfiguration};

#[derive(Debug, thiserror::Error)]
pub enum WriteAssetError {
    #[error("while performing write_asset call")]
    Fidl(#[from] fidl::Error),

    #[error("write_asset responded with")]
    Status(#[from] zx::Status),
}

async fn paver_write_firmware(
    data_sink: &fpaver::DataSinkProxy,
    configuration: fpaver::Configuration,
    type_: &str,
    buffer: fmem::Buffer,
) -> anyhow::Result<()> {
    let res = data_sink
        .write_firmware(configuration, type_, buffer)
        .await
        .context("DataSink.WriteFirmware FIDL error")?;
    match res {
        fpaver::WriteFirmwareResult::Status(status) => {
            zx::Status::ok(status).context("firmware failed to write")?;
        }
        fpaver::WriteFirmwareResult::Unsupported(_) => {
            info!("skipping unsupported firmware type: {type_}");
        }
    };
    Ok(())
}

async fn paver_write_asset(
    data_sink: &fpaver::DataSinkProxy,
    configuration: fpaver::Configuration,
    asset: fpaver::Asset,
    buffer: fmem::Buffer,
) -> Result<(), WriteAssetError> {
    Ok(zx::Status::ok(data_sink.write_asset(configuration, asset, buffer).await?)?)
}

/// The size field of the fmem::Buffer will be either the size of the entire partition or the size
/// of just the image.
pub async fn read_image(
    data_sink: &fpaver::DataSinkProxy,
    configuration: fpaver::Configuration,
    image_type: super::ImageType<'_>,
) -> anyhow::Result<fmem::Buffer> {
    use super::ImageType::*;
    match image_type {
        Asset(asset) => data_sink
            .read_asset(configuration, asset)
            .await
            .context("DataSink.ReadAsset FIDL error")?
            .map_err(|s| {
                let status = zx::Status::err_from_raw(s);
                anyhow!("DataSink.ReadAsset error {status:?}")
            }),
        Firmware { type_ } => data_sink
            .read_firmware(configuration, type_)
            .await
            .context("DataSink.ReadFirmware FIDL error")?
            .map_err(|s| {
                let status = zx::Status::err_from_raw(s);
                anyhow!("DataSink.ReadFirmware error {status:?}")
            }),
    }
}

fn clone_buffer(buffer: &fmem::Buffer) -> anyhow::Result<fmem::Buffer> {
    Ok(fmem::Buffer {
        vmo: buffer
            .vmo
            .create_child(
                zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE | zx::VmoChildOptions::RESIZABLE,
                0,
                buffer.size,
            )
            .context("clone_buffer: creating child VMO")?,
        size: buffer.size,
    })
}

/// Writes the given image to the configuration/asset location. If configuration is not given, the
/// image is written to both A and B (if the B partition exists).
async fn write_asset_to_configurations(
    data_sink: &fpaver::DataSinkProxy,
    configuration: TargetConfiguration,
    asset: fpaver::Asset,
    buffer: fmem::Buffer,
) -> anyhow::Result<()> {
    match configuration {
        TargetConfiguration::Single(configuration) => {
            // Devices supports ABR and/or a specific configuration (ex. Recovery) was requested.
            paver_write_asset(data_sink, configuration, asset, buffer).await?
        }
        TargetConfiguration::AB => {
            // Device does not support ABR, so write the image to the A partition.
            //
            // Also try to write the image to the B partition to be forwards compatible with devices
            // that will eventually support ABR. If the device does not have a B partition yet, log
            // the error and continue.
            let () = paver_write_asset(
                data_sink,
                fpaver::Configuration::A,
                asset,
                clone_buffer(&buffer)?,
            )
            .await?;
            match paver_write_asset(data_sink, fpaver::Configuration::B, asset, buffer).await {
                Ok(()) => (),
                Err(WriteAssetError::Status(zx::Status::NOT_SUPPORTED)) => {
                    warn!(
                        "skipping write of {asset:?} to B partition, B not supported by the device"
                    );
                }
                Err(e) => Err(e)?,
            }
        }
    };
    Ok(())
}

async fn write_firmware_to_configurations(
    data_sink: &fpaver::DataSinkProxy,
    configuration: TargetConfiguration,
    type_: &str,
    buffer: fmem::Buffer,
) -> anyhow::Result<()> {
    match configuration {
        TargetConfiguration::Single(configuration) => {
            // Device supports ABR or a specific configuration (ex. Recovery) was requested.
            paver_write_firmware(data_sink, configuration, type_, buffer).await?
        }
        TargetConfiguration::AB => {
            // For devices that do not support ABR. There will only be one single
            // partition for that firmware. The configuration parameter should be Configuration::A.
            let () = paver_write_firmware(
                data_sink,
                fpaver::Configuration::A,
                type_,
                clone_buffer(&buffer)?,
            )
            .await?;
            // Similar to asset, we also write Configuration::B to be forwards compatible with
            // devices that will eventually support ABR. For device that does not support A/B, it
            // will log/report WriteFirmwareResult::Unsupported and the paving  will be
            // skipped.
            paver_write_firmware(data_sink, fpaver::Configuration::B, type_, buffer).await?
        }
    };
    Ok(())
}

pub async fn write_image(
    data_sink: &fpaver::DataSinkProxy,
    buffer: fmem::Buffer,
    target_config: TargetConfiguration,
    image_type: super::ImageType<'_>,
) -> anyhow::Result<()> {
    use super::ImageType::*;
    match image_type {
        Asset(asset) => {
            write_asset_to_configurations(data_sink, target_config, asset, buffer).await
        }
        Firmware { type_ } => {
            write_firmware_to_configurations(data_sink, target_config, type_, buffer).await
        }
    }
}

pub fn connect_in_namespace() -> anyhow::Result<(fpaver::DataSinkProxy, fpaver::BootManagerProxy)> {
    let paver = fuchsia_component::client::connect_to_protocol::<fpaver::PaverMarker>()
        .context("connect to fuchsia.paver.Paver")?;

    let (data_sink, server_end) = fidl::endpoints::create_proxy::<fpaver::DataSinkMarker>();
    let () = paver.find_data_sink(server_end).context("connect to fuchsia.paver.DataSink")?;

    let (boot_manager, server_end) = fidl::endpoints::create_proxy::<fpaver::BootManagerMarker>();
    let () = paver.find_boot_manager(server_end).context("connect to fuchsia.paver.BootManager")?;

    Ok((data_sink, boot_manager))
}

/// Retrieve the currently-running configuration from the paver service (the configuration the
/// device booted from) which may be distinct from the 'active' configuration.
pub async fn query_current_configuration(
    boot_manager: &fpaver::BootManagerProxy,
) -> anyhow::Result<CurrentConfiguration> {
    match boot_manager.query_current_configuration().await {
        Ok(Ok(fpaver::Configuration::A)) => Ok(CurrentConfiguration::A),
        Ok(Ok(fpaver::Configuration::B)) => Ok(CurrentConfiguration::B),
        Ok(Ok(fpaver::Configuration::Recovery)) => Ok(CurrentConfiguration::Recovery),
        Ok(Err(status)) => {
            let status = zx::Status::err_from_raw(status);
            Err(anyhow!("query_current_configuration responded with {status:?}"))
        }
        Err(fidl::Error::ClientChannelClosed { epitaph, .. })
            if epitaph == zx::Status::NOT_SUPPORTED =>
        {
            warn!("device does not support ABR. Kernel image updates will not be atomic.");
            Ok(CurrentConfiguration::NotSupported)
        }
        Err(err) => Err(anyhow!(err).context("while performing query_current_configuration call")),
    }
}

async fn paver_query_configuration_status(
    boot_manager: &fpaver::BootManagerProxy,
    configuration: fpaver::Configuration,
) -> anyhow::Result<fpaver::ConfigurationStatus> {
    match boot_manager.query_configuration_status(configuration).await {
        Ok(Ok(configuration_status)) => Ok(configuration_status),
        Ok(Err(status)) => {
            let status = zx::Status::err_from_raw(status);
            Err(anyhow!("query_configuration_status responded with {status:?}"))
        }
        Err(err) => Err(anyhow!(err).context("while performing query_configuration_status call")),
    }
}

/// Error conditions possibly returned by prepare_partition_metadata.
#[derive(Debug, thiserror::Error)]
pub enum PreparePartitionMetadataError {
    #[error(
        "current configuration ({current_configuration:?}) is not Healthy \
        (status = {current_configuration_status:?}). Refusing to perform update."
    )]
    CurrentConfigurationUnhealthy {
        current_configuration: fpaver::Configuration,
        current_configuration_status: fpaver::ConfigurationStatus,
    },

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Ensure that the partition boot metadata is in a state where we're ready to write the update.
///
/// Specifically, this means:
/// - The current partition must be marked as Healthy. If it isn't, this function returns an error.
/// - The non-current partition must be marked as Unbootable. If it isn't, this function will mark
///   it as Unbootable and flush the BootManager.
///
/// As a side effect of marking non-current Unbootable, current will be made Active (if it wasn't
/// already).
///
/// If the result is OK, this function returns the current partition. The update should be written
/// to the current partition's respective non-current partition. If the result is an error, the
/// update should be aborted. Each time we return an error in the code below we justify why the
/// update shouldn't continue.
///
/// This operation can fail, and it is very important that all states it can end in (by returning)
/// don't end in a bricked device when the update continues. Some devices use BootManager's flush to
/// flush operations, and some don't. This function needs to support both strategies.
pub async fn prepare_partition_metadata(
    boot_manager: &fpaver::BootManagerProxy,
) -> Result<CurrentConfiguration, PreparePartitionMetadataError> {
    // ERROR JUSTIFICATION: If we can't get the current configuration, we won't know where to write
    // the update.
    let current = query_current_configuration(boot_manager)
        .await
        .context("while querying current configuration")?;

    let current_configuration = match current.to_configuration() {
        None => {
            info!("ABR not supported, no partition preparation necessary");
            return Ok(CurrentConfiguration::NotSupported);
        }
        Some(current_configuration) => current_configuration,
    };

    // ERROR JUSTIFICATION: We only want to write the update if `current` is `Healthy` (see below),
    // so if we can't determine its status, we don't know if we want to continue.
    let current_configuration_status =
        paver_query_configuration_status(boot_manager, current_configuration)
            .await
            .context("while querying current configuration status")?;

    match current_configuration_status {
        // It's the responsibility of the caller who triggered the update to ensure `current_config`
        // is Healthy (probably by calling `system_health_check::check_and_set_system_health()`
        // before we'll apply an update. If they didn't, bail out.
        //
        // ERROR JUSTIFICATION: We want to be confident the OTA will succeed before throwing away
        // our rollback target in the non-current configuration. (Note: In almost all circumstances,
        // the non-current configuration will be bootable at this point, because we never mark it
        // `Unbootable` until after `current_config` has been marked `Healthy`). Thus, we refuse to
        // update unless `current_config` has been marked `Healthy`.
        fpaver::ConfigurationStatus::Pending | fpaver::ConfigurationStatus::Unbootable => {
            Err(PreparePartitionMetadataError::CurrentConfigurationUnhealthy {
                current_configuration,
                current_configuration_status,
            })
        }
        // If the current configuration is Healthy, we mark the non-current configuration as
        // Unbootable.
        fpaver::ConfigurationStatus::Healthy => {
            // We need to flush the boot manager regardless, but if that operation succeeded while
            // set_non_current_configuration_unbootable failed, propagate the error.
            //
            // ERROR JUSTIFICATION: the non-current configuration might be active, meaning it'll be
            // used on the next reboot. If we don't mark it Unbootable, and we reboot halfway
            // through writing the update for some reason, we'll reboot into a totally broken
            // system.
            let set_unbootable_result = set_non_current_configuration_unbootable(
                boot_manager,
                current.to_non_current_configuration(),
            )
            .await
            .context("while setting non-current configuration unbootable");

            // ERROR JUSTIFICATION: Same as above; we want to make sure marking it Unbootable goes
            // through.
            let () = paver_flush_boot_manager(boot_manager)
                .await
                .context("while flushing boot manager")?;

            let () = set_unbootable_result?;
            Ok(current)
        }
    }
}

/// Sets an arbitrary configuration active. Not pub because it's possible to use this function to set a
/// current partition or recovery partition active, which is almost certainly not what external users want.
async fn paver_set_arbitrary_configuration_active(
    boot_manager: &fpaver::BootManagerProxy,
    configuration: fpaver::Configuration,
) -> anyhow::Result<()> {
    let status = boot_manager
        .set_configuration_active(configuration)
        .await
        .context("while performing set_configuration_active call")?;
    zx::Status::ok(status).context("set_configuration_active responded with")?;
    Ok(())
}

/// Sets the given desired `configuration` as active for subsequent boot attempts. If ABR is not
/// supported, do nothing.
pub async fn set_configuration_active(
    boot_manager: &fpaver::BootManagerProxy,
    desired_configuration: NonCurrentConfiguration,
) -> anyhow::Result<()> {
    if let Some(configuration) = desired_configuration.to_configuration() {
        return paver_set_arbitrary_configuration_active(boot_manager, configuration).await;
    }
    Ok(())
}

/// Set a non-current configuration as unbootable. Dangerous! If ABR is not supported, return an
/// error.
async fn set_non_current_configuration_unbootable(
    boot_manager: &fpaver::BootManagerProxy,
    configuration: NonCurrentConfiguration,
) -> anyhow::Result<()> {
    if let Some(configuration) = configuration.to_configuration() {
        set_arbitrary_configuration_unbootable(boot_manager, configuration).await
    } else {
        Err(anyhow!("could not set non-current configuration unbootable: {:?}", configuration))
    }
}

/// Set an arbitrary configuration as unbootable. Extra dangerous!
async fn set_arbitrary_configuration_unbootable(
    boot_manager: &fpaver::BootManagerProxy,
    configuration: fpaver::Configuration,
) -> anyhow::Result<()> {
    let status = boot_manager
        .set_configuration_unbootable(configuration)
        .await
        .context("while performing set_configuration_unbootable call")?;
    zx::Status::ok(status).context("set_configuration_unbootable responded with")?;
    Ok(())
}

/// Sets the recovery configuration as the only valid boot option by marking the A and B
/// configurations unbootable.
pub async fn set_recovery_configuration_active(
    boot_manager: &fpaver::BootManagerProxy,
) -> anyhow::Result<()> {
    let () = set_arbitrary_configuration_unbootable(boot_manager, fpaver::Configuration::A)
        .await
        .context("while marking Configuration::A unbootable")?;
    let () = set_arbitrary_configuration_unbootable(boot_manager, fpaver::Configuration::B)
        .await
        .context("while marking Configuration::B unbootable")?;
    Ok(())
}

pub async fn paver_flush_boot_manager(
    boot_manager: &fpaver::BootManagerProxy,
) -> anyhow::Result<()> {
    let () = zx::Status::ok(
        boot_manager
            .flush()
            .await
            .context("while performing fuchsia.paver.BootManager/Flush call")?,
    )
    .context("fuchsia.paver.BootManager/Flush responded with")?;
    Ok(())
}

pub async fn paver_flush_data_sink(data_sink: &fpaver::DataSinkProxy) -> anyhow::Result<()> {
    let () = zx::Status::ok(
        data_sink.flush().await.context("while performing fuchsia.paver.DataSink/Flush call")?,
    )
    .context("fuchsia.paver.DataSink/Flush responded with")?;
    Ok(())
}

#[cfg(test)]
fn make_buffer(contents: impl AsRef<[u8]>) -> fmem::Buffer {
    let contents = contents.as_ref();
    let size = contents.len().try_into().unwrap();

    let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, size).unwrap();
    vmo.write(contents, 0).unwrap();

    fmem::Buffer { vmo, size }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use mock_paver::{MockPaverServiceBuilder, PaverEvent, hooks as mphooks};
    use std::sync::Arc;

    #[fuchsia::test]
    async fn query_non_current_configuration_with_a_current() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new().current_config(fpaver::Configuration::A).build(),
        );
        let boot_manager = paver.spawn_boot_manager_service();

        assert_eq!(
            query_current_configuration(&boot_manager)
                .await
                .unwrap()
                .to_non_current_configuration(),
            NonCurrentConfiguration::B,
        );

        assert_eq!(paver.take_events(), vec![PaverEvent::QueryCurrentConfiguration]);
    }

    #[fuchsia::test]
    async fn query_non_current_configuration_with_b_current() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new().current_config(fpaver::Configuration::B).build(),
        );
        let boot_manager = paver.spawn_boot_manager_service();

        assert_eq!(
            query_current_configuration(&boot_manager)
                .await
                .unwrap()
                .to_non_current_configuration(),
            NonCurrentConfiguration::A,
        );

        assert_eq!(paver.take_events(), vec![PaverEvent::QueryCurrentConfiguration]);
    }

    #[fuchsia::test]
    async fn query_non_current_configuration_with_r_current() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new().current_config(fpaver::Configuration::Recovery).build(),
        );
        let boot_manager = paver.spawn_boot_manager_service();

        assert_eq!(
            query_current_configuration(&boot_manager)
                .await
                .unwrap()
                .to_non_current_configuration(),
            NonCurrentConfiguration::A, // We default to A
        );

        assert_eq!(paver.take_events(), vec![PaverEvent::QueryCurrentConfiguration]);
    }

    #[fuchsia::test]
    async fn set_configuration_active_makes_call() {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let boot_manager = paver.spawn_boot_manager_service();

        set_configuration_active(&boot_manager, NonCurrentConfiguration::B).await.unwrap();

        assert_eq!(
            paver.take_events(),
            vec![PaverEvent::SetConfigurationActive { configuration: fpaver::Configuration::B }]
        );
    }

    #[fuchsia::test]
    async fn set_recovery_configuration_active_makes_calls() {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let boot_manager = paver.spawn_boot_manager_service();

        set_recovery_configuration_active(&boot_manager).await.unwrap();

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::SetConfigurationUnbootable { configuration: fpaver::Configuration::A },
                PaverEvent::SetConfigurationUnbootable { configuration: fpaver::Configuration::B },
            ]
        );
    }

    #[fuchsia::test]
    async fn set_arbitrary_configuration_active_makes_call() {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let boot_manager = paver.spawn_boot_manager_service();

        paver_set_arbitrary_configuration_active(&boot_manager, fpaver::Configuration::A)
            .await
            .unwrap();

        assert_eq!(
            paver.take_events(),
            vec![PaverEvent::SetConfigurationActive { configuration: fpaver::Configuration::A }]
        );
    }

    #[fuchsia::test]
    async fn query_configuration_status_makes_call() {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let boot_manager = paver.spawn_boot_manager_service();

        paver_query_configuration_status(&boot_manager, fpaver::Configuration::B).await.unwrap();

        assert_eq!(
            paver.take_events(),
            vec![PaverEvent::QueryConfigurationStatus { configuration: fpaver::Configuration::B }]
        );
    }

    #[fuchsia::test]
    async fn flush_boot_manager_makes_call() {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let boot_manager = paver.spawn_boot_manager_service();

        paver_flush_boot_manager(&boot_manager).await.unwrap();

        assert_eq!(paver.take_events(), vec![PaverEvent::BootManagerFlush]);
    }

    async fn assert_prepare_partition_metadata_bails_out_with_unhealthy_current(
        current_config: fpaver::Configuration,
        status: fpaver::ConfigurationStatus,
    ) {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .current_config(current_config)
                .insert_hook(mphooks::config_status(move |_| Ok(status)))
                .build(),
        );
        let boot_manager = paver.spawn_boot_manager_service();

        assert_matches!(prepare_partition_metadata(&boot_manager).await,
            Err(PreparePartitionMetadataError::CurrentConfigurationUnhealthy{
                current_configuration: cc,
                current_configuration_status: ccs,
            })
            if cc == current_config && ccs == status
        );
        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::QueryCurrentConfiguration,
                PaverEvent::QueryConfigurationStatus { configuration: current_config },
            ]
        );
    }

    #[fuchsia::test]
    async fn prepare_partition_metadata_bails_out_if_current_pending_a() {
        assert_prepare_partition_metadata_bails_out_with_unhealthy_current(
            fpaver::Configuration::A,
            fpaver::ConfigurationStatus::Pending,
        )
        .await;
    }

    #[fuchsia::test]
    async fn prepare_partition_metadata_bails_out_if_current_unbootable_a() {
        assert_prepare_partition_metadata_bails_out_with_unhealthy_current(
            fpaver::Configuration::A,
            fpaver::ConfigurationStatus::Unbootable,
        )
        .await;
    }

    #[fuchsia::test]
    async fn prepare_partition_metadata_bails_out_if_current_pending_b() {
        assert_prepare_partition_metadata_bails_out_with_unhealthy_current(
            fpaver::Configuration::B,
            fpaver::ConfigurationStatus::Pending,
        )
        .await;
    }

    #[fuchsia::test]
    async fn prepare_partition_metadata_bails_out_if_current_unbootable_b() {
        assert_prepare_partition_metadata_bails_out_with_unhealthy_current(
            fpaver::Configuration::B,
            fpaver::ConfigurationStatus::Unbootable,
        )
        .await;
    }

    async fn assert_successful_prepare_partition_metadata(
        current_config: fpaver::Configuration,
        target_config: fpaver::Configuration,
    ) {
        let paver = Arc::new(MockPaverServiceBuilder::new().current_config(current_config).build());
        let boot_manager = paver.spawn_boot_manager_service();

        prepare_partition_metadata(&boot_manager).await.unwrap();
        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::QueryCurrentConfiguration,
                PaverEvent::QueryConfigurationStatus { configuration: current_config },
                PaverEvent::SetConfigurationUnbootable { configuration: target_config },
                PaverEvent::BootManagerFlush,
            ]
        );
    }

    #[fuchsia::test]
    async fn prepare_partition_metadata_targets_b_in_config_a() {
        assert_successful_prepare_partition_metadata(
            fpaver::Configuration::A,
            fpaver::Configuration::B,
        )
        .await;
    }

    #[fuchsia::test]
    async fn prepare_partition_metadata_targets_a_in_config_b() {
        assert_successful_prepare_partition_metadata(
            fpaver::Configuration::B,
            fpaver::Configuration::A,
        )
        .await;
    }

    #[fuchsia::test]
    async fn prepare_partition_metadata_targets_a_in_config_r() {
        assert_successful_prepare_partition_metadata(
            fpaver::Configuration::Recovery,
            fpaver::Configuration::A,
        )
        .await;
    }

    #[fuchsia::test]
    async fn prepare_partition_metadata_does_nothing_if_abr_not_supported() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .boot_manager_close_with_epitaph(zx::Status::NOT_SUPPORTED)
                .build(),
        );
        let boot_manager = paver.spawn_boot_manager_service();

        prepare_partition_metadata(&boot_manager).await.unwrap();
        assert_eq!(paver.take_events(), vec![]);
    }

    #[fuchsia::test]
    async fn write_image_writes_firmware() {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let data_sink = paver.spawn_data_sink_service();

        write_image(
            &data_sink,
            make_buffer("firmware contents"),
            TargetConfiguration::Single(fpaver::Configuration::A),
            super::super::ImageType::Firmware { type_: "" },
        )
        .await
        .unwrap();

        write_image(
            &data_sink,
            make_buffer("firmware_foo contents"),
            TargetConfiguration::Single(fpaver::Configuration::B),
            super::super::ImageType::Firmware { type_: "foo" },
        )
        .await
        .unwrap();

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::WriteFirmware {
                    configuration: fpaver::Configuration::A,
                    firmware_type: "".to_owned(),
                    payload: b"firmware contents".to_vec()
                },
                PaverEvent::WriteFirmware {
                    configuration: fpaver::Configuration::B,
                    firmware_type: "foo".to_owned(),
                    payload: b"firmware_foo contents".to_vec()
                }
            ]
        );
    }

    #[fuchsia::test]
    async fn write_image_ignores_unsupported_firmware() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .insert_hook(mphooks::write_firmware(|_, _, _| {
                    fpaver::WriteFirmwareResult::Unsupported(true)
                }))
                .build(),
        );
        let data_sink = paver.spawn_data_sink_service();

        write_image(
            &data_sink,
            make_buffer("firmware of the future!"),
            TargetConfiguration::Single(fpaver::Configuration::A),
            super::super::ImageType::Firmware { type_: "unknown" },
        )
        .await
        .unwrap();

        assert_eq!(
            paver.take_events(),
            vec![PaverEvent::WriteFirmware {
                configuration: fpaver::Configuration::A,
                firmware_type: "unknown".to_owned(),
                payload: b"firmware of the future!".to_vec()
            },]
        );
    }

    #[fuchsia::test]
    async fn write_image_forwards_other_firmware_errors() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .insert_hook(mphooks::write_firmware(|_, _, _| {
                    fpaver::WriteFirmwareResult::Status(zx::Status::INTERNAL.into_raw())
                }))
                .build(),
        );
        let data_sink = paver.spawn_data_sink_service();

        assert_matches!(
            write_image(
                &data_sink,
                make_buffer("oops"),
                TargetConfiguration::Single(fpaver::Configuration::A),
                super::super::ImageType::Firmware{type_: ""},
            )
            .await,
            Err(e) if e.to_string().contains("firmware failed to write")
        );

        assert_eq!(
            paver.take_events(),
            vec![PaverEvent::WriteFirmware {
                configuration: fpaver::Configuration::A,
                firmware_type: "".to_owned(),
                payload: b"oops".to_vec()
            }]
        );
    }

    #[fuchsia::test]
    async fn write_image_writes_asset_to_single_config() {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let data_sink = paver.spawn_data_sink_service();

        write_image(
            &data_sink,
            make_buffer("zbi contents"),
            TargetConfiguration::Single(fpaver::Configuration::A),
            super::super::ImageType::Asset(fpaver::Asset::Kernel),
        )
        .await
        .unwrap();

        assert_eq!(
            paver.take_events(),
            vec![PaverEvent::WriteAsset {
                configuration: fpaver::Configuration::A,
                asset: fpaver::Asset::Kernel,
                payload: b"zbi contents".to_vec()
            }]
        );
    }

    #[fuchsia::test]
    async fn write_image_write_asset_forwards_errors() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .insert_hook(mphooks::return_error(|event| match event {
                    PaverEvent::WriteAsset { .. } => Err(zx::Status::INTERNAL),
                    _ => panic!("Unexpected event: {event:?}"),
                }))
                .build(),
        );
        let data_sink = paver.spawn_data_sink_service();

        assert_matches!(
            write_image(
                &data_sink,
                make_buffer("zbi contents"),
                TargetConfiguration::Single(fpaver::Configuration::A),
                super::super::ImageType::Asset(fpaver::Asset::Kernel)
            )
            .await,
            Err(e) if e.to_string().contains("write_asset responded with")
        );

        assert_matches!(
            write_image(
                &data_sink,
                make_buffer("vbmeta contents"),
                TargetConfiguration::Single(fpaver::Configuration::A),
                super::super::ImageType::Asset(fpaver::Asset::VerifiedBootMetadata)
            )
            .await,
            Err(e) if e.to_string().contains("write_asset responded with")
        );

        assert_matches!(
            write_image(
                &data_sink,
                make_buffer("zbi contents"),
                TargetConfiguration::Single(fpaver::Configuration::B),
                super::super::ImageType::Asset(fpaver::Asset::Kernel)
            )
            .await,
            Err(e) if e.to_string().contains("write_asset responded with")
        );

        assert_matches!(
            write_image(
                &data_sink,
                make_buffer("vbmeta contents"),
                TargetConfiguration::Single(fpaver::Configuration::B),
                super::super::ImageType::Asset(fpaver::Asset::VerifiedBootMetadata)
            )
            .await,
            Err(e) if e.to_string().contains("write_asset responded with")
        );
    }

    #[fuchsia::test]
    async fn paver_flush_boot_manager_makes_calls() {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let boot_manager = paver.spawn_boot_manager_service();

        paver_flush_boot_manager(&boot_manager).await.unwrap();
        assert_eq!(paver.take_events(), vec![PaverEvent::BootManagerFlush,]);
    }
}

#[cfg(test)]
mod abr_not_supported_tests {
    use super::*;
    use assert_matches::assert_matches;
    use mock_paver::{MockPaverServiceBuilder, PaverEvent, hooks as mphooks};
    use std::sync::Arc;

    #[fuchsia::test]
    async fn query_non_current_configuration_returns_not_supported() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .boot_manager_close_with_epitaph(zx::Status::NOT_SUPPORTED)
                .build(),
        );
        let boot_manager = paver.spawn_boot_manager_service();

        assert_eq!(
            query_current_configuration(&boot_manager)
                .await
                .unwrap()
                .to_non_current_configuration(),
            NonCurrentConfiguration::NotSupported
        );

        assert_eq!(paver.take_events(), vec![]);
    }

    #[fuchsia::test]
    async fn query_current_configuration_returns_not_supported() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .boot_manager_close_with_epitaph(zx::Status::NOT_SUPPORTED)
                .build(),
        );
        let boot_manager = paver.spawn_boot_manager_service();

        assert_eq!(
            query_current_configuration(&boot_manager).await.unwrap(),
            CurrentConfiguration::NotSupported
        );

        assert_eq!(paver.take_events(), vec![]);
    }

    #[fuchsia::test]
    async fn set_configuration_active_is_noop() {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let boot_manager = paver.spawn_boot_manager_service();

        set_configuration_active(&boot_manager, NonCurrentConfiguration::NotSupported)
            .await
            .unwrap();

        assert_eq!(paver.take_events(), vec![]);
    }

    #[fuchsia::test]
    async fn write_image_writes_asset_to_both_configs() {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let data_sink = paver.spawn_data_sink_service();

        write_image(
            &data_sink,
            make_buffer("zbi contents"),
            TargetConfiguration::AB,
            super::super::ImageType::Asset(fpaver::Asset::Kernel),
        )
        .await
        .unwrap();

        write_image(
            &data_sink,
            make_buffer("the new vbmeta"),
            TargetConfiguration::AB,
            super::super::ImageType::Asset(fpaver::Asset::VerifiedBootMetadata),
        )
        .await
        .unwrap();

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::WriteAsset {
                    configuration: fpaver::Configuration::A,
                    asset: fpaver::Asset::Kernel,
                    payload: b"zbi contents".to_vec()
                },
                PaverEvent::WriteAsset {
                    configuration: fpaver::Configuration::B,
                    asset: fpaver::Asset::Kernel,
                    payload: b"zbi contents".to_vec()
                },
                PaverEvent::WriteAsset {
                    configuration: fpaver::Configuration::A,
                    asset: fpaver::Asset::VerifiedBootMetadata,
                    payload: b"the new vbmeta".to_vec()
                },
                PaverEvent::WriteAsset {
                    configuration: fpaver::Configuration::B,
                    asset: fpaver::Asset::VerifiedBootMetadata,
                    payload: b"the new vbmeta".to_vec()
                },
            ]
        );
    }

    #[fuchsia::test]
    async fn write_image_writes_firmware_to_both_configs() {
        let paver = Arc::new(MockPaverServiceBuilder::new().build());
        let data_sink = paver.spawn_data_sink_service();

        write_image(
            &data_sink,
            make_buffer("firmware contents"),
            TargetConfiguration::AB,
            super::super::ImageType::Firmware { type_: "" },
        )
        .await
        .unwrap();

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::WriteFirmware {
                    configuration: fpaver::Configuration::A,
                    firmware_type: "".to_owned(),
                    payload: b"firmware contents".to_vec()
                },
                PaverEvent::WriteFirmware {
                    configuration: fpaver::Configuration::B,
                    firmware_type: "".to_owned(),
                    payload: b"firmware contents".to_vec()
                },
            ]
        );
    }

    #[fuchsia::test]
    async fn write_image_forwards_config_a_error() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .insert_hook(mphooks::return_error(|event| match event {
                    PaverEvent::WriteAsset {
                        configuration: fpaver::Configuration::A,
                        asset: fpaver::Asset::Kernel,
                        payload: _,
                    } => Err(zx::Status::INTERNAL),
                    _ => Ok(()),
                }))
                .build(),
        );
        let data_sink = paver.spawn_data_sink_service();

        assert_matches!(
            write_image(
                &data_sink,
                make_buffer("zbi contents"),
                TargetConfiguration::AB,
                super::super::ImageType::Asset(fpaver::Asset::Kernel)
            )
            .await,
            Err(_)
        );

        assert_eq!(
            paver.take_events(),
            vec![PaverEvent::WriteAsset {
                configuration: fpaver::Configuration::A,
                asset: fpaver::Asset::Kernel,
                payload: b"zbi contents".to_vec()
            },]
        );
    }

    #[fuchsia::test]
    async fn write_image_ignores_config_b_not_supported_error() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .insert_hook(mphooks::return_error(|event| match event {
                    PaverEvent::WriteAsset {
                        configuration: fpaver::Configuration::B,
                        asset: fpaver::Asset::Kernel,
                        payload: _,
                    } => Err(zx::Status::NOT_SUPPORTED),
                    _ => Ok(()),
                }))
                .build(),
        );
        let data_sink = paver.spawn_data_sink_service();

        write_image(
            &data_sink,
            make_buffer("zbi contents"),
            TargetConfiguration::AB,
            super::super::ImageType::Asset(fpaver::Asset::Kernel),
        )
        .await
        .unwrap();

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::WriteAsset {
                    configuration: fpaver::Configuration::A,
                    asset: fpaver::Asset::Kernel,
                    payload: b"zbi contents".to_vec()
                },
                PaverEvent::WriteAsset {
                    configuration: fpaver::Configuration::B,
                    asset: fpaver::Asset::Kernel,
                    payload: b"zbi contents".to_vec()
                },
            ]
        );
    }

    #[fuchsia::test]
    async fn write_image_write_firmware_ignores_unsupported_config_b() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .insert_hook(mphooks::write_firmware(|configuration, _, _| match configuration {
                    fpaver::Configuration::B => fpaver::WriteFirmwareResult::Unsupported(true),
                    _ => fpaver::WriteFirmwareResult::Status(zx::sys::ZX_OK),
                }))
                .build(),
        );
        let data_sink = paver.spawn_data_sink_service();

        write_image(
            &data_sink,
            make_buffer("firmware contents"),
            TargetConfiguration::AB,
            super::super::ImageType::Firmware { type_: "" },
        )
        .await
        .unwrap();

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::WriteFirmware {
                    configuration: fpaver::Configuration::A,
                    firmware_type: "".to_owned(),
                    payload: b"firmware contents".to_vec()
                },
                PaverEvent::WriteFirmware {
                    configuration: fpaver::Configuration::B,
                    firmware_type: "".to_owned(),
                    payload: b"firmware contents".to_vec()
                },
            ]
        );
    }

    #[fuchsia::test]
    async fn write_image_forwards_other_config_b_errors() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .insert_hook(mphooks::return_error(|event| match event {
                    PaverEvent::WriteAsset {
                        configuration: fpaver::Configuration::B,
                        asset: fpaver::Asset::Kernel,
                        ..
                    } => Err(zx::Status::INTERNAL),
                    _ => Ok(()),
                }))
                .build(),
        );
        let data_sink = paver.spawn_data_sink_service();

        assert_matches!(
            write_image(
                &data_sink,
                make_buffer("zbi contents"),
                TargetConfiguration::AB,
                super::super::ImageType::Asset(fpaver::Asset::Kernel)
            )
            .await,
            Err(_)
        );

        assert_eq!(
            paver.take_events(),
            vec![
                PaverEvent::WriteAsset {
                    configuration: fpaver::Configuration::A,
                    asset: fpaver::Asset::Kernel,
                    payload: b"zbi contents".to_vec()
                },
                PaverEvent::WriteAsset {
                    configuration: fpaver::Configuration::B,
                    asset: fpaver::Asset::Kernel,
                    payload: b"zbi contents".to_vec()
                },
            ]
        );
    }

    #[fuchsia::test]
    async fn paver_flush_boot_manager_doesnt_makes_calls() {
        let paver = Arc::new(
            MockPaverServiceBuilder::new()
                .boot_manager_close_with_epitaph(zx::Status::NOT_SUPPORTED)
                .build(),
        );

        let boot_manager = paver.spawn_boot_manager_service();

        assert_matches!(paver_flush_boot_manager(&boot_manager).await, Err(_));
    }
}
