// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use emulator_instance::AccelerationMode;
use ffx_config::FfxConfigBacked;
use ffx_core::ffx_command;
use ffx_emulator_common::host_is_mac;
use std::path::PathBuf;

#[ffx_command()]
#[derive(Clone, ArgsInfo, FromArgs, FfxConfigBacked, Debug, Default, PartialEq)]
#[argh(
    subcommand,
    name = "start",
    description = "Start the Fuchsia emulator.",
    example = "ffx emu start",
    note = "The `start` subcommand is the starting point for all emulator interactions.
The name provided here will be used for all later interactions to indicate
which emulator to target. Emulator names must be unique.

The start command will compile all of the necessary configuration for an
emulator, launch the emulator, and then store the configuration on disk for
future reference. The configuration comes from the Product Bundle, which
includes a virtual device specification and a start-up flag template. See
https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0100_product_metadata
for more information.

Unless run in console mode, the emulator runs as a background daemon and must
be terminated by `ffx emu stop`."
)]
pub struct StartCommand {
    /// virtualization acceleration. Valid choices are "none" to disable acceleration, "hyper" to
    /// use the host's hypervisor interface (KVM on Linux and HVF on MacOS), or "auto" to use the
    /// hypervisor if detected. The default value is "auto".
    #[argh(option, default = "AccelerationMode::Auto")]
    pub accel: AccelerationMode,

    /// specify a configuration file to populate the command line flags for the emulator.
    /// Defaults to a Handlebars config specified in the Product Bundle manifest.
    #[argh(option)]
    pub config: Option<PathBuf>,

    /// specify developer config file to append onto the configuration. This is a JSON file with the
    /// object structure:
    /// {{
    ///   "args": [],
    ///   "kernel_args": [],
    ///   "env" : {{"key": "value"}}
    ///  }}
    ///
    #[argh(option)]
    pub dev_config: Option<PathBuf>,

    /// launch the emulator in serial console mode. This redirects the virtual serial port to the
    /// host's input/output streams, multi-plexed with the QEMU monitor console, then maintains a
    /// connection to those streams rather than returning control to the host terminal. This is
    /// especially useful when the guest is running without networking enabled.
    ///
    /// Use the QEMU key sequence `Ctrl-a c` to switch between the serial console and the QEMU
    /// monitor console.
    ///
    /// Note: Control sequences are passed through to the guest system in this mode, so Crtl-c will
    /// terminate the guest system's shell, rather than the emulator process itself. If you need to
    /// hard-kill the emulator, use the QEMU sequence 'Ctrl-a x' instead.
    #[argh(switch)]
    pub console: bool,

    /// pause on launch and wait for a debugger process to attach before resuming. The guest
    /// operating system will not begin execution until a debugger, such as gdb or lldb, attaches
    /// to the emulator process and signals the emulator to continue.
    #[argh(switch)]
    pub debugger: bool,

    /// specify a custom smp (symmetric multiprocessing) configuration. This will override the
    /// default values from the product bundle or emulator flags template. Often takes the form
    /// of a single integer (e.g. "4") or a more complex string (e.g. "16,threads=2").
    #[argh(option)]
    pub smp: Option<String>,

    /// the virtual device specification used to configure the emulator. This can be the name of a
    /// device listed in the product bundle, or the path to a custom virtual device file. A default
    /// for this flag can be set by running `ffx config set emu.device <type>`. If --device is not
    /// specified and no default is set, then `ffx emu` will attempt to use the product bundle's
    /// recommended device.
    #[argh(option)]
    #[ffx_config_default(key = "emu.device")]
    pub device: Option<String>,

    /// print the list of available virtual devices.
    #[argh(switch)]
    pub device_list: bool,

    /// sets up the emulation configuration, but doesn't stage files or start the emulator. The
    /// command line arguments that the current configuration generates will be printed to stdout
    /// for review.
    #[argh(switch)]
    pub dry_run: bool,

    /// open the user's default editor to modify the command line flags for the emulator.
    #[argh(switch)]
    pub edit: bool,

