// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/redact/cache.h"

#include <unordered_map>

namespace forensics {

RedactionIdCache::RedactionIdCache(inspect::UintProperty size_node, const int starting_id,
                                   const size_t capacity)
    : next_id_(starting_id), size_node_(std::move(size_node)), capacity_(capacity) {
  size_node_.Set(0u);
}

int RedactionIdCache::GetId(const std::string& value) {
  const size_t hash = std::hash<std::string>{}(value);

  if (ids_.contains(hash)) {
    // Move |hash| to the front of |lru_|.
    lru_.splice(lru_.begin(), lru_, ids_[hash].second);
    return ids_[hash].first;
  }

  if (lru_.size() == capacity_) {
    ids_.erase(lru_.back());
    lru_.pop_back();
  } else {
    size_node_.Add(1u);
  }

  lru_.push_front(hash);
  ids_[hash] = {++next_id_, lru_.begin()};

  return ids_[hash].first;
}

}  // namespace forensics
