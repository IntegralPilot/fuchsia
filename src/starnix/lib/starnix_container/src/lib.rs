// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Component, Path};

use anyhow::{Context, Result, bail};
use camino::{Utf8Path, Utf8PathBuf};
use ext4_extract::ext4_extract;
use fidl_fuchsia_component_decl as fcdecl;
use flate2::read::GzDecoder;
use fuchsia_pkg::{PackageBuilder, PackageManifest};
use fuchsia_url::{FuchsiaPkgAbsoluteComponentUrl, RelativeComponentUrl, RelativePackageUrl};

use ext4_extract::remote_bundle as rb;

pub mod hal_manifest;
pub mod remote_bundle;
pub mod repackage;

use crate::remote_bundle::{Writer, apply_overrides};
use assembly_config_schema::product_settings::{StarnixFileOperation, StarnixFileOverride};
use depfile::Depfile;
pub use repackage::repackage_starnix_containers;

#[derive(Debug, Clone)]
pub struct StarnixContainerGenerator {
    /// Name of the starnix container.
    pub name: String,
    /// Directory to place outputs into.
    pub outdir: Utf8PathBuf,
    /// Path to package archive containing additional resources to include.
    pub base: Utf8PathBuf,
    /// Path to hal package archive.
    pub hals: Vec<Utf8PathBuf>,
    /// Whether to skip including HALs as subpackages.
    pub skip_subpackages: bool,
    /// Path to an Android system image.
    pub system: Utf8PathBuf,
    /// Path to an arbitrary number of ramdisk images which will be concatenated.
    pub ramdisk: Vec<Utf8PathBuf>,
    /// Path to an Android vendor partition image.
    pub vendor: Option<Utf8PathBuf>,
    /// Path to fstab, will go in /odm which overrides the one in /vendor.
    pub fstab: Option<Utf8PathBuf>,
    /// Path to extra init scripts, will go in /odm/etc/init. Can be passed more than once.
    pub init: Vec<Utf8PathBuf>,
    /// File overrides to apply.
    pub file_overrides: Vec<StarnixFileOverride>,
}

#[derive(Debug, Clone)]
pub struct ImageOverridesResult {
    /// Inodes that were skipped (removed or overwritten) in the original image.
    pub skipped_inodes: std::collections::HashSet<u64>,
    /// New files added by overrides, mapping inode to source path on host.
    pub new_files: Vec<(u64, Utf8PathBuf)>,
}

#[derive(Debug, Clone)]
pub struct StarnixContainerRepackager {
    /// Name of the starnix container.
    pub name: String,
    /// Directory to place outputs into.
    pub outdir: Utf8PathBuf,
    /// Path to the existing starnix container's package manifest.
    pub container_manifest_path: Utf8PathBuf,
    /// Path to package archive containing additional resources to include.
    pub base: Utf8PathBuf,
    /// Path to hal package archive.
    pub hals: Vec<Utf8PathBuf>,
    /// Whether to skip including HALs as subpackages.
    pub skip_subpackages: bool,
}

pub const S_IFDIR: u16 = 0x4000;
pub const S_IFREG: u16 = 0x8000;
pub const S_IFLNK: u16 = 0xa000;
pub const S_IFMT: u16 = 0xf000;

impl StarnixContainerGenerator {
    // Build the StarnixContainer
    fn add_ext4_image(
        self,
        name: impl AsRef<Utf8Path>,
        outdir: impl AsRef<Utf8Path>,
        image_path: impl AsRef<Utf8Path>,
        builder: &mut PackageBuilder,
    ) -> Result<HashMap<String, String>> {
        // Put all the system image files into the container.
        let name = name.as_ref();
        let outdir = outdir.as_ref();
        let image_path = image_path.as_ref();

        let image_outdir = outdir.join(name);
        std::fs::create_dir_all(&image_outdir)
            .with_context(|| format!("Preparing directory for image files: {}", image_outdir))?;
        let mut image_files = ext4_extract(image_path.as_str(), image_outdir.as_str()).with_context(
            || format!("Failed to extract EXT4 image from {}. Please ensure the file is a valid EXT4 filesystem image.", image_path),
        )?;

        let my_overrides: Vec<StarnixFileOverride> =
            self.file_overrides.iter().filter(|o| o.image_name == name.as_str()).cloned().collect();

        if !my_overrides.is_empty() {
            let metadata_file_path = image_outdir.join("metadata.v1");
            let bytes = std::fs::read(&metadata_file_path)
                .with_context(|| format!("Failed to read metadata at {}", metadata_file_path))?;
            let metadata = ext4_metadata::Metadata::deserialize(&bytes)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize metadata: {:?}", e))?;

            let result = apply_overrides(metadata, my_overrides, name.as_str())?;

            let new_metadata_bytes = result.metadata.serialize();
            std::fs::write(&metadata_file_path, new_metadata_bytes)
                .with_context(|| format!("Failed to write metadata at {}", metadata_file_path))?;

            let skipped_inode_strings: std::collections::HashSet<String> =
                result.skipped_inodes.iter().map(|i| i.to_string()).collect();

            image_files.retain(|dst, _| !skipped_inode_strings.contains(dst));

            for (inode, src_path) in result.new_files {
                image_files.insert(inode.to_string(), src_path.to_string());
            }
        }

        for (dst, src) in &image_files {
            let dst = format!("data/{}/{}", name, dst);
            builder
                .add_file_as_blob(dst, &src)
                .with_context(|| format!("Adding blob from file: {}", src))?;
        }

        Ok(image_files)
    }

    fn add_ramdisks(
        self,
        name: impl AsRef<Utf8Path>,
        outdir: impl AsRef<Utf8Path>,
        ramdisk_paths: &[impl AsRef<Utf8Path>],
        builder: &mut PackageBuilder,
    ) -> Result<HashMap<String, String>> {
        let name = name.as_ref();
        let outdir = outdir.as_ref();

        let image_outdir = outdir.join(name);
        std::fs::create_dir_all(&image_outdir)
            .with_context(|| format!("Preparing directory for image files: {}", image_outdir))?;

        let mut writer = rb::Writer::new(
            &image_outdir,
            ext4_metadata::ROOT_INODE_NUM,
            crate::remote_bundle::DIRECTORY_MODE,
            rb::Owner::root(),
            Default::default(),
        )?;

        for ramdisk_path in ramdisk_paths {
            let ramdisk_path = ramdisk_path.as_ref();
            let file = std::fs::File::open(&ramdisk_path)
                .with_context(|| format!("Unable to open `{:?}'", ramdisk_path))?;
            let mut file_reader = GzDecoder::new(file);
            loop {
                let mut cpio_reader = cpio::NewcReader::new(file_reader).with_context(
                    || format!("Failed to parse ramdisk at {}. Please ensure the file is a valid GZIP-compressed CPIO archive.", ramdisk_path),
                )?;
                let entry = cpio_reader.entry();

                if cpio_reader.entry().is_trailer() {
                    break;
                }

                let name_string = entry.name().to_string();
                let path = Path::new(&name_string);
                let inode: u64 = entry.ino().into();
                let mode: u16 = entry.mode().try_into().unwrap();
                let uid: u16 = entry.uid().try_into().unwrap();
                let gid: u16 = entry.gid().try_into().unwrap();

                let components: Vec<&str> = path
                    .components()
                    .filter_map(|c| match c {
                        Component::Normal(os_str) => os_str.to_str(),
                        _ => None, // Ignore RootDir ("/"), CurDir ("."), etc.
                    })
                    .collect();

                match mode & S_IFMT {
                    m if m == S_IFREG => {
                        writer.add_file(
                            &components,
                            &mut cpio_reader,
                            inode,
                            mode,
                            rb::Owner { uid, gid },
                            Default::default(),
                        )?;
                    }
                    m if m == S_IFLNK => {
                        let mut data = vec![];
                        cpio_reader.read_to_end(&mut data)?;
                        writer
                            .add_symlink(
                                &components,
                                data,
                                inode,
                                mode,
                                rb::Owner { uid, gid },
                                Default::default(),
                            )
                            .unwrap();
                    }
                    m if m == S_IFDIR => {
                        writer.add_directory(
                            &components,
                            inode,
                            mode,
                            rb::Owner { uid, gid },
                            Default::default(),
                        );
                    }
                    _ => {}
                }

                // Advance to the next entry
                file_reader = cpio_reader.finish().expect("Failed to finish reader");
            }
        }

        let image_files = writer.export()?;
        for (dst, src) in &image_files {
            let dst = format!("data/{}/{}", name, dst);
            builder
                .add_file_as_blob(dst, &src)
                .with_context(|| format!("Adding blob from file: {}", src))?;
        }

        Ok(image_files)
    }

