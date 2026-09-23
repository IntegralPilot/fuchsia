// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async::Scope;
use fuchsia_inspect::*;
use futures::prelude::*;
use inspect_runtime::PublishOptions;
use inspect_test_component_config_bindings::Config;

#[fuchsia::main]
async fn main() {
    let config = Config::take_from_startup_handle();
    let trees = Scope::new();
    for i in 0..config.publish_n_trees {
        trees.spawn(launch_inspect_tree(i));
    }

    trees.join().await
}

async fn launch_inspect_tree(unique_id: u64) {
    let inspector = Inspector::default();
    let root = inspector.root();
    root.record_uint(format!("tree-{unique_id}"), unique_id);
    root.record_int("int", 3);
    root.record_lazy_child("lazy-node", || {
        async move {
            let inspector = Inspector::default();
            inspector.root().record_string("a", "test");
            let child = inspector.root().create_child("child");
            child.record_lazy_values("lazy-values", || {
                async move {
                    let inspector = Inspector::default();
                    inspector.root().record_double("double", 3.25);
                    Ok(inspector)
                }
                .boxed()
            });
            inspector.root().record(child);
            Ok(inspector)
        }
        .boxed()
    });
    if let Some(inspect_server) = inspect_runtime::publish(
        &inspector,
        PublishOptions::default().inspect_tree_name(format!("tree-{unique_id}")),
    ) {
        inspect_server.await;
    }
}
