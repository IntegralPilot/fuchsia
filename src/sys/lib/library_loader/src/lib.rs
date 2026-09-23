// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl::prelude::*;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_ldsvc::{LoaderRequest, LoaderRequestStream};
use fuchsia_async as fasync;
use futures::{TryFutureExt, TryStreamExt};
use log::*;

/// Helper function to load `object_name` from `search_dirs`.
/// This function looks in the given directories, and returns the
/// first VMO matching |object_name| that is found.
pub async fn load_object(
    search_dirs: &Vec<fio::DirectoryProxy>,
    object_name: &str,
) -> Result<zx::Vmo, Vec<Error>> {
    let mut errors = vec![];
    for dir_proxy in search_dirs {
        match load_vmo(dir_proxy, &object_name).await {
            Ok(b) => {
                return Ok(b);
            }
            Err(e) => errors.push(e),
        }
    }
    Err(errors.into())
}

/// start will expose the `fuchsia.ldsvc.Loader` service over the given channel, providing VMO
/// buffers of requested library object names from `lib_proxy`.
///
/// `lib_proxy` must have been opened with at minimum OPEN_RIGHT_READABLE and OPEN_RIGHT_EXECUTABLE
/// rights.
pub fn start(lib_proxy: fio::DirectoryProxy, chan: zx::Channel) {
    start_with_multiple_dirs(vec![lib_proxy], chan);
}

/// start_with_multiple_dirs will expose the `fuchsia.ldsvc.Loader` service over the given channel,
/// providing VMO buffers of requested library object names from any of the library directories in
/// `lib_dirs`.
///
/// Each library directory must have been opened with at minimum OPEN_RIGHT_READABLE and
/// OPEN_RIGHT_EXECUTABLE rights.
pub fn start_with_multiple_dirs(lib_dirs: Vec<fio::DirectoryProxy>, chan: zx::Channel) {
    fasync::Task::spawn(
        async move {
            let mut search_dirs = lib_dirs.clone();
            // Wait for requests
            let mut stream = LoaderRequestStream::from_channel(fasync::Channel::from_channel(chan));
            while let Some(req) = stream.try_next().await? {
                match req {
                    LoaderRequest::Done { control_handle } => {
                        control_handle.shutdown();
                    }
                    LoaderRequest::LoadObject { object_name, responder } => {
                        match load_object(&search_dirs, &object_name).await {
                            Ok(vmo) => responder.send(zx::sys::ZX_OK, Some(vmo))?,
                            Err(e) => {
                                warn!("failed to load object: {:?}", e);
                                responder.send(zx::sys::ZX_ERR_NOT_FOUND, None)?;
                            }
                        }
                    }
                    LoaderRequest::Config { config, responder } => {
                        match parse_config_string(&lib_dirs, &config) {
                            Ok(new_search_path) => {
                                search_dirs = new_search_path;
                                responder.send(zx::sys::ZX_OK)?;
                            }
                            Err(e) => {
                                warn!("failed to parse config: {}", e);
                                responder.send(zx::sys::ZX_ERR_INVALID_ARGS)?;
                            }
                        }
                    }
                    LoaderRequest::Clone { loader, responder } => {
                        start_with_multiple_dirs(lib_dirs.clone(), loader.into_channel());
                        responder.send(zx::sys::ZX_OK)?;
                    }
                }
            }
            Ok(())
        }
        .unwrap_or_else(|e: Error| warn!("couldn't run library loader service: {}", e)),
    )
    .detach();
}

/// load_vmo will attempt to open the provided name in `dir` and return an executable VMO
/// with the contents.
///
/// `dir` must have been opened with at minimum OPEN_RIGHT_READABLE and OPEN_RIGHT_EXECUTABLE
/// rights.
pub async fn load_vmo<'a>(
    dir: &impl fuchsia_component::directory::AsRefDirectory,
    object_name: &'a str,
) -> Result<zx::Vmo, Error> {
    let file_proxy =
        fuchsia_component::directory::open_file_async(dir, object_name, fio::RX_STAR_DIR)?;
    // TODO(https://fxbug.dev/42129773): This does not ask or wait for a Describe event, which means a failure to
    // open the file will appear as a PEER_CLOSED error on this call.
    let vmo = file_proxy
        .get_backing_memory(
            // Clone the VMO because it could still be written by the debugger.
            fio::VmoFlags::READ | fio::VmoFlags::EXECUTE | fio::VmoFlags::PRIVATE_CLONE,
        )
        .await
        .map_err(|e| format_err!("reading object at {:?} failed: {}", object_name, e))?
        .map_err(|status| {
            let status = zx::Status::err_from_raw(status);
            format_err!("reading object at {:?} failed: {}", object_name, status)
        })?;
    Ok(vmo)
}

