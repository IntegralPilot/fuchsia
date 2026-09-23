// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "intrusive_container_test_support.h"

namespace {

using SharedUniqueObject = fbl::tests::interop::SharedUniqueObject;
using SharedRefObject = fbl::tests::interop::SharedRefObject;

using UniqueTree = fbl::WAVLTree<int, std::unique_ptr<SharedUniqueObject>>;
using RefTree = fbl::WAVLTree<int, fbl::RefPtr<SharedRefObject>>;

}  // namespace

extern "C" {

// UniqueTree Helpers
void* cpp_create_unique_tree() { return new UniqueTree(); }
void cpp_destroy_unique_tree(void* tree) { delete static_cast<UniqueTree*>(tree); }
void cpp_unique_tree_insert(void* tree, void* item) {
  auto t = static_cast<UniqueTree*>(tree);
  auto it = std::unique_ptr<SharedUniqueObject>(static_cast<SharedUniqueObject*>(item));
  t->insert(std::move(it));
}
void* cpp_unique_tree_erase(void* tree, int key) {
  auto t = static_cast<UniqueTree*>(tree);
  auto it = t->erase(key);
  return it.release();
}
void* cpp_unique_tree_find(void* tree, int key) {
  auto t = static_cast<UniqueTree*>(tree);
  auto iter = t->find(key);
  if (iter.IsValid()) {
    return &(*iter);
  }
  return nullptr;
}
bool cpp_unique_tree_is_empty(void* tree) { return static_cast<UniqueTree*>(tree)->is_empty(); }

// RefTree Helpers
void* cpp_create_ref_tree() { return new RefTree(); }
void cpp_destroy_ref_tree(void* tree) { delete static_cast<RefTree*>(tree); }
void cpp_ref_tree_insert(void* tree, void* item) {
  auto t = static_cast<RefTree*>(tree);
  auto it = fbl::ImportFromRawPtr(static_cast<SharedRefObject*>(item));
  t->insert(std::move(it));
}
void* cpp_ref_tree_erase(void* tree, int key) {
  auto t = static_cast<RefTree*>(tree);
  auto it = t->erase(key);
  return fbl::ExportToRawPtr(&it);
}
void* cpp_ref_tree_find(void* tree, int key) {
  auto t = static_cast<RefTree*>(tree);
  auto iter = t->find(key);
  if (iter.IsValid()) {
    return &(*iter);
  }
  return nullptr;
}
bool cpp_ref_tree_is_empty(void* tree) { return static_cast<RefTree*>(tree)->is_empty(); }

}  // extern "C"
