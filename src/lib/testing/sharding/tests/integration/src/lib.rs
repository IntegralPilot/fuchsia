// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use fidl_fuchsia_test_manager::{CaseStatus, RunSuiteOptions};
use fuchsia_async;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use regex::Regex;
use test_manager_test_lib::RunEvent;

pub async fn run_test(
    test_url: String,
    run_disabled_tests: bool,
    parallel: Option<u16>,
    test_args: Vec<String>,
) -> Vec<RunEvent> {
    let time_taken = Regex::new(r" \(.*?\)$").unwrap();
    let suite_runner =
        test_runners_test_lib::connect_to_suite_runner().await.expect("connect to suite runner");
    let runner = test_manager_test_lib::SuiteRunner::new(suite_runner);
    let run_options = RunSuiteOptions {
        run_disabled_tests: Some(run_disabled_tests),
        max_concurrent_test_case_runs: parallel,
        arguments: Some(test_args),
        ..Default::default()
    };
    let suite_instance = runner
        .start_suite_run(&test_url, run_options)
        .expect("should successfully create suite instance");
    let (mut events, _logs) = test_runners_test_lib::process_events(suite_instance, false)
        .await
        .expect("process events without error");
    for event in events.iter_mut() {
        match event {
            RunEvent::CaseStdout { name, stdout_message } => {
                println!("[{name}] {stdout_message}");
                // Clear away timestamps to allow for cleaner assertions on test stdout.
                let log = time_taken.replace(&stdout_message, "");
                *event = RunEvent::case_stdout(name.to_string(), log.to_string());
            }
            _ => {}
        }
    }
    events
}

#[fuchsia_async::run_singlethreaded(test)]
async fn run_example_sharded_test() {
    let test_urls = [0, 1, 2]
        .into_iter()
        .map(|n| {
            format!(
                "fuchsia-pkg://fuchsia.com/example-sharded-test#meta/\
                 example-test_shard_{n}_of_3.cm"
            )
        })
        .collect::<Vec<_>>();
    let events = futures::stream::FuturesUnordered::from_iter(
        test_urls.into_iter().map(|url| run_test(url, false, Some(10), vec![])),
    )
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    let relevant_case_events = events
        .into_iter()
        .filter_map(|event| match event {
            RunEvent::CaseStopped { name, status } => {
                assert_eq!(status, CaseStatus::Passed, "case {name} failed");
                Some(name)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let num_events = relevant_case_events.len();
    let seen_cases = relevant_case_events.into_iter().collect::<HashSet<_>>();
    assert_eq!(seen_cases.len(), num_events, "each case should have been run exactly once");

    for section in 1..=9 {
        for case in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"] {
            let want = format!("section_{section}::case_{case}");
            assert!(seen_cases.contains(&want), "failed to see case {}", want);
        }
    }
}

#[fuchsia_async::run_singlethreaded(test)]
async fn run_example_sharded_test_with_expectations() {
    let test_urls = [0, 1, 2]
        .into_iter()
        .map(|n| {
            format!(
                "fuchsia-pkg://fuchsia.com/example-sharded-test#meta/\
                 example-test-with-expectations_shard_{n}_of_3.cm"
            )
        })
        .collect::<Vec<_>>();
    let events = futures::stream::FuturesUnordered::from_iter(
        test_urls.into_iter().map(|url| run_test(url, false, Some(10), vec![])),
    )
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    let relevant_case_events = events
        .into_iter()
        .filter_map(|event| match event {
            RunEvent::CaseStopped { name, status } => {
                assert_eq!(status, CaseStatus::Passed, "case {name} failed");
                Some(name)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let num_events = relevant_case_events.len();
    let seen_cases = relevant_case_events.into_iter().collect::<HashSet<_>>();
    assert_eq!(seen_cases.len(), num_events, "each case should have been run exactly once");

    for section in 1..=9 {
        for case in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"] {
            let want = format!("section_{section}::case_{case}");
            assert!(seen_cases.contains(&want), "failed to see case {}", want);
        }
    }
}

#[fuchsia_async::run_singlethreaded(test)]
async fn run_example_sharded_test_with_shard_part_regex() {
    let test_urls = [0, 1, 2]
        .into_iter()
        .map(|n| {
            format!(
                "fuchsia-pkg://fuchsia.com/example-sharded-test#meta/\
                 example-test-sharded-by-section_shard_{n}_of_3.cm"
            )
        })
        .collect::<Vec<_>>();
    let events_by_shard = futures::stream::FuturesUnordered::from_iter(
        test_urls.into_iter().map(|url| run_test(url, false, Some(10), vec![])),
    )
    .collect::<Vec<_>>()
    .await;

    let mut section_to_shard = HashMap::new();
    for (shard, events) in events_by_shard.into_iter().enumerate() {
        for event in events {
            match event {
                RunEvent::CaseStopped { name, status } => {
                    assert_eq!(status, CaseStatus::Passed, "case {name} failed");
                    let (section, _case) =
                        name.split_once("::").expect("should match format section_*::case_*");
                    match section_to_shard.entry(section.to_string()) {
                        Entry::Vacant(vacant) => {
                            let _ = vacant.insert(shard);
                        }
                        Entry::Occupied(occupied) => {
                            assert_eq!(
                                shard,
                                *occupied.get(),
                                "section {section} was not all sharded identically"
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }

    for section in 1..=9 {
        assert!(
            section_to_shard.contains_key(format!("section_{section}").as_str()),
            "failed to see section_{section}"
        );
    }
}
