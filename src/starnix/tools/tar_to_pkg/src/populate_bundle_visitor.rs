// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ext4_metadata::{S_IFDIR, S_IFLNK, S_IFREG};
use fuchsia_pkg::PathToStringExt;
use std::collections::{HashMap, HashSet};

use crate::layered_image;
use crate::layered_image::DirectoryVisitor;

/// A directory visitor that fills an ext4 Metadata object.
pub struct PopulateBundleVisitor {
    pub ext4_metadata: ext4_metadata::Metadata,
    pub visited_inode_nums: HashSet<u64>,
    pub name_stack: Vec<String>,
    pub manifest: HashMap<String, String>,
}

impl PopulateBundleVisitor {
    pub fn new() -> PopulateBundleVisitor {
        PopulateBundleVisitor {
            ext4_metadata: ext4_metadata::Metadata::new(),
            visited_inode_nums: HashSet::new(),
            name_stack: Vec::new(),
            manifest: HashMap::new(),
        }
    }

    pub fn add_child_at_current_name_stack(&mut self, inode_num: u64) {
        let path: Vec<&str> = self.name_stack.iter().map(String::as_str).collect();
        self.ext4_metadata.add_child(&path, inode_num);
    }
}

impl DirectoryVisitor for PopulateBundleVisitor {
    fn visit_file(&mut self, name: &[u8], file: &layered_image::File) {
        let inode_num = file.metadata().inode_num();
        let is_first_visit = self.visited_inode_nums.insert(inode_num);

        if is_first_visit {
            self.ext4_metadata.insert_file(
                inode_num,
                file.metadata().mode() | S_IFREG,
                file.metadata().uid(),
                file.metadata().gid(),
                file.metadata().extended_attributes().clone().into(),
            );
            self.manifest
                .insert(format!("{inode_num}"), file.data_file_path().path_to_string().unwrap());
        }

        let name = String::from_utf8(name.to_vec()).expect("Invalid utf8 in name");
        self.name_stack.push(name);
        self.add_child_at_current_name_stack(inode_num);
        self.name_stack.pop();
    }

    fn visit_symlink(&mut self, name: &[u8], symlink: &layered_image::Symlink) {
        let inode_num = symlink.metadata().inode_num();
        let is_first_visit = self.visited_inode_nums.insert(inode_num);

        if is_first_visit {
            self.ext4_metadata.insert_symlink(
                inode_num,
                String::from_utf8(symlink.target().to_vec()).expect("Invalid utf8 in target"),
                symlink.metadata().mode() | S_IFLNK,
                symlink.metadata().uid(),
                symlink.metadata().gid(),
                symlink.metadata().extended_attributes().clone().into(),
            );
        }

        let name = String::from_utf8(name.to_vec()).expect("Invalid utf8 in name");
        self.name_stack.push(name);
        self.add_child_at_current_name_stack(inode_num);
        self.name_stack.pop();
    }

    fn visit_directory(&mut self, name: &[u8], directory: &layered_image::Directory) {
        let inode_num = directory.metadata().inode_num();
        let is_first_visit = self.visited_inode_nums.insert(inode_num);

        if is_first_visit {
            self.ext4_metadata.insert_directory(
                inode_num,
                directory.metadata().mode() | S_IFDIR,
                directory.metadata().uid(),
                directory.metadata().gid(),
                directory.metadata().extended_attributes().clone().into(),
            );
        }

        let name = String::from_utf8(name.to_vec()).expect("Invalid utf8 in name");
        self.name_stack.push(name);
        self.add_child_at_current_name_stack(inode_num);
        if is_first_visit {
            directory.visit(self); // recurse into directory
        }
        self.name_stack.pop();
    }
}
