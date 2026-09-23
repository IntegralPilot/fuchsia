// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashSet;

use fidl_fuchsia_component_resolution as fresolution;
use fidl_fuchsia_developer_console as fconsole;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fprocess;
use fuchsia_async as fasync;
use futures::io::BufReader;
use futures::{
    AsyncBufReadExt as _, AsyncReadExt, AsyncWriteExt as _, StreamExt as _, TryStreamExt as _,
};
use test_case::test_case;

fn connect_launcher() -> fconsole::LauncherProxy {
    fuchsia_component::client::connect_to_protocol::<fconsole::LauncherMarker>()
        .expect("connect to launcher")
}

fn launch(options: fconsole::LaunchOptions) -> fasync::Task<i64> {
    let launcher = connect_launcher();
    fasync::Task::spawn(async move {
        launcher.launch(options).await.expect("fidl error").expect("launch failed")
    })
}

async fn get_package() -> fresolution::Package {
    fuchsia_component::client::realm()
        .expect("connect to realm")
        .get_resolved_info()
        .await
        .expect("calling get resolved info")
        .expect("get resolve info")
        .package
        .expect("missing package")
}

#[derive(Debug, Copy, Clone)]
enum IoHandles {
    Raw,
    Pty,
}

impl IoHandles {
    fn into_resources(self) -> (fasync::Socket, fconsole::IoHandles) {
        let (socket, server) = zx::Socket::create_stream();
        let socket = fasync::Socket::from_socket(socket);
        let io = match self {
            IoHandles::Raw => {
                let stdout = server;
                let stderr =
                    stdout.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("duplicate handle");
                let stdin =
                    stdout.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("duplicate handle");
                fconsole::IoHandles::RawHandles(fconsole::RawHandles {
                    stdin: Some(stdin.into_handle()),
                    stdout: Some(stdout.into_handle()),
                    stderr: Some(stderr.into_handle()),
                })
            }
            IoHandles::Pty => fconsole::IoHandles::PtySocket(server),
        };
        (socket, io)
    }
}

// Test that dropping the handle makes dash exit. Unfortunately we can't test
// the same thing for PTY because the PTY service doesn't do anything on server
// close.
#[fasync::run_singlethreaded(test)]
async fn raw_handles_drop_socket() {
    let (socket, io_handles) = IoHandles::Raw.into_resources();
    let task =
        launch(fconsole::LaunchOptions { io_handles: Some(io_handles), ..Default::default() });
    // Dropping the socket should make dash exit immediately.
    drop(socket);
    assert_eq!(task.await, 0);
}

#[test_case(IoHandles::Raw)]
#[test_case(IoHandles::Pty)]
#[fasync::run_singlethreaded(test)]
async fn interactive_dash_data(io: IoHandles) {
    let (socket, io_handles) = io.into_resources();
    let task =
        launch(fconsole::LaunchOptions { io_handles: Some(io_handles), ..Default::default() });
    let (reader, mut writer) = socket.split();
    writer.write_all(b"echo hello\nexit 0\n").await.unwrap();
    let lines = BufReader::new(reader).lines().try_collect::<Vec<_>>().await.expect("read lines");
    match io {
        IoHandles::Pty => {
            assert_eq!(lines, vec!["$ echo hello", "hello", "$ exit 0"]);
        }
        IoHandles::Raw => {
            assert_eq!(lines, vec!["hello"]);
        }
    }

    assert_eq!(task.await, 0);
}

#[fasync::run_singlethreaded(test)]
async fn exit_code() {
    let (mut socket, io_handles) = IoHandles::Raw.into_resources();
    let task =
        launch(fconsole::LaunchOptions { io_handles: Some(io_handles), ..Default::default() });
    socket.write_all(b"exit 123\n").await.unwrap();
    assert_eq!(task.await, 123);
}

