// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

use assert_matches::assert_matches;
use fidl::endpoints::Proxy as _;
use fidl_fuchsia_io as fio;
use fsverity_merkle::{FsVerityHasher, FsVerityHasherOptions, MerkleTreeBuilder, Sha256Hash};
use fuchsia_async as fasync;
use fxfs_testing::TestFixture;
use std::mem::MaybeUninit;
use std::sync::Arc;
use syncio::{
    SeekOrigin, SelinuxContextAttr, XattrSetMode, ZXIO_ROOT_HASH_LENGTH, Zxio, ZxioOpenOptions,
    zxio, zxio_fsverity_descriptor_t, zxio_node_attr_has_t, zxio_node_attributes_t,
};
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::execution_scope::ExecutionScope;
use vfs::file::{FidlIoConnection, File, FileIo, FileLike, FileOptions, SyncMode};
use vfs::node::Node;
use vfs::path::Path;
use vfs::remote::RemoteLike;
use vfs::symlink::Symlink;
use vfs::{ObjectRequestRef, pseudo_directory};
use zx::{self as zx, Status};

#[fuchsia::test]
async fn test_symlink() {
    let fixture = TestFixture::new().await;

    let (dir_client, dir_server) = zx::Channel::create();
    fixture.root().clone(dir_server.into()).expect("clone failed");

    fasync::unblock(|| {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");

        let symlink_zxio =
            dir_zxio.create_symlink("symlink", b"target").expect("create_symlink failed");
        assert_eq!(symlink_zxio.read_link().expect("read_link failed"), b"target");

        // Test some error cases
        assert_eq!(
            dir_zxio.create_symlink("symlink", b"target").expect_err("create symlink succeeded"),
            zx::Status::ALREADY_EXISTS
        );
        assert_eq!(
            dir_zxio.create_symlink("a/b", b"target").expect_err("create symlink succeeded"),
            zx::Status::INVALID_ARGS
        );
        assert_eq!(
            dir_zxio
                .create_symlink("symlink2", &vec![65; 300])
                .expect_err("create symlink succeeded"),
            zx::Status::BAD_PATH
        );

        let symlink_zxio =
            dir_zxio.open("symlink", fio::PERM_READABLE, Default::default()).expect("open failed");
        assert_eq!(symlink_zxio.read_link().expect("read_link failed"), b"target");
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_fsverity_enabled() {
    let fixture = TestFixture::new().await;
    let root = fixture.root();
    let file = fuchsia_fs::directory::open_file(
        &root,
        "foo",
        fio::Flags::FLAG_MAYBE_CREATE
            | fio::PERM_READABLE
            | fio::PERM_WRITABLE
            | fio::Flags::PROTOCOL_FILE,
    )
    .await
    .unwrap();
    let data = vec![0xFF; 8192];
    file.write(&data).await.expect("FIDL call failed").expect("write failed");

    fuchsia_fs::file::close(file).await.unwrap();

    let (dir_client, dir_server) = zx::Channel::create();
    root.clone(dir_server.into()).expect("clone failed");

    fasync::unblock(move || {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");
        let mut attrs = zxio_node_attributes_t {
            has: zxio_node_attr_has_t { fsverity_enabled: true, ..Default::default() },
            ..Default::default()
        };
        let foo_zxio = Arc::new(
            dir_zxio
                .open(
                    "foo",
                    fio::Flags::PROTOCOL_FILE
                        | fio::Flags::PERM_UPDATE_ATTRIBUTES
                        | fio::Flags::PERM_GET_ATTRIBUTES,
                    ZxioOpenOptions::new(Some(&mut attrs), None),
                )
                .expect("open failed"),
        );
        assert!(!attrs.fsverity_enabled);
        let mut builder = MerkleTreeBuilder::<Sha256Hash>::new(FsVerityHasher::Sha256(
            FsVerityHasherOptions::new(vec![0xFF; 8], 4096),
        ));
        builder.write(data.as_slice());
        let tree = builder.finish();
        let mut expected_root: [u8; 64] = [0u8; 64];
        expected_root[0..32].copy_from_slice(tree.root());

        // NOTE: The root hash will be calculated and set by the filesystem.
        let expected_descriptor =
            zxio_fsverity_descriptor_t { hash_algorithm: 1, salt_size: 8, salt: [0xFF; 32] };
        let () = foo_zxio
            .enable_verity(&expected_descriptor)
            .expect("failed to set verified file metadata");
        let query = zxio_node_attr_has_t {
            content_size: true,
            fsverity_options: true,
            fsverity_root_hash: true,
            fsverity_enabled: true,
            ..Default::default()
        };

        let mut fsverity_root_hash = [0; ZXIO_ROOT_HASH_LENGTH];
        let attrs = foo_zxio
            .attr_get_with_root_hash(query, &mut fsverity_root_hash)
            .expect("attr_get failed");
        assert!(attrs.fsverity_enabled);
        assert_eq!(attrs.fsverity_options.hash_alg, 1);
        assert_eq!(attrs.fsverity_options.salt_size, 8);
        assert_eq!(attrs.content_size, data.len() as u64);
        assert_eq!(fsverity_root_hash, expected_root);
        let mut buf = [0; 32];
        buf[0..8].copy_from_slice(&[0xFF; 8]);
        assert_eq!(attrs.fsverity_options.salt, buf);
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_not_fsverity_enabled() {
    let fixture = TestFixture::new().await;
    let root = fixture.root();
    let file = fuchsia_fs::directory::open_file(
        &root,
        "foo",
        fio::Flags::FLAG_MAYBE_CREATE
            | fio::PERM_READABLE
            | fio::PERM_WRITABLE
            | fio::Flags::PROTOCOL_FILE,
    )
    .await
    .unwrap();
    let data = vec![0xFF; 8192];
    file.write(&data).await.expect("FIDL call failed").expect("write failed");

    fuchsia_fs::file::close(file).await.unwrap();

    let (dir_client, dir_server) = zx::Channel::create();
    root.clone(dir_server.into()).expect("clone failed");

    fasync::unblock(move || {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");
        let mut attrs = zxio_node_attributes_t {
            has: zxio_node_attr_has_t { fsverity_enabled: true, ..Default::default() },
            ..Default::default()
        };
        let foo_zxio = Arc::new(
            dir_zxio
                .open(
                    "foo",
                    fio::Flags::PROTOCOL_FILE
                        | fio::Flags::PERM_UPDATE_ATTRIBUTES
                        | fio::Flags::PERM_GET_ATTRIBUTES,
                    ZxioOpenOptions::new(Some(&mut attrs), None),
                )
                .expect("open failed"),
        );
        assert!(!attrs.fsverity_enabled);

        let mut fsverity_root_hash = [0; ZXIO_ROOT_HASH_LENGTH];
        let query = zxio_node_attr_has_t {
            fsverity_enabled: true,
            fsverity_options: true,
            fsverity_root_hash: true,
            ..Default::default()
        };
        let attrs = foo_zxio
            .attr_get_with_root_hash(query, &mut fsverity_root_hash)
            .expect("attr_get failed");
        // We expect fxfs to report the value of fsverity_enabled, but it should be turned off.
        assert!(attrs.has.fsverity_enabled && !attrs.fsverity_enabled);
        // fxfs does not support the following attributes yet:
        assert!(!attrs.has.fsverity_options);
        assert!(!attrs.has.fsverity_root_hash);
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_read_link_error() {
    struct ErrorSymlink;

    impl Symlink for ErrorSymlink {
        async fn read_target(&self) -> Result<Vec<u8>, zx::Status> {
            Err(zx::Status::IO)
        }
    }

    impl Node for ErrorSymlink {
        async fn get_attributes(
            &self,
            _requested_attributes: fio::NodeAttributesQuery,
        ) -> Result<fio::NodeAttributes2, zx::Status> {
            unreachable!();
        }
    }

    impl RemoteLike for ErrorSymlink {
        fn open(
            self: Arc<Self>,
            scope: ExecutionScope,
            path: Path,
            flags: fio::Flags,
            object_request: ObjectRequestRef<'_>,
        ) -> Result<(), Status> {
            if !path.is_empty() {
                return Err(Status::NOT_DIR);
            }
            object_request
                .take()
                .create_connection_sync::<vfs::symlink::Connection<_>, _>(scope, self, flags);
            Ok(())
        }
    }

    impl DirectoryEntry for ErrorSymlink {
        fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
            request.open_remote(self)
        }
    }

    impl GetEntryInfo for ErrorSymlink {
        fn entry_info(&self) -> EntryInfo {
            EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Symlink)
        }
    }

    let dir = pseudo_directory! {
        "error_symlink" => Arc::new(ErrorSymlink),
    };

    let dir_proxy = vfs::directory::serve_read_only(dir, ExecutionScope::new());
    fasync::unblock(|| {
        let handle = dir_proxy.into_channel().unwrap().into_zx_channel().into_handle();
        let dir_zxio = Zxio::create(handle).expect("create failed");

        assert_eq!(
            dir_zxio
                .open("error_symlink", fio::PERM_READABLE, Default::default())
                .expect_err("open succeeded"),
            zx::Status::IO
        );
    })
    .await;
}

#[fuchsia::test]
async fn test_xattr_dir() {
    let fixture = TestFixture::new().await;

    let (dir_client, dir_server) = zx::Channel::create();
    fixture
        .root()
        .open(
            "foo",
            fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::FLAG_MAYBE_CREATE,
            &Default::default(),
            dir_server,
        )
        .expect("open failed");

    fasync::unblock(|| {
        let foo_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");

        assert_matches!(foo_zxio.xattr_get(b"security.selinux"), Err(zx::Status::NOT_FOUND));

        foo_zxio.xattr_set(b"security.selinux", b"bar", XattrSetMode::Set).unwrap();

        assert_eq!(foo_zxio.xattr_get(b"security.selinux").unwrap(), b"bar");

        {
            let names = foo_zxio.xattr_list().unwrap();
            assert_eq!(names, vec![b"security.selinux".to_owned()]);
        }

        foo_zxio.xattr_remove(b"security.selinux").unwrap();

        assert_matches!(foo_zxio.xattr_get(b"security.selinux"), Err(zx::Status::NOT_FOUND));

        {
            let names = foo_zxio.xattr_list().unwrap();
            assert_eq!(names, Vec::<Vec::<u8>>::new());
        }
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_xattr_dir_multiple_attributes() {
    let fixture = TestFixture::new().await;

    let (dir_client, dir_server) = zx::Channel::create();
    fixture
        .root()
        .open(
            "foo",
            fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::PROTOCOL_DIRECTORY
                | fio::Flags::FLAG_MAYBE_CREATE,
            &Default::default(),
            dir_server,
        )
        .expect("open failed");

    fasync::unblock(|| {
        let foo_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");

        let names = &[
            b"security.selinux".to_vec(),
            b"user.sha".to_vec(),
            b"fuchsia.merkle".to_vec(),
            b"starnix.mode".to_vec(),
            b"invalid in linux but fine for fuchsia!".to_vec(),
        ];
        let mut sorted_names = names.to_vec();
        sorted_names.sort();
        let values = &[
            b"important security attribute".to_vec(),
            b"abc1234".to_vec(),
            b"fffffffffff".to_vec(),
            b"drwxrwxrwx".to_vec(),
            b"\0\0 nulls are fine in the value \0\0\0".to_vec(),
        ];

        for (name, value) in names.iter().zip(values.iter()) {
            foo_zxio.xattr_set(&name, &value, XattrSetMode::Set).unwrap();
        }

        let mut listed_names = foo_zxio.xattr_list().unwrap();
        listed_names.sort();
        // Sort the two lists, because there isn't any guaranteed order they will come back from the
        // server.
        assert_eq!(listed_names, sorted_names);
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_xattr_symlink() {
    let fixture = TestFixture::new().await;

    let (dir_client, dir_server) = zx::Channel::create();
    fixture.root().clone(dir_server.into()).expect("clone failed");

    fasync::unblock(|| {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");

        let symlink_zxio =
            dir_zxio.create_symlink("symlink", b"target").expect("create_symlink failed");
        assert_eq!(symlink_zxio.read_link().expect("read_link failed"), b"target");

        assert_matches!(symlink_zxio.xattr_get(b"security.selinux"), Err(zx::Status::NOT_FOUND));

        symlink_zxio.xattr_set(b"security.selinux", b"bar", XattrSetMode::Set).unwrap();

        assert_eq!(symlink_zxio.xattr_get(b"security.selinux").unwrap(), b"bar");

        {
            let names = symlink_zxio.xattr_list().unwrap();
            assert_eq!(names, vec![b"security.selinux".to_owned()]);
        }

        symlink_zxio.xattr_remove(b"security.selinux").unwrap();

        assert_matches!(symlink_zxio.xattr_get(b"security.selinux"), Err(zx::Status::NOT_FOUND));

        {
            let names = symlink_zxio.xattr_list().unwrap();
            assert_eq!(names, Vec::<Vec::<u8>>::new());
        }
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_xattr_file_large_attribute() {
    let fixture = TestFixture::new().await;

    let (foo_client, foo_server) = zx::Channel::create();
    fixture
        .root()
        .open(
            "foo",
            fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::FLAG_MAYBE_CREATE
                | fio::Flags::PROTOCOL_FILE,
            &Default::default(),
            foo_server,
        )
        .expect("open failed");

    fasync::unblock(|| {
        let foo_zxio = Zxio::create(foo_client.into_handle()).expect("create failed");

        let value_len = fio::MAX_INLINE_ATTRIBUTE_VALUE as usize + 64;
        let value = std::iter::repeat(0xff).take(value_len).collect::<Vec<u8>>();
        foo_zxio.xattr_set(b"user.big_attribute", &value, XattrSetMode::Set).unwrap();

        assert_eq!(foo_zxio.xattr_get(b"user.big_attribute").unwrap(), value);
    })
    .await;

    fixture.close().await;
}

// TODO(https://fxbug.dev/293943124): once fxfs supports allocate, use the test fixture.
struct AllocateFile {
    res: Result<(), Status>,
}

impl AllocateFile {
    fn new(res: Result<(), Status>) -> Arc<Self> {
        Arc::new(AllocateFile { res })
    }
}

impl DirectoryEntry for AllocateFile {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_file(self)
    }
}

impl GetEntryInfo for AllocateFile {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)
    }
}

impl FileIo for AllocateFile {
    async fn read_at(&self, _offset: u64, _buffer: &mut [u8]) -> Result<u64, Status> {
        unimplemented!()
    }
    async fn write_at(&self, _offset: u64, _content: &[u8]) -> Result<u64, Status> {
        unimplemented!()
    }
    async fn append(&self, _content: &[u8]) -> Result<(u64, u64), Status> {
        unimplemented!()
    }
}

impl vfs::node::Node for AllocateFile {
    async fn get_attributes(
        &self,
        _query: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        unimplemented!()
    }
}

impl File for AllocateFile {
    fn writable(&self) -> bool {
        true
    }
    async fn open_file(&self, _options: &FileOptions) -> Result<(), Status> {
        Ok(())
    }
    async fn truncate(&self, _length: u64) -> Result<(), Status> {
        unimplemented!()
    }
    async fn get_size(&self) -> Result<u64, Status> {
        unimplemented!()
    }
    async fn update_attributes(
        &self,
        _attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        unimplemented!()
    }
    async fn sync(&self, _mode: SyncMode) -> Result<(), Status> {
        Ok(())
    }
    async fn allocate(
        &self,
        _offset: u64,
        _length: u64,
        _mode: fio::AllocateMode,
    ) -> Result<(), Status> {
        self.res
    }
}

impl FileLike for AllocateFile {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        options: FileOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        FidlIoConnection::create_sync(scope, self, options, object_request.take());
        Ok(())
    }
}

#[fuchsia::test]
async fn test_allocate_file() {
    let dir = pseudo_directory! {
        "foo" => AllocateFile::new(Ok(())),
    };
    let dir_proxy =
        vfs::directory::serve(dir, ExecutionScope::new(), fio::PERM_READABLE | fio::PERM_WRITABLE);

    fasync::unblock(|| {
        let handle = dir_proxy.into_channel().unwrap().into_zx_channel().into_handle();
        let dir_zxio = Zxio::create(handle).expect("create failed");
        let foo_zxio = dir_zxio
            .open("foo", fio::PERM_READABLE | fio::PERM_WRITABLE, Default::default())
            .expect("open failed");

        foo_zxio.allocate(0, 10, syncio::AllocateMode::empty()).unwrap();
    })
    .await;
}

#[fuchsia::test]
async fn test_allocate_file_not_sup() {
    // For now we just tell it what error to send back, but this is mimicking the first pass at
    // allocate which won't support any options.
    let dir = pseudo_directory! {
        "foo" => AllocateFile::new(Err(Status::NOT_SUPPORTED)),
    };
    let dir_proxy =
        vfs::directory::serve(dir, ExecutionScope::new(), fio::PERM_READABLE | fio::PERM_WRITABLE);

    fasync::unblock(|| {
        let handle = dir_proxy.into_channel().unwrap().into_zx_channel().into_handle();
        let dir_zxio = Zxio::create(handle).expect("create failed");
        let foo_zxio = dir_zxio
            .open("foo", fio::PERM_READABLE | fio::PERM_WRITABLE, Default::default())
            .expect("open failed");

        assert_matches!(
            foo_zxio.allocate(0, 10, syncio::AllocateMode::COLLAPSE_RANGE),
            Err(zx::Status::NOT_SUPPORTED)
        );
        assert_matches!(
            foo_zxio.allocate(0, 10, syncio::AllocateMode::KEEP_SIZE),
            Err(zx::Status::NOT_SUPPORTED)
        );
        assert_matches!(
            foo_zxio.allocate(0, 10, syncio::AllocateMode::INSERT_RANGE),
            Err(zx::Status::NOT_SUPPORTED)
        );
        assert_matches!(
            foo_zxio.allocate(0, 10, syncio::AllocateMode::PUNCH_HOLE),
            Err(zx::Status::NOT_SUPPORTED)
        );
        assert_matches!(
            foo_zxio.allocate(0, 10, syncio::AllocateMode::UNSHARE_RANGE),
            Err(zx::Status::NOT_SUPPORTED)
        );
        assert_matches!(
            foo_zxio.allocate(0, 10, syncio::AllocateMode::ZERO_RANGE),
            Err(zx::Status::NOT_SUPPORTED)
        );
    })
    .await;
}

#[fuchsia::test]
async fn test_get_set_attributes_node() {
    let fixture = TestFixture::new().await;

    let (dir_client, dir_server) = zx::Channel::create();
    fixture.root().clone(dir_server.into()).expect("clone failed");

    fasync::unblock(|| {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");

        // Create a file.
        let test_file = dir_zxio
            .open(
                "test_file",
                fio::Flags::FLAG_MAYBE_CREATE
                    | fio::Flags::PROTOCOL_FILE
                    | fio::Flags::PERM_UPDATE_ATTRIBUTES
                    | fio::Flags::PERM_GET_ATTRIBUTES,
                ZxioOpenOptions::default(),
            )
            .expect("open failed");

        let attr = zxio_node_attributes_t {
            gid: 111,
            access_time: 222,
            modification_time: 333,
            has: zxio_node_attr_has_t {
                gid: true,
                access_time: true,
                modification_time: true,
                ..Default::default()
            },
            ..Default::default()
        };

        test_file.attr_set(&attr).expect("attr_set failed");

        let query = zxio_node_attr_has_t {
            gid: true,
            access_time: true,
            modification_time: true,
            ..Default::default()
        };

        let attributes = test_file.attr_get(query).expect("attr_get failed");
        assert_eq!(attributes.gid, 111);
        assert_eq!(attributes.access_time, 222);
        assert_eq!(attributes.modification_time, 333);
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_open() {
    let fixture = TestFixture::new().await;

    let (dir_client, dir_server) = zx::Channel::create();
    fixture.root().clone(dir_server.into()).expect("clone failed");

    fasync::unblock(|| {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");

        // Create a test directory.
        let test_dir = dir_zxio
            .open(
                "test_dir",
                fio::Flags::FLAG_MUST_CREATE
                    | fio::Flags::PROTOCOL_DIRECTORY
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE,
                ZxioOpenOptions::default(),
            )
            .expect("open failed");

        // Now create a file in that directory.
        let test_file = test_dir
            .open(
                "test_file",
                fio::Flags::FLAG_MUST_CREATE | fio::Flags::PROTOCOL_FILE | fio::PERM_WRITABLE,
                ZxioOpenOptions::default(),
            )
            .expect("open failed");

        // Write something to the file.
        const CONTENT: &[u8] = b"hello";
        test_file.write(CONTENT).expect("write failed");

        // Open the directory without specifying any protocols and ask for attributes (we expect
        // that the server is able to negotiate the node protocol to be the directory protocol).
        let mut attr = zxio_node_attributes_t {
            has: zxio_node_attr_has_t {
                protocols: true,
                abilities: true,
                id: true,
                link_count: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let _test_dir = dir_zxio
            .open(
                "test_dir",
                fio::PERM_READABLE | fio::PERM_WRITABLE,
                ZxioOpenOptions::new(Some(&mut attr), None),
            )
            .expect("open failed");

        assert_eq!(attr.protocols, zxio::ZXIO_NODE_PROTOCOL_DIRECTORY);
        assert_eq!(
            attr.abilities,
            (fio::Operations::ENUMERATE
                | fio::Operations::TRAVERSE
                | fio::Operations::MODIFY_DIRECTORY
                | fio::Operations::GET_ATTRIBUTES
                | fio::Operations::UPDATE_ATTRIBUTES)
                .bits()
        );
        // Fxfs will always return a non-zero ID.
        assert_ne!(attr.id, 0);
        assert_eq!(attr.link_count, 2);

        // And now the file, and ask for attributes.
        let mut attr = zxio_node_attributes_t {
            has: zxio_node_attr_has_t {
                protocols: true,
                abilities: true,
                id: true,
                content_size: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let _test_file = test_dir
            .open("test_file", fio::Flags::empty(), ZxioOpenOptions::new(Some(&mut attr), None))
            .expect("open failed");
        assert_eq!(attr.protocols, zxio::ZXIO_NODE_PROTOCOL_FILE);
        assert_eq!(
            attr.abilities,
            (fio::Operations::READ_BYTES
                | fio::Operations::WRITE_BYTES
                | fio::Operations::GET_ATTRIBUTES
                | fio::Operations::UPDATE_ATTRIBUTES)
                .bits()
        );
        // Fxfs will always return a non-zero ID.
        assert_ne!(attr.id, 0);
        assert_eq!(attr.content_size, 5);
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_open_symlink() {
    let fixture = TestFixture::new().await;

    let (dir_client, dir_server) = zx::Channel::create();
    fixture.root().clone(dir_server.into()).expect("clone failed");

    fasync::unblock(|| {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");

        // Create and open a symlink.
        dir_zxio.create_symlink("symlink", b"target").expect("create_symlink failed");
        let symlink = dir_zxio
            .open(
                "symlink",
                fio::Flags::PROTOCOL_SYMLINK | fio::Flags::PERM_GET_ATTRIBUTES,
                ZxioOpenOptions::default(),
            )
            .expect("open failed");
        assert_eq!(symlink.read_link().expect("read_link failed"), b"target");

        // Open should also work without specifying any protocol.
        let symlink = dir_zxio
            .open("symlink", fio::Flags::PERM_GET_ATTRIBUTES, ZxioOpenOptions::default())
            .expect("open failed");
        assert_eq!(symlink.read_link().expect("read_link failed"), b"target");
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_open_file_protocol_flags() {
    let fixture = TestFixture::new().await;

    let (dir_client, dir_server) = zx::Channel::create();
    fixture.root().clone(dir_server.into()).expect("clone failed");

    fasync::unblock(|| {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");

        let test_file = dir_zxio
            .open(
                "test_file",
                fio::Flags::FLAG_MUST_CREATE
                    | fio::Flags::PROTOCOL_FILE
                    | fio::PERM_WRITABLE
                    | fio::PERM_READABLE
                    | fio::Flags::FILE_APPEND,
                ZxioOpenOptions::default(),
            )
            .expect("open failed");

        // Write something to the file.
        const CONTENT: &[u8] = b"hello";
        test_file.write(CONTENT).expect("write failed");

        const APPEND_CONTENT: &[u8] = b" there";
        test_file.write(APPEND_CONTENT).expect("write failed");

        test_file.seek(SeekOrigin::Start, 0).expect("seek failed");
        let mut buf = [0; 20];
        assert_eq!(
            test_file.read(&mut buf).expect("read failed"),
            CONTENT.len() + APPEND_CONTENT.len()
        );
        assert_eq!(&buf[..CONTENT.len()], CONTENT);
        assert_eq!(&buf[CONTENT.len()..CONTENT.len() + APPEND_CONTENT.len()], APPEND_CONTENT);
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_open_create_attributes() {
    let fixture = TestFixture::new().await;

    let (dir_client, dir_server) = zx::Channel::create();
    fixture.root().clone(dir_server.into()).expect("clone failed");

    fasync::unblock(|| {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");

        // Ask for attributes.
        let mut attr = zxio_node_attributes_t {
            has: zxio_node_attr_has_t {
                protocols: true,
                abilities: true,
                id: true,
                uid: true,
                ..Default::default()
            },
            ..Default::default()
        };

        // Set node id.
        let create_attr = zxio_node_attributes_t {
            uid: 123456,
            has: zxio_node_attr_has_t { uid: true, ..Default::default() },
            ..Default::default()
        };

        let _test_directory = dir_zxio
            .open(
                "test_dir",
                fio::Flags::FLAG_MUST_CREATE
                    | fio::Flags::PROTOCOL_DIRECTORY
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE,
                ZxioOpenOptions::new(Some(&mut attr), Some(create_attr)),
            )
            .expect("open failed");

        assert_eq!(attr.protocols, zxio::ZXIO_NODE_PROTOCOL_DIRECTORY);
        assert_eq!(
            attr.abilities,
            (fio::Operations::ENUMERATE
                | fio::Operations::TRAVERSE
                | fio::Operations::MODIFY_DIRECTORY
                | fio::Operations::GET_ATTRIBUTES
                | fio::Operations::UPDATE_ATTRIBUTES)
                .bits()
        );
        // Fxfs will always return a non-zero ID.
        assert_ne!(attr.id, 0);
        assert_eq!(attr.uid, 123456);
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_open_rights() {
    let fixture = TestFixture::new().await;

    let (dir_client, dir_server) = zx::Channel::create();
    fixture.root().clone(dir_server.into()).expect("clone failed");

    fasync::unblock(|| {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");

        // Create and write to a file.
        let test_file = dir_zxio
            .open(
                "test_file",
                fio::Flags::FLAG_MUST_CREATE | fio::Flags::PROTOCOL_FILE | fio::PERM_WRITABLE,
                ZxioOpenOptions::default(),
            )
            .expect("open failed");

        // Write something to the file.
        const CONTENT: &[u8] = b"hello";
        test_file.write(CONTENT).expect("write failed");

        // Check rights
        let test_file = dir_zxio
            .open("test_file", fio::PERM_READABLE, ZxioOpenOptions::default())
            .expect("open failed");
        let mut buf = [0; 20];
        assert_eq!(test_file.read(&mut buf).expect("read failed"), CONTENT.len());
        assert_eq!(&buf[..CONTENT.len()], CONTENT);

        // Make sure we can't write to the file.
        assert_eq!(test_file.write(&buf).expect_err("write succeeded"), zx::Status::BAD_HANDLE);

        // Check that no rights are inherited when specifying no permission flags.
        let test_file = dir_zxio
            .open("test_file", fio::Flags::empty(), ZxioOpenOptions::default())
            .expect("open failed");
        // Make sure we can't read or write to the file.
        assert_eq!(test_file.read(&mut buf).expect_err("read succeeded"), zx::Status::BAD_HANDLE);
        assert_eq!(test_file.write(&buf).expect_err("write succeeded"), zx::Status::BAD_HANDLE);

        // Optional rights on directories.
        let test_dir = dir_zxio
            .open(
                ".",
                fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE,
                ZxioOpenOptions::default(),
            )
            .expect("open failed");

        // Now make sure we can open and write to the test file.
        let test_file = test_dir
            .open("test_file", fio::PERM_READABLE | fio::PERM_WRITABLE, ZxioOpenOptions::default())
            .expect("open failed");
        assert_eq!(test_file.read(&mut buf).expect("read failed"), CONTENT.len());
        assert_eq!(&buf[..CONTENT.len()], CONTENT);
        // This time we should be able to write to the file.
        test_file.write(b"foo").expect("write failed");
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_open_node() {
    let fixture = TestFixture::new().await;

    let (dir_client, dir_server) = zx::Channel::create();
    fixture.root().clone(dir_server.into()).expect("clone failed");

    fasync::unblock(|| {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");

        dir_zxio.open_node(".", fio::Flags::PROTOCOL_DIRECTORY, None).expect("open_node failed");

        // Create a file.
        let test_file = dir_zxio
            .open(
                "test_file",
                fio::Flags::FLAG_MUST_CREATE | fio::Flags::PROTOCOL_FILE | fio::PERM_WRITABLE,
                ZxioOpenOptions::default(),
            )
            .expect("open failed");

        // Write something to the file.
        const CONTENT: &[u8] = b"hello";
        test_file.write(CONTENT).expect("write failed");

        // Check we can get attributes.
        let mut attr = zxio_node_attributes_t {
            has: zxio_node_attr_has_t { protocols: true, content_size: true, ..Default::default() },
            ..Default::default()
        };
        dir_zxio
            .open_node("test_file", fio::Flags::empty(), Some(&mut attr))
            .expect("open_node failed");
        assert_eq!(attr.protocols, zxio::ZXIO_NODE_PROTOCOL_FILE);
        assert_eq!(attr.content_size, 5);

        // It should work if other protocols are specified, as long as it matches the target node.
        assert_eq!(
            dir_zxio
                .open_node("test_file", fio::Flags::PROTOCOL_DIRECTORY, None,)
                .expect_err("open_node unexpectedly succeeded"),
            zx::Status::NOT_DIR
        );

        let mut attr = zxio_node_attributes_t {
            has: zxio_node_attr_has_t { protocols: true, content_size: true, ..Default::default() },
            ..Default::default()
        };
        dir_zxio
            .open_node("test_file", fio::Flags::PROTOCOL_FILE, Some(&mut attr))
            .expect("open_node failed");
        assert_eq!(attr.protocols, zxio::ZXIO_NODE_PROTOCOL_FILE);
        assert_eq!(attr.content_size, 5);
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_casefold() {
    let fixture = TestFixture::new().await;
    let root = fixture.root();
    let test_directory = fuchsia_fs::directory::open_directory(
        &root,
        "test_dir",
        fio::PERM_READABLE
            | fio::PERM_WRITABLE
            | fio::Flags::PROTOCOL_DIRECTORY
            | fio::Flags::FLAG_MAYBE_CREATE,
    )
    .await
    .unwrap();

    let (dir_client, dir_server) = zx::Channel::create();
    test_directory.clone(dir_server.into()).expect("clone failed");

    fasync::unblock(move || {
        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");
        let attr = zxio_node_attributes_t {
            gid: 111,
            modification_time: 333,
            casefold: true,
            has: zxio_node_attr_has_t {
                gid: true,
                modification_time: true,
                casefold: true,
                ..Default::default()
            },
            ..Default::default()
        };

        dir_zxio.attr_set(&attr).expect("attr_set failed");

        let query = zxio_node_attr_has_t {
            gid: true,
            modification_time: true,
            casefold: true,
            ..Default::default()
        };

        let attributes = dir_zxio.attr_get(query).expect("attr_get failed");
        assert_eq!(attributes.gid, 111);
        assert_eq!(attributes.modification_time, 333);
        assert_eq!(attributes.casefold, true);

        let _file = dir_zxio
            .open(
                "foo",
                fio::Flags::FLAG_MUST_CREATE
                    | fio::Flags::PROTOCOL_FILE
                    | fio::PERM_READABLE
                    | fio::PERM_WRITABLE,
                Default::default(),
            )
            .expect("open failed");

        let _file = dir_zxio
            .open(
                "FOO",
                fio::Flags::PROTOCOL_FILE | fio::PERM_READABLE | fio::PERM_WRITABLE,
                Default::default(),
            )
            .expect("open failed");
    })
    .await;

    fixture.close().await;
}

#[fuchsia::test]
async fn test_open_selinux_context_attr() {
    let fixture = TestFixture::new().await;
    let root = fixture.root();

    let (dir_client, dir_server) = zx::Channel::create();
    root.clone(dir_server.into()).expect("clone failed");

    fasync::unblock(move || {
        const CONTEXT_STRING: &str = "context";
        const TEST_FILE: &str = "foo";

        assert!(CONTEXT_STRING.as_bytes().len() <= fio::MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN as usize);

        let dir_zxio = Zxio::create(dir_client.into_handle()).expect("create failed");
        // Set value during creation.
        {
            let write_buf = CONTEXT_STRING.as_bytes().to_vec();
            let mut selinux_context_buffer =
                MaybeUninit::<[u8; fio::MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN as usize]>::uninit();
            let mut read_buf = SelinuxContextAttr::new(&mut selinux_context_buffer);
            // Verify that we can still fetch attributes properly at the same time.
            let mut attr: zxio_node_attributes_t = Default::default();
            let _file = dir_zxio
                .open(
                    TEST_FILE,
                    fio::Flags::FLAG_MUST_CREATE
                        | fio::Flags::PROTOCOL_FILE
                        | fio::PERM_READABLE
                        | fio::PERM_WRITABLE
                        | fio::Flags::PERM_GET_ATTRIBUTES
                        | fio::Flags::PERM_UPDATE_ATTRIBUTES,
                    ZxioOpenOptions::new(Some(&mut attr), None)
                        .with_selinux_context_read(&mut read_buf)
                        .unwrap()
                        .with_selinux_context_write(&write_buf)
                        .unwrap(),
                )
                .expect("Creating file");
            assert!(attr.has.selinux_context);
            assert_eq!(attr.selinux_context_state, zxio::ZXIO_SELINUX_CONTEXT_STATE_DATA);
            assert_eq!(attr.selinux_context_length as usize, CONTEXT_STRING.as_bytes().len());
            assert_eq!(read_buf.get().unwrap(), CONTEXT_STRING.as_bytes());
        }

        // Retrieve the value on reopen
        {
            let mut attr: zxio_node_attributes_t = Default::default();
            let mut selinux_context_buffer =
                MaybeUninit::<[u8; fio::MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN as usize]>::uninit();
            let mut read_buf = SelinuxContextAttr::new(&mut selinux_context_buffer);
            let file = dir_zxio
                .open(
                    TEST_FILE,
                    fio::Flags::PROTOCOL_FILE
                        | fio::PERM_READABLE
                        | fio::PERM_WRITABLE
                        | fio::Flags::PERM_GET_ATTRIBUTES
                        | fio::Flags::PERM_UPDATE_ATTRIBUTES,
                    ZxioOpenOptions::new(Some(&mut attr), None)
                        .with_selinux_context_read(&mut read_buf)
                        .unwrap(),
                )
                .expect("Opening file");

            assert!(attr.has.selinux_context);
            assert_eq!(attr.selinux_context_state, zxio::ZXIO_SELINUX_CONTEXT_STATE_DATA);
            assert_eq!(attr.selinux_context_length as usize, CONTEXT_STRING.as_bytes().len());
            assert_eq!(read_buf.get().unwrap(), CONTEXT_STRING.as_bytes());

            // Make the value too long to fetch.
            file.xattr_set(
                fio::SELINUX_CONTEXT_NAME.as_bytes(),
                &[0x45u8; fio::MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN as usize + 1],
                XattrSetMode::Replace,
            )
            .expect("Setting a long xattr");
        }

        // Value is too long to return on open, so it says so.
        {
            let mut attr: zxio_node_attributes_t = Default::default();
            let mut selinux_context_buffer =
                MaybeUninit::<[u8; fio::MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN as usize]>::uninit();
            let mut read_buf = SelinuxContextAttr::new(&mut selinux_context_buffer);
            let _file = dir_zxio
                .open(
                    TEST_FILE,
                    fio::Flags::PROTOCOL_FILE
                        | fio::PERM_READABLE
                        | fio::PERM_WRITABLE
                        | fio::Flags::PERM_GET_ATTRIBUTES
                        | fio::Flags::PERM_UPDATE_ATTRIBUTES,
                    ZxioOpenOptions::new(Some(&mut attr), None)
                        .with_selinux_context_read(&mut read_buf)
                        .unwrap(),
                )
                .expect("Opening file");

            assert_eq!(attr.selinux_context_state, zxio::ZXIO_SELINUX_CONTEXT_STATE_USE_XATTRS);
        }
    })
    .await;

    fixture.close().await;
}
