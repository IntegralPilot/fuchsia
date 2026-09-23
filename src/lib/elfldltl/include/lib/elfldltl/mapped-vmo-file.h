// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MAPPED_VMO_FILE_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MAPPED_VMO_FILE_H_

#include <lib/zx/result.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include "memory.h"

namespace elfldltl {

// elfldltl::MappedVmoFile provides the File and Memory APIs and most other
// features of elfldltl::DirectMemory (see <lib/elfldltl/memory.h>), but on
// a read-only mapping of a VMO's entire contents as the file contents.
//
// The object is default-constructible and move-only.  The Init() function uses
// an unowned VMO handle to set up the mapping but does not need the handle
// thereafter.  The mapping will be removed on the object's destruction.
class MappedVmoFile : public DirectMemory {
 public:
  MappedVmoFile() = default;

  MappedVmoFile(const MappedVmoFile&) = delete;

  MappedVmoFile(MappedVmoFile&& other) noexcept
      : DirectMemory(other.image(), other.base()),
        mapped_size_(other.mapped_size_),
        vmar_(other.vmar_->get()) {
    other.set_image({});
    other.mapped_size_ = 0;
  }

  MappedVmoFile& operator=(MappedVmoFile&& other) noexcept {
    auto old_image = image();
    set_image(other.image());
    other.set_image(old_image);
    set_base(other.base());
    std::swap(mapped_size_, other.mapped_size_);
    return *this;
  }

  // This initializes for read-only access to the whole VMO.
  // The Memory API considers the start of the VMO to be address zero.
  zx::result<> Init(zx::unowned_vmo vmo, zx::unowned_vmar vmar = zx::vmar::root_self());

  // This initializes for read-write access to the VMO of the given size.  The
  // base address for the Memory API to assign the beginning of the VMO can be
  // set with the optional third argument.
  zx::result<> InitMutable(zx::unowned_vmo vmo, size_t size, uintptr_t base = 0,
                           zx::unowned_vmar vmar = zx::vmar::root_self());

  ~MappedVmoFile();

  // The DirectMemory methods are non-const so that things tested with
  // DirectMemory don't assume const-ness that the Memory API doesn't require.
  // But they actually have const-safe behavior, and it's convenient for
  // MappedVmoFile to be usable as const.

  template <typename T>
  auto ReadFromFile(size_t offset) const {
    return const_cast<MappedVmoFile*>(this)->DirectMemory::ReadFromFile<T>(offset);
  }

  template <typename T, typename Allocator>
  auto ReadArrayFromFile(size_t offset, Allocator&& allocator, size_t count) const {
    return const_cast<MappedVmoFile*>(this)->DirectMemory::ReadArrayFromFile<T>(
        offset, std::forward<Allocator>(allocator), count);
  }

  template <typename T>
  auto ReadArray(uintptr_t ptr, size_t count) const {
    return const_cast<MappedVmoFile*>(this)->DirectMemory::ReadArray<T>(ptr, count);
  }

  template <typename T>
  auto ReadArray(uintptr_t ptr) const {
    return const_cast<MappedVmoFile*>(this)->DirectMemory::ReadArray<T>(ptr);
  }

 private:
  // Make this private so it can't be used.
  using DirectMemory::set_image;

  size_t mapped_size_ = 0;
  zx::unowned_vmar vmar_;
};

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MAPPED_VMO_FILE_H_
