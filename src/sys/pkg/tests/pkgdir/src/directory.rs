// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{PackageSource, dirs_to_test, repeat_by_n};
use anyhow::{Context as _, Error, anyhow};
use fidl::endpoints::{DiscoverableProtocolMarker as _, Proxy as _, create_proxy};
use fidl_fuchsia_io as fio;
use fuchsia_fs::directory::{DirEntry, DirentKind};
use futures::StreamExt;
use futures::future::Future;
use itertools::Itertools as _;
use pretty_assertions::assert_eq;
use std::collections::HashSet;

#[fuchsia::test]
async fn open() {
    for source in dirs_to_test().await {
        open_per_package_source(source).await
    }
}

async fn open_per_package_source(source: PackageSource) {
    // Testing dimensions:
    //   1. Receiver of the open call: /, meta/, subdir below meta/, subdir not below meta/
    //   2. Type of node the path points at: self, meta/, subdir below meta/, file below meta/,
    //      subdir not below meta/, file not below meta/ (not all receivers can open every type of
    //      target)
    //   3. Whether the path being opened is segmented
    // The flags and modes are handled by the helper functions.
    assert_open_root_directory(&source, ".", ".").await;
    assert_open_content_directory(&source, ".", "dir").await;
    assert_open_content_directory(&source, ".", "dir/dir").await;
    assert_open_content_file(&source, ".", "file").await;
    assert_open_content_file(&source, ".", "dir/file").await;
    assert_open_meta_as_directory_and_file(&source, ".", "meta").await;
    assert_open_meta_subdirectory(&source, ".", "meta/dir").await;
    assert_open_meta_file(&source, ".", "meta/file").await;

    // Self-opening "meta" does not trigger the file/dir duality.
    assert_open_meta_subdirectory(&source, "meta", ".").await;
    assert_open_meta_subdirectory(&source, "meta", "dir").await;
    assert_open_meta_subdirectory(&source, "meta", "dir/dir").await;
    assert_open_meta_file(&source, "meta", "file").await;
    assert_open_meta_file(&source, "meta", "dir/file").await;

    assert_open_meta_subdirectory(&source, "meta/dir", ".").await;
    assert_open_meta_subdirectory(&source, "meta/dir", "dir").await;
    assert_open_meta_subdirectory(&source, "meta/dir", "dir/dir").await;
    assert_open_meta_file(&source, "meta/dir", "file").await;
    assert_open_meta_file(&source, "meta/dir", "dir/file").await;

    assert_open_content_directory(&source, "dir", ".").await;
    assert_open_content_directory(&source, "dir", "dir").await;
    assert_open_content_directory(&source, "dir", "dir/dir").await;
    assert_open_content_file(&source, "dir", "file").await;
    assert_open_content_file(&source, "dir", "dir/file").await;
}

const ALL_FLAGS: [fio::Flags; 12] = [
    fio::PERM_READABLE,
    fio::PERM_WRITABLE,
    fio::PERM_EXECUTABLE,
    fio::Flags::FLAG_MAYBE_CREATE,
    fio::Flags::FLAG_MUST_CREATE,
    fio::Flags::FLAG_MAYBE_CREATE.union(fio::Flags::FLAG_MUST_CREATE),
    fio::Flags::FILE_APPEND,
    fio::Flags::FILE_TRUNCATE,
    fio::Flags::FLAG_SEND_REPRESENTATION,
    fio::Flags::PERM_INHERIT_WRITE,
    fio::Flags::PERM_INHERIT_EXECUTE,
    fio::Flags::PROTOCOL_SYMLINK,
];

async fn assert_open_root_directory(
    source: &PackageSource,
    parent_path: &str,
    child_base_path: &str,
) {
    let package_root = &source.dir;

    let success_flags = [
        fio::PERM_READABLE,
        fio::PERM_EXECUTABLE,
        fio::Flags::PROTOCOL_DIRECTORY,
        fio::Flags::PROTOCOL_NODE,
        fio::Flags::FLAG_SEND_REPRESENTATION,
        fio::Flags::PERM_INHERIT_WRITE,
        fio::Flags::PERM_INHERIT_EXECUTE,
    ];

    let child_paths = generate_valid_directory_paths(child_base_path);
    let lax_child_paths = generate_lax_directory_paths(child_base_path);
    let all_flag_and_child_paths =
        itertools::iproduct!(ALL_FLAGS, lax_child_paths.iter().map(String::as_str));

    let success_flags_and_child_paths =
        itertools::iproduct!(success_flags, child_paths.iter().map(String::as_str))
            .filter_map(filter_out_contradictory_open_parameters);
    assert_open_success(
        package_root,
        parent_path,
        success_flags_and_child_paths.clone(),
        verify_directory_opened,
    )
    .await;

    assert_open_flag_and_child_path_failure(
        package_root,
        parent_path,
        subtract(all_flag_and_child_paths, success_flags_and_child_paths).into_iter(),
        verify_open_failed,
    )
    .await;
}

