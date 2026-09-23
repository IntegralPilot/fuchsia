// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LOADINFO_MAPPED_MEMORY_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LOADINFO_MAPPED_MEMORY_H_

#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>
#include <variant>

#include "memory.h"

namespace elfldltl {

// elfldltl::LoadInfoMappedMemory<LoadInfo, ...> adapts a Memory object with
// "addresses" that are essentially file offsets (e.g. MappedVmoFile, MappedFdFile)
// into a Memory object with proper file-relative addresses, as expected by the
// Memory API.
//
// Provided a reference to a Memory object and the LoadInfo associated with it,
// LoadInfoMappedMemory translates a file-relative memory address `ptr` into
// the exact segment offset to read from in the underlying Memory object.
template <class LoadInfo, class Memory>
class LoadInfoMappedMemory : public DirectMemory {
 public:
  LoadInfoMappedMemory(const LoadInfoMappedMemory&) = default;

  LoadInfoMappedMemory(const LoadInfo& load_info, Memory& mem) : load_info_(load_info), mem_(mem) {}

  template <typename T>
  std::optional<std::span<const T>> ReadArray(uintptr_t ptr, size_t count) {
    return ReadArrayImpl<T>(ptr, count);
  }

  template <typename T>
  std::optional<std::span<const T>> ReadArray(uintptr_t ptr) {
    return ReadArrayImpl<T>(ptr, std::nullopt);
  }

 private:
  using size_type = typename LoadInfo::size_type;

  // Locate the segment that contains the `ptr` value in its `vaddr` range.
  // `read_underyling` calculates the target offset to begin to read from using
  // the segment's offset value, ensuring the `request_count` is within bounds.
  template <typename T>
  std::optional<std::span<const T>> ReadArrayImpl(uintptr_t ptr, std::optional<size_t> count) {
    auto target_vaddr = static_cast<size_type>(ptr);
    auto segment = load_info_.FindSegment(static_cast<size_type>(target_vaddr));
    if (segment == load_info_.segments().end()) {
      return std::nullopt;
    }

    auto read_underlying = [this, target_vaddr,
                            count](const auto& segment) -> std::optional<std::span<const T>> {
      const size_type offset_in_segment = target_vaddr - segment.vaddr();
      if (segment.filesz() < offset_in_segment) [[unlikely]] {
        return std::nullopt;
      }
      const size_t avail = static_cast<size_t>(segment.filesz() - offset_in_segment) / sizeof(T);
      const size_t request_count = count.value_or(avail);
      if (request_count <= avail) {
        return mem_.template ReadArray<T>(segment.offset() + offset_in_segment, request_count);
      }
      return std::nullopt;
    };

    return std::visit(read_underlying, *segment);
  }

  const LoadInfo& load_info_;
  Memory& mem_;
};

// Deduction guide.
template <class LoadInfo, class Memory>
LoadInfoMappedMemory(const LoadInfo&, Memory&) -> LoadInfoMappedMemory<LoadInfo, Memory>;

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LOADINFO_MAPPED_MEMORY_H_
