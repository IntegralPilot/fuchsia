// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/heap.h>

#include <fbl/alloc_checker.h>

void* operator new(size_t size, fbl::AllocChecker& ac) noexcept {
  return fbl::internal::checked(size, ac, malloc(size));
}

void* operator new(size_t size, std::align_val_t align, fbl::AllocChecker& ac) noexcept {
  return fbl::internal::checked(size, ac, memalign(static_cast<size_t>(align), size));
}

void* operator new[](size_t size, fbl::AllocChecker& ac) noexcept {
  return fbl::internal::checked(size, ac, malloc(size));
}

void* operator new[](size_t size, std::align_val_t align, fbl::AllocChecker& ac) noexcept {
  return fbl::internal::checked(size, ac, memalign(static_cast<size_t>(align), size));
}
