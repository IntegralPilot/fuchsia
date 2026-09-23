// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_config::EnvironmentContext;
use ffx_config::logging::LogDirHandling;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(thiserror::Error, Debug)]
pub enum DaemonizeError {
    #[error("Config error: {0}")]
    Config(#[from] ffx_config::environment::ContextError),

    #[error("Failed to spawn daemon: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("Failed to wait for daemon: {0}")]
    Wait(#[source] std::io::Error),

    #[error("Logging error: {0}")]
    Logging(#[from] ffx_config::logging::LoggingError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Daemonize the given command. This is used for long running tools like the daemon
///  and package serving. The args are the command line arguments, not including ffx, and any
/// `--config` or `--env` options. The ffx path and isolation root, if any, are taken from
///  the current invocation.
#[allow(clippy::unused_async)] // TODO(https://fxbug.dev/386387845)
pub async fn daemonize(
    args: &[String],
    log_basename: String,
    context: EnvironmentContext,
    keep_current_dir: bool,
) -> Result<(), DaemonizeError> {
    let mut cmd = context.rerun_prefix()?;

    let mut stdout = Stdio::null();
    let mut stderr = Stdio::null();

    if ffx_config::logging::is_enabled(&context) {
        let file = PathBuf::from(format!("{log_basename}.log"));
        stdout = Stdio::from(ffx_config::logging::log_file(
            &context,
            &file,
            LogDirHandling::WithDirWithRotate,
        )?);
        // Third argument says not to rotate the logs.  We rotated the logs once
        // for the call above, we shouldn't do it again.
        stderr = Stdio::from(ffx_config::logging::log_file(
            &context,
            &file,
            LogDirHandling::WithDirWithoutRotate,
        )?);
    }

    cmd.stdin(Stdio::null()).stdout(stdout).stderr(stderr).env("RUST_BACKTRACE", "full");
    cmd.args(args);

    log::info!("Starting new background process {:?} {:?}", cmd.get_program(), cmd.get_args());
    // Run the command as a daemon process, keeping the current working directory
    // for the daemon process.
    let mut child =
        daemonize_cmd(&mut cmd, keep_current_dir).spawn().map_err(DaemonizeError::Spawn)?;

    child.wait().map(|_| ()).map_err(DaemonizeError::Wait)
}

/// daemonize adds a pre_exec to call daemon(3) causing the spawned
/// process to be forked again and detached from the controlling
/// terminal.
///
/// The implementation does not violate any of the constraints documented on
/// `Command::pre_exec`, and this code is expected to be safe.
///
/// This code may however cause a process hang if not used appropriately. Reading on
/// the subtleties of CLOEXEC, CLOFORK and forking multi-threaded programs will
/// provide ample background reading. For the sake of safe use, callers should work
/// to ensure that uses of `daemonize` occur early in the program lifecycle, before
/// many threads have been spawned, libraries have been used or files have been
/// opened that may introduce CLOEXEC behaviors that could cause EXTBUSY outcomes in
/// a Linux environment.
fn daemonize_cmd(c: &mut Command, keep_current_dir: bool) -> &mut Command {
    let nochdir = if keep_current_dir { 1 } else { 0 };
    unsafe {
        c.pre_exec(move || {
            // daemonize(3) is deprecated on macOS 10.15. The replacement is not
            // yet clear, we may want to replace this with a manual double fork
            // setsid, etc.
            #[allow(deprecated)]
            // First argument: chdir(/)
            // Second argument: do not close stdio (we use stdio to write to the daemon log file)
            match libc::daemon(nochdir, 1) {
                0 => Ok(()),
                x => Err(std::io::Error::from_raw_os_error(x)),
            }
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_daemonize() {
        let started = std::time::Instant::now();
        // TODO(raggi): this technically leaks a sleep process, which is
        // not ideal, but the much better approach would be a
        // significant amount of work, as we'd really want a program
        // that will wait for a signal on some other channel (such as a
        // unix socket) and otherwise linger as a daemon. If we had
        // that, we could then check the ppid and assert that daemon(3)
        // really did the work we're expecting it to. As that would
        // involve specific subprograms, finding those, and so on, it is
        // likely beyond ROI for this test coverage, which aims to just
        // prove that the immediate spawn() succeeded was detached from
        // the program in question. There is a risk that this
        // implementation passes if sleep(1) is not found, which is also
        // not ideal.
        let mut child =
            daemonize_cmd(Command::new("sleep").arg("10"), false).spawn().expect("child spawned");
        child.wait().expect("child exited successfully");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }
}
