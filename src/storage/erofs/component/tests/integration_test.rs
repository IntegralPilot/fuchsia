// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::{DiscoverableProtocolMarker as _, Proxy as _};
use fidl_fuchsia_erofs::{ErofsMarker, ErofsProxy, ErofsServeRequest};
use fidl_fuchsia_io as fio;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};
use fuchsia_fs::directory::{
    DirEntry, DirentKind, WatchEvent, Watcher, readdir, readdir_inclusive,
};
use futures::StreamExt as _;
use std::fs;
use std::io::Read as _;
use test_case::test_case;

const LONG_SELINUX_CONTEXT: &[u8] = b"u:object_r:very_long_selinux_context_exceeding_the_inline_limit_of_two_hundred_and_fifty_six_bytes_and_requiring_the_use_of_extended_attributes_instead_of_returning_the_context_inline_in_the_node_attributes_table_representation_as_dictated_by_the_fuchsia_io_node_fidl_specification:s0";

async fn setup_realm() -> (ErofsProxy, fuchsia_component_test::RealmInstance) {
    let builder = RealmBuilder::new().await.expect("Failed to create RealmBuilder");

    let erofs = builder
        .add_child("erofs", "#meta/erofs.cm", ChildOptions::new())
        .await
        .expect("Failed to add erofs child");

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(ErofsMarker::PROTOCOL_NAME))
                .from(&erofs)
                .to(Ref::parent()),
        )
        .await
        .expect("Failed to add route");

    let realm = builder.build().await.expect("Failed to build realm");

    let erofs_server: ErofsProxy =
        realm.root.connect_to_protocol_at_exposed_dir().expect("Failed to connect to Erofs");

    (erofs_server, realm)
}

