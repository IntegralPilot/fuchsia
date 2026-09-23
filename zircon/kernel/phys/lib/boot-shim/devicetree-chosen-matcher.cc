// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/devicetree/matcher.h>
#include <lib/fit/defer.h>

#include <span>

#include "lib/boot-shim/devicetree.h"

namespace boot_shim {

devicetree::ScanState DevicetreeChosenNodeMatcherBase::HandleTtyNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  auto [compatible, interrupts, reg_property, reg_offset] =
      decoder.FindProperties("compatible", "interrupts", "reg", "reg-offset");
  // Without this we cant figure out what driver to use.
  if (!compatible) {
    return devicetree::ScanState::kActive;
  }

  // No MMIO region, we cant do anything.
  if (!reg_property) {
    return devicetree::ScanState::kActive;
  }

  auto reg = reg_property->AsReg(decoder);
  if (!reg) {
    return devicetree::ScanState::kActive;
  }

  auto compatible_list = compatible->AsStringList();
  if (!compatible_list) {
    return devicetree::ScanState::kActive;
  }

  // Verify that the `tty.type` has the right prefix, vendor.
  bool possible_tty =
      tty_->vendor.empty() || std::ranges::any_of(*compatible_list, [this](std::string_view entry) {
        return entry.starts_with(tty_->vendor);
      });

  if (!possible_tty || !uart_selector_(decoder)) {
    return devicetree::ScanState::kActive;
  }

  if (tty_->index > tty_index_) {
    (*tty_index_)++;
    return devicetree::ScanState::kActive;
  }
  // We matched a uart driver AND we are the right index.
  return SetUpUart(decoder, *reg, reg_offset, interrupts);
}

devicetree::ScanState DevicetreeChosenNodeMatcherBase::SetUpUart(
    const devicetree::PropertyDecoder& decoder, devicetree::RegProperty& reg,
    const std::optional<devicetree::PropertyValue>& reg_offset,
    const std::optional<devicetree::PropertyValue>& interrupts) {
  auto addr = reg[0].address();
  if (addr) {
    auto translated_addr = decoder.TranslateAddress(*addr);
    if (!translated_addr) {
      return devicetree::ScanState::kDone;
    }

    if (!uart_selector_(decoder)) {
      return devicetree::ScanState::kDone;
    }

    uart_config_->mmio_phys = *translated_addr;
    uart_config_->irq = 0;

    if (reg_offset) {
      if (auto offset = reg_offset->AsUint32()) {
        uart_config_->mmio_phys += *offset;
      } else {
        OnError("Failed to parse 'reg-offset' property from UART node.");
      }
    }

    if (!interrupts) {
      Log("Warning: UART Device does not provide interrupt cells. Proceeding with irq = 0.\n");
    } else {
      uart_irq_ = DevicetreeIrqResolver(interrupts->AsBytes());
      if (auto result = uart_irq_.ResolveIrqController(decoder); result.is_ok()) {
        if (!*result) {
          return devicetree::ScanState::kActive;
        }
        SetUartIrq();
      }
    }
  }
  return devicetree::ScanState::kDone;
}

devicetree::ScanState DevicetreeChosenNodeMatcherBase::HandleBootstrapStdout(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  auto resolved_path = decoder.ResolvePath(stdout_path_);
  if (resolved_path.is_error()) {
    if (resolved_path.error_value() == devicetree::PropertyDecoder::PathResolveError::kNoAliases) {
      return devicetree::ScanState::kNeedsPathResolution;
    }
    return devicetree::ScanState::kDone;
  }

  // for hand off.
  resolved_stdout_ = *resolved_path;

  switch (path.CompareWith(*resolved_path)) {
    case devicetree::NodePath::Comparison::kEqual:
      break;
    case devicetree::NodePath::Comparison::kParent:
    case devicetree::NodePath::Comparison::kIndirectAncestor:
      return devicetree::ScanState::kActive;
    default:
      return devicetree::ScanState::kDoneWithSubtree;
  }

  auto [compatible, interrupts, reg_property, reg_offset] =
      decoder.FindProperties("compatible", "interrupts", "reg", "reg-offset");

  // Without this we cant figure out what driver to use.
  if (!compatible) {
    return devicetree::ScanState::kDone;
  }

  // No MMIO region, we cant do anything.
  if (!reg_property) {
    return devicetree::ScanState::kDone;
  }

  auto reg = reg_property->AsReg(decoder);
  if (!reg) {
    return devicetree::ScanState::kDone;
  }

  return SetUpUart(decoder, *reg, reg_offset, interrupts);
}

