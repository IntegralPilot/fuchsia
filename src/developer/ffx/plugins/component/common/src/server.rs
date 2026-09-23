// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_client::fidl::{DiscoverableProtocolMarker, Proxy};
use fdomain_fuchsia_developer_remotecontrol as rc;
use fdomain_fuchsia_pkg as fpkg;
use futures::future::FutureExt;
use futures::select;
use pkg::{PkgServerInstanceInfo, PkgServerInstances};
use rcs::open_with_timeout_at;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[must_use = "The guard must be kept alive to keep the package server running"]
pub struct ServerGuard {
    pub ffx_path: PathBuf,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        eprintln!("Stopping ephemeral package server...");
        let _ = Command::new(&self.ffx_path).arg("repository").arg("server").arg("stop").spawn();
    }
}

/// Trait for running package server commands.
/// Mockable for tests.
#[async_trait]
pub trait PackageServerRunner {
    async fn check_for_running_server(&self) -> anyhow::Result<bool>;
    async fn run_package_server(
        &self,
        build_dir: Option<&str>,
    ) -> anyhow::Result<Option<ServerGuard>>;
}

pub struct DefaultPackageServerRunner {
    pub process_dir: PathBuf,
}

#[async_trait]
impl PackageServerRunner for DefaultPackageServerRunner {
    async fn check_for_running_server(&self) -> anyhow::Result<bool> {
        let mgr = PkgServerInstances::new(self.process_dir.clone());
        match mgr.list_instances() {
            Ok(instances) if !instances.is_empty() => return Ok(true),
            _ => {}
        }

        eprintln!(
            "Warning: PkgServerInstances found no running servers or failed. Falling back to ffx command with timeout."
        );
        let ffx_path = std::env::current_exe()?;
        let child = Command::new(&ffx_path)
            .args(&["repository", "server", "list"])
            .stdout(std::process::Stdio::piped())
            .spawn()?;

        let child = std::sync::Arc::new(std::sync::Mutex::new(child));
        let child_clone = std::sync::Arc::clone(&child);

        let unblock_fut = fuchsia_async::unblock(move || {
            let mut output = String::new();
            let mut stdout = child_clone
                .lock()
                .unwrap()
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("no stdout"))?;
            use std::io::Read;
            stdout.read_to_string(&mut output)?;
            let status = child_clone.lock().unwrap().wait()?;
            Ok::<(std::process::ExitStatus, String), anyhow::Error>((status, output))
        });

        select! {
            res = unblock_fut.fuse() => {
                let (status, output) = res?;
                if status.success() {
                    return Ok(!output.trim().is_empty());
                }
                Ok(false)
            },
            _ = fuchsia_async::Timer::new(std::time::Duration::from_secs(5)).fuse() => {
                eprintln!("Warning: ffx repository server list timed out. Killing process.");
                let _ = child.lock().unwrap().kill();
                Ok(false)
            }
        }
    }

    async fn run_package_server(
        &self,
        build_dir: Option<&str>,
    ) -> anyhow::Result<Option<ServerGuard>> {
        let ffx_path = std::env::current_exe()?;
        let build_dir_owned = build_dir.map(|s| s.to_string());
        let args = get_server_start_args(build_dir_owned.as_deref());

        let child =
            Command::new(&ffx_path).args(&args).stderr(std::process::Stdio::piped()).spawn()?;

        let child = std::sync::Arc::new(std::sync::Mutex::new(child));
        let child_clone = std::sync::Arc::clone(&child);

        let unblock_fut = fuchsia_async::unblock(move || {
            let mut stderr = child_clone
                .lock()
                .unwrap()
                .stderr
                .take()
                .ok_or_else(|| anyhow::anyhow!("no stderr"))?;
            let mut err_output = String::new();
            use std::io::Read;
            stderr.read_to_string(&mut err_output)?;
            let status = child_clone.lock().unwrap().wait()?;
            Ok::<(std::process::ExitStatus, String), anyhow::Error>((status, err_output))
        });

        select! {
            res = unblock_fut.fuse() => {
                let (status, err_output) = res?;
                if status.success() {
                    return Ok(Some(ServerGuard { ffx_path }));
                }
                eprintln!(
                    "Warning: Failed to start package server: {}",
                    err_output
                );
                Ok(None)
            },
            _ = fuchsia_async::Timer::new(std::time::Duration::from_secs(5)).fuse() => {
                eprintln!("Warning: ffx repository server start timed out. Killing process.");
                let _ = child.lock().unwrap().kill();
                Ok(None)
            }
        }
    }
}

