// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fidl_fuchsia_update_installer_ext::{
    Progress, StageFailureReason, State, StateId, UpdateInfo, UpdateInfoAndProgress,
};
use maplit::btreemap;
use pretty_assertions::assert_eq;

#[fuchsia::test]
async fn images_manifest_update_package_firmware_no_match() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .firmware_package(btreemap! {
            "".to_owned() =>
                ::update_package::ImageMetadata::new(
                    5,
                    sha256(5),
                    image_package_resource_url("update-images-firmware", 5, "a")
                ),
        })
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                0,
                EMPTY_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_firmware(|_, _| Ok(b"not a match".to_vec())))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver
        .url(image_package_url_to_string("update-images-firmware", 5))
        .resolve(&env.resolver.package("firmware", hashstr(7)).add_file("a", "_ contents"));

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        // Events we really care about testing
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::A,
            firmware_type: "".to_string(),
        }),
        ReplaceRetainedPackages(vec![hashstr(5).parse().unwrap()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-firmware", 5)),
        Paver(PaverEvent::WriteFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "".to_string(),
            payload: b"_ contents".to_vec(),
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn images_manifest_firmware_no_match_packageless() {
    let firmware_content = b"_ contents";
    let firmware_hash = fuchsia_merkle::root_from_slice(firmware_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Firmware("".to_string()),
                blob: manifest::Blob {
                    uncompressed_size: firmware_content.len() as u64,
                    fuchsia_merkle_root: firmware_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_firmware(|_, _| Ok(b"not a match".to_vec())))
        })
        .ota_manifest(manifest)
        .blob(firmware_hash, firmware_content.to_vec())
        .build()
        .await;
    env.run_packageless_update().await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions()
            .chain([ReplaceRetainedBlobs(vec![empty_merkle().into(), firmware_hash.into()]), Gc]),
        [
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            // Events we really care about testing
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "".to_string(),
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::A,
                firmware_type: "".to_string(),
            }),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(firmware_hash.into())),
            Paver(PaverEvent::WriteFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "".to_string(),
                payload: b"_ contents".to_vec(),
            }),
        ],
        [
            Paver(PaverEvent::DataSinkFlush),
            ReplaceRetainedBlobs(vec![]),
            Gc,
            BlobfsSync,
            Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
            Paver(PaverEvent::BootManagerFlush),
            Reboot,
        ],
    );
}

#[fuchsia::test]
async fn images_manifest_update_package_firmware_match_desired_config() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .firmware_package(btreemap! {
            "".to_owned() => ::update_package::ImageMetadata::new(
                8,
                MATCHING_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-firmware", 6, "a")
            ),
        })
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                0,
                EMPTY_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_firmware(|_, _| Ok(b"matching".to_vec())))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        // Events we really care about testing
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "".to_string(),
        }),
        // rest of the events.
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn images_manifest_firmware_match_desired_config_packageless() {
    let firmware_content = b"matching";
    let firmware_hash = fuchsia_merkle::root_from_slice(firmware_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Firmware("".to_string()),
                blob: manifest::Blob {
                    uncompressed_size: firmware_content.len() as u64,
                    fuchsia_merkle_root: firmware_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_firmware(|_, _| Ok(b"matching".to_vec())))
        })
        .ota_manifest(manifest)
        .build()
        .await;
    env.run_packageless_update().await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions()
            .chain([ReplaceRetainedBlobs(vec![empty_merkle().into(), firmware_hash.into()]), Gc]),
        [
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            // Events we really care about testing
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "".to_string(),
            }),
        ],
        [
            // rest of the events.
            Paver(PaverEvent::DataSinkFlush),
            ReplaceRetainedBlobs(vec![]),
            Gc,
            BlobfsSync,
            Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
            Paver(PaverEvent::BootManagerFlush),
            Reboot,
        ],
    );
}

