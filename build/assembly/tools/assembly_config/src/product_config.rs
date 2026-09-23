// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{ExtractProductPackageArgs, HybridProductArgs, ProductArgs};
use anyhow::{Context, Result};
use assembly_config_schema::product_settings::{ProductPackageDetails, StarnixImagesOrPackage};
use assembly_config_schema::{BuildType, ProductConfig};
use assembly_container::AssemblyContainer;
use assembly_release_info::{ProductReleaseInfo, ReleaseInfo};
use assembly_util::{get_release_repository, get_release_version, validate_release_info_string};
use camino::Utf8PathBuf;
use depfile::Depfile;
use fuchsia_archive::Utf8Reader;
use fuchsia_pkg::{PackageBuilder, PackageManifest};
use product_input_bundle::ProductInputBundle;
use starnix_container::StarnixContainerGenerator;
use std::io::Cursor;

pub fn new(args: &ProductArgs) -> Result<()> {
    let mut config = ProductConfig::from_config_path(&args.config)?;
    let mut depfile = Depfile::new();

    for path in &args.product_input_bundles {
        let pib = ProductInputBundle::from_dir(path)?;
        config.product_input_bundles.insert(pib.release_info.name.clone(), pib);
    }

    let name = match config.product.build_info {
        Some(ref value) => value.name.clone(),
        // TODO(https://fxbug.dev/418249336): Make
        // product.build_info.name a required field.
        None => "unknown".to_string(),
    };
    let version = get_release_version(&args.version, &args.version_file)?;
    let repository = get_release_repository(&args.repo, &args.repo_file)?;

    config.product.release_info = ProductReleaseInfo {
        info: ReleaseInfo {
            name: validate_release_info_string(name)?,
            version: validate_release_info_string(version)?,
            repository: validate_release_info_string(repository)?,
        },
        pibs: config.product_input_bundles.values().map(|p| p.release_info.clone()).collect(),
    };

    // Build starnix container
    let starnix_containers = &mut config.product.starnix_containers;
    let temp_dir = tempfile::tempdir().context("creating temp dir for starnix containers")?;
    let package_set = if config.platform.build_type == BuildType::Eng {
        &mut config.product.packages.cache
    } else {
        &mut config.product.packages.base
    };
    for container in starnix_containers {
        let (system, ramdisk, vendor) = match &container.images_or_package {
            StarnixImagesOrPackage::Images(i) => {
                Ok((i.system.clone(), i.ramdisk.clone(), i.vendor.clone()))
            }
            StarnixImagesOrPackage::Package(_) => Err(anyhow::anyhow!(
                "Existing containers from prebuilt product configs should be repackaged using the hybrid product config command",
            )),
        }?;
        let outdir_path = temp_dir.path().join(&container.name);
        std::fs::create_dir_all(&outdir_path).context("creating container temp dir")?;
        let outdir =
            Utf8PathBuf::try_from(outdir_path).context("converting temp dir path to utf8")?;
        let base =
            ProductConfig::find_package_in_pibs(&config.product_input_bundles, &container.base)
                .with_context(|| {
                    format!(
                        "finding starnix base package '{}' in product input bundles",
                        container.base
                    )
                })?
                .clone();
        let hals =
            ProductConfig::find_hals_in_pibs(&config.product_input_bundles, &container.hals)?;
        for (name, manifest) in container.hals.iter().zip(hals.iter()) {
            package_set.insert(
                name.clone(),
                ProductPackageDetails { manifest: manifest.clone(), config_data: Vec::new() },
            );
        }

        StarnixContainerGenerator {
            name: container.name.clone(),
            outdir: outdir.clone(),
            base,
            hals,
            skip_subpackages: container.skip_subpackages,
            system,
            ramdisk,
            vendor,
            fstab: container.fstab.clone(),
            init: container.init.clone(),
            file_overrides: container.file_overrides.clone(),
        }
        .build(&mut depfile)?;

        // Update the container to point to the new package.
        let manifest_path = outdir.join("package_manifest.json");
        container.images_or_package = StarnixImagesOrPackage::Package(manifest_path.clone());
        package_set.insert(
            container.name.clone(),
            ProductPackageDetails { manifest: manifest_path, config_data: vec![] },
        );
    }
    // Build systems generally don't add package names to the config, so it
    // serializes index numbers in place of package names by default.
    // We add the package names in now, so all the rest of the rules can assume
    // the config has proper package names.
    let config = config.add_package_names()?;
    config.write_to_dir_with_depfile(&args.output, Some(&mut depfile))?;

    if let Some(depfile_path) = &args.depfile {
        depfile.write_to(depfile_path)?;
    }
    Ok(())
}

