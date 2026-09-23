// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "intrusive_container_test_support.h"

namespace {

using SharedUniqueObject = fbl::tests::interop::SharedUniqueObject;
using SharedRefObject = fbl::tests::interop::SharedRefObject;

using UniqueList = fbl::SinglyLinkedList<std::unique_ptr<SharedUniqueObject>>;
using RefList = fbl::SinglyLinkedList<fbl::RefPtr<SharedRefObject>>;

struct BaseItem {
  fbl::SinglyLinkedListNodeState<BaseItem*> sll_node_state_;
};
struct CppItem : public BaseItem {
  int value;
};
using OldListType = fbl::SinglyLinkedList<BaseItem*>;

}  // namespace

extern "C" {

// UniqueList Helpers
void* cpp_sll_create_unique_list() { return new UniqueList(); }
void cpp_sll_destroy_unique_list(void* list) { delete static_cast<UniqueList*>(list); }
void cpp_sll_unique_list_push_front(void* list, void* item) {
  auto l = static_cast<UniqueList*>(list);
  auto it = std::unique_ptr<SharedUniqueObject>(static_cast<SharedUniqueObject*>(item));
  l->push_front(std::move(it));
}
void* cpp_sll_unique_list_pop_front(void* list) {
  auto l = static_cast<UniqueList*>(list);
  if (l->is_empty())
    return nullptr;
  return l->pop_front().release();
}
bool cpp_sll_unique_list_is_empty(void* list) { return static_cast<UniqueList*>(list)->is_empty(); }

// RefList Helpers
void* cpp_sll_create_ref_list() { return new RefList(); }
void cpp_sll_destroy_ref_list(void* list) { delete static_cast<RefList*>(list); }
void cpp_sll_ref_list_push_front(void* list, void* item) {
  auto l = static_cast<RefList*>(list);
  auto it = fbl::ImportFromRawPtr(static_cast<SharedRefObject*>(item));
  l->push_front(std::move(it));
}
void* cpp_sll_ref_list_pop_front(void* list) {
  auto l = static_cast<RefList*>(list);
  if (l->is_empty())
    return nullptr;
  auto it = l->pop_front();
  return fbl::ExportToRawPtr(&it);
}
bool cpp_sll_ref_list_is_empty(void* list) { return static_cast<RefList*>(list)->is_empty(); }

// Old Raw Pointer SLL FFI Helpers (needed by singly_linked_list.rs::test_cross_lang_list)
void* create_cpp_list() { return new OldListType(); }
void destroy_cpp_list(void* list_ptr) { delete static_cast<OldListType*>(list_ptr); }
void* create_cpp_item(int value) {
  auto item = new CppItem();
  item->value = value;
  return item;
}
void destroy_cpp_item(void* item_ptr) { delete static_cast<CppItem*>(item_ptr); }
void list_push_front(void* list_ptr, void* item_ptr) {
  auto list = static_cast<OldListType*>(list_ptr);
  auto item = static_cast<BaseItem*>(item_ptr);
  list->push_front(item);
}
void* list_pop_front(void* list_ptr) {
  auto list = static_cast<OldListType*>(list_ptr);
  return list->pop_front();
}
bool list_is_empty(void* list_ptr) { return static_cast<OldListType*>(list_ptr)->is_empty(); }
int get_cpp_item_value(void* item_ptr) { return static_cast<CppItem*>(item_ptr)->value; }

}  // extern "C"
