// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// This module tests the diagnostics code with the actual component hierarchy.
mod tests {
    use crate::model::component::testing::wait_until_event_get_timestamp;
    use crate::model::component::{ComponentInstance, StartReason};
    use crate::model::start::Start;
    use crate::model::testing::routing_test_helpers::{RoutingTest, RoutingTestBuilder};
    use crate::model::testing::test_helpers::{
        ActionsTest, ComponentInfo, component_decl_with_test_runner,
    };
    use ::routing_test_helpers::component_id_index::make_index_file;
    use cm_rust_testing::*;
    use component_id_index::InstanceId;
    use diagnostics::escrow::DurationStats;
    use diagnostics::lifecycle::ComponentLifecycleTimeStats;
    use diagnostics_assertions::{AnyProperty, HistogramAssertion, assert_data_tree};
    use diagnostics_hierarchy::DiagnosticsHierarchy;
    use fidl::endpoints::ServerEnd;
    use fidl_fuchsia_component_runner as fcrunner;
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use fuchsia_inspect as inspect;
    use fuchsia_inspect::DiagnosticsHierarchyGetter;
    use fuchsia_sync as fsync;
    use futures::StreamExt;
    use futures::channel::mpsc;
    use hooks::EventType;
    use inspect::{ExponentialHistogramParams, LinearHistogramParams};
    use moniker::Moniker;
    use std::future;
    use std::sync::Arc;

    fn get_data(
        hierarchy: &DiagnosticsHierarchy,
        moniker: &str,
        task: Option<&str>,
    ) -> (Vec<i64>, Vec<i64>, Vec<i64>) {
        let mut path = vec!["stats", "measurements", "components", moniker];
        if let Some(task) = task {
            path.push(task);
        }
        get_data_at(&hierarchy, &path)
    }

    fn get_data_at(
        hierarchy: &DiagnosticsHierarchy,
        path: &[&str],
    ) -> (Vec<i64>, Vec<i64>, Vec<i64>) {
        let node = hierarchy.get_child_by_path(&path).expect("found stats node");
        let cpu_times = node
            .get_property("cpu_times")
            .expect("found cpu")
            .int_array()
            .expect("cpu are ints")
            .raw_values();
        let queue_times = node
            .get_property("queue_times")
            .expect("found queue")
            .int_array()
            .expect("queue are ints")
            .raw_values();
        let timestamps = node
            .get_property("timestamps")
            .expect("found timestamps")
            .int_array()
            .expect("timestamps are ints")
            .raw_values();
        (timestamps.into_owned(), cpu_times.into_owned(), queue_times.into_owned())
    }

    #[fuchsia::test]
    async fn component_manager_stats_are_tracked() {
        // Set up the test
        let test = RoutingTest::new(
            "root",
            vec![(
                "root",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("a").eager().build())
                    .build(),
            )],
        )
        .await;

        let koid =
            fuchsia_runtime::job_default().basic_info().expect("got basic info").koid.raw_koid();

