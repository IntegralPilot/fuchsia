// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_ITEMS_CPU_TOPOLOGY_H_
#define SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_ITEMS_CPU_TOPOLOGY_H_

#include <lib/fit/result.h>
#include <lib/zbi-format/cpu.h>
#include <lib/zbi-format/internal/deprecated-cpu.h>

#include <span>
#include <variant>

#include "../storage-traits.h"

namespace zbitl {

// CpuTopologyTable encodes the ZBI description of a CPU topology (per
// ZBI_TYPE_CPU_TOPOLOGY). Its main utility lies in provided backwards
// compatibility with arm bootloaders that pass the deprecated
// ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1 or ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2
// types.
class CpuTopologyTable {
 public:
  class iterator;

  // Create a CpuTopologyTable from a ZBI item payload, which may be either
  // ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1 or ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2.
  static fit::result<std::string_view, CpuTopologyTable> FromPayload(uint32_t item_type,
                                                                     ByteView payload);

  template <class Iterator>
  static fit::result<std::string_view, CpuTopologyTable> FromItem(Iterator it) {
    auto [header, payload] = *it;
    return FromPayload(header->type, payload);
  }

  // Return the number of zbi_topology_node_v2_t entries in the table.
  size_t size() const;

  size_t size_bytes() const { return size() * sizeof(zbi_topology_node_v2_t); }

  iterator begin() const;

  iterator end() const;

 private:
  class V1ConvertingIterator;
  class V2ConvertingIterator;
  struct Dispatch;

  std::variant<std::span<const zbi_topology_node_t>,     //
               std::span<const zbi_topology_node_v2_t>,  //
               const zbi_cpu_config_t*>
      table_;
};

class CpuTopologyTable::V1ConvertingIterator {
 public:
  bool operator==(const V1ConvertingIterator& other) const {
    return logical_id_ == other.logical_id_;
  }

  bool operator!=(const V1ConvertingIterator& other) const { return !(*this == other); }

  V1ConvertingIterator& operator++();  // prefix

  V1ConvertingIterator operator++(int) {  // postfix
    V1ConvertingIterator old = *this;
    ++*this;
    return old;
  }

  zbi_topology_node_t operator*() const;

 private:
  friend Dispatch;

  std::span<const zbi_cpu_cluster_t> clusters_;
  size_t next_node_idx_ = 0;
  size_t cluster_node_idx_ = 0;
  uint8_t cluster_idx_ = 0;
  std::optional<uint8_t> cpu_idx_;
  std::optional<uint8_t> logical_id_;
};

class CpuTopologyTable::V2ConvertingIterator {
 public:
  bool operator==(const V2ConvertingIterator& other) const { return idx_ == other.idx_; }

  bool operator!=(const V2ConvertingIterator& other) const { return !(*this == other); }

  V2ConvertingIterator& operator++() {  // prefix
    ZX_ASSERT_MSG(idx_, "cannot increment default-constructed iterator");
    ZX_ASSERT_MSG(*idx_ < v2_nodes_.size(), "cannot increment end iterator");
    ++*idx_;
    return *this;
  }

  V2ConvertingIterator operator++(int) {  // postfix
    V2ConvertingIterator old = *this;
    ++*this;
    return old;
  }

  zbi_topology_node_t operator*() const;

 private:
  friend Dispatch;

  std::span<const zbi_topology_node_v2_t> v2_nodes_;
  std::optional<size_t> idx_;
};

class CpuTopologyTable::iterator {
 public:
  using iterator_category = std::input_iterator_tag;
  using value_type = zbi_topology_node_t;
  using reference = zbi_topology_node_t&;
  using pointer = zbi_topology_node_t*;
  using difference_type = ptrdiff_t;

  bool operator==(const iterator& other) const { return it_ == other.it_; }
  bool operator!=(const iterator& other) const { return it_ != other.it_; }

  iterator& operator++() {  // prefix
    std::visit([](auto& it) { ++it; }, it_);
    return *this;
  }

  iterator operator++(int) {  // postfix
    iterator old = *this;
    ++*this;
    return old;
  }

  zbi_topology_node_t operator*() const {
    return std::visit([](const auto& it) { return *it; }, it_);
  }

 private:
  friend Dispatch;

  std::variant<std::span<const zbi_topology_node_t>::iterator, V1ConvertingIterator,
               V2ConvertingIterator>
      it_;
};

}  // namespace zbitl

#endif  // SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_ITEMS_CPU_TOPOLOGY_H_
