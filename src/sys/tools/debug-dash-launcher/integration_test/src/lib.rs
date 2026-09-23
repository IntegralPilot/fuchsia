// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_dash as fdash;
use fuchsia_component::client::connect_to_protocol;
use futures::StreamExt;

#[fuchsia::test]
pub async fn unknown_tools_package() {
    let (_stdio, stdio_server) = zx::Socket::create_stream();

    let launcher = connect_to_protocol::<fdash::LauncherMarker>().unwrap();

    let urls = &["fuchsia-pkg://fuchsia.com/bar".to_string()];
    let result = launcher
        .explore_component_over_socket(
            ".",
            stdio_server,
            urls,
            None,
            fdash::DashNamespaceLayout::NestAllInstanceDirs,
        )
        .await
        .unwrap();

    assert!(result.is_ok());

    let mut buf = [0u8; 1024];
    let bytes_read = _stdio.read(&mut buf).unwrap();
    let output = std::str::from_utf8(&buf[..bytes_read]).unwrap();

    assert!(output.contains("Failed to load at least one tool package"));
    assert!(output.contains("fuchsia-pkg://fuchsia.com/bar"));
}

#[fuchsia::test]
pub async fn bad_moniker() {
    let (_stdio, stdio_server) = zx::Socket::create_stream();

    let launcher = connect_to_protocol::<fdash::LauncherMarker>().unwrap();

    // Give a string that won't parse correctly as a moniker.
    let err = launcher
        .explore_component_over_socket(
            "!@#$%^&*(",
            stdio_server,
            &[],
            None,
            fdash::DashNamespaceLayout::NestAllInstanceDirs,
        )
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err, fdash::LauncherError::BadMoniker);
}

#[fuchsia::test]
pub async fn instance_not_found() {
    let (_stdio, stdio_server) = zx::Socket::create_stream();

    let launcher = connect_to_protocol::<fdash::LauncherMarker>().unwrap();

    // Give a moniker to an instance that does not exist.
    let err = launcher
        .explore_component_over_socket(
            "./does_not_exist",
            stdio_server,
            &[],
            None,
            fdash::DashNamespaceLayout::NestAllInstanceDirs,
        )
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err, fdash::LauncherError::InstanceNotFound);
}

#[fuchsia::test]
pub async fn bad_url() {
    let (_stdio, stdio_server) = zx::Socket::create_stream();

    let launcher = connect_to_protocol::<fdash::LauncherMarker>().unwrap();

    let urls = &["#".to_string()];
    let result = launcher
        .explore_component_over_socket(
            ".",
            stdio_server,
            urls,
            None,
            fdash::DashNamespaceLayout::NestAllInstanceDirs,
        )
        .await
        .unwrap();

    assert!(result.is_ok());

    let mut buf = [0u8; 1024];
    let bytes_read = _stdio.read(&mut buf).unwrap();
    let output = std::str::from_utf8(&buf[..bytes_read]).unwrap();

    assert!(output.contains("Failed to load at least one tool package"));
    assert!(output.contains("while parsing tool url #"));
}

#[fuchsia::test]
pub async fn exit_with_no_error() {
    let (_stdio, stdio_server) = zx::Socket::create_stream();

    let launcher = connect_to_protocol::<fdash::LauncherMarker>().unwrap();

    let result = launcher
        .explore_component_over_socket(
            ".",
            stdio_server,
            &[],
            None, // Interactive
            fdash::DashNamespaceLayout::NestAllInstanceDirs,
        )
        .await
        .unwrap();

    assert!(result.is_ok());

    // Send exit.
    _stdio.write(b"exit\n").unwrap();

    // Wait for the socket to close.
    match _stdio.wait_one(zx::Signals::SOCKET_PEER_CLOSED, zx::MonotonicInstant::INFINITE) {
        zx::WaitResult::Ok(_) => {}
        zx::WaitResult::TimedOut(_) => {}
        zx::WaitResult::Canceled(_) => {}
        zx::WaitResult::Err(status) => panic!("Wait failed: {:?}", status),
    }

    // Read any remaining data.
    let mut output = String::new();
    let mut buf = [0u8; 1024];
    while let Ok(bytes_read) = _stdio.read(&mut buf) {
        if bytes_read == 0 {
            break;
        }
        output.push_str(std::str::from_utf8(&buf[..bytes_read]).unwrap());
    }

    // Verify there are no error logs in the output (ignoring the expected tools package resolution warning).
    let expected_warning = "Failed to load at least one tool package: while resolving tool package fuchsia-pkg://fuchsia.com/debug-dash-launcher-test: fuchsia.pkg/PackageResolver application error: PackageNotFound";
    let cleaned_output = output.replace(expected_warning, "");
    let lower_output = cleaned_output.to_lowercase();
    assert!(!lower_output.contains("error"), "Output contained 'error': {}", output);
    assert!(!lower_output.contains("fail"), "Output contained 'fail': {}", output);

    // Verify the launcher reports a clean exit (return code 0).
    let mut event_stream = launcher.take_event_stream();
    match event_stream.next().await {
        Some(Ok(fdash::LauncherEvent::OnTerminated { return_code })) => {
            assert_eq!(return_code, 0);
        }
        other => {
            panic!("Expected OnTerminated event with return code, got {:?}", other);
        }
    }
}

