// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_BOOT_PROPERTIES_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_BOOT_PROPERTIES_H_

#include <lib/linux-boot-config/linux-boot-config.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <optional>
#include <string_view>

namespace boot_shim {

class BootProperties {
 public:
  explicit BootProperties(
      std::string_view cmdline,
      std::optional<linux_boot_config::LinuxBootConfig> bootconfig = std::nullopt)
      : cmdline_(cmdline), bootconfig_(bootconfig) {}

  // Extracts a property value (e.g. "kernel.param", "driver.option").
  // Precedence: Bootconfig first, then command line.
  zx::result<std::string_view> GetProperty(std::string_view key) const;

  // Invokes a callback for all matching property definitions/appends for a key.
  //
  // BootConfig is small enough that it's worthwhile to parse the entirety for each key we care
  // about to keep the API for each given item straightforward. We could refactor this to scan once,
  // but the ergonomics change to Item classes wouldn't be worth it.
  void EnumerateProperty(std::string_view key, auto&& cb) const {
    bool found_in_bootconfig = false;
    if (bootconfig_) {
      std::ignore = bootconfig_->Parse(
          [&](const linux_boot_config::Key& k, const linux_boot_config::Value& val) {
            if (k == key) {
              found_in_bootconfig = true;
              cb(val.value, val.action);
            }
          });
    }
    if (!found_in_bootconfig) {
      if (zx::result<std::string_view> res = GetFromCmdline(key); res.is_ok()) {
        cb(*res, linux_boot_config::Value::Action::kDefine);
      }
    }
  }

 private:
  zx::result<std::string_view> GetFromCmdline(std::string_view key) const;

  std::string_view cmdline_;
  std::optional<linux_boot_config::LinuxBootConfig> bootconfig_;
};

}  // namespace boot_shim

#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_BOOT_PROPERTIES_H_