    fn add_to_odm(
        self,
        src: &fuchsia_pkg::BlobInfo,
        dst: &[&str],
        odm_writer: &mut Writer,
    ) -> Result<String> {
        let src = &src.source_path;
        File::open(src)
            .and_then(|mut file| odm_writer.add_file(dst, &mut file))
            .with_context(|| format!("Adding {src} in HAL package to {dst:?}"))?;
        Ok(src.clone())
    }

    fn clone_package(
        self,
        manifest_path: &Utf8PathBuf,
        outdir: &String,
        deps: &mut Depfile,
    ) -> Result<PackageBuilder> {
        let manifest = PackageManifest::try_load_from(manifest_path)
            .with_context(|| format!("Reading base starnix package: {}", manifest_path))?;

        // Our tool will eventually read everything in the base package.
        deps.add_inputs(manifest.blobs().iter().map(|b| b.source_path.clone()));

        // [`PackageBuilder::from_manifest`] will unpack the contents of the `meta.far` into `outdir`.
        // Track those outputs too.
        if let Some(blob) =
            manifest.blobs().iter().find(|b| b.path == PackageManifest::META_FAR_BLOB_PATH)
        {
            let bytes = std::fs::read(&blob.source_path)
                .with_context(|| format!("reading {}", blob.source_path))?;
            let meta_far =
                fuchsia_archive::Utf8Reader::new(Cursor::new(bytes)).context("reading FAR")?;
            deps.add_outputs(meta_far.list().map(|e| format!("{}/{}", outdir, e.path())));
        }

        let builder = PackageBuilder::from_manifest(manifest, outdir)
            .context("Parsing base starnix package")?;

        Ok(builder)
    }

    pub fn build(self, deps: &mut Depfile) -> Result<()> {
        // Track inputs and outputs for producing a depfile for incremental build correctness.

        validate_manifests_in_base(&self.base, &self.hals, self.skip_subpackages)?;

        // Bootstrap the package builder with the contents of the base package, but update the
        // internal and published names.
        let mut builder = self.clone().clone_package(&self.base, &self.outdir.to_string(), deps)?;
        builder.name(&self.name);
        builder.published_name(&self.name);
        builder.manifest_blobs_relative_to(fuchsia_pkg::RelativeTo::File);

        if !self.ramdisk.is_empty() {
            let ramdisk_files =
                self.clone().add_ramdisks("ramdisk", &self.outdir, &self.ramdisk, &mut builder)?;
            deps.add_inputs(&self.ramdisk);
            deps.add_outputs(ramdisk_files.into_values());
        }

        let system_files =
            self.clone().add_ext4_image("system", &self.outdir, &self.system, &mut builder)?;
        deps.add_input(&self.system);
        deps.add_outputs(system_files.into_values());

        // Combine the vendor image with the system image.
        if let Some(vendor_path) = &self.vendor {
            let vendor_files =
                self.clone().add_ext4_image("vendor", &self.outdir, &vendor_path, &mut builder)?;
            deps.add_input(vendor_path);
            deps.add_outputs(vendor_files.into_values());
        }

        // Initialize ODM filesystem.
        let odm_outdir = self.outdir.join("odm");
        std::fs::create_dir_all(&odm_outdir)
            .with_context(|| format!("Preparing directory for ODM files: {}", odm_outdir))?;
        let mut odm_writer = Writer::new(&odm_outdir, |path| {
            // Mimic the SELinux labeling patterns defined for "/odm" in AOSP.
            let label: &[u8] = if path.len() == 0 {
                b"u:object_r:vendor_file:s0"
            } else if path.starts_with(&["etc"]) {
                b"u:object_r:vendor_configs_file:s0"
            } else {
                panic!("No SELinux xattr specified for path {:?}", path);
            };
            [((*b"security.selinux").into(), (*label).into())].into()
        })?;
        odm_writer.add_directory(&["etc"]);
        odm_writer.add_directory(&["etc", "init"]);
        odm_writer.add_directory(&["etc", "vintf"]);
        odm_writer.add_directory(&["etc", "vintf", "manifest"]);

        // Add all the HALs as subpackages.
        for hal in &self.hals {
            let manifest = PackageManifest::try_load_from(&hal)
                .with_context(|| format!("Reading hal package manifest: {}", hal))?;

            if !self.skip_subpackages {
                let name: RelativePackageUrl = manifest.name().to_owned().into();
                builder
                    .add_subpackage(&name, manifest.hash(), hal.into())
                    .with_context(|| format!("Adding subpackage from manifest: {}", hal))?;
            }

            let hal_package_name = manifest.name().to_string();
            let (hal_manifest, hal_manifest_source_path) =
                hal_manifest::load_from_package(&manifest)
                    .with_context(|| format!("Reading hal manifest from package: {}", hal))?;
            deps.add_inputs(hal_manifest_source_path);
            // If a HAL manifest contains `init_rc`, copy that file to
            // `etc/init/{hal_package_name}.rc` in the ODM filesystem.
            if let Some(blob) = hal_manifest.init_rc {
                deps.add_input(self.clone().add_to_odm(
                    &blob,
                    &["etc", "init", &format!("{hal_package_name}.rc")],
                    &mut odm_writer,
                )?);
            }
            // If a HAL manifest contains `vintf_manifest`, copy that file to
            // `etc/vintf/manifest/{hal_package_name}.xml` in the ODM filesystem.
            if let Some(blob) = hal_manifest.vintf_manifest {
                deps.add_input(self.clone().add_to_odm(
                    &blob,
                    &["etc", "vintf", "manifest", &format!("{hal_package_name}.xml")],
                    &mut odm_writer,
                )?);
            }
        }

        // Add the fstab.
        if let Some(fstab) = self.fstab {
            if let Some(file_name) = fstab.file_name() {
                match (fstab.file_stem(), fstab.extension()) {
                    (Some("fstab"), Some(_)) => {}
                    _ => bail!("fstab must be named \"fstab.<ro.hardware>\""),
                }
                let mut fstab_file =
                    File::open(&fstab).with_context(|| format!("opening fstab from {fstab:?}"))?;
                odm_writer
                    .add_file(&["etc", file_name], &mut fstab_file)
                    .with_context(|| format!("adding fstab to odm"))?;
            }
        }

        // Add any extra init files provided.
        for init in self.init {
            if let Some(file_name) = init.file_name() {
                let mut init_file =
                    File::open(&init).with_context(|| format!("opening init from {init}"))?;
                odm_writer
                    .add_file(&["etc", "init", file_name], &mut init_file)
                    .context("adding init to /odm/etc/init")?;
            } else {
                bail!("{init} doesn't have a filename");
            }
        }

        // Put all the ODM files into the container.
        let odm_files = odm_writer.inner.export().context("Exporting ODM files")?;
        for (dst, src) in &odm_files {
            let dst = format!("data/odm/{}", dst);
            builder
                .add_file_as_blob(dst, &src)
                .with_context(|| format!("Adding blob from file: {}", src))?;
            deps.add_output(src.clone());
        }

        // Build the starnix container.
        let metafar_path = self.outdir.join("meta.far");
        let manifest_path = self.outdir.join("package_manifest.json");
        builder.manifest_path(manifest_path);
        builder.build(&self.outdir, &metafar_path).context("Building starnix container")?;
        deps.add_outputs(
            [
                self.outdir.join("meta.far"),
                self.outdir.join("meta/fuchsia.abi/abi-revision"),
                self.outdir.join("meta/fuchsia.pkg/subpackages"),
                self.outdir.join("meta/package"),
            ]
            .iter()
            .map(|p| p.to_string()),
        );

        Ok(())
    }
}