        let hierarchy = test.model.context().inspector().get_diagnostics_hierarchy().await;
        let (timestamps, cpu_times, queue_times) =
            get_data(&hierarchy, "<component_manager>", Some(&koid.to_string()));
        assert_eq!(timestamps.len(), 1);
        assert_eq!(cpu_times.len(), 1);
        assert_eq!(queue_times.len(), 1);
    }

    // If a component stops without escrow, we should not record anything into the histograms.
    #[fuchsia::test]
    async fn no_escrow_no_values() {
        let components = vec![("root", ComponentDeclBuilder::new().build())];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();

        let inspector = inspect::Inspector::default();
        let stats = Arc::new(DurationStats::new(inspector.root().create_child("escrow")));
        root.hooks.install(stats.hooks());
        root.ensure_started(&StartReason::Debug).await.unwrap();
        root.stop().await.unwrap();

        assert_data_tree!(inspector, root: {
            escrow: {
                started_durations: {
                    histogram: {},
                },
                stopped_durations: {
                    histogram: {},
                }
            }
        });
    }

    // Tests the escrow stats as a component transitions between some lifecycles:
    //
    // - Start -> no records
    // - Stop with escrow once -> one started record
    // - Start again -> one started and one stopped record
    // - Stop again without escrow -> one started and two stopped records
    //
    #[fuchsia::test(allow_stalls = false)]
    async fn escrow_progression() {
        let (out_dir_tx, mut out_dir_rx) = mpsc::channel(1);
        let out_dir_tx = fsync::Mutex::new(out_dir_tx);

        let components = vec![("root", ComponentDeclBuilder::new().build())];
        let url = "test:///root";
        let test = ActionsTest::new(components[0].0, components, None).await;
        test.runner.add_host_fn(
            url,
            Box::new(move |server_end: ServerEnd<fio::DirectoryMarker>| {
                out_dir_tx.lock().try_send(server_end).unwrap();
            }),
        );
        let root = test.model.root();
        let inspector = inspect::Inspector::default();
        let stats = Arc::new(DurationStats::new(inspector.root().create_child("escrow")));
        root.hooks.install(stats.hooks());

        let event_stream =
            test.new_event_stream(vec![EventType::Started, EventType::Stopped]).await;

        // Start -> no records
        root.ensure_started(&StartReason::Debug).await.unwrap();
        test.runner.wait_for_url(url).await;
        _ = fasync::TestExecutor::poll_until_stalled(future::pending::<()>()).await;
        let start_timestamp =
            wait_until_event_get_timestamp(&event_stream, EventType::Started).await;
        assert_data_tree!(inspector, root: {
            escrow: {
                started_durations: {
                    histogram: {},
                },
                stopped_durations: {
                    histogram: {},
                }
            }
        });

        let mut started_assertion = HistogramAssertion::exponential(ExponentialHistogramParams {
            floor: 4,
            initial_step: 3,
            step_multiplier: 2,
            buckets: 12,
        });
        let mut stopped_assertion = HistogramAssertion::linear(LinearHistogramParams {
            floor: 10,
            step_size: 10,
            buckets: 24,
        });

        // Stop with escrow once -> one started record
        let info = ComponentInfo::new(root.clone()).await;
        let outgoing_server_end = out_dir_rx.next().await.unwrap();
        test.runner.send_on_escrow(
            &info.channel_id,
            fcrunner::ComponentControllerOnEscrowRequest {
                outgoing_dir: Some(outgoing_server_end),
                ..Default::default()
            },
        );
        test.runner.reset_wait_for_url(url);
        test.runner.abort_controller(&info.channel_id);
        _ = fasync::TestExecutor::poll_until_stalled(future::pending::<()>()).await;
        let stop_timestamp =
            wait_until_event_get_timestamp(&event_stream, EventType::Stopped).await;
        started_assertion.insert_values(vec![(stop_timestamp - start_timestamp).into_seconds()]);
        assert_data_tree!(inspector, root: {
            escrow: {
                started_durations: {
                    histogram: {
                        ".": started_assertion.clone(),
                    },
                },
                stopped_durations: {
                    histogram: {},
                }
            }
        });

        // Start again -> one started and one stopped record
        root.ensure_started(&StartReason::Debug).await.unwrap();
        test.runner.wait_for_url(url).await;
        _ = fasync::TestExecutor::poll_until_stalled(future::pending::<()>()).await;
        let start_timestamp =
            wait_until_event_get_timestamp(&event_stream, EventType::Started).await;
        stopped_assertion.insert_values(vec![(start_timestamp - stop_timestamp).into_seconds()]);
        assert_data_tree!(inspector, root: {
            escrow: {
                started_durations: {
                    histogram: {
                        ".": started_assertion.clone(),
                    },
                },
                stopped_durations: {
                    histogram: {
                        ".": stopped_assertion.clone(),
                    },
                }
            }
        });

        // Stop again without escrow -> one started and two stopped records
        root.stop().await.unwrap();
        _ = fasync::TestExecutor::poll_until_stalled(future::pending::<()>()).await;
        let stop_timestamp =
            wait_until_event_get_timestamp(&event_stream, EventType::Stopped).await;
        started_assertion.insert_values(vec![(stop_timestamp - start_timestamp).into_seconds()]);
        assert_data_tree!(inspector, root: {
            escrow: {
                started_durations: {
                    histogram: {
                        ".": started_assertion,
                    },
                },
                stopped_durations: {
                    histogram: {
                        ".": stopped_assertion,
                    },
                }
            }
        });
    }

    #[fuchsia::test]
    async fn tracks_events() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child_default("b").build()),
            ("b", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();

        let inspector = inspect::Inspector::default();
        let stats =
            Arc::new(ComponentLifecycleTimeStats::new(inspector.root().create_child("lifecycle")));
        root.hooks.install(stats.hooks());

        let root_timestamp = start_and_get_timestamp(root, &Moniker::root()).await.into_nanos();
        let a_timestamp = start_and_get_timestamp(root, &"a".parse().unwrap()).await.into_nanos();
        let b_timestamp = start_and_get_timestamp(root, &"a/b".parse().unwrap()).await.into_nanos();
        root.find(&"a/b".parse().unwrap()).await.unwrap().stop().await.unwrap();

        assert_data_tree!(inspector, root: {
            lifecycle: {
                early: {
                    "0": {
                        moniker: ".",
                        time: root_timestamp,
                        "type": "started",
                    },
                    "1": {
                        moniker: "a",
                        time: a_timestamp,
                        "type": "started",
                    },
                    "2": {
                        moniker: "a/b",
                        time: b_timestamp,
                        "type": "started",
                    },
                    "3": contains {
                        moniker: "a/b",
                        "type": "stopped",
                        time: AnyProperty,
                    }
                },
                late: {
                }
            }
        });
    }

    async fn start_and_get_timestamp(
        root_component: &Arc<ComponentInstance>,
        moniker: &Moniker,
    ) -> zx::BootInstant {
        let component = root_component
            .start_instance(moniker, &StartReason::Root)
            .await
            .expect("failed to bind");
        let state = component.lock_state().await;
        state.get_started_state().unwrap().timestamp
    }

    #[fuchsia::test]
    async fn component_id_index_is_populated_with_assigned_ids() {
        let mut index = component_id_index::Index::default();
        let instance_id = InstanceId::new_random(&mut rand::rng());
        index
            .insert(component_id_index::IndexEntry {
                moniker: Moniker::parse_str("/a").unwrap(),
                instance_id: instance_id.clone(),
                ignore_duplicate_id: false,
            })
            .unwrap();
        let component_id_index_path = make_index_file(index).expect("make index file");

        let test = RoutingTestBuilder::new("a", vec![])
            .set_component_id_index_path(
                component_id_index_path.path().to_owned().try_into().unwrap(),
            )
            .build()
            .await;

        let inspector = test.model.context().inspector();
        assert_data_tree!(inspector, root: contains {
            component_id_index: {
                "a": format!("{instance_id}"),
            }
        });
    }
}