fn filter_out_contradictory_open_parameters(
    (flag, child_path): (fio::Flags, &str),
) -> Option<(fio::Flags, &'_ str)> {
    if flag.intersects(fio::Flags::PROTOCOL_FILE) && child_path.ends_with('/') {
        None
    } else {
        Some((flag, child_path))
    }
}

async fn assert_open_success<V, Fut>(
    package_root: &fio::DirectoryProxy,
    parent_path: &str,
    allowed_flags_and_child_paths: impl Iterator<Item = (fio::Flags, &str)>,
    verifier: V,
) where
    V: Fn(fio::NodeProxy, fio::Flags) -> Fut,
    Fut: Future<Output = Result<(), Error>>,
{
    let parent = open_parent(package_root, parent_path).await;
    for (flag, child_path) in allowed_flags_and_child_paths {
        let node = open_node(&parent, flag, child_path);
        if let Err(e) = verifier(node, flag).await {
            panic!(
                "failed to verify open. parent: {parent_path:?}, child: {child_path:?}, flag: {flag:?}, \
                       error: {e:#}"
            );
        }
    }
}

async fn assert_open_content_directory(
    source: &PackageSource,
    parent_path: &str,
    child_base_path: &str,
) {
    let package_root = &source.dir;

    let success_flags = [
        fio::PERM_READABLE,
        fio::PERM_EXECUTABLE,
        fio::Flags::PROTOCOL_DIRECTORY,
        fio::Flags::PROTOCOL_NODE,
        fio::Flags::FLAG_SEND_REPRESENTATION,
        fio::Flags::PERM_INHERIT_WRITE,
        fio::Flags::PERM_INHERIT_EXECUTE,
    ];
    let child_paths = generate_valid_directory_paths(child_base_path);
    let lax_child_paths = generate_lax_directory_paths(child_base_path);
    let all_flag_and_child_paths =
        itertools::iproduct!(ALL_FLAGS, lax_child_paths.iter().map(String::as_str));

    let success_flags_and_child_paths =
        itertools::iproduct!(success_flags, child_paths.iter().map(String::as_str))
            .filter_map(filter_out_contradictory_open_parameters);
    assert_open_success(
        package_root,
        parent_path,
        success_flags_and_child_paths.clone(),
        verify_directory_opened,
    )
    .await;

    assert_open_flag_and_child_path_failure(
        package_root,
        parent_path,
        subtract(all_flag_and_child_paths, success_flags_and_child_paths).into_iter(),
        verify_open_failed,
    )
    .await;
}

fn subtract<'a, I, J, T>(minuend: I, subtrahend: J) -> Vec<T>
where
    I: IntoIterator<Item = T>,
    <I as IntoIterator>::IntoIter: Clone + 'a,
    J: IntoIterator<Item = T>,
    T: Eq + std::hash::Hash + 'a,
{
    let subtrahend = HashSet::<T>::from_iter(subtrahend);
    minuend.into_iter().filter(|v| !subtrahend.contains(v)).collect()
}

#[test]
fn test_subtract() {
    assert_eq!(subtract(["foo", "bar"], ["bar", "baz"]), vec!["foo"]);
}

async fn assert_open_flag_and_child_path_failure<V, Fut>(
    package_root: &fio::DirectoryProxy,
    parent_path: &str,
    disallowed_flags_and_child_paths: impl Iterator<Item = (fio::Flags, &str)>,
    verifier: V,
) where
    V: Fn(fio::NodeProxy) -> Fut,
    Fut: Future<Output = Result<(), Error>>,
{
    let parent = open_parent(package_root, parent_path).await;
    for (flag, child_path) in disallowed_flags_and_child_paths {
        let node = open_node(&parent, flag, child_path);
        if let Err(e) = verifier(node).await {
            panic!(
                "failed to verify open failed. parent: {parent_path:?}, child: {child_path:?}, flag: {flag:?}, \
                       error: {e:#}"
            );
        }
    }
}

