// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_FBL_RUST_TEST_INTRUSIVE_CONTAINER_TEST_SUPPORT_H_
#define ZIRCON_SYSTEM_ULIB_FBL_RUST_TEST_INTRUSIVE_CONTAINER_TEST_SUPPORT_H_

#include <atomic>
#include <cstddef>
#include <memory>

#include <fbl/intrusive_double_list.h>
#include <fbl/intrusive_single_list.h>
#include <fbl/intrusive_wavl_tree.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

extern "C" {
void rust_recycle_shared_ref_object(void* ptr);
void rust_free_shared_unique_object(void* ptr);
}

namespace fbl {
namespace tests {
namespace interop {

struct SharedUniqueObject {
  int value;
  int GetKey() const { return value; }
  fbl::SinglyLinkedListNodeState<std::unique_ptr<SharedUniqueObject>> sll_node_state_;
  fbl::DoublyLinkedListNodeState<std::unique_ptr<SharedUniqueObject>> dll_node_state_;
  fbl::WAVLTreeNodeState<std::unique_ptr<SharedUniqueObject>> wavl_node_state_;
  bool allocated_in_rust = false;
  std::atomic<bool>* destruction_flag = nullptr;

  ~SharedUniqueObject() {
    if (destruction_flag) {
      destruction_flag->store(true, std::memory_order_relaxed);
    }
  }

  void operator delete(void* ptr) {
    auto obj = static_cast<SharedUniqueObject*>(ptr);
    if (obj->allocated_in_rust) {
      rust_free_shared_unique_object(ptr);
    } else {
      ::operator delete(ptr);
    }
  }
};

struct SharedRefObject : public fbl::RefCounted<SharedRefObject>,
                         public fbl::Recyclable<SharedRefObject> {
  int value;
  int GetKey() const { return value; }
  fbl::SinglyLinkedListNodeState<fbl::RefPtr<SharedRefObject>> sll_node_state_;
  fbl::DoublyLinkedListNodeState<fbl::RefPtr<SharedRefObject>> dll_node_state_;
  fbl::WAVLTreeNodeState<fbl::RefPtr<SharedRefObject>> wavl_node_state_;
  bool allocated_in_rust = false;
  std::atomic<bool>* destruction_flag = nullptr;

  ~SharedRefObject() {
    if (destruction_flag) {
      destruction_flag->store(true, std::memory_order_relaxed);
    }
  }

  void fbl_recycle() {
    if (allocated_in_rust) {
      rust_recycle_shared_ref_object(this);
    } else {
      delete this;
    }
  }
};

#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Winvalid-offsetof"
static_assert(offsetof(SharedUniqueObject, value) == 0,
              "SharedUniqueObject::value offset mismatch");
static_assert(offsetof(SharedUniqueObject, sll_node_state_) == 8,
              "SharedUniqueObject::sll_node_state_ offset mismatch");
static_assert(offsetof(SharedUniqueObject, dll_node_state_) == 16,
              "SharedUniqueObject::dll_node_state_ offset mismatch");
static_assert(offsetof(SharedUniqueObject, wavl_node_state_) == 32,
              "SharedUniqueObject::wavl_node_state_ offset mismatch");
static_assert(offsetof(SharedUniqueObject, allocated_in_rust) == 64,
              "SharedUniqueObject::allocated_in_rust offset mismatch");
static_assert(offsetof(SharedUniqueObject, destruction_flag) == 72,
              "SharedUniqueObject::destruction_flag offset mismatch");
static_assert(sizeof(SharedUniqueObject) == 80, "SharedUniqueObject size mismatch");

static_assert(offsetof(SharedRefObject, value) == 4, "SharedRefObject::value offset mismatch");
static_assert(offsetof(SharedRefObject, sll_node_state_) == 8,
              "SharedRefObject::sll_node_state_ offset mismatch");
static_assert(offsetof(SharedRefObject, dll_node_state_) == 16,
              "SharedRefObject::dll_node_state_ offset mismatch");
static_assert(offsetof(SharedRefObject, wavl_node_state_) == 32,
              "SharedRefObject::wavl_node_state_ offset mismatch");
static_assert(offsetof(SharedRefObject, allocated_in_rust) == 64,
              "SharedRefObject::allocated_in_rust offset mismatch");
static_assert(offsetof(SharedRefObject, destruction_flag) == 72,
              "SharedRefObject::destruction_flag offset mismatch");
static_assert(sizeof(SharedRefObject) == 80, "SharedRefObject size mismatch");
#pragma GCC diagnostic pop

}  // namespace interop
}  // namespace tests
}  // namespace fbl

#endif  // ZIRCON_SYSTEM_ULIB_FBL_RUST_TEST_INTRUSIVE_CONTAINER_TEST_SUPPORT_H_