pub fn hybrid(args: &HybridProductArgs) -> Result<()> {
    let config = ProductConfig::from_dir(&args.input)?;
    let mut depfile = Depfile::new();

    // Normally this would not be necessary, because all generated configs come
    // from this tool, which adds the package names above, but we still need to
    // support older product configs without names.
    let mut config = config.add_package_names()?;

    for package_manifest_path in &args.replace_package {
        let package_manifest = PackageManifest::try_load_from(&package_manifest_path)?;
        let package_name = package_manifest.name();
        if let Some(path) = config.find_package_in_product(&package_name) {
            *path = package_manifest_path.clone();
        }
    }

    // Replace PIBs that match an existing PIB by name.
    for path in &args.product_input_bundles {
        let pib = ProductInputBundle::from_dir(path)?;
        config.product_input_bundles.insert(pib.release_info.name.clone(), pib);
    }

    let temp_repackaged_dir = tempfile::tempdir().context("creating temp repackaged dir")?;
    let temp_repackaged_path = Utf8PathBuf::from_path_buf(temp_repackaged_dir.path().to_path_buf())
        .map_err(|p| anyhow::anyhow!("converting temp path to utf8: {:?}", p))?;

    starnix_container::repackage_starnix_containers(&mut config, &temp_repackaged_path)?;

    config.write_to_dir_with_depfile(&args.output, Some(&mut depfile))?;

    if let Some(depfile_path) = &args.depfile {
        depfile.write_to(depfile_path)?;
    }

    Ok(())
}