async fn assert_open_content_file(
    source: &PackageSource,
    parent_path: &str,
    child_base_path: &str,
) {
    let package_root = &source.dir;

    let success_flags = [
        fio::PERM_READABLE,
        fio::PERM_EXECUTABLE,
        fio::Flags::PROTOCOL_NODE,
        fio::Flags::FLAG_SEND_REPRESENTATION,
        fio::Flags::PERM_INHERIT_WRITE,
        fio::Flags::PERM_INHERIT_EXECUTE,
        fio::Flags::PROTOCOL_FILE,
    ];

    let child_paths = generate_valid_file_paths(child_base_path);
    let lax_child_paths = generate_lax_directory_paths(child_base_path);
    let all_flag_and_child_paths =
        itertools::iproduct!(ALL_FLAGS, lax_child_paths.iter().map(String::as_str));

    let success_flags_and_child_paths =
        itertools::iproduct!(success_flags, child_paths.iter().map(String::as_str))
            .filter_map(filter_out_contradictory_open_parameters);
    assert_open_success(
        package_root,
        parent_path,
        success_flags_and_child_paths.clone(),
        verify_content_file_opened,
    )
    .await;

    assert_open_flag_and_child_path_failure(
        package_root,
        parent_path,
        subtract(all_flag_and_child_paths, success_flags_and_child_paths).into_iter(),
        verify_open_failed,
    )
    .await;
}

async fn assert_open_meta_as_directory_and_file(
    source: &PackageSource,
    parent_path: &str,
    child_base_path: &str,
) {
    let package_root = &source.dir;

    // To open "meta" as a directory either PROTOCOL_DIRECTORY or PROTOCOL_NODE must be specified.
    let directory_success_flags = [
        fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE,
        fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::FLAG_SEND_REPRESENTATION,
        fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::PERM_INHERIT_WRITE,
        fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::PERM_INHERIT_EXECUTE,
        fio::Flags::PROTOCOL_DIRECTORY
            | fio::Flags::PROTOCOL_NODE
            | fio::Flags::PERM_GET_ATTRIBUTES,
        fio::Flags::PROTOCOL_DIRECTORY
            | fio::Flags::PROTOCOL_NODE
            | fio::Flags::FLAG_SEND_REPRESENTATION,
    ];

    let directory_child_paths = generate_valid_directory_paths(child_base_path);
    let lax_child_paths = generate_lax_directory_paths(child_base_path);

    let directory_only_child_paths = generate_valid_directory_only_paths(child_base_path);
    let all_flag_and_child_paths =
        itertools::iproduct!(ALL_FLAGS, lax_child_paths.iter().map(String::as_str));

    let directory_flags_and_child_paths = itertools::iproduct!(
        directory_success_flags,
        directory_child_paths.iter().map(String::as_str)
    )
    .chain(itertools::iproduct!(
        directory_success_flags,
        directory_only_child_paths.iter().map(String::as_str)
    ))
    .filter_map(filter_out_contradictory_open_parameters);
    assert_open_success(
        package_root,
        parent_path,
        directory_flags_and_child_paths.clone(),
        verify_directory_opened,
    )
    .await;

    // To open "meta" as a directory, no protocols or PROTOCOL_FILE must be specified.
    let file_flags = [
        fio::PERM_READABLE,
        fio::Flags::FLAG_SEND_REPRESENTATION,
        fio::Flags::PERM_INHERIT_WRITE,
        fio::Flags::PERM_INHERIT_EXECUTE,
        fio::Flags::PROTOCOL_FILE,
        fio::Flags::PROTOCOL_FILE | fio::Flags::PROTOCOL_NODE,
    ];

    let file_child_paths = generate_valid_file_paths(child_base_path);

    let file_flags_and_child_paths =
        itertools::iproduct!(file_flags, file_child_paths.iter().map(String::as_str))
            .filter_map(filter_out_contradictory_open_parameters);

    assert_open_success(
        package_root,
        parent_path,
        file_flags_and_child_paths.clone(),
        verify_meta_as_file_opened,
    )
    .await;

    let failure_flags_and_child_paths = subtract(
        subtract(all_flag_and_child_paths, directory_flags_and_child_paths),
        file_flags_and_child_paths,
    )
    .into_iter();

    assert_open_flag_and_child_path_failure(
        package_root,
        parent_path,
        failure_flags_and_child_paths,
        verify_open_failed,
    )
    .await;
}

