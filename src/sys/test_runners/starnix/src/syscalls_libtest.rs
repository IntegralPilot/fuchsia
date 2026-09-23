// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::debian_guest::DebianGuest;
use crate::helpers::{self, clone_start_info};
use anyhow::{Context, Error, anyhow, bail};
use cm_types::NamespacePath;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_test as ftest;
use fuchsia_async as fasync;
use futures::AsyncReadExt;
use std::collections::HashSet;
use std::sync::Arc;

const CML_TARGET_KERNEL_FIELD: &str = "test_target_kernel";

/// Enumerates libtest test cases by dry-running the binary with `--list` and `--list --ignored`.
pub async fn get_cases_list_for_libtests(
    mut start_info: frunner::ComponentStartInfo,
    component_runner: &frunner::ComponentRunnerProxy,
) -> Result<Vec<ftest::Case>, Error> {
    let all_tests = run_list_command(&mut start_info, component_runner, false).await?;
    let disabled_tests = run_list_command(&mut start_info, component_runner, true)
        .await?
        .into_iter()
        .collect::<HashSet<_>>();

    Ok(all_tests
        .into_iter()
        .map(|name| {
            let enabled = !disabled_tests.contains(&name);
            ftest::Case { name: Some(name), enabled: Some(enabled), ..Default::default() }
        })
        .collect())
}

async fn run_list_command(
    start_info: &mut frunner::ComponentStartInfo,
    component_runner: &frunner::ComponentRunnerProxy,
    ignored_only: bool,
) -> Result<Vec<String>, Error> {
    let mut cloned_info = clone_start_info(start_info)?;
    let mut args = vec!["--list".to_string()];
    if ignored_only {
        args.push("--ignored".to_string());
    }

    let (numbered_handles, std_handles) = helpers::create_numbered_handles();
    cloned_info.numbered_handles = Some(numbered_handles);
    helpers::replace_program_args(args, cloned_info.program.as_mut().context("No program")?);

    let stdout_sink = std_handles.out.unwrap();
    let mut async_stdout = fasync::Socket::from_socket(stdout_sink);

    let controller = helpers::start_test_component(cloned_info, component_runner)?;

    let mut output_bytes = Vec::new();
    let (status, read_res) = futures::join!(
        helpers::read_result(controller.take_event_stream()),
        async_stdout.read_to_end(&mut output_bytes),
    );
    read_res?;

    if status.status != Some(ftest::Status::Passed) {
        bail!("Failed to list tests: {:?}", status.status);
    }

    let output_str = String::from_utf8_lossy(&output_bytes);
    let mut test_names = Vec::new();
    for line in output_str.lines() {
        let trimmed = line.trim();
        if let Some(test_name) = trimmed.strip_suffix(": test") {
            test_names.push(test_name.to_string());
        }
    }
    Ok(test_names)
}

/// Executes libtest syscall tests on either the Machina Debian guest or Starnix kernel.
pub async fn run_syscall_libtests(
    tests: Vec<ftest::Invocation>,
    mut test_start_info: frunner::ComponentStartInfo,
    run_listener_proxy: &ftest::RunListenerProxy,
    component_runner: &frunner::ComponentRunnerProxy,
    debian_guest: Arc<DebianGuest>,
    options: ftest::RunOptions,
) -> Result<(), Error> {
    let program_data = test_start_info.program.as_mut().context("Missing program")?;
    let test_target_kernel =
        helpers::take_opt_str_value_from_dict(program_data, CML_TARGET_KERNEL_FIELD)?;

    match test_target_kernel.as_deref() {
        Some("linux") => {
            log::info!("Executing libtest syscall tests on Machina Debian guest.");
            run_libtests_on_debian_guest(
                tests,
                test_start_info,
                run_listener_proxy,
                debian_guest,
                options,
            )
            .await
        }
        None => {
            log::info!("Executing libtest syscall tests on Starnix kernel.");
            run_libtests_on_starnix(
                tests,
                test_start_info,
                run_listener_proxy,
                component_runner,
                options,
            )
            .await
        }
        Some(unexpected_target) => {
            bail!("Unexpected {} specified: {}", CML_TARGET_KERNEL_FIELD, unexpected_target)
        }
    }
}

