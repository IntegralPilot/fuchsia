// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// Test parsing and validation of the flat system topology.
#[unittest::suite(name = "system-topology_rust")]
mod system_topology_tests {
    use super::super::{Graph, Node};
    use crate::kernel::types::cpu_num_t;
    use unittest::{assert_eq, assert_err, assert_ok, assert_true, unwrap_ok};
    use zx_status::Status;

    // Heap-backed: the largest topology here is 3.5 KiB of nodes, too much for a kernel stack.
    type FlatTopo = fbl::Vector<zbi::TopologyNode>;

    // Appends `node`, returning its index.
    fn push_node(nodes: &mut FlatTopo, node: zbi::TopologyNode) -> u16 {
        let index = nodes.len() as u16;
        assert!(nodes.push_back(node).is_ok(), "out of memory building test topology");
        index
    }

    const fn default_arch_info() -> zbi::TopologyArchitectureInfo {
        zbi::TopologyArchitectureInfo::Arm64(zbi::TopologyArm64Info {
            cluster_1_id: 0,
            cluster_2_id: 0,
            cluster_3_id: 0,
            cpu_id: 0,
            gic_id: 0,
        })
    }

    /// Parse flat topology, simple.
    #[test]
    fn test_flat_to_heap_simple() {
        let topo = simple_topology();

        let mut graph = Graph::new();
        assert_ok!(Graph::initialize(&mut graph, &topo));
        assert_eq!(3, graph.processors().len());
        assert_eq!(4, graph.logical_processor_count()); // One of the cores is SMT2.

        // Test lookup.
        let node = unwrap_ok!(graph.processor_by_logical_id(1));
        if let zbi::TopologyEntity::Processor(ref processor) = *node.entity() {
            assert_eq!(processor.flags.bits(), zbi::TopologyProcessorFlags::PRIMARY.bits());
        } else {
            assert_true!(false, "Expected processor entity");
        }
        assert_true!(node.parent().is_some(), "Expected parent node");
        let parent = node.parent().unwrap();
        if let zbi::TopologyEntity::Cluster(ref cluster) = *parent.entity() {
            assert_eq!(cluster.performance_class, 1);
        } else {
            assert_true!(false, "Expected cluster entity");
        }

        // Test out of bounds lookup.
        assert_err!(
            graph.processor_by_logical_id(graph.logical_processor_count() as cpu_num_t),
            Status::NOT_FOUND
        );
    }

    /// Parse flat topology, complex.
    #[test]
    fn test_flat_to_heap_complex() {
        let topo = complex_topology();

        let mut graph = Graph::new();
        assert_ok!(Graph::initialize(&mut graph, &topo));
        assert_eq!(32, graph.processors().len());

        // Non-processor nodes keep their attached info: the first processor's root is the
        // first NUMA region.
        let mut root = unwrap_ok!(graph.processor_by_logical_id(0));
        while let Some(parent) = root.parent() {
            root = parent;
        }
        if let zbi::TopologyEntity::NumaRegion(ref numa_region) = *root.entity() {
            assert_eq!(numa_region.start, 0x1);
            assert_eq!(numa_region.size, 1);
        } else {
            assert_true!(false, "Expected numa region entity");
        }
    }

    /// Parse complex then walk result.
    #[test]
    fn test_flat_to_heap_walk_result() {
        let topo = complex_topology();

        let mut graph = Graph::new();
        assert_ok!(Graph::initialize(&mut graph, &topo));
        assert_eq!(32, graph.processors().len());

        // For each processor we walk all the way up the graph.
        for &processor in graph.processors() {
            let mut current: &Node = processor;
            let mut next = current.parent();
            while let Some(next_node) = next {
                // Ensure that the children lists contain all children.
                let mut found = false;
                for &child in next_node.children() {
                    found |= core::ptr::eq(child, current);
                }
                assert_true!(found, "A node is not listed as a child of its parent.");

                current = next_node;
                next = current.parent();
            }
        }
    }

    /// Fail validation if processor is not a leaf.
    #[test]
    fn test_validate_processor_not_leaf() {
        let mut topo = complex_topology();

        // Replace a die node with a processor.
        topo[1].entity = zbi::TopologyEntity::Processor(zbi::TopologyProcessor {
            architecture_info: default_arch_info(),
            flags: zbi::TopologyProcessorFlags::empty(),
            logical_ids: [0; 4],
            logical_id_count: 0,
        });

        let mut graph = Graph::new();
        assert_err!(Graph::initialize(&mut graph, &topo), Status::INVALID_ARGS);
    }