    /// emulation engine to use for this instance. Allowed values are "femu" which is based on
    /// Android Emulator, and "qemu" which uses the version of Qemu packaged with Fuchsia. Default
    /// is "femu", which can be overridden by running `ffx config set emu.engine <type>`. Engine
    /// defaults are overridden to "qemu" in cases of incompatibility (cross cpu or uefi emulation).
    #[argh(option)]
    #[ffx_config_default(key = "emu.engine", default = "femu")]
    pub engine: Option<String>,

    /// GPU acceleration mode. Allowed values are "swiftshader_indirect", "host", or "auto".
    /// Default is "swiftshader_indirect". "host" and "auto" are for experimental use only and are
    /// not officially supported by the Fuchsia emulator team; graphics artifacts, test failures and
    /// emulator crashes may occur. Note: this is unused when using the "qemu" engine type. See
    /// https://developer.android.com/studio/run/emulator-acceleration#command-gpu for details on
    /// the available options. This can be overridden by running `ffx config set emu.gpu <type>`.
    #[argh(option)]
    #[ffx_config_default(key = "emu.gpu", default = "swiftshader_indirect")]
    pub gpu: Option<String>,

    /// run the emulator without a GUI. The guest system may still initialize graphics drivers,
    /// but no graphics interface will be presented to the user.
    #[argh(switch, short = 'H')]
    pub headless: bool,

    /// enable pixel scaling on HiDPI devices. Defaults to true for MacOS, false otherwise.
    #[argh(option, default = "host_is_mac()")]
    pub hidpi_scaling: bool,

    /// passes the given string to the emulator executable, appended after all other arguments
    /// (since duplicated values favor the later value). This means command-line values will
    /// override configuration-provided values for any of these kernel arguments. Can be repeated
    /// arbitrarily many times for multiple additional kernel arguments.
    #[argh(option, short = 'c')]
    pub kernel_args: Vec<String>,

    /// store the emulator log at the provided filesystem path. By default, all output goes to
    /// a log file in the emulator working directory. The path to this file is printed onscreen
    /// during start-up. Note that the virtual serial port logs are automatically written to an
    /// accompanying file at the same path with a `.serial` suffix (e.g., `<path>.serial`).
    #[argh(option, short = 'l')]
    pub log: Option<PathBuf>,

    /// name of this emulator instance. This is used to identify the instance in other commands and
    /// tools. Default is "fuchsia-emulator". This value can also be set via configuration using the
    /// key `emu.name`. Note that when using the `--uefi` flag, the visible target name in
    /// `ffx target list` will be overwritten by a name "fuchsia-X-Y-Z" where X,Y,Z are derived from
    /// the generated mac address for this emulator. This is currently required to support a
    /// seamless OTA testing workflow.
    #[argh(option)]
    #[ffx_config_default(key = "emu.name", default = "fuchsia-emulator")]
    pub name: Option<String>,

    /// disable serial number generation. Overrides the "emu.serial_number.enabled" config.
    #[argh(switch)]
    pub no_serial_number: bool,

    /// specify the networking mode for the emulator. Allowed values are "none" which disables
    /// networking, "tap" which attaches to a Tun/Tap interface, "user" which sets up mapped ports
    /// via SLiRP, and "auto" which will check the host system's capabilities and select "tap" if
    /// it is available and "user" otherwise. Default is "auto". This can be overridden by
    /// running `ffx config set emu.net <type>`.
    #[argh(option)]
    #[ffx_config_default(key = "emu.net", default = "auto")]
    pub net: Option<String>,

    /// specify a host port mapping for user-networking mode. Ignored in other networking modes.
    /// Syntax is "--port-map <portname>:<port>". The <portname> must be one of those specified in
    /// the virtual device specification. This flag may be repeated for multiple port mappings.
    #[argh(option)]
    pub port_map: Vec<String>,

