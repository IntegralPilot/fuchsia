// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ID_MAP_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ID_MAP_H_

#include <cstddef>
#include <functional>

#include <fbl/intrusive_hash_table.h>
#include <fbl/intrusive_single_list.h>

namespace display_coordinator {

// Helper for allowing structs which are identified by unique ids to be put in a hashmap.
template <typename PtrType, typename IdType>
class IdMappable {
 private:
  // Private forward-declarations to define the hash table.
  using IdMappableNodeState = fbl::SinglyLinkedListNodeState<PtrType>;
  struct IdMappableTraits {
    static IdMappableNodeState& node_state(IdMappable& node) { return node.id_mappable_state_; }
  };
  using HashTableLinkedListType = fbl::SinglyLinkedListCustomTraits<PtrType, IdMappableTraits>;

 public:
  using Map = fbl::HashTable</*KeyType=*/IdType, PtrType, /*BucketType=*/HashTableLinkedListType>;

  explicit IdMappable(IdType id) : id_(id) {}

  IdMappable(const IdMappable&) = delete;
  IdMappable(IdMappable&&) = delete;
  IdMappable& operator=(const IdMappable&) = delete;
  IdMappable& operator=(IdMappable&&) = delete;

  ~IdMappable() = default;

  IdType id() const { return id_; }

  static size_t GetHash(IdType id) { return std::hash<IdType>()(id); }
  IdType GetKey() const { return id_; }

 private:
  const IdType id_;

  IdMappableNodeState id_mappable_state_;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ID_MAP_H_
