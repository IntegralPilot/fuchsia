// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/boot-shim/boot-properties.h"

#include <lib/boot-options/word-view.h>

#include <string_view>

namespace boot_shim {

zx::result<std::string_view> BootProperties::GetProperty(std::string_view key) const {
  std::optional<std::string_view> result;

  EnumerateProperty(key, [&](std::string_view val, linux_boot_config::Value::Action action) {
    if (action == linux_boot_config::Value::Action::kDefine ||
        action == linux_boot_config::Value::Action::kOverride) {
      result = val;
    } else if (!result.has_value()) {
      // Fallback for keys that only have append entries.
      result = val;
    }
  });

  if (result.has_value()) {
    return zx::ok(*result);
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

zx::result<std::string_view> BootProperties::GetFromCmdline(std::string_view key) const {
  std::optional<std::string_view> result;

  for (std::string_view word : WordView(cmdline_)) {
    if (word.starts_with(key)) {
      word.remove_prefix(key.size());
      if (word.empty() || word.front() == '=') {
        if (!word.empty()) {
          word.remove_prefix(1);
        }
        // In the case of multiple entries the last one wins, so we continue iterating.
        result = word;
      }
    }
  }

  if (!result.has_value()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  std::string_view val = *result;
  // Strip surrounding quotes if present.
  if (val.size() >= 2 && val.starts_with('"') && val.ends_with('"')) {
    val = val.substr(1, val.size() - 2);
  }
  return zx::ok(val);
}

}  // namespace boot_shim
