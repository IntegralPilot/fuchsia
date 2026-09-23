// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_fxfs::{BlobCreatorMarker, BlobReaderMarker};
use fshost_assembly_config;
use fuchsia_component_test::{Capability, ChildOptions, ChildRef, RealmBuilder, Ref, Route};
use futures::future::FutureExt as _;
use std::collections::HashMap;

use fidl_fuchsia_fshost as ffshost;
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_hardware_block_volume as fvolume;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_logger as flogger;
use fidl_fuchsia_process as fprocess;
use fidl_fuchsia_storage_partitions as fpartitions;
use fidl_fuchsia_update_verify as ffuv;

pub trait IntoValueSpec {
    fn into_value_spec(self) -> cm_rust::ConfigValueSpec;
}

impl IntoValueSpec for bool {
    fn into_value_spec(self) -> cm_rust::ConfigValueSpec {
        cm_rust::ConfigValueSpec {
            value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(self)),
        }
    }
}

impl IntoValueSpec for u64 {
    fn into_value_spec(self) -> cm_rust::ConfigValueSpec {
        cm_rust::ConfigValueSpec {
            value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Uint64(self)),
        }
    }
}

impl IntoValueSpec for String {
    fn into_value_spec(self) -> cm_rust::ConfigValueSpec {
        cm_rust::ConfigValueSpec {
            value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::String(self.into())),
        }
    }
}

impl<'a> IntoValueSpec for &'a str {
    fn into_value_spec(self) -> cm_rust::ConfigValueSpec {
        self.to_string().into_value_spec()
    }
}

/// Builder for the fshost component. This handles configuring the fshost component to use and
/// structured config overrides to set, as well as setting up the expected protocols to be routed
/// between the realm builder root and the fshost child when the test realm is built.
///
/// Any desired additional config overrides should be added to this builder. New routes for exposed
/// capabilities from the fshost component or offered capabilities to the fshost component should
/// be added to the [`FshostBuilder::build`] function below.
#[derive(Debug, Clone)]
pub struct FshostBuilder {
    component_name: &'static str,
    config_values: HashMap<&'static str, cm_rust::ConfigValueSpec>,
    create_starnix_volume_crypt: bool,
    block_device_config_json: String,
    crypt_policy: crypt_policy::Policy,
}

impl FshostBuilder {
    pub fn new(component_name: &'static str) -> FshostBuilder {
        FshostBuilder {
            component_name,
            config_values: HashMap::new(),
            create_starnix_volume_crypt: false,
            block_device_config_json: String::from("[]"),
            crypt_policy: crypt_policy::Policy::Null,
        }
    }

    pub fn create_starnix_volume_crypt(&mut self) -> &mut Self {
        self.create_starnix_volume_crypt = true;
        self
    }

    pub fn set_config_value(&mut self, key: &'static str, value: impl IntoValueSpec) -> &mut Self {
        assert!(
            self.config_values.insert(key, value.into_value_spec()).is_none(),
            "Attempted to insert duplicate config value '{}'!",
            key
        );
        self
    }

    pub fn set_device_config(
        &mut self,
        config: Vec<fshost_assembly_config::BlockDeviceConfig>,
    ) -> &mut Self {
        self.block_device_config_json = serde_json::to_string(&config).unwrap();
        self
    }

    pub fn set_crypt_policy(&mut self, policy: crypt_policy::Policy) -> &mut Self {
        self.crypt_policy = policy;
        self
    }

