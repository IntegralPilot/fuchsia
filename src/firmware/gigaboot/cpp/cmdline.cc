// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "cmdline.h"

#include <ctype.h>

#include <algorithm>
#include <iterator>

namespace gigaboot {

bool Commandline::Add(const std::string_view key, const std::string_view val) {
  if (key.size() > kCmdlineMaxArgSize || val.size() > kCmdlineMaxArgSize) {
    return false;
  }

  auto entry = std::find_if(valid_entries_.begin(), valid_entries_.end(),
                            [&key](auto& e) { return e.key == key; });
  if (entry != valid_entries_.end()) {
    entry->val = val;
    return true;
  }

  if (valid_entries_.size() == entries_.size()) {
    // Entries array is full, can't append.
    return false;
  }

  valid_entries_ = {entries_.begin(), valid_entries_.size() + 1};
  valid_entries_.back() = {key, val};

  return true;
}

const std::string_view Commandline::Get(const std::string_view key,
                                        const std::string_view _default) {
  auto entry = std::find_if(valid_entries_.begin(), valid_entries_.end(),
                            [&key](auto& e) { return e.key == key; });

  return (entry == valid_entries_.end()) ? _default : entry->val;
}

bool Commandline::AppendItems(std::string_view cmdline) {
  auto end = cmdline.cend();
  auto pos = std::find_if_not(cmdline.cbegin(), end, isspace);

  while (pos != end) {
    auto key_end = std::find_if(pos, end, [](auto& c) { return c == '=' || isspace(c); });
    std::string_view key(&(*pos), key_end - pos);
    pos = key_end + 1;

    bool res;
    if (*key_end == '=') {
      auto val_end = std::find_if(pos, end, isspace);
      std::string_view val(&(*pos), val_end - pos);
      pos = val_end;

      res = Add(key, val);
    } else {
      res = Add(key);
    }

    if (!res) {
      return res;
    }

    pos = std::find_if_not(pos, end, isspace);
  }

  return true;
}

zx::result<size_t> Commandline::ToString(std::span<char> cmdline) {
  auto pos = cmdline.begin();
  for (const auto& entry : valid_entries_) {
    // One char for a space, one for an =, one for a null terminator
    if (pos + entry.key.size() + entry.val.size() + 1 + 1 + 1 > cmdline.end()) {
      return zx::error(ZX_ERR_BUFFER_TOO_SMALL);
    }

    pos += entry.key.copy(&(*pos), entry.key.size());
    if (!entry.val.empty()) {
      *pos++ = '=';
      pos += entry.val.copy(&(*pos), entry.val.size());
    }

    *pos++ = ' ';
  }

  // Remove the trailing space
  if (pos != cmdline.begin()) {
    --pos;
  }

  *pos = '\0';
  return zx::ok(pos - cmdline.begin());
}

}  // namespace gigaboot