pub fn extract_package(args: &ExtractProductPackageArgs) -> Result<()> {
    let mut config = ProductConfig::from_dir(&args.config)?;
    let mut deps = Depfile::new();

    if let Some(package_manifest_path) = config.find_package_in_product(&args.package_name) {
        let manifest =
            PackageManifest::try_load_from(&package_manifest_path).with_context(|| {
                format!("Loading package manifest to extract: {}", package_manifest_path)
            })?;

        if args.depfile.is_some() {
            // The config file is a dependency.
            let config_path = args.config.join("product_configuration.json");
            deps.add_input(config_path.as_str());

            // The manifest we are extracting from is a dependency.
            deps.add_input(package_manifest_path.as_str());

            // The source blobs of that manifest are dependencies.
            deps.add_inputs(manifest.blobs().iter().map(|b| b.source_path.clone()));

            // The contents of the `meta.far` like components will be extracted into `outdir`.
            // Track those outputs too.
            if let Some(blob) =
                manifest.blobs().iter().find(|b| b.path == PackageManifest::META_FAR_BLOB_PATH)
            {
                let bytes = std::fs::read(&blob.source_path)
                    .with_context(|| format!("reading {}", blob.source_path))?;
                let meta_far = Utf8Reader::new(Cursor::new(bytes)).context("reading FAR")?;
                deps.add_outputs(
                    meta_far
                        .list()
                        .map(|e| args.outdir.join(e.path()).to_string())
                        .filter(|p| !p.ends_with('/')),
                );
            }
        }

        let mut builder = PackageBuilder::from_manifest(manifest, &args.outdir)
            .with_context(|| format!("Loading package to extract: {}", args.package_name))?;

        let metafar_path =
            args.output_package_manifest.parent().context("Invalid outdir")?.join("meta.far");
        builder.manifest_path(args.output_package_manifest.clone());
        builder
            .build(&args.outdir, &metafar_path)
            .with_context(|| format!("Writing out extracted package: {}", args.package_name))?;

        if let Some(depfile_path) = &args.depfile {
            deps.add_outputs(
                [
                    metafar_path,
                    args.outdir.join("meta/fuchsia.abi/abi-revision"),
                    args.outdir.join("meta/fuchsia.pkg/subpackages"),
                    args.outdir.join("meta/package"),
                ]
                .iter()
                .map(|p| p.to_string()),
            );
            deps.add_output(args.output_package_manifest.as_str());
            deps.write_to(depfile_path)?;
        }
    } else {
        anyhow::bail!("Could not find package to extract: {}", args.package_name);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProductInputBundleArgs;
    use fuchsia_pkg::PackageName;
    use fuchsia_url::RelativePackageUrl;
    use std::fs;
    use std::fs::File;
    use std::io::Write;
    use tempfile::{NamedTempFile, tempdir};
    use version_history::AbiRevision;

    const FAKE_ABI_REVISION: AbiRevision = AbiRevision::from_u64(0x5836508c2defac54);

    fn create_tmp_file(content: String) -> (NamedTempFile, Utf8PathBuf) {
        let file = NamedTempFile::new().unwrap();
        let path = Utf8PathBuf::from_path_buf(file.path().to_path_buf()).unwrap();
        let file_write = file.reopen();
        file_write.unwrap().write_all(content.as_bytes()).unwrap();
        (file, path)
    }

    #[test]
    fn test_versioned() {
        let (_version_file, version_path) = create_tmp_file("fake_version".to_string());
        let (_jiri_snapshot_file, jiri_snapshot_path) = create_tmp_file("snapshot".to_string());
        let (_latest_commit_date_file, latest_commit_date_path) =
            create_tmp_file("timestamp".to_string());

        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();
        let product_path = tmp_path.join("my_product");
        fs::create_dir(&product_path).unwrap();

        let config_path = product_path.join("product_configuration.json");
        let config_file = File::create(&config_path).unwrap();
        let config_value = serde_json::json!({
            "platform": {
                "build_type": "eng",
            },
            "product": {
                "build_info": {
                    "name": "fake_product_name",
                    "version": version_path,
                    "jiri_snapshot": jiri_snapshot_path,
                    "latest_commit_date": latest_commit_date_path,
                }
            }
        });
        serde_json::to_writer(&config_file, &config_value).unwrap();

        let args = ProductArgs {
            config: config_path,
            repo: None,
            repo_file: None,
            output: product_path.clone(),
            depfile: None,
            product_input_bundles: vec![],
            version: Some("fake_version".to_string()),
            version_file: None,
        };
        let _ = new(&args);
        let config = ProductConfig::from_dir(product_path).unwrap();
        let expected = "fake_version".to_string();
        assert_eq!(expected, config.product.release_info.info.version);
    }

    #[test]
    fn test_unversioned() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();
        let product_path = tmp_path.join("my_product");
        fs::create_dir(&product_path).unwrap();

        let config_path = product_path.join("product_configuration.json");
        let config_file = File::create(&config_path).unwrap();
        let config_value = serde_json::json!({
            "platform": {
                "build_type": "eng",
            },
            "product": {
            }
        });
        serde_json::to_writer(&config_file, &config_value).unwrap();

        let args = ProductArgs {
            config: config_path,
            repo: None,
            repo_file: None,
            output: product_path.clone(),
            depfile: None,
            product_input_bundles: vec![],
            version: None,
            version_file: None,
        };
        let _ = new(&args);
        let config = ProductConfig::from_dir(product_path).unwrap();
        let expected = "unversioned".to_string();
        assert_eq!(expected, config.product.release_info.info.version);
    }

    #[test]
    fn test_extract_package() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();
        let product_path = tmp_path.join("my_product");
        fs::create_dir(&product_path).unwrap();

        let packages_path = product_path.join("packages");
        fs::create_dir(&packages_path).unwrap();

        let gendir = tmp_path.join("gendir");
        fs::create_dir(&gendir).unwrap();

        let test_package_path = packages_path.join("test");
        let mut builder = PackageBuilder::new("test", FAKE_ABI_REVISION);
        builder.add_contents_as_blob("some/file", "foobar", &gendir).unwrap();
        builder.manifest_path(test_package_path.clone());
        let metafar_path = packages_path.join("meta.far");
        builder.build(&packages_path, &metafar_path).unwrap();

        let config_path = product_path.join("product_configuration.json");
        let config_file = File::create(&config_path).unwrap();
        let config_value = serde_json::json!({
            "platform": {
                "build_type": "eng",
            },
            "product": {
                "packages": {
                    "base": {
                       "test" :
                       {
                         "manifest": "packages/test"
                       },
                    },
                },
            },
        });
        serde_json::to_writer(&config_file, &config_value).unwrap();

        let outdir_path = tmp_path.join("outdir");
        let output_package_manifest_path = tmp_path.join("manifest.json");
        let depfile_path = tmp_path.join("depfile");

        let args = ExtractProductPackageArgs {
            config: product_path,
            package_name: "test".into(),
            outdir: outdir_path.clone(),
            output_package_manifest: output_package_manifest_path.clone(),
            depfile: Some(depfile_path.clone()),
        };
        extract_package(&args).unwrap();
        let extracted_package = PackageManifest::try_load_from(&output_package_manifest_path)
            .expect("Package manifest loaded");

        assert_eq!(extracted_package.name(), &"test".parse::<PackageName>().unwrap());

        let depfile_contents = fs::read_to_string(depfile_path).unwrap();
        println!("{}", depfile_contents);
        let mut expected_outputs = vec![
            tmp_path.join("meta.far").to_string(),
            outdir_path.join("meta/contents").to_string(),
            outdir_path.join("meta/fuchsia.abi/abi-revision").to_string(),
            outdir_path.join("meta/fuchsia.pkg/subpackages").to_string(),
            outdir_path.join("meta/package").to_string(),
            output_package_manifest_path.to_string(),
        ];
        expected_outputs.sort();
        let mut expected_inputs = vec![
            packages_path.join("meta.far").to_string(),
            config_path.to_string(),
            test_package_path.to_string(),
            gendir.join("some/file").into(),
        ];
        expected_inputs.sort();

        let mut actual_parts = depfile_contents.split(":").collect::<Vec<_>>();
        let actual_inputs_str = actual_parts.pop().unwrap().trim();
        let actual_outputs_str = actual_parts.pop().unwrap().trim();

        let mut actual_outputs =
            actual_outputs_str.split_whitespace().filter(|x| x != &"\\").collect::<Vec<_>>();
        actual_outputs.sort();

        let mut actual_inputs =
            actual_inputs_str.split_whitespace().filter(|x| x != &"\\").collect::<Vec<_>>();
        actual_inputs.sort();

        assert_eq!(actual_inputs, expected_inputs);
        assert_eq!(actual_outputs, expected_outputs);
    }

    #[test]
    fn test_hybrid_repackager() {
        let tmp_dir = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp_dir.path().to_path_buf()).unwrap();

        let gendir = tmp_path.join("gendir");
        fs::create_dir(&gendir).unwrap();

        // 1. Create base package
        let base_path = tmp_path.join("base_manifest.json");
        let base_metafar_path = tmp_path.join("base_meta.far");
        let mut builder = PackageBuilder::new("base", FAKE_ABI_REVISION);
        builder.manifest_path(&base_path);

        // Add a blob to the base package.
        let blob_path = tmp_path.join("blob.txt");
        fs::write(&blob_path, "blob content").unwrap();
        builder.add_file_as_blob("data/blob.txt", blob_path).unwrap();

        builder.build(&tmp_path, &base_metafar_path).unwrap();

        // 2. Create HAL package
        let hal_path = tmp_path.join("hal_manifest.json");
        let hal_metafar_path = tmp_path.join("hal_meta.far");
        let mut builder = PackageBuilder::new("hal", FAKE_ABI_REVISION);
        builder.manifest_path(&hal_path);

        let hal_blob_path = tmp_path.join("hal_blob.txt");
        fs::write(&hal_blob_path, "new hal content").unwrap();
        builder.add_file_as_blob("data/hal_blob.txt", hal_blob_path).unwrap();

        builder.build(&tmp_path, &hal_metafar_path).unwrap();

        // 3. Create OLD HAL package (to put in container)
        let old_hal_path = tmp_path.join("old_hal_manifest.json");
        let old_hal_metafar_path = tmp_path.join("old_hal_meta.far");
        let mut builder = PackageBuilder::new("hal", FAKE_ABI_REVISION);
        builder.manifest_path(&old_hal_path);

        let old_hal_blob_path = tmp_path.join("old_hal_blob.txt");
        fs::write(&old_hal_blob_path, "old hal content").unwrap();
        builder.add_file_as_blob("data/hal_blob.txt", old_hal_blob_path).unwrap();

        builder.build(&tmp_path, &old_hal_metafar_path).unwrap();

        // 4. Create container package (the one to be repackaged)
        let container_path = tmp_path.join("container_manifest.json");
        let container_metafar_path = tmp_path.join("container_meta.far");
        let mut builder = PackageBuilder::new("container", FAKE_ABI_REVISION);
        builder.manifest_path(&container_path);

        let old_hal_manifest = PackageManifest::try_load_from(&old_hal_path).unwrap();
        builder
            .add_subpackage(
                &"hal".parse::<RelativePackageUrl>().unwrap(),
                old_hal_manifest.hash(),
                old_hal_path.clone().into(),
            )
            .unwrap();

        builder.build(&tmp_path, &container_metafar_path).unwrap();

        // 5. Create PIB
        let pib_dir = tmp_path.join("pib");
        fs::create_dir(&pib_dir).unwrap();
        let pib_args = ProductInputBundleArgs {
            name: "test_pib".into(),
            base_packages: vec![],
            cache_packages: vec![],
            flexible_packages: vec![],
            packages_for_product_config: vec![base_path.clone(), hal_path.clone()],
            output: pib_dir.clone(),
            version: Some("1.2.3".into()),
            version_file: None,
            repo: Some("fuchsia.com".into()),
            repo_file: None,
            depfile: None,
        };
        crate::product_input_bundle::new(&pib_args).unwrap();

        // 5. Create ProductConfig
        let product_dir = tmp_path.join("product");
        fs::create_dir(&product_dir).unwrap();
        let config_path = product_dir.join("product_configuration.json");
        let config_value = serde_json::json!({
            "platform": {
                "build_type": "eng",
            },
            "product": {
                "starnix_containers": [
                    {
                        "name": "container",
                        "base": "base",
                        "hals": ["hal"],
                        "images_or_package": {
                            "package": container_path.as_str(),
                        }
                    }
                ]
            }
        });
        fs::write(&config_path, config_value.to_string()).unwrap();

        // 6. Call hybrid()
        let output_dir = tmp_path.join("output");
        let args = HybridProductArgs {
            input: product_dir,
            product_input_bundles: vec![pib_dir],
            replace_package: vec![],
            output: output_dir.clone(),
            depfile: None,
        };
        hybrid(&args).unwrap();

        // 7. Verify
        let output_config = ProductConfig::from_dir(&output_dir).unwrap();
        let container = &output_config.product.starnix_containers[0];
        match &container.images_or_package {
            assembly_config_schema::product_settings::StarnixImagesOrPackage::Package(p) => {
                assert!(p.starts_with(output_dir.join("packages")));
                assert!(p.ends_with("container"));

                // Verify that the blob from the base package is in the repackaged container.
                let repackaged_manifest = PackageManifest::try_load_from(p).unwrap();
                assert!(repackaged_manifest.blobs().iter().any(|b| b.path == "data/blob.txt"));

                // Verify that the hal subpackage points to the NEW hash.
                let subpackages = repackaged_manifest.subpackages();
                let hal_sub = subpackages.iter().find(|s| s.name == "hal").unwrap();
                let expected_hal_manifest = PackageManifest::try_load_from(&hal_path).unwrap();
                assert_eq!(hal_sub.merkle, expected_hal_manifest.hash());
            }
            _ => panic!("Expected package"),
        }
    }
}
