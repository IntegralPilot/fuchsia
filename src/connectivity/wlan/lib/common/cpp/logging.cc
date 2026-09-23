// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <map>
#include <string>

#include <wlan/common/logging.h>

template <typename flags_t>
static void DebugFlags(flags_t flags, std::map<flags_t, std::string> flags_string_map,
                       std::string flags_display_name, std::string flag_on_display_string,
                       std::string flag_off_display_string) {
  static_assert(std::is_integral<flags_t>::value, "flags_t must be an integral type.");
  static_assert(sizeof(flags_t) <= sizeof(uint32_t), "flags_t can be at most 4 bytes.");

  // Setup output stream for formatting hexidecimal numbers
  debugflags("%s: 0x%04x\n", flags_display_name.c_str(), flags);
  for (const auto& [mask, mask_name] : flags_string_map) {
    std::string flag_status_display_string = flag_off_display_string;
    if (mask & flags) {
      flag_status_display_string = flag_on_display_string;
    }
    debugflags("  %s: %s\n", mask_name.c_str(), flag_status_display_string.c_str());
  }
}