async fn assert_open_meta_subdirectory(
    source: &PackageSource,
    parent_path: &str,
    child_base_path: &str,
) {
    let package_root = &source.dir;

    let success_flags = [
        fio::PERM_READABLE,
        fio::Flags::PROTOCOL_DIRECTORY,
        fio::Flags::PROTOCOL_NODE,
        fio::Flags::FLAG_SEND_REPRESENTATION,
        fio::Flags::PERM_INHERIT_WRITE,
        fio::Flags::PERM_INHERIT_EXECUTE,
    ];

    let child_paths = generate_valid_directory_paths(child_base_path);

    let lax_child_paths = generate_lax_directory_paths(child_base_path);
    let all_flag_and_child_paths =
        itertools::iproduct!(ALL_FLAGS, lax_child_paths.iter().map(String::as_str));

    let success_flags_and_child_paths =
        itertools::iproduct!(success_flags, child_paths.iter().map(String::as_str))
            .filter_map(filter_out_contradictory_open_parameters);
    assert_open_success(
        package_root,
        parent_path,
        success_flags_and_child_paths.clone(),
        verify_directory_opened,
    )
    .await;

    assert_open_flag_and_child_path_failure(
        package_root,
        parent_path,
        subtract(all_flag_and_child_paths, success_flags_and_child_paths).into_iter(),
        verify_open_failed,
    )
    .await;
}

async fn assert_open_meta_file(source: &PackageSource, parent_path: &str, child_base_path: &str) {
    let package_root = &source.dir;

    let success_flags = [
        fio::PERM_READABLE,
        fio::Flags::PROTOCOL_NODE,
        fio::Flags::FLAG_SEND_REPRESENTATION,
        fio::Flags::PERM_INHERIT_WRITE,
        fio::Flags::PERM_INHERIT_EXECUTE,
        fio::Flags::PROTOCOL_FILE,
    ];

    let child_paths = generate_valid_file_paths(child_base_path);

    let lax_child_paths = generate_lax_directory_paths(child_base_path);
    let all_flag_and_child_paths =
        itertools::iproduct!(ALL_FLAGS, lax_child_paths.iter().map(String::as_str));

    let success_flags_and_child_paths =
        itertools::iproduct!(success_flags, child_paths.iter().map(String::as_str))
            .filter_map(filter_out_contradictory_open_parameters);
    assert_open_success(
        package_root,
        parent_path,
        success_flags_and_child_paths.clone(),
        verify_meta_as_file_opened,
    )
    .await;

    assert_open_flag_and_child_path_failure(
        package_root,
        parent_path,
        subtract(all_flag_and_child_paths, success_flags_and_child_paths).into_iter(),
        verify_open_failed,
    )
    .await;
}

async fn open_parent(package_root: &fio::DirectoryProxy, parent_path: &str) -> fio::DirectoryProxy {
    let parent_rights = if parent_path == "meta"
        || parent_path == "/meta"
        || parent_path.starts_with("meta/")
        || parent_path.starts_with("/meta/")
    {
        fuchsia_fs::PERM_READABLE
    } else {
        fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_EXECUTABLE
    };
    fuchsia_fs::directory::open_directory(package_root, parent_path, parent_rights)
        .await
        .expect("open parent directory")
}

fn open_node(parent: &fio::DirectoryProxy, flags: fio::Flags, path: &str) -> fio::NodeProxy {
    let (node, server_end) = create_proxy::<fio::NodeMarker>();
    parent.open(path, flags, &Default::default(), server_end.into_channel()).expect("open node");
    node
}

/// Generates the same path variations as [`generate_valid_directory_paths`]
/// plus extra invalid path variations using segments of "." and "..", leading "/", trailing "/",
/// and repeated "/".
fn generate_lax_directory_paths(base: &str) -> Vec<String> {
    let mut paths = generate_valid_directory_paths(base);
    if base == "." {
        paths.extend([format!("{base}/"), format!("/{base}"), format!("/{base}/")]);
    }
    // "path segment rules are checked"
    paths.extend([format!("./{base}"), format!("{base}/.")]);
    if base.contains('/') {
        paths.push(base.replace('/', "//"));
        paths.push(base.replace('/', "/to-be-removed/../"));
        paths.push(base.replace('/', "/./"));
    }
    paths
}

