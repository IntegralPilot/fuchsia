// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::doctor_ledger::{LedgerMode, LedgerNodeGuard, LedgerOutcome};
use crate::types::{get_abi_revision, get_api_level};
use anyhow::Result;
use emulator_instance::{EmulatorInstanceInfo, EmulatorInstances};
use ffx_build_version::VersionInfo;
use ffx_config::{EnvironmentContext, print_config};
use ffx_ssh::{SshKeyErrorKind, SshKeyFiles};
use fuchsia_lockfile::{LockfileCreateError, LockfileCreateErrorKind};
use std::io::{BufWriter, Write};
use std::time::Duration;

const RECORD_CONFIG_SETTING: &str = "doctor.record_config";

pub async fn get_config_permission<W: Write>(
    context: &EnvironmentContext,
    mut writer: W,
) -> Result<bool> {
    match context.get(RECORD_CONFIG_SETTING) {
        Ok(true) => {
            writeln!(
                &mut writer,
                "Config recording is enabled - config data will be recorded. You can change this \
                     with `ffx config set doctor.record_config false"
            )?;
            return Ok(true);
        }
        Ok(false) => {
            writeln!(
                &mut writer,
                "Config recording is disabled - config data will not be recorded. You can change \
                     this with `ffx config set doctor.record_config true"
            )?;
            return Ok(false);
        }
        _ => (),
    }

    let permission: bool;
    loop {
        let mut input = String::new();
        writeln!(&mut writer, "Do you want to include your config data `ffx config get`? [y/n]")?;
        std::io::stdin().read_line(&mut input)?;
        permission = match input.to_lowercase().trim() {
            "yes" | "y" => true,
            "no" | "n" => false,
            _ => continue,
        };
        break;
    }

    writeln!(
        &mut writer,
        "You can permanently enable or disable including config data in doctor records with:"
    )?;
    writeln!(&mut writer, "`ffx config set {} [true|false]`", RECORD_CONFIG_SETTING)?;
    fuchsia_async::Timer::new(Duration::from_millis(1000)).await;

    Ok(permission)
}

pub fn get_user_config(ctx: &EnvironmentContext) -> Result<String> {
    let mut writer = BufWriter::new(Vec::new());
    print_config(ctx, &mut writer)?;
    let config_str = String::from_utf8(writer.into_inner()?)?;
    Ok(config_str)
}

pub async fn check_ffx_info<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    version_info: &VersionInfo,
) {
    let mut ffx_node = ledger.add_node("FFX doctor", LedgerMode::Automatic);
    let frontend_version =
        version_info.build_version.clone().unwrap_or_else(|| "UNKNOWN".to_string());
    ffx_node.add_node_with_outcome(
        &format!("Frontend version: {}", frontend_version),
        LedgerMode::Verbose,
        LedgerOutcome::Success,
    );
    ffx_node.add_node_with_outcome(
        &format!("abi-revision: {}", get_abi_revision(version_info.abi_revision)),
        LedgerMode::Verbose,
        LedgerOutcome::Success,
    );
    ffx_node.add_node_with_outcome(
        &format!("api-level: {}", get_api_level(version_info.api_level)),
        LedgerMode::Verbose,
        LedgerOutcome::Success,
    );

    let ffx_path = match std::env::current_exe() {
        Ok(path) => format!("{}", path.display()),
        _ => "not found".to_string(),
    };
    ffx_node.add_node_with_outcome(
        &format!("Path to ffx: {}", ffx_path),
        LedgerMode::Normal,
        LedgerOutcome::Info,
    );
}

