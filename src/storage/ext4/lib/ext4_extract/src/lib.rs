// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::remote_bundle::{Owner, Writer};

use anyhow::{Context, Error, Result};
use ext4_lib::parser::Parser as ExtParser;
use ext4_lib::readers::{IoAdapter, Reader};
use ext4_lib::structs::{DirEntry2, EntryType};
use ext4_metadata::{ExtendedAttributes, ROOT_INODE_NUM};
use sparse::reader::SparseReader;
use std::collections::HashMap;
use std::io::Cursor;

pub mod remote_bundle;

pub(crate) const METADATA_PATH: &str = "metadata.v1";

/// Extract the files from an ext4 image at `path` and return a map of the destination
/// to source pairs. Additionally, create a metadata file that provides information
/// necessary for mounting the files from a fuchsia package.
pub fn ext4_extract(path: &str, out_dir: &str) -> Result<HashMap<String, String>, Error> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("Unable to open `{:?}'", path))?;
    let reader = if sparse::is_sparse_image(&mut file) {
        Box::new(IoAdapter::new(SparseReader::new(file)?)) as Box<dyn Reader>
    } else {
        Box::new(IoAdapter::new(file)) as Box<dyn Reader>
    };
    let parser = ExtParser::new(reader);

    let inode = parser.inode(ROOT_INODE_NUM as u32).context("Failed to read root inode")?;
    let root_xattrs: ExtendedAttributes = parser
        .inode_xattrs(ROOT_INODE_NUM as u32)?
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
    let mut writer = Writer::new(
        &out_dir,
        ROOT_INODE_NUM,
        inode.e2di_mode.get(),
        Owner::from_inode(&inode),
        root_xattrs,
    )?;

    parser.index(parser.root_inode()?, vec![], &mut |parser: &ExtParser,
                                                      path: Vec<&str>,
                                                      entry: &DirEntry2|
     -> Result<bool, Error> {
        let xattr: ExtendedAttributes = parser
            .inode_xattrs(entry.e2d_ino.get())?
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();

        let inode_num = entry.e2d_ino.get();
        let inode =
            parser.inode(inode_num).with_context(|| format!("Failed to read inode {inode_num}"))?;
        let mode = inode.e2di_mode.get();
        let owner = Owner::from_inode(&inode);

        match EntryType::from_u8(entry.e2d_type)? {
            EntryType::RegularFile => {
                let data = parser.read_data(inode_num)?;
                let mut cursor = Cursor::new(data);
                writer
                    .add_file(&path, &mut cursor, inode_num as u64, mode, owner, xattr)
                    .context("Failed to add file")?;
            }
            EntryType::SymLink => {
                let data = parser.read_data(inode_num)?;
                writer
                    .add_symlink(&path, data, inode_num as u64, mode, owner, xattr)
                    .context("Failed to add symlink")?;
            }
            EntryType::Directory => {
                writer.add_directory(&path, inode_num as u64, mode, owner, xattr);
            }
            _ => {}
        }
        Ok(true)
    })?;

    Ok(writer.export()?)
}
