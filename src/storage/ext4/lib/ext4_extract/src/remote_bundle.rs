// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::METADATA_PATH;

use ext4_lib::structs::INode;
use ext4_metadata::{ExtendedAttributes, Metadata};
use std::collections::HashMap;
use std::io::{Error, ErrorKind, Result};

pub struct Writer {
    out_dir: String,
    metadata: Metadata,
    manifest: HashMap<String, String>,
}

impl Writer {
    /// Creates a remote bundle writer which will store its files at `out_dir`.
    pub fn new(
        out_dir: &impl AsRef<str>,
        inode: u64,
        mode: u16,
        owner: Owner,
        xattr: ExtendedAttributes,
    ) -> Result<Writer> {
        let mut metadata = Metadata::new();
        // Insert the root entry.
        metadata.insert_directory(inode, mode, owner.uid, owner.gid, xattr);
        let out_dir: String = out_dir.as_ref().to_owned();
        Ok(Writer { out_dir, metadata, manifest: HashMap::new() })
    }

    /// Add the contents of `data` as a file at `path` in the remote bundle.
    pub fn add_file(
        &mut self,
        path: &[&str],
        data: &mut impl std::io::Read,
        inode: u64,
        mode: u16,
        owner: Owner,
        xattr: ExtendedAttributes,
    ) -> Result<()> {
        self.metadata.insert_file(inode, mode, owner.uid, owner.gid, xattr);
        self.metadata.add_child(path, inode);

        let out_dir = &self.out_dir;
        let blob_path = format!("{out_dir}/{inode}");

        let mut file = std::fs::File::create(&blob_path)?;
        std::io::copy(data, &mut file)?;

        self.manifest.insert(format!("{inode}"), blob_path);
        Ok(())
    }

    /// Add an empty directory at `path` in the remote bundle.
    pub fn add_directory(
        &mut self,
        path: &[&str],
        inode: u64,
        mode: u16,
        owner: Owner,
        xattr: ExtendedAttributes,
    ) {
        self.metadata.insert_directory(inode, mode, owner.uid, owner.gid, xattr);
        self.metadata.add_child(path, inode);
    }

    /// Add the contents of `data` as a symlink at `path` in the remote bundle.
    pub fn add_symlink(
        &mut self,
        path: &[&str],
        data: Vec<u8>,
        inode: u64,
        mode: u16,
        owner: Owner,
        xattr: ExtendedAttributes,
    ) -> Result<()> {
        let target = String::from_utf8(data).map_err(|e| {
            Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "Symbolic link at {} has non-utf8 target: {:?}",
                    path.join("/"),
                    e.into_bytes()
                ),
            )
        })?;
        self.metadata.insert_symlink(inode, target, mode, owner.uid, owner.gid, xattr);
        self.metadata.add_child(path, inode);
        Ok(())
    }

    /// Finish writing and return a map of the destination to source pairs.
    pub fn export(mut self) -> Result<HashMap<String, String>> {
        let out_dir = self.out_dir;
        let metadata_path = format!("{out_dir}/{METADATA_PATH}");
        std::fs::write(&metadata_path, self.metadata.serialize())?;
        self.manifest.insert(METADATA_PATH.to_string(), metadata_path);
        Ok(self.manifest)
    }
}

#[derive(Clone, Copy)]
pub struct Owner {
    pub uid: u16,
    pub gid: u16,
}

impl Owner {
    pub fn root() -> Owner {
        Owner { uid: 0, gid: 0 }
    }

    pub fn from_inode(inode: &INode) -> Owner {
        Owner { uid: inode.e2di_uid.get(), gid: inode.e2di_gid.get() }
    }
}