pub async fn check_emulators<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    env_context: &EnvironmentContext,
) -> Result<(), anyhow::Error> {
    let emu_instance_root = env_context.get(ffx_config::keys::EMU_INSTANCE_ROOT_DIR)?;
    let emu_instances = EmulatorInstances::new(emu_instance_root);
    let instances = emu_instances.get_all_instances()?;
    let mut emu_node = ledger.add_node("FFX Emulator Instances", LedgerMode::Normal);
    for instance in &instances {
        let mut instance_node = emu_node.add_node("Instance", LedgerMode::Normal);
        instance_node.add_node_with_outcome(
            &format!("Name: {}", instance.get_name()),
            LedgerMode::Normal,
            LedgerOutcome::Info,
        );
        instance_node.add_node_with_outcome(
            &format!("Is Running: {}", instance.is_running()),
            LedgerMode::Normal,
            LedgerOutcome::Info,
        );
        instance_node.add_node_with_outcome(
            &format!("Engine State: {}", instance.get_engine_state()),
            LedgerMode::Normal,
            LedgerOutcome::Info,
        );
    }
    for instance in &instances {
        if !instance.is_running() {
            let instance_dir = match emu_instances.get_instance_dir(instance.get_name(), false) {
                Ok(dir) => dir,
                Err(e) => {
                    emu_node.add_node_with_outcome(
                        &format!(
                            "Could not determine directory for instance '{}': {}",
                            instance.get_name(),
                            e
                        ),
                        LedgerMode::Normal,
                        LedgerOutcome::Warning,
                    );
                    continue;
                }
            };

            let should_clean = match emulator_instance::read_from_disk_untyped(&instance_dir) {
                Ok(value) => {
                    let state = value.get("engine_state").and_then(|s| s.as_str());
                    state.is_some_and(|s| s.eq_ignore_ascii_case("running"))
                }
                Err(emulator_instance::EmulatorInstanceError::MissingEngineFile(_))
                | Err(emulator_instance::EmulatorInstanceError::ParseJson { .. }) => {
                    // Clean up if it's missing or fundamentally corrupted.
                    true
                }
                Err(e) => {
                    emu_node.add_node_with_outcome(
                        &format!(
                            "Unexpected error reading config for '{}': {}",
                            instance.get_name(),
                            e
                        ),
                        LedgerMode::Normal,
                        LedgerOutcome::Warning,
                    );
                    false
                }
            };

            if should_clean {
                let mut cleanup_node = emu_node.add_node(
                    &format!("Cleanup Stale Emulator: {}", instance.get_name()),
                    LedgerMode::Normal,
                );
                cleanup_node.add_node_with_outcome(
                    &format!("Cleaning up orphaned instance '{}'", instance.get_name()),
                    LedgerMode::Normal,
                    LedgerOutcome::Info,
                );
                if let Err(e) = emu_instances.clean_up_instance_dir(instance.get_name()) {
                    cleanup_node.add_node_with_outcome(
                        &format!("Failed to clean up instance dir: {}", e),
                        LedgerMode::Normal,
                        LedgerOutcome::Failure,
                    );
                } else {
                    cleanup_node.add_node_with_outcome(
                        "Cleanup successful.",
                        LedgerMode::Normal,
                        LedgerOutcome::Success,
                    );
                }
            }
        }
    }
    if instances.is_empty() {
        emu_node.add_node_with_outcome(
            "No Emulator instances",
            LedgerMode::Normal,
            LedgerOutcome::Info,
        );
    }
    Ok(())
}

pub async fn check_env_context<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    env_context: &EnvironmentContext,
) -> Result<(), anyhow::Error> {
    let mut env_node = ledger.add_node("FFX Environment Context", LedgerMode::Normal);
    env_node.add_node_with_outcome(
        &format!("Kind of Environment: {kind}", kind = env_context.env_kind()),
        LedgerMode::Normal,
        LedgerOutcome::Success,
    );
    let (outcome, description) = match env_context.env_file_path() {
        Ok(env_file) => (
            LedgerOutcome::Success,
            format!("Environment File Location: {env_file}", env_file = env_file.display()),
        ),
        Err(e) => {
            (LedgerOutcome::Failure, format!("Error find or loading the environment file: {e:?}"))
        }
    };
    env_node.add_node_with_outcome(&description, LedgerMode::Verbose, outcome);
    if let Some(build_dir) = env_context.build_dir() {
        env_node.add_node_with_outcome(
            &format!(
                "Environment-default build directory: {build_dir}",
                build_dir = build_dir.display()
            ),
            LedgerMode::Normal,
            LedgerOutcome::Success,
        );
    } else {
        env_node.add_node_with_outcome(
            "No build directory discovered in the environment.",
            LedgerMode::Verbose,
            LedgerOutcome::Success,
        );
    };
    check_lock_files(&mut env_node, env_context).await?;
    check_ssh_keys(env_context, &mut env_node).await;
    Ok(())
}

pub async fn check_lock_files<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    env_context: &EnvironmentContext,
) -> Result<(), anyhow::Error> {
    let locks = ffx_config::environment::Environment::check_locks(env_context).await?;
    let mut lock_node = ledger.add_node("Config Lock Files", LedgerMode::Automatic);
    for (file, locked) in locks {
        let (outcome, description) = match locked {
            Ok(lockfile) => (
                LedgerOutcome::Success,
                format!(
                    "{path} locked by {lock}",
                    path = file.display(),
                    lock = lockfile.display()
                ),
            ),
            Err(err) => match *err {
                LockfileCreateError {
                    kind: LockfileCreateErrorKind::TimedOut,
                    lock_path,
                    owner,
                    ..
                } => {
                    let mut msg = format!(
                        "Lockfile `{lockfile}` was owned by another process that didn't release it in our timeout.",
                        lockfile = lock_path.display(),
                    );

                    if let Some(owner) = owner {
                        msg = format!("{msg} Check that it's running? Pid {pid}", pid = owner.pid);
                    }

                    (LedgerOutcome::Failure, msg)
                }
                LockfileCreateError {
                    kind: LockfileCreateErrorKind::Io(error), lock_path, ..
                } => (
                    LedgerOutcome::Failure,
                    format!(
                        "Could not open lockfile `{lockfile}` due to error: {error:?}. Check permissions on the directory.",
                        lockfile = lock_path.display(),
                    ),
                ),
            },
        };
        lock_node.add_node_with_outcome(&description, LedgerMode::Automatic, outcome);
    }
    Ok(())
}