#[fuchsia::test]
async fn images_manifest_update_package_firmware_match_active_config() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .firmware_package(btreemap! {
            "".to_owned() => ::update_package::ImageMetadata::new(
                8,
                MATCHING_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-firmware", 6, "a")
            ),
        })
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                0,
                EMPTY_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_firmware(|configuration, _| match configuration {
                paver::Configuration::A => Ok(b"matching".to_vec()),
                _ => Ok(b"no match".to_vec()),
            }))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        // Events we really care about testing
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::A,
            firmware_type: "".to_string(),
        }),
        Paver(PaverEvent::WriteFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "".to_string(),
            payload: b"matching".to_vec(),
        }),
        // rest of the events.
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn images_manifest_firmware_match_active_config_packageless() {
    let firmware_content = b"matching";
    let firmware_hash = fuchsia_merkle::root_from_slice(firmware_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Firmware("".to_string()),
                blob: manifest::Blob {
                    uncompressed_size: firmware_content.len() as u64,
                    fuchsia_merkle_root: firmware_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_firmware(|configuration, _| match configuration {
                paver::Configuration::A => Ok(b"matching".to_vec()),
                _ => Ok(b"no match".to_vec()),
            }))
        })
        .ota_manifest(manifest)
        .build()
        .await;
    env.run_packageless_update().await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions()
            .chain([ReplaceRetainedBlobs(vec![empty_merkle().into(), firmware_hash.into()]), Gc]),
        [
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            // Events we really care about testing
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "".to_string(),
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::A,
                firmware_type: "".to_string(),
            }),
            Paver(PaverEvent::WriteFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "".to_string(),
                payload: b"matching".to_vec(),
            }),
        ],
        [
            // rest of the events.
            Paver(PaverEvent::DataSinkFlush),
            ReplaceRetainedBlobs(vec![]),
            Gc,
            BlobfsSync,
            Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
            Paver(PaverEvent::BootManagerFlush),
            Reboot,
        ],
    );
}

#[fuchsia::test]
async fn firmware_comparing_respects_fuchsia_mem_buffer_size() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .firmware_package(btreemap! {
            "".to_owned() => ::update_package::ImageMetadata::new(
                8,
                MATCHING_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-firmware", 6, "a")
            ),
        })
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                0,
                EMPTY_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_firmware_custom_buffer_size(|configuration, _| {
                match configuration {
                    // The read will return a VMO with the correct contents, but the
                    // fuchsia.mem.Buffer size is too small so system-updater will not use it.
                    paver::Configuration::A => Ok((b"matching".to_vec(), 7)),
                    _ => Ok((b"no match".to_vec(), 8)),
                }
            }))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver.url(image_package_url_to_string("update-images-firmware", 6)).resolve(
        &env.resolver.package("update-images-firmware", hashstr(6)).add_file("a", "matching"),
    );

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "".to_string(),
        }),
        // Firmware in A is read.
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::A,
            firmware_type: "".to_string(),
        }),
        // But because the Buffer size is respected, it is not a match and the image is resolved.
        ReplaceRetainedPackages(vec![hash(6).into()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-firmware", 6)),
        Paver(PaverEvent::WriteFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "".to_string(),
            payload: b"matching".to_vec(),
        }),
        // rest of the events.
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn firmware_comparing_respects_fuchsia_mem_buffer_size_packageless() {
    let firmware_content = b"matching";
    let firmware_hash = fuchsia_merkle::root_from_slice(firmware_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Firmware("".to_string()),
                blob: manifest::Blob {
                    uncompressed_size: firmware_content.len() as u64,
                    fuchsia_merkle_root: firmware_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_firmware_custom_buffer_size(|configuration, _| {
                match configuration {
                    // The read will return a VMO with the correct contents, but the
                    // fuchsia.mem.Buffer size is too small so system-updater will not use it.
                    paver::Configuration::A => Ok((b"matching".to_vec(), 7)),
                    _ => Ok((b"no match".to_vec(), 8)),
                }
            }))
        })
        .ota_manifest(manifest)
        .blob(firmware_hash, firmware_content.to_vec())
        .build()
        .await;
    env.run_packageless_update().await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions()
            .chain([ReplaceRetainedBlobs(vec![empty_merkle().into(), firmware_hash.into()]), Gc]),
        [
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "".to_string(),
            }),
            // Firmware in A is read.
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::A,
                firmware_type: "".to_string(),
            }),
            // But because the Buffer size is respected, it is not a match and the image is downloaded.
            OtaDownloader(OtaDownloaderEvent::FetchBlob(firmware_hash.into())),
            Paver(PaverEvent::WriteFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "".to_string(),
                payload: b"matching".to_vec(),
            }),
        ],
        [
            // rest of the events.
            Paver(PaverEvent::DataSinkFlush),
            ReplaceRetainedBlobs(vec![]),
            Gc,
            BlobfsSync,
            Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
            Paver(PaverEvent::BootManagerFlush),
            Reboot,
        ],
    );
}

