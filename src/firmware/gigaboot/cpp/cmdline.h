// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/result.h>

#include <array>
#include <span>
#include <string_view>

#ifndef SRC_FIRMWARE_GIGABOOT_CPP_CMDLINE_H_
#define SRC_FIRMWARE_GIGABOOT_CPP_CMDLINE_H_

namespace gigaboot {

// Helper class for constructing, manipulating, and querying the kernel command line.
class Commandline {
 public:
  static constexpr size_t kMaxCmdlineItems = 128;
  static constexpr size_t kCmdlineStrSize = 0x1000;
  static constexpr size_t kCmdlineMaxArgSize = 1024;

  // Given an empty buffer, serialize the command line to a string.
  // If successful, returns the length of the command line string.
  zx::result<size_t> ToString(std::span<char> cmdline);

  // Add or update a flag or key/value entry to the command line.
  // Returns true on success.
  //
  // Postcondition: key and val remain valid and unmodified at least as long as this instance.
  bool Add(const std::string_view key, const std::string_view val = "");

  // Given a string representing a command line,
  // parse the string into an internal representation
  // so that command line items can be updated and queried.
  // Returns true on success.
  //
  // Postcondition: cmdline remains valid and unmodified at least as long as this instance.
  bool AppendItems(const std::string_view cmdline);

  // Given a parameter name, returns the associated value if present,
  // or the provided default if not present.
  const std::string_view Get(const std::string_view key, const std::string_view _default = "");

 private:
  struct CommandlineEntry {
    std::string_view key;
    std::string_view val;
  };

  std::array<CommandlineEntry, kMaxCmdlineItems> entries_;
  std::span<CommandlineEntry> valid_entries_ = {entries_.begin(), 0};
};

}  // namespace gigaboot

#endif  // SRC_FIRMWARE_GIGABOOT_CPP_CMDLINE_H_
