// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Context;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at_dir_root};
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use rust_measure_tape_for_case::Measurable as _;
use serde_derive::Deserialize;
use zx::sys::ZX_CHANNEL_MAX_MSG_BYTES;
use {fidl_fuchsia_test as ftest, fuchsia_async as fasync};

#[derive(Deserialize, Debug)]
struct Config {
    cases: Vec<CaseConfig>,
}

#[derive(Deserialize, Debug)]
struct CaseConfig {
    name: String,

    #[serde(flatten)]
    launch_config: LaunchConfig,
}

#[derive(Deserialize, Debug)]
struct LaunchConfig {
    url: String,
    args: Vec<String>,
    collection: String,
}

#[fuchsia::main] // logging_tags ?
async fn main() -> Result<(), anyhow::Error> {
    let config = load_config()?;
    let mut names = Vec::new();
    let mut launch_configs = HashMap::new();
    for case_config in config.cases {
        names.push(case_config.name.clone());
        launch_configs.insert(case_config.name, case_config.launch_config);
    }
    let names = Rc::new(names);
    let launch_configs = Rc::new(launch_configs);

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(move |stream| {
        let names = names.clone();
        let launch_configs = launch_configs.clone();
        fasync::Task::local(async move {
            start_runner(names, launch_configs, stream).await.expect("failed to start runner.")
        })
        .detach();
    });
    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;
    Ok(())
}

fn load_config() -> Result<Config, anyhow::Error> {
    let path = "/config/test_config.json5";
    let contents =
        std::fs::read_to_string(path).with_context(|| format!("loading path: {path}"))?;
    let config: Config = serde_json5::from_str(&contents).context("parsing configuration file")?;
    Ok(config)
}

async fn start_runner(
    names: Rc<Vec<String>>,
    launch_configs: Rc<HashMap<String, LaunchConfig>>,
    mut request_stream: ftest::SuiteRequestStream,
) -> Result<(), anyhow::Error> {
    while let Some(event) = request_stream.try_next().await.map_err(|e| anyhow::anyhow!(e))? {
        match event {
            ftest::SuiteRequest::GetTests { iterator, control_handle: _ } => {
                // Return list of test cases from config
                let names = names.clone();
                fasync::Task::local(async move {
                    start_case_iterator(names, iterator.into_stream())
                        .await
                        .expect("iterating test cases")
                })
                .detach();
            }
            ftest::SuiteRequest::Run { tests, options, listener, control_handle: _ } => {
                let parent_listener = listener.into_proxy();
                for invocation in tests {
                    let name = invocation.name.unwrap();
                    let tag = invocation.tag;
                    let launch_config = match launch_configs.get(&name) {
                        Some(config) => config,
                        None => {
                            return Err(anyhow::anyhow!("Unexpected test name: {name}"));
                        }
                    };
                    let test_started = run_test_case_in_child(
                        name.clone(),
                        tag.clone(),
                        launch_config,
                        &options,
                        &parent_listener,
                    )
                    .await?;
                    if !test_started {
                        // Report a failure if we didn't start a test.
                        let (case_listener, case_listener_server) =
                            fidl::endpoints::create_proxy::<ftest::CaseListenerMarker>();
                        case_listener.finished(&ftest::Result_ {
                            status: Some(ftest::Status::Failed),
                            ..Default::default()
                        })?;
                        parent_listener.on_test_case_started(
                            &ftest::Invocation { name: Some(name), tag, ..Default::default() },
                            ftest::StdHandles { ..Default::default() },
                            case_listener_server,
                        )?;
                    }
                }
                parent_listener.on_finished()?;
            }
        }
    }
    Ok(())
}

