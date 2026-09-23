// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{EXIT_FAILURE, EXIT_SUCCESS, ShellState};
use bstr::{BString, ByteSlice};
use fidl_fuchsia_hardware_power_statecontrol as fidl_power;
use fidl_fuchsia_kernel as fidl_kernel;
use std::io::{Read, Write};

// Matches dash, which limits debug commands to 256 bytes due to its fixed stack buffer.
// Note that the FIDL definition (`fuchsia.kernel.DEBUG_COMMAND_MAX`) permits up to 1024 bytes
// (`fidl_kernel::DEBUG_COMMAND_MAX`). We might want to adjust this constant to match the FIDL value.
const MAX_COMMAND_LEN: usize = 256;

fn command_cmp(long_command: &[u8], short_command: Option<&[u8]>, input: &[u8]) -> bool {
    let matches = |cmd: &[u8]| {
        input.len() >= cmd.len()
            && &input[..cmd.len()] == cmd
            && (input.len() == cmd.len() || input[cmd.len()] == b' ')
    };
    if let Some(short) = short_command {
        if matches(short) {
            return true;
        }
    }
    matches(long_command)
}

fn run_statecontrol_shutdown(action: fidl_power::ShutdownAction, stdout: &mut dyn Write) -> i32 {
    let options = fidl_power::ShutdownOptions {
        action: Some(action),
        reasons: Some(vec![fidl_power::ShutdownReason::DeveloperRequest]),
        ..Default::default()
    };

    let (client_end, server_end) = zx::Channel::create();
    if let Err(_status) = fuchsia_component::client::connect_channel_to_protocol::<
        fidl_power::AdminMarker,
    >(server_end)
    {
        return EXIT_FAILURE;
    }
    let admin = fidl_power::AdminSynchronousProxy::new(client_end);

    match admin.shutdown(&options, zx::MonotonicInstant::INFINITE) {
        Err(e) => {
            let _ = writeln!(stdout, "Command failed: {}", e);
            EXIT_FAILURE
        }
        Ok(Err(error_value)) => {
            let _ = writeln!(stdout, "Command failed: {}", error_value);
            EXIT_SUCCESS
        }
        Ok(Ok(())) => EXIT_SUCCESS,
    }
}

const DM_USAGE: &str = "\
poweroff             - power off the system
shutdown             - power off the system
reboot               - reboot the system
reboot-bootloader/rb - reboot the system into bootloader
reboot-recovery/rr   - reboot the system into recovery";

const POWER_USAGE: &str = "\
off                  - power off the system
shutdown             - power off the system
reboot               - reboot the system
reboot-bootloader/rb - reboot the system into bootloader
reboot-recovery/rr   - reboot the system into recovery";

pub fn builtin_dm(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    _stderr: &mut dyn Write,
) -> i32 {
    if args.len() != 1 {
        let _ = writeln!(stdout, "usage: dm <command>");
        return EXIT_FAILURE;
    }

    let cmd = args[0].as_bytes();
    if command_cmp(b"help", None, cmd) {
        let _ = writeln!(stdout, "{DM_USAGE}");
        return EXIT_SUCCESS;
    }

    let action = if command_cmp(b"reboot", None, cmd) {
        fidl_power::ShutdownAction::Reboot
    } else if command_cmp(b"reboot-bootloader", Some(b"rb"), cmd) {
        fidl_power::ShutdownAction::RebootToBootloader
    } else if command_cmp(b"reboot-recovery", Some(b"rr"), cmd) {
        fidl_power::ShutdownAction::RebootToRecovery
    } else if command_cmp(b"poweroff", None, cmd) || command_cmp(b"shutdown", None, cmd) {
        fidl_power::ShutdownAction::Poweroff
    } else {
        let _ = writeln!(stdout, "Unknown command '{}'\n\nValid commands:\n{DM_USAGE}", args[0]);
        return EXIT_FAILURE;
    };

    run_statecontrol_shutdown(action, stdout)
}

pub fn builtin_power(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    _stderr: &mut dyn Write,
) -> i32 {
    if args.len() != 1 {
        let _ = writeln!(stdout, "usage: power <command>");
        return EXIT_FAILURE;
    }

    let cmd = args[0].as_bytes();
    if command_cmp(b"help", None, cmd) {
        let _ = writeln!(stdout, "{POWER_USAGE}");
        return EXIT_SUCCESS;
    }

    let action = if command_cmp(b"reboot", None, cmd) {
        fidl_power::ShutdownAction::Reboot
    } else if command_cmp(b"reboot-bootloader", Some(b"rb"), cmd) {
        fidl_power::ShutdownAction::RebootToBootloader
    } else if command_cmp(b"reboot-recovery", Some(b"rr"), cmd) {
        fidl_power::ShutdownAction::RebootToRecovery
    } else if command_cmp(b"off", None, cmd) || command_cmp(b"shutdown", None, cmd) {
        fidl_power::ShutdownAction::Poweroff
    } else {
        let _ = writeln!(stdout, "Unknown command '{}'\n\nValid commands:\n{POWER_USAGE}", args[0]);
        return EXIT_FAILURE;
    };

    run_statecontrol_shutdown(action, stdout)
}

pub fn builtin_k(
    args: &[BString],
    state: &mut ShellState,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.is_empty() {
        let _ = writeln!(stdout, "usage: k <command>");
        return EXIT_FAILURE;
    }

    if args[0] == "poweroff" || args[0] == "reboot" || args[0] == "reboot-bootloader" {
        return builtin_dm(args, state, stdin, stdout, stderr);
    }

    let mut command_bytes = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            command_bytes.push(b' ');
        }
        command_bytes.extend_from_slice(arg.as_bytes());
    }
    if command_bytes.len() >= MAX_COMMAND_LEN {
        let _ = writeln!(stderr, "error: kernel debug command too long");
        return EXIT_FAILURE;
    }

    let (client_end, server_end) = zx::Channel::create();
    if let Err(_status) = fuchsia_component::client::connect_channel_to_protocol::<
        fidl_kernel::DebugBrokerMarker,
    >(server_end)
    {
        return EXIT_FAILURE;
    }
    let broker = fidl_kernel::DebugBrokerSynchronousProxy::new(client_end);
    let command_str = match std::str::from_utf8(command_bytes.as_slice()) {
        Ok(s) => s,
        Err(_) => {
            let _ = writeln!(stderr, "error: invalid UTF-8 in command");
            return EXIT_FAILURE;
        }
    };

    let status = match broker.send_debug_command(command_str, zx::MonotonicInstant::INFINITE) {
        Ok(status) => status,
        Err(e) => {
            let _ = writeln!(
                stderr,
                "error: unable to send kernel debug command ({}), is kernel debugging disabled?",
                e
            );
            return EXIT_FAILURE;
        }
    };

    if let Err(zx_status) = zx::Status::ok(status) {
        let hint = if zx_status == zx::Status::NOT_SUPPORTED {
            ", is kernel debugging disabled?"
        } else {
            ""
        };
        let _ = writeln!(stderr, "error: {}{}", zx_status, hint);
        return EXIT_FAILURE;
    }

    EXIT_SUCCESS
}
