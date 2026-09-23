// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_TESTING_FIDL_BOUND_SERVER_H_
#define LIB_DRIVER_POWER_CPP_TESTING_FIDL_BOUND_SERVER_H_

#include <lib/fidl/cpp/wire/channel.h>

namespace fdf_power::testing {

// Wrapper for a FIDL server implementation that manages its own binding.
template <typename ProtocolImpl, typename Protocol = typename ProtocolImpl::_EnclosingProtocol>
class FidlBoundServer : public ProtocolImpl {
 public:
  template <typename... Args>
  FidlBoundServer(async_dispatcher_t* dispatcher, fidl::ServerEnd<Protocol> server_end,
                  Args&&... args)
      : ProtocolImpl(std::forward<Args>(args)...),
        binding_(
            fidl::BindServer(dispatcher, std::move(server_end), this,
                             [](ProtocolImpl*, fidl::UnbindInfo, fidl::ServerEnd<Protocol>) {})) {}

  fidl::ServerBindingRef<Protocol>& binding() { return binding_; }

 private:
  fidl::ServerBindingRef<Protocol> binding_;
};

}  // namespace fdf_power::testing

#endif  // LIB_DRIVER_POWER_CPP_TESTING_FIDL_BOUND_SERVER_H_
