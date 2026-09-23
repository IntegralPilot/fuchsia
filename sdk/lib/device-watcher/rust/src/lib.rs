// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, format_err};
use fidl_fuchsia_device::ControllerMarker;
use fidl_fuchsia_io as fio;
use fuchsia_fs::directory::{WatchEvent, Watcher};
use futures::stream::{Stream, StreamExt as _, TryStreamExt as _};
use std::path::PathBuf;

// Device metadata.
pub struct DeviceInfo<'a> {
    // The device's file name within the directory in which it was found.
    pub filename: &'a str,
    // The device's topological path.
    pub topological_path: String,
}

/// Watches the directory for a device for which the predicate returns `Some(t)`
/// and returns `t`.
pub async fn wait_for_device_with<T>(
    dev_dir: &fio::DirectoryProxy,
    predicate: impl Fn(DeviceInfo<'_>) -> Option<T>,
) -> Result<T, anyhow::Error> {
    let stream = watch_for_files(dev_dir).await?;
    let stream = stream.try_filter_map(|filename| {
        let predicate = &predicate;
        async move {
            let filename =
                filename.to_str().ok_or_else(|| format_err!("to_str for filename failed"))?;
            let controller_filename = filename.to_owned() + "/device_controller";

            let (controller_proxy, server_end) =
                fidl::endpoints::create_proxy::<ControllerMarker>();
            #[cfg(fuchsia_api_level_at_least = "27")]
            if dev_dir
                .open(
                    &controller_filename,
                    fio::Flags::PROTOCOL_SERVICE,
                    &fio::Options::default(),
                    server_end.into_channel(),
                )
                .is_err()
            {
                return Ok(None);
            }
            #[cfg(not(fuchsia_api_level_at_least = "27"))]
            if dev_dir
                .open3(
                    &controller_filename,
                    fio::Flags::PROTOCOL_SERVICE,
                    &fio::Options::default(),
                    server_end.into_channel(),
                )
                .is_err()
            {
                return Ok(None);
            }

            let topological_path = controller_proxy.get_topological_path().await;
            let topological_path = match topological_path {
                Ok(topological_path) => topological_path,
                // Special case PEER_CLOSED; the peer is expected to close the
                // connection if it doesn't implement the controller protocol.
                Err(err) => match err {
                    fidl::Error::ClientChannelClosed { .. } => return Ok(None),
                    err => {
                        return Err(err).with_context(|| {
                            format!("failed to send get_topological_path on \"{}\"", filename)
                        });
                    }
                },
            };
            let topological_path = topological_path
                .map_err(zx_status::Status::err_from_raw)
                .with_context(|| format!("failed to get topological path on \"{}\"", filename))?;

            Ok(predicate(DeviceInfo { filename, topological_path }))
        }
    });
    futures::pin_mut!(stream);
    let item = stream.try_next().await?;
    item.ok_or_else(|| format_err!("stream ended prematurely"))
}

/// Returns a stream that contains the paths of any existing files and
/// directories in `dir` and any new files or directories created after this
/// function was invoked. These paths are relative to `dir`.
pub async fn watch_for_files(
    dir: &fio::DirectoryProxy,
) -> Result<impl Stream<Item = Result<PathBuf>> + use<>> {
    let watcher = Watcher::new(dir).await.context("failed to create watcher")?;
    Ok(watcher.map(|result| result.context("failed to get watcher event")).try_filter_map(|msg| {
        futures::future::ok(match msg.event {
            WatchEvent::EXISTING | WatchEvent::ADD_FILE => {
                if msg.filename == std::path::Path::new(".") { None } else { Some(msg.filename) }
            }
            _ => None,
        })
    }))
}

async fn wait_for_file(dir: &fio::DirectoryProxy, name: &str) -> Result<()> {
    let mut watcher = fuchsia_fs::directory::Watcher::new(dir).await?;
    while let Some(msg) = watcher.try_next().await? {
        if msg.event != fuchsia_fs::directory::WatchEvent::EXISTING
            && msg.event != fuchsia_fs::directory::WatchEvent::ADD_FILE
        {
            continue;
        }
        if msg.filename.to_str().unwrap() == name {
            return Ok(());
        }
    }
    unreachable!();
}

/// Open the path `name` within `dir`. This function waits for each directory to
/// be available before it opens it. If the path never appears this function
/// will wait forever.
async fn recursive_wait_and_open_with_flags<T, F>(
    mut dir: fio::DirectoryProxy,
    name: &str,
    flags: fio::Flags,
    op: F,
) -> Result<T>
where
    F: FnOnce(&fio::DirectoryProxy, &str, fio::Flags) -> T,
{
    let path = std::path::Path::new(name);
    let mut components = path.components().peekable();
    loop {
        let component =
            components.next().ok_or_else(|| format_err!("cannot wait for empty path"))?;
        let file = match component {
            std::path::Component::Normal(file) => file,
            // Per fuchsia.io/Directory.Open[0]:
            //
            // A leading '/' is allowed (and is treated the same way as if not present, i.e.
            // "/foo/bar' and "foo/bar" are the same).
            //
            // [0] https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.io/directory.fidl;l=211-237;drc=02426e16b637b25a21b1e53f9861855d476aaf49
            std::path::Component::RootDir => continue,
            component => {
                return Err(format_err!("path contains non-normal component {:?}", component));
            }
        };
        let file = file.to_str().unwrap();
        let () = wait_for_file(&dir, file).await?;
        if components.peek().is_some() {
            dir = fuchsia_fs::directory::open_directory_async(&dir, file, flags)?;
        } else {
            break Ok(op(&dir, file, flags));
        }
    }
}

/// Wait for `name` to be available in `dir`. This function waits for each directory along
/// the path and returns once it has waited on the final component in the path. If the path
/// never appears this function will wait forever.
pub async fn recursive_wait(dir: &fio::DirectoryProxy, name: &str) -> Result<()> {
    recursive_wait_and_open_with_flags(
        Clone::clone(dir),
        name,
        fuchsia_fs::PERM_READABLE,
        |_, _, _| (),
    )
    .await
}

/// Open the path `name` within `dir`. This function waits for each directory to
/// be available before it opens it. If the path never appears this function
/// will wait forever.
pub async fn recursive_wait_and_open_directory(
    dir: &fio::DirectoryProxy,
    name: &str,
) -> Result<fio::DirectoryProxy> {
    recursive_wait_and_open_with_flags(
        Clone::clone(dir),
        name,
        fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE,
        fuchsia_fs::directory::open_async::<fio::DirectoryMarker>,
    )
    .await
    .and_then(|res| res.map_err(Into::into))
}

/// Connect to an instance of FIDL protocol hosted at `name` within `dir`. This function waits for
/// each directory to be available before it opens it. If the path never appears this function will
/// wait forever.
pub async fn recursive_wait_and_open<P: fidl::endpoints::ProtocolMarker>(
    dir: &fio::DirectoryProxy,
    name: &str,
) -> Result<P::Proxy> {
    recursive_wait_and_open_with_flags(
        Clone::clone(dir),
        name,
        fuchsia_fs::PERM_READABLE,
        |dir, path, _flags| {
            // Cannot open services with other flags.
            fuchsia_fs::directory::open_async::<P>(dir, path, fio::Flags::PROTOCOL_SERVICE)
        },
    )
    .await
    .and_then(|res| res.map_err(Into::into))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_device as fdev;
    use futures::StreamExt;
    use std::collections::HashSet;
    use std::str::FromStr;
    use std::sync::Arc;
    use vfs::file::vmo::read_only;

    fn create_controller_service(topo_path: &'static str) -> Arc<vfs::service::Service> {
        vfs::service::host(move |mut stream: fdev::ControllerRequestStream| async move {
            match stream.try_next().await.unwrap() {
                Some(fdev::ControllerRequest::GetTopologicalPath { responder }) => {
                    let _ = responder.send(Ok(topo_path));
                }
                e => panic!("Unexpected request: {:?}", e),
            }
        })
    }

    #[fuchsia::test]
    async fn wait_for_device_by_topological_path() {
        let dir = vfs::pseudo_directory! {
          "a" => vfs::pseudo_directory! {
            "device_controller" => create_controller_service("/dev/test2/a/dev"),
          },
          "1" => vfs::pseudo_directory! {
            "device_controller" => create_controller_service("/dev/test2/1/dev"),
          },
          "x" => vfs::pseudo_directory! {
            "device_controller" => create_controller_service("/dev/test2/x/dev"),
          },
          "y" => vfs::pseudo_directory! {
            "device_controller" => create_controller_service("/dev/test2/y/dev"),
          },
        };
        let dir_proxy =
            vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new());
        let path = wait_for_device_with(&dir_proxy, |DeviceInfo { filename, topological_path }| {
            (topological_path == "/dev/test2/x/dev").then(|| filename.to_string())
        })
        .await
        .unwrap();
        assert_eq!("x", path);
    }

    #[fuchsia::test]
    async fn watch_for_two_files() {
        let dir = vfs::pseudo_directory! {
          "a" => read_only(b"/a"),
          "b" => read_only(b"/b"),
        };

        let dir_proxy =
            vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new());

        let stream = watch_for_files(&dir_proxy).await.unwrap();
        futures::pin_mut!(stream);
        let actual: HashSet<PathBuf> =
            vec![stream.next().await.unwrap().unwrap(), stream.next().await.unwrap().unwrap()]
                .into_iter()
                .collect();
        let expected: HashSet<PathBuf> =
            vec![PathBuf::from_str("a").unwrap(), PathBuf::from_str("b").unwrap()]
                .into_iter()
                .collect();
        assert_eq!(actual, expected);
    }

    #[fuchsia::test]
    async fn wait_for_device_topo_path_allows_files_and_dirs() {
        let dir = vfs::pseudo_directory! {
          "1" => vfs::pseudo_directory! {
            "test" => read_only("test file 1"),
            "test2" => read_only("test file 2"),
          },
          "2" => read_only("file 2"),
          "x" => vfs::pseudo_directory! {
            "device_controller" => create_controller_service("/dev/test2/x/dev"),
          },
          "3" => read_only("file 3"),
        };

        let dir_proxy =
            vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new());
        let path = wait_for_device_with(&dir_proxy, |DeviceInfo { filename, topological_path }| {
            (topological_path == "/dev/test2/x/dev").then(|| filename.to_string())
        })
        .await
        .unwrap();
        assert_eq!("x", path);
    }

    #[fuchsia::test]
    async fn open_two_directories() {
        let root = vfs::pseudo_directory! {
            "test" => vfs::pseudo_directory! {
                "dir" => vfs::pseudo_directory! {},
            },
        };
        let client =
            vfs::directory::serve_read_only(root, vfs::execution_scope::ExecutionScope::new());
        let directory = recursive_wait_and_open_directory(&client, "test/dir").await.unwrap();
        let () = directory.close().await.unwrap().unwrap();
    }

    #[fuchsia::test]
    async fn open_directory_with_leading_slash() {
        let root = vfs::pseudo_directory! {
            "test" => vfs::pseudo_directory! {},
        };
        let client =
            vfs::directory::serve_read_only(root, vfs::execution_scope::ExecutionScope::new());
        let directory = recursive_wait_and_open_directory(&client, "/test").await.unwrap();
        let () = directory.close().await.unwrap().unwrap();
    }
}