/// Executes individual libtest tests inside Machina with isolated per-test stdout/stderr sockets.
async fn run_libtests_on_debian_guest(
    tests: Vec<ftest::Invocation>,
    mut test_start_info: frunner::ComponentStartInfo,
    run_listener_proxy: &ftest::RunListenerProxy,
    debian_guest: Arc<DebianGuest>,
    options: ftest::RunOptions,
) -> Result<(), Error> {
    let mut test_component_ns = namespace::Namespace::try_from(test_start_info.ns.take().unwrap())?;
    test_start_info.ns = Some(test_component_ns.clone().try_into()?);
    let test_pkg_dir = test_component_ns
        .remove(&NamespacePath::new("/pkg")?)
        .ok_or_else(|| anyhow!("Could not find /pkg in namespace!"))?
        .into_proxy();

    let data_mount = helpers::get_syscall_data_mount(&test_pkg_dir).await?;
    debian_guest.configure_data_mount(data_mount);

    let guest_binary = helpers::get_guest_syscall_test_binary_path(&test_start_info)?;
    let test_binary_file =
        helpers::open_guest_syscall_test_binary(&test_pkg_dir, &test_start_info).await?;
    debian_guest.push_data_to_guest(test_binary_file, &guest_binary).await?;

    for test in tests {
        let test_name = test.name.clone().ok_or_else(|| anyhow!("Invocation missing test name"))?;
        let (case_listener_proxy, case_listener) =
            fidl::endpoints::create_proxy::<ftest::CaseListenerMarker>();

        let (test_stdout, stdout_client) = zx::Socket::create_stream();
        let (test_stderr, stderr_client) = zx::Socket::create_stream();
        let std_handles = ftest::StdHandles {
            out: Some(stdout_client),
            err: Some(stderr_client),
            ..Default::default()
        };

        run_listener_proxy.on_test_case_started(&test, std_handles, case_listener)?;

        // Execute specific test case with --exact and --nocapture
        let mut extra_args = String::new();
        if options.include_disabled_tests.unwrap_or(false) {
            extra_args.push_str(" --include-ignored");
        }
        if let Some(user_args) = &options.arguments {
            for arg in user_args {
                extra_args.push(' ');
                extra_args.push_str(arg);
            }
        }
        let command =
            format!("{} {} --exact --nocapture{}", guest_binary.display(), test_name, extra_args);
        let exec_result =
            debian_guest.execute(&command, &[], None, Some(test_stdout), Some(test_stderr)).await;

        let status = match exec_result {
            Ok(0) => ftest::Status::Passed,
            Ok(return_code) => {
                log::warn!("Test {} failed on guest with exit code {}", test_name, return_code);
                ftest::Status::Failed
            }
            Err(e) => {
                log::warn!("Test {} failed on guest: {:?}", test_name, e);
                ftest::Status::Failed
            }
        };

        case_listener_proxy
            .finished(&ftest::Result_ { status: Some(status), ..Default::default() })?;
    }

    Ok(())
}