/// Generates a set of path variations which are valid when opening directories.
fn generate_valid_directory_paths(base: &str) -> Vec<String> {
    if base == "." {
        vec![base.to_string()]
    } else {
        vec![base.to_string(), format!("{base}/"), format!("/{base}"), format!("/{base}/")]
    }
}

/// Generates a set of path variations which are only valid when opening directories.
///
/// Paths ending in "/" can only be used when opening directories.
fn generate_valid_directory_only_paths(base: &str) -> Vec<String> {
    if base == "." {
        return vec![];
    }
    vec![format!("{base}/"), format!("/{base}/")]
}

/// Generates a set of path variations which are valid when opening files.
fn generate_valid_file_paths(base: &str) -> Vec<String> {
    vec![base.to_string(), format!("/{base}")]
}

async fn verify_directory_opened(node: fio::NodeProxy, flag: fio::Flags) -> Result<(), Error> {
    let protocol = String::from_utf8(node.query().await.context("failed to call query")?).unwrap();
    let expected_protocol = if flag.intersects(fio::Flags::PROTOCOL_NODE) {
        fio::NodeMarker::PROTOCOL_NAME
    } else {
        fio::DirectoryMarker::PROTOCOL_NAME
    };
    if protocol != expected_protocol {
        return Err(anyhow!("wrong protocol returned: {:?}", protocol));
    }

    if flag.intersects(fio::Flags::FLAG_SEND_REPRESENTATION) {
        let event = node.take_event_stream().next().await.ok_or_else(|| anyhow!("no events!"))?;
        let event = event.context("event error")?;
        match event {
            fio::NodeEvent::OnRepresentation { payload: representation } => match representation {
                fio::Representation::Node(_) => {
                    if !flag.contains(fio::Flags::PROTOCOL_NODE) {
                        return Err(anyhow!("wrong protocol returned: {:?}", representation));
                    }
                }
                fio::Representation::Directory(_) => {
                    if flag.contains(fio::Flags::PROTOCOL_NODE) {
                        return Err(anyhow!("wrong protocol returned: {:?}", representation));
                    }
                }
                _ => return Err(anyhow!("wrong protocol returned: {:?}", representation)),
            },
            event => return Err(anyhow!("unexpected event returned: {:?}", event)),
        }
    };
    Ok(())
}

async fn verify_content_file_opened(node: fio::NodeProxy, flag: fio::Flags) -> Result<(), Error> {
    // Calling Node.Query to determine the channel's protocol causes the OnRepresentation event to
    // be read from the channel and stored in the NodeProxy. When the channel is then moved from the
    // NodeProxy to the FileProxy, the OnRepresentation event gets dropped. The event is read here
    // so it doesn't get dropped.
    let on_representation = if flag.intersects(fio::Flags::FLAG_SEND_REPRESENTATION) {
        Some(
            node.take_event_stream()
                .next()
                .await
                .ok_or_else(|| anyhow!("no events!"))?
                .context("event error")?,
        )
    } else {
        None
    };

    let protocol = String::from_utf8(node.query().await.context("failed to call query")?).unwrap();

    // The entry should always be opened as a file unless we explicitly specified the node protocol.
    if flag.intersects(fio::Flags::PROTOCOL_NODE) {
        if protocol != fio::NodeMarker::PROTOCOL_NAME {
            return Err(anyhow!("wrong protocol returned: {:?}", protocol));
        }
        if let Some(event) = on_representation {
            let fio::NodeEvent::OnRepresentation { payload: representation } = event else {
                return Err(anyhow!("unexpected event returned: {:?}", event));
            };
            let fio::Representation::Node(_) = representation else {
                return Err(anyhow!("unexpected representation returned: {:?}", representation));
            };
        }
        return Ok(());
    }
    if protocol != fio::FileMarker::PROTOCOL_NAME {
        return Err(anyhow!("wrong protocol returned: {:?}", protocol));
    }
    {
        let file = fio::FileProxy::new(node.into_channel().unwrap());
        let fio::FileInfo { observer, .. } =
            file.describe().await.context("failed to call describe")?;
        // Only blobfs blobs set the observer to indicate when the blob is readable. The blobs
        // should be immediately readable here.
        if let Some(observer) = observer {
            let _: zx::Signals = observer
                .wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE_PAST)
                .to_result()
                .context("FILE_SIGNAL_READABLE not set")?;
        }
    }

    if let Some(event) = on_representation {
        match event {
            fio::NodeEvent::OnRepresentation { payload: representation } => {
                let fio::Representation::File(file_info) = representation else {
                    return Err(anyhow!("wrong representation returned: {:?}", representation));
                };
                // Only blobfs blobs set the event to indicate when the blob is readable.
                // The blobs should be immediately readable here.
                if let Some(event) = file_info.observer {
                    let _: zx::Signals = event
                        .wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE_PAST)
                        .to_result()
                        .context("FILE_SIGNAL_READABLE not set")?;
                }
            }
            event => return Err(anyhow!("unexpected event returned: {:?}", event)),
        }
    }
    Ok(())
}

