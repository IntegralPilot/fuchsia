// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_TEST_REALM_SRC_ROOT_JOB_H_
#define LIB_DRIVER_TEST_REALM_SRC_ROOT_JOB_H_

#include <fidl/fuchsia.kernel/cpp/wire.h>

namespace driver_test_realm {

class RootJob final : public fidl::WireServer<fuchsia_kernel::RootJob> {
 public:
  void Serve(async_dispatcher_t* dispatcher, fidl::ServerEnd<fuchsia_kernel::RootJob> server_end) {
    bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
  }

  fidl::ProtocolHandler<fuchsia_kernel::RootJob> CreateHandler(async_dispatcher_t* dispatcher) {
    return bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure);
  }

  void Get(GetCompleter::Sync& completer) override;

 private:
  fidl::ServerBindingGroup<fuchsia_kernel::RootJob> bindings_;
};

}  // namespace driver_test_realm

#endif  // LIB_DRIVER_TEST_REALM_SRC_ROOT_JOB_H_
