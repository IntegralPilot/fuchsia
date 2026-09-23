// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod json {
    use crate::processes_data::processed;
    use fdomain_fuchsia_buildinfo::BuildInfo;
    use serde::Serialize;
    use std::collections::HashSet;
    use zx_types;

    /// A node is part of the graph, and it represents a process.
    #[derive(Serialize, PartialEq, Debug)]
    pub struct Node {
        /// Process name
        pub name: String,
        /// Process koid
        pub id: u64,
    }

    /// A link is an edge of the graph, and it represents a channel.
    #[derive(Serialize, PartialEq, Debug, Eq, Hash)]
    pub struct Link {
        /// The koid of the process owning one end of the channel
        pub source: u64,
        /// The koid of the process owning the other end of the channel
        pub target: u64,
    }

    /// Metadata about the build used to generate the data.
    #[derive(Serialize, PartialEq, Debug, Eq, Hash, Default)]
    pub struct BuildMetadata {
        /// Product configuration at build time
        pub product_config: String,
        /// Board configuration at build time
        pub board_config: String,
        /// Latest commit date
        pub latest_commit_date: String,
    }

    impl From<BuildInfo> for BuildMetadata {
        fn from(build_info: BuildInfo) -> Self {
            BuildMetadata {
                product_config: build_info.product_config.unwrap_or_default(),
                board_config: build_info.board_config.unwrap_or_default(),
                latest_commit_date: build_info.latest_commit_date.unwrap_or_default(),
                ..BuildMetadata::default()
            }
        }
    }

    /// The JSON structure representing the format expected by fuchsia-map
    #[derive(Serialize, PartialEq, Debug)]
    pub struct Json {
        /// A vector containing all the nodes (processes)
        pub nodes: Vec<Node>,
        /// A set containing all the links (channels)
        pub links: HashSet<Link>,
        /// Metadata about the build
        pub build_metadata: BuildMetadata,
    }

    /// Conversion trait from processed processes information to the
    /// JSON format data needed for generating a map (Fuchsia-Map)
    pub fn make_fuchsia_map_json(
        processed: processed::ProcessesData,
        build_info: BuildInfo,
    ) -> Json {
        let nodes: Vec<Node> = processed
            .processes
            .iter()
            .map(|process| Node { name: process.name.clone(), id: process.koid })
            .collect();

        let mut links = HashSet::new();
        for process in processed.processes {
            for objects_by_type in process.objects {
                if objects_by_type.objects_type != zx_types::ZX_OBJ_TYPE_CHANNEL {
                    continue;
                }

                for channel in objects_by_type.objects {
                    if channel.peer_owner_koid != 0 {
                        let l = Link { source: process.koid, target: channel.peer_owner_koid };
                        links.insert(l);
                    }
                }
            }
        }

        let build_metadata = BuildMetadata::from(build_info);
        Json { nodes, links, build_metadata }
    }
}

#[cfg(test)]
mod tests {
    use fdomain_fuchsia_buildinfo::BuildInfo;

    use crate::fuchsia_map::json;
    use crate::processes_data::processed;
    use pretty_assertions::assert_eq;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn processed_to_json_test() {
        let processed = processed::ProcessesData {
            processes_count: 2,
            processes: vec![
                processed::Process {
                    koid: 789,
                    name: "process789".to_string(),
                    objects_count: 3,
                    objects: vec![
                        processed::KernelObjectsByType {
                            objects_type: 4,
                            objects_count: 2,
                            objects: vec![
                                processed::KernelObject {
                                    koid: 78,
                                    related_koid: 79,
                                    peer_owner_koid: 987,
                                },
                                processed::KernelObject {
                                    koid: 52,
                                    related_koid: 53,
                                    peer_owner_koid: 0,
                                },
                            ],
                        },
                        processed::KernelObjectsByType {
                            objects_type: 17,
                            objects_count: 1,
                            objects: vec![processed::KernelObject {
                                koid: 36,
                                related_koid: 0,
                                peer_owner_koid: 0,
                            }],
                        },
                    ],
                },
                processed::Process {
                    koid: 987,
                    name: "process987".to_string(),
                    objects_count: 2,
                    objects: vec![
                        processed::KernelObjectsByType {
                            objects_type: 4,
                            objects_count: 1,
                            objects: vec![processed::KernelObject {
                                koid: 78,
                                related_koid: 79,
                                peer_owner_koid: 789,
                            }],
                        },
                        processed::KernelObjectsByType {
                            objects_type: 19,
                            objects_count: 1,
                            objects: vec![processed::KernelObject {
                                koid: 28,
                                related_koid: 0,
                                peer_owner_koid: 0,
                            }],
                        },
                    ],
                },
            ],
        };

        let build_info = BuildInfo {
            product_config: Some("bazingaProduct".to_string()),
            board_config: Some("bazingaBoard".to_string()),
            version: Some("bazingaVersion".to_string()),
            latest_commit_date: Some("bazingaDate".to_string()),
            ..Default::default()
        };
        let json_map = json::make_fuchsia_map_json(processed, build_info);

        let expected_nodes = HashMap::from([
            (789, json::Node { name: "process789".to_string(), id: 789 }),
            (987, json::Node { name: "process987".to_string(), id: 987 }),
        ]);
        let expected_links = HashSet::from([
            json::Link { source: 789, target: 987 },
            json::Link { source: 987, target: 789 },
        ]);
        for node in json_map.nodes {
            let expected = expected_nodes
                .get(&node.id)
                .unwrap_or_else(|| panic!("Data contains unexpected node {:?}", node));
            assert_eq!(node, *expected);
        }
        assert_eq!(json_map.links, expected_links);
        assert_eq!(json_map.build_metadata.product_config, "bazingaProduct");
        assert_eq!(json_map.build_metadata.board_config, "bazingaBoard");
        assert_eq!(json_map.build_metadata.latest_commit_date, "bazingaDate");
    }
}