impl StarnixContainerRepackager {
    pub fn build(self, deps: &mut Depfile) -> Result<Utf8PathBuf> {
        validate_manifests_in_base(&self.base, &self.hals, self.skip_subpackages)?;

        std::fs::create_dir_all(&self.outdir)
            .with_context(|| format!("Failed to create output directory {}", self.outdir))?;

        let new_base_package_manifest = PackageManifest::try_load_from(&self.base)
            .with_context(|| format!("Reading new base package: {}", self.base))?;

        let container_manifest = PackageManifest::try_load_from(&self.container_manifest_path)
            .with_context(|| {
                format!("Reading existing starnix container: {}", self.container_manifest_path)
            })?;

        // Track inputs early so we don't forget.
        deps.add_inputs(new_base_package_manifest.blobs().iter().map(|b| b.source_path.clone()));

        let mut builder = PackageBuilder::from_manifest(container_manifest.clone(), &self.outdir)
            .context("Parsing container package for repackaging")?;

        builder.overwrite_files(true);
        builder.overwrite_subpackages(true);

        // Add subpackages from the base package.
        for subpackage in new_base_package_manifest.subpackages() {
            let name =
                subpackage.name.parse::<RelativePackageUrl>().context("parsing subpackage name")?;
            builder.add_subpackage(
                &name,
                subpackage.merkle,
                subpackage.manifest_path.clone().into(),
            )?;
        }

        // Add HALs.
        if !self.hals.is_empty() {
            let mut overrides = Vec::new();

            for hal in &self.hals {
                let hal_package_manifest = PackageManifest::try_load_from(hal)?;
                if !self.skip_subpackages {
                    let name = hal_package_manifest
                        .name()
                        .to_string()
                        .parse::<RelativePackageUrl>()
                        .with_context(|| {
                            format!("parsing hal name: {}", hal_package_manifest.name())
                        })?;
                    builder.add_subpackage(
                        &name,
                        hal_package_manifest.hash(),
                        hal.clone().into(),
                    )?;
                }

                let hal_package_name = hal_package_manifest.name().to_string();
                let (hal_manifest, hal_manifest_source_path) =
                    hal_manifest::load_from_package(&hal_package_manifest)
                        .with_context(|| format!("Reading hal manifest from package: {}", hal))?;
                if let Some(src_path) = hal_manifest_source_path {
                    deps.add_input(src_path);
                }
                if let Some(blob) = hal_manifest.init_rc {
                    deps.add_input(blob.source_path.clone());
                    overrides.push(StarnixFileOverride {
                        image_name: "odm".to_string(),
                        file_path: format!("etc/init/{hal_package_name}.rc"),
                        operation: StarnixFileOperation::Create(Utf8PathBuf::from(
                            blob.source_path,
                        )),
                        mode: None,
                        uid: None,
                        gid: None,
                        seclabel: Some("u:object_r:vendor_configs_file:s0".to_string()),
                    });
                }
                if let Some(blob) = hal_manifest.vintf_manifest {
                    deps.add_input(blob.source_path.clone());
                    overrides.push(StarnixFileOverride {
                        image_name: "odm".to_string(),
                        file_path: format!("etc/vintf/manifest/{hal_package_name}.xml"),
                        operation: StarnixFileOperation::Create(Utf8PathBuf::from(
                            blob.source_path,
                        )),
                        mode: None,
                        uid: None,
                        gid: None,
                        seclabel: Some("u:object_r:vendor_configs_file:s0".to_string()),
                    });
                }
            }

            if !overrides.is_empty() {
                let original_metadata = if let Some(blob) =
                    container_manifest.blobs().iter().find(|b| b.path == "data/odm/metadata.v1")
                {
                    let metadata_bytes = std::fs::read(&blob.source_path).with_context(|| {
                        format!("reading base ODM metadata from {}", blob.source_path)
                    })?;
                    ext4_metadata::Metadata::deserialize(&metadata_bytes)
                        .context("deserializing base ODM metadata")?
                } else {
                    ext4_metadata::Metadata::new()
                };

                let result = apply_overrides(original_metadata, overrides, "odm")
                    .context("applying HAL overrides to ODM")?;

                let metadata_bytes = result.metadata.serialize();

                let metadata_path = self.outdir.join("odm_metadata.v1");
                std::fs::write(&metadata_path, &metadata_bytes)
                    .with_context(|| format!("writing metadata to {metadata_path}"))?;

                builder.add_file_as_blob("data/odm/metadata.v1", &metadata_path)?;
                deps.add_output(&metadata_path);

                for (inode, src_path) in result.new_files {
                    let dst = format!("data/odm/{inode}");
                    builder.add_file_as_blob(dst, &src_path)?;
                    deps.add_input(&src_path);
                }
            }
        }

        // Add content blobs from the base package.
        for blob in new_base_package_manifest.blobs() {
            if blob.path != PackageManifest::META_FAR_BLOB_PATH {
                builder.add_file_as_blob(&blob.path, &blob.source_path)?;
            }
        }

        // Add meta.far contents (specifically components).
        let meta_far_blob = new_base_package_manifest
            .blobs()
            .iter()
            .find(|b| b.path == PackageManifest::META_FAR_BLOB_PATH)
            .context("base package missing meta.far")?;
        let meta_far_bytes = std::fs::read(&meta_far_blob.source_path).with_context(|| {
            format!("Failed to read meta.far blob at {}", meta_far_blob.source_path)
        })?;
        let mut far_reader =
            fuchsia_archive::Utf8Reader::new(std::io::Cursor::new(meta_far_bytes))?;
        let paths: Vec<String> = far_reader.list().map(|e| e.path().to_string()).collect();
        for path in paths {
            if path.ends_with(".cm") {
                let contents = far_reader.read_file(&path)?;
                builder.add_contents_to_far(&path, contents, &self.outdir)?;
            }
        }

        builder.abi_revision = new_base_package_manifest
            .abi_revision()
            .context("base package missing abi_revision")?;

        // Rebuild the starnix container.
        let metafar_path = self.outdir.join("meta.far");
        let output_manifest_path = self.outdir.join("package_manifest.json");
        builder.manifest_path(output_manifest_path.clone());
        builder.build(&self.outdir, &metafar_path).context("Building new starnix container")?;

        deps.add_outputs(
            [
                self.outdir.join("meta.far"),
                self.outdir.join("meta/fuchsia.abi/abi-revision"),
                self.outdir.join("meta/fuchsia.pkg/subpackages"),
                self.outdir.join("meta/package"),
                output_manifest_path.clone(),
            ]
            .iter()
            .map(|p| p.to_string()),
        );

        Ok(output_manifest_path)
    }
}

