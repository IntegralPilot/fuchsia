// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The config module contains keys to the ffx_config structure for values used
//! by the emulator command.

/// FEMU tool.
pub const FEMU_TOOL: &'static str = "aemu_internal";

/// QEMU tool.
pub const QEMU_TOOL: &'static str = "qemu_internal";

/// CROSVM tool.
pub const CROSVM_TOOL: &'static str = "crosvm_internal";

/// The SDK tool "fvm" used to modify fvm files.
pub const FVM_HOST_TOOL: &'static str = "fvm";

/// The SDK tool "zbi" used to modify the zbi image.
pub const ZBI_HOST_TOOL: &'static str = "zbi";

/// The SDK tool file for the x64 UEFI (OVMF_CODE.fd)
pub const OVMF_CODE_X64: &'static str = "uefi_internal_x64";

/// The SDK tool file for the arm64 UEFI (QEMU_EFI.fd)
pub const OVMF_CODE_ARM64: &'static str = "uefi_internal_arm64";

/// An arm64 secure bootloader capable of booting a QEMU kernel via Linux's
/// boot protocol.
pub const BOOTLOADER_SECURE_ARM64: &'static str = "secure_bootloader_arm64";

/// The experimental flag for the console subcommand. Defaults to false.
pub const EMU_CONSOLE_FLAG: &'static str = "emu.console.enabled";

/// The default virtual device name to configure the emulator. Defaults to
/// the empty string, but can be overridden by the user.
pub const EMU_DEFAULT_DEVICE: &'static str = "emu.device";

/// The default engine to launch from `ffx emu start`. Defaults to
/// femu, but can be overridden by the user.
pub const EMU_DEFAULT_ENGINE: &'static str = "emu.engine";

/// The default gpu type to use in `ffx emu start`. Defaults to
/// "auto", but can be overridden by the user.
pub const EMU_DEFAULT_GPU: &'static str = "emu.gpu";

/// The root directory for storing instance specific data. Instances
/// should create a subdirectory in this directory to store data.
pub const EMU_INSTANCE_ROOT_DIR: &'static str = "emu.instance_dir";

/// Whether or not to generate and enable emulator serial numbers. Defaults to true.
pub const EMU_SERIAL_ENABLED: &'static str = "emu.serial_number.enabled";

/// The filesystem path to the system's KVM device. Must be writable by the
/// running process to utilize KVM for acceleration.
pub const KVM_PATH: &'static str = "emu.kvm_path";

/// The duration (in seconds) to attempt to establish an RCS connection with
/// a new emulator before returning to the terminal. Not used in --console or
/// ---monitor modes. Defaults to 60 seconds.
pub const EMU_START_TIMEOUT: &'static str = "emu.start.timeout";

/// The full path to the script to run initializing any network interfaces
/// before starting the emulator.
pub const EMU_UPSCRIPT_FILE: &'static str = "emu.upscript";

/// The full path to the vbmeta key file for re-signing a ZBI
pub const VBMETA_KEY_FILE: &'static str = "emu.vbmeta.key";

/// The full path to the vbmeta metadata file for re-signing a ZBI
pub const VBMETA_METADATA_FILE: &'static str = "emu.vbmeta.metadata";
