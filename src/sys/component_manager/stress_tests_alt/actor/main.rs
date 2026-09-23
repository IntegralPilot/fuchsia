// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod stressor;

use crate::stressor::Stressor;
use anyhow::Result;
use futures::FutureExt;
use futures::future::BoxFuture;
use log::warn;
use moniker::Moniker;
use rand::RngExt as _;
use rand::rngs::SmallRng;
use rand::seq::IndexedRandom as _;
use stress_test_actor::{Action, actor_loop};

const COLLECTION_NAME: &'static str = "dynamic_children";
const ECHO_CLIENT_URL: &'static str = "#meta/unreliable_echo_client.cm";
const NO_BINARY_URL: &'static str = "#meta/no_binary.cm";

#[fuchsia::main]
pub async fn main() -> Result<()> {
    let stressor = Stressor::from_namespace();

    actor_loop(
        stressor,
        vec![
            Action { name: "create_child", run: create_child },
            Action { name: "destroy_child", run: destroy_child },
        ],
    )
    .await
}

pub fn create_child<'a>(
    stressor: &'a mut Stressor,
    mut rng: SmallRng,
) -> BoxFuture<'a, Result<()>> {
    async move {
        let instances = stressor.get_instances_in_realm().await;

        // The root must always be a choice
        let parent_moniker = instances.choose(&mut rng).unwrap();
        let child_name = format!("C{}", rng.random::<u64>());
        let url = if rng.random_bool(0.5) { ECHO_CLIENT_URL } else { NO_BINARY_URL };
        let url = url.to_string();

        let result = stressor
            .create_child(parent_moniker, COLLECTION_NAME.to_string(), child_name, url)
            .await;
        if let Err(e) = result {
            // Errors from creation are assumed to be because of collisions with another actor.
            warn!("Ignoring error in create_child operation: {:?}", e)
        }
        Ok(())
    }
    .boxed()
}

pub fn destroy_child<'a>(
    stressor: &'a mut Stressor,
    mut rng: SmallRng,
) -> BoxFuture<'a, Result<()>> {
    async move {
        let instances = stressor.get_instances_in_realm().await;

        // The root cannot be destroyed. Remove it.
        let instances: Vec<String> = instances
            .into_iter()
            .filter(|m| !Moniker::try_from(m.as_str()).unwrap().is_root())
            .collect();

        if let Some(moniker) = instances.choose(&mut rng) {
            let moniker = Moniker::parse_str(moniker).unwrap();
            let (parent_moniker, child_name) = moniker.split_leaf().unwrap();
            let child_name = child_name.name().to_string();
            let parent_moniker = parent_moniker.to_string();

            let result = stressor
                .destroy_child(&parent_moniker, COLLECTION_NAME.to_string(), child_name)
                .await;
            if let Err(e) = result {
                // Errors from creation are assumed to be because of collisions with another actor.
                warn!("Ignoring error in destroy_child operation: {:?}", e)
            }
        }
        Ok(())
    }
    .boxed()
}