async fn setup_erofs_from_image(
    filename: &str,
) -> (fio::DirectoryProxy, fuchsia_component_test::RealmInstance) {
    let (erofs_server, realm) = setup_realm().await;

    let erofs_image =
        fs::read(format!("/pkg/data/{filename}")).expect("Failed to read erofs image");
    let vmo = zx::Vmo::create(erofs_image.len() as u64).expect("Failed to create VMO");
    vmo.write(&erofs_image, 0).expect("Failed to write VMO");

    let (root_client, root_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();

    let payload =
        ErofsServeRequest { backing_vmo: Some(vmo), root: Some(root_server), ..Default::default() };

    let () = erofs_server
        .serve(payload)
        .await
        .expect("Failed to call Serve")
        .expect("Serve returned an error");

    (root_client, realm)
}

async fn setup_erofs() -> (fio::DirectoryProxy, fuchsia_component_test::RealmInstance) {
    setup_erofs_from_image("simple.erofs").await
}

#[test_case("simple.erofs" ; "uncompressed")]
#[test_case("simple_lz4.erofs" ; "lz4 compressed")]
#[test_case("simple_lz4_legacy.erofs" ; "lz4 legacy compressed")]
#[fuchsia::test]
async fn test_erofs_directory_traversal(filename: &str) {
    let (root_client, _realm) = setup_erofs_from_image(filename).await;

    let entries = readdir_inclusive(&root_client).await.expect("Failed to readdir root");

    let expected_entries = [
        DirEntry { name: ".".to_string(), kind: DirentKind::Directory },
        DirEntry { name: "file1".to_string(), kind: DirentKind::File },
        DirEntry { name: "large_dir".to_string(), kind: DirentKind::Directory },
        DirEntry { name: "mixed_compression".to_string(), kind: DirentKind::File },
        DirEntry { name: "photosynthesis".to_string(), kind: DirentKind::File },
        DirEntry { name: "quantum".to_string(), kind: DirentKind::File },
        DirEntry { name: "symlink_to_file1".to_string(), kind: DirentKind::Symlink },
    ];
    assert_eq!(entries, expected_entries);

    let large_dir =
        fuchsia_fs::directory::open_directory(&root_client, "large_dir", fio::PERM_READABLE)
            .await
            .expect("Failed to open large_dir");

    let large_entries = readdir(&large_dir).await.expect("Failed to readdir large_dir");
    assert!(large_entries.len() > 0);
    for entry in &large_entries {
        if entry.name == ".." {
            assert_eq!(entry.kind, DirentKind::Directory);
            continue;
        }
        assert!(entry.name.starts_with("file_number_"));
        assert_eq!(entry.kind, DirentKind::File);
    }

    // Assert lookup non-existent file returns NOT_FOUND
    match fuchsia_fs::directory::open_file(&root_client, "non_existent", fio::PERM_READABLE).await {
        Err(fuchsia_fs::node::OpenError::OpenError(zx::Status::NOT_FOUND)) => (),
        res => panic!("Expected OpenError(NOT_FOUND), got {:?}", res),
    }
}

#[test_case("simple.erofs" ; "uncompressed")]
#[test_case("simple_lz4.erofs" ; "lz4 compressed")]
#[test_case("simple_lz4_legacy.erofs" ; "lz4 legacy compressed")]
#[fuchsia::test]
async fn test_erofs_file_get_backing_memory(filename: &str) {
    let (root_client, _realm) = setup_erofs_from_image(filename).await;

    let file = fuchsia_fs::directory::open_file(&root_client, "file1", fio::PERM_READABLE)
        .await
        .expect("Failed to open file1");

    let expected = fs::read("/pkg/data/simple/file1").expect("Failed to read file1 source");

    let (_, immut_attrs) = file
        .get_attributes(fio::NodeAttributesQuery::all())
        .await
        .expect("Failed to get attributes")
        .map_err(zx::Status::err_from_raw)
        .expect("get_attributes returned error");

    assert_eq!(immut_attrs.content_size, Some(expected.len() as u64));
    assert_eq!(
        immut_attrs.abilities,
        Some(fio::Operations::GET_ATTRIBUTES | fio::Operations::READ_BYTES)
    );
    assert!(immut_attrs.id.is_some());
    assert!(immut_attrs.id.unwrap() > 0);

    let paged_vmo = file
        .get_backing_memory(fio::VmoFlags::READ)
        .await
        .expect("get_backing_memory FIDL call failed")
        .map_err(zx::Status::err_from_raw)
        .expect("get_backing_memory returned error");

    let info = paged_vmo.info().expect("Failed to query VMO info");
    assert_eq!(info.committed_bytes, 0);

    let mut buf = vec![0u8; expected.len()];
    paged_vmo.read(&mut buf, 0).expect("Failed to read paged VMO");
    assert_eq!(buf, expected);
}

#[test_case("simple.erofs", "file1")]
#[test_case("simple_lz4.erofs", "file1")]
#[test_case("simple_lz4_legacy.erofs", "file1")]
#[test_case("simple.erofs", "photosynthesis")]
#[test_case("simple_lz4.erofs", "photosynthesis")]
#[test_case("simple_lz4_legacy.erofs", "photosynthesis")]
#[test_case("simple.erofs", "quantum")]
#[test_case("simple_lz4.erofs", "quantum")]
#[test_case("simple_lz4_legacy.erofs", "quantum")]
#[test_case("simple.erofs", "mixed_compression")]
#[test_case("simple_lz4.erofs", "mixed_compression")]
#[test_case("simple_lz4_legacy.erofs", "mixed_compression")]
#[fuchsia::test]
async fn test_erofs_file_read(image_name: &str, filename: &str) {
    let (root_client, _realm) = setup_erofs_from_image(image_name).await;

    let file = fuchsia_fs::directory::open_file(&root_client, filename, fio::PERM_READABLE)
        .await
        .expect("Failed to open file");

    let file_channel = file.into_channel().unwrap().into_zx_channel();
    let fd = fdio::create_fd(file_channel.into()).expect("Failed to create FD from VMO connection");
    let mut std_file: std::fs::File = fd.into();

    let mut content = Vec::new();
    std_file.read_to_end(&mut content).expect("Failed to read std_file using std::io::Read");

    let expected =
        fs::read(format!("/pkg/data/simple/{}", filename)).expect("Failed to read source file");
    assert_eq!(content, expected);
}

#[fuchsia::test]
async fn test_erofs_xattrs() {
    let (root_client, _realm) = setup_erofs().await;

    // Open "file1" which has xattrs
    let file = fuchsia_fs::directory::open_file(&root_client, "file1", fio::PERM_READABLE)
        .await
        .expect("Failed to open file1");

    // Get the file protocol node
    let node_channel = file.into_channel().unwrap();
    let file_proxy = fio::FileProxy::from_channel(node_channel);

    // List extended attributes
    let (iterator_client, iterator_server) =
        fidl::endpoints::create_proxy::<fio::ExtendedAttributeIteratorMarker>();
    file_proxy
        .list_extended_attributes(iterator_server)
        .expect("Failed to call list_extended_attributes");

    // Read all attributes from the iterator
    let mut attributes = Vec::new();
    loop {
        let (chunk, last) = iterator_client
            .get_next()
            .await
            .expect("Failed to call get_next")
            .map_err(zx::Status::err_from_raw)
            .expect("get_next returned error");
        attributes.extend(chunk);
        if last {
            break;
        }
    }

    // Sort and assert
    attributes.sort();
    let expected_attributes = vec![
        b"security.selinux".to_vec(),
        b"user.flavor".to_vec(),
        b"user.security".to_vec(),
        b"user.shared".to_vec(),
    ];
    assert_eq!(attributes, expected_attributes);

    // Get specific attributes
    let selinux_val = file_proxy
        .get_extended_attribute(b"security.selinux")
        .await
        .expect("Failed to call get_extended_attribute")
        .map_err(zx::Status::err_from_raw)
        .expect("get_extended_attribute returned error");
    let selinux_val_bytes = match selinux_val {
        fio::ExtendedAttributeValue::Bytes(b) => b,
        _ => panic!("Expected bytes"),
    };
    assert_eq!(selinux_val_bytes, b"u:object_r:file1_t:s0");

    let flavor_val = file_proxy
        .get_extended_attribute(b"user.flavor")
        .await
        .expect("Failed to call get_extended_attribute")
        .map_err(zx::Status::err_from_raw)
        .expect("get_extended_attribute returned error");
    let flavor_val_bytes = match flavor_val {
        fio::ExtendedAttributeValue::Bytes(b) => b,
        _ => panic!("Expected bytes"),
    };
    assert_eq!(flavor_val_bytes, b"vanilla");

    let security_val = file_proxy
        .get_extended_attribute(b"user.security")
        .await
        .expect("Failed to call get_extended_attribute")
        .map_err(zx::Status::err_from_raw)
        .expect("get_extended_attribute returned error");
    let security_val_bytes = match security_val {
        fio::ExtendedAttributeValue::Bytes(b) => b,
        _ => panic!("Expected bytes"),
    };
    assert_eq!(security_val_bytes, b"high");

    let shared_val = file_proxy
        .get_extended_attribute(b"user.shared")
        .await
        .expect("Failed to call get_extended_attribute")
        .map_err(zx::Status::err_from_raw)
        .expect("get_extended_attribute returned error");
    let shared_val_bytes = match shared_val {
        fio::ExtendedAttributeValue::Bytes(b) => b,
        _ => panic!("Expected bytes"),
    };
    assert_eq!(shared_val_bytes, b"same_value");

    // Get non-existent attribute should return NOT_FOUND
    let err = file_proxy
        .get_extended_attribute(b"user.non_existent")
        .await
        .expect("Failed to call get_extended_attribute");
    assert_eq!(err.unwrap_err(), zx::Status::NOT_FOUND.into_raw());
}

#[fuchsia::test]
async fn test_erofs_file_paging_after_close() {
    let (root_client, _realm) = setup_erofs().await;

    // Open "photosynthesis", which spans two pages (4128 bytes).
    let file = fuchsia_fs::directory::open_file(&root_client, "photosynthesis", fio::PERM_READABLE)
        .await
        .expect("Failed to open photosynthesis");

    // Request backing VMO memory
    let paged_vmo = file
        .get_backing_memory(fio::VmoFlags::READ)
        .await
        .expect("get_backing_memory FIDL call failed")
        .map_err(zx::Status::err_from_raw)
        .expect("get_backing_memory returned error");

    // Verify no pages are committed initially.
    let info = paged_vmo.info().expect("Failed to query VMO info");
    assert_eq!(info.committed_bytes, 0);

    // Close the connection to the file. This should drop the file proxy on our end, and on the
    // server, the VFS connection to the ErofsFile is dropped. If lifecycle tracking works
    // properly, the ErofsFile stays alive because of the active VMO child reference, and it will
    // continue to page in data.
    drop(file);

    // Read page 2 from the VMO (offset 4100). This forces a page-in.
    let mut buf = [0u8; 10];
    paged_vmo.read(&mut buf, 4100).expect("Failed to read VMO after closing file connection");
    assert_ne!(buf, [0u8; 10]); // The read should succeed and return actual data.
}

#[fuchsia::test]
async fn test_erofs_directory_watcher() {
    let (root_client, _realm) = setup_erofs().await;

    let mut watcher = Watcher::new(&root_client).await.expect("Failed to create watcher");

    let mut existing_files = std::collections::HashSet::new();

    while let Some(msg) = watcher.next().await {
        let msg = msg.expect("Watcher error");
        match msg.event {
            WatchEvent::EXISTING => {
                existing_files.insert(msg.filename);
            }
            WatchEvent::IDLE => {
                break;
            }
            event => panic!("Unexpected watch event: {:?}", event),
        }
    }

    let expected_files: std::collections::HashSet<_> = [
        ".",
        "file1",
        "large_dir",
        "mixed_compression",
        "photosynthesis",
        "quantum",
        "symlink_to_file1",
    ]
    .iter()
    .map(std::path::PathBuf::from)
    .collect();

    assert_eq!(existing_files, expected_files);
}

#[fuchsia::test]
async fn test_erofs_file_readahead() {
    let (root_client, _realm) = setup_erofs().await;

    // Open "photosynthesis", which spans two pages (4128 bytes).
    let file = fuchsia_fs::directory::open_file(&root_client, "photosynthesis", fio::PERM_READABLE)
        .await
        .expect("Failed to open photosynthesis");

    // Request backing VMO memory
    let paged_vmo = file
        .get_backing_memory(fio::VmoFlags::READ)
        .await
        .expect("get_backing_memory FIDL call failed")
        .map_err(zx::Status::err_from_raw)
        .expect("get_backing_memory returned error");

    // Verify no pages are committed initially.
    let info = paged_vmo.info().expect("Failed to query VMO info");
    assert_eq!(info.committed_bytes, 0);

    // Read 1 byte from the beginning of the VMO (offset 0).
    // This should trigger page_in for 0..4096, which will readahead to 8192.
    let mut buf1 = [0u8; 1];
    paged_vmo.read(&mut buf1, 0).expect("Failed to read VMO at 0");

    // Read 1 byte from the second page (offset 4100).
    // If readahead worked, this should NOT trigger another page_in.
    let mut buf2 = [0u8; 1];
    paged_vmo.read(&mut buf2, 4100).expect("Failed to read VMO at 4100");

    // Verify the data is correct.
    let expected = fs::read("/pkg/data/simple/photosynthesis").expect("Failed to read source");
    assert_eq!(buf1[0], expected[0]);
    assert_eq!(buf2[0], expected[4100]);
}

#[fuchsia::test]
async fn test_erofs_file_attributes() {
    let (root_client, _realm) = setup_erofs().await;

    let file = fuchsia_fs::directory::open_file(&root_client, "file1", fio::PERM_READABLE)
        .await
        .expect("Failed to open file1");

    let (mut_attrs, immut_attrs) = file
        .get_attributes(fio::NodeAttributesQuery::all())
        .await
        .expect("Failed to get attributes")
        .map_err(zx::Status::err_from_raw)
        .expect("get_attributes returned error");

    assert_eq!(immut_attrs.protocols, Some(fio::NodeProtocolKinds::FILE));
    assert_eq!(
        immut_attrs.abilities,
        Some(fio::Operations::GET_ATTRIBUTES | fio::Operations::READ_BYTES)
    );
    assert!(immut_attrs.id.is_some());
    assert_eq!(immut_attrs.link_count, Some(1));

    assert!(mut_attrs.mode.is_some());
    let mode = mut_attrs.mode.unwrap();
    assert_eq!(mode & 0o170000, 0o100000); // Regular file

    assert!(mut_attrs.uid.is_some());
    assert!(mut_attrs.gid.is_some());
    assert!(mut_attrs.modification_time.is_some());
    assert!(mut_attrs.modification_time.unwrap() > 0);
    assert_eq!(
        mut_attrs.selinux_context,
        Some(fio::SelinuxContext::Data(b"u:object_r:file1_t:s0".to_vec()))
    );

    // Verify a file without selinux context returns None
    let quantum_file =
        fuchsia_fs::directory::open_file(&root_client, "quantum", fio::PERM_READABLE)
            .await
            .expect("Failed to open quantum");
    let (quantum_mut_attrs, _) = quantum_file
        .get_attributes(fio::NodeAttributesQuery::all())
        .await
        .expect("Failed to get attributes")
        .map_err(zx::Status::err_from_raw)
        .expect("get_attributes returned error");
    assert_eq!(quantum_mut_attrs.selinux_context, None);

    // Verify a file with a large selinux context returns UseExtendedAttributes
    let large_dir =
        fuchsia_fs::directory::open_directory(&root_client, "large_dir", fio::PERM_READABLE)
            .await
            .expect("Failed to open large_dir");
    let large_file =
        fuchsia_fs::directory::open_file(&large_dir, "file_number_1", fio::PERM_READABLE)
            .await
            .expect("Failed to open large_dir/file_number_1");
    let (large_file_mut_attrs, _) = large_file
        .get_attributes(fio::NodeAttributesQuery::all())
        .await
        .expect("Failed to get attributes")
        .map_err(zx::Status::err_from_raw)
        .expect("get_attributes returned error");
    assert_eq!(
        large_file_mut_attrs.selinux_context,
        Some(fio::SelinuxContext::UseExtendedAttributes(fio::EmptyStruct {}))
    );

    let large_file_proxy = fio::FileProxy::from_channel(large_file.into_channel().unwrap());
    let selinux_val = large_file_proxy
        .get_extended_attribute(b"security.selinux")
        .await
        .expect("Failed to call get_extended_attribute")
        .map_err(zx::Status::err_from_raw)
        .expect("get_extended_attribute returned error");
    let selinux_val_bytes = match selinux_val {
        fio::ExtendedAttributeValue::Bytes(b) => b,
        _ => panic!("Expected bytes"),
    };
    assert_eq!(selinux_val_bytes, LONG_SELINUX_CONTEXT);
}

#[fuchsia::test]
async fn test_erofs_directory_attributes() {
    let (root_client, _realm) = setup_erofs().await;

    let (mut_attrs, immut_attrs) = root_client
        .get_attributes(fio::NodeAttributesQuery::all())
        .await
        .expect("Failed to get attributes")
        .map_err(zx::Status::err_from_raw)
        .expect("get_attributes returned error");

    assert_eq!(immut_attrs.protocols, Some(fio::NodeProtocolKinds::DIRECTORY));
    assert_eq!(
        immut_attrs.abilities,
        Some(
            fio::Operations::GET_ATTRIBUTES
                | fio::Operations::ENUMERATE
                | fio::Operations::TRAVERSE,
        )
    );
    assert!(immut_attrs.id.is_some());
    // root link count should be at least 2 (. and ..) + subdirs (large_dir)
    assert!(immut_attrs.link_count.unwrap() >= 3);

    assert!(mut_attrs.mode.is_some());
    let mode = mut_attrs.mode.unwrap();
    assert_eq!(mode & 0o170000, 0o040000); // Directory

    assert!(mut_attrs.uid.is_some());
    assert!(mut_attrs.gid.is_some());
    assert!(mut_attrs.modification_time.is_some());
    assert!(mut_attrs.modification_time.unwrap() > 0);
    assert_eq!(mut_attrs.selinux_context, None);

    // Verify a directory with a large selinux context returns UseExtendedAttributes
    let large_dir =
        fuchsia_fs::directory::open_directory(&root_client, "large_dir", fio::PERM_READABLE)
            .await
            .expect("Failed to open large_dir");
    let (large_dir_mut_attrs, _) = large_dir
        .get_attributes(fio::NodeAttributesQuery::all())
        .await
        .expect("Failed to get attributes")
        .map_err(zx::Status::err_from_raw)
        .expect("get_attributes returned error");
    assert_eq!(
        large_dir_mut_attrs.selinux_context,
        Some(fio::SelinuxContext::UseExtendedAttributes(fio::EmptyStruct {}))
    );

    let large_dir_proxy = fio::DirectoryProxy::from_channel(large_dir.into_channel().unwrap());
    let selinux_val = large_dir_proxy
        .get_extended_attribute(b"security.selinux")
        .await
        .expect("Failed to call get_extended_attribute")
        .map_err(zx::Status::err_from_raw)
        .expect("get_extended_attribute returned error");
    let selinux_val_bytes = match selinux_val {
        fio::ExtendedAttributeValue::Bytes(b) => b,
        _ => panic!("Expected bytes"),
    };
    assert_eq!(selinux_val_bytes, LONG_SELINUX_CONTEXT);
}

#[fuchsia::test]
async fn test_erofs_query_filesystem() {
    let (root_client, _realm) = setup_erofs().await;

    let (status, info) =
        root_client.query_filesystem().await.expect("query_filesystem FIDL call failed");

    assert_eq!(zx::Status::ok(status), Ok(()));
    assert!(info.is_some());
    let info = info.unwrap();

    assert!(info.total_bytes > 0);
    assert_eq!(info.used_bytes, info.total_bytes);
    assert!(info.total_nodes > 0);
    assert_eq!(info.used_nodes, info.total_nodes);
    assert_eq!(info.block_size, 4096);
    assert_eq!(info.max_filename_size, 255);
    assert_eq!(info.fs_type, 0x65726f66); // EROFS magic or VfsType

    let name_bytes: Vec<u8> = info.name.iter().map(|&b| b as u8).take_while(|&b| b != 0).collect();
    assert_eq!(name_bytes, b"erofs");
}

#[fuchsia::test]
async fn test_erofs_symlink() {
    let (root_client, _realm) = setup_erofs().await;

    let (symlink_proxy, server_end) = fidl::endpoints::create_proxy::<fio::SymlinkMarker>();
    root_client
        .open(
            "symlink_to_file1",
            fio::Flags::PROTOCOL_SYMLINK | fio::PERM_READABLE,
            &fio::Options::default(),
            server_end.into_channel().into(),
        )
        .expect("open symlink failed");

    let target_bytes = symlink_proxy.describe().await.expect("describe failed").target.unwrap();

    assert_eq!(target_bytes, b"file1");

    let (mut_attrs, immut_attrs) = symlink_proxy
        .get_attributes(fio::NodeAttributesQuery::all())
        .await
        .expect("Failed to get attributes")
        .map_err(zx::Status::err_from_raw)
        .expect("get_attributes returned error");

    assert_eq!(immut_attrs.content_size, Some(5));
    assert_eq!(immut_attrs.abilities, Some(fio::Operations::GET_ATTRIBUTES));
    assert!(immut_attrs.id.is_some());
    assert!(immut_attrs.id.unwrap() > 0);
    assert_eq!(
        mut_attrs.selinux_context,
        Some(fio::SelinuxContext::Data(b"u:object_r:symlink_t:s0".to_vec()))
    );
}