#[fuchsia::test]
async fn firmware_copying_sets_fuchsia_mem_buffer_size() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .firmware_package(btreemap! {
            "".to_owned() => ::update_package::ImageMetadata::new(
                8,
                MATCHING_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-firmware", 6, "a")
            ),
        })
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                0,
                EMPTY_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_firmware(|configuration, _| match configuration {
                // The Buffer contains an extra 0u8, but ReadFirmware returns the VMO of
                // the entire partition (not just the image), so system-updater should use
                // the size from the manifest in the update package and still find a match and
                // write only the 8 bytes of the image.
                paver::Configuration::A => Ok(b"matching\0".to_vec()),
                _ => Ok(b"no match".to_vec()),
            }))
        })
        .build()
        .await;

    env.resolver
        .register_custom_package("another-update/4", "update", "upd4t3r", "fuchsia.com")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.run_update_with_options("fuchsia-pkg://fuchsia.com/another-update/4", default_options())
        .await
        .expect("run system updater");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve("fuchsia-pkg://fuchsia.com/another-update/4".to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::A,
            firmware_type: "".to_string(),
        }),
        Paver(PaverEvent::WriteFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "".to_string(),
            // Only 8 bytes are written, the trailing 0u8 returned by ReadFirmware is ignored.
            payload: b"matching".to_vec(),
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn firmware_copying_sets_fuchsia_mem_buffer_size_packageless() {
    let firmware_content = b"matching";
    let firmware_hash = fuchsia_merkle::root_from_slice(firmware_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Firmware("".to_string()),
                blob: manifest::Blob {
                    uncompressed_size: firmware_content.len() as u64,
                    fuchsia_merkle_root: firmware_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::read_firmware(|configuration, _| match configuration {
                // The Buffer contains an extra 0u8, but ReadFirmware returns the VMO of
                // the entire partition (not just the image), so system-updater should use
                // the size from the manifest in the update package and still find a match and
                // write only the 8 bytes of the image.
                paver::Configuration::A => Ok(b"matching\0".to_vec()),
                _ => Ok(b"no match".to_vec()),
            }))
        })
        .ota_manifest(manifest)
        .build()
        .await;
    env.run_packageless_update().await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions()
            .chain([ReplaceRetainedBlobs(vec![empty_merkle().into(), firmware_hash.into()]), Gc]),
        [
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "".to_string(),
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::A,
                firmware_type: "".to_string(),
            }),
            Paver(PaverEvent::WriteFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "".to_string(),
                // Only 8 bytes are written, the trailing 0u8 returned by ReadFirmware is ignored.
                payload: b"matching".to_vec(),
            }),
        ],
        [
            Paver(PaverEvent::DataSinkFlush),
            ReplaceRetainedBlobs(vec![]),
            Gc,
            BlobfsSync,
            Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
            Paver(PaverEvent::BootManagerFlush),
            Reboot,
        ],
    );
}

#[fuchsia::test]
async fn writes_multiple_firmware_types() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .firmware_package(btreemap! {
            "a".to_owned() => ::update_package::ImageMetadata::new(
                5, sha256(5), image_package_resource_url("update-images-firmware", 5, "A")
            ),
            "b".to_owned() => ::update_package::ImageMetadata::new(
                5, sha256(5), image_package_resource_url("update-images-firmware", 5, "B")
            ),
        })
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                0,
                EMPTY_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();
    let env = TestEnv::builder().build().await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver.url(image_package_url_to_string("update-images-firmware", 5)).resolve(
        &env.resolver
            .package("firmware", hashstr(7))
            .add_file("A", "A contents")
            .add_file("B", "B contents"),
    );

    env.run_update().await.expect("success");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "a".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::A,
            firmware_type: "a".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "b".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::A,
            firmware_type: "b".to_string(),
        }),
        ReplaceRetainedPackages(vec![hashstr(5).parse().unwrap()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-firmware", 5)),
        Paver(PaverEvent::WriteFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "a".to_string(),
            payload: b"A contents".to_vec(),
        }),
        Paver(PaverEvent::WriteFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "b".to_string(),
            payload: b"B contents".to_vec(),
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn writes_multiple_firmware_types_packageless() {
    let firmware_a_content = b"A contents";
    let firmware_a_hash = fuchsia_merkle::root_from_slice(firmware_a_content);
    let firmware_b_content = b"B contents";
    let firmware_b_hash = fuchsia_merkle::root_from_slice(firmware_b_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Firmware("a".to_string()),
                blob: manifest::Blob {
                    uncompressed_size: firmware_a_content.len() as u64,
                    fuchsia_merkle_root: firmware_a_hash,
                },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Firmware("b".to_string()),
                blob: manifest::Blob {
                    uncompressed_size: firmware_b_content.len() as u64,
                    fuchsia_merkle_root: firmware_b_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .ota_manifest(manifest)
        .blob(firmware_a_hash, firmware_a_content.to_vec())
        .blob(firmware_b_hash, firmware_b_content.to_vec())
        .build()
        .await;
    env.run_packageless_update().await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions().chain([
            ReplaceRetainedBlobs(vec![
                empty_merkle().into(),
                firmware_a_hash.into(),
                firmware_b_hash.into(),
            ]),
            Gc,
        ]),
        [
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "a".to_string(),
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::A,
                firmware_type: "a".to_string(),
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "b".to_string(),
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::A,
                firmware_type: "b".to_string(),
            }),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(firmware_a_hash.into())),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(firmware_b_hash.into())),
            Paver(PaverEvent::WriteFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "a".to_string(),
                payload: b"A contents".to_vec(),
            }),
            Paver(PaverEvent::WriteFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "b".to_string(),
                payload: b"B contents".to_vec(),
            }),
        ],
        [
            Paver(PaverEvent::DataSinkFlush),
            ReplaceRetainedBlobs(vec![]),
            Gc,
            BlobfsSync,
            Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
            Paver(PaverEvent::BootManagerFlush),
            Reboot,
        ],
    );
}

