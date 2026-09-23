// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DDKTL_INCLUDE_DDKTL_FIDL_H_
#define SRC_LIB_DDKTL_INCLUDE_DDKTL_FIDL_H_

#include <lib/ddk/device.h>
#include <lib/fidl/cpp/wire/transaction.h>

namespace ddk {

device_fidl_txn_t IntoDeviceFIDLTransaction(fidl::Transaction* txn);
fidl::Transaction* FromDeviceFIDLTransaction(device_fidl_txn_t& txn);

}  // namespace ddk

#endif  // SRC_LIB_DDKTL_INCLUDE_DDKTL_FIDL_H_
