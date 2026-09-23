// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This module is used to house common "global" configuration values that may
// cross multiple crates, plugins or tools so as to avoid large,
// cross-binary dependency graphs.

/// The default target to communicate with if no target is specified.
pub const TARGET_DEFAULT_KEY: &str = "target.default";

/// The timeout used before giving up on attempting to connect to a FIDL proxy.
pub const PROXY_TIMEOUT: &'static str = "proxy.timeout_secs";

/// The timeout used before giving up on uploading metrics in fractional seconds.
pub const METRICS_UPLOAD_TIMEOUT_KEY: &'static str = "metrics.upload_timeout";

/// The timeout, in milliseconds, when discovering a device.
pub const DISCOVERY_TIMEOUT_MS: &str = "discovery.timeout";

/// This is a bit of a special case: the upload default timeout could potentially
/// be inaccessible due to not being able to initialize an `EnvironmentContext` correctly, so there
/// _needs_ to be a reasonable backup somewhere if that is the case. See `ffx_command::report_bug()`
/// for where this use-case is taken into account. While this isn't a "key" per se, it's just being
/// kept in this module for ease of discoverability/autocomplete.
// LINT.IfChange
pub const METRICS_UPLOAD_TIMEOUT_DEFAULT: f64 = 2.0;
// LINT.ThenChange(../../../docs/configuration.md, ../../../data/config.json)

/// The location of the fastboot devices file. Defaults to ${HOME}/.fastboot/devices
pub const FASTBOOT_FILE_PATH: &str = "fastboot.devices_file.path";

/// The root directory for storing instance specific emulator data. Instances
/// should create a subdirectory in this directory to store data.
pub const EMU_INSTANCE_ROOT_DIR: &'static str = "emu.instance_dir";

/// The root directory for storing instance specific GCE data.
pub const GCE_INSTANCE_ROOT_DIR: &'static str = "gce.instance_dir";

/// Whether or not to enable vsock connectivity.
pub const VSOCK_ENABLED: &'static str = "connectivity.enable_vsock";

/// Whether or not to enable usb connectivity.
pub const USB_ENABLED: &'static str = "connectivity.enable_usb";

/// Whether or not to enable uart connectivity.
pub const UART_ENABLED: &'static str = "connectivity.enable_uart";

/// The default baud rate for host-side UART connections.
pub const UART_BAUD: &'static str = "connectivity.uart.baud";

/// Whether or not to enable network connectivity.
pub const NETWORK_ENABLED: &'static str = "connectivity.enable_network";

/// Whether to use direct mode instead of daemon mode
pub const DIRECT_CONNECTIONS: &str = "connectivity.direct";

/// This is the port that, on the device, service the authorized_keys file.
pub const AUTHORIZED_KEYS_HTTP_PORT_QUERY: &str = "ssh.authorized_keys_server_port";

/// The path to the ssh public key.
pub const SSH_PUB_KEY: &str = "ssh.pub";

/// The path to the ssh private key.
pub const SSH_PRIVATE_KEY: &str = "ssh.priv";

/// The path to the discovery cache directory
pub const DISCOVERY_CACHE_DIR_CONFIG: &str = "target.discovery_cache_dir";

/// Whether or not to enable colored output for log commands.
pub const LOG_CMD_COLOR: &str = "log_cmd.color";

/// Whether or not to allow build-level configuration overrides for SDK host tools (sdk.overrides.*).
pub const SDK_ALLOW_BUILD_HOST_TOOLS: &str = "sdk.overrides.allow-build-host-tools";
