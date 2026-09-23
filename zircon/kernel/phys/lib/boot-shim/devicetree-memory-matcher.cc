// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/boot-shim/devicetree.h"
#include "lib/memalloc/range.h"

namespace boot_shim {
namespace {

constexpr std::string_view kRamOops = "ramoops";

bool IsRamOops(const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  if (path.back().name() == kRamOops) {
    return true;
  }

  auto compatible_list =
      decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");
  if (!compatible_list) {
    return false;
  }

  return std::find(compatible_list->begin(), compatible_list->end(), kRamOops) !=
         compatible_list->end();
}

}  // namespace

devicetree::ScanState DevicetreeMemoryMatcher::OnNode(const devicetree::NodePath& path,
                                                      const devicetree::PropertyDecoder& decoder) {
  if (path.IsDescendentOf("/reserved-memory")) {
    if (IsRamOops(path, decoder)) {
      OnEachRangeFromReg(decoder, [this](uint64_t addr, uint64_t size) {
        nvram_ = {
            .base = addr,
            .length = size,
        };
        return true;
      });
      return devicetree::ScanState::kActive;
    }

    if (!AppendRangesFromReg(decoder, memalloc::Type::kReserved)) {
      return devicetree::ScanState::kDone;
    }
    return devicetree::ScanState::kActive;
  }

  if (path == "/") {
    return devicetree::ScanState::kActive;
  }

  // Only look for direct children of the root node.
  if (path.IsChildOf("/")) {
    auto name = path.back().name();
    if (name == "memory") {
      return HandleMemoryNode(path, decoder);
    }

    if (name == "reserved-memory") {
      return HandleReservedMemoryNode(path, decoder);
    }
  }

  // No need to look at other things.
  return devicetree::ScanState::kDoneWithSubtree;
}

// |path.back()| must be 'memory'
// Each node may define N ranges in their reg property.
// Each node may have children defining subregions with special purpose (RESERVED ranges.)/
devicetree::ScanState DevicetreeMemoryMatcher::HandleMemoryNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  ZX_DEBUG_ASSERT(path.back().name() == "memory");

  if (!AppendRangesFromReg(decoder, memalloc::Type::kFreeRam)) {
    return devicetree::ScanState::kDone;
  }

  return devicetree::ScanState::kActive;
}

// |path.back()| must be 'reserved-memory'
// Each child node is a reserved region.
devicetree::ScanState DevicetreeMemoryMatcher::HandleReservedMemoryNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  ZX_DEBUG_ASSERT(path.back() == "reserved-memory");

  if (!AppendRangesFromReg(decoder, memalloc::Type::kReserved)) {
    return devicetree::ScanState::kDone;
  }

  return devicetree::ScanState::kActive;
}

void DevicetreeMemoryMatcher::OnEachRangeFromReg(const devicetree::PropertyDecoder& decoder,
                                                 RangeVisitor range_visitor) {
  // Look at the reg property for possible memory banks.
  const auto& [reg_property] = decoder.FindProperties("reg");

  if (!reg_property) {
    return;
  }

  auto reg_ptr = reg_property->AsReg(decoder);
  if (!reg_ptr) {
    OnError("Memory: Failed to decode 'reg'.");
    return;
  }

  auto& reg = *reg_ptr;

  for (size_t i = 0; i < reg.size(); ++i) {
    auto addr = reg[i].address();
    auto size = reg[i].size();
    if (!addr || !size) {
      continue;
    }

    auto translated_address = decoder.TranslateAddress(*addr);
    if (!translated_address) {
      continue;
    }

    if (!range_visitor(*translated_address, *size)) {
      return;
    }
  }
}

bool DevicetreeMemoryMatcher::AppendRangesFromReg(const devicetree::PropertyDecoder& decoder,
                                                  memalloc::Type memrange_type) {
  bool had_errors = false;
  auto appender = [this, &had_errors, memrange_type](uint64_t addr, uint64_t size) -> bool {
    had_errors = !AppendRange({
        .addr = addr,
        .size = size,
        .type = memrange_type,
    });
    return !had_errors;
  };
  OnEachRangeFromReg(decoder, appender);

  return !had_errors;
}

}  // namespace boot_shim