#[test_case(IoHandles::Raw)]
#[test_case(IoHandles::Pty)]
#[fasync::run_singlethreaded(test)]
async fn single_command(io: IoHandles) {
    let (socket, io_handles) = io.into_resources();
    let task = launch(fconsole::LaunchOptions {
        io_handles: Some(io_handles),
        args: Some(vec!["echo hello".to_string()]),
        ..Default::default()
    });
    let lines = BufReader::new(socket).lines().try_collect::<Vec<_>>().await.expect("read lines");
    assert_eq!(lines, vec!["hello"]);
    assert_eq!(task.await, 0);
}

#[fasync::run_singlethreaded(test)]
async fn env() {
    let (socket, io_handles) = IoHandles::Raw.into_resources();
    let task = launch(fconsole::LaunchOptions {
        io_handles: Some(io_handles),
        env: Some(vec!["FOO=bar".to_string()]),
        args: Some(vec!["echo $FOO".to_string()]),
        ..Default::default()
    });
    let lines = BufReader::new(socket).lines().try_collect::<Vec<_>>().await.expect("read lines");
    assert_eq!(lines, vec!["bar"]);
    assert_eq!(task.await, 0);
}

#[fasync::run_singlethreaded(test)]
async fn stopper() {
    let (_socket, io_handles) = IoHandles::Raw.into_resources();
    let (stopper, stopper_server) = zx::EventPair::create();
    let task = launch(fconsole::LaunchOptions {
        io_handles: Some(io_handles),
        stopper: Some(stopper_server),
        ..Default::default()
    });
    drop(stopper);
    assert_eq!(task.await, zx::sys::ZX_TASK_RETCODE_SYSCALL_KILL);
}

#[test_case(true; "directories rewrite")]
#[test_case(false; "no directories rewrite")]
#[fasync::run_singlethreaded(test)]
async fn base_namespace(directories_fixup: bool) {
    let (socket, io_handles) = IoHandles::Raw.into_resources();
    let task = launch(fconsole::LaunchOptions {
        io_handles: Some(io_handles),
        directories_fixup: Some(directories_fixup),
        ..Default::default()
    });
    let (reader, mut writer) = socket.split();
    let mut lines = BufReader::new(reader).lines();

    writer.write_all(b"echo *\n").await.unwrap();
    let entries = lines.next().await.unwrap().unwrap();
    if directories_fixup {
        assert_eq!(entries, "config foo svc");
    } else {
        assert_eq!(entries, "directories svc");
    }

    writer.write_all(b"echo svc/*\n").await.unwrap();
    // We only check presence in the services directory because some
    // configurations inject services and we don't want to be a change detector
    // for that.
    let svc = lines.next().await.unwrap().unwrap();
    let svc = svc.split(" ").collect::<HashSet<_>>();
    assert!(svc.contains("svc/fuchsia.component.Realm"), "{svc:?}");
    assert!(svc.contains("svc/fuchsia.developer.console.Launcher"), "{svc:?}");
    assert!(svc.contains("svc/fuchsia.logger.LogSink"), "{svc:?}");

    drop((lines, writer));
    assert_eq!(task.await, 0);
}

#[fasync::run_singlethreaded(test)]
async fn program_from_package() {
    let package = get_package().await;
    let (socket, io_handles) = IoHandles::Raw.into_resources();
    let task = launch(fconsole::LaunchOptions {
        program: Some(fconsole::Program::FromPackage(fconsole::PackageProgram {
            package,
            path: "bin/developer_console_integration_test_support_bin".to_string(),
        })),
        io_handles: Some(io_handles),
        ..Default::default()
    });
    assert_eq!(task.await, 0);
    assert_eq!(
        BufReader::new(socket).lines().try_collect::<Vec<_>>().await.expect("read lines"),
        vec!["hello world"]
    );
}