    /// Fail validation if leaf is not processor.
    #[test]
    fn test_validate_leaf_not_processor() {
        let mut topo = simple_topology();

        // Replace a processor node with a cluster.
        topo[4].entity =
            zbi::TopologyEntity::Cluster(zbi::TopologyCluster { performance_class: 0 });

        let mut graph = Graph::new();
        assert_err!(Graph::initialize(&mut graph, &topo), Status::INVALID_ARGS);
    }

    /// Fail validation if there is a cycle.
    #[test]
    fn test_validate_cycle() {
        let mut topo = complex_topology();

        // Set the parent index of the die to a processor under it.
        topo[1].parent_index = 4;

        let mut graph = Graph::new();
        assert_err!(Graph::initialize(&mut graph, &topo), Status::INVALID_ARGS);
    }

    // This is a cycle like above but fails due to parent mismatch with other nodes
    // on its level.
    /// Fail validation if a cycle with a shared parent.
    #[test]
    fn test_validate_cycle_shared_parent() {
        let mut topo = complex_topology();

        // Set the parent index of the die to a processor under it.
        topo[2].parent_index = 4;

        let mut graph = Graph::new();
        assert_err!(Graph::initialize(&mut graph, &topo), Status::INVALID_ARGS);
    }

    // Another logical way to store the graph would be hierarchical, all the top
    // level nodes together, followed by the next level, and so on.
    // We are proscriptive however that they should be stored in a depth-first
    // ordering, so this other ordering should fail validation.
    /// Fail validation if storage order is incorrect.
    #[test]
    fn test_validate_hierarchical_storage() {
        let mut topo = hierarchical_topology();

        // Set the parent index of the die to a processor under it.
        topo[2].parent_index = 4;

        let mut graph = Graph::new();
        assert_err!(Graph::initialize(&mut graph, &topo), Status::INVALID_ARGS);
    }

    /// Fail validation if processor logical_id_count exceeds capacity.
    #[test]
    fn test_validate_logical_id_count_exceeds_capacity() {
        let mut topo = simple_topology();

        if let zbi::TopologyEntity::Processor(ref mut processor) = topo[1].entity {
            processor.logical_id_count = (processor.logical_ids.len() + 1) as u8;
        } else {
            assert_true!(false, "Expected processor node at index 1");
        }

        let mut graph = Graph::new();
        assert_err!(Graph::initialize(&mut graph, &topo), Status::INVALID_ARGS);
    }

    // Defined at bottom of file, they are long and noisy.