fn get_server_start_args(build_dir: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "repository".to_string(),
        "server".to_string(),
        "start".to_string(),
        "--background".to_string(),
        "--alias".to_string(),
        "fuchsia.com".to_string(),
    ];

    if let Some(bd) = build_dir {
        args.push("--auto-publish".to_string());
        let mut path = std::path::PathBuf::from(bd.trim());
        path.push("all_package_manifests.list");
        args.push(path.to_string_lossy().into_owned());
    }

    args
}

async fn check_iterator(
    iterator: fpkg::PackageIndexIteratorProxy,
    package_name: &str,
) -> anyhow::Result<bool> {
    loop {
        let entries = iterator.next().await?;
        if entries.is_empty() {
            break;
        }
        for entry in entries {
            let url = entry.package_url.url;
            let parsed_url = fuchsia_url::fuchsia_pkg::AbsolutePackageUrl::parse(&url)?;
            if parsed_url.name().to_string() == package_name {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Checks if a package is already on the device in either the base or cache package index.
async fn is_package_on_device(
    cache_proxy: &fpkg::PackageCacheProxy,
    client: std::sync::Arc<fdomain_client::Client>,
    package_name: &str,
) -> anyhow::Result<bool> {
    let (iterator_proxy, iterator_server) =
        client.create_proxy::<fpkg::PackageIndexIteratorMarker>();
    cache_proxy.base_package_index(iterator_server)?;
    if check_iterator(iterator_proxy, package_name).await? {
        return Ok(true);
    }

    let (iterator_proxy, iterator_server) =
        client.create_proxy::<fpkg::PackageIndexIteratorMarker>();
    cache_proxy.cache_package_index(iterator_server)?;
    if check_iterator(iterator_proxy, package_name).await? {
        return Ok(true);
    }

    Ok(false)
}

/// Starts the package server if it's not already running.
/// Returns a guard that stops the server when dropped.
pub async fn maybe_start_server<R: PackageServerRunner>(
    runner: &R,
    build_dir: Option<&std::path::Path>,
    rcs_proxy: Option<&rc::RemoteControlProxy>,
    package_url: Option<&fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl>,
) -> anyhow::Result<Option<ServerGuard>> {
    if let (Some(url), Some(rcs)) = (package_url, rcs_proxy) {
        let pkg_name = url.package_url().name().to_string();
        let cache_proxy = open_with_timeout_at::<fpkg::PackageCacheMarker>(
            Duration::from_secs(5),
            "core/pkg-cache",
            fdomain_fuchsia_sys2::OpenDirType::ExposedDir,
            fpkg::PackageCacheMarker::PROTOCOL_NAME,
            rcs,
        )
        .await;

        match cache_proxy {
            Ok(cache) => {
                if is_package_on_device(&cache, rcs.domain().clone(), &pkg_name).await? {
                    eprintln!(
                        "Package {} is in base or cache on device. Skipping server start.",
                        pkg_name
                    );
                    return Ok(None);
                }
            }
            Err(e) => {
                eprintln!(
                    "Warning: Failed to connect to PackageCache: {}. Assuming package is not on device.",
                    e
                );
            }
        }
    }

    if !runner.check_for_running_server().await? {
        eprintln!("No package server running. Starting an ephemeral one...");

        if let Some(dir) = build_dir {
            let manifest_path = dir.join("all_package_manifests.list");
            if !manifest_path.exists() {
                anyhow::bail!("Could not find package manifest at: {}", manifest_path.display());
            }
        }

        let build_dir_str = build_dir.map(|p| p.to_string_lossy());
        runner.run_package_server(build_dir_str.as_deref()).await
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdomain_local;
    use futures::TryStreamExt;
    use target_holders::fake_proxy;

    struct MockPackageServerRunner {
        server_running: bool,
        start_success: bool,
        calls: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl PackageServerRunner for MockPackageServerRunner {
        async fn check_for_running_server(&self) -> anyhow::Result<bool> {
            self.calls.lock().unwrap().push("check".to_string());
            Ok(self.server_running)
        }

        async fn run_package_server(
            &self,
            _build_dir: Option<&str>,
        ) -> anyhow::Result<Option<ServerGuard>> {
            self.calls.lock().unwrap().push("run".to_string());
            if self.start_success {
                let ffx_path = std::path::PathBuf::from("/bin/true");
                Ok(Some(ServerGuard { ffx_path }))
            } else {
                Ok(None)
            }
        }
    }

    #[test]
    fn test_get_server_start_args_no_build_dir() {
        let args = get_server_start_args(None);
        assert_eq!(
            args,
            vec!["repository", "server", "start", "--background", "--alias", "fuchsia.com"]
        );
    }

    #[test]
    fn test_get_server_start_args_with_build_dir() {
        let args = get_server_start_args(Some("out/default\n"));
        assert_eq!(
            args,
            vec![
                "repository",
                "server",
                "start",
                "--background",
                "--alias",
                "fuchsia.com",
                "--auto-publish",
                "out/default/all_package_manifests.list"
            ]
        );
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_maybe_start_server_already_running() {
        let runner = MockPackageServerRunner {
            server_running: true,
            start_success: false,
            calls: std::sync::Mutex::new(Vec::new()),
        };

        let result = maybe_start_server(&runner, None, None, None).await.unwrap();
        assert!(result.is_none());
        assert_eq!(runner.calls.lock().unwrap().len(), 1);
        assert_eq!(runner.calls.lock().unwrap()[0], "check");
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_maybe_start_server_not_running_success() {
        let runner = MockPackageServerRunner {
            server_running: false,
            start_success: true,
            calls: std::sync::Mutex::new(Vec::new()),
        };

        let result = maybe_start_server(&runner, None, None, None).await.unwrap();
        assert!(result.is_some());
        assert_eq!(runner.calls.lock().unwrap().len(), 2);
        assert_eq!(runner.calls.lock().unwrap()[0], "check");
        assert_eq!(runner.calls.lock().unwrap()[1], "run");
    }

    async fn setup_fake_iterator(
        client: std::sync::Arc<fdomain_client::Client>,
        entries: Vec<fpkg::PackageIndexEntry>,
    ) -> fpkg::PackageIndexIteratorProxy {
        let (proxy, server) = client.create_proxy::<fpkg::PackageIndexIteratorMarker>();
        fuchsia_async::Task::local(async move {
            let mut stream = server.into_stream();
            let mut sent = false;
            while let Ok(Some(req)) = stream.try_next().await {
                let fpkg::PackageIndexIteratorRequest::Next { responder } = req;
                if !sent {
                    sent = true;
                    responder.send(&entries).unwrap();
                } else {
                    responder.send(&[]).unwrap();
                }
            }
        })
        .detach();
        proxy
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_check_iterator_found() {
        let client = fdomain_local::local_client_empty();
        let entries = vec![
            fpkg::PackageIndexEntry {
                package_url: fpkg::PackageUrl { url: "fuchsia-pkg://fuchsia.com/pkg1".to_string() },
                meta_far_blob_id: fpkg::BlobId { merkle_root: [0; 32] },
            },
            fpkg::PackageIndexEntry {
                package_url: fpkg::PackageUrl { url: "fuchsia-pkg://fuchsia.com/pkg2".to_string() },
                meta_far_blob_id: fpkg::BlobId { merkle_root: [0; 32] },
            },
        ];
        let iterator = setup_fake_iterator(client.clone(), entries).await;
        assert!(check_iterator(iterator, "pkg2").await.unwrap());
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_check_iterator_not_found() {
        let client = fdomain_local::local_client_empty();
        let entries = vec![fpkg::PackageIndexEntry {
            package_url: fpkg::PackageUrl { url: "fuchsia-pkg://fuchsia.com/pkg1".to_string() },
            meta_far_blob_id: fpkg::BlobId { merkle_root: [0; 32] },
        }];
        let iterator = setup_fake_iterator(client.clone(), entries).await;
        assert!(!check_iterator(iterator, "pkg2").await.unwrap());
    }

    async fn setup_fake_cache(
        client: std::sync::Arc<fdomain_client::Client>,
        base_entries: Vec<fpkg::PackageIndexEntry>,
        cache_entries: Vec<fpkg::PackageIndexEntry>,
    ) -> fpkg::PackageCacheProxy {
        fake_proxy(client.clone(), move |req| match req {
            fpkg::PackageCacheRequest::BasePackageIndex { iterator, .. } => {
                let entries = base_entries.clone();
                fuchsia_async::Task::local(async move {
                    let mut stream = iterator.into_stream();
                    let mut sent = false;
                    while let Ok(Some(req)) = stream.try_next().await {
                        let fpkg::PackageIndexIteratorRequest::Next { responder } = req;
                        if !sent {
                            sent = true;
                            responder.send(&entries).unwrap();
                        } else {
                            responder.send(&[]).unwrap();
                        }
                    }
                })
                .detach();
            }
            fpkg::PackageCacheRequest::CachePackageIndex { iterator, .. } => {
                let entries = cache_entries.clone();
                fuchsia_async::Task::local(async move {
                    let mut stream = iterator.into_stream();
                    let mut sent = false;
                    while let Ok(Some(req)) = stream.try_next().await {
                        let fpkg::PackageIndexIteratorRequest::Next { responder } = req;
                        if !sent {
                            sent = true;
                            responder.send(&entries).unwrap();
                        } else {
                            responder.send(&[]).unwrap();
                        }
                    }
                })
                .detach();
            }
            other => panic!("Unexpected request: {:?}", other),
        })
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_is_package_on_device_in_base() {
        let client = fdomain_local::local_client_empty();
        let base_entries = vec![fpkg::PackageIndexEntry {
            package_url: fpkg::PackageUrl { url: "fuchsia-pkg://fuchsia.com/pkg1".to_string() },
            meta_far_blob_id: fpkg::BlobId { merkle_root: [0; 32] },
        }];
        let cache_entries = vec![];
        let cache_proxy = setup_fake_cache(client.clone(), base_entries, cache_entries).await;
        assert!(is_package_on_device(&cache_proxy, client.clone(), "pkg1").await.unwrap());
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_is_package_on_device_in_cache() {
        let client = fdomain_local::local_client_empty();
        let base_entries = vec![];
        let cache_entries = vec![fpkg::PackageIndexEntry {
            package_url: fpkg::PackageUrl { url: "fuchsia-pkg://fuchsia.com/pkg2".to_string() },
            meta_far_blob_id: fpkg::BlobId { merkle_root: [0; 32] },
        }];
        let cache_proxy = setup_fake_cache(client.clone(), base_entries, cache_entries).await;
        assert!(is_package_on_device(&cache_proxy, client.clone(), "pkg2").await.unwrap());
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_is_package_on_device_not_found() {
        let client = fdomain_local::local_client_empty();
        let base_entries = vec![];
        let cache_entries = vec![];
        let cache_proxy = setup_fake_cache(client.clone(), base_entries, cache_entries).await;
        assert!(!is_package_on_device(&cache_proxy, client.clone(), "pkg2").await.unwrap());
    }
}
