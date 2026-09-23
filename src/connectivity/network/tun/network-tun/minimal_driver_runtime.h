// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_MINIMAL_DRIVER_RUNTIME_H_
#define SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_MINIMAL_DRIVER_RUNTIME_H_

#include <lib/zx/result.h>

#include <memory>

namespace network::tun {

class MinimalDriverRuntime {
 public:
  ~MinimalDriverRuntime();

  static zx::result<std::unique_ptr<MinimalDriverRuntime>> Create();

 private:
  MinimalDriverRuntime() = default;
};

}  // namespace network::tun

#endif  // SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_MINIMAL_DRIVER_RUNTIME_H_
