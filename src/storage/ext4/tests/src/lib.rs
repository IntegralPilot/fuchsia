// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use ext4_metadata::{ExtendedAttributes, Metadata, NodeInfo, ROOT_INODE_NUM, Symlink};

// To generate the test image:
//
//   $ sudo apt-get install android-sdk-libsparse-utils
//   $ truncate /tmp/image.blk -s $((1024 * 1024))
//   $ mke2fs /tmp/image.blk -t ext4 -O ^64bit
//   # mkdir /tmp/mount
//   $ sudo mount /tmp/image.blk /tmp/mount
//   $ sudo chown <username> /tmp/mount
//   $ cd /tmp/mount
//   $ mkdir foo
//   $ cd foo
//   $ echo hello >file
//   $ setfattr -n user.a -v apple file
//   $ setfattr -n user.b -v ball file
//   $ ln -s file symlink
//   $ cd ~
//   $ sudo umount /tmp/mount
//   $ img2simg /tmp/image.blk /tmp/image.sparse
#[test]
fn test_read_image() {
    const UID: u16 = 49152; // This would need to change depending on who builds the image.
    const GID: u16 = 24403;

    // The test image should be present in the package at data/test-img.
    let m = Metadata::deserialize(
        &std::fs::read("/pkg/data/test-image/metadata.v1").expect("Failed to read metadata"),
    )
    .expect("Failed to deserialize metadata");

    let root = m.get(ROOT_INODE_NUM).expect("Missing root node");
    assert_eq!(root.mode, 0o40755);
    assert_eq!(root.uid, UID);
    assert_eq!(root.gid, 0);
    assert!(root.extended_attributes.is_empty());

    let dir_inode_num = m.lookup(ROOT_INODE_NUM, "foo").expect("foo not found");
    let node = m.get(dir_inode_num).expect("root not found");
    assert_matches!(node.info(), NodeInfo::Directory(_));
    assert_eq!(node.mode, 0o40750);
    assert_eq!(node.uid, UID);
    assert_eq!(node.gid, GID);
    assert!(node.extended_attributes.is_empty());

    let inode_num = m.lookup(dir_inode_num, "file").expect("foo not found");
    let node = m.get(inode_num).expect("root not found");
    assert_matches!(node.info(), NodeInfo::File(_));
    assert_eq!(node.mode, 0o100640);
    assert_eq!(node.uid, UID);
    assert_eq!(node.gid, GID);
    let xattr: ExtendedAttributes =
        [((*b"user.a").into(), (*b"apple").into()), ((*b"user.b").into(), (*b"ball").into())]
            .into();
    assert_eq!(&node.extended_attributes, &xattr);
    assert_eq!(
        &std::fs::read(format!("/pkg/data/test-image/{inode_num}"))
            .expect("Failed to read foo/file"),
        b"hello\n"
    );

    let inode_num = m.lookup(dir_inode_num, "symlink").expect("foo not found");
    let node = m.get(inode_num).expect("root not found");
    assert_matches!(node.info(), NodeInfo::Symlink(Symlink { target }) if target == "file");
    assert_eq!(node.mode, 0o120777);
    assert_eq!(node.uid, UID);
    assert_eq!(node.gid, GID);
    assert!(node.extended_attributes.is_empty());
}
