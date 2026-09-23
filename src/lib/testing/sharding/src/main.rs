// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::hash::{DefaultHasher, Hash as _, Hasher as _};

use futures::{StreamExt as _, TryStreamExt as _};
use itertools::Itertools;
use regex::Regex;

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
struct ShardConfig {
    num_shards: u64,
    shard_index: u64,
    shard_part_regex: Option<SerdeRegex>,
}

#[derive(Debug, Clone)]
struct SerdeRegex(Regex);

impl<'de> serde::Deserialize<'de> for SerdeRegex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw_regex = <String as serde::Deserialize>::deserialize(deserializer)?;
        Regex::new(&raw_regex)
            .map_err(|err| <D::Error as serde::de::Error>::custom(err))
            .map(SerdeRegex)
    }
}

const SHARD_CONFIG_PATH: &str = "/testshard/config.json5";

#[fuchsia::main]
async fn main() {
    let shard_config = fuchsia_fs::file::read_in_namespace_to_string(SHARD_CONFIG_PATH).await;
    let shard_config = match shard_config {
        // NOTE: Backslashes have already been escaped by GN (see https://fxbug.dev/430527805).
        Ok(s) => s,
        Err(err) => {
            panic!("failed to read shard config: {err:?}");
        }
    };
    let shard_config: ShardConfig =
        serde_json::from_str(shard_config.as_str()).expect("failed to parse shard config file");
    {
        let ShardConfig { num_shards, shard_index, shard_part_regex: _ } = &shard_config;
        assert!(shard_index < num_shards);

        log::info!("running (0-indexed) shard {shard_index} of {num_shards} total shards");
    }

    let mut fs = fuchsia_component::server::ServiceFs::new_local();
    let _: &mut fuchsia_component::server::ServiceFsDir<'_, _> =
        fs.dir("svc").add_fidl_service(|s: fidl_fuchsia_test::SuiteRequestStream| s);
    let _: &mut fuchsia_component::server::ServiceFs<_> =
        fs.take_and_serve_directory_handle().expect("failed to serve ServiceFs directory");

    let original_suite =
        fuchsia_component::client::connect_to_protocol::<fidl_fuchsia_test::SuiteMarker>()
            .expect("failed to connect to original suite");

    let original_suite = &original_suite;
    let shard_config = &shard_config;

    fs.for_each_concurrent(None, |s| async move {
        handle_suite_request_stream(original_suite.clone(), s, shard_config)
            .await
            .expect("error handling fuchsia.test/Suite request stream")
    })
    .await
}

async fn handle_suite_request_stream(
    original_suite: fidl_fuchsia_test::SuiteProxy,
    suite_request_stream: fidl_fuchsia_test::SuiteRequestStream,
    shard_config: &ShardConfig,
) -> Result<(), anyhow::Error> {
    log::debug!(
        "handling fuchsia.test.Suite request stream for shard {}",
        shard_config.shard_index
    );
    let original_suite = &original_suite;
    suite_request_stream
        .map_err(anyhow::Error::from)
        .try_for_each_concurrent(None, |req| async move {
            let original_suite = original_suite.clone();
            match req {
                fidl_fuchsia_test::SuiteRequest::GetTests { iterator, control_handle: _ } => {
                    log::debug!(
                        "handling fuchsia.test.Suite#GetTests request for shard {}",
                        shard_config.shard_index
                    );
                    handle_suite_get_tests(original_suite, iterator, &shard_config).await
                }
                fidl_fuchsia_test::SuiteRequest::Run {
                    tests,
                    options,
                    listener,
                    control_handle: _,
                } => {
                    log::debug!(
                        "handling fuchsia.test.Suite#Run request for shard {}",
                        shard_config.shard_index
                    );
                    handle_suite_run_request(
                        original_suite,
                        tests,
                        options,
                        listener,
                        &shard_config,
                    )
                    .await
                }
            }
        })
        .await
}

