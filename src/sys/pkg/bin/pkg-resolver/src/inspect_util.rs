// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_pkg_ext::RepositoryConfig;
use fuchsia_inspect::{self as inspect, NumericProperty};
use fuchsia_inspect_contrib::inspectable::{Inspectable, Watch};
use std::borrow::Cow;
use std::sync::Arc;

pub type InspectableRepositoryConfig =
    Inspectable<Arc<RepositoryConfig>, InspectableRepositoryConfigWatcher>;

pub struct InspectableRepositoryConfigWatcher {
    node: inspect::Node,
}

impl Watch<Arc<RepositoryConfig>> for InspectableRepositoryConfigWatcher {
    fn new<'a>(
        config: &Arc<RepositoryConfig>,
        node: &inspect::Node,
        name: impl Into<Cow<'a, str>>,
    ) -> Self {
        let node = node.create_child(name);
        let mut ret = Self { node };
        ret.watch(config);
        ret
    }

    fn watch(&mut self, config: &Arc<RepositoryConfig>) {
        self.node.clear_recorded();
        self.node.record_string("repo_url", format!("{}", config.repo_url()));
        self.node.record_uint("root_version", config.root_version().into());
        self.node.record_uint("root_threshold", config.root_threshold().into());
        self.node.record_bool("use_local_mirror", config.use_local_mirror());
        self.node.record_string("repo_storage_type", format!("{:?}", config.repo_storage_type()));
        self.node.record_child("root_keys", |n| {
            let () =
                config.root_keys().iter().enumerate().for_each(|(i, root_key)| {
                    n.record_string(i.to_string(), format!("{root_key:?}"))
                });
        });
        self.node.record_child("mirrors", |n| {
            config.mirrors().iter().enumerate().for_each(|(i, mirror_config)| {
                n.record_child(i.to_string(), |n| {
                    n.record_string("mirror_url", format!("{}", mirror_config.mirror_url()));
                    n.record_string("subscribe", format!("{:?}", mirror_config.subscribe()));
                    n.record_string(
                        "blob_mirror_url",
                        format!("{}", mirror_config.blob_mirror_url()),
                    );
                })
            });
        });
    }
}

#[derive(Debug)]
pub struct Counter {
    prop: inspect::UintProperty,
}

impl Counter {
    pub fn new(parent: &inspect::Node, name: &str) -> Self {
        Self { prop: parent.create_uint(name, 0) }
    }

    pub fn increment(&self) {
        self.prop.add(1);
    }
}

#[cfg(test)]
mod test_inspectable_repository_config {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fidl_fuchsia_pkg_ext::{MirrorConfigBuilder, RepositoryConfigBuilder, RepositoryKey};
    use http::Uri;

    #[fuchsia::test]
    async fn test_initialization() {
        let inspector = inspect::Inspector::default();
        let fuchsia_url = fuchsia_url::RepositoryUrl::parse("fuchsia-pkg://fuchsia.test/").unwrap();
        let mirror_config =
            MirrorConfigBuilder::new("http://fake-mirror.com".parse::<Uri>().unwrap())
                .unwrap()
                .build();
        let config = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url)
                .add_root_key(RepositoryKey::Ed25519(vec![0]))
                .add_mirror(mirror_config.clone())
                .build(),
        );
        let inspectable =
            InspectableRepositoryConfig::new(config, inspector.root(), "test-property");

        assert_data_tree!(
            inspector,
            root: {
                "test-property": {
                  root_keys: {
                    "0": format!("{:?}", inspectable.root_keys()[0])
                  },
                  mirrors: {
                    "0": {
                        mirror_url: "http://fake-mirror.com/",
                        subscribe: "false",
                        blob_mirror_url: "http://fake-mirror.com/blobs",
                    }
                  },
                  repo_storage_type: "Ephemeral",
                  repo_url: "fuchsia-pkg://fuchsia.test",
                  root_threshold: 1u64,
                  root_version: 1u64,
                  use_local_mirror: false,
                }
            }
        );
    }

    #[fuchsia::test]
    async fn test_watcher() {
        let inspector = inspect::Inspector::default();
        let fuchsia_url = fuchsia_url::RepositoryUrl::parse("fuchsia-pkg://fuchsia.test").unwrap();
        let config = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url)
                .add_root_key(RepositoryKey::Ed25519(vec![0]))
                .build(),
        );
        let mirror_config =
            MirrorConfigBuilder::new("http://fake-mirror.com".parse::<Uri>().unwrap())
                .unwrap()
                .build();
        let mut inspectable =
            InspectableRepositoryConfig::new(config, inspector.root(), "test-property");

        Arc::get_mut(&mut inspectable.get_mut())
            .expect("get repo config")
            .insert_mirror(mirror_config.clone());

        assert_data_tree!(
            inspector,
            root: {
                "test-property": {
                  root_keys: {
                    "0": format!("{:?}", inspectable.root_keys()[0])
                  },
                  mirrors: {
                    "0": {
                        mirror_url: "http://fake-mirror.com/",
                        subscribe: "false",
                        blob_mirror_url: "http://fake-mirror.com/blobs",
                    }
                  },
                  repo_storage_type: "Ephemeral",
                  repo_url: "fuchsia-pkg://fuchsia.test",
                  root_threshold: 1u64,
                  root_version: 1u64,
                  use_local_mirror: false,
                }
            }
        );
    }
}
