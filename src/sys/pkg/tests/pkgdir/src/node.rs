// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{PackageSource, dirs_to_test};
use anyhow::{Context as _, Error, anyhow};
use fidl::endpoints::{DiscoverableProtocolMarker as _, Proxy as _};
use fidl_fuchsia_io as fio;

#[fuchsia::test]
async fn get_attributes() {
    for source in dirs_to_test().await {
        get_attributes_per_package_source(source).await
    }
}

trait U64Verifier: std::fmt::Debug {
    #[track_caller]
    fn verify(&self, num: u64);
}

impl U64Verifier for u64 {
    fn verify(&self, num: u64) {
        assert_eq!(num, *self)
    }
}

#[derive(Debug)]
struct AnyU64;
impl U64Verifier for AnyU64 {
    fn verify(&self, _num: u64) {}
}

async fn get_attributes_per_package_source(source: PackageSource) {
    let root_dir = &source.dir;
    #[derive(Debug)]
    struct Args {
        open_flags: fio::Flags,
        expected_protocols: fio::NodeProtocolKinds,
        expected_abilities: fio::Abilities,
        id_verifier: Option<Box<dyn U64Verifier>>,
        expected_content_size: Option<u64>,
        storage_size_verifier: Option<Box<dyn U64Verifier>>,
    }

    impl Default for Args {
        fn default() -> Self {
            Self {
                open_flags: Default::default(),
                expected_protocols: Default::default(),
                expected_abilities: Default::default(),
                id_verifier: Some(Box::new(1)),
                expected_content_size: None,
                storage_size_verifier: None,
            }
        }
    }

    async fn verify_get_attributes(root_dir: &fio::DirectoryProxy, path: &str, args: Args) {
        let node = fuchsia_fs::directory::open_node(root_dir, path, args.open_flags).await.unwrap();
        let (_, immut_attrs) = node
            .get_attributes(fio::NodeAttributesQuery::all())
            .await
            .unwrap()
            .expect("fuchsia.io/Node.GetAttributes failed");
        assert_eq!(immut_attrs.protocols.unwrap(), args.expected_protocols);
        assert_eq!(immut_attrs.abilities.unwrap(), args.expected_abilities);
        if let Some(verifier) = args.id_verifier {
            verifier.verify(immut_attrs.id.unwrap());
        }
        assert_eq!(immut_attrs.content_size, args.expected_content_size);
        if let Some(verifier) = args.storage_size_verifier {
            verifier.verify(immut_attrs.storage_size.unwrap());
        } else {
            assert!(immut_attrs.storage_size.is_none());
        }
    }

    verify_get_attributes(
        root_dir,
        ".",
        Args {
            open_flags: fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_EXECUTABLE,
            expected_protocols: fio::NodeProtocolKinds::DIRECTORY,
            expected_abilities: fio::Abilities::GET_ATTRIBUTES
                | fio::Abilities::ENUMERATE
                | fio::Abilities::TRAVERSE,
            ..Default::default()
        },
    )
    .await;
    verify_get_attributes(
        root_dir,
        "dir",
        Args {
            open_flags: fio::Flags::PERM_GET_ATTRIBUTES,
            expected_protocols: fio::NodeProtocolKinds::DIRECTORY,
            expected_abilities: fio::Abilities::GET_ATTRIBUTES
                | fio::Abilities::ENUMERATE
                | fio::Abilities::TRAVERSE,
            ..Default::default()
        },
    )
    .await;
    verify_get_attributes(
        root_dir,
        "file",
        Args {
            open_flags: fuchsia_fs::PERM_READABLE,
            expected_protocols: fio::NodeProtocolKinds::FILE,
            expected_abilities: fio::Abilities::GET_ATTRIBUTES
                | fio::Abilities::READ_BYTES
                | fio::Abilities::EXECUTE,
            id_verifier: None,
            expected_content_size: Some(4),
            storage_size_verifier: Some(Box::new(AnyU64)),
        },
    )
    .await;
    verify_get_attributes(
        root_dir,
        "meta",
        Args {
            open_flags: fio::Flags::PROTOCOL_FILE | fio::Flags::PERM_GET_ATTRIBUTES,
            expected_protocols: fio::NodeProtocolKinds::FILE,
            expected_abilities: fio::Abilities::GET_ATTRIBUTES | fio::Abilities::READ_BYTES,
            expected_content_size: Some(64),
            storage_size_verifier: Some(Box::new(AnyU64)),
            ..Default::default()
        },
    )
    .await;
    verify_get_attributes(
        root_dir,
        "meta",
        Args {
            open_flags: fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::PERM_GET_ATTRIBUTES,
            expected_protocols: fio::NodeProtocolKinds::DIRECTORY,
            expected_abilities: fio::Abilities::GET_ATTRIBUTES
                | fio::Abilities::ENUMERATE
                | fio::Abilities::TRAVERSE,
            expected_content_size: Some(75),
            storage_size_verifier: Some(Box::new(AnyU64)),
            ..Default::default()
        },
    )
    .await;
    verify_get_attributes(
        root_dir,
        "meta/dir",
        Args {
            open_flags: fio::Flags::PERM_GET_ATTRIBUTES,
            expected_protocols: fio::NodeProtocolKinds::DIRECTORY,
            expected_abilities: fio::Abilities::GET_ATTRIBUTES
                | fio::Abilities::ENUMERATE
                | fio::Abilities::TRAVERSE,
            expected_content_size: Some(75),
            storage_size_verifier: Some(Box::new(AnyU64)),
            ..Default::default()
        },
    )
    .await;
    verify_get_attributes(
        root_dir,
        "meta/file",
        Args {
            open_flags: fio::Flags::PERM_GET_ATTRIBUTES,
            expected_protocols: fio::NodeProtocolKinds::FILE,
            expected_abilities: fio::Abilities::GET_ATTRIBUTES | fio::Abilities::READ_BYTES,
            expected_content_size: Some(9),
            storage_size_verifier: Some(Box::new(AnyU64)),
            ..Default::default()
        },
    )
    .await;
}

