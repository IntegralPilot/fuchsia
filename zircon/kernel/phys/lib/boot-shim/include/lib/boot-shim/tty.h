// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_TTY_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_TTY_H_

#include <lib/uart/all.h>
#include <lib/zbi-format/driver-config.h>

#include <cstdint>
#include <span>

namespace boot_shim {

enum class TtyType : uint8_t {
  // Any UART type.
  kAny,

  // Filtered to serial only.
  kSerial,

  // Used in arm.
  kAml,
  kMsm,
  kSac,
};

// Parses `console=` option, for example "console=ttyS0" would yield `{TtyType::kSerial, 0}`.
struct Tty {
  TtyType type = TtyType::kAny;
  size_t index = 0;

  // Provided when the `TtyType` is mapped to a specific vendor/manufacturer.
  // Should match a devicetree binding vendor string.
  std::string_view vendor;
};

// Best effort at parsing Linux-compatible `console=` command, if present.
std::optional<Tty> TtyFromCmdline(std::string_view cmdline);

// Resolves a TTY against a list of serial aliases.
std::optional<std::string_view> ResolveTtyAlias(const Tty& tty,
                                                std::span<const std::string_view> serial_aliases);

}  // namespace boot_shim

#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_TTY_H_
