// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef SRC_FIRMWARE_LIB_BOOT_SHIM_UTILS_INCLUDE_LIB_BOOT_SHIM_UTILS_COMMANDLINE_BOOTLOADER_FILE_ITEM_H_
#define SRC_FIRMWARE_LIB_BOOT_SHIM_UTILS_INCLUDE_LIB_BOOT_SHIM_UTILS_COMMANDLINE_BOOTLOADER_FILE_ITEM_H_

#include <lib/boot-shim/item-base.h>
#include <stddef.h>

#include <string_view>

// Custom item to construct a `ZBI_TYPE_BOOTLOADER_FILE` item from Base64-encoded commandline args.
//
// Useful for device bootloaders which do not support standard OEM commands for staging bootloader
// files (such as SSH keys), but do provide the ability to register custom commandline arguments.
//
// The file data may be split across multiple commandline args with the given prefix.
//
// Example:
//   * prefix = "ssh_creds=", filename = "ssh.authorized_keys"
//   * commandline: "ssh_creds=Zm9vIGJ ssh_creds=hciBiYXo=" ("Zm9vIGJhciBiYXo=" -> "foo bar baz")
//   * payload: name = "ssh.authorized_keys", contents = "foo bar baz"
class CommandlineBootloaderFileItem : public boot_shim::ItemBase {
 public:
  // Stores `cmdline`, `prefix`, and `filename` for processing during shim execution.
  void Init(std::string_view cmdline, std::string_view prefix, std::string_view filename);

  // Required `boot_shim::ItemBase` functions.
  size_t size_bytes() const;
  fit::result<DataZbi::Error> AppendItems(DataZbi& zbi) const;

 private:
  // The full kernel commandline.
  std::string_view cmdline_;
  // The commandline argument prefix to look for.
  std::string_view prefix_;
  // The destination bootloader file name.
  std::string_view filename_;
  // Length of the Base64 encoded chunks, or 0 if none existed in the commandline.
  size_t base64_size_ = 0;
};

#endif  // SRC_FIRMWARE_LIB_BOOT_SHIM_UTILS_INCLUDE_LIB_BOOT_SHIM_UTILS_COMMANDLINE_BOOTLOADER_FILE_ITEM_H_
