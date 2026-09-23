// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zerocopy::{BigEndian, FromBytes, Immutable, KnownLayout, U32, U64};

/// The devictree header.
///
/// See https://devicetree-specification.readthedocs.io/en/v0.3/flattened-format.html#header
#[derive(FromBytes, Debug, KnownLayout, Immutable, Copy, Clone, Default)]
#[repr(C)]
pub struct Header {
    /// The value 0xd00dfeed.
    pub magic: U32<BigEndian>,

    /// The total size in bytes of the devicetree data structure.
    ///
    /// This includes the header, the memory reservation block, structure block, as well
    /// as any gaps between or after the blocks.
    pub totalsize: U32<BigEndian>,

    /// The offset, in bytes, of the structure block.
    pub off_dt_struct: U32<BigEndian>,

    /// The offset, in bytes, of the strings block.
    pub off_dt_strings: U32<BigEndian>,

    /// The offset, in bytes, of the memory reservation block.
    pub off_mem_rsvmap: U32<BigEndian>,

    /// The version of the devicetree structure.
    pub version: U32<BigEndian>,

    /// The lowest version of the devicetree data structure with which this version is
    /// backwards compatible.
    pub last_comp_version: U32<BigEndian>,

    /// The physical ID of the system's boot CPU.
    pub boot_cpuid_phys: U32<BigEndian>,

    /// The size, in bytes, of the strings block section.
    pub size_dt_strings: U32<BigEndian>,

    /// The size, in bytes, of the structure block section.
    pub size_dt_struct: U32<BigEndian>,
}

#[derive(FromBytes, Debug, KnownLayout, Immutable, Copy, Clone)]
#[repr(C)]
pub struct ReserveEntry {
    /// The physical address of the reserved memory region.
    pub address: U64<BigEndian>,

    /// The size, in bytes, of the reserved memory region.
    pub size: U64<BigEndian>,
}

#[derive(Debug, Default, Clone)]
pub struct Property {
    /// The name of the property, which is stored as a null-terminated string in the strings
    /// section.
    pub name: String,

    /// The value of the property.
    pub value: Vec<u8>,
}

#[derive(Debug, Default, Clone)]
pub struct Node {
    pub name: String,
    pub properties: Vec<Property>,
    pub children: Vec<Node>,
}

impl Node {
    pub(crate) fn new(name: String) -> Self {
        Node { name, properties: vec![], children: vec![] }
    }

    // Finds the first node with a name matching `prefix`, using a depth first search.
    pub fn find(&self, prefix: &str) -> Option<&Node> {
        if self.name.starts_with(prefix) {
            return Some(self);
        }

        for child in &self.children {
            let found_child = child.find(prefix);
            if found_child.is_some() {
                return found_child;
            }
        }

        return None;
    }

    pub fn get_property(&self, name: &str) -> Option<&Property> {
        self.properties.iter().find(|p| p.name == name)
    }
}

#[derive(Debug, Default)]
pub struct Devicetree {
    pub header: Header,
    pub reserve_entries: Vec<ReserveEntry>,
    pub root_node: Node,
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_node_find() {
        let child1 = Node { name: "child1".to_string(), properties: vec![], children: vec![] };
        let child2 = Node { name: "child2".to_string(), properties: vec![], children: vec![] };
        let root = Node { name: "root".to_string(), properties: vec![], children: vec![child1, child2] };

        assert_eq!(root.find("root").unwrap().name, "root");
        assert_eq!(root.find("child1").unwrap().name, "child1");
        assert_eq!(root.find("child2").unwrap().name, "child2");
        assert!(root.find("nonexistent").is_none());
    }

    #[test]
    fn test_node_find_nested() {
        let grandchild = Node { name: "grandchild".to_string(), properties: vec![], children: vec![] };
        let child = Node { name: "child".to_string(), properties: vec![], children: vec![grandchild] };
        let root = Node { name: "root".to_string(), properties: vec![], children: vec![child] };

        assert_eq!(root.find("root").unwrap().name, "root");
        assert_eq!(root.find("child").unwrap().name, "child");
        assert_eq!(root.find("grandchild").unwrap().name, "grandchild");
        assert!(root.find("nonexistent").is_none());
    }

    #[test]
    fn test_node_find_duplicate() {
        let property_value = vec![];
        let property = Property { name: "p1".to_string(), value: property_value };
        let child1 = Node { name: "child".to_string(), properties: vec![property], children: vec![] };
        let child2 = Node { name: "child".to_string(), properties: vec![], children: vec![] };
        let root = Node { name: "root".to_string(), properties: vec![], children: vec![child1, child2] };

        assert_eq!(root.find("child").unwrap().name, "child");
        assert!(root.find("child").unwrap().get_property("p1").is_some());
    }
}
