// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_NODE_REMOVER_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_NODE_REMOVER_H_

#include <lib/fit/function.h>

namespace driver_manager {
class NodeRemover {
 public:
  virtual void ShutdownAllDrivers(fit::callback<void()> callback) = 0;
  virtual void ShutdownPkgDrivers(fit::callback<void()> callback) = 0;
  virtual void SetOnRemovalTimeoutCallback(fit::callback<void()> callback) = 0;
  virtual void LeaseAllDriversForShutdown(fit::callback<void()> callback) = 0;
};

}  // namespace driver_manager
#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_NODE_REMOVER_H_
