// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "intrusive_container_test_support.h"

namespace {

using SharedUniqueObject = fbl::tests::interop::SharedUniqueObject;
using SharedRefObject = fbl::tests::interop::SharedRefObject;

using UniqueList = fbl::DoublyLinkedList<std::unique_ptr<SharedUniqueObject>>;
using RefList = fbl::DoublyLinkedList<fbl::RefPtr<SharedRefObject>>;

}  // namespace

extern "C" {

// UniqueList Helpers
void* cpp_create_unique_list() { return new UniqueList(); }
void cpp_destroy_unique_list(void* list) { delete static_cast<UniqueList*>(list); }
void cpp_unique_list_push_back(void* list, void* item) {
  auto l = static_cast<UniqueList*>(list);
  auto it = std::unique_ptr<SharedUniqueObject>(static_cast<SharedUniqueObject*>(item));
  l->push_back(std::move(it));
}
void* cpp_unique_list_pop_front(void* list) {
  auto l = static_cast<UniqueList*>(list);
  if (l->is_empty())
    return nullptr;
  return l->pop_front().release();
}
bool cpp_unique_list_is_empty(void* list) { return static_cast<UniqueList*>(list)->is_empty(); }

// RefList Helpers
void* cpp_create_ref_list() { return new RefList(); }
void cpp_destroy_ref_list(void* list) { delete static_cast<RefList*>(list); }
void cpp_ref_list_push_back(void* list, void* item) {
  auto l = static_cast<RefList*>(list);
  auto it = fbl::ImportFromRawPtr(static_cast<SharedRefObject*>(item));
  l->push_back(std::move(it));
}
void* cpp_ref_list_pop_front(void* list) {
  auto l = static_cast<RefList*>(list);
  if (l->is_empty())
    return nullptr;
  // pop_back/pop_front returns RefPtr. We need to release it to raw pointer.
  // RefPtr doesn't have release(), we use ExportToRawPtr.
  auto it = l->pop_front();
  return fbl::ExportToRawPtr(&it);
}
bool cpp_ref_list_is_empty(void* list) { return static_cast<RefList*>(list)->is_empty(); }

}  // extern "C"
