// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/device.h>

#include <variant>

#include <ddktl/fidl.h>

namespace ddk {

device_fidl_txn_t IntoDeviceFIDLTransaction(fidl::Transaction* txn) {
  return {
      .driver_host_context = reinterpret_cast<uintptr_t>(txn),
  };
}

fidl::Transaction* FromDeviceFIDLTransaction(device_fidl_txn_t& txn) {
  // Invalidate the source transaction.
  uintptr_t raw = std::exchange(txn.driver_host_context, 0);
  ZX_ASSERT_MSG(raw != 0, "Reused a device_fidl_txn_t!");
  return reinterpret_cast<fidl::Transaction*>(raw);
}

}  // namespace ddk
