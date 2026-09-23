// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "i2cutil2.h"

#include <span>

namespace {

zx_status_t execute_paginated(fidl::ClientEnd<fuchsia_hardware_i2c::Device>& client,
                              cpp20::span<i2cutil::TransactionData> transactions) {
  if (transactions.size() > i2cutil::kMaxTransactionCount) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  fidl::Arena arena;
  auto i2c_transactions =
      fidl::VectorView<fuchsia_hardware_i2c::wire::Transaction>(arena, transactions.size());

  for (size_t i = 0; i < transactions.size(); i++) {
    switch (transactions[i].type) {
      case i2cutil::TransactionType::Read: {
        i2c_transactions[i] =
            fuchsia_hardware_i2c::wire::Transaction::Builder(arena)
                .data_transfer(fuchsia_hardware_i2c::wire::DataTransfer::WithReadSize(
                    static_cast<uint32_t>(transactions[i].count)))
                .Build();
        break;
      }
      case i2cutil::TransactionType::Write: {
        auto write_data = fidl::VectorView<uint8_t>::FromExternal(transactions[i].bytes);
        i2c_transactions[i] =
            fuchsia_hardware_i2c::wire::Transaction::Builder(arena)
                .data_transfer(
                    fuchsia_hardware_i2c::wire::DataTransfer::WithWriteData(arena, write_data))
                .Build();
        break;
      }
    }
  }

  // Execute the list of transactions.
  const fidl::WireResult result = fidl::WireCall(client)->Transfer(i2c_transactions);
  if (!result.ok()) {
    fprintf(stderr, "i2cutil: error executing transaction: %s\n", result.status_string());
    return result.status();
  }

  const fit::result response = result.value();
  if (response.is_error()) {
    fprintf(stderr, "i2cutil: error executing transaction: %d\n", response.error_value());
    return response.error_value();
  }

  // Fill in any reads that were requested in the transaction list.
  size_t read_txn_num = 0;
  for (size_t i = 0; i < transactions.size(); i++) {
    if (transactions[i].type == i2cutil::TransactionType::Read) {
      std::copy(response->read_data[read_txn_num].begin(), response->read_data[read_txn_num].end(),
                std::back_inserter(transactions[i].bytes));
      if (transactions[i].bytes.size() != transactions[i].count) {
        fprintf(stderr, "warning: requested %lu bytes, got %lu\n", transactions[i].count,
                transactions[i].bytes.size());
      }
      read_txn_num++;
    }
  }

  return ZX_OK;
}

}  // namespace

namespace i2cutil {

zx_status_t execute(fidl::ClientEnd<fuchsia_hardware_i2c::Device> client,
                    std::vector<i2cutil::TransactionData>& transactions) {
  for (size_t i = 0; i < transactions.size(); i += kMaxTransactionCount) {
    const size_t start = i;
    const size_t count = std::min(kMaxTransactionCount, transactions.size() - i);
    zx_status_t result = execute_paginated(client, {transactions.data() + start, count});
    if (result != ZX_OK) {
      return result;
    }
  }

  return ZX_OK;
}

}  // namespace i2cutil