devicetree::ScanState DevicetreeChosenNodeMatcherBase::OnNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  if (path.CompareWith("/aliases") == devicetree::NodePath::Comparison::kEqual) {
    for (auto prop : decoder.properties()) {
      constexpr std::string_view kSerialPrefix = "serial";
      if (prop.name.starts_with(kSerialPrefix)) {
        std::string_view index_str = prop.name.substr(kSerialPrefix.size());
        auto index = BootOptions::ParseInt(index_str);
        if (index && *index >= 0 && static_cast<size_t>(*index) < serial_aliases_.size()) {
          size_t idx = static_cast<size_t>(*index);
          serial_aliases_[idx] = prop.value.AsString().value_or("");
        }
      }
    }
    return devicetree::ScanState::kActive;
  }

  if (found_chosen_) {
    if (uart_irq_.NeedsInterruptParent()) {
      if (auto result = uart_irq_.ResolveIrqController(decoder); result.is_ok()) {
        if (!*result) {
          return devicetree::ScanState::kActive;
        }
        SetUartIrq();
      }
      return devicetree::ScanState::kDone;
    }

    if (!stdout_path_.empty()) {
      return HandleBootstrapStdout(path, decoder);
    }

    if (tty_) {
      if (tty_index_) {
        return HandleTtyNode(path, decoder);
      }
      return devicetree::ScanState::kDoneWithSubtree;
    }
    return devicetree::ScanState::kDone;
  }

  switch (path.CompareWith("/chosen")) {
    case devicetree::NodePath::Comparison::kParent:
    case devicetree::NodePath::Comparison::kIndirectAncestor:
      return devicetree::ScanState::kActive;
    case devicetree::NodePath::Comparison::kMismatch:
    case devicetree::NodePath::Comparison::kChild:
    case devicetree::NodePath::Comparison::kIndirectDescendent:
      return devicetree::ScanState::kDoneWithSubtree;
    case devicetree::NodePath::Comparison::kEqual:
      found_chosen_ = true;
      break;
  };

  // We are on /chosen, pull the cmdline, zbi and uart device path.
  auto [bootargs, stdout_path, legacy_stdout_path, ramdisk_start, ramdisk_end] =
      decoder.FindProperties("bootargs", "stdout-path", "linux,stdout-path", "linux,initrd-start",
                             "linux,initrd-end");
  if (bootargs) {
    if (auto cmdline = bootargs->AsString()) {
      cmdline_ = *cmdline;
    }
  }

  // If stdout-path is present, use it. Otherwise we will try to find a tty device via the command
  // line.
  if (stdout_path) {
    stdout_path_ = stdout_path->AsString().value_or("");
  } else if (legacy_stdout_path) {
    stdout_path_ = legacy_stdout_path->AsString().value_or("");
  }

  // Make it just contain the prefix. This string can be formatted as 'path:UART_ARGS'.
  // Where UART_ARGS can be things such as baud rate, parity, etc. We only need |path| from the
  // format.
  if (!stdout_path_.empty()) {
    stdout_path_ = stdout_path_.substr(0, stdout_path_.find(':'));
  } else {
    // If we haven't found the stdout path, try to resolve the tty via the command line.
    tty_ = boot_shim::TtyFromCmdline(cmdline_);
    if (tty_) {
      // Check if the tty is an alias in the aliases node.
      if (auto resolved_path = boot_shim::ResolveTtyAlias(*tty_, serial_aliases_)) {
        stdout_path_ = *resolved_path;
        Log("DevicetreeChosenNodeMatcherBase::OnNode: resolved tty via aliases\n");
      }
    }
  }

  if (ramdisk_start && ramdisk_end) {
    // RISC V and ARM disagree on what the type of these fields are. In both cases its an integer,
    // but sometimes is u32 and sometimes is u64. So based on the number of bytes, guess.
    std::optional<uint64_t> address_start =
        ramdisk_start->AsBytes().size() > sizeof(uint32_t)
            ? ramdisk_start->AsUint64()
            : static_cast<std::optional<uint64_t>>(ramdisk_start->AsUint32());

    std::optional<uint64_t> address_end =
        ramdisk_end->AsBytes().size() > sizeof(uint32_t)
            ? ramdisk_end->AsUint64()
            : static_cast<std::optional<uint64_t>>(ramdisk_end->AsUint32());
    if (!address_start) {
      OnError("Failed to parse chosen node's \"linux,initrd-start\" property.");
      return devicetree::ScanState::kActive;
    }

    if (!address_end) {
      OnError("Failed to parse chosen node's \"linux,initrd-end\" property.");
      return devicetree::ScanState::kActive;
    }

    ramdisk_ = std::span<const std::byte>(reinterpret_cast<const std::byte*>(*address_start),
                                          static_cast<size_t>(*address_end - *address_start));
  }

  return devicetree::ScanState::kActive;
}

}  // namespace boot_shim
