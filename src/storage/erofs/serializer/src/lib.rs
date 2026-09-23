// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An extremely simple erofs serializer. Doesn't do any efficient allocation, everything is
//! compact inodes with FlatPlain data layouts. This is intended to allow creating on-the-fly erofs
//! images for conformance testing the vfs implementation as opposed to making robust erofs images.

use erofs::format;
use zerocopy::IntoBytes;
use zerocopy::byteorder::little_endian::{U16 as LEU16, U32 as LEU32, U64 as LEU64};

/// A node in the tree to be serialized.
#[derive(Debug, Clone)]
pub enum SerializerNode {
    /// A directory with node children.
    Directory { name: String, entries: Vec<SerializerNode> },
    /// A file with byte contents.
    File { name: String, data: Vec<u8> },
    /// A symlink pointing to a target.
    Symlink { name: String, target: Vec<u8> },
}

impl SerializerNode {
    pub fn name(&self) -> &str {
        match self {
            Self::Directory { name, .. } => name,
            Self::File { name, .. } => name,
            Self::Symlink { name, .. } => name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeType {
    Dir,
    File,
    Symlink,
}

struct FlatNode {
    nid: u64,
    node_type: NodeType,
    contents: FlatNodeContents,
    data_block: u32,
    size: u64,
}

enum FlatNodeContents {
    Directory {
        // (name, nid, node_type)
        entries: Vec<(String, u64, NodeType)>,
    },
    File {
        data: Vec<u8>,
    },
    Symlink {
        target: Vec<u8>,
    },
}

fn add_node(node: &SerializerNode, nodes: &mut Vec<FlatNode>, parent_nid: u64) -> u64 {
    let nid = nodes.len() as u64;
    match node {
        SerializerNode::Directory { name: _, entries } => {
            // Push placeholder
            nodes.push(FlatNode {
                nid,
                node_type: NodeType::Dir,
                contents: FlatNodeContents::Directory { entries: Vec::new() },
                data_block: 0,
                size: 0,
            });

            let mut child_entries = Vec::new();
            child_entries.push((".".to_string(), nid, NodeType::Dir));
            child_entries.push(("..".to_string(), parent_nid, NodeType::Dir));

            for child in entries {
                let child_name = child.name().to_string();
                let child_nid = add_node(child, nodes, nid);
                let child_type = nodes[child_nid as usize].node_type;
                child_entries.push((child_name, child_nid, child_type));
            }

            child_entries.sort_by(|a, b| a.0.cmp(&b.0));
            nodes[nid as usize].contents = FlatNodeContents::Directory { entries: child_entries };
        }
        SerializerNode::File { name: _, data } => {
            nodes.push(FlatNode {
                nid,
                node_type: NodeType::File,
                contents: FlatNodeContents::File { data: data.clone() },
                data_block: 0,
                size: 0,
            });
        }
        SerializerNode::Symlink { name: _, target } => {
            nodes.push(FlatNode {
                nid,
                node_type: NodeType::Symlink,
                contents: FlatNodeContents::Symlink { target: target.clone() },
                data_block: 0,
                size: 0,
            });
        }
    }
    nid
}

/// Serialize a directory tree into a very simple erofs image. Intended for testing at the moment.
/// As such, it may panic in various edge-cases and is not especially efficient at laying out the
/// metadata.
pub fn serialize(root_entries: &[SerializerNode]) -> Vec<u8> {
    let mut nodes = Vec::<FlatNode>::new();

    // Create root directory at NID 0
    nodes.push(FlatNode {
        nid: 0,
        node_type: NodeType::Dir,
        contents: FlatNodeContents::Directory { entries: Vec::new() },
        data_block: 0,
        size: 0,
    });

    let mut root_child_entries = Vec::new();
    root_child_entries.push((".".to_string(), 0, NodeType::Dir));
    root_child_entries.push(("..".to_string(), 0, NodeType::Dir));

    for child in root_entries {
        let child_name = child.name().to_string();
        let child_nid = add_node(child, &mut nodes, 0);
        let child_type = nodes[child_nid as usize].node_type;
        root_child_entries.push((child_name, child_nid, child_type));
    }

    root_child_entries.sort_by(|a, b| a.0.cmp(&b.0));
    nodes[0].contents = FlatNodeContents::Directory { entries: root_child_entries };

    // Allocate blocks
    let inode_blocks = (nodes.len() * 32).div_ceil(4096);
    let mut next_free_block = 1 + inode_blocks as u32;

    for i in 0..nodes.len() {
        match &nodes[i].contents {
            FlatNodeContents::Directory { .. } => {
                nodes[i].data_block = next_free_block;
                nodes[i].size = 4096;
                next_free_block += 1;
            }
            FlatNodeContents::File { data } => {
                let len = data.len() as u64;
                nodes[i].size = len;
                if len > 0 {
                    nodes[i].data_block = next_free_block;
                    let blocks_needed = len.div_ceil(4096);
                    next_free_block += blocks_needed as u32;
                } else {
                    nodes[i].data_block = 0;
                }
            }
            FlatNodeContents::Symlink { target } => {
                let len = target.len() as u64;
                nodes[i].size = len;
                if len > 0 {
                    nodes[i].data_block = next_free_block;
                    let blocks_needed = len.div_ceil(4096);
                    next_free_block += blocks_needed as u32;
                } else {
                    nodes[i].data_block = 0;
                }
            }
        }
    }

    let total_blocks = next_free_block;
    let mut image = vec![0u8; total_blocks as usize * 4096];

    // 1. Write Superblock
    let sb = format::SuperBlock {
        magic: LEU32::new(format::EROFS_MAGIC),
        checksum: LEU32::new(0),
        feature_compat: LEU32::new(0),
        block_size_bits: 12, // 4096
        sb_ext_slots: 0,
        root_nid: LEU16::new(0),
        inode_count: LEU64::new(nodes.len() as u64),
        epoch: LEU64::new(1700000000),
        fixed_nsec: LEU32::new(0),
        blocks: LEU32::new(total_blocks),
        meta_block_addr: LEU32::new(1),
        xattr_block_addr: LEU32::new(0),
        uuid: [0; 16],
        volume_name: [0; 16],
        feature_incompat: LEU32::new(0),
        available_compr_algs: LEU16::new(0),
        extra_devices: LEU32::new(0),
        dirblkbits: 0,
        reserved: [0; 37],
    };
    image[1024..1024 + 128].copy_from_slice(sb.as_bytes());

    // 2. Write Inodes
    for i in 0..nodes.len() {
        let mode = match nodes[i].node_type {
            NodeType::Dir => 0o040000 | 0o755,     // S_IFDIR | rwxr-xr-x
            NodeType::File => 0o100000 | 0o644,    // S_IFREG | rw-r--r--
            NodeType::Symlink => 0o120000 | 0o777, // S_IFLNK | rwxrwxrwx
        };

        let link_count = if nodes[i].node_type == NodeType::Dir { 2 } else { 1 };
        let i_u = nodes[i].data_block.to_le_bytes();

        let inode = format::InodeCompact {
            format: LEU16::new(0), // Compact + FlatPlain
            xattr_icount: LEU16::new(0),
            mode: LEU16::new(mode),
            link_count: LEU16::new(link_count),
            size: LEU32::new(nodes[i].size as u32),
            reserved_1: [0; 4],
            i_u,
            ino: LEU32::new(nodes[i].nid as u32),
            uid: LEU16::new(100),
            gid: LEU16::new(200),
            reserved_2: [0; 4],
        };
        let offset = 4096 + i * 32;
        image[offset..offset + 32].copy_from_slice(inode.as_bytes());
    }

    // 3. Write Data Blocks
    for i in 0..nodes.len() {
        match &nodes[i].contents {
            FlatNodeContents::Directory { entries } => {
                let mut dir_block = vec![0u8; 4096];
                let k = entries.len();
                let mut current_nameoff = (k * 12) as u16;

                let mut dirents = Vec::new();
                let mut name_bytes = Vec::new();

                for (name, nid, node_type) in entries {
                    let file_type = match node_type {
                        NodeType::Dir => 2,
                        NodeType::File => 1,
                        NodeType::Symlink => 7,
                    };
                    dirents.push(format::Dirent {
                        nid: LEU64::new(*nid),
                        nameoff: LEU16::new(current_nameoff),
                        file_type,
                        reserved: 0,
                    });

                    name_bytes.extend_from_slice(name.as_bytes());
                    current_nameoff += name.as_bytes().len() as u16;
                }

                let dirents_bytes = dirents.as_slice().as_bytes();
                dir_block[..dirents_bytes.len()].copy_from_slice(dirents_bytes);

                let names_start = dirents_bytes.len();
                dir_block[names_start..names_start + name_bytes.len()].copy_from_slice(&name_bytes);

                let offset = nodes[i].data_block as usize * 4096;
                image[offset..offset + 4096].copy_from_slice(&dir_block);
            }
            FlatNodeContents::File { data } => {
                if !data.is_empty() {
                    let offset = nodes[i].data_block as usize * 4096;
                    image[offset..offset + data.len()].copy_from_slice(data);
                }
            }
            FlatNodeContents::Symlink { target } => {
                if !target.is_empty() {
                    let offset = nodes[i].data_block as usize * 4096;
                    image[offset..offset + target.len()].copy_from_slice(target);
                }
            }
        }
    }

    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use erofs::readers::VecReader;
    use erofs::{ErofsFilesystem, Node};
    use std::sync::Arc;

    fn assert_dir_recursive(
        fs: &ErofsFilesystem,
        expected_entries: &[SerializerNode],
        actual_dir: &erofs::DirectoryNode,
    ) {
        let mut actual_entries_buf = vec![erofs::DirectoryEntry::default(); 100];
        let filled = fs.read_directory(actual_dir, 0, &mut actual_entries_buf).unwrap();
        let mut actual_entries = actual_entries_buf[..filled].to_vec();

        // EROFS includes '.' and '..' so filter them out first
        actual_entries.retain(|e| e.name != "." && e.name != "..");

        assert_eq!(actual_entries.len(), expected_entries.len(), "Directory entry count mismatch");

        let mut sorted_expected: Vec<&SerializerNode> = expected_entries.iter().collect();
        sorted_expected.sort_by(|a, b| a.name().cmp(b.name()));

        for (i, expected_node) in sorted_expected.iter().enumerate() {
            let actual_entry = &actual_entries[i];
            assert_eq!(actual_entry.name, expected_node.name());

            let child_node = fs.node(actual_entry.nid).expect("failed to read child node");

            match expected_node {
                SerializerNode::Directory { entries, .. } => {
                    let actual_child_dir = match child_node {
                        Node::Directory(d) => d,
                        _ => panic!("Expected directory node for {}", expected_node.name()),
                    };
                    assert_dir_recursive(fs, entries, &actual_child_dir);
                }
                SerializerNode::File { data, .. } => {
                    let actual_child_file = match child_node {
                        Node::File(f) => f,
                        _ => panic!("Expected file node for {}", expected_node.name()),
                    };
                    assert_eq!(actual_child_file.size(), data.len() as u64);
                    let mut file_buf = vec![0u8; data.len()];
                    fs.read_file_range(&actual_child_file, 0, &mut file_buf).unwrap();
                    assert_eq!(&file_buf, data);
                }
                SerializerNode::Symlink { target, .. } => {
                    let actual_child_symlink = match child_node {
                        Node::Symlink(s) => s,
                        _ => panic!("Expected symlink node for {}", expected_node.name()),
                    };
                    assert_eq!(actual_child_symlink.size(), target.len() as u64);
                    let actual_target = fs.read_symlink(&actual_child_symlink).unwrap();
                    assert_eq!(&actual_target, target);
                }
            }
        }
    }

    #[fuchsia::test]
    fn test_serialize_and_parse() {
        let tree = vec![
            SerializerNode::File { name: "file1".to_string(), data: b"hello world".to_vec() },
            SerializerNode::Symlink { name: "symlink1".to_string(), target: b"file1".to_vec() },
            SerializerNode::Directory {
                name: "dir1".to_string(),
                entries: vec![
                    SerializerNode::File {
                        name: "file2".to_string(),
                        data: b"another file!".to_vec(),
                    },
                    SerializerNode::Directory {
                        name: "subdir".to_string(),
                        entries: vec![
                            SerializerNode::File {
                                name: "file3".to_string(),
                                data: b"nested file".to_vec(),
                            },
                            SerializerNode::Symlink {
                                name: "nested_symlink".to_string(),
                                target: b"../../file1".to_vec(),
                            },
                        ],
                    },
                ],
            },
        ];

        let image = serialize(&tree);
        let reader = Arc::new(VecReader::new(image));
        let fs = ErofsFilesystem::new(reader).expect("Failed to parse serialized EROFS image");

        let root = fs.root_node();
        assert_eq!(root.ino(), 0);

        assert_dir_recursive(&fs, &tree, &root);
    }
}
