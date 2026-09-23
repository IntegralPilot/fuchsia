// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <fbl/recycler.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

extern "C" void rust_recycle_test_rust_ref_counted(void* ptr);

namespace {

class TestRustRefCounted : public fbl::RefCounted<TestRustRefCounted>,
                           public fbl::Recyclable<TestRustRefCounted> {
 public:
  void fbl_recycle() { rust_recycle_test_rust_ref_counted(this); }
};

}  // namespace

extern "C" {

void test_import_rust_ref_counted(void* ptr) {
  auto obj = fbl::ImportFromRawPtr(static_cast<TestRustRefCounted*>(ptr));
  // obj goes out of scope and drops its ref count.
}

}  // extern "C"