#[fuchsia::test]
async fn skips_unsupported_firmware_type() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .firmware_package(btreemap! {
            "a".to_owned() => ::update_package::ImageMetadata::new(
                5, sha256(5), image_package_resource_url("update-images-firmware", 5, "A")
            ),
        })
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                0,
                EMPTY_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::write_firmware(|_, _, _| {
                paver::WriteFirmwareResult::Unsupported(true)
            }))
        })
        .build()
        .await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver
        .url(image_package_url_to_string("update-images-firmware", 5))
        .resolve(&env.resolver.package("firmware", hashstr(7)).add_file("A", "A contents"));

    // Update should still succeed, we want to skip unsupported firmware types.
    env.run_update().await.expect("success");

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "a".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::A,
            firmware_type: "a".to_string(),
        }),
        ReplaceRetainedPackages(vec![hashstr(5).parse().unwrap()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-firmware", 5)),
        Paver(PaverEvent::WriteFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "a".to_string(),
            payload: b"A contents".to_vec(),
        }),
        Paver(PaverEvent::DataSinkFlush),
        ReplaceRetainedPackages(vec![]),
        Gc,
        BlobfsSync,
        Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
        Reboot,
    ]));
}

#[fuchsia::test]
async fn skips_unsupported_firmware_type_packageless() {
    let firmware_content = b"A contents";
    let firmware_hash = fuchsia_merkle::root_from_slice(firmware_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Firmware("a".to_string()),
                blob: manifest::Blob {
                    uncompressed_size: firmware_content.len() as u64,
                    fuchsia_merkle_root: firmware_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::write_firmware(|_, _, _| {
                paver::WriteFirmwareResult::Unsupported(true)
            }))
        })
        .ota_manifest(manifest)
        .blob(firmware_hash, firmware_content.to_vec())
        .build()
        .await;
    env.run_packageless_update().await.expect("run system updater");

    env.assert_unordered_interactions(
        initial_interactions()
            .chain([ReplaceRetainedBlobs(vec![empty_merkle().into(), firmware_hash.into()]), Gc]),
        [
            Paver(PaverEvent::ReadAsset {
                configuration: paver::Configuration::B,
                asset: paver::Asset::Kernel,
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "a".to_string(),
            }),
            Paver(PaverEvent::ReadFirmware {
                configuration: paver::Configuration::A,
                firmware_type: "a".to_string(),
            }),
            OtaDownloader(OtaDownloaderEvent::FetchBlob(firmware_hash.into())),
            Paver(PaverEvent::WriteFirmware {
                configuration: paver::Configuration::B,
                firmware_type: "a".to_string(),
                payload: b"A contents".to_vec(),
            }),
        ],
        [
            Paver(PaverEvent::DataSinkFlush),
            ReplaceRetainedBlobs(vec![]),
            Gc,
            BlobfsSync,
            Paver(PaverEvent::SetConfigurationActive { configuration: paver::Configuration::B }),
            Paver(PaverEvent::BootManagerFlush),
            Reboot,
        ],
    );
}

