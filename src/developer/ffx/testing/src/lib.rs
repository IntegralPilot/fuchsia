// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::bail;
use ffx_config::environment::ExecutableKind;
use ffx_config::{ConfigMap, EnvironmentContext};
use fuchsia_async as fasync;
use std::env;
use std::path::PathBuf;
use tempfile::TempDir;

mod emu;

pub use emu::Emu;

const KVM_PATH: &str = "/dev/kvm";

pub struct TestContext {
    isolate: ffx_isolate::Isolate,
    emulator_allowed: bool,
}

impl TestContext {
    pub fn isolate(&self) -> &ffx_isolate::Isolate {
        &self.isolate
    }

    pub fn emulator_allowed(&self) -> bool {
        self.emulator_allowed
    }
}

// Matches `gn` terminology (for host toolchain)
static ROOT_OUT_DIR: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
    let mut dir = std::env::current_exe().expect("get path").canonicalize().unwrap();
    assert!(dir.pop());
    dir
});

// Matches `gn` terminology
static ROOT_BUILD_DIR: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
    let mut dir = ROOT_OUT_DIR.clone();
    assert!(dir.pop());
    dir
});

// Matches `out_dir` var in `BUILD.gn`
static OUT_DIR: std::sync::LazyLock<PathBuf> =
    std::sync::LazyLock::new(|| ROOT_OUT_DIR.join("src/developer/ffx/testing"));

static TEMP_DIR: std::sync::LazyLock<TempDir> =
    std::sync::LazyLock::new(|| TempDir::new().expect("could not create test harness temp dir"));

/// Fixture that handles launching and tearing down a test after execution.
async fn fixture_inner<F, Fut>(case_name: &str, test_fn: F, emulator_allowed: bool)
where
    F: FnOnce(TestContext) -> Fut + Send + 'static,
    Fut: futures::future::Future<Output = ()>,
{
    // Start with the non-isolated environment context - then build the isolate.
    let env_context = EnvironmentContext::detect(
        ExecutableKind::Test,
        ConfigMap::new(),
        &env::current_dir().expect("current directory"),
        None,
        false,
    )
    .expect("new detected context");
    // Not actually used. ffx will generate an ssh key for usage with the emulator.
    let ssh_path = OUT_DIR.join("ssh");

    let isolate = ffx_isolate::Isolate::new_in_test(case_name, ssh_path, &env_context)
        .await
        .expect("create isolate");
    let config = TestContext { isolate, emulator_allowed };

    // Spawn a new thread so that we can catch panics from the test. To avoid blocking this thread's
    // future executor, we check completion of the test thread using a oneshot channel.
    let (done_sender, done) = futures::channel::oneshot::channel();
    let join_handle = std::thread::spawn(move || {
        let mut test_executor = fasync::LocalExecutor::default();
        test_executor.run_singlethreaded(test_fn(config));
        // The sender will notify the receiver if it is used or dropped during a panic.
        let _ = done_sender.send(());
    });
    let _ = done.await;
    // After the receiver completes we know the test is done, so we can do a blocking join
    // without issue.
    let test_result = join_handle.join();

    // Propagate the panic from the test thread to fail the test, if needed.
    match test_result {
        Ok(()) => (),
        Err(test_err) => std::panic::resume_unwind(test_err),
    }
}

/// Test fixture that handles launching and tearing down a test after execution.
pub async fn base_fixture<F, Fut>(case_name: &str, test_fn: F)
where
    F: FnOnce(TestContext) -> Fut + Send + 'static,
    Fut: futures::future::Future<Output = ()>,
{
    fixture_inner(case_name, test_fn, false).await
}

/// Test fixture that handles launching and tearing down a test after execution
/// where an emulator is expected to be used.
pub async fn emulator_fixture<F, Fut>(case_name: &str, test_fn: F)
where
    F: FnOnce(TestContext) -> Fut + Send + 'static,
    Fut: futures::future::Future<Output = ()>,
{
    assert_virtualization_available!();
    fixture_inner(case_name, test_fn, true).await
}

fn check_virtualization_available() -> anyhow::Result<()> {
    match env::consts::OS {
        "linux" => {
            let path = KVM_PATH.to_string();
            match std::fs::OpenOptions::new().write(true).open(&path) {
                Err(e) => match e.kind() {
                    std::io::ErrorKind::PermissionDenied => {
                        bail!(
                            "Emulation acceleration unavailable.\n\n\
                                        Caused by: No write permission on {}.\n\n\
                                        To adjust permissions and enable acceleration via KVM:\n\n    \
                                        sudo usermod -a -G kvm $USER\n\n\
                                        You may need to reboot your machine for the permission change \
                                        to take effect.\n",
                            path
                        );
                    }
                    std::io::ErrorKind::NotFound => {
                        bail!(
                            "KVM path {path} does not exist. \n\
                                        Test requires KVM to be enabled.\n"
                        );
                    }
                    _ => {
                        bail!("Unknown error setting up acceleration: {:?}", e);
                    }
                },
                Ok(_) => {
                    // No issues with KVM acceleration
                    Ok(())
                }
            }
        }
        "macos" => {
            // Always assume that MacOS has HVF installed
            Ok(())
        }
        other_host_os => {
            bail!("Unsupported host Operating System: {}", other_host_os);
        }
    }
}

#[macro_export]
macro_rules! assert_virtualization_available {
    () => {
        check_virtualization_available().unwrap()
    };
}
