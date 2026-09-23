// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "shared_vmo.h"

#include <cassert>

#include <perfetto/ext/tracing/core/shared_memory.h>

SharedVmo::SharedVmo(zx::vmo vmo, zx::vmar vmar, void* addr, size_t size)
    : vmo_(std::move(vmo)), vmar_(std::move(vmar)), addr_(addr), size_(size) {}

std::unique_ptr<perfetto::SharedMemory> SharedVmo::Factory::CreateSharedMemory(size_t size) {
  zx::vmo vmo;
  zx_status_t result = zx::vmo::create(size, 0, &vmo);
  if (result != ZX_OK) {
    return nullptr;
  }
  return AdoptVmo(std::move(vmo));
}

std::unique_ptr<SharedVmo> SharedVmo::AdoptVmo(zx::vmo vmo) {
  size_t vmo_size;
  zx_status_t result = vmo.get_stream_size(&vmo_size);
  if (result != ZX_OK) {
    return nullptr;
  }

  zx_vaddr_t vmar_base;
  zx::vmar vmar;
  result = zx::vmar::root_self()->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0, vmo_size,
                                           &vmar, &vmar_base);
  if (result != ZX_OK) {
    return nullptr;
  }

  zx_vaddr_t vmo_addr_out;
  result = vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, vmo_size, &vmo_addr_out);
  if (result != ZX_OK) {
    return nullptr;
  }
  return std::make_unique<SharedVmo>(std::move(vmo), std::move(vmar),
                                     reinterpret_cast<void*>(vmo_addr_out), vmo_size);
}

SharedVmo::~SharedVmo() { vmar_.unmap(reinterpret_cast<uintptr_t>(addr_), size_); }
const void* SharedVmo::start() const { return addr_; }
size_t SharedVmo::size() const { return size_; }
