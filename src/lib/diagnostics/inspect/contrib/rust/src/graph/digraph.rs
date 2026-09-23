// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{Vertex, VertexMetadata};
use fuchsia_inspect as inspect;

/// A directed graph on top of Inspect.
#[derive(Debug)]
pub struct Digraph {
    _node: inspect::Node,
    topology_node: inspect::Node,
}

/// Options used to configure the `Digraph`.
#[derive(Debug, Default)]
pub struct DigraphOpts {}

impl Digraph {
    /// Create a new directed graph under the given `parent` node.
    pub fn new(parent: &inspect::Node, _options: DigraphOpts) -> Digraph {
        let node = parent.create_child("fuchsia.inspect.Graph");
        let topology_node = node.create_child("topology");
        Digraph { _node: node, topology_node }
    }

    /// Add a new vertex to the graph identified by the given ID and with the given initial
    /// metadata.
    pub fn add_vertex<VM: VertexMetadata>(
        &self,
        id: VM::Id,
        init_metadata: impl FnOnce(inspect::Node) -> VM,
    ) -> Vertex<VM> {
        Vertex::new(id, &self.topology_node, init_metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EdgeMetadata;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_inspect as inspect;
    use fuchsia_inspect::reader::snapshot::Snapshot;
    use fuchsia_inspect::{Inspector, Property};
    use inspect_format::{BlockIndex, BlockType};
    use std::collections::BTreeSet;

    struct BasicMeta {
        #[allow(dead_code)] // just to keep the node alive and syntax sugar.
        meta: inspect::Node,
    }

    impl VertexMetadata for BasicMeta {
        type Id = &'static str;
        type EdgeMeta = BasicMeta;
    }
    impl EdgeMetadata for BasicMeta {}

    struct NoOpWithNumericMeta;
    impl VertexMetadata for NoOpWithNumericMeta {
        type Id = &'static str;
        type EdgeMeta = NumericMeta;
    }
    impl EdgeMetadata for NoOpWithNumericMeta {}

    struct NoOpMeta;
    impl VertexMetadata for NoOpMeta {
        type Id = &'static str;
        type EdgeMeta = NoOpMeta;
    }
    impl EdgeMetadata for NoOpMeta {}

    struct NumericMeta {
        node: inspect::Node,
        int: Option<inspect::IntProperty>,
        uint: Option<inspect::UintProperty>,
        double: Option<inspect::DoubleProperty>,
    }
    impl EdgeMetadata for NumericMeta {}

    impl VertexMetadata for NumericMeta {
        type Id = &'static str;
        type EdgeMeta = NoOpMeta;
    }

    #[fuchsia::test]
    async fn test_simple_graph() {
        let inspector = inspect::Inspector::default();

        // Create a new graph.
        let graph = Digraph::new(inspector.root(), DigraphOpts::default());

        // Create a new node with some properties.
        let mut vertex_foo = graph.add_vertex("element-1", |meta| {
            meta.record_string("name", "foo");
            meta.record_uint("level", 1u64);
            BasicMeta { meta }
        });

        let mut vertex_bar = graph.add_vertex("element-2", |meta| {
            meta.record_string("name", "bar");
            meta.record_int("level", 2i64);
            BasicMeta { meta }
        });

        // Create a new edge.
        let edge_foo_bar = vertex_foo.add_edge(&mut vertex_bar, |meta| {
            meta.record_string("src", "on");
            meta.record_string("dst", "off");
            meta.record_string("type", "passive");
            BasicMeta { meta }
        });

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "element-1": {
                        "meta": {
                            name: "foo",
                            level: 1u64,
                        },
                        "relationships": {
                            "element-2": {
                                "edge_id": edge_foo_bar.id(),
                                "meta": {
                                    "type": "passive",
                                    src: "on",
                                    dst: "off"
                                }
                            }
                        }
                    },
                    "element-2": {
                        "meta": {
                            name: "bar",
                            level: 2i64,
                        },
                        "relationships": {}
                    }
                }
            }
        });
    }

    #[fuchsia::test]
    async fn test_all_metadata_types_on_nodes() {
        let inspector = inspect::Inspector::default();

        // Create a new graph.
        let graph = Digraph::new(inspector.root(), DigraphOpts::default());

        // Create a new node with some properties.
        let mut vertex = graph.add_vertex("test-node", |node| {
            let int = Some(node.create_int("int_property", 2i64));
            let uint = Some(node.create_uint("uint_property", 4u64));
            let double = Some(node.create_double("double_property", 2.5));
            NumericMeta { node, int, uint, double }
        });

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "test-node": {
                        "meta": {
                            double_property: 2.5,
                            int_property: 2i64,
                            uint_property: 4u64,
                        },
                        "relationships": {}
                    },
                }
            }
        });

        // We can update all properties.
        vertex.meta().int.as_ref().unwrap().set(1i64);
        vertex.meta().uint.as_ref().unwrap().set(3u64);
        vertex.meta().double.as_ref().unwrap().set(4.25);

        // Or insert properties.
        vertex.meta().node.record_int("new_one", 123);

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "test-node": {
                        "meta": {
                            int_property: 1i64,
                            uint_property: 3u64,
                            double_property: 4.25f64,
                            new_one: 123i64,
                        },
                        "relationships": {}
                    },
                },
            }
        });

        // Or remove them.
        vertex.meta().int = None;
        vertex.meta().uint = None;
        vertex.meta().double = None;

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "test-node": {
                        "meta": {
                            new_one: 123i64,
                        },
                        "relationships": {}
                    },
                }
            }
        });
    }

    #[fuchsia::test]
    async fn test_all_metadata_types_on_edges() {
        let inspector = inspect::Inspector::default();

        // Create a new graph.
        let graph = Digraph::new(inspector.root(), DigraphOpts::default());

        // Create a new node with some properties.
        let mut vertex_one = graph.add_vertex("test-node-1", |_| NoOpWithNumericMeta);
        let mut vertex_two = graph.add_vertex("test-node-2", |_| NoOpWithNumericMeta);
        let edge = vertex_one.add_edge(&mut vertex_two, |node| {
            let int = Some(node.create_int("int_property", 2i64));
            let uint = Some(node.create_uint("uint_property", 4u64));
            let double = Some(node.create_double("double_property", 2.5));
            NumericMeta { node, int, uint, double }
        });

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "test-node-1": {
                        "relationships": {
                            "test-node-2": {
                                "edge_id": edge.id(),
                                "meta": {
                                    int_property: 2i64,
                                    uint_property: 4u64,
                                    double_property: 2.5,
                                },
                            }
                        }
                    },
                    "test-node-2": {
                        "relationships": {},
                    }
                }
            }
        });

        edge.maybe_update_meta(|meta| {
            // We can update all properties.
            meta.int.as_ref().unwrap().set(1i64);
            meta.uint.as_ref().unwrap().set(3u64);
            meta.double.as_ref().unwrap().set(4.25);
            // Or insert properties.
            meta.node.record_int("new_one", 123);
        });

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "test-node-1": {
                        "relationships": {
                            "test-node-2": {
                                "edge_id": edge.id(),
                                "meta": {
                                    int_property: 1i64,
                                    uint_property: 3u64,
                                    double_property: 4.25f64,
                                    new_one: 123i64,
                                },
                            }
                        }
                    },
                    "test-node-2": {
                        "relationships": {},
                    }
                }
            }
        });

        // Or remove them.
        edge.maybe_update_meta(|meta| {
            meta.int = None;
            meta.uint = None;
            meta.double = None;
        });

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "test-node-1": {
                        "relationships": {
                            "test-node-2": {
                                "edge_id": edge.id(),
                                "meta": {
                                    new_one: 123i64,
                                }
                            }
                        }
                    },
                    "test-node-2": {
                        "relationships": {},
                    }
                }
            }
        });
    }

    #[fuchsia::test]
    async fn test_raii_semantics() {
        let inspector = inspect::Inspector::default();
        let graph = Digraph::new(inspector.root(), DigraphOpts::default());
        let mut foo = graph.add_vertex("foo", |meta| {
            meta.record_bool("hello", true);
            BasicMeta { meta }
        });
        let mut bar = graph.add_vertex("bar", |meta| {
            meta.record_bool("hello", false);
            BasicMeta { meta }
        });
        let mut baz = graph.add_vertex("baz", |meta| BasicMeta { meta });

        let edge_foo = bar.add_edge(&mut foo, |meta| {
            meta.record_string("hey", "hi");
            BasicMeta { meta }
        });
        let edge_to_baz = bar.add_edge(&mut baz, |meta| {
            meta.record_string("good", "bye");
            BasicMeta { meta }
        });

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "foo": {
                        "meta": {
                            hello: true,
                        },
                        "relationships": {},
                    },
                    "bar": {
                        "meta": {
                            hello: false,
                        },
                        "relationships": {
                            "foo": {
                                "edge_id": edge_foo.id(),
                                "meta": {
                                    hey: "hi",
                                },
                            },
                            "baz": {
                                "edge_id": edge_to_baz.id(),
                                "meta": {
                                    good: "bye",
                                },
                            }
                        }
                    },
                    "baz": {
                        "meta": {},
                        "relationships": {}
                    }
                }
            }
        });

        // Dropping an edge removes it from the graph, along with all properties.
        drop(edge_foo);

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "foo": {
                        "meta": {
                            hello: true,
                        },
                        "relationships": {},
                    },
                    "bar": {
                        "meta": {
                            hello: false,
                        },
                        "relationships": {
                            "baz": {
                                "edge_id": edge_to_baz.id(),
                                "meta": {
                                    good: "bye",
                                },
                            }
                        }
                    },
                    "baz": {
                        "meta": {},
                        "relationships": {}
                    }
                }
            }
        });

        // Dropping a node removes it from the graph along with all edges and properties.
        drop(bar);

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "foo": {
                        "meta": {
                            hello: true,
                        },
                        "relationships": {}
                    },
                    "baz": {
                        "meta": {},
                        "relationships": {}
                    }
                }
            }
        });

        // Dropping all nodes leaves an empty graph.
        drop(foo);
        drop(baz);

        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {},
            }
        });
    }

    #[fuchsia::test]
    async fn drop_target_semantics() {
        let inspector = inspect::Inspector::default();
        let graph = Digraph::new(inspector.root(), DigraphOpts::default());
        let mut vertex_one = graph.add_vertex("test-node-1", |_| NoOpMeta);
        let mut vertex_two = graph.add_vertex("test-node-2", |_| NoOpMeta);
        let edge = vertex_one.add_edge(&mut vertex_two, |_| NoOpMeta);
        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "test-node-1": {
                        "relationships": {
                            "test-node-2": {
                                "edge_id": edge.id(),
                            }
                        }
                    },
                    "test-node-2": {
                        "relationships": {},
                    }
                }
            }
        });

        // Drop the target vertex.
        drop(vertex_two);

        // The edge is gone too regardless of us still holding it.
        assert_data_tree!(inspector, root: {
            "fuchsia.inspect.Graph": {
                "topology": {
                    "test-node-1": {
                        "relationships": {}
                    }
                }
            }
        });
    }

    #[fuchsia::test]
    fn validate_inspect_vmo_on_edge_drop() {
        let inspector = inspect::Inspector::default();
        let graph = Digraph::new(inspector.root(), DigraphOpts::default());
        let mut vertex_a = graph.add_vertex("a", |meta| BasicMeta { meta });
        let mut vertex_b = graph.add_vertex("b", |meta| BasicMeta { meta });

        let blocks_before = non_free_blocks(&inspector);

        // Creating an edge and dropping it, is a no-op operation on the blocks in the VMO. All of
        // them should be gone.
        let edge_a_b = vertex_a.add_edge(&mut vertex_b, |meta| {
            meta.record_string("src", "on");
            BasicMeta { meta }
        });
        drop(edge_a_b);

        let blocks_after = non_free_blocks(&inspector);
        assert_eq!(
            blocks_after.symmetric_difference(&blocks_before).collect::<BTreeSet<_>>(),
            BTreeSet::new()
        );
    }

    #[fuchsia::test]
    fn validate_inspect_vmo_on_dest_vertex_drop() {
        let inspector = inspect::Inspector::default();
        let graph = Digraph::new(inspector.root(), DigraphOpts::default());
        let mut vertex_a = graph.add_vertex("a", |meta| BasicMeta { meta });

        let initial_blocks = non_free_blocks(&inspector);

        let mut vertex_b = graph.add_vertex("b", |meta| BasicMeta { meta });
        let _edge_a_b = vertex_a.add_edge(&mut vertex_b, |meta| {
            meta.record_string("src", "on");
            BasicMeta { meta }
        });

        // Dropping the vertex should drop all of the edge and vertex_b blocks.
        drop(vertex_b);

        let after_blocks = non_free_blocks(&inspector);
        // There should be no difference between the remaining blocks and the original ones before
        // the operations executed.
        assert_eq!(
            after_blocks.symmetric_difference(&initial_blocks).collect::<BTreeSet<_>>(),
            BTreeSet::new()
        );
    }

    #[fuchsia::test]
    fn validate_inspect_vmo_on_source_vertex_drop() {
        let inspector = inspect::Inspector::default();
        let graph = Digraph::new(inspector.root(), DigraphOpts::default());

        let initial_blocks = non_free_blocks(&inspector);
        let mut vertex_a = graph.add_vertex("a", |meta| BasicMeta { meta });

        let before_blocks = non_free_blocks(&inspector);
        let mut vertex_b = graph.add_vertex("b", |meta| BasicMeta { meta });
        let vertex_b_blocks = non_free_blocks(&inspector)
            .difference(&before_blocks)
            .cloned()
            .collect::<BTreeSet<_>>();

        let _edge_a_b = vertex_a.add_edge(&mut vertex_b, |meta| {
            meta.record_string("src", "on");
            BasicMeta { meta }
        });

        // Dropping the vertex should drop all of the edge and vertex_a blocks, except for the
        // blocks shared with vertex_b (for example string references for the strings "meta" and
        // "relationships".
        drop(vertex_a);

        let mut expected = initial_blocks.union(&vertex_b_blocks).cloned().collect::<BTreeSet<_>>();
        // These two blocks are expected, since they are the "meta" and "relationships" strings.
        expected.insert((BlockIndex::new(14), BlockType::StringReference));
        expected.insert((BlockIndex::new(16), BlockType::StringReference));
        let after_blocks = non_free_blocks(&inspector);
        assert_eq!(after_blocks, expected);
    }

    #[fuchsia::test]
    async fn test_multiple_edges_between_vertices_drop() {
        let inspector = inspect::Inspector::default();
        let graph = Digraph::new(inspector.root(), DigraphOpts::default());
        let mut vertex_a = graph.add_vertex("a", |meta| BasicMeta { meta });
        let mut vertex_b = graph.add_vertex("b", |meta| BasicMeta { meta });

        let edge1 = vertex_a.add_edge(&mut vertex_b, |meta| BasicMeta { meta });
        let edge2 = vertex_a.add_edge(&mut vertex_b, |meta| BasicMeta { meta });

        let mut updated1 = false;
        edge1.maybe_update_meta(|_| {
            updated1 = true;
        });
        assert!(updated1);

        let mut updated2 = false;
        edge2.maybe_update_meta(|_| {
            updated2 = true;
        });
        assert!(updated2);

        drop(vertex_a);

        let mut updated1_after = false;
        edge1.maybe_update_meta(|_| {
            updated1_after = true;
        });
        assert!(!updated1_after);

        let mut updated2_after = false;
        edge2.maybe_update_meta(|_| {
            updated2_after = true;
        });
        assert!(!updated2_after);
    }

    fn non_free_blocks(inspector: &Inspector) -> BTreeSet<(BlockIndex, BlockType)> {
        let snapshot = Snapshot::try_from(inspector).unwrap();
        snapshot
            .scan()
            .filter(|block| block.block_type() != Some(BlockType::Free))
            .map(|b| (b.index(), b.block_type().unwrap()))
            .collect::<BTreeSet<_>>()
    }
}