/// Executes individual libtest tests on Starnix kernel.
async fn run_libtests_on_starnix(
    tests: Vec<ftest::Invocation>,
    mut test_start_info: frunner::ComponentStartInfo,
    run_listener_proxy: &ftest::RunListenerProxy,
    component_runner: &frunner::ComponentRunnerProxy,
    options: ftest::RunOptions,
) -> Result<(), Error> {
    for test in tests {
        let test_name = test.name.clone().ok_or_else(|| anyhow!("Invocation missing test name"))?;
        let (case_listener_proxy, case_listener) =
            fidl::endpoints::create_proxy::<ftest::CaseListenerMarker>();

        let (numbered_handles, std_handles) = helpers::create_numbered_handles();
        let mut start_info = clone_start_info(&mut test_start_info)?;
        start_info.numbered_handles = Some(numbered_handles);

        let mut args = vec![test_name, "--exact".to_string(), "--nocapture".to_string()];
        if options.include_disabled_tests.unwrap_or(false) {
            args.push("--include-ignored".to_string());
        }
        if let Some(user_args) = &options.arguments {
            args.extend(user_args.clone());
        }

        helpers::replace_program_args(args, start_info.program.as_mut().unwrap());

        run_listener_proxy.on_test_case_started(&test, std_handles, case_listener)?;
        let component_controller = helpers::start_test_component(start_info, component_runner)?;
        let result = helpers::read_result(component_controller.take_event_stream()).await;
        case_listener_proxy.finished(&result)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_data as fdata;
    use fuchsia_async as fasync;
    use futures::{AsyncWriteExt, TryStreamExt};

    fn spawn_mock_runner_with_filter() -> (frunner::ComponentRunnerProxy, fasync::Task<()>) {
        let (proxy, mut request_stream) =
            create_proxy_and_stream::<frunner::ComponentRunnerMarker>();
        let task = fasync::Task::local(async move {
            while let Some(event) =
                request_stream.try_next().await.expect("Error in test runner request stream")
            {
                match event {
                    frunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                        let mut is_ignored = false;
                        if let Some(entries) = start_info.program.and_then(|p| p.entries) {
                            for entry in entries {
                                if entry.key == "args" {
                                    if let Some(fdata::DictionaryValue::StrVec(args)) =
                                        entry.value.map(|b| *b)
                                    {
                                        if args.contains(&"--ignored".to_string()) {
                                            is_ignored = true;
                                        }
                                    }
                                }
                            }
                        }
                        let list_output = if is_ignored {
                            "foo::test_b: test\n"
                        } else {
                            "foo::test_a: test\nfoo::test_b: test\n"
                        };

                        if let Some(handles) = start_info.numbered_handles {
                            for handle_info in handles {
                                if handle_info.id
                                    == fuchsia_runtime::HandleInfo::new(
                                        fuchsia_runtime::HandleType::FileDescriptor,
                                        1,
                                    )
                                    .as_raw()
                                {
                                    let socket = zx::Socket::from(handle_info.handle);
                                    let mut async_socket = fasync::Socket::from_socket(socket);
                                    let _ = async_socket.write_all(list_output.as_bytes()).await;
                                }
                            }
                        }
                        controller
                            .close_with_epitaph(Ok(()))
                            .expect("Could not close with epitaph");
                    }
                    frunner::ComponentRunnerRequest::_UnknownMethod { ordinal, .. } => {
                        log::warn!(ordinal:%; "Unknown ComponentRunner request");
                    }
                }
            }
        });
        (proxy, task)
    }

    #[fuchsia::test]
    async fn test_get_cases_list_for_libtests() {
        let (runner, _task) = spawn_mock_runner_with_filter();
        let start_info = frunner::ComponentStartInfo {
            program: Some(fdata::Dictionary { entries: Some(vec![]), ..Default::default() }),
            ns: Some(vec![]),
            ..Default::default()
        };

        let cases = get_cases_list_for_libtests(start_info, &runner).await.unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].name.as_deref(), Some("foo::test_a"));
        assert_eq!(cases[0].enabled, Some(true));
        assert_eq!(cases[1].name.as_deref(), Some("foo::test_b"));
        assert_eq!(cases[1].enabled, Some(false));
    }

    #[fuchsia::test]
    async fn test_get_cases_list_for_libtests_error_on_disabled_tests() {
        let (proxy, mut request_stream) =
            create_proxy_and_stream::<frunner::ComponentRunnerMarker>();
        let _task = fasync::Task::local(async move {
            if let Some(event) =
                request_stream.try_next().await.expect("Error in test runner request stream")
            {
                match event {
                    frunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                        let list_output = "foo::test_a: test\n";
                        if let Some(handles) = start_info.numbered_handles {
                            for handle_info in handles {
                                if handle_info.id
                                    == fuchsia_runtime::HandleInfo::new(
                                        fuchsia_runtime::HandleType::FileDescriptor,
                                        1,
                                    )
                                    .as_raw()
                                {
                                    let socket = zx::Socket::from(handle_info.handle);
                                    let mut async_socket = fasync::Socket::from_socket(socket);
                                    let _ = async_socket.write_all(list_output.as_bytes()).await;
                                }
                            }
                        }
                        // Drop request_stream before closing controller so the runner channel is closed
                        // for the subsequent disabled tests query.
                        drop(request_stream);
                        controller
                            .close_with_epitaph(Ok(()))
                            .expect("Could not close with epitaph");
                    }
                    _ => {}
                }
            }
        });

        let start_info = frunner::ComponentStartInfo {
            program: Some(fdata::Dictionary { entries: Some(vec![]), ..Default::default() }),
            ns: Some(vec![]),
            ..Default::default()
        };

        assert!(get_cases_list_for_libtests(start_info, &proxy).await.is_err());
    }

    #[fuchsia::test]
    async fn test_run_libtests_on_starnix_with_options() {
        let (proxy, mut request_stream) =
            create_proxy_and_stream::<frunner::ComponentRunnerMarker>();
        let _task = fasync::Task::local(async move {
            while let Some(event) =
                request_stream.try_next().await.expect("Error in test runner request stream")
            {
                match event {
                    frunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                        let entries = start_info.program.unwrap().entries.unwrap();
                        let args_entry = entries.into_iter().find(|e| e.key == "args").unwrap();
                        let fdata::DictionaryValue::StrVec(args) = *args_entry.value.unwrap()
                        else {
                            panic!("expected StrVec");
                        };
                        assert_eq!(
                            args,
                            vec![
                                "foo::test_a".to_string(),
                                "--exact".to_string(),
                                "--nocapture".to_string(),
                                "--include-ignored".to_string(),
                                "custom_arg".to_string(),
                            ]
                        );
                        controller
                            .close_with_epitaph(Ok(()))
                            .expect("Could not close with epitaph");
                    }
                    _ => {}
                }
            }
        });

        let (run_listener_proxy, mut run_listener_stream) =
            create_proxy_and_stream::<ftest::RunListenerMarker>();
        let _listener_task = fasync::Task::local(async move {
            while let Some(event) = run_listener_stream.try_next().await.unwrap() {
                match event {
                    ftest::RunListenerRequest::OnTestCaseStarted { listener, .. } => {
                        let mut stream = listener.into_stream();
                        while let Some(req) = stream.try_next().await.unwrap() {
                            match req {
                                ftest::CaseListenerRequest::Finished { .. } => {}
                            }
                        }
                    }
                    ftest::RunListenerRequest::OnFinished { .. } => {}
                }
            }
        });

        let start_info = frunner::ComponentStartInfo {
            program: Some(fdata::Dictionary { entries: Some(vec![]), ..Default::default() }),
            ns: Some(vec![]),
            ..Default::default()
        };

        let tests =
            vec![ftest::Invocation { name: Some("foo::test_a".to_string()), ..Default::default() }];
        let options = ftest::RunOptions {
            include_disabled_tests: Some(true),
            arguments: Some(vec!["custom_arg".to_string()]),
            ..Default::default()
        };

        run_libtests_on_starnix(tests, start_info, &run_listener_proxy, &proxy, options)
            .await
            .unwrap();
    }
}