fn validate_manifests_in_base(
    base_package_path: &Utf8Path,
    hals: &[Utf8PathBuf],
    skip_subpackages: bool,
) -> Result<()> {
    let hal_names = hals
        .iter()
        .map(|hal| {
            let manifest = PackageManifest::try_load_from(hal)?;
            Ok(manifest.name().to_string())
        })
        .collect::<Result<Vec<String>>>()?;

    if hal_names.is_empty() {
        return Ok(());
    }

    let base_manifest = PackageManifest::try_load_from(base_package_path)?;
    let meta_far_blob = base_manifest
        .blobs()
        .iter()
        .find(|b| b.path == PackageManifest::META_FAR_BLOB_PATH)
        .context("base package missing meta.far")?;

    let meta_far_bytes = std::fs::read(&meta_far_blob.source_path)?;
    let mut far_reader = fuchsia_archive::Utf8Reader::new(Cursor::new(meta_far_bytes))?;
    let paths: Vec<String> = far_reader.list().map(|e| e.path().to_string()).collect();
    for path in paths {
        if path.ends_with(".cm") {
            let contents = far_reader.read_file(&path)?;
            validate_manifest_bytes(&contents, &hal_names, skip_subpackages)
                .with_context(|| format!("Validating manifest {}", path))?;
        }
    }
    Ok(())
}

