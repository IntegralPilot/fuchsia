// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_MICROBENCHMARKS_SCOPED_MAPPING_H_
#define SRC_TESTS_MICROBENCHMARKS_SCOPED_MAPPING_H_

#include <sys/mman.h>
#include <unistd.h>

#include "lib/syslog/cpp/macros.h"

// ScopedMapping returns an mmapped region of memory that gets unmapped when it
// goes out of scope.
class ScopedMapping {
 public:
  ScopedMapping(size_t size, int prot, int flags) : size_(size) {
    void* addr = mmap(NULL, size_, prot, flags, -1, 0);
    FX_CHECK(addr != MAP_FAILED);
    base_ = reinterpret_cast<uintptr_t>(addr);
  }

  ~ScopedMapping() { reset(); }

  ScopedMapping(const ScopedMapping&) = delete;
  ScopedMapping& operator=(const ScopedMapping&) = delete;

  uintptr_t base() const { return base_; }

  size_t size() const { return size_; }

 private:
  void reset() {
    if (base_ != 0 && size_ != 0) {
      FX_CHECK(munmap(reinterpret_cast<void*>(base_), size_) == 0);
      base_ = 0;
      size_ = 0;
    }
  }

  uintptr_t base_;
  size_t size_;
};

#endif  // SRC_TESTS_MICROBENCHMARKS_SCOPED_MAPPING_H_
