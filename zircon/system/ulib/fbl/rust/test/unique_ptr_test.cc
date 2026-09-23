// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <atomic>

namespace {

class TestCppObject {
 public:
  explicit TestCppObject(bool* destroyed) : destroyed_(destroyed) {}
  ~TestCppObject() { std::atomic_ref<bool>(*destroyed_).store(true, std::memory_order_relaxed); }

 private:
  bool* destroyed_;
};

}  // namespace

extern "C" {

void* create_cpp_object(bool* destroyed) { return new TestCppObject(destroyed); }

void destroy_cpp_object(void* ptr) { delete static_cast<TestCppObject*>(ptr); }

}  // extern "C"