pub async fn check_ssh_keys<W: Write>(
    ctx: &EnvironmentContext,
    ledger: &mut LedgerNodeGuard<'_, W>,
) {
    match SshKeyFiles::load(ctx) {
        Ok(ssh_files) => {
            let (description, outcome) = match ssh_files.check_keys(false) {
                Ok(_) => (
                    "The public & private Fuchsia keys are consistent".to_string(),
                    LedgerOutcome::Success,
                ),
                Err(e) => match e.kind {
                    SshKeyErrorKind::BadKeyType => (
                        format!("SSH keys type not supported: {}", e.message),
                        LedgerOutcome::Warning,
                    ),
                    SshKeyErrorKind::BadConfiguration => {
                        (format!("SSH keys configuration problem: {e}"), LedgerOutcome::Failure)
                    }
                    _ => (
                        format!(
                            "{}. Check configuration or run `ffx doctor --repair-keys`",
                            e.message
                        ),
                        LedgerOutcome::Failure,
                    ),
                },
            };
            ledger.add_node_with_outcome(&description, LedgerMode::Automatic, outcome);
        }
        Err(e) => {
            ledger.add_node_with_outcome(
                &format!("Could not get SSH key paths {e}"),
                LedgerMode::Automatic,
                LedgerOutcome::Failure,
            );
        }
    };
}

#[cfg(all(target_os = "linux", not(test)))]
pub async fn check_inotify_watches<W: Write>(ledger: &mut LedgerNodeGuard<'_, W>) {
    use std::os::unix::fs::MetadataExt;
    let mut watch_node = ledger.add_node("System Inotify Watches", LedgerMode::Automatic);
    let mut total_watches = 0;

    let max_watches = match std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches") {
        Ok(content) => match content.trim().parse::<usize>() {
            Ok(v) => v,
            Err(_) => {
                watch_node.add_node_with_outcome(
                    "Could not parse max_user_watches",
                    LedgerMode::Verbose,
                    LedgerOutcome::Failure,
                );
                return;
            }
        },
        Err(e) => {
            watch_node.add_node_with_outcome(
                &format!("Could not read max_user_watches: {}", e),
                LedgerMode::Verbose,
                LedgerOutcome::Failure,
            );
            return;
        }
    };

    let uid = match std::fs::metadata("/proc/self") {
        Ok(m) => m.uid(),
        Err(e) => {
            watch_node.add_node_with_outcome(
                &format!("Could not get uid: {}", e),
                LedgerMode::Verbose,
                LedgerOutcome::Failure,
            );
            return;
        }
    };

    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let fname = entry.file_name();
            let fname_str = fname.to_string_lossy();
            if !fname_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }

            if let Ok(meta) = std::fs::metadata(&path) {
                if meta.uid() != uid {
                    continue;
                }
            } else {
                continue;
            }

            let fd_path = path.join("fd");
            if let Ok(fd_entries) = std::fs::read_dir(fd_path) {
                for fd_entry in fd_entries.flatten() {
                    let fd_path = fd_entry.path();
                    if let Ok(target) = std::fs::read_link(&fd_path) {
                        if target.to_string_lossy() == "anon_inode:inotify" {
                            let fd_num = fd_entry.file_name();
                            let fdinfo_path = path.join("fdinfo").join(fd_num);
                            if let Ok(content) = std::fs::read_to_string(fdinfo_path) {
                                total_watches += content
                                    .lines()
                                    .filter(|l| l.starts_with("inotify wd:"))
                                    .count();
                            }
                        }
                    }
                }
            }
        }
    }

    if max_watches > 0 {
        let percent = (total_watches * 100) / max_watches;
        let remaining = max_watches.saturating_sub(total_watches);
        if percent >= 80 && remaining < 10000 {
            watch_node.add_node_with_outcome(
                &format!("User is consuming {} / {} inotify watches", total_watches, max_watches),
                LedgerMode::Automatic,
                LedgerOutcome::Warning,
            );
            watch_node.add_node_with_outcome(
                &format!(
                    "Consider increasing max_user_watches: `sudo sysctl fs.inotify.max_user_watches={}`",
                    max_watches + 1048576
                ),
                LedgerMode::Automatic,
                LedgerOutcome::Warning,
            );
        } else {
            watch_node.add_node_with_outcome(
                &format!("User is consuming {} / {} inotify watches", total_watches, max_watches),
                LedgerMode::Verbose,
                LedgerOutcome::Success,
            );
        }
    }
}

#[cfg(any(not(target_os = "linux"), test))]
pub async fn check_inotify_watches<W: Write>(_ledger: &mut LedgerNodeGuard<'_, W>) {}