async fn handle_suite_get_tests(
    original_suite: fidl_fuchsia_test::SuiteProxy,
    iterator: fidl::endpoints::ServerEnd<fidl_fuchsia_test::CaseIteratorMarker>,
    shard_config: &ShardConfig,
) -> Result<(), anyhow::Error> {
    let (proxy, server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_test::CaseIteratorMarker>();
    let proxy = &proxy;
    original_suite.get_tests(server_end)?;
    let (iterator_requests, handle) = iterator.into_stream_and_control_handle();
    let handle = &handle;
    iterator_requests
        .map_err(anyhow::Error::from)
        .try_for_each(|fidl_fuchsia_test::CaseIteratorRequest::GetNext { responder }| async move {
            log::debug!(
                "handling fuchsia.test.CaseIterator#GetNext request for shard {}",
                shard_config.shard_index
            );
            let tests = loop {
                let tests = match proxy.get_next().await {
                    Ok(tests) => tests,
                    Err(err) => match err {
                        fidl::Error::ClientChannelClosed { epitaph, .. } => {
                            handle.shutdown_with_epitaph(epitaph);
                            return Ok(());
                        }
                        err => {
                            panic!("unexpected error while invoking suite.get_next(): {err:?}");
                        }
                    },
                };
                // Pass along a truly empty list of tests.
                if tests.is_empty() {
                    break tests;
                }

                let tests = tests
                    .into_iter()
                    .filter(|fidl_fuchsia_test::Case { name, enabled: _, __source_breaking: _ }| {
                        is_in_shard(name.as_ref().expect("name should be present"), shard_config)
                    })
                    .collect::<Vec<_>>();

                // However, if it's only empty because we filtered, we should fetch another batch.
                if !tests.is_empty() {
                    break tests;
                }
            };

            responder.send(&tests).expect("sending response should succeed");
            Ok(())
        })
        .await
}

async fn handle_suite_run_request(
    original_suite: fidl_fuchsia_test::SuiteProxy,
    tests: Vec<fidl_fuchsia_test::Invocation>,
    options: fidl_fuchsia_test::RunOptions,
    listener: fidl::endpoints::ClientEnd<fidl_fuchsia_test::RunListenerMarker>,
    shard_config: &ShardConfig,
) -> Result<(), anyhow::Error> {
    let tests = tests
        .into_iter()
        .inspect(|invocation| {
            let fidl_fuchsia_test::Invocation { name, tag: _, __source_breaking: _ } = invocation;
            let name = name.as_ref().expect("name should be present");
            assert!(
                is_in_shard(name, shard_config),
                "got run request for case {name} not in current shard {}",
                shard_config.shard_index
            );
        })
        .collect::<Vec<_>>();
    original_suite.run(&tests, &options, listener).map_err(anyhow::Error::from)
}

fn is_in_shard(
    name: &str,
    ShardConfig { num_shards, shard_index, shard_part_regex }: &ShardConfig,
) -> bool {
    let mut hasher = DefaultHasher::new();
    match shard_part_regex {
        Some(SerdeRegex(regex)) => {
            let caps = regex
                .captures(name)
                .unwrap_or_else(|| panic!("shard_part_regex {regex:?} does not match {name:?}"));
            let matching_capture = Itertools::exactly_one(
                caps.iter()
                    // the first capture is just the whole regex, not anything in parens
                    .skip(1)
                    .flatten(),
            )
            .unwrap_or_else(|e| {
                panic!(
                    "shard_part_regex {} \
                         does not have exactly one matching capture in {name:?}: {:?}",
                    regex.as_str(),
                    e.collect::<Vec<_>>()
                )
            });
            matching_capture.as_str().hash(&mut hasher);
        }
        None => {
            name.hash(&mut hasher);
        }
    };
    (hasher.finish() / (u64::MAX / *num_shards)).clamp(0, *num_shards - 1) == *shard_index
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    use crate::{SerdeRegex, ShardConfig, is_in_shard};

    #[test]
    fn shards_reasonably_evenly() {
        const NUM_SHARDS: u64 = 10;
        let mut counts = [0usize; NUM_SHARDS as usize];

        for casename in (1..=1000).map(|x| x.to_string()) {
            let mut num_times_triggered = 0;
            for shard in 0..counts.len() {
                if is_in_shard(
                    &casename,
                    &ShardConfig {
                        num_shards: NUM_SHARDS,
                        shard_index: shard as u64,
                        shard_part_regex: None,
                    },
                ) {
                    num_times_triggered += 1;
                    counts[shard] += 1;
                }
            }
            assert_eq!(num_times_triggered, 1);
        }

        for count in counts {
            assert!(count >= 50);
            assert!(count <= 200);
        }

        println!("{counts:?}");
    }

    #[test]
    fn applies_shard_part_regex() {
        const NUM_SHARDS: u64 = 10;
        for casenum in 1..=1000 {
            let name_a = format!("some_ignored_prefix::{casenum}");
            let name_b = format!("some_other_ignored_prefix::{casenum}");
            let shard_part_regex =
                Regex::new(r#"(?:(?:some_ignored_prefix)|(?:some_other_ignored_prefix))::(\d+)"#)
                    .expect("compile regex");

            for shard in 0..NUM_SHARDS {
                let shard_config = ShardConfig {
                    num_shards: NUM_SHARDS,
                    shard_index: shard,
                    shard_part_regex: Some(SerdeRegex(shard_part_regex.clone())),
                };
                assert_eq!(
                    is_in_shard(&name_a, &shard_config),
                    is_in_shard(&name_b, &shard_config),
                    "{name_a} and {name_b} should both be in shard {shard}"
                );
            }
        }
    }
}