async fn start_case_iterator(
    names: Rc<Vec<String>>,
    mut request_stream: ftest::CaseIteratorRequestStream,
) -> Result<(), anyhow::Error> {
    let cases = names
        .iter()
        .map(|name| ftest::Case {
            name: Some(name.to_string()),
            enabled: Some(true),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let mut remaining_cases = &cases[..];
    while let Some(event) = request_stream.try_next().await.map_err(|e| anyhow::anyhow!(e))? {
        match event {
            ftest::CaseIteratorRequest::GetNext { responder } => {
                // Paginate cases
                // Page overhead of message header + vector
                let mut bytes_used: usize = 32;
                let mut case_count = 0;
                for case in remaining_cases {
                    bytes_used += case.measure().num_bytes;
                    if bytes_used > ZX_CHANNEL_MAX_MSG_BYTES as usize {
                        break;
                    }
                    case_count += 1;
                }
                responder.send(&remaining_cases[..case_count]).map_err(|e| anyhow::anyhow!(e))?;
                remaining_cases = &remaining_cases[case_count..];
            }
        }
    }
    Ok(())
}

async fn run_test_case_in_child(
    name: String,
    tag: Option<String>,
    launch_config: &LaunchConfig,
    options: &ftest::RunOptions,
    parent_listener: &ftest::RunListenerProxy,
) -> Result<bool, anyhow::Error> {
    // Connect to fidl_fuchsia_component::RealmMarker
    let realm = connect_to_protocol::<fidl_fuchsia_component::RealmMarker>()
        .context("connecting to Realm protocol")?;

    // It's natural to give the child component the same name as the test case.
    // However, gtest likes to give parameterized test cases names with
    // slashes, which are illegal characters for a component name. Accordingly,
    // we replace those with, say, underscores.
    let component_name = name.replace("/", "_");

    // Create a new child declaration.
    let child_decl = fidl_fuchsia_component_decl::Child {
        name: Some(component_name.clone()),
        url: Some(launch_config.url.clone()),
        startup: Some(fidl_fuchsia_component_decl::StartupMode::Lazy),
        ..Default::default()
    };
    // Create a child in the collection specified in launch_config
    let (child_controller, child_controller_server) = fidl::endpoints::create_proxy();
    let create_child_args = fidl_fuchsia_component::CreateChildArgs {
        controller: Some(child_controller_server),
        ..Default::default()
    };
    if let Err(e) = realm
        .create_child(
            &fidl_fuchsia_component_decl::CollectionRef {
                name: launch_config.collection.to_string(),
            },
            &child_decl,
            create_child_args,
        )
        .await
    {
        anyhow::bail!(
            "Could not create child component in collection {}: {e:?}",
            launch_config.collection,
        );
    }
    // Open fuchsia.test.Suite protocol via realm.open_exposed_dir on the child
    let (child_exposed_dir, child_exposed_dir_server) = fidl::endpoints::create_proxy();
    if let Err(e) = realm
        .open_exposed_dir(
            &fidl_fuchsia_component_decl::ChildRef {
                name: component_name.clone(),
                collection: Some(launch_config.collection.to_string()),
            },
            child_exposed_dir_server,
        )
        .await
    {
        anyhow::bail!("Could not open exposed dir on child {component_name}: {e:?}");
    }
    let child_test_protocol =
        connect_to_protocol_at_dir_root::<ftest::SuiteMarker>(&child_exposed_dir)
            .context("connecting to fuchsia.test.Suite protocol on child")?;
    // Run test case
    let child_invocation = vec![ftest::Invocation {
        name: Some("main".to_string()),
        tag: tag.clone(),
        ..Default::default()
    }];
    let child_options = ftest::RunOptions {
        include_disabled_tests: options.include_disabled_tests,
        parallel: options.parallel,
        arguments: Some(launch_config.args.clone()),
        break_on_failure: options.break_on_failure,
        ..Default::default()
    };
    let (run_listener_client, mut run_listener) =
        fidl::endpoints::create_request_stream::<ftest::RunListenerMarker>();
    child_test_protocol.run(&child_invocation, &child_options, run_listener_client)?;
    let mut test_started = false;
    while let Some(result_event) = run_listener.try_next().await? {
        match result_event {
            ftest::RunListenerRequest::OnTestCaseStarted {
                mut invocation,
                std_handles,
                listener,
                control_handle: _,
            } => {
                // The reported invocation from the child will not have the correct
                // name so override it before reporting to the parent.
                invocation.name = Some(name.clone());
                parent_listener.on_test_case_started(&invocation, std_handles, listener)?;
            }
            ftest::RunListenerRequest::OnFinished { control_handle: _ } => {
                test_started = true;
                // This child is done reporting tests. We can't report OnFinished to the parent
                // listener until all children have reported their results.
                break;
            }
        }
    }
    std::mem::drop(child_controller);
    // If the run listener closed without reporting that the test we asked it to run
    // started then something went wrong. Let the caller know.
    Ok(test_started)
}