    // Generic ARM big.LITTLE layout.
    //   [cluster]       [cluster]
    //   [p1a,p1b]      [p3]   [p4]
    fn simple_topology() -> FlatTopo {
        let mut topo = FlatTopo::new();
        let nodes = &mut topo;

        let mut logical_processor: u16 = 0;

        let big_cluster = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Cluster(zbi::TopologyCluster { performance_class: 1 }),
                parent_index: zbi::TOPOLOGY_NO_PARENT,
            },
        );

        let lp0 = logical_processor;
        let lp1 = logical_processor + 1;
        logical_processor += 2;
        push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Processor(zbi::TopologyProcessor {
                    architecture_info: default_arch_info(),
                    flags: zbi::TopologyProcessorFlags::PRIMARY,
                    logical_ids: [lp0, lp1, 0, 0],
                    logical_id_count: 2,
                }),
                parent_index: big_cluster,
            },
        );

        let little_cluster = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Cluster(zbi::TopologyCluster { performance_class: 0 }),
                parent_index: zbi::TOPOLOGY_NO_PARENT,
            },
        );

        let lp2 = logical_processor;
        logical_processor += 1;
        push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Processor(zbi::TopologyProcessor {
                    architecture_info: default_arch_info(),
                    flags: zbi::TopologyProcessorFlags::empty(),
                    logical_ids: [lp2, 0, 0, 0],
                    logical_id_count: 1,
                }),
                parent_index: little_cluster,
            },
        );

        let lp3 = logical_processor;
        push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Processor(zbi::TopologyProcessor {
                    architecture_info: default_arch_info(),
                    flags: zbi::TopologyProcessorFlags::empty(),
                    logical_ids: [lp3, 0, 0, 0],
                    logical_id_count: 1,
                }),
                parent_index: little_cluster,
            },
        );

        topo
    }

    // Same as Simple but stored with all nodes on a level adjacent to each other.
    fn hierarchical_topology() -> FlatTopo {
        let mut topo = FlatTopo::new();
        let nodes = &mut topo;

        let mut logical_processor: u16 = 0;

        let big_cluster = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Cluster(zbi::TopologyCluster { performance_class: 1 }),
                parent_index: zbi::TOPOLOGY_NO_PARENT,
            },
        );

        let little_cluster = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Cluster(zbi::TopologyCluster { performance_class: 0 }),
                parent_index: zbi::TOPOLOGY_NO_PARENT,
            },
        );

        let lp0 = logical_processor;
        let lp1 = logical_processor + 1;
        logical_processor += 2;
        push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Processor(zbi::TopologyProcessor {
                    architecture_info: default_arch_info(),
                    flags: zbi::TopologyProcessorFlags::PRIMARY,
                    logical_ids: [lp0, lp1, 0, 0],
                    logical_id_count: 2,
                }),
                parent_index: big_cluster,
            },
        );

        let lp2 = logical_processor;
        logical_processor += 1;
        push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Processor(zbi::TopologyProcessor {
                    architecture_info: default_arch_info(),
                    flags: zbi::TopologyProcessorFlags::empty(),
                    logical_ids: [lp2, 0, 0, 0],
                    logical_id_count: 1,
                }),
                parent_index: little_cluster,
            },
        );

        let lp3 = logical_processor;
        push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Processor(zbi::TopologyProcessor {
                    architecture_info: default_arch_info(),
                    flags: zbi::TopologyProcessorFlags::empty(),
                    logical_ids: [lp3, 0, 0, 0],
                    logical_id_count: 1,
                }),
                parent_index: little_cluster,
            },
        );

        topo
    }

    // Add a threadripper CCX (CPU complex), a four core cluster.
    fn add_ccx(parent: u16, nodes: &mut FlatTopo, logical_processor: &mut u16) {
        let cluster = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Cluster(zbi::TopologyCluster { performance_class: 0 }),
                parent_index: parent,
            },
        );

        let cache = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Cache(zbi::TopologyCache { cache_id: 0 }),
                parent_index: cluster,
            },
        );

        for _ in 0..4 {
            let lp0 = *logical_processor;
            let lp1 = *logical_processor + 1;
            *logical_processor += 2;
            push_node(
                nodes,
                zbi::TopologyNode {
                    entity: zbi::TopologyEntity::Processor(zbi::TopologyProcessor {
                        architecture_info: default_arch_info(),
                        flags: zbi::TopologyProcessorFlags::empty(),
                        logical_ids: [lp0, lp1, 0, 0],
                        logical_id_count: 2,
                    }),
                    parent_index: cache,
                },
            );
        }
    }

    // Roughly a threadripper 2990X.
    // Four sets of the following:
    //                [numa1]
    //                [die1]
    //     [cluster1]         [cluster2]
    //      [cache1]           [cache2]
    //  [p1][p2][p3][p4]   [p5][p6][p7][p8]
    fn complex_topology() -> FlatTopo {
        let mut topo = FlatTopo::new();
        let nodes = &mut topo;

        let mut logical_processor: u16 = 0;
        let mut die = [0u16; 4];
        let mut numa = [0u16; 4];

        numa[0] = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::NumaRegion(zbi::TopologyNumaRegion {
                    start: 0x1,
                    size: 1,
                }),
                parent_index: zbi::TOPOLOGY_NO_PARENT,
            },
        );

        die[0] = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Die(zbi::TopologyDie { reserved: 0 }),
                parent_index: numa[0],
            },
        );

        add_ccx(die[0], nodes, &mut logical_processor);
        add_ccx(die[0], nodes, &mut logical_processor);

        numa[1] = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::NumaRegion(zbi::TopologyNumaRegion {
                    start: 0x3,
                    size: 1,
                }),
                parent_index: zbi::TOPOLOGY_NO_PARENT,
            },
        );

        die[1] = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Die(zbi::TopologyDie { reserved: 0 }),
                parent_index: numa[1],
            },
        );

        add_ccx(die[1], nodes, &mut logical_processor);
        add_ccx(die[1], nodes, &mut logical_processor);

        numa[2] = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::NumaRegion(zbi::TopologyNumaRegion {
                    start: 0x5,
                    size: 1,
                }),
                parent_index: zbi::TOPOLOGY_NO_PARENT,
            },
        );

        die[2] = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Die(zbi::TopologyDie { reserved: 0 }),
                parent_index: numa[2],
            },
        );

        add_ccx(die[2], nodes, &mut logical_processor);
        add_ccx(die[2], nodes, &mut logical_processor);

        numa[3] = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::NumaRegion(zbi::TopologyNumaRegion {
                    start: 0x7,
                    size: 1,
                }),
                parent_index: zbi::TOPOLOGY_NO_PARENT,
            },
        );

        die[3] = push_node(
            nodes,
            zbi::TopologyNode {
                entity: zbi::TopologyEntity::Die(zbi::TopologyDie { reserved: 0 }),
                parent_index: numa[3],
            },
        );

        add_ccx(die[3], nodes, &mut logical_processor);
        add_ccx(die[3], nodes, &mut logical_processor);

        topo
    }
}
