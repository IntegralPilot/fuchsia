// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "intrusive_container_test_support.h"

extern "C" {

using SharedUniqueObject = fbl::tests::interop::SharedUniqueObject;
using SharedRefObject = fbl::tests::interop::SharedRefObject;

// SharedUniqueObject Helpers
void* cpp_create_unique_object(int value, void* destruction_flag) {
  auto obj = new SharedUniqueObject();
  obj->value = value;
  obj->destruction_flag = static_cast<std::atomic<bool>*>(destruction_flag);
  return obj;
}
int cpp_get_unique_object_value(void* obj) { return static_cast<SharedUniqueObject*>(obj)->value; }
void cpp_destroy_unique_object(void* obj) { delete static_cast<SharedUniqueObject*>(obj); }

// SharedRefObject Helpers
void* cpp_create_ref_object(int value, void* destruction_flag) {
  auto ptr = fbl::AdoptRef(new SharedRefObject());
  ptr->value = value;
  ptr->destruction_flag = static_cast<std::atomic<bool>*>(destruction_flag);
  return fbl::ExportToRawPtr(&ptr);
}
int cpp_get_ref_object_value(void* obj) { return static_cast<SharedRefObject*>(obj)->value; }
void cpp_delete_ref_object(void* obj) { delete static_cast<SharedRefObject*>(obj); }

}  // extern "C"