#[fuchsia::test]
async fn close() {
    for source in dirs_to_test().await {
        close_per_package_source(source).await
    }
}

async fn close_per_package_source(source: PackageSource) {
    let root_dir = source.dir;
    async fn verify_close(root_dir: &fio::DirectoryProxy, path: &str, flags: fio::Flags) {
        let node =
            fuchsia_fs::directory::open_node(root_dir, path, flags | fuchsia_fs::PERM_READABLE)
                .await
                .unwrap();

        let () = node.close().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        assert_matches::assert_matches!(
            node.close().await,
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. })
        );
    }

    verify_close(&root_dir, ".", fio::Flags::PROTOCOL_DIRECTORY).await;
    verify_close(&root_dir, "dir", fio::Flags::PROTOCOL_DIRECTORY).await;
    verify_close(&root_dir, "meta", fio::Flags::PROTOCOL_DIRECTORY).await;
    verify_close(&root_dir, "meta/dir", fio::Flags::PROTOCOL_DIRECTORY).await;

    verify_close(&root_dir, "file", fio::Flags::PROTOCOL_FILE).await;
    verify_close(&root_dir, "meta/file", fio::Flags::PROTOCOL_FILE).await;
    verify_close(&root_dir, "meta", fio::Flags::PROTOCOL_FILE).await;
}

#[fuchsia::test]
async fn describe() {
    for source in dirs_to_test().await {
        describe_per_package_source(source).await
    }
}

async fn describe_per_package_source(source: PackageSource) {
    let root_dir = source.dir;
    assert_query_directory(&root_dir, ".").await;

    assert_query_directory(&root_dir, "meta").await;
    assert_describe_meta_file(&root_dir, "meta").await;

    assert_query_directory(&root_dir, "meta/dir").await;
    assert_query_directory(&root_dir, "dir").await;

    assert_describe_file(&root_dir, "file").await;
    assert_describe_meta_file(&root_dir, "meta/file").await;
}

async fn assert_query_directory(package_root: &fio::DirectoryProxy, path: &str) {
    for flag in [fio::Flags::empty(), fio::Flags::PROTOCOL_NODE] {
        let node = fuchsia_fs::directory::open_node(
            package_root,
            path,
            flag | fio::Flags::PROTOCOL_DIRECTORY,
        )
        .await
        .unwrap();

        if let Err(e) = verify_query_directory(node, flag).await {
            panic!(
                "failed to verify query. path: {path:?}, flag: {flag:?}, \
                    error: {e:#}",
            );
        }
    }
}

