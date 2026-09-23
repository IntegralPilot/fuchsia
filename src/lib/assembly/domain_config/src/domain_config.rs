// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use assembly_constants::FileEntry;
use assembly_platform_configuration::{DomainConfig, FileOrContents};
use camino::{Utf8Path, Utf8PathBuf};
use cml::RelativePath;
use cml::types::common::synthetic_span;
use cml::types::document::DocumentContext;
use cml::types::expose::{ContextExpose, ExposeFromRef};
use fidl::persist;
use fuchsia_pkg::{PackageBuilder, PackageManifest, RelativeTo};
use std::io::Write;

/// A builder that takes domain configs and places them in a new package that
/// can be placed in the assembly.
pub struct DomainConfigPackage {
    config: DomainConfig,
}

impl DomainConfigPackage {
    /// Construct a new domain config package.
    pub fn new(config: DomainConfig) -> Self {
        Self { config }
    }

    /// Build the package and return the package manifest and path to manifest.
    pub fn build(self, outdir: impl AsRef<Utf8Path>) -> Result<(Utf8PathBuf, PackageManifest)> {
        let outdir = outdir.as_ref();
        std::fs::create_dir_all(&outdir)
            .with_context(|| format!("creating directory {}", outdir))?;

        // Domain config packages are never produced by assembly tools from one
        // Fuchsia release and then read by binaries from another Fuchsia
        // release. Give them the platform ABI revision.
        let mut builder =
            PackageBuilder::new_platform_internal_package(&self.config.name.to_string());

        let manifest_path = outdir.join("package_manifest.json");
        let metafar_path = outdir.join("meta.far");
        builder.manifest_path(&manifest_path);
        builder.manifest_blobs_relative_to(RelativeTo::File);

        // Find all the directory routes to expose.
        let mut exposes = vec![];
        for (directory, directory_config) in self.config.directories {
            let subdir = RelativePath::new(&format!("meta/fuchsia.domain_config/{}", directory))
                .with_context(|| format!("Calculating relative path for {directory}"))?;
            let name = cml::Name::new(&directory)
                .with_context(|| format!("Calculating name for {directory}"))?;

            exposes.push(synthetic_span(ContextExpose {
                // unwrap is safe, because try_new cannot fail with "pkg".
                directory: Some(synthetic_span(cml::OneOrMany::One(
                    cml::Name::new("pkg").unwrap(),
                ))),
                r#as: Some(synthetic_span(name)),
                subdir: Some(synthetic_span(subdir)),
                from: synthetic_span(cml::OneOrMany::One(ExposeFromRef::Framework)),
                ..Default::default()
            }));

            if directory_config.entries.is_empty() {
                // Add an empty file to the directory to ensure the directory gets created.
                let empty_file_name = "_ensure_directory_creation";
                let empty_file_path = outdir.join(empty_file_name);
                let destination = Utf8PathBuf::from("meta/fuchsia.domain_config")
                    .join(&directory)
                    .join(empty_file_name);
                std::fs::write(&empty_file_path, "").context("writing empty file")?;
                builder
                    .add_file_to_far(destination, &empty_file_path)
                    .with_context(|| format!("adding empty file {empty_file_path}"))?;
            }

            // Add the necessary config files to the directory.
            for (destination, entry) in directory_config.entries {
                let destination = Utf8PathBuf::from("meta/fuchsia.domain_config")
                    .join(&directory)
                    .join(destination);
                match entry {
                    FileOrContents::File(FileEntry { source, .. }) => {
                        builder
                            .add_file_to_far(destination, &source)
                            .with_context(|| format!("adding config {source}"))?;
                    }
                    FileOrContents::Contents(contents) => {
                        builder
                            .add_contents_to_far(&destination, &contents, &outdir)
                            .with_context(|| format!("adding config to {destination}"))?;
                    }
                    FileOrContents::BinaryContents(contents) => {
                        builder
                            .add_contents_to_far(&destination, &contents, &outdir)
                            .with_context(|| format!("adding binary config to {destination}"))?;
                    }
                }
            }
        }

        if self.config.expose_directories {
            let cml = DocumentContext { expose: Some(exposes), ..Default::default() };
            let out_data = cml::translate::compile(&cml, cml::CompileOptions::default())
                .with_context(|| format!("compiling domain config routes"))?;

            let cm_name = format!("{}.cm", self.config.name);
            let cm_path = outdir.join(&cm_name);
            let mut cm_file = std::fs::File::create(&cm_path)
                .with_context(|| format!("creating domain config routes: {cm_path}"))?;

            cm_file
                .write_all(&persist(&out_data)?)
                .with_context(|| format!("writing domain config routes: {cm_path}"))?;

            builder
                .add_file_to_far(format!("meta/{cm_name}"), &cm_path)
                .with_context(|| format!("adding file to domain config package: {cm_path}"))?;
        }

        let manifest = builder
            .build(&outdir, metafar_path)
            .with_context(|| format!("building domain config package: {}", self.config.name))?;

        Ok((manifest_path, manifest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_constants::{PackageDestination, PackageSetDestination};
    use assembly_platform_configuration::DomainConfigDirectory;
    use assembly_util::NamedMap;
    use assert_matches::assert_matches;
    use cm_rust::{ComponentDecl, ExposeDecl, ExposeDirectoryDecl, ExposeSource, ExposeTarget};
    use fidl::unpersist;
    use fidl_fuchsia_component_decl::Component;
    use fuchsia_archive::Utf8Reader;
    use fuchsia_pkg::PackageName;
    use pretty_assertions::assert_eq;
    use std::fs::File;
    use std::str::FromStr;
    use tempfile::tempdir;

    #[test]
    fn build() {
        let tmp = tempdir().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();

        // Prepare the domain config input.
        let config_source = outdir.join("config_source.json");
        std::fs::write(&config_source, "bleep bloop").unwrap();
        let mut entries = NamedMap::<String, FileOrContents>::new("config files");
        entries
            .try_insert_unique(
                "config.json".to_string(),
                FileOrContents::File(FileEntry {
                    source: config_source.clone(),
                    destination: "config.json".into(),
                }),
            )
            .unwrap();
        let config = DomainConfig {
            name: PackageSetDestination::Blob(PackageDestination::ForTest),
            directories: [("config-dir".into(), DomainConfigDirectory { entries })].into(),
            expose_directories: true,
        };

        let package = DomainConfigPackage::new(config);
        let (path, manifest) = package.build(&outdir).unwrap();

        // Assert the manifest is correct.
        assert_eq!(path, outdir.join("package_manifest.json"));
        let loaded_manifest = PackageManifest::try_load_from(path).unwrap();
        assert_eq!(manifest, loaded_manifest);
        assert_eq!(manifest.name(), &PackageName::from_str("for-test").unwrap());
        let blobs = manifest.into_blobs();
        assert_eq!(blobs.len(), 1);
        let blob = blobs.iter().find(|&b| &b.path == "meta/").unwrap();
        assert_eq!(blob.source_path, outdir.join("meta.far").to_string());

        // Assert the contents of the package are correct.
        let far_path = outdir.join("meta.far");
        let mut far_reader = Utf8Reader::new(File::open(&far_path).unwrap()).unwrap();
        let package = far_reader.read_file("meta/package").unwrap();
        assert_eq!(package, br#"{"name":"for-test","version":"0"}"#);
        let cm_bytes = far_reader.read_file("meta/for-test.cm").unwrap();
        let fidl_component_decl: Component = unpersist(&cm_bytes).unwrap();
        let component = ComponentDecl::try_from(fidl_component_decl).unwrap();
        assert_eq!(component.exposes.len(), 1);
        assert_matches!(&component.exposes[0], ExposeDecl::Directory(ExposeDirectoryDecl {
            source: ExposeSource::Framework,
            source_name,
            source_dictionary,
            target: ExposeTarget::Parent,
            target_name,
            rights: _,
            subdir,
            availability: _,
        }) => {
            assert_eq!(source_name, &cml::Name::new("pkg").unwrap());
            assert!(source_dictionary.is_dot());
            assert_eq!(target_name, &cml::Name::new("config-dir").unwrap());
            assert_eq!(subdir, &RelativePath::new("meta/fuchsia.domain_config/config-dir").unwrap());
        });
        let contents = far_reader.read_file("meta/contents").unwrap();
        let contents = std::str::from_utf8(&contents).unwrap();
        let expected_contents = "\
        "
        .to_string();
        assert_eq!(expected_contents, contents);
        let contents =
            far_reader.read_file("meta/fuchsia.domain_config/config-dir/config.json").unwrap();
        let contents = std::str::from_utf8(&contents).unwrap();
        let expected_contents = "bleep bloop".to_string();
        assert_eq!(expected_contents, contents);
    }

    #[test]
    fn build_no_routing() {
        let tmp = tempdir().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();

        // Prepare the domain config input.
        let config_source = outdir.join("config_source.json");
        std::fs::write(&config_source, "bleep bloop").unwrap();
        let mut entries = NamedMap::<String, FileOrContents>::new("config files");
        entries
            .try_insert_unique(
                "config.json".to_string(),
                FileOrContents::File(FileEntry {
                    source: config_source.clone(),
                    destination: "config.json".into(),
                }),
            )
            .unwrap();
        let config = DomainConfig {
            name: PackageSetDestination::Blob(PackageDestination::ForTest),
            directories: [("config-dir".into(), DomainConfigDirectory { entries })].into(),
            expose_directories: false,
        };

        let package = DomainConfigPackage::new(config);
        let (path, manifest) = package.build(&outdir).unwrap();

        // Assert the manifest is correct.
        assert_eq!(path, outdir.join("package_manifest.json"));
        let loaded_manifest = PackageManifest::try_load_from(path).unwrap();
        assert_eq!(manifest, loaded_manifest);
        assert_eq!(manifest.name(), &PackageName::from_str("for-test").unwrap());
        let blobs = manifest.into_blobs();
        assert_eq!(blobs.len(), 1);
        let blob = blobs.iter().find(|&b| &b.path == "meta/").unwrap();
        assert_eq!(blob.source_path, outdir.join("meta.far").to_string());

        // Assert the contents of the package are correct.
        let far_path = outdir.join("meta.far");
        let mut far_reader = Utf8Reader::new(File::open(&far_path).unwrap()).unwrap();
        let package = far_reader.read_file("meta/package").unwrap();
        assert_eq!(package, br#"{"name":"for-test","version":"0"}"#);
        let contents = far_reader.read_file("meta/contents").unwrap();
        let contents = std::str::from_utf8(&contents).unwrap();
        let expected_contents = "\
        "
        .to_string();
        assert_eq!(expected_contents, contents);
        let contents =
            far_reader.read_file("meta/fuchsia.domain_config/config-dir/config.json").unwrap();
        let contents = std::str::from_utf8(&contents).unwrap();
        let expected_contents = "bleep bloop".to_string();
        assert_eq!(expected_contents, contents);
    }

    #[test]
    fn build_no_directories() {
        let tmp = tempdir().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();

        // Prepare the domain config input.
        let config = DomainConfig {
            name: PackageSetDestination::Blob(PackageDestination::ForTest),
            directories: NamedMap::new("directories"),
            expose_directories: true,
        };
        let package = DomainConfigPackage::new(config);
        let (path, manifest) = package.build(&outdir).unwrap();

        // Assert the manifest is correct.
        assert_eq!(path, outdir.join("package_manifest.json"));
        let loaded_manifest = PackageManifest::try_load_from(path).unwrap();
        assert_eq!(manifest, loaded_manifest);
        assert_eq!(manifest.name(), &PackageName::from_str("for-test").unwrap());
        let blobs = manifest.into_blobs();
        assert_eq!(blobs.len(), 1);
        let blob = blobs.iter().find(|&b| b.path == "meta/").unwrap();
        assert_eq!(blob.source_path, outdir.join("meta.far").to_string());

        // Assert the contents of the package are correct.
        let far_path = outdir.join("meta.far");
        let mut far_reader = Utf8Reader::new(File::open(&far_path).unwrap()).unwrap();
        let package = far_reader.read_file("meta/package").unwrap();
        assert_eq!(package, br#"{"name":"for-test","version":"0"}"#);
        let cm_bytes = far_reader.read_file("meta/for-test.cm").unwrap();
        let fidl_component_decl: Component = unpersist(&cm_bytes).unwrap();
        let component = ComponentDecl::try_from(fidl_component_decl).unwrap();
        assert_eq!(component.exposes.len(), 0);
        let contents = far_reader.read_file("meta/contents").unwrap();
        let contents = std::str::from_utf8(&contents).unwrap();
        let expected_contents = "\
        "
        .to_string();
        assert_eq!(expected_contents, contents);
    }

    #[test]
    fn build_no_configs() {
        let tmp = tempdir().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();

        // Prepare the domain config input.
        let entries = NamedMap::<String, FileOrContents>::new("config files");
        let config = DomainConfig {
            name: PackageSetDestination::Blob(PackageDestination::ForTest),
            directories: [("config-dir".into(), DomainConfigDirectory { entries })].into(),
            expose_directories: true,
        };

        let package = DomainConfigPackage::new(config);
        let (path, manifest) = package.build(&outdir).unwrap();

        // Assert the manifest is correct.
        assert_eq!(path, outdir.join("package_manifest.json"));
        let loaded_manifest = PackageManifest::try_load_from(path).unwrap();
        assert_eq!(manifest, loaded_manifest);
        assert_eq!(manifest.name(), &PackageName::from_str("for-test").unwrap());
        let blobs = manifest.into_blobs();
        assert_eq!(blobs.len(), 1);
        let blob = blobs.iter().find(|&b| &b.path == "meta/").unwrap();
        assert_eq!(blob.source_path, outdir.join("meta.far").to_string());

        // Assert the contents of the package are correct.
        let far_path = outdir.join("meta.far");
        let mut far_reader = Utf8Reader::new(File::open(&far_path).unwrap()).unwrap();
        let package = far_reader.read_file("meta/package").unwrap();
        assert_eq!(package, br#"{"name":"for-test","version":"0"}"#);
        let cm_bytes = far_reader.read_file("meta/for-test.cm").unwrap();
        let fidl_component_decl: Component = unpersist(&cm_bytes).unwrap();
        let component = ComponentDecl::try_from(fidl_component_decl).unwrap();
        assert_eq!(component.exposes.len(), 1);
        assert_matches!(&component.exposes[0], ExposeDecl::Directory(ExposeDirectoryDecl {
            source: ExposeSource::Framework,
            source_name,
            source_dictionary,
            target: ExposeTarget::Parent,
            target_name,
            rights: _,
            subdir,
            availability: _,
        }) => {
            assert_eq!(source_name, &cml::Name::new("pkg").unwrap());
            assert!(source_dictionary.is_dot());
            assert_eq!(target_name, &cml::Name::new("config-dir").unwrap());
            assert_eq!(subdir, &RelativePath::new("meta/fuchsia.domain_config/config-dir").unwrap());
        });
        let contents = far_reader.read_file("meta/contents").unwrap();
        let contents = std::str::from_utf8(&contents).unwrap();
        let expected_contents = "\
        "
        .to_string();
        assert_eq!(expected_contents, contents);
        let contents = far_reader
            .read_file("meta/fuchsia.domain_config/config-dir/_ensure_directory_creation")
            .unwrap();
        let contents = std::str::from_utf8(&contents).unwrap();
        let expected_contents = "".to_string();
        assert_eq!(expected_contents, contents);
    }
}
