// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Main entry point for the font service.

#![warn(missing_docs)]

mod font_service;

use self::font_service::{FontServiceBuilder, ProviderRequestStream};
use anyhow::{format_err, Context, Result};
use argh::FromArgs;
use config::Config;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component::inspector;
use log::{debug, error, info, warn};
use std::path::PathBuf;
use {fuchsia_trace as trace, fuchsia_trace_provider as trace_provider};

const FONT_MANIFEST_PATH: &str = "/config/data/all.font_manifest.json";
/// TODO(https://fxbug.dev/42120295): Remove after Chromium tests are made hermetic.
const TEST_COMPATIBILITY_FONT_MANIFEST_PATH: &str =
    "/config/data/downstream_test_fonts.font_manifest.json";

/// Default capacity of the in-memory font cache when not specified in manifest.
///
/// 4 MB is enough to fit several smaller fonts; large fonts will never be cached.
///
/// TODO(https://fxbug.dev/42125535): Listen for memory pressure events and trim cache.
const DEFAULT_CACHE_CAPACITY_BYTES: u64 = 4_000_000;

#[derive(FromArgs)]
/// Font Server
struct Args {
    /// load fonts from the specified font manifest file instead of the default
    #[argh(option, short = 'm')]
    font_manifest: Option<String>,
    /// no-op, deprecated
    #[argh(switch, short = 'n')]
    no_default_fonts: bool,
}

#[fuchsia::main(logging_tags = ["fonts"])]
async fn main() -> Result<()> {
    trace_provider::trace_provider_create_with_fdio();
    trace::instant!(c"fonts", c"startup", trace::Scope::Process);

    // We have to convert legacy uses of "--font-manifest=<PATH>" to "--font-manifest <PATH>".
    let arg_strings: Vec<String> = std::env::args()
        .collect::<Vec<String>>()
        .iter()
        .flat_map(|s| s.split("=").map(|s| s.to_owned()))
        .collect();
    let arg_strs: Vec<&str> = arg_strings.iter().map(|s| s.as_str()).collect();

    let args: Args = Args::from_args(&[arg_strs[0]], &arg_strs[1..])
        .map_err(|early_exit| format_err!("{}", early_exit.output))
        .context("malformed args, this is a fatal error")?;

    if args.no_default_fonts {
        warn!("--no-default-fonts is deprecated and is treated as a no-op")
    }

    let structured_config = font_service::config::as_ref();
    let font_manifest_paths = select_manifests(&args, structured_config)
        .context("no usable font manifests, this is a fatal error")?;

    let mut service_builder = FontServiceBuilder::with_default_asset_loader(
        DEFAULT_CACHE_CAPACITY_BYTES,
        inspector().root(),
    );
    debug!("Building service with manifest(s) {:?}", font_manifest_paths);
    for path in &font_manifest_paths {
        service_builder.add_manifest_from_file(path);
    }
    let service = service_builder.build().await.map_err(|err| {
        error!("Failed to build service with manifest(s) {:?}: {:#?}", font_manifest_paths, err);
        err
    })?;
    debug!("Built service with manifest(s) {:?}", font_manifest_paths);

    debug!("Adding FIDL services");
    let mut fs = ServiceFs::new();
    fs.dir("svc")
        .add_fidl_service(ProviderRequestStream::Stable)
        .add_fidl_service(ProviderRequestStream::Experimental);
    fs.take_and_serve_directory_handle()
        .context("could not serve directory handle, this is a fatal error")?;

    let _inspect_server_task =
        inspect_runtime::publish(inspector(), inspect_runtime::PublishOptions::default());

    let fs = fs;
    service.run(fs).await;

    Ok(())
}

/// Negotiate which manifest(s) to load.
/// TODO(https://fxbug.dev/42120295): Remove compatibility manifest after Chromium tests are made hermetic.
fn select_manifests(args: &Args, config: &Config) -> Result<Vec<PathBuf>> {
    select_manifests_for_test(args, config, /*check_files=*/ true)
}

// If `check_files` == false, the code will not go out to the filesystem to verify
// that a file exists; useful for lightweight testing.
fn select_manifests_for_test(
    args: &Args,
    config: &Config,
    check_files: bool,
) -> Result<Vec<PathBuf>> {
    let mut manifest_paths: Vec<PathBuf> = vec![];

    // Load a font manifest from structured config.
    let manifest_from_config = PathBuf::from(&config.font_manifest);
    let manifest_str = manifest_from_config.as_os_str();
    if manifest_str.len() != 0 {
        if !check_files || manifest_from_config.is_file() {
            info!("Adding manifest file: {}", manifest_from_config.display());
            manifest_paths.push(manifest_from_config);
        } else {
            // A file name was specified in the structured config, but that file
            // could not be found. This is most likely not expected.
            warn!(
                "Structured config requested font manifest that was not found: {}",
                manifest_from_config.display()
            );
        }
    }

    // Also try loading a font manifest from the command line args.
    let main_manifest_path = match &args.font_manifest {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(FONT_MANIFEST_PATH),
    };
    if (main_manifest_path.is_file() || !check_files) && !main_manifest_path.as_os_str().is_empty()
    {
        manifest_paths.push(main_manifest_path);
    } else {
        warn!(
            concat!(
                "Specified manifest file {:?} does not exist. ",
                "Looking for test compatibility manifest instead."
            ),
            main_manifest_path
        );
    }

    // Support legacy non-hermetic tests (e.g. Chromium) that expect some minimum set of fonts but
    // don't specify what it should be.  This happens when a font manifest is specified but is not
    // found under the specified path.
    if manifest_paths.is_empty() {
        let compatibility_manifest_path = PathBuf::from(TEST_COMPATIBILITY_FONT_MANIFEST_PATH);
        if compatibility_manifest_path.is_file() || !check_files {
            manifest_paths.push(compatibility_manifest_path);
        }
    }

    // The ordering of manifests should not matter.
    manifest_paths.sort();

    if manifest_paths.is_empty() {
        Err(format_err!("Either no font manifests were specified, or they do not exist"))
    } else {
        Ok(manifest_paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(
        Args { font_manifest: Some("".into()), no_default_fonts: false },
        Config { font_manifest: "".into(), verbose_logging: true },
        vec![TEST_COMPATIBILITY_FONT_MANIFEST_PATH]; "empty means default manifest is used")]
    #[test_case(
        Args { font_manifest: Some("/test/foo.txt".into()), no_default_fonts: false },
        Config { font_manifest: "".into(), verbose_logging: true },
        vec!["/test/foo.txt"]; "from args only")]
    #[test_case(
        Args { font_manifest: None, no_default_fonts: false },
        Config { font_manifest: "/test/foo.txt".into(), verbose_logging: true },
        vec![FONT_MANIFEST_PATH, "/test/foo.txt" ]; "from config only")]
    #[test_case(
        Args { font_manifest: Some("/test/foo.txt".into()), no_default_fonts: false },
        Config { font_manifest: "/test/foo2.txt".into(), verbose_logging: true },
        vec!["/test/foo.txt", "/test/foo2.txt"]; "from both")]
    fn test_manifest_selection(args: Args, config: Config, expected: Vec<&str>) {
        let result = select_manifests_for_test(&args, &config, /*check_files=*/ false).unwrap();
        let actual: Vec<&str> = result.iter().map(|p| p.as_os_str().to_str().unwrap()).collect();
        assert_eq!(expected, actual);
    }
}
