// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use signal_hook::consts::signal::{SIGINT, SIGQUIT, SIGTERM};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use itertools::Itertools;

// Time to wait for antlion to cleanup after receiving a termination signal. If
// the process is unable to terminate within this time, the process will be
// killed without further warning.
const TERM_TIMEOUT_SEC: u64 = 3;

// Busy-wait sleep duration between polling for antlion termination.
const TERM_CHECK_INTERVAL_MS: u64 = 100;

/// Runner for dispatching antlion.
pub(crate) trait Runner {
    /// Run antlion using the provided config and output directory.
    fn run(&self, config: PathBuf) -> Result<ExitStatus>;
}

/// Executes antlion as a local process.
pub(crate) struct ProcessRunner {
    pub python_bin: String,
    pub antlion_pyz: PathBuf,
    pub test_cases: Vec<String>,
}

// TODO(http://b/401318909): Remove this once Fuchsia Controller no longer panics during teardown.
fn test_affected_by_b_401318909(test_name: String) -> bool {
    let test_substrings_affected_by_b_401318909 = [
        "beacon_loss_test",
        "channel_switch_test",
        "connection_stress_test",
        "deprecated_configuration_test",
        "dhcpv4_duplicate_address_test",
        "dhcpv4_interop_basic_test",
        "dhcpv4_interop_combinatorial_options_test",
        "dhcpv4_interop_fixture_test",
        "hidden_networks_test",
        "ping_test",
        "policy_scan_test",
        "regulatory_compliance_test",
        "saved_networks_test",
        "vape_interop_test",
        "wlan_driver_restart_test",
        "wlan_misc_scenario",
        "wlan_phy_compliance_",
        "wlan_policy_initiated_roam_test",
        "wlan_reboot_",
        "wlan_rvr_test_",
        "wlan_scan_test",
        "wlan_security_compliance_",
        "wlan_target_security_test",
        "wlan_wireless_network_management_test",
        "wlan_wmm_test",
    ];

    for test in test_substrings_affected_by_b_401318909 {
        if test_name.contains(test) {
            return true;
        }
    }

    false
}

impl Runner for ProcessRunner {
    fn run(&self, config: PathBuf) -> Result<ExitStatus> {
        let mut args = vec![
            self.antlion_pyz.clone().into_os_string().into_string().unwrap(),
            "--config".to_string(),
            config.into_os_string().into_string().unwrap(),
        ];

        if !self.test_cases.is_empty() {
            args.push("--test_case".to_string());
            for test_case in self.test_cases.iter() {
                args.push(test_case.clone());
            }
        }

        println!("Launching antlion to run: \"{} {}\"\n", self.python_bin, args.iter().format(" "),);

        let mut child =
            Command::new(&self.python_bin).args(args).spawn().context("Failed to spawn antlion")?;

        // Start monitoring for termination signals.
        let term = Arc::new(AtomicUsize::new(0));
        signal_hook::flag::register_usize(SIGINT, term.clone(), SIGINT as usize)?;
        signal_hook::flag::register_usize(SIGTERM, term.clone(), SIGTERM as usize)?;
        signal_hook::flag::register_usize(SIGQUIT, term.clone(), SIGQUIT as usize)?;

        loop {
            if let Some(exit_status) =
                child.try_wait().context("Failed waiting for antlion to finish")?
            {
                if exit_status.core_dumped() {
                    if test_affected_by_b_401318909(
                        self.antlion_pyz.clone().into_os_string().into_string().unwrap(),
                    ) {
                        eprintln!(
                            "Received expected core dump after running test. \
                            Remove this once http://b/401318909 has been resolved."
                        );
                        return Ok(ExitStatus::Ok);
                    } else {
                        bail!(
                            "Got core dump after running test, but this test isn't in core-dump \
                            exception list. Core dump bug: http://b/401318909."
                        );
                    }
                }

                return Ok(ExitStatus::from(exit_status));
            }

            let signal = term.load(Ordering::Relaxed) as i32;
            if signal != 0 {
                println!("Forwarding signal {signal} to antlion");
                nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(
                        child.id().try_into().context("Failed to convert pid to i32")?,
                    ),
                    Some(signal.try_into().context("Failed to convert signal")?),
                )
                .context("Failed to forward signal to antlion")?;

                println!("Waiting {} seconds for antlion to terminate", TERM_TIMEOUT_SEC);
                let timeout = Instant::now() + Duration::from_secs(TERM_TIMEOUT_SEC);
                while Instant::now() < timeout {
                    if let Some(_) =
                        child.try_wait().context("Failed waiting for antlion to finish")?
                    {
                        return Ok(ExitStatus::Interrupt(Some(signal)));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(TERM_CHECK_INTERVAL_MS));
                }

                eprintln!("antlion is unresponsive, killing process");
                child.kill().context("Failed to kill antlion process")?;
                return Ok(ExitStatus::Interrupt(Some(signal)));
            }

            std::thread::sleep(std::time::Duration::from_millis(TERM_CHECK_INTERVAL_MS));
        }
    }
}

/// Describes the result of a child process after it has terminated.
pub(crate) enum ExitStatus {
    /// Process terminated without error.
    Ok,
    /// Process terminated with a non-zero status code.
    Err(i32),
    /// Process was interrupted by a signal.
    Interrupt(Option<i32>),
}

impl From<std::process::ExitStatus> for ExitStatus {
    fn from(status: std::process::ExitStatus) -> Self {
        match status.code() {
            Some(0) => ExitStatus::Ok,
            Some(code) => ExitStatus::Err(code),
            None if cfg!(target_os = "unix") => ExitStatus::Interrupt(status.signal()),
            None => ExitStatus::Interrupt(None),
        }
    }
}

impl Into<ExitCode> for ExitStatus {
    fn into(self) -> ExitCode {
        match self {
            ExitStatus::Ok => ExitCode::SUCCESS,
            ExitStatus::Err(code) => {
                let code = match u8::try_from(code) {
                    Ok(c) => c,
                    Err(_) => 1,
                };
                ExitCode::from(code)
            }
            ExitStatus::Interrupt(_) => ExitCode::FAILURE,
        }
    }
}