async fn verify_meta_as_file_opened(node: fio::NodeProxy, flag: fio::Flags) -> Result<(), Error> {
    let protocol =
        String::from_utf8(node.query().await.context("failed to call describe")?).unwrap();
    let expected_protocol = if flag.intersects(fio::Flags::PROTOCOL_NODE) {
        fio::NodeMarker::PROTOCOL_NAME
    } else {
        fio::FileMarker::PROTOCOL_NAME
    };
    if protocol != expected_protocol {
        return Err(anyhow!("wrong protocol returned: {:?}", protocol));
    }

    if flag.intersects(fio::Flags::FLAG_SEND_REPRESENTATION) {
        let event = node.take_event_stream().next().await.ok_or_else(|| anyhow!("no events!"))?;
        let event = event.context("event error")?;
        match event {
            fio::NodeEvent::OnRepresentation { payload: representation } => {
                let fio::Representation::File(_) = representation else {
                    return Err(anyhow!("wrong representation returned: {:?}", representation));
                };
            }
            event => {
                return Err(anyhow!("unexpected event returned: {:?}", event));
            }
        }
    }
    Ok(())
}

async fn verify_open_failed(node: fio::NodeProxy) -> Result<(), Error> {
    match node.query().await {
        Ok(protocol) => Err(anyhow!("node should be closed: {:?}", protocol)),
        Err(fidl::Error::ClientChannelClosed { .. }) => Ok(()),
        Err(e) => Err(e).context("failed with unexpected error"),
    }
}

#[fuchsia::test]
async fn clone() {
    for source in dirs_to_test().await {
        clone_per_package_source(source).await
    }
}

async fn clone_per_package_source(source: PackageSource) {
    let root_dir = &source.dir;

    assert_clone_directory_overflow(
        root_dir,
        ".",
        vec![
            DirEntry { name: "dir".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "dir_overflow_readdirents".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "exceeds_max_buf".to_string(), kind: DirentKind::File },
            DirEntry { name: "file".to_string(), kind: DirentKind::File },
            DirEntry { name: "meta".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "file_0".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_1".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4095".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4096".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4097".to_string(), kind: DirentKind::File },
        ],
    )
    .await;

    assert_clone_directory_no_overflow(
        root_dir,
        "dir",
        vec![
            DirEntry { name: "dir".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "file".to_string(), kind: DirentKind::File },
        ],
    )
    .await;

    assert_clone_directory_overflow(
        root_dir,
        "meta",
        vec![
            DirEntry { name: "contents".to_string(), kind: DirentKind::File },
            DirEntry { name: "dir".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "dir_overflow_readdirents".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "exceeds_max_buf".to_string(), kind: DirentKind::File },
            DirEntry { name: "file".to_string(), kind: DirentKind::File },
            DirEntry { name: "package".to_string(), kind: DirentKind::File },
            DirEntry { name: "fuchsia.abi".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "file_0".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_1".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4095".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4096".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4097".to_string(), kind: DirentKind::File },
        ],
    )
    .await;

    assert_clone_directory_no_overflow(
        root_dir,
        "meta/dir",
        vec![
            DirEntry { name: "dir".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "file".to_string(), kind: DirentKind::File },
        ],
    )
    .await;
}

async fn assert_clone_directory_no_overflow(
    package_root: &fio::DirectoryProxy,
    path: &str,
    expected_dirents: Vec<DirEntry>,
) {
    let parent = open_parent(package_root, path).await;
    let (clone, server_end) = create_proxy::<fio::DirectoryMarker>();

    let node_request = fidl::endpoints::ServerEnd::new(server_end.into_channel());
    parent.clone(node_request).expect("cloned node");
    assert_read_dirents_no_overflow(&clone, expected_dirents).await;
}