#[fasync::run_singlethreaded(test)]
async fn extra_namespace() {
    let scope = vfs::ExecutionScope::new();

    let simple1 = vfs::pseudo_directory!(
        "a" => vfs::pseudo_directory!(),
        "b" => vfs::pseudo_directory!(),
    );
    let simple2 = vfs::pseudo_directory!(
        "c" => vfs::pseudo_directory!(),
        "d" => vfs::pseudo_directory!(),
    );

    let (dir1, server_end) = fidl::endpoints::create_endpoints();
    vfs::directory::serve_on(simple1, fio::PERM_READABLE, scope.clone(), server_end);
    let (dir2, server_end) = fidl::endpoints::create_endpoints();
    vfs::directory::serve_on(simple2, fio::PERM_READABLE, scope.clone(), server_end);

    let (socket, io_handles) = IoHandles::Raw.into_resources();
    let task = launch(fconsole::LaunchOptions {
        io_handles: Some(io_handles),
        namespace_entries: Some(vec![
            fprocess::NameInfo { path: "/dir1".to_string(), directory: dir1 },
            fprocess::NameInfo { path: "/dir2".to_string(), directory: dir2 },
        ]),
        ..Default::default()
    });

    let (reader, mut writer) = socket.split();
    let mut lines = BufReader::new(reader).lines();

    writer.write_all(b"echo dir1/*\n").await.unwrap();
    assert_eq!(lines.next().await.unwrap().unwrap(), "dir1/a dir1/b");

    writer.write_all(b"echo dir2/*\n").await.unwrap();
    assert_eq!(lines.next().await.unwrap().unwrap(), "dir2/c dir2/d");

    drop((writer, lines));
    assert_eq!(task.await, 0);
}

#[test_case(""; "empty")]
#[test_case(".")]
#[test_case("foo")]
#[fasync::run_singlethreaded(test)]
async fn invalid_namespace_path(invalid: &str) {
    let launcher = connect_launcher();
    let (directory, _server_end) = fidl::endpoints::create_endpoints();
    let result = launcher
        .launch(fconsole::LaunchOptions {
            namespace_entries: Some(vec![fprocess::NameInfo {
                path: invalid.to_string(),
                directory,
            }]),
            ..Default::default()
        })
        .await
        .expect("calling launch");
    assert_eq!(result, Err(fconsole::LauncherError::InvalidNamespacePath));
}

#[fasync::run_singlethreaded(test)]
async fn duplicate_namespace_path() {
    let launcher = connect_launcher();
    let (dir1, _server_end) = fidl::endpoints::create_endpoints();
    let (dir2, _server_end) = fidl::endpoints::create_endpoints();
    let path = "/my_dir";
    let result = launcher
        .launch(fconsole::LaunchOptions {
            namespace_entries: Some(vec![
                fprocess::NameInfo { path: path.to_string(), directory: dir1 },
                fprocess::NameInfo { path: path.to_string(), directory: dir2 },
            ]),
            ..Default::default()
        })
        .await
        .expect("calling launch");
    assert_eq!(result, Err(fconsole::LauncherError::DuplicateNamespacePath));
}

#[fasync::run_singlethreaded(test)]
async fn program_load_failed() {
    let launcher = connect_launcher();

    let package = get_package().await;
    let result = launcher
        .launch(fconsole::LaunchOptions {
            program: Some(fconsole::Program::FromPackage(fconsole::PackageProgram {
                path: "bin/foo".to_string(),
                package,
            })),
            ..Default::default()
        })
        .await
        .expect("calling launch");
    assert_eq!(result, Err(fconsole::LauncherError::ProgramLoadFailed));

    let (directory, _) = fidl::endpoints::create_endpoints();
    let result = launcher
        .launch(fconsole::LaunchOptions {
            program: Some(fconsole::Program::FromPackage(fconsole::PackageProgram {
                path: "bin/foo".to_string(),
                package: fresolution::Package { directory: Some(directory), ..Default::default() },
            })),
            ..Default::default()
        })
        .await
        .expect("calling launch");
    assert_eq!(result, Err(fconsole::LauncherError::ProgramLoadFailed));
}
