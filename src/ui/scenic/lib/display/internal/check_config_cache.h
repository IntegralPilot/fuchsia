// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_CHECK_CONFIG_CACHE_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_CHECK_CONFIG_CACHE_H_

#include <lib/syslog/cpp/macros.h>

#include <cstddef>
#include <functional>
#include <list>
#include <unordered_map>

#include "src/ui/scenic/lib/display/internal/display_equivalence.h"

namespace display::internal {

// Helper for `class BoundedLruCache`, to allow the std::unordered_map key to be a reference.
template <typename T>
struct ReferenceHasher {
  // Not necessary for `CheckConfigCache`, but useful if the key type should allow heteregeneous
  // lookup, e.g. both `const char*` and `std::string`.
  using is_transparent = void;

  size_t operator()(std::reference_wrapper<const T> key) const { return std::hash<T>{}(key.get()); }
  size_t operator()(const T& key) const { return std::hash<T>{}(key); }
};

// Helper for `class BoundedLruCache`, to allow the std::unordered_map key to be a reference.
template <typename T>
struct ReferenceEquality {
  // Not necessary for `CheckConfigCache`, but useful if the key type should allow heteregeneous
  // lookup, e.g. both `const char*` and `std::string`.
  using is_transparent = void;

  bool operator()(std::reference_wrapper<const T> lhs, std::reference_wrapper<const T> rhs) const {
    return lhs.get() == rhs.get();
  }
  bool operator()(std::reference_wrapper<const T> lhs, const T& rhs) const {
    return lhs.get() == rhs;
  }
};

// `BoundedLruCache` implements a bounded-size cache where the least recently used entry is evicted
// when a new one is added.  Accessing an existing entry, either by `Get()` or setting a new value
// with `Put()`, causes that entry to become the most recently used.
//
// This implementation is optimized for cases where the keys are quite large compared to the values
// (although it will work fine in the opposite case, too).  A naive implementation would store two
// copies of each key: one in `map_` for lookup and one in `lru_list_` for eviction.  Instead, the
// key exists only in `lru_list_`, and `map_` is keyed by a reference to that key.
//
// Both the key and value types must be copy-constructable.
//
// Thread-safety: This class is thread-unsafe; concurrent access must be externally synchronized.
template <typename K, typename V>
class BoundedLruCache {
 public:
  using Key = K;
  using Value = V;
  static_assert(std::is_copy_constructible_v<Key>);
  static_assert(std::is_copy_constructible_v<Value>);

  // Public so `Iterator` can be public.
  struct CacheNode {
    const Key key;
    Value value;
  };

  // Allows iteration of cache entries in MRU order; see `begin()`, `end()`.
  using Iterator = typename std::list<CacheNode>::const_iterator;

  explicit BoundedLruCache(size_t capacity) : capacity_(capacity) {
    FX_CHECK(capacity_ > 0) << capacity_;
  }

  // Not moveable, not copyable.
  BoundedLruCache(const BoundedLruCache& other) = delete;
  BoundedLruCache(BoundedLruCache&& other) = delete;
  BoundedLruCache& operator=(const BoundedLruCache& other) = delete;
  BoundedLruCache& operator=(BoundedLruCache&& other) = delete;

  void Put(const Key& key, const Value& value) {
    auto map_it = map_.find(key);

    // Key already exists: update value and move to front.
    if (map_it != map_.end()) {
      map_it->second->value = value;
      lru_list_.splice(lru_list_.begin(), lru_list_, map_it->second);
      return;
    }

    // Key is new.  Check for capacity and evict if necessary.
    if (lru_list_.size() == capacity_) {
      // The key to evict is in the node at the back of the list.
      const Key& lru_key = lru_list_.back().key;
      bool evicted_lru = map_.erase(lru_key);
      FX_DCHECK(evicted_lru);
      lru_list_.pop_back();
    }

    // Insert the new element at the front of the list
    lru_list_.push_front({key, value});

    // The key now lives in lru_list_.begin()->key.
    // Insert a reference to that key into the map.
    map_.emplace(std::cref(lru_list_.begin()->key), lru_list_.begin());
  }

  std::optional<Value> Get(const Key& key) {
    auto map_it = map_.find(key);
    if (map_it == map_.end()) {
      return std::nullopt;
    }

    // Move the accessed node to the front of the list
    lru_list_.splice(lru_list_.begin(), lru_list_, map_it->second);
    return map_it->second->value;
  }

  // Iterators in MRU (most recently used) order.
  Iterator begin() const { return lru_list_.begin(); }
  Iterator end() const { return lru_list_.end(); }

  size_t size() const {
    FX_DCHECK(map_.size() == lru_list_.size());
    return map_.size();
  }

 private:
  using MapKey = std::reference_wrapper<const Key>;
  using LruListIterator = typename std::list<CacheNode>::iterator;
  using HashMap =
      std::unordered_map<MapKey, LruListIterator, ReferenceHasher<Key>, ReferenceEquality<Key>>;

  const size_t capacity_;
  std::list<CacheNode> lru_list_;
  HashMap map_;
};

// Caches the results of `fuchsia.hardware.display.Coordinator/CheckConfig()`, so that it is
// unnecessary to make subsequent calls for equivalent configs (using the notion of equivalence
// defined by `DisplayEquivalence`).  Keeps track of which "equivs" were used most recently, in
// order to maintain a maximum cache size by trimming equivs that have not been used for a while.
using CheckConfigCache = BoundedLruCache<DisplayEquivalence, bool>;

}  // namespace display::internal

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_CHECK_CONFIG_CACHE_H_