async fn assert_clone_directory_overflow(
    package_root: &fio::DirectoryProxy,
    path: &str,
    expected_dirents: Vec<DirEntry>,
) {
    let parent = open_parent(package_root, path).await;
    let (clone, server_end) = create_proxy::<fio::DirectoryMarker>();

    let node_request = fidl::endpoints::ServerEnd::new(server_end.into_channel());
    parent.clone(node_request).expect("cloned node");

    assert_read_dirents_overflow(&clone, expected_dirents).await;
}

#[fuchsia::test]
async fn read_dirents() {
    for source in dirs_to_test().await {
        read_dirents_per_package_source(source).await
    }
}

async fn read_dirents_per_package_source(source: PackageSource) {
    let root_dir = source.dir;
    // Handle overflow cases (e.g. when size of total dirents exceeds MAX_BUF).
    assert_read_dirents_overflow(
        &root_dir,
        vec![
            DirEntry { name: "dir".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "dir_overflow_readdirents".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "exceeds_max_buf".to_string(), kind: DirentKind::File },
            DirEntry { name: "file".to_string(), kind: DirentKind::File },
            DirEntry { name: "meta".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "file_0".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_1".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4095".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4096".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4097".to_string(), kind: DirentKind::File },
        ],
    )
    .await;
    assert_read_dirents_overflow(
        &fuchsia_fs::directory::open_directory(&root_dir, "meta", fio::PERM_READABLE)
            .await
            .expect("open meta as dir"),
        vec![
            DirEntry { name: "contents".to_string(), kind: DirentKind::File },
            DirEntry { name: "dir".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "dir_overflow_readdirents".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "exceeds_max_buf".to_string(), kind: DirentKind::File },
            DirEntry { name: "file".to_string(), kind: DirentKind::File },
            DirEntry { name: "package".to_string(), kind: DirentKind::File },
            DirEntry { name: "fuchsia.abi".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "file_0".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_1".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4095".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4096".to_string(), kind: DirentKind::File },
            DirEntry { name: "file_4097".to_string(), kind: DirentKind::File },
        ],
    )
    .await;
    assert_read_dirents_overflow(
        &fuchsia_fs::directory::open_directory(
            &root_dir,
            "dir_overflow_readdirents",
            fio::PERM_READABLE,
        )
        .await
        .expect("open dir_overflow_readdirents"),
        vec![],
    )
    .await;
    assert_read_dirents_overflow(
        &fuchsia_fs::directory::open_directory(
            &root_dir,
            "meta/dir_overflow_readdirents",
            fio::PERM_READABLE,
        )
        .await
        .expect("open meta/dir_overflow_readdirents"),
        vec![],
    )
    .await;

    // Handle no-overflow cases (e.g. when size of total dirents does not exceed MAX_BUF).
    assert_read_dirents_no_overflow(
        &fuchsia_fs::directory::open_directory(&root_dir, "dir", fio::PERM_READABLE)
            .await
            .expect("open dir"),
        vec![
            DirEntry { name: "dir".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "file".to_string(), kind: DirentKind::File },
        ],
    )
    .await;
    assert_read_dirents_no_overflow(
        &fuchsia_fs::directory::open_directory(&root_dir, "meta/dir", fio::PERM_READABLE)
            .await
            .expect("open meta/dir"),
        vec![
            DirEntry { name: "dir".to_string(), kind: DirentKind::Directory },
            DirEntry { name: "file".to_string(), kind: DirentKind::File },
        ],
    )
    .await;
}

/// For a particular directory, verify that the overflow case is being hit on ReadDirents (e.g. it
/// should take two ReadDirents calls to read all of the directory entries).
/// Note: we considered making this a unit test for pkg-harness, but opted to include this in the
/// integration tests so all the test cases are in one place.
async fn assert_read_dirents_overflow(
    dir: &fio::DirectoryProxy,
    additional_contents: Vec<DirEntry>,
) {
    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert!(!buf.is_empty(), "first call should yield non-empty buffer");

    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert!(!buf.is_empty(), "second call should yield non-empty buffer");

    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert_eq!(buf, &[] as &[u8]);

    assert_eq!(
        fuchsia_fs::directory::readdir(dir).await.unwrap().into_iter().sorted().collect::<Vec<_>>(),
        ('a'..='z')
            .chain('A'..='E')
            .map(|seed| DirEntry {
                name: repeat_by_n(seed, fio::MAX_NAME_LENGTH.try_into().unwrap()),
                kind: DirentKind::File
            })
            .chain(additional_contents)
            .sorted()
            .collect::<Vec<_>>()
    );
}

/// For a particular directory, verify that the overflow case is NOT being hit on ReadDirents
/// (e.g. it should only take one ReadDirents call to read all of the directory entries).
async fn assert_read_dirents_no_overflow(
    dir: &fio::DirectoryProxy,
    expected_dirents: Vec<DirEntry>,
) {
    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert!(!buf.is_empty(), "first call should yield non-empty buffer");

    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert_eq!(buf, &[] as &[u8]);

    assert_eq!(
        fuchsia_fs::directory::readdir(dir).await.unwrap().into_iter().sorted().collect::<Vec<_>>(),
        expected_dirents.into_iter().sorted().collect::<Vec<_>>()
    );
}

#[fuchsia::test]
async fn rewind() {
    for source in dirs_to_test().await {
        rewind_per_package_source(source).await
    }
}

async fn rewind_per_package_source(source: PackageSource) {
    let root_dir = source.dir;
    // Handle overflow cases.
    for path in [".", "meta", "dir_overflow_readdirents", "meta/dir_overflow_readdirents"] {
        let dir = fuchsia_fs::directory::open_directory(&root_dir, path, fio::PERM_READABLE)
            .await
            .unwrap();
        assert_rewind_overflow_when_seek_offset_at_end(&dir).await;
        assert_rewind_overflow_when_seek_offset_in_middle(&dir).await;
    }

    // Handle non-overflow cases.
    for path in ["dir", "meta/dir"] {
        assert_rewind_no_overflow(
            &fuchsia_fs::directory::open_directory(&root_dir, path, fio::PERM_READABLE)
                .await
                .unwrap(),
        )
        .await;
    }
}

async fn assert_rewind_overflow_when_seek_offset_at_end(dir: &fio::DirectoryProxy) {
    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert!(!buf.is_empty(), "first read_dirents call should yield non-empty buffer");

    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert!(!buf.is_empty(), "second read_dirents call should yield non-empty buffer");

    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert_eq!(buf, &[] as &[u8]);

    let status = dir.rewind().await.unwrap();
    zx::Status::ok(status).expect("status ok");

    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert!(!buf.is_empty(), "read_dirents call after rewind should yield non-empty buffer");
}

async fn assert_rewind_overflow_when_seek_offset_in_middle(dir: &fio::DirectoryProxy) {
    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert!(!buf.is_empty(), "first read_dirents call should yield non-empty buffer");

    let status = dir.rewind().await.unwrap();
    zx::Status::ok(status).expect("status ok");

    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert!(!buf.is_empty(), "first read_dirents call after rewind should yield non-empty buffer");

    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert!(!buf.is_empty(), "second read_dirents call after rewind should yield non-empty buffer");

    let (status, buf) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert_eq!(buf, &[] as &[u8]);
}

async fn assert_rewind_no_overflow(dir: &fio::DirectoryProxy) {
    let (status, buf0) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert!(!buf0.is_empty(), "first read_dirents call should yield non-empty buffer");

    let status = dir.rewind().await.unwrap();
    zx::Status::ok(status).expect("status ok");

    let (status, buf1) = dir.read_dirents(fio::MAX_BUF).await.unwrap();
    zx::Status::ok(status).expect("status ok");
    assert!(!buf1.is_empty(), "first read_dirents call after rewind should yield non-empty buffer");

    // We can't guarantee ordering will be the same, so the next best thing is to verify the
    // returned buffers are the same length.
    assert_eq!(buf0.len(), buf1.len());
}

#[fuchsia::test]
async fn get_token() {
    for source in dirs_to_test().await {
        get_token_per_package_source(source).await
    }
}

async fn get_token_per_package_source(source: PackageSource) {
    let root_dir = &source.dir;
    for path in [".", "dir", "meta", "meta/dir"] {
        let dir = fuchsia_fs::directory::open_directory(root_dir, path, fio::PERM_READABLE)
            .await
            .unwrap();

        let (status, token) = dir.get_token().await.unwrap();
        let status = zx::Status::ok(status);
        assert_eq!(status, Err(zx::Status::NOT_SUPPORTED));
        assert!(token.is_none(), "token should be absent");
    }
}