async fn verify_query_directory(node: fio::NodeProxy, flag: fio::Flags) -> Result<(), Error> {
    let protocol = String::from_utf8(node.query().await.context("failed to call query")?).unwrap();
    let expected_protocol = if flag.intersects(fio::Flags::PROTOCOL_NODE) {
        fio::NodeMarker::PROTOCOL_NAME
    } else {
        fio::DirectoryMarker::PROTOCOL_NAME
    };
    if protocol != expected_protocol {
        return Err(anyhow!("wrong protocol returned: {:?}", protocol));
    }
    Ok(())
}

async fn assert_describe_file(package_root: &fio::DirectoryProxy, path: &str) {
    for flag in [fuchsia_fs::PERM_READABLE, fio::Flags::PROTOCOL_NODE] {
        let node = fuchsia_fs::directory::open_node(package_root, path, flag).await.unwrap();
        if let Err(e) = verify_describe_file(node, flag).await {
            panic!(
                "failed to verify describe. path: {path:?}, flag: {flag:?}, \
                    error: {e:#}",
            );
        }
    }
}

async fn verify_describe_file(node: fio::NodeProxy, flag: fio::Flags) -> Result<(), Error> {
    let protocol = String::from_utf8(node.query().await.context("failed to call query")?).unwrap();
    let expected_protocol = if flag.intersects(fio::Flags::PROTOCOL_NODE) {
        fio::NodeMarker::PROTOCOL_NAME
    } else {
        fio::FileMarker::PROTOCOL_NAME
    };
    if protocol != expected_protocol {
        return Err(anyhow!("wrong protocol returned: {:?}", protocol));
    }
    if protocol == fio::FileMarker::PROTOCOL_NAME {
        let fio::FileInfo { observer, .. } = fio::FileProxy::new(node.into_channel().unwrap())
            .describe()
            .await
            .context("failed to call describe")?;
        // Only blobfs blobs set the observer to indicate when the blob is readable. The blobs
        // should be immediately readable here.
        if let Some(observer) = observer {
            let _: zx::Signals = observer
                .wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE_PAST)
                .to_result()
                .context("FILE_SIGNAL_READABLE not set")?;
        }
        // TODO(https://fxbug.dev/327633753): Check for stream support.
    }
    Ok(())
}

async fn assert_describe_meta_file(package_root: &fio::DirectoryProxy, path: &str) {
    for flag in [fio::Flags::empty(), fio::Flags::PROTOCOL_NODE] {
        let node = fuchsia_fs::directory::open_node(package_root, path, flag).await.unwrap();
        if let Err(e) = verify_describe_file(node, flag).await {
            panic!(
                "failed to verify describe. path: {path:?}, flag: {flag:?}, \
                    error: {e:#}"
            );
        }
    }
}

#[fuchsia::test]
async fn sync() {
    for source in dirs_to_test().await {
        sync_per_package_source(source).await
    }
}

async fn sync_per_package_source(source: PackageSource) {
    let root_dir = source.dir;
    assert_sync(&root_dir, ".", fio::Flags::PROTOCOL_DIRECTORY).await;
    assert_sync(&root_dir, "meta", fio::Flags::PROTOCOL_DIRECTORY).await;
    assert_sync(&root_dir, "meta/dir", fio::Flags::PROTOCOL_DIRECTORY).await;
    assert_sync(&root_dir, "dir", fio::Flags::PROTOCOL_DIRECTORY).await;
    assert_sync(&root_dir, "meta", fio::Flags::PROTOCOL_FILE).await;
    assert_sync(&root_dir, "meta/file", fio::Flags::PROTOCOL_FILE).await;
}

async fn assert_sync(package_root: &fio::DirectoryProxy, path: &str, flags: fio::Flags) {
    let node = fuchsia_fs::directory::open_node(package_root, path, flags).await.unwrap();

    if let Err(e) = verify_sync(node).await {
        panic!("sync failed. path: {path:?}, error: {e:#}");
    }
}

async fn verify_sync(node: fio::NodeProxy) -> Result<(), Error> {
    let result = node.sync().await.context("failed to call sync")?;
    let result = result.map_err(zx::Status::err_from_raw);
    // All of the files and directories are immutable so it's valid to return either success or
    // NOT_SUPPORTED.
    match result {
        Ok(()) => Ok(()),
        Err(zx::Status::NOT_SUPPORTED) => Ok(()),
        Err(_) => Err(anyhow!("wrong status returned: {:?}", result)),
    }
}
