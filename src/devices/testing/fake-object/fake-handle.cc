// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file

#include <lib/fake-object/object.h>
#include <lib/zx/result.h>
#include <zircon/rights.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <memory>
#include <mutex>

#include "src/devices/testing/fake-object/internal.h"

namespace fake_object {

__EXPORT
bool HandleTable::IsValidFakeHandle(zx_handle_t handle) {
  char prop_name[ZX_MAX_NAME_LEN] = {0};

  zx_status_t status =
      REAL_SYSCALL(zx_object_get_property)(handle, ZX_PROP_NAME, prop_name, sizeof(prop_name));
  if (status != ZX_OK) {
    return false;
  }

  uint64_t size;
  status = REAL_SYSCALL(zx_vmo_get_size)(handle, &size);
  if (status != ZX_OK || size != 0) {
    return false;
  }

  return (strncmp(prop_name, kFakeObjectPropName, ZX_MAX_NAME_LEN) == 0);
}

__EXPORT
zx::result<std::shared_ptr<Object>> HandleTable::Get(zx_handle_t handle) __TA_EXCLUDES(lock_) {
  std::lock_guard guard(lock_);
  auto iter = handles_.find(handle);
  if (iter == handles_.end()) {
    ftracef("handle = 0x%x, not found\n", handle);
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  auto& obj = handles_[handle];
  ftracef("handle = 0x%x, obj = %p, type = %u\n", handle, obj.get(), obj->type());
  return zx::success(obj);
}

__EXPORT
zx::result<zx_handle_t> HandleTable::Add(std::shared_ptr<Object> obj) {
  // Fake objects are represented as empty VMOs because:
  // 1. We need a simple object that will have minimal effect on the test environment
  // 2. We need a valid handle that can be by default transferred over a channel
  // 3. We need an object type whose handle rights by default allow reading/writing properties
  zx_handle_t handle = ZX_HANDLE_INVALID;
  zx_status_t status = REAL_SYSCALL(zx_vmo_create)(/*size=*/0, /*options=*/0, &handle);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  // Use this prop name as a way to validate this event object is backing a fake
  // object. This allows us to check validity at any point in a process's lifecycle,
  // including when it has begun tearing down various sorts of storage.
  status = REAL_SYSCALL(zx_object_set_property)(handle, ZX_PROP_NAME, kFakeObjectPropName,
                                                strlen(kFakeObjectPropName));
  if (status != ZX_OK) {
    return zx::error(status);
  }

  std::lock_guard guard(lock_);
  [[maybe_unused]] void* obj_ptr = obj.get();
  [[maybe_unused]] zx_obj_type_t type = obj->type();
  handles_[handle] = std::move(obj);
  ftracef("handle = 0x%x, obj = %p, type = %u\n", handle, obj_ptr, type);
  return zx::success(handle);
}

__EXPORT
zx::result<> HandleTable::Remove(zx_handle_t handle) {
  // Pull the object out of the handle table so that we can release the handle
  // table lock before running the object's dtor. This prevents issues like
  // deadlocks if the object asserts in its dtor as a test object may do.
  std::shared_ptr<Object> obj;
  {
    std::lock_guard guard(lock_);
    ftracef("handle = 0x%x, obj = %p, type = %u\n", handle, handles_[handle].get(),
            handles_[handle]->type());
    handles_.erase(handle);
  }

  return zx::ok();
}

__EXPORT
void HandleTable::Clear() {
  std::lock_guard guard(lock_);
  handles_.clear();
}

__EXPORT
void HandleTable::Dump() {
  std::lock_guard guard(lock_);
  printf("Fake Handle Table [size: %zu]:\n", handles_.size());
  for (auto& e : handles_) {
    printf("handle %#x (type: %u)", e.first, static_cast<uint32_t>(e.second->type()));
    printf("\n");
  }
}

}  // namespace fake_object
