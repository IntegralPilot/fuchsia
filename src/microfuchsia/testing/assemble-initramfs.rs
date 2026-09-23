// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeSet;
use std::io::{self, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::{env, fs};

use anyhow::{Context as _, bail};
use argh::FromArgs;
use camino::Utf8PathBuf;
use cpio::{newc, write_cpio};
use product_bundle::{ProductBundle, ProductBundleV2};
use serde::Deserialize;

/// Constructs a Linux initramfs for testing comprised of a distribution
/// manifest, and the boot shim and ZBI from a product bundle.
#[derive(Debug, FromArgs)]
struct Args {
    /// path to the input distribution manifest.
    #[argh(option)]
    pub distribution_manifest: String,

    /// path to a product bundle manifest.
    #[argh(option)]
    pub product_bundle_manifest: String,

    /// path at which to write the CPIO archive.
    #[argh(option)]
    pub output: String,

    /// path at which to write the depfile.
    #[argh(option)]
    pub depfile: String,
}

#[derive(Debug, Deserialize)]
struct ProductBundleManifestEntry {
    json: Utf8PathBuf,
    path: Utf8PathBuf,
}

#[derive(Debug, Deserialize)]
struct Entry {
    source: Utf8PathBuf,
    destination: Utf8PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args: Args = argh::from_env();

    // Pick out our product bundle from the manifest of them.
    let pb_manifest_content =
        fs::read_to_string(&args.product_bundle_manifest).with_context(|| {
            format!("Failed to read product bundle manifest {}", args.product_bundle_manifest)
        })?;
    let pb_manifest: Vec<ProductBundleManifestEntry> = serde_json::from_str(&pb_manifest_content)
        .with_context(|| {
        format!("Failed to deserialize product bundle manifest {}", args.product_bundle_manifest)
    })?;
    if pb_manifest.len() != 1 {
        bail!("Expected exactly one product bundle manifest entry; found {}", pb_manifest.len());
    }
    let pb_json_path = &pb_manifest[0].json; // An input in the depfile
    let product_bundle = ProductBundle::try_load_from(&pb_manifest[0].path)
        .with_context(|| format!("Failed to load product bundle {}", pb_manifest[0].path))?;
    let product_bundle: &ProductBundleV2 = match &product_bundle {
        ProductBundle::V2(pb) => pb,
    };

    // Load our distribution manifest entries...
    let dist_manifest_content =
        fs::read_to_string(&args.distribution_manifest).with_context(|| {
            format!("Failed to read distribution manifest {}", args.distribution_manifest)
        })?;
    let mut entries: Vec<Entry> =
        serde_json::from_str(&dist_manifest_content).with_context(|| {
            format!("Failed to deserialize distribution manifest {}", args.product_bundle_manifest)
        })?;
    let dist_manifest_len = entries.len();

    // ...and extend it with the QEMU kernel and ZBI from the product bundle.
    let cwd = env::current_dir()?;
    if let Some(images) = &product_bundle.system_a {
        for image in images {
            match image {
                assembled_system::Image::QemuKernel(path) => {
                    entries.push(Entry {
                        source: path.strip_prefix(&cwd).unwrap().to_path_buf(),
                        destination: Utf8PathBuf::from("data/kernel"),
                    });
                }
                assembled_system::Image::ZBI { path, .. } => {
                    entries.push(Entry {
                        source: path.strip_prefix(&cwd).unwrap().to_path_buf(),
                        destination: Utf8PathBuf::from("data/ramdisk"),
                    });
                }
                _ => {}
            }
        }
    }
    if entries.len() != dist_manifest_len + 2 {
        bail!("failed to find both the QEMU kernel and ZBI in the product bundle");
    }

    // We'll need to create the ancestor directories of the desired file
    // contents as well.
    let mut directories = BTreeSet::new();
    for entry in &entries {
        for parent in entry.destination.ancestors().skip(1) {
            if parent.as_str().is_empty() || !directories.insert(parent) {
                break;
            }
        }
    }

    // Create the CPIO archive.
    let mut queued = Vec::new();
    for dir in &directories {
        let mode = u32::from(newc::ModeFileType::Directory) | 0o755;
        let builder = newc::Builder::new(dir.as_str()).mode(mode);
        queued.push((builder, io::Cursor::new(Vec::new())));
    }
    for entry in &entries {
        let content =
            fs::read(&entry.source).with_context(|| format!("Failed to read {}", entry.source))?;
        let metadata = fs::metadata(&entry.source)
            .with_context(|| format!("Failed to read metadata of {}", entry.source))?;
        let builder =
            newc::Builder::new(&entry.destination.as_str()).mode(metadata.permissions().mode());
        queued.push((builder, io::Cursor::new(content)));
    }
    let output_file = fs::File::create(&args.output)
        .with_context(|| format!("Failed to create file at {}", args.output))?;
    let mut writer = io::BufWriter::new(output_file);
    let _ = write_cpio(queued.into_iter(), &mut writer)
        .with_context(|| "Failed to create CPIO archive")?;
    writer.flush()?;

    // Write the depfile.
    let sources = entries.iter().map(|entry| entry.source.as_str()).collect::<Vec<_>>();
    let depfile_content = format!("{}: {} {pb_json_path}", args.output, sources.join(" "));
    fs::write(&args.depfile, depfile_content)
        .with_context(|| format!("Failed to write depfile at {}", args.depfile))?;

    Ok(())
}
