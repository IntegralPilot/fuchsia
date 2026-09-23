// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_I2C_BIN_I2CUTIL2_H_
#define SRC_DEVICES_I2C_BIN_I2CUTIL2_H_

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>

#include <vector>

#include "args.h"

namespace i2cutil {

constexpr size_t kMaxTransactionCount = fuchsia_hardware_i2c::wire::kMaxCountTransactions;

zx_status_t execute(fidl::ClientEnd<fuchsia_hardware_i2c::Device> client,
                    std::vector<i2cutil::TransactionData>& transactions);

}  // namespace i2cutil

#endif  // SRC_DEVICES_I2C_BIN_I2CUTIL2_H_
