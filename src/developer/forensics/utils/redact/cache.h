// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_REDACT_CACHE_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_REDACT_CACHE_H_

#include <lib/inspect/cpp/vmo/types.h>

#include <list>
#include <string>
#include <unordered_map>

#include "src/developer/forensics/feedback/constants.h"

namespace forensics {

// Associates unique integer identifiers with strings, e.g. the string "12345" will be the only
// string to have an ID X. Once the number of strings exceeds |capacity_| the least recently used
// string "x" will be deleted from the cache to make room for the new string. If "x" is later used
// again, "x" will be assigned a new unique integer identifier.
class RedactionIdCache {
 public:
  explicit RedactionIdCache(inspect::UintProperty size_node, int starting_id = 0,
                            size_t capacity = feedback::kRedactionIdCacheCapacity);

  int GetId(const std::string& value);

  // Require move-only semantics are required because the cache is stateful.
  RedactionIdCache(const RedactionIdCache&) = delete;
  RedactionIdCache& operator=(const RedactionIdCache&) = delete;
  RedactionIdCache(RedactionIdCache&&) = default;
  RedactionIdCache& operator=(RedactionIdCache&&) = default;

 private:
  int next_id_;

  // Key: hash of the string. The hash is used to save space.
  // Value: pair of <id, entry in |lru_|>.
  std::unordered_map<size_t, std::pair<int, std::list<size_t>::iterator>> ids_;

  // List of keys in |ids_|. The back of the list is the least recently used.
  std::list<size_t> lru_;
  inspect::UintProperty size_node_;
  size_t capacity_;
};

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_REDACT_CACHE_H_
