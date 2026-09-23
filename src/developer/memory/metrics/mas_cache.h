// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_METRICS_MAS_CACHE_H_
#define SRC_DEVELOPER_MEMORY_METRICS_MAS_CACHE_H_

#include <optional>
#include <unordered_map>
#include <utility>

namespace memory {

// MarkAndSweepCache implements a cache that uses a mark-and-sweep strategy
// to prune entries. It supports O(1) lookup and insertion.
// Thread safety: only safe to use from a single thread.
template <typename Key, typename Value>
class MarkAndSweepCache {
 private:
  // Wraps the cached value and the mark bit indicating whether the entry
  // was recently accessed.
  struct Entry {
    Value value;
    bool marked;

    Entry(Value val, bool m) : value(std::move(val)), marked(m) {}
  };

  std::unordered_map<Key, Entry> map_;

 public:
  MarkAndSweepCache() = default;

  // Returns the value associated with the key if it exists, and marks it as active.
  std::optional<Value> Find(const Key& key) {
    auto it = map_.find(key);
    if (it != map_.end()) {
      // Accessing the entry marks it so it survives the next sweep.
      it->second.marked = true;
      return it->second.value;
    }
    return std::nullopt;
  }

  // Inserts the value for the key if not present, and always marks the entry as active.
  bool Emplace(Key key, Value value) {
    auto [it, inserted] = map_.try_emplace(std::move(key), std::move(value), true);
    if (!inserted) {
      // If the entry already exists, we do not overwrite the value, but we do
      // mark it as active so it survives the next sweep.
      it->second.marked = true;
    }
    return inserted;
  }

  // Removes the entry for the key if present.
  // Returns true if an entry was removed, false otherwise.
  bool Erase(const Key& key) { return map_.erase(key) > 0; }

  // Removes all entries that have not been marked active since the last sweep.
  void Sweep() {
    for (auto it = map_.begin(); it != map_.end();) {
      if (!it->second.marked) {
        it = map_.erase(it);
      } else {
        // Unmark surviving entries so they must be accessed again to survive
        // the subsequent sweep.
        it->second.marked = false;
        ++it;
      }
    }
  }
};

}  // namespace memory

#endif  // SRC_DEVELOPER_MEMORY_METRICS_MAS_CACHE_H_