fn validate_manifest_bytes(
    bytes: &[u8],
    hal_names: &[String],
    skip_subpackages: bool,
) -> Result<()> {
    let component_decl: fcdecl::Component =
        fidl::unpersist(bytes).context("failed to unpersist component decl")?;

    if let Some(children) = component_decl.children {
        for child in children {
            let Some(url_str) = child.url else {
                continue;
            };

            if let Ok(relative_url) = RelativeComponentUrl::parse(&url_str) {
                let package_name = relative_url.package_url().as_ref();
                if hal_names.iter().any(|n| n == package_name) {
                    if skip_subpackages {
                        bail!(
                            "HAL child '{}' uses relative URL '{}' but skip_subpackages is true",
                            child.name.as_deref().unwrap_or_default(),
                            url_str
                        );
                    }
                }
            } else if let Ok(absolute_url) = FuchsiaPkgAbsoluteComponentUrl::parse(&url_str) {
                let package_name = absolute_url.name().as_ref();
                if hal_names.iter().any(|n| n == package_name) {
                    if !skip_subpackages {
                        bail!(
                            "HAL child '{}' uses absolute URL '{}' but skip_subpackages is false",
                            child.name.as_deref().unwrap_or_default(),
                            url_str
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_config_schema::product_settings::{
        StarnixFileOperation as FileOperation, StarnixFileOverride as FileOverride,
    };
    use assert_matches::assert_matches;
    use ext4_metadata::{Metadata, NodeInfo, ROOT_INODE_NUM};
    use itertools::Itertools;
    use serde_json::Value;
    use std::str::FromStr;
    use tempfile::TempDir;

    const EXT4_IMAGE_PATH: &str =
        concat!(env!("ROOT_OUT_DIR"), "/test_data/gen-starnix-container/test.img");

    fn fake_base(outdir: &Utf8Path) -> Utf8PathBuf {
        // Build a fake "base".
        let base_manifest_path = outdir.join("base_package_manifest.json");
        let mut builder = PackageBuilder::new_platform_internal_package("test-base");
        let test_blob_path = outdir.join("test-blob-file");
        std::fs::write(&test_blob_path, "test-base-blob").unwrap();
        builder.add_file_as_blob("data/test", &test_blob_path).unwrap();
        builder.manifest_path(&base_manifest_path);
        let _ = builder.build(&outdir, outdir.join("base-meta.far")).unwrap();
        base_manifest_path
    }

    /// Test that the generator correctly produces a package manifest with the expected blobs and subpackages.
    #[test]
    fn test_generate() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        // Build a fake HAL.
        let hal_manifest_path = outdir.join("hal_package_manifest.json");
        let mut builder = PackageBuilder::new_platform_internal_package("test-hal");
        builder.add_contents_as_blob("data/hal", "test-hal-blob", &outdir).unwrap();
        builder.manifest_path(&hal_manifest_path);
        let _ = builder.build(&outdir, outdir.join("hal-meta.far")).unwrap();

        // Run the generator.
        let container = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path,
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: Some(Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap()),
            ramdisk: vec![],
            hals: vec![hal_manifest_path],
            init: vec![],
            skip_subpackages: false,
            fstab: None,
            file_overrides: vec![],
        };
        let mut deps = Depfile::new();
        container.build(&mut deps).unwrap();

        // Read the package manifest, and ensure the correct files are present as blobs, and the
        // HALs are listed as subpackages.
        let manifest_path = outdir.join("package_manifest.json");
        let manifest = PackageManifest::try_load_from(&manifest_path).unwrap();
        assert_eq!(manifest.name().as_ref(), "test-name");
        let (blobs, subpackages) = manifest.into_blobs_and_subpackages();
        assert_eq!(blobs.len(), 7);
        assert_eq!(subpackages.len(), 1);
        let blob_filenames: Vec<String> = blobs.iter().map(|b| b.path.clone()).collect();

        // Verify that the test blob content is preserved.
        let test_blob = blobs.iter().find(|b| b.path == "data/test").unwrap();
        let content = std::fs::read_to_string(&test_blob.source_path).unwrap();
        assert_eq!(content, "test-base-blob");

        // Check that the paths in the file are relative to the file, not the current directory.
        // We can't use the typed reader since it will resolve them to absolute paths.
        let manifest_file = File::open(&manifest_path)
            .with_context(|| format!("Opening package manifest: {manifest_path}"))?;
        let manifest_json: Value = serde_json::from_reader(manifest_file)?;

        let blob_source_paths: Vec<String> = manifest_json["blobs"]
            .as_array()
            .context("checking relative manifest blobs")?
            .iter()
            .filter_map(|b| b["source_path"].as_str())
            .map(|p| p.to_string())
            .sorted()
            .collect();

        assert_eq!(
            blob_filenames,
            vec![
                "meta/".to_string(),
                "data/odm/metadata.v1".to_string(),
                "data/system/13".to_string(),
                "data/system/metadata.v1".to_string(),
                "data/test".to_string(),
                "data/vendor/13".to_string(),
                "data/vendor/metadata.v1".to_string(),
            ]
        );

        assert_eq!(
            blob_source_paths,
            vec![
                "meta.far".to_string(),
                "odm/metadata.v1".to_string(),
                "system/13".to_string(),
                "system/metadata.v1".to_string(),
                "test-blob-file".to_string(),
                "vendor/13".to_string(),
                "vendor/metadata.v1".to_string(),
            ]
        );

        Ok(())
    }

    #[test]
    fn test_apply_overrides_create_dirs() {
        let mut metadata = Metadata::new();
        metadata.insert_directory(ROOT_INODE_NUM, 0o040000 | 0o755, 0, 0, Default::default());

        let overrides = vec![FileOverride {
            image_name: "system".into(),
            file_path: "a/b/c".into(),
            operation: FileOperation::Create("src/path".into()),
            mode: None,
            uid: None,
            gid: None,
            seclabel: None,
        }];

        let result = apply_overrides(metadata, overrides, "system").unwrap();
        let new_m = result.metadata;

        // Verify /a exists and is a directory.
        let root = new_m.get(ROOT_INODE_NUM).unwrap();
        let root_dir = match root.info() {
            ext4_metadata::NodeInfo::Directory(d) => d,
            _ => panic!("Expected directory"),
        };
        let a_inode = *root_dir.children.iter().find(|(k, _)| k.as_str() == "a").unwrap().1;

        let a_node = new_m.get(a_inode).unwrap();
        assert_eq!(a_node.mode & 0o170000, 0o040000); // Directory

        // Verify /a/b exists and is a directory.
        let a_dir = match a_node.info() {
            ext4_metadata::NodeInfo::Directory(d) => d,
            _ => panic!("Expected directory"),
        };
        let b_inode = *a_dir.children.iter().find(|(k, _)| k.as_str() == "b").unwrap().1;

        let b_node = new_m.get(b_inode).unwrap();
        assert_eq!(b_node.mode & 0o170000, 0o040000); // Directory

        // Verify /a/b/c exists and is a file!
        let b_dir = match b_node.info() {
            ext4_metadata::NodeInfo::Directory(d) => d,
            _ => panic!("Expected directory"),
        };
        let c_inode = *b_dir.children.iter().find(|(k, _)| k.as_str() == "c").unwrap().1;

        let c_node = new_m.get(c_inode).unwrap();
        assert_eq!(c_node.mode & 0o170000, 0o100000); // File
    }

    #[test]
    fn test_generator_remove_non_existent_file() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        let generator = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path.clone(),
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: None,
            ramdisk: vec![],
            hals: vec![],
            init: vec![],
            skip_subpackages: false,
            fstab: None,
            file_overrides: vec![FileOverride {
                image_name: "system".into(),
                file_path: "non/existent/file".into(),
                operation: FileOperation::Remove,
                mode: None,
                uid: None,
                gid: None,
                seclabel: None,
            }],
        };

        let mut deps = Depfile::new();
        let result = generator.build(&mut deps);
        assert!(result.is_err());
        let error_msg = format!("{:?}", result.err().unwrap());
        eprintln!("Error message: {}", error_msg);
        assert!(error_msg.contains("File to remove not found"));

        Ok(())
    }

    #[test]
    fn test_generator_overwrite_non_existent_file() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        // Create a file to use for override.
        let src_file = outdir.join("src_file");
        std::fs::write(&src_file, "new content").unwrap();

        let generator = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path.clone(),
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: None,
            ramdisk: vec![],
            hals: vec![],
            init: vec![],
            skip_subpackages: false,
            fstab: None,
            file_overrides: vec![FileOverride {
                image_name: "system".into(),
                file_path: "non/existent/file".into(),
                operation: FileOperation::Overwrite(src_file),
                mode: None,
                uid: None,
                gid: None,
                seclabel: None,
            }],
        };

        let mut deps = Depfile::new();
        let result = generator.build(&mut deps);
        assert!(result.is_err());
        let error_msg = format!("{:?}", result.err().unwrap());
        eprintln!("Error message: {}", error_msg);
        assert!(error_msg.contains("File to overwrite not found"));

        Ok(())
    }

    #[test]
    fn test_generator_override_create() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        // Create a file to use for override.
        let src_file = outdir.join("src_file");
        std::fs::write(&src_file, "new content").unwrap();

        let generator = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path.clone(),
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: None,
            ramdisk: vec![],
            hals: vec![],
            init: vec![],
            skip_subpackages: false,
            fstab: None,
            file_overrides: vec![FileOverride {
                image_name: "system".into(),
                file_path: "new_file".into(),
                operation: FileOperation::Create(src_file),
                mode: Some(0o100000 | 0o755),
                uid: Some(1000),
                gid: Some(1000),
                seclabel: None,
            }],
        };
        let mut deps = Depfile::new();
        generator.build(&mut deps).unwrap();
        let output_manifest_path = outdir.join("package_manifest.json");

        // Verify the package manifest.
        let manifest = PackageManifest::try_load_from(&output_manifest_path).unwrap();
        let (blobs, _subpackages) = manifest.into_blobs_and_subpackages();

        let system_metadata_blob =
            blobs.iter().find(|b| b.path == "data/system/metadata.v1").unwrap();
        let m = Metadata::deserialize(&std::fs::read(&system_metadata_blob.source_path).unwrap())
            .unwrap();

        // new_file should be there.
        let new_file_inode = m.lookup(ROOT_INODE_NUM, "new_file").expect("new_file not found");
        let new_file_node = m.get(new_file_inode).expect("new_file node not found");
        assert_matches!(new_file_node.info(), NodeInfo::File(_));
        assert_eq!(new_file_node.mode, 0o100000 | 0o755);
        assert_eq!(new_file_node.uid, 1000);
        assert_eq!(new_file_node.gid, 1000);

        Ok(())
    }

    #[test]
    fn test_generator_override_overwrite() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        // Create a file to use for override.
        let src_file = outdir.join("src_file");
        std::fs::write(&src_file, "new content").unwrap();

        let generator = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path.clone(),
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: None,
            ramdisk: vec![],
            hals: vec![],
            init: vec![],
            skip_subpackages: false,
            fstab: None,
            file_overrides: vec![FileOverride {
                image_name: "system".into(),
                file_path: "foo/file".into(),
                operation: FileOperation::Overwrite(src_file),
                mode: Some(0o100000 | 0o644), // Force file mode
                uid: None,
                gid: None,
                seclabel: None,
            }],
        };
        let mut deps = Depfile::new();
        generator.build(&mut deps).unwrap();
        let output_manifest_path = outdir.join("package_manifest.json");

        // Verify the package manifest.
        let manifest = PackageManifest::try_load_from(&output_manifest_path).unwrap();
        let (blobs, _subpackages) = manifest.into_blobs_and_subpackages();

        let system_metadata_blob =
            blobs.iter().find(|b| b.path == "data/system/metadata.v1").unwrap();
        let m = Metadata::deserialize(&std::fs::read(&system_metadata_blob.source_path).unwrap())
            .unwrap();

        // foo/file should be there and be updated!
        let foo = m.lookup(ROOT_INODE_NUM, "foo").expect("foo not found");
        let file_inode = m.lookup(foo, "file").expect("file not found");
        let node = m.get(file_inode).expect("file node not found");
        assert_matches!(node.info(), NodeInfo::File(_));
        assert_eq!(node.mode, 0o100000 | 0o644);

        Ok(())
    }

    #[test]
    fn test_generator_override_remove() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        let generator = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path.clone(),
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: None,
            ramdisk: vec![],
            hals: vec![],
            init: vec![],
            skip_subpackages: false,
            fstab: None,
            file_overrides: vec![FileOverride {
                image_name: "system".into(),
                file_path: "foo/file".into(),
                operation: FileOperation::Remove,
                mode: None,
                uid: None,
                gid: None,
                seclabel: None,
            }],
        };

        let mut deps = Depfile::new();
        generator.build(&mut deps).unwrap();
        let output_manifest_path = outdir.join("package_manifest.json");

        // Verify the package manifest.
        let manifest = PackageManifest::try_load_from(&output_manifest_path).unwrap();
        let (blobs, _subpackages) = manifest.into_blobs_and_subpackages();

        let system_metadata_blob =
            blobs.iter().find(|b| b.path == "data/system/metadata.v1").unwrap();
        let m = Metadata::deserialize(&std::fs::read(&system_metadata_blob.source_path).unwrap())
            .unwrap();

        // foo/file should NOT be there.
        let foo = m.lookup(ROOT_INODE_NUM, "foo").expect("foo not found");
        assert!(m.lookup(foo, "file").is_err());

        // Verify that untouched existing container files are strictly preserved.
        let foo_node = m.get(foo).expect("foo node not found");
        assert_matches!(foo_node.info(), NodeInfo::Directory(_));

        Ok(())
    }

    #[test]
    fn test_repackage_preserves_custom_files() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        // Create a test host file to add as content.
        let src_file = outdir.join("test.txt");
        std::fs::write(&src_file, b"test content").unwrap();

        // Run generator to populate two new custom files in the remote bundle.
        let generator = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path.clone(),
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: None,
            ramdisk: vec![],
            hals: vec![],
            init: vec![],
            skip_subpackages: false,
            fstab: None,
            file_overrides: vec![
                FileOverride {
                    image_name: "system".into(),
                    file_path: "added_1".into(),
                    operation: FileOperation::Create(src_file.clone()),
                    mode: None,
                    uid: None,
                    gid: None,
                    seclabel: None,
                },
                FileOverride {
                    image_name: "system".into(),
                    file_path: "added_2".into(),
                    operation: FileOperation::Create(src_file),
                    mode: None,
                    uid: None,
                    gid: None,
                    seclabel: None,
                },
            ],
        };
        let mut deps = Depfile::new();
        generator.build(&mut deps).unwrap();
        let container_manifest_path = outdir.join("package_manifest.json");

        // Run repackager (WITHOUT overrides) on the generated container.
        let repackager = StarnixContainerRepackager {
            name: "test-repack".into(),
            outdir: outdir.join("repacked"),
            container_manifest_path,
            base: base_manifest_path,
            hals: vec![],
            skip_subpackages: false,
        };
        let mut deps = Depfile::new();
        let final_manifest_path = repackager.build(&mut deps).unwrap();

        // Verify final package contents.
        let manifest = PackageManifest::try_load_from(final_manifest_path).unwrap();
        let (blobs, _) = manifest.into_blobs_and_subpackages();

        let system_metadata_blob =
            blobs.iter().find(|b| b.path == "data/system/metadata.v1").unwrap();
        let m = Metadata::deserialize(&std::fs::read(&system_metadata_blob.source_path).unwrap())
            .unwrap();

        // Verify 'added_1' is preserved.
        let added_1_inode = m.lookup(ROOT_INODE_NUM, "added_1").expect("added_1 missing");
        let added_1_node = m.get(added_1_inode).expect("added_1 node missing");
        assert_matches!(added_1_node.info(), NodeInfo::File(_));

        // Verify 'added_2' is preserved.
        let added_2_inode = m.lookup(ROOT_INODE_NUM, "added_2").expect("added_2 missing");
        let added_2_node = m.get(added_2_inode).expect("added_2 node missing");
        assert_matches!(added_2_node.info(), NodeInfo::File(_));

        Ok(())
    }

    #[test]
    fn test_skip_subpackages() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        // Build a fake HAL.
        let hal_manifest_path = outdir.join("hal_package_manifest.json");
        let mut builder = PackageBuilder::new_platform_internal_package("test-hal");
        builder.add_contents_as_blob("data/hal", "test-hal-blob", &outdir).unwrap();
        builder.manifest_path(&hal_manifest_path);
        let _ = builder.build(&outdir, outdir.join("hal-meta.far")).unwrap();

        // Run the generator.
        let container = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path,
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: Some(Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap()),
            ramdisk: vec![],
            hals: vec![hal_manifest_path],
            init: vec![],
            skip_subpackages: true,
            fstab: None,
            file_overrides: vec![],
        };
        let mut deps = Depfile::new();
        container.build(&mut deps).unwrap();

        // Read the package manifest, and ensure the correct files are present as blobs, and the
        // HALs are not listed as subpackages.
        let manifest_path = outdir.join("package_manifest.json");
        let manifest = PackageManifest::try_load_from(&manifest_path).unwrap();
        assert_eq!(manifest.name().as_ref(), "test-name");
        let (blobs, subpackages) = manifest.into_blobs_and_subpackages();
        assert_eq!(blobs.len(), 7);
        assert_eq!(subpackages.len(), 0);

        Ok(())
    }

    fn fake_base_with_component(outdir: &Utf8Path, component: &fcdecl::Component) -> Utf8PathBuf {
        let base_manifest_path = outdir.join("base_package_manifest.json");
        let mut builder = PackageBuilder::new_platform_internal_package("test-base");
        let test_blob_path = outdir.join("test-blob-file");
        std::fs::write(&test_blob_path, "test-base-blob").unwrap();
        builder.add_file_as_blob("data/test", &test_blob_path).unwrap();

        let component_bytes = ::fidl::persist(component).unwrap();
        builder.add_contents_to_far("meta/test.cm", component_bytes, outdir).unwrap();

        builder.manifest_path(&base_manifest_path);
        let _ = builder.build(&outdir, outdir.join("base-meta.far")).unwrap();
        base_manifest_path
    }

    #[test]
    fn test_hal_manifest_validation() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();

        // Build a fake HAL.
        let hal_manifest_path = outdir.join("hal_package_manifest.json");
        let mut builder = PackageBuilder::new_platform_internal_package("test-hal");
        builder.add_contents_as_blob("data/hal", "test-hal-blob", &outdir).unwrap();
        builder.manifest_path(&hal_manifest_path);
        let _ = builder.build(&outdir, outdir.join("hal-meta.far")).unwrap();

        // Case 1: skip_subpackages = true, relative URL -> Should Fail
        {
            let component = fcdecl::Component {
                children: Some(vec![fcdecl::Child {
                    name: Some("test-hal-child".to_string()),
                    url: Some("test-hal#meta/some_hal.cm".to_string()),
                    startup: Some(fcdecl::StartupMode::Lazy),
                    ..Default::default()
                }]),
                ..Default::default()
            };
            let base_dir = outdir.join("case1-base");
            std::fs::create_dir_all(&base_dir).unwrap();
            let base_manifest_path = fake_base_with_component(&base_dir, &component);
            let container = StarnixContainerGenerator {
                name: "test-name".into(),
                outdir: outdir.join("case1"),
                base: base_manifest_path,
                system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
                vendor: Some(Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap()),
                ramdisk: vec![],
                hals: vec![hal_manifest_path.clone()],
                init: vec![],
                skip_subpackages: true,
                fstab: None,
                file_overrides: vec![],
            };
            let mut deps = Depfile::new();
            let result = container.build(&mut deps);
            assert!(result.is_err());
            assert!(format!("{:#}", result.unwrap_err()).contains("uses relative URL"));
        }

        // Case 2: skip_subpackages = true, absolute URL -> Should Pass
        {
            let component = fcdecl::Component {
                children: Some(vec![fcdecl::Child {
                    name: Some("test-hal-child".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/test-hal#meta/some_hal.cm".to_string()),
                    startup: Some(fcdecl::StartupMode::Lazy),
                    ..Default::default()
                }]),
                ..Default::default()
            };
            let base_dir = outdir.join("case2-base");
            std::fs::create_dir_all(&base_dir).unwrap();
            let base_manifest_path = fake_base_with_component(&base_dir, &component);
            let container = StarnixContainerGenerator {
                name: "test-name".into(),
                outdir: outdir.join("case2"),
                base: base_manifest_path,
                system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
                vendor: Some(Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap()),
                ramdisk: vec![],
                hals: vec![hal_manifest_path.clone()],
                init: vec![],
                skip_subpackages: true,
                fstab: None,
                file_overrides: vec![],
            };
            let mut deps = Depfile::new();
            let result = container.build(&mut deps);
            assert!(result.is_ok());
        }

        // Case 3: skip_subpackages = false, relative URL -> Should Pass
        {
            let component = fcdecl::Component {
                children: Some(vec![fcdecl::Child {
                    name: Some("test-hal-child".to_string()),
                    url: Some("test-hal#meta/some_hal.cm".to_string()),
                    startup: Some(fcdecl::StartupMode::Lazy),
                    ..Default::default()
                }]),
                ..Default::default()
            };
            let base_dir = outdir.join("case3-base");
            std::fs::create_dir_all(&base_dir).unwrap();
            let base_manifest_path = fake_base_with_component(&base_dir, &component);
            let container = StarnixContainerGenerator {
                name: "test-name".into(),
                outdir: outdir.join("case3"),
                base: base_manifest_path,
                system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
                vendor: Some(Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap()),
                ramdisk: vec![],
                hals: vec![hal_manifest_path.clone()],
                init: vec![],
                skip_subpackages: false,
                fstab: None,
                file_overrides: vec![],
            };
            let mut deps = Depfile::new();
            let result = container.build(&mut deps);
            assert!(result.is_ok());
        }

        // Case 4: skip_subpackages = false, absolute URL -> Should Fail
        {
            let component = fcdecl::Component {
                children: Some(vec![fcdecl::Child {
                    name: Some("test-hal-child".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/test-hal#meta/some_hal.cm".to_string()),
                    startup: Some(fcdecl::StartupMode::Lazy),
                    ..Default::default()
                }]),
                ..Default::default()
            };
            let base_dir = outdir.join("case4-base");
            std::fs::create_dir_all(&base_dir).unwrap();
            let base_manifest_path = fake_base_with_component(&base_dir, &component);
            let container = StarnixContainerGenerator {
                name: "test-name".into(),
                outdir: outdir.join("case4"),
                base: base_manifest_path,
                system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
                vendor: Some(Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap()),
                ramdisk: vec![],
                hals: vec![hal_manifest_path.clone()],
                init: vec![],
                skip_subpackages: false,
                fstab: None,
                file_overrides: vec![],
            };
            let mut deps = Depfile::new();
            let result = container.build(&mut deps);
            assert!(result.is_err());
            assert!(format!("{:#}", result.unwrap_err()).contains("uses absolute URL"));
        }

        // Case 5: Non-HAL child relative URL with skip_subpackages = true -> Should Pass
        {
            let component = fcdecl::Component {
                children: Some(vec![fcdecl::Child {
                    name: Some("other-child".to_string()),
                    url: Some("other-package#meta/other.cm".to_string()),
                    startup: Some(fcdecl::StartupMode::Lazy),
                    ..Default::default()
                }]),
                ..Default::default()
            };
            let base_dir = outdir.join("case5-base");
            std::fs::create_dir_all(&base_dir).unwrap();
            let base_manifest_path = fake_base_with_component(&base_dir, &component);
            let container = StarnixContainerGenerator {
                name: "test-name".into(),
                outdir: outdir.join("case5"),
                base: base_manifest_path,
                system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
                vendor: Some(Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap()),
                ramdisk: vec![],
                hals: vec![hal_manifest_path.clone()],
                init: vec![],
                skip_subpackages: true,
                fstab: None,
                file_overrides: vec![],
            };
            let mut deps = Depfile::new();
            let result = container.build(&mut deps);
            assert!(result.is_ok());
        }

        Ok(())
    }

    #[test]
    fn test_hal_init_rc() {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        // Build a fake HAL with an init.rc file.
        let hal_manifest_path = outdir.join("hal_package_manifest.json");
        let mut builder = PackageBuilder::new_platform_internal_package("test-hal");
        builder.add_contents_as_blob("data/hal", "test-hal-blob", &outdir).unwrap();
        builder.add_contents_as_blob("system/init.rc", "service foo bar", &outdir).unwrap();
        builder
            .add_contents_as_blob(
                "__android_config__/manifest.json",
                r#"{ "init_rc": "system/init.rc" }"#,
                &outdir,
            )
            .unwrap();
        builder.manifest_path(&hal_manifest_path);
        let _ = builder.build(&outdir, outdir.join("hal-meta.far")).unwrap();

        // Run the generator.
        let container = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path,
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: None,
            ramdisk: vec![],
            hals: vec![hal_manifest_path],
            init: vec![],
            skip_subpackages: false,
            fstab: None,
            file_overrides: vec![],
        };
        let mut deps = Depfile::new();
        container.build(&mut deps).unwrap();

        // Read the package manifest, and ensure the correct files are present as blobs, and
        // there is an additional `.rc` file corresponding to `test-hal`.
        let manifest_path = outdir.join("package_manifest.json");
        let manifest = PackageManifest::try_load_from(manifest_path).unwrap();
        assert_eq!(manifest.name().as_ref(), "test-name");
        let (blobs, _subpackages) = manifest.into_blobs_and_subpackages();
        assert_eq!(blobs.len(), 6);
        let blob_filenames: Vec<String> = blobs.iter().map(|b| b.path.clone()).collect();
        assert_eq!(
            blob_filenames,
            vec![
                "meta/".to_string(),
                "data/odm/7".to_string(),
                "data/odm/metadata.v1".to_string(),
                "data/system/13".to_string(),
                "data/system/metadata.v1".to_string(),
                "data/test".to_string(),
            ]
        );

        // Find the rc file and check its properties.
        let odm_metadata_path =
            &blobs.iter().find(|b| b.path == "data/odm/metadata.v1").unwrap().source_path;
        let m = Metadata::deserialize(
            &std::fs::read(odm_metadata_path).expect("Failed to read metadata"),
        )
        .expect("Failed to deserialize metadata");
        let etc = m.lookup(ROOT_INODE_NUM, "etc").expect("etc not found");
        let init = m.lookup(etc, "init").expect("init not found");
        let rc = m.lookup(init, "test-hal.rc").expect("rc not found");
        let rc = m.get(rc).expect("rc not found");
        assert_matches!(rc.info(), NodeInfo::File(_));
        assert_eq!(rc.mode, 0o100444);
        assert_eq!(rc.uid, 0);
        assert_eq!(rc.gid, 0);
    }

    #[test]
    fn test_hal_vintf_manifest() {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        // Build a fake HAL with an init.rc file.
        let hal_manifest_path = outdir.join("hal_package_manifest.json");
        let mut builder = PackageBuilder::new_platform_internal_package("test-hal");
        builder.add_contents_as_blob("data/hal", "test-hal-blob", &outdir).unwrap();
        builder
            .add_contents_as_blob("system/manifest.xml", "<manifest></manifest>", &outdir)
            .unwrap();
        builder
            .add_contents_as_blob(
                "__android_config__/manifest.json",
                r#"{ "vintf_manifest": "system/manifest.xml" }"#,
                &outdir,
            )
            .unwrap();
        builder.manifest_path(&hal_manifest_path);
        let _ = builder.build(&outdir, outdir.join("hal-meta.far")).unwrap();

        // Run the generator.
        let container = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path,
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: None,
            ramdisk: vec![],
            hals: vec![hal_manifest_path],
            init: vec![],
            skip_subpackages: false,
            fstab: None,
            file_overrides: vec![],
        };
        let mut deps = Depfile::new();
        container.build(&mut deps).unwrap();

        // Read the package manifest, and ensure the correct files are present as blobs, and
        // there is an additional `.xml` file corresponding to `test-hal`.
        let manifest_path = outdir.join("package_manifest.json");
        let manifest = PackageManifest::try_load_from(manifest_path).unwrap();
        assert_eq!(manifest.name().as_ref(), "test-name");
        let (blobs, _subpackages) = manifest.into_blobs_and_subpackages();
        assert_eq!(blobs.len(), 6);
        let blob_filenames: Vec<String> = blobs.iter().map(|b| b.path.clone()).collect();
        assert_eq!(
            blob_filenames,
            vec![
                "meta/".to_string(),
                "data/odm/7".to_string(),
                "data/odm/metadata.v1".to_string(),
                "data/system/13".to_string(),
                "data/system/metadata.v1".to_string(),
                "data/test".to_string(),
            ]
        );

        // Find the xml file and check its properties.
        let odm_metadata_path =
            &blobs.iter().find(|b| b.path == "data/odm/metadata.v1").unwrap().source_path;
        let m = Metadata::deserialize(
            &std::fs::read(odm_metadata_path).expect("Failed to read metadata"),
        )
        .expect("Failed to deserialize metadata");
        let etc = m.lookup(ROOT_INODE_NUM, "etc").expect("etc not found");
        let vintf = m.lookup(etc, "vintf").expect("vintf not found");
        let manifest = m.lookup(vintf, "manifest").expect("manifest not found");
        let xml = m.lookup(manifest, "test-hal.xml").expect("xml not found");
        let xml = m.get(xml).expect("xml not found");
        assert_matches!(xml.info(), NodeInfo::File(_));
        assert_eq!(xml.mode, 0o100444);
        assert_eq!(xml.uid, 0);
        assert_eq!(xml.gid, 0);
    }

    #[test]
    fn test_fstab() {
        const FSTAB: &'static str = r#"
# Android fstab file.
#<dev>  <mnt_point> <type>  <mnt_flags options> <fs_mgr_flags>
tmpfs   /data       tmpfs   defaults            wait
        "#;
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        // Make a fake fstab.
        let fstab_path = outdir.join("fstab.foo");
        std::fs::write(&fstab_path, FSTAB).unwrap();

        // Run the generator.
        let container = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path,
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: None,
            ramdisk: vec![],
            hals: vec![],
            init: vec![],
            skip_subpackages: false,
            fstab: Some(fstab_path),
            file_overrides: vec![],
        };
        let mut deps = Depfile::new();
        container.build(&mut deps).unwrap();

        // Read the package manifest, and ensure the correct files are present as blobs, and
        // there is an additional `.xml` file corresponding to `test-hal`.
        let manifest_path = outdir.join("package_manifest.json");
        let manifest = PackageManifest::try_load_from(manifest_path).unwrap();
        assert_eq!(manifest.name().as_ref(), "test-name");
        let (blobs, _subpackages) = manifest.into_blobs_and_subpackages();
        assert_eq!(blobs.len(), 6);
        let blob_filenames: Vec<String> = blobs.iter().map(|b| b.path.clone()).collect();
        assert_eq!(
            blob_filenames,
            vec![
                "meta/".to_string(),
                "data/odm/7".to_string(),
                "data/odm/metadata.v1".to_string(),
                "data/system/13".to_string(),
                "data/system/metadata.v1".to_string(),
                "data/test".to_string(),
            ]
        );

        let odm_metadata_path =
            &blobs.iter().find(|b| b.path == "data/odm/metadata.v1").unwrap().source_path;
        let m = Metadata::deserialize(
            &std::fs::read(odm_metadata_path).expect("Failed to read metadata"),
        )
        .expect("Failed to deserialize metadata");
        let etc = m.lookup(ROOT_INODE_NUM, "etc").expect("etc not found");
        let fstab = m.lookup(etc, "fstab.foo").expect("fstab not found");
        let fstab = m.get(fstab).expect("fstab not found");
        assert_matches!(fstab.info(), NodeInfo::File(_));
    }

    #[test]
    fn test_init() {
        const INIT: &str = "on boot\n  setprop foo.bar 1";
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        let init_path = outdir.join("test.rc");
        std::fs::write(&init_path, INIT).unwrap();

        // Run the generator.
        let container = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path,
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: None,
            ramdisk: vec![],
            hals: vec![],
            init: vec![init_path],
            skip_subpackages: false,
            fstab: None,
            file_overrides: vec![],
        };
        let mut deps = Depfile::new();
        container.build(&mut deps).unwrap();

        // Read the package manifest, and ensure the correct files are present as blobs, and
        // there is an additional `.xml` file corresponding to `test-hal`.
        let manifest_path = outdir.join("package_manifest.json");
        let manifest = PackageManifest::try_load_from(manifest_path).unwrap();
        assert_eq!(manifest.name().as_ref(), "test-name");
        let (blobs, _subpackages) = manifest.into_blobs_and_subpackages();
        assert_eq!(blobs.len(), 6);
        let blob_filenames: Vec<String> = blobs.iter().map(|b| b.path.clone()).collect();
        assert_eq!(
            blob_filenames,
            vec![
                "meta/".to_string(),
                "data/odm/7".to_string(),
                "data/odm/metadata.v1".to_string(),
                "data/system/13".to_string(),
                "data/system/metadata.v1".to_string(),
                "data/test".to_string(),
            ]
        );

        let odm_metadata_path =
            &blobs.iter().find(|b| b.path == "data/odm/metadata.v1").unwrap().source_path;
        let m = Metadata::deserialize(
            &std::fs::read(odm_metadata_path).expect("Failed to read metadata"),
        )
        .expect("Failed to deserialize metadata");
        let etc = m.lookup(ROOT_INODE_NUM, "etc").expect("etc not found");
        let etc_init = m.lookup(etc, "init").expect("init dir not found");
        let init = m.lookup(etc_init, "test.rc").expect("test.rc not found");
        let init_node = m.get(init).expect("test.rc not found");
        assert_matches!(init_node.info(), NodeInfo::File(_));
        let selinux_attr = init_node
            .extended_attributes
            .get(b"security.selinux".as_slice())
            .expect("selinux xattr missing");
        assert_eq!(selinux_attr, &b"u:object_r:vendor_configs_file:s0"[..]);
    }

    #[test]
    fn test_file_override_seclabel() {
        let mut original = Metadata::new();
        original.insert_directory(ROOT_INODE_NUM, 0o040000 | 0o755, 0, 0, Default::default());
        let tmp = TempDir::new().unwrap();
        let src_file = tmp.path().join("foo.rc");
        std::fs::write(&src_file, "on boot\n").unwrap();
        let src_file_utf8 = Utf8PathBuf::from_path_buf(src_file).unwrap();

        let overrides = vec![StarnixFileOverride {
            image_name: "odm".to_string(),
            file_path: "etc/init/foo.rc".to_string(),
            operation: StarnixFileOperation::Create(src_file_utf8),
            mode: None,
            uid: None,
            gid: None,
            seclabel: Some("u:object_r:vendor_configs_file:s0".to_string()),
        }];

        let result = crate::remote_bundle::apply_overrides(original, overrides, "odm").unwrap();
        let m = result.metadata;
        let etc = m.lookup(ROOT_INODE_NUM, "etc").expect("etc not found");
        let etc_init = m.lookup(etc, "init").expect("init dir not found");
        let foo_inode = m.lookup(etc_init, "foo.rc").expect("foo.rc not found");
        let foo_node = m.get(foo_inode).expect("foo.rc node not found");

        let selinux_attr = foo_node
            .extended_attributes
            .get(b"security.selinux".as_slice())
            .expect("selinux xattr missing");
        assert_eq!(selinux_attr, &b"u:object_r:vendor_configs_file:s0"[..]);
    }

    #[test]
    fn test_invalid_system_image() {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        // Create an invalid image file
        let invalid_image_path = outdir.join("invalid.img");
        std::fs::write(&invalid_image_path, "not an ext4 image").unwrap();

        let container = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path,
            system: invalid_image_path,
            vendor: None,
            ramdisk: vec![],
            hals: vec![],
            fstab: None,
            init: vec![],
            skip_subpackages: false,
            file_overrides: vec![],
        };

        let result = container.build(&mut Depfile::new());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{:#}", err).contains("Failed to extract EXT4 image"));
    }

    #[test]
    fn test_invalid_ramdisk() {
        let tmp = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmp.path()).unwrap();
        let base_manifest_path = fake_base(outdir);

        // Create an invalid ramdisk file
        let invalid_ramdisk_path = outdir.join("invalid_ramdisk");
        std::fs::write(&invalid_ramdisk_path, "not a cpio archive").unwrap();

        let container = StarnixContainerGenerator {
            name: "test-name".into(),
            outdir: outdir.to_owned(),
            base: base_manifest_path,
            system: Utf8PathBuf::from_str(EXT4_IMAGE_PATH).unwrap(),
            vendor: None,
            ramdisk: vec![invalid_ramdisk_path],
            hals: vec![],
            fstab: None,
            init: vec![],
            skip_subpackages: false,
            file_overrides: vec![],
        };

        let result = container.build(&mut Depfile::new());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{:#}", err).contains("Failed to parse ramdisk"));
    }
}
