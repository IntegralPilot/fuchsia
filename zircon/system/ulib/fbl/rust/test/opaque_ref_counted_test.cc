// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <atomic>

#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

namespace {

class TestCppRefCountedObject : public fbl::RefCounted<TestCppRefCountedObject> {
 public:
  explicit TestCppRefCountedObject(bool* destroyed) : destroyed_(destroyed) {}
  ~TestCppRefCountedObject() {
    std::atomic_ref<bool>(*destroyed_).store(true, std::memory_order_relaxed);
  }

 private:
  bool* destroyed_;
};

}  // namespace

extern "C" {

void* create_cpp_ref_counted_object(bool* destroyed) {
  auto obj = fbl::MakeRefCounted<TestCppRefCountedObject>(destroyed);
  return fbl::ExportToRawPtr(&obj);
}

void destroy_cpp_ref_counted_object(void* ptr) {
  delete static_cast<TestCppRefCountedObject*>(ptr);
}

}  // extern "C"
