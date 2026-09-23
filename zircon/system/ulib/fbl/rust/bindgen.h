// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_FBL_RUST_BINDGEN_H_
#define ZIRCON_SYSTEM_ULIB_FBL_RUST_BINDGEN_H_

#include <stddef.h>

#include <fbl/canary.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/intrusive_single_list.h>
#include <fbl/intrusive_wavl_tree.h>
#include <fbl/ref_counted.h>

namespace fbl_bindgen {

struct DummyObject;

using Canary32 = fbl::Canary<0x12345678>;

struct CanaryContainer {
  fbl::Canary<0x12345678> canary;
};

struct SinglyNodeWrapper {
  fbl::SinglyLinkedListNodeState<DummyObject*> state;
};

struct DoublyNodeWrapper {
  fbl::DoublyLinkedListNodeState<DummyObject*> state;
};

struct WAVLNodeWrapper {
  fbl::WAVLTreeNodeState<DummyObject*> state;
};

struct RefCountedObject : public fbl::RefCounted<RefCountedObject> {
  int value;
};

}  // namespace fbl_bindgen

#endif  // ZIRCON_SYSTEM_ULIB_FBL_RUST_BINDGEN_H_
