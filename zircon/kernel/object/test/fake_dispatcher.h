// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT__TEST_FAKE_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT__TEST_FAKE_DISPATCHER_H_

#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <object/dispatcher.h>

// A Dispatcher-like class that tracks the number of calls to on_zero_handles()
// for testing purposes.
//
// This base class is available so that we can test that KernelHandle can
// properly upcast child->base RefPtrs.
class FakeDispatcherBase : public fbl::RefCounted<FakeDispatcherBase> {
 public:
  virtual ~FakeDispatcherBase() = default;

  uint32_t current_handle_count() const { return 0; }
  int on_zero_handles_calls() const { return on_zero_handles_calls_; }
  void on_zero_handles() { on_zero_handles_calls_++; }

 protected:
  FakeDispatcherBase() = default;

 private:
  int on_zero_handles_calls_ = 0;
};

class FakeDispatcher : public FakeDispatcherBase {
 public:
  static fbl::RefPtr<FakeDispatcher> Create() {
    fbl::AllocChecker ac;
    auto dispatcher = fbl::AdoptRef(new (&ac) FakeDispatcher());
    if (!ac.check()) {
      unittest_printf("Failed to allocate FakeDispatcher\n");
      return nullptr;
    }
    return dispatcher;
  }

  ~FakeDispatcher() override = default;

 private:
  FakeDispatcher() = default;
};

#endif  // ZIRCON_KERNEL_OBJECT__TEST_FAKE_DISPATCHER_H_