    pub async fn build(mut self, realm_builder: &RealmBuilder) -> ChildRef {
        let fshost_url = format!("#meta/{}.cm", self.component_name);
        log::info!(fshost_url:%; "building test fshost instance");
        let fshost =
            realm_builder.add_child("test-fshost", fshost_url, ChildOptions::new()).await.unwrap();

        let bootfs = vfs::pseudo_directory! {
            "boot" => vfs::pseudo_directory! {
                "config" => vfs::pseudo_directory! {
                    "fshost" => vfs::file::read_only(&self.block_device_config_json),
                    "zxcrypt" => vfs::file::read_only(&format!("{}", self.crypt_policy)),
                },
            },
        };
        let bootfs = realm_builder
            .add_local_child(
                "bootfs",
                move |handles| {
                    let bootfs = bootfs.clone();
                    async move {
                        let scope = vfs::ExecutionScope::new();
                        vfs::directory::serve_on(
                            bootfs,
                            fio::PERM_READABLE,
                            scope.clone(),
                            handles.outgoing_dir,
                        );
                        scope.wait().await;
                        Ok(())
                    }
                    .boxed()
                },
                ChildOptions::new(),
            )
            .await
            .unwrap();
        realm_builder
            .add_route(
                Route::new()
                    .capability(Capability::directory("boot").rights(fio::R_STAR_DIR).path("/boot"))
                    .from(&bootfs)
                    .to(&fshost),
            )
            .await
            .unwrap();

        // This is a map from config keys to configuration capability names.
        let mut map = HashMap::from([
            ("no_zxcrypt", "fuchsia.fshost.NoZxcrypt"),
            ("ramdisk_image", "fuchsia.fshost.RamdiskImage"),
            ("gpt_all", "fuchsia.fshost.GptAll"),
            ("check_filesystems", "fuchsia.fshost.CheckFilesystems"),
            ("blob_max_bytes", "fuchsia.fshost.BlobMaxBytes"),
            ("data_max_bytes", "fuchsia.fshost.DataMaxBytes"),
            ("format_data_on_corruption", "fuchsia.fshost.FormatDataOnCorruption"),
            ("data_filesystem_format", "fuchsia.fshost.DataFilesystemFormat"),
            ("blobfs", "fuchsia.fshost.Blobfs"),
            ("factory", "fuchsia.fshost.Factory"),
            ("fvm", "fuchsia.fshost.Fvm"),
            ("gpt", "fuchsia.fshost.Gpt"),
            ("merge_super_and_userdata", "fuchsia.fshost.MergeSuperAndUserdata"),
            ("data", "fuchsia.fshost.Data"),
            ("disable_block_watcher", "fuchsia.fshost.DisableBlockWatcher"),
            ("fvm_slice_size", "fuchsia.fshost.FvmSliceSize"),
            ("blobfs_initial_inodes", "fuchsia.fshost.BlobfsInitialInodes"),
            (
                "blobfs_use_deprecated_padded_format",
                "fuchsia.fshost.BlobfsUseDeprecatedPaddedFormat",
            ),
            ("fxfs_blob", "fuchsia.fshost.FxfsBlob"),
            ("fxfs_crypt_url", "fuchsia.fshost.FxfsCryptUrl"),
            ("disable_automount", "fuchsia.fshost.DisableAutomount"),
            ("starnix_volume_name", "fuchsia.fshost.StarnixVolumeName"),
            ("inline_crypto", "fuchsia.fshost.InlineCrypto"),
            ("provision_fxfs", "fuchsia.fshost.ProvisionFxfs"),
            ("watch_deprecated_v1_drivers", "fuchsia.fshost.WatchDeprecatedV1Drivers"),
        ]);

        if self.create_starnix_volume_crypt {
            let user_fxfs_crypt = realm_builder
                .add_child("user_fxfs_crypt", "#meta/fxfs-crypt.cm", ChildOptions::new().eager())
                .await
                .unwrap();
            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol::<ffxfs::CryptMarker>())
                        .capability(Capability::protocol::<ffxfs::CryptManagementMarker>())
                        .from(&user_fxfs_crypt)
                        .to(Ref::parent()),
                )
                .await
                .unwrap();
        }

        // Add the overrides as capabilities and route them.
        self.config_values.insert("fxfs_crypt_url", "#meta/fxfs-crypt.cm".into_value_spec());
        for (key, value) in self.config_values {
            let cap_name = map[key];
            realm_builder
                .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                    name: cap_name.parse().unwrap(),
                    value: value.value,
                }))
                .await
                .unwrap();
            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::configuration(cap_name))
                        .from(Ref::self_())
                        .to(&fshost),
                )
                .await
                .unwrap();
            map.remove(key);
        }

        // Add the remaining keys from the config component.
        let fshost_config_url = format!("#meta/{}_config.cm", self.component_name);
        let fshost_config = realm_builder
            .add_child("test-fshost-config", fshost_config_url, ChildOptions::new().eager())
            .await
            .unwrap();
        for (_, value) in map.iter() {
            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::configuration(*value))
                        .from(&fshost_config)
                        .to(&fshost),
                )
                .await
                .unwrap();
        }

        realm_builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<ffshost::AdminMarker>())
                    .capability(Capability::protocol::<ffshost::RecoveryMarker>())
                    .capability(Capability::protocol::<ffuv::ComponentOtaHealthCheckMarker>())
                    .capability(Capability::protocol::<ffshost::StarnixVolumeProviderMarker>())
                    .capability(Capability::protocol::<fpartitions::PartitionsManagerMarker>())
                    .capability(Capability::protocol::<BlobCreatorMarker>())
                    .capability(Capability::protocol::<BlobReaderMarker>())
                    .capability(Capability::directory("blob").rights(fio::RW_STAR_DIR))
                    .capability(
                        Capability::directory("blob-exec")
                            .rights(fio::RW_STAR_DIR | fio::Operations::EXECUTE),
                    )
                    .capability(Capability::directory("block").rights(fio::R_STAR_DIR))
                    .capability(Capability::directory("debug_block").rights(fio::R_STAR_DIR))
                    .capability(Capability::directory("data").rights(fio::RW_STAR_DIR))
                    .capability(Capability::directory("tmp").rights(fio::RW_STAR_DIR))
                    .capability(Capability::directory("volumes").rights(fio::RW_STAR_DIR))
                    .capability(Capability::service::<fpartitions::PartitionServiceMarker>())
                    .capability(Capability::service::<fvolume::ServiceMarker>())
                    .from(&fshost)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();

        realm_builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<flogger::LogSinkMarker>())
                    .capability(Capability::protocol::<fprocess::LauncherMarker>())
                    .from(Ref::parent())
                    .to(&fshost),
            )
            .await
            .unwrap();

        realm_builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::protocol_by_name("fuchsia.scheduler.RoleManager").optional(),
                    )
                    .capability(
                        Capability::protocol_by_name("fuchsia.tracing.provider.Registry")
                            .optional(),
                    )
                    .capability(
                        Capability::protocol_by_name("fuchsia.memorypressure.Provider").optional(),
                    )
                    .from(Ref::void())
                    .to(&fshost),
            )
            .await
            .unwrap();

        fshost
    }
}