    /// specify additional port mappings for user-networking mode.
    /// Syntax is "--additional-port-forwards <portname>:<guest_port>".
    /// This flag may be repeated for multiple port mappings.
    /// The guest_port must be non-zero.
    #[argh(option)]
    pub additional_port_forwards: Vec<String>,

    /// use named product information from Product Bundle Metadata (PBM). If no
    /// product bundle is specified and there is an obvious choice, that will be
    /// used (e.g. if there is only one PBM available).
    #[argh(positional)]
    pub product_bundle: Option<String>,

    /// reuse a persistent emulator's (i.e. stopped with `ffx emu stop --persist`) state when
    /// starting up. If an emulator with the same name as this instance has been previously started
    /// and then stopped without cleanup, this instance will reuse the images from the previous
    /// instance. If no previous instance is found, or if the old instance is still running, the new
    /// emulator will not attempt to start.
    #[argh(switch)]
    pub reuse: bool,

    /// reuse a persistent emulator's (i.e. stopped with `ffx emu stop --persist`) state when
    /// starting up after version check. If an emulator with the same name as this instance has been
    /// previously started and then stopped without cleanup, the zbi and disk volume files are
    /// compared against the original. If they match, the instance will reuse the images from the
    /// previous instance. If the files do not match, the instance is started using the latest
    /// files. If there is no staged instance, the emulator is started using the latest files and
    /// the hash information is recorded so this instance can take advantage of this option.
    #[argh(switch)]
    pub reuse_with_check: bool,

    /// sets up the emulation configuration and stages files, but doesn't start the emulator. The
    /// command line arguments that the staged configuration generates will be printed to stdout
    /// for review.
    #[argh(switch)]
    pub stage: bool,

    /// the maximum time (in seconds) to wait on an emulator to boot before returning control
    /// to the user. A value of 0 will skip the check entirely. Default is 60 seconds. This
    /// can be overridden with `ffx config set emu.start.timeout <seconds>`.
    #[argh(option, short = 's')]
    #[ffx_config_default(key = "emu.start.timeout", default = "60")]
    pub startup_timeout: Option<u64>,

    /// create and start an emulator with a full GPT disk including UEFI boot environment and all
    /// partitions in one flat file. This approximates a physical device as closely as possible.
    /// Note that this is currently only enabled for x64 and arm64 targets. RISC-V is unsupported.
    /// Note that for full GPT disks, it is also required to provide the `--vbmeta-key` and
    /// `--vbmeta-key-metadata` arguments, otherwise the resulting GPT image will not be able to
    /// boot into its A and B slots.
    #[argh(switch)]
    pub uefi: bool,

    /// path to the key (a PEM file) that should be used to sign a vbmeta file for the ZBI after
    /// embedding the SSH keys, for example:
    /// https://cs.opensource.google/fuchsia/fuchsia/+/main:boards/x64/BUILD.gn;l=44-46;drc=04892e7f8875e2d16c3fcda89bc462dc6b0f35f8)
    /// Note that this is only required when using `--uefi`. Also, when an emulator is `--reuse`d
    /// after it was stopped with the `--persist` flag, this argument is not necessary as the
    /// previously created images will be used.
    #[argh(option)]
    #[ffx_config_default(key = "emu.vbmeta.key")]
    pub vbmeta_key: Option<String>,

    /// path to the key metadata (a binary file accompanying the PEM, for example
    /// https://cs.opensource.google/fuchsia/fuchsia/+/main:boards/x64/BUILD.gn;l=44-46;drc=04892e7f8875e2d16c3fcda89bc462dc6b0f35f8)
    /// that should be used to sign a vbmeta file for the ZBI after embedding the SSH keys.
    /// Note that this is only required when using `--uefi`. Also, when an emulator is `--reuse`d
    /// after it was stopped with the `--persist` flag, this argument is not necessary as the
    #[argh(option)]
    #[ffx_config_default(key = "emu.vbmeta.metadata")]
    pub vbmeta_key_metadata: Option<String>,

    /// enables extra logging for debugging.
    #[argh(switch, short = 'V')]
    pub verbose: bool,
}
