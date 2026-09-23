// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_DRIVERS_CONTROLLER_TEST_FAKE_SYSMEM_H_
#define SRC_CAMERA_DRIVERS_CONTROLLER_TEST_FAKE_SYSMEM_H_

#include <fidl/fuchsia.sysmem2/cpp/wire_test_base.h>
#include <fuchsia/sysmem/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/sys/cpp/component_context.h>

class FakeSysmem : public fidl::testing::WireTestBase<fuchsia_sysmem2::Allocator> {
 public:
  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Connect(fidl::ServerEnd<fuchsia_sysmem2::Allocator> request) {
    sysmem_bindings_.AddBinding(async_get_default_dispatcher(), std::move(request), this,
                                fidl::kIgnoreBindingClosure);
  }

 private:
  fidl::ServerBindingGroup<fuchsia_sysmem2::Allocator> sysmem_bindings_;
};

#endif  // SRC_CAMERA_DRIVERS_CONTROLLER_TEST_FAKE_SYSMEM_H_
