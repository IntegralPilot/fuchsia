// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use io_conformance_util::test_harness::TestHarness;
use io_conformance_util::*;

#[fuchsia::test]
async fn get_attributes_query_none() {
    let harness = TestHarness::new().await;
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let file_proxy =
        dir.open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_READABLE, None).await.unwrap();

    // fuchsia.io/Node.GetAttributes
    // Node attributes that were not requested should return None
    let attributes = file_proxy
        .get_attributes(fio::NodeAttributesQuery::empty())
        .await
        .unwrap()
        .expect("get_attributes failed");
    assert_eq!(attributes, Default::default());
}

#[fuchsia::test]
async fn get_attributes_file_query_all() {
    let harness = TestHarness::new().await;
    let supported_attrs = harness.config.supported_attributes;
    const FILE_CONTENTS: &'static [u8] = b"test-file-contents";

    let entries = vec![file(TEST_FILE, FILE_CONTENTS.to_owned())];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let file_proxy =
        dir.open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_READABLE, None).await.unwrap();

    // Set some mutable attributes if we can, so the mutable filesystems that support these match
    // the immutable ones that also support them (EROFS in particular).
    if harness.supports_mutable_attrs() {
        let file_writable = dir
            .open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_READABLE | fio::PERM_WRITABLE, None)
            .await
            .unwrap();
        let initial_attrs = fio::MutableNodeAttributes {
            mode: supported_attrs.contains(fio::NodeAttributesQuery::MODE).then_some(0o100644),
            uid: supported_attrs.contains(fio::NodeAttributesQuery::UID).then_some(100),
            gid: supported_attrs.contains(fio::NodeAttributesQuery::GID).then_some(200),
            rdev: supported_attrs.contains(fio::NodeAttributesQuery::RDEV).then_some(300),
            ..Default::default()
        };
        file_writable
            .update_attributes(&initial_attrs)
            .await
            .unwrap()
            .expect("update_attributes failed");
    }

    // fuchsia.io/Node.GetAttributes
    // All of the attributes are requested. Filesystems are allowed to return None for attributes
    // they don't support.
    let (mutable_attrs, immutable_attrs) = file_proxy
        .get_attributes(
            fio::NodeAttributesQuery::all() - fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE,
        )
        .await
        .unwrap()
        .expect("get_attributes failed");

    // If ctime and mtime are supported then they shouldn't be 0.
    if supported_attrs.contains(fio::NodeAttributesQuery::CREATION_TIME) {
        assert_matches!(mutable_attrs.creation_time, Some(1..));
    } else {
        assert_matches!(mutable_attrs.creation_time, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::MODIFICATION_TIME) {
        assert_matches!(mutable_attrs.modification_time, Some(1..));
    } else {
        assert_matches!(mutable_attrs.modification_time, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::ACCESS_TIME) {
        assert_matches!(mutable_attrs.access_time, Some(1..));
    } else {
        assert_matches!(mutable_attrs.access_time, None);
    }

    // Check exact values for supported POSIX attributes.
    if supported_attrs.contains(fio::NodeAttributesQuery::MODE) {
        assert_matches!(mutable_attrs.mode, Some(0o100644));
    } else {
        assert_matches!(mutable_attrs.mode, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::UID) {
        assert_matches!(mutable_attrs.uid, Some(100));
    } else {
        assert_matches!(mutable_attrs.uid, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::GID) {
        assert_matches!(mutable_attrs.gid, Some(200));
    } else {
        assert_matches!(mutable_attrs.gid, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::RDEV) {
        assert_matches!(mutable_attrs.rdev, Some(300));
    } else {
        assert_matches!(mutable_attrs.rdev, None);
    }

    // All node types must report at least protocols and abilities.
    assert_matches!(immutable_attrs.protocols, Some(fio::NodeProtocolKinds::FILE));
    assert!(immutable_attrs.abilities.is_some());

    // Other attributes have conditional support.
    if supported_attrs.contains(fio::NodeAttributesQuery::CONTENT_SIZE) {
        assert_matches!(immutable_attrs.content_size, Some(x) if x == FILE_CONTENTS.len() as u64);
    } else {
        assert_matches!(immutable_attrs.content_size, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::STORAGE_SIZE) {
        assert_matches!(immutable_attrs.storage_size, Some(..));
    } else {
        assert_matches!(immutable_attrs.storage_size, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::LINK_COUNT) {
        assert_matches!(immutable_attrs.link_count, Some(..));
    } else {
        assert_matches!(immutable_attrs.link_count, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::ID) {
        assert_matches!(immutable_attrs.id, Some(..));
    } else {
        assert_matches!(immutable_attrs.id, None);
    }
}

#[fuchsia::test]
async fn get_attributes_directory_query_all() {
    let harness = TestHarness::new().await;
    let supported_attrs = harness.config.supported_attributes;

    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let dir_proxy =
        dir.open_node::<fio::DirectoryMarker>("dir", fio::PERM_READABLE, None).await.unwrap();

    // Set some mutable attributes if we can, so the mutable filesystems that support these match
    // the immutable ones that also support them (EROFS in particular).
    if harness.supports_mutable_attrs() {
        let dir_writable = dir
            .open_node::<fio::DirectoryMarker>("dir", fio::PERM_READABLE | fio::PERM_WRITABLE, None)
            .await
            .unwrap();
        let initial_attrs = fio::MutableNodeAttributes {
            mode: supported_attrs.contains(fio::NodeAttributesQuery::MODE).then_some(0o40755),
            uid: supported_attrs.contains(fio::NodeAttributesQuery::UID).then_some(100),
            gid: supported_attrs.contains(fio::NodeAttributesQuery::GID).then_some(200),
            rdev: supported_attrs.contains(fio::NodeAttributesQuery::RDEV).then_some(300),
            ..Default::default()
        };
        dir_writable
            .update_attributes(&initial_attrs)
            .await
            .unwrap()
            .expect("update_attributes failed");
    }

    // fuchsia.io/Node.GetAttributes
    // All of the attributes are requested. Filesystems are allowed to return None for attributes
    // they don't support.
    let (mutable_attrs, immutable_attrs) = dir_proxy
        .get_attributes(
            fio::NodeAttributesQuery::all() - fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE,
        )
        .await
        .unwrap()
        .expect("get_attributes failed");

    // If timestamps are supported then they shouldn't be 0.
    if supported_attrs.contains(fio::NodeAttributesQuery::CREATION_TIME) {
        assert_matches!(mutable_attrs.creation_time, Some(1..));
    } else {
        assert_matches!(mutable_attrs.creation_time, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::MODIFICATION_TIME) {
        assert_matches!(mutable_attrs.modification_time, Some(1..));
    } else {
        assert_matches!(mutable_attrs.modification_time, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::ACCESS_TIME) {
        assert_matches!(mutable_attrs.access_time, Some(1..));
    } else {
        assert_matches!(mutable_attrs.access_time, None);
    }

    // Check exact values for supported POSIX attributes.
    if supported_attrs.contains(fio::NodeAttributesQuery::MODE) {
        assert_matches!(mutable_attrs.mode, Some(0o40755));
    } else {
        assert_matches!(mutable_attrs.mode, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::UID) {
        assert_matches!(mutable_attrs.uid, Some(100));
    } else {
        assert_matches!(mutable_attrs.uid, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::GID) {
        assert_matches!(mutable_attrs.gid, Some(200));
    } else {
        assert_matches!(mutable_attrs.gid, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::RDEV) {
        assert_matches!(mutable_attrs.rdev, Some(300));
    } else {
        assert_matches!(mutable_attrs.rdev, None);
    }

    // All node types must report at least protocols and abilities.
    assert_matches!(immutable_attrs.protocols, Some(fio::NodeProtocolKinds::DIRECTORY));
    assert!(immutable_attrs.abilities.is_some());

    // Other attributes have conditional support.
    if supported_attrs.contains(fio::NodeAttributesQuery::LINK_COUNT) {
        assert_matches!(immutable_attrs.link_count, Some(..));
    } else {
        assert_matches!(immutable_attrs.link_count, None);
    }
    if supported_attrs.contains(fio::NodeAttributesQuery::ID) {
        assert_matches!(immutable_attrs.id, Some(..));
    } else {
        assert_matches!(immutable_attrs.id, None);
    }
}

#[fuchsia::test]
async fn update_attributes_file_unsupported() {
    let harness = TestHarness::new().await;
    // Skip this test if the harness does support updating attributes.
    if harness.supports_mutable_attrs() || !harness.config.supports_mutable_file {
        return;
    }
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let file_proxy =
        dir.open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_WRITABLE, None).await.unwrap();
    // fuchsia.io/Node.UpdateAttributes
    assert_eq!(
        file_proxy.update_attributes(&fio::MutableNodeAttributes::default()).await.unwrap(),
        Err(zx::Status::NOT_SUPPORTED.into_raw())
    );
}

#[fuchsia::test]
async fn update_attributes_file_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.supports_mutable_attrs() {
        return;
    }

    let entries = vec![file(TEST_FILE, TEST_FILE_CONTENTS.to_vec())];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let file_proxy =
        dir.open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_READABLE, None).await.unwrap();

    let status = file_proxy
        .update_attributes(&fio::MutableNodeAttributes {
            modification_time: Some(111),
            ..Default::default()
        })
        .await
        .expect("FIDL call failed")
        .map_err(zx::Status::err_from_raw);
    assert_eq!(status, Err(zx::Status::BAD_HANDLE));
}

#[fuchsia::test]
async fn update_attributes_file_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.supports_mutable_attrs() {
        return;
    }
    // Don't want to test for `fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE` in this test.
    let supported_attrs =
        harness.config.supported_attributes - fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE;

    let entries = vec![file(TEST_FILE, TEST_FILE_CONTENTS.to_vec())];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let file_proxy = dir
        .open_node::<fio::FileMarker>(TEST_FILE, fio::PERM_READABLE | fio::PERM_WRITABLE, None)
        .await
        .unwrap();

    let new_attrs = fio::MutableNodeAttributes {
        creation_time: supported_attrs
            .contains(fio::NodeAttributesQuery::CREATION_TIME)
            .then_some(111),
        modification_time: supported_attrs
            .contains(fio::NodeAttributesQuery::MODIFICATION_TIME)
            .then_some(222),
        mode: supported_attrs.contains(fio::NodeAttributesQuery::MODE).then_some(333),
        uid: supported_attrs.contains(fio::NodeAttributesQuery::UID).then_some(444),
        gid: supported_attrs.contains(fio::NodeAttributesQuery::GID).then_some(555),
        rdev: supported_attrs.contains(fio::NodeAttributesQuery::RDEV).then_some(666),
        access_time: supported_attrs.contains(fio::NodeAttributesQuery::ACCESS_TIME).then_some(777),
        selinux_context: supported_attrs
            .contains(fio::NodeAttributesQuery::SELINUX_CONTEXT)
            .then_some(fio::SelinuxContext::Data(vec![7u8; 10])),
        ..Default::default()
    };

    let _ = file_proxy
        .update_attributes(&new_attrs)
        .await
        .expect("FIDL call failed")
        .map_err(zx::Status::err_from_raw)
        .expect("update_attributes failed");

    let (mutable_attrs, _) = file_proxy
        .get_attributes(supported_attrs)
        .await
        .expect("FIDL call failed")
        .map_err(zx::Status::err_from_raw)
        .expect("get_attributes failed");
    assert_eq!(mutable_attrs, new_attrs);

    // Test that we should not be able to update non-supported attributes
    let unsupported_new_attrs = fio::MutableNodeAttributes {
        creation_time: (!supported_attrs.contains(fio::NodeAttributesQuery::CREATION_TIME))
            .then_some(111),
        modification_time: (!supported_attrs.contains(fio::NodeAttributesQuery::MODIFICATION_TIME))
            .then_some(222),
        mode: (!supported_attrs.contains(fio::NodeAttributesQuery::MODE)).then_some(333),
        uid: (!supported_attrs.contains(fio::NodeAttributesQuery::UID)).then_some(444),
        gid: (!supported_attrs.contains(fio::NodeAttributesQuery::GID)).then_some(555),
        rdev: (!supported_attrs.contains(fio::NodeAttributesQuery::RDEV)).then_some(666),
        access_time: (!supported_attrs.contains(fio::NodeAttributesQuery::ACCESS_TIME))
            .then_some(777),
        ..Default::default()
    };
    if unsupported_new_attrs != fio::MutableNodeAttributes::default() {
        let status = file_proxy
            .update_attributes(&unsupported_new_attrs)
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw)
            .expect_err("update unsupported attributes passed");
        assert_eq!(status, zx::Status::NOT_SUPPORTED);
    }
}

#[fuchsia::test]
async fn get_attributes_file_node_reference() {
    let harness = TestHarness::new().await;
    let entries = vec![file(TEST_FILE, TEST_FILE_CONTENTS.to_vec())];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let file_proxy = dir
        .open_node::<fio::NodeMarker>(
            TEST_FILE,
            fio::Flags::PROTOCOL_NODE | fio::Flags::PERM_GET_ATTRIBUTES,
            None,
        )
        .await
        .unwrap();

    // fuchsia.io/Node.GetAttributes
    let (_mutable_attributes, immutable_attributes) = file_proxy
        .get_attributes(fio::NodeAttributesQuery::PROTOCOLS)
        .await
        .unwrap()
        .expect("get_attributes failed");
    assert_eq!(immutable_attributes.protocols.unwrap(), fio::NodeProtocolKinds::FILE);
}

#[fuchsia::test]
async fn update_attributes_file_node_reference_not_allowed() {
    let harness = TestHarness::new().await;
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let file_proxy = dir
        .open_node::<fio::NodeMarker>(
            TEST_FILE,
            fio::Flags::PROTOCOL_NODE | fio::Flags::PERM_GET_ATTRIBUTES,
            None,
        )
        .await
        .unwrap();

    // Node references does not support fuchsia.io/Node.UpdateAttributes
    assert_eq!(
        file_proxy.update_attributes(&fio::MutableNodeAttributes::default()).await.unwrap(),
        Err(zx::Status::BAD_HANDLE.into_raw())
    );
}

#[fuchsia::test]
async fn get_attributes_directory() {
    let harness = TestHarness::new().await;
    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let dir_proxy =
        dir.open_node::<fio::DirectoryMarker>("dir", fio::PERM_READABLE, None).await.unwrap();

    let (_mutable_attributes, immutable_attributes) = dir_proxy
        .get_attributes(fio::NodeAttributesQuery::PROTOCOLS)
        .await
        .unwrap()
        .expect("get_attributes failed");
    assert_eq!(immutable_attributes.protocols.unwrap(), fio::NodeProtocolKinds::DIRECTORY);
}

#[fuchsia::test]
async fn update_attributes_directory_unsupported() {
    let harness = TestHarness::new().await;
    if harness.supports_mutable_attrs() {
        // Skip test if harness supports updating attributes.
        return;
    }

    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let dir_proxy =
        dir.open_node::<fio::DirectoryMarker>("dir", fio::PERM_WRITABLE, None).await.unwrap();

    // fuchsia.io/Node.UpdateAttributes
    assert_eq!(
        dir_proxy.update_attributes(&fio::MutableNodeAttributes::default()).await.unwrap(),
        Err(zx::Status::NOT_SUPPORTED.into_raw())
    );
}

#[fuchsia::test]
async fn update_attributes_directory_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.supports_mutable_attrs() {
        return;
    }

    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let dir_proxy =
        dir.open_node::<fio::DirectoryMarker>("dir", fio::PERM_READABLE, None).await.unwrap();

    let status = dir_proxy
        .update_attributes(&fio::MutableNodeAttributes {
            modification_time: Some(111),
            ..Default::default()
        })
        .await
        .expect("FIDL call failed")
        .map_err(zx::Status::err_from_raw);
    assert_eq!(status, Err(zx::Status::BAD_HANDLE));
}

#[fuchsia::test]
async fn update_attributes_directory_with_sufficient_rights() {
    let harness = TestHarness::new().await;
    if !harness.supports_mutable_attrs() {
        return;
    }
    let supported_attrs = harness.config.supported_attributes;

    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let dir_proxy = dir
        .open_node::<fio::DirectoryMarker>("dir", fio::PERM_READABLE | fio::PERM_WRITABLE, None)
        .await
        .unwrap();

    let new_attrs = fio::MutableNodeAttributes {
        creation_time: supported_attrs
            .contains(fio::NodeAttributesQuery::CREATION_TIME)
            .then_some(111),
        modification_time: supported_attrs
            .contains(fio::NodeAttributesQuery::MODIFICATION_TIME)
            .then_some(222),
        mode: supported_attrs.contains(fio::NodeAttributesQuery::MODE).then_some(333),
        uid: supported_attrs.contains(fio::NodeAttributesQuery::UID).then_some(444),
        gid: supported_attrs.contains(fio::NodeAttributesQuery::GID).then_some(555),
        rdev: supported_attrs.contains(fio::NodeAttributesQuery::RDEV).then_some(666),
        access_time: supported_attrs.contains(fio::NodeAttributesQuery::ACCESS_TIME).then_some(777),
        casefold: supported_attrs.contains(fio::NodeAttributesQuery::CASEFOLD).then_some(false),
        selinux_context: supported_attrs
            .contains(fio::NodeAttributesQuery::SELINUX_CONTEXT)
            .then_some(fio::SelinuxContext::Data(vec![7u8; 10])),
        ..Default::default()
    };

    let _ = dir_proxy
        .update_attributes(&new_attrs)
        .await
        .expect("FIDL call failed")
        .map_err(zx::Status::err_from_raw)
        .expect("update_attributes failed");

    let (mutable_attrs, _) = dir_proxy
        .get_attributes(
            fio::NodeAttributesQuery::all() - fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE,
        )
        .await
        .expect("FIDL call failed")
        .map_err(zx::Status::err_from_raw)
        .expect("get_attributes failed");
    assert_eq!(mutable_attrs, new_attrs);

    // Test that we should not be able to update non-supported attributes
    let unsupported_new_attrs = fio::MutableNodeAttributes {
        creation_time: (!supported_attrs.contains(fio::NodeAttributesQuery::CREATION_TIME))
            .then_some(111),
        modification_time: (!supported_attrs.contains(fio::NodeAttributesQuery::MODIFICATION_TIME))
            .then_some(222),
        mode: (!supported_attrs.contains(fio::NodeAttributesQuery::MODE)).then_some(333),
        uid: (!supported_attrs.contains(fio::NodeAttributesQuery::UID)).then_some(444),
        gid: (!supported_attrs.contains(fio::NodeAttributesQuery::GID)).then_some(555),
        rdev: (!supported_attrs.contains(fio::NodeAttributesQuery::RDEV)).then_some(666),
        access_time: (!supported_attrs.contains(fio::NodeAttributesQuery::ACCESS_TIME))
            .then_some(777),
        ..Default::default()
    };
    if unsupported_new_attrs != fio::MutableNodeAttributes::default() {
        let status = dir_proxy
            .update_attributes(&unsupported_new_attrs)
            .await
            .expect("FIDL call failed")
            .map_err(zx::Status::err_from_raw)
            .expect_err("update unsupported attributes passed");
        assert_eq!(status, zx::Status::NOT_SUPPORTED);
    }
}

#[fuchsia::test]
async fn get_attributes_directory_node_reference() {
    let harness = TestHarness::new().await;
    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let dir_proxy = dir
        .open_node::<fio::NodeMarker>(
            "dir",
            fio::Flags::PROTOCOL_NODE | fio::Flags::PERM_GET_ATTRIBUTES,
            None,
        )
        .await
        .unwrap();

    // fuchsia.io/Node.GetAttributes
    let (_mutable_attributes, immutable_attributes) = dir_proxy
        .get_attributes(fio::NodeAttributesQuery::PROTOCOLS)
        .await
        .unwrap()
        .expect("get_attributes failed");
    assert_eq!(immutable_attributes.protocols.unwrap(), fio::NodeProtocolKinds::DIRECTORY);
}

#[fuchsia::test]
async fn update_attributes_directory_node_reference_not_allowed() {
    let harness = TestHarness::new().await;
    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());
    let dir_proxy = dir
        .open_node::<fio::NodeMarker>(
            "dir",
            fio::Flags::PROTOCOL_NODE | fio::Flags::PERM_GET_ATTRIBUTES,
            None,
        )
        .await
        .unwrap();

    // Node reference doesn't allow for updating attributes
    assert_eq!(
        dir_proxy.update_attributes(&fio::MutableNodeAttributes::default()).await.unwrap(),
        Err(zx::Status::BAD_HANDLE.into_raw())
    );
}

#[fuchsia::test]
async fn get_attributes_file_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    let entries = vec![file(TEST_FILE, vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    // Test opening file connection as node reference
    {
        let file_proxy = dir
            .open_node::<fio::NodeMarker>(TEST_FILE, fio::Flags::PROTOCOL_NODE, None)
            .await
            .unwrap();

        assert_eq!(
            file_proxy
                .get_attributes(fio::NodeAttributesQuery::empty())
                .await
                .expect("FIDL call failed")
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::ACCESS_DENIED)
        );
    }

    // Test file connection
    {
        let file_proxy = dir
            .open_node::<fio::FileMarker>(TEST_FILE, fio::Flags::PROTOCOL_FILE, None)
            .await
            .unwrap();

        assert_eq!(
            file_proxy
                .get_attributes(fio::NodeAttributesQuery::empty())
                .await
                .expect("FIDL call failed")
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::ACCESS_DENIED)
        );
    }
}

#[fuchsia::test]
async fn get_attributes_directory_with_insufficient_rights() {
    let harness = TestHarness::new().await;
    let entries = vec![directory("dir", vec![])];
    let dir = harness.get_directory(entries, harness.dir_rights.all_flags());

    // Test opening directory connection as node reference
    {
        let dir_proxy =
            dir.open_node::<fio::NodeMarker>("dir", fio::Flags::PROTOCOL_NODE, None).await.unwrap();

        assert_eq!(
            dir_proxy
                .get_attributes(fio::NodeAttributesQuery::empty())
                .await
                .expect("FIDL call failed")
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::ACCESS_DENIED)
        );
    }

    // Test directory connection
    {
        let dir_proxy = dir
            .open_node::<fio::DirectoryMarker>("dir", fio::Flags::PROTOCOL_DIRECTORY, None)
            .await
            .unwrap();

        assert_eq!(
            dir_proxy
                .get_attributes(fio::NodeAttributesQuery::empty())
                .await
                .expect("FIDL call failed")
                .map_err(zx::Status::err_from_raw),
            Err(zx::Status::ACCESS_DENIED)
        );
    }
}

#[fuchsia::test]
async fn open_symlink_without_get_attributes_fails() {
    let harness = TestHarness::new().await;
    let dir = harness.get_directory(vec![], harness.dir_rights.all_flags());

    // Create a symlink.
    let (symlink_client, symlink_server) = fidl::endpoints::create_proxy::<fio::SymlinkMarker>();
    let create_result = dir
        .create_symlink("symlink", b"target", Some(symlink_server))
        .await
        .expect("FIDL call failed")
        .map_err(zx::Status::err_from_raw);

    if let Err(status) = create_result {
        if status == zx::Status::NOT_SUPPORTED {
            // Symlinks not supported by this filesystem.
            return;
        }
        panic!("create_symlink failed: {:?}", status);
    }

    // Close the client we got from creation (it has all rights).
    drop(symlink_client);

    // Attempt to open the symlink without PERM_GET_ATTRIBUTES.
    let status = dir
        .open_node::<fio::SymlinkMarker>("symlink", fio::Flags::PROTOCOL_SYMLINK, None)
        .await
        .unwrap_err();
    assert_eq!(status, zx::Status::INVALID_ARGS);
}
