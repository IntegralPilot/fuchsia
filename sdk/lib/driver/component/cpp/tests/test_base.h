// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_TESTS_TEST_BASE_H_
#define LIB_DRIVER_COMPONENT_CPP_TESTS_TEST_BASE_H_

#include <fuchsia/io/cpp/fidl_test_base.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/fpromise/promise.h>

namespace fdf::testing {

struct FakeContext : public fpromise::context {
  fpromise::executor* executor() const override { return nullptr; }
  fpromise::suspended_task suspend_task() override { return fpromise::suspended_task(); }
};

class Directory : public fuchsia::io::testing::Directory_TestBase {
 public:
  using OpenHandler =
      fit::function<void(std::string path, fidl::InterfaceRequest<fuchsia::io::Node> object)>;

  void SetOpenHandler(OpenHandler open_handler) { open_handler_ = std::move(open_handler); }

 private:
  void Open(std::string path, fuchsia::io::Flags flags, fuchsia::io::Options options,
            zx::channel object) override {
    open_handler_(std::move(path), fidl::InterfaceRequest<fuchsia::io::Node>(std::move(object)));
  }

  void NotImplemented_(const std::string& name) override {
    printf("Not implemented: Directory::%s\n", name.data());
  }

  OpenHandler open_handler_;
};

zx::result<Namespace> CreateNamespace(fidl::ClientEnd<fuchsia_io::Directory> client_end);

}  // namespace fdf::testing

// TODO(https://fxbug.dev/42066155): remove this once migration from driver to fdf is complete.
namespace driver::testing {
using namespace fdf::testing;
}  // namespace driver::testing

#endif  // LIB_DRIVER_COMPONENT_CPP_TESTS_TEST_BASE_H_