#[fuchsia::test]
pub async fn background_job() {
    let (_stdio, stdio_server) = zx::Socket::create_stream();

    let launcher = connect_to_protocol::<fdash::LauncherMarker>().unwrap();

    let result = launcher
        .explore_component_over_socket(
            ".",
            stdio_server,
            &[],
            Some("true &"),
            fdash::DashNamespaceLayout::NestAllInstanceDirs,
        )
        .await
        .unwrap();

    assert!(result.is_ok());

    let mut output = String::new();
    loop {
        match _stdio.wait_one(
            zx::Signals::SOCKET_READABLE | zx::Signals::SOCKET_PEER_CLOSED,
            zx::MonotonicInstant::INFINITE,
        ) {
            zx::WaitResult::Ok(signals) => {
                if signals.contains(zx::Signals::SOCKET_READABLE) {
                    let mut buf = [0u8; 1024];
                    match _stdio.read(&mut buf) {
                        Ok(bytes_read) => {
                            if bytes_read == 0 {
                                break;
                            }
                            output.push_str(std::str::from_utf8(&buf[..bytes_read]).unwrap());
                        }
                        Err(zx::Status::SHOULD_WAIT) => {
                            continue;
                        }
                        Err(zx::Status::PEER_CLOSED) => {
                            break;
                        }
                        Err(e) => {
                            panic!("Socket read failed: {:?}", e);
                        }
                    }
                }
                if signals.contains(zx::Signals::SOCKET_PEER_CLOSED) {
                    // Read any remaining data.
                    let mut buf = [0u8; 1024];
                    while let Ok(bytes_read) = _stdio.read(&mut buf) {
                        if bytes_read == 0 {
                            break;
                        }
                        output.push_str(std::str::from_utf8(&buf[..bytes_read]).unwrap());
                    }
                    break;
                }
            }
            zx::WaitResult::TimedOut(_) => {
                break;
            }
            zx::WaitResult::Canceled(_) => {
                break;
            }
            zx::WaitResult::Err(status) => {
                panic!("Wait failed: {:?}", status);
            }
        }
    }

    assert!(!output.contains("Failed to create subshell"), "Output was: {}", output);
}

#[fuchsia::test]
pub async fn disconnect_kills_jobs() {
    let (_stdio, stdio_server) = zx::Socket::create_stream();

    let launcher = connect_to_protocol::<fdash::LauncherMarker>().unwrap();

    let result = launcher
        .explore_component_over_socket(
            ".",
            stdio_server,
            &[],
            None, // Interactive
            fdash::DashNamespaceLayout::NestAllInstanceDirs,
        )
        .await
        .unwrap();

    assert!(result.is_ok());

    // Drop the launcher proxy to simulate client disconnect.
    std::mem::drop(launcher);

    // Wait for the socket to close. We expect this to happen because the launcher
    // should kill the job (and thus the shell process) when the client disconnects.
    match _stdio.wait_one(zx::Signals::SOCKET_PEER_CLOSED, zx::MonotonicInstant::INFINITE) {
        zx::WaitResult::Ok(_) => {} // Success, socket closed.
        zx::WaitResult::TimedOut(_) => unreachable!(),
        zx::WaitResult::Canceled(_) => panic!("Wait canceled"),
        zx::WaitResult::Err(status) => panic!("Wait failed: {:?}", status),
    }
}