/// parses a config string from the `fuchsia.ldsvc.Loader` service. See
/// `//docs/concepts/booting/program_loading.md` for a description of the format. Returns the set
/// of directories which should be searched for objects.
pub fn parse_config_string(
    lib_dirs: &Vec<fio::DirectoryProxy>,
    config: &str,
) -> Result<Vec<fio::DirectoryProxy>, Error> {
    if config.contains("/") {
        return Err(format_err!("'/' character found in loader service config string"));
    }
    let (config, search_root) = match config.strip_suffix('!') {
        Some(config) => (config, false),
        None => (config, true),
    };
    let mut search_dirs = vec![];
    for dir_proxy in lib_dirs {
        let sub_dir_proxy = fuchsia_fs::directory::open_directory_async(
            dir_proxy,
            config,
            fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_EXECUTABLE,
        )?;
        search_dirs.push(sub_dir_proxy);
    }
    if search_root {
        search_dirs.append(&mut lib_dirs.clone());
    }
    Ok(search_dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_ldsvc::LoaderMarker;

    async fn list_directory<'a>(root_proxy: &'a fio::DirectoryProxy) -> Vec<String> {
        let entries = fuchsia_fs::directory::readdir(root_proxy).await.expect("readdir failed");
        entries.iter().map(|entry| entry.name.clone()).collect::<Vec<String>>()
    }

    #[fuchsia::test]
    async fn load_objects_test() -> Result<(), Error> {
        // Open this test's real /pkg/lib directory to use for this test, and then check to see
        // whether an asan subdirectory is present, and use it instead if so.
        // TODO(https://fxbug.dev/42061196): Replace conditional logic with a pseudo-directory using Rust VFS.
        let rights = fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_EXECUTABLE;
        let mut pkg_lib = fuchsia_fs::directory::open_in_namespace("/pkg/lib", rights)?;
        let entries = list_directory(&pkg_lib).await;
        if let Some(name) = [
            "asan",
            "asan-ubsan",
            "coverage",
            "coverage-cts",
            "coverage-rust",
            "hwasan",
            "hwasan-ubsan",
            "profile",
        ]
        .iter()
        .find(|&&name| entries.iter().any(|f| f == name))
        {
            pkg_lib = fuchsia_fs::directory::open_directory_async(&pkg_lib, name, rights)?;
        }
        let (loader_proxy, loader_service) = fidl::endpoints::create_proxy::<LoaderMarker>();
        start(pkg_lib.into(), loader_service.into_channel());

        for (obj_name, should_succeed) in vec![
            // Should be able to access lib/ld.so.1
            ("ld.so.1", true),
            // Should be able to access lib/libfdio.so
            ("libfdio.so", true),
            // Should not be able to access lib/lib/ld.so.1
            ("lib/ld.so.1", false),
            // Should not be able to access lib/../lib/ld.so.1
            ("../lib/ld.so.1", false),
            // Should not be able to access test/component_manager_tests
            ("../test/component_manager_tests", false),
            // Should not be able to access lib/bin/hello_world
            ("bin/hello_world", false),
            // Should not be able to access bin/hello_world
            ("../bin/hello_world", false),
            // Should not be able to access meta/hello_world.cm
            ("../meta/hello_world.cm", false),
        ] {
            let (res, o_vmo) = loader_proxy.load_object(obj_name).await?;
            if should_succeed {
                assert_eq!(zx::sys::ZX_OK, res, "loading {} did not succeed", obj_name);
                assert!(o_vmo.is_some());
            } else {
                assert_ne!(zx::sys::ZX_OK, res, "loading {} did not fail", obj_name);
                assert!(o_vmo.is_none());
            }
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn config_test() -> Result<(), Error> {
        // This /pkg/lib/config_test/ directory is added by the build rules for this test package,
        // since we need a directory that supports OPEN_RIGHT_EXECUTABLE. It contains a file 'foo'
        // which contains 'hippos' and a file 'bar/baz' (that is, baz in a subdirectory bar) which
        // contains 'rule'.
        // TODO(https://fxbug.dev/42061196): Replace conditional logic with a pseudo-directory using Rust VFS.
        let pkg_lib = fuchsia_fs::directory::open_in_namespace(
            "/pkg/lib/config_test/",
            fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_EXECUTABLE,
        )?;
        let (loader_proxy, loader_service) = fidl::endpoints::create_proxy::<LoaderMarker>();
        start(pkg_lib.into(), loader_service.into_channel());

        // Attempt to access things with different configurations
        for (obj_name, config, expected_result) in vec![
            // Should be able to load foo
            ("foo", None, Some("hippos")),
            // Should not be able to load bar (it's a directory)
            ("bar", None, None),
            // Should not be able to load baz (it's in a sub directory)
            ("baz", None, None),
            // Should be able to load baz with config "bar!" (only look in sub directory bar)
            ("baz", Some("bar!"), Some("rule")),
            // Should not be able to load foo with config "bar!" (only look in sub directory bar)
            ("foo", Some("bar!"), None),
            // Should be able to load foo with config "bar" (also look in sub directory bar)
            ("foo", Some("bar"), Some("hippos")),
            // Should be able to load baz with config "bar" (also look in sub directory bar)
            ("baz", Some("bar"), Some("rule")),
        ] {
            if let Some(config) = config {
                assert_eq!(zx::sys::ZX_OK, loader_proxy.config(config).await?);
            }

            let (res, o_vmo) = loader_proxy.load_object(obj_name).await?;
            if let Some(expected_result) = expected_result {
                assert_eq!(zx::sys::ZX_OK, res);
                let mut buf = vec![0; expected_result.len()];
                o_vmo.ok_or_else(|| format_err!("missing vmo"))?.read(&mut buf, 0)?;
                assert_eq!(expected_result.as_bytes(), buf.as_slice());
            } else {
                assert_ne!(zx::sys::ZX_OK, res);
                assert!(o_vmo.is_none());
            }
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn load_objects_multiple_dir_test() -> Result<(), Error> {
        // This /pkg/lib/config_test/ directory is added by the build rules for this test package,
        // since we need a directory that supports OPEN_RIGHT_EXECUTABLE. It contains a file 'foo'
        // which contains 'hippos' and a file 'bar/baz' (that is, baz in a subdirectory bar) which
        // contains 'rule'.
        // TODO(https://fxbug.dev/42061196): Replace conditional logic with a pseudo-directory using Rust VFS.
        let pkg_lib_1 = fuchsia_fs::directory::open_in_namespace(
            "/pkg/lib/config_test/",
            fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_EXECUTABLE,
        )?;
        let pkg_lib_2 = fuchsia_fs::directory::open_in_namespace(
            "/pkg/lib/config_test/bar",
            fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_EXECUTABLE,
        )?;

        let (loader_proxy, loader_service) = fidl::endpoints::create_proxy::<LoaderMarker>();
        start_with_multiple_dirs(
            vec![pkg_lib_1.into(), pkg_lib_2.into()],
            loader_service.into_channel(),
        );

        for (obj_name, should_succeed) in vec![
            // Should be able to access foo from dir #1
            ("foo", true),
            // Should be able to access baz from dir #2
            ("baz", true),
            // Should not be able to access bar (it's a directory)
            ("bar", false),
        ] {
            let (res, o_vmo) = loader_proxy.load_object(obj_name).await?;
            if should_succeed {
                assert_eq!(zx::sys::ZX_OK, res, "loading {} did not succeed", obj_name);
                assert!(o_vmo.is_some());
            } else {
                assert_ne!(zx::sys::ZX_OK, res, "loading {} did not fail", obj_name);
                assert!(o_vmo.is_none());
            }
        }
        Ok(())
    }
}
