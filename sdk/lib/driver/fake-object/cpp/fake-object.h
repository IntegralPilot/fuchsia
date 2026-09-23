// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_FAKE_OBJECT_CPP_FAKE_OBJECT_H_
#define LIB_DRIVER_FAKE_OBJECT_CPP_FAKE_OBJECT_H_

#include <lib/zx/result.h>
#include <limits.h>
#include <stdio.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <memory>
#include <mutex>
#include <unordered_map>

namespace fake_object {

// This class is a fake Object base class that's provided to build fake
// derived object types from
// https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/zircon/system/ulib/zx/include/lib/zx/object.h.
//
// The library provides strong symbols for the following zx::Object syscalls so that they
// can be dispatched by the linker to virtual methods within the implemented Object:
// - zx_object_get_child()
// - zx_object_get_info()
// - zx_object_get_property()
// - zx_object_set_profile()
// - zx_object_set_property()
// - zx_object_signal()
// - zx_object_signal_peer()
// - zx_object_wait_one()
// - zx_object_wait_async()
// See https://fuchsia.dev/reference/syscalls for more information on syscalls.
//
// To use this library, create a derived class that inherits from FakeObject. The derived
// class can override the virtual methods associated with the syscall. Each virtual method
// maps to a syscall by appending zx_object_ to the function name. For example,
// the get_info() function in FakeObject maps to the zx_object_get_info() syscall.
//
// All objects derived from the FakeObject class need to be added to the FakeHandleTable singleton
// after they're instantiated. For example, say FakeBti is a derived class from FakeObject. After
// a FakeBti object is instantiated, the FakeHandleTable singleton is retrieved via
// FakeHandleTable() and the FakeBti object is added to it via its Add() function:
//
// std::shared_ptr<fake_object::FakeObject> new_bti = FakeBti::Create(paddrs, &new_bti);
// zx::result result = fake_object::FakeHandleTable().Add(std::move(new_bti));
//
class FakeObject {
 public:
  FakeObject() = delete;
  explicit FakeObject(zx_obj_type_t type) : type_(type) {}

  FakeObject(const FakeObject&) = delete;
  FakeObject& operator=(const FakeObject&) = delete;

  // For each object-related syscall we stub out a fake-specific version that
  // can be overridden and implemented by the derived fake objects. These functions
  // don't follow the C++ style guide since they need match the zx::Object class function
  // names defined in
  // https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/zircon/system/ulib/zx/include/lib/zx/object.h.
  virtual zx_status_t get_child(zx_handle_t /* handle */, uint64_t /* koid */,
                                zx_rights_t /* rights */, zx_handle_t* /* out */) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t get_info(zx_handle_t /* handle */, uint32_t /* topic */, void* /* buffer */,
                               size_t /* buffer_size */, size_t* /* actual_count */,
                               size_t* /* aval_count */) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t get_property(zx_handle_t /* handle */, uint32_t /* property */,
                                   void* /* value */, size_t /* value_size */) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t set_profile(zx_handle_t /* handle */, zx_handle_t /* profile */,
                                  uint32_t /* options */) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t set_property(zx_handle_t /* handle */, uint32_t /* property */,
                                   const void* /* value */, size_t /* value_size */) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t signal(zx_handle_t /* handle */, uint32_t /* clear_mask */,
                             uint32_t /* set_mask */) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t signal_peer(zx_handle_t /* handle */, uint32_t /* clear_mask */,
                                  uint32_t /* set_mask */) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t wait_one(zx_handle_t /* handle */, zx_signals_t /* signals */,
                               zx_time_t /* deadline */, zx_signals_t* /* observed */) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual zx_status_t wait_async(zx_handle_t /* handle */, zx_handle_t /* port */,
                                 uint64_t /* key */, zx_signals_t /* signals */,
                                 uint32_t /* options */) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  virtual ~FakeObject() = default;

  // For the purposes of tests we only need to ensure the koid is unique to the object.
  zx_koid_t get_koid() const { return reinterpret_cast<zx_koid_t>(this); }
  zx_obj_type_t type() const { return type_; }

 private:
  zx_obj_type_t type_;
};

// FakeHandleTable manipulates handle-related syscalls to use the fake handles
// vended by the fake-object library. All fake object instances are expected to be
// added to the FakeHandleTable singleton.
// See https://fuchsia.dev/fuchsia-src/concepts/kernel/handles for more details on handles.
class FakeHandleTable {
 public:
  FakeHandleTable() = default;
  ~FakeHandleTable() = default;

  FakeHandleTable(const FakeHandleTable&) = delete;
  FakeHandleTable& operator=(const FakeHandleTable&) = delete;
  FakeHandleTable(FakeHandleTable&&) = delete;
  FakeHandleTable& operator=(FakeHandleTable&&) = delete;

  // Returns true if |handle| routes to a fake object added to a FakeHandleTable. This function
  // is used decide whether the real or fake function should be called based on the handle.
  static bool IsValidFakeHandle(zx_handle_t handle);

  // Creates and adds a fake handle for |obj| into FakeHandleTable. |obj| is moved and stored
  // into FakeHandleTable.
  zx::result<zx_handle_t> Add(std::shared_ptr<FakeObject> obj) __TA_EXCLUDES(lock_);

  // Returns a shared pointer to the FakeObject that maps to |handle|.
  zx::result<std::shared_ptr<FakeObject>> Get(zx_handle_t handle) __TA_EXCLUDES(lock_);

  // Remove the handle from FakeHandleTable.
  zx::result<> Remove(zx_handle_t handle) __TA_EXCLUDES(lock_);

  // Removes all handles in FakeHandleTable.
  void Clear() __TA_EXCLUDES(lock_);

  // Walks the handle table and calls |cb| on each handle that matches the
  // provided |type|. Stops walking the table when |cb| returns false.
  //
  // |cb| must NOT attempt to acquire the lock, so this method is not suitable
  // for internal methods.
  template <typename ObjectCallback>
  void ForEach(zx_obj_type_t type, const ObjectCallback cb) __TA_EXCLUDES(lock_) {
    std::lock_guard lock(lock_);
    for (const auto& e : handles_) {
      if (e.second->type() == type) {
        if (!std::forward<const ObjectCallback>(cb)(e.second.get())) {
          break;
        }
      }
    }
  }

  // Prints information on all the handles in FakeHandleTable.
  void Dump() __TA_EXCLUDES(lock_);

  size_t size() __TA_EXCLUDES(lock_) {
    std::lock_guard lock(lock_);
    return handles_.size();
  }

 private:
  // This prop name is used as a way to validate if a handle is backing a fake
  // object. This allows us to check validity at any point in a process's lifecycle,
  // including when it has begun tearing down various sorts of storage.
  static constexpr const char kFakeObjectPropName[ZX_MAX_NAME_LEN] = "FAKEOBJECT";

  std::mutex lock_;
  std::unordered_map<zx_handle_t, std::shared_ptr<FakeObject>> handles_ __TA_GUARDED(lock_);
};

// Singleton accessor to a FakeHandleTable for tests and any derived fake object type. All
// fake object instances are expected to be added to this singleton.
FakeHandleTable& FakeHandleTable();

// Creates a base fake object and adds it to FakeHandleTable(). This is used to create basic fake
// objects for testing handle methods.
zx::result<zx_handle_t> CreateFakeObject(zx_obj_type_t type = ZX_OBJ_TYPE_NONE);

}  // namespace fake_object

#endif  // LIB_DRIVER_FAKE_OBJECT_CPP_FAKE_OBJECT_H_