#[fuchsia::test]
async fn fails_on_firmware_write_error() {
    let images_json = ::update_package::ImagePackagesManifest::builder()
        .firmware_package(btreemap! {
            "a".to_owned() => ::update_package::ImageMetadata::new(
                5, sha256(5), image_package_resource_url("update-images-firmware", 5, "A")
            ),
        })
        .fuchsia_package(
            ::update_package::ImageMetadata::new(
                0,
                EMPTY_SHA256.parse().unwrap(),
                image_package_resource_url("update-images-fuchsia", 9, "zbi"),
            ),
            None,
        )
        .clone()
        .build();

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::write_firmware(|_, _, _| {
                paver::WriteFirmwareResult::Status(Status::INTERNAL.into_raw())
            }))
        })
        .build()
        .await;

    env.resolver
        .register_package("update", "upd4t3")
        .add_file("packages.json", make_packages_json([]))
        .add_file("epoch.json", make_current_epoch_json())
        .add_file("images.json", serde_json::to_string(&images_json).unwrap());

    env.resolver
        .url(image_package_url_to_string("update-images-firmware", 5))
        .resolve(&env.resolver.package("firmware", hashstr(7)).add_file("A", "A contents"));

    let mut attempt = env.start_update().await.unwrap();
    let info = UpdateInfo::builder().download_size(0).build();
    let progress = Progress::builder().fraction_completed(0.0).bytes_downloaded(0).build();
    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::Stage(UpdateInfoAndProgress::builder().info(info).progress(progress).build())
    );
    assert_eq!(
        attempt.next().await.unwrap().unwrap(),
        State::FailStage(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(progress)
                .build()
                .with_stage_reason(StageFailureReason::Internal)
        )
    );

    env.assert_interactions(crate::initial_interactions().chain([
        PackageResolve(UPDATE_PKG_URL.to_string()),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "a".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::A,
            firmware_type: "a".to_string(),
        }),
        ReplaceRetainedPackages(vec![hashstr(5).parse().unwrap()]),
        Gc,
        PackageResolve(image_package_url_to_string("update-images-firmware", 5)),
        Paver(PaverEvent::WriteFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "a".to_string(),
            payload: b"A contents".to_vec(),
        }),
    ]));
}

#[fuchsia::test]
async fn fails_on_firmware_write_error_packageless() {
    let firmware_content = b"A contents";
    let firmware_hash = fuchsia_merkle::root_from_slice(firmware_content);

    let manifest = OtaManifest {
        images: vec![
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Asset(AssetType::Zbi),
                blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
            },
            manifest::Image {
                slot: manifest::Slot::AB,
                image_type: manifest::ImageType::Firmware("a".to_string()),
                blob: manifest::Blob {
                    uncompressed_size: firmware_content.len() as u64,
                    fuchsia_merkle_root: firmware_hash,
                },
            },
        ],
        ..make_manifest([])
    };

    let env = TestEnv::builder()
        .paver_service(|builder| {
            builder.insert_hook(mphooks::write_firmware(|_, _, _| {
                paver::WriteFirmwareResult::Status(Status::INTERNAL.into_raw())
            }))
        })
        .ota_manifest(manifest)
        .blob(firmware_hash, firmware_content.to_vec())
        .build()
        .await;

    let mut attempt = env.start_packageless_update().await.unwrap();
    let info = UpdateInfo::builder().download_size(0).build();
    let manifest_size = env.ota_manifest_size() as u64;
    let total_goal = manifest_size + firmware_content.len() as u64 * 2;
    let progress = Progress::builder()
        .fraction_completed(manifest_size as f32 / total_goal as f32)
        .bytes_downloaded(manifest_size)
        .build();
    assert_eq!(attempt.next().await.unwrap().unwrap(), State::Prepare);
    let state = loop {
        let state = attempt.next().await.unwrap().unwrap();
        if state.id() != StateId::Stage {
            break state;
        }
    };
    assert_eq!(
        state,
        State::FailStage(
            UpdateInfoAndProgress::builder()
                .info(info)
                .progress(progress)
                .build()
                .with_stage_reason(StageFailureReason::Internal)
        )
    );

    env.assert_interactions(initial_interactions().chain([
        ReplaceRetainedBlobs(vec![empty_merkle().into(), firmware_hash.into()]),
        Gc,
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::B,
            asset: paver::Asset::Kernel,
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "a".to_string(),
        }),
        Paver(PaverEvent::ReadFirmware {
            configuration: paver::Configuration::A,
            firmware_type: "a".to_string(),
        }),
        OtaDownloader(OtaDownloaderEvent::FetchBlob(firmware_hash.into())),
        Paver(PaverEvent::WriteFirmware {
            configuration: paver::Configuration::B,
            firmware_type: "a".to_string(),
            payload: b"A contents".to_vec(),
        }),
    ]));
}
