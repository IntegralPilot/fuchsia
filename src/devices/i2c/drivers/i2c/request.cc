// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "request.h"

namespace i2c {

zx_status_t RequestConverter::Convert(
    fit::inline_function<fidl::ObjectView<fidl::VectorView<uint8_t>>(fidl::VectorView<uint8_t>&)>
        save_write_vector,
    std::span<fuchsia_hardware_i2cimpl::wire::I2cImplOp> out_ops) const {
  ZX_DEBUG_ASSERT(request_->transactions.size() == out_ops.size());

  auto op_it = out_ops.begin();
  size_t total_transfer_size = 0;
  for (const auto& transaction : request_->transactions) {
    if (!transaction.has_data_transfer()) {
      return ZX_ERR_INVALID_ARGS;
    }

    // Same address for all ops, since there is one address per channel.
    op_it->address = address_;
    op_it->stop = transaction.has_stop() && transaction.stop();

    auto& data_transfer = transaction.data_transfer();
    if (data_transfer.is_read_size()) {
      if (data_transfer.read_size() == 0 || data_transfer.read_size() > max_transfer_size_) {
        return ZX_ERR_INVALID_ARGS;
      }

      op_it->type =
          fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithReadSize(data_transfer.read_size());

      total_transfer_size += data_transfer.read_size();
    } else if (data_transfer.is_write_data()) {
      if (data_transfer.write_data().empty() ||
          data_transfer.write_data().size() > max_transfer_size_) {
        return ZX_ERR_INVALID_ARGS;
      }

      op_it->type = fuchsia_hardware_i2cimpl::wire::I2cImplOpType::WithWriteData(
          save_write_vector(data_transfer.write_data()));

      total_transfer_size += data_transfer.write_data().size();
    } else {
      return ZX_ERR_INVALID_ARGS;
    }

    if (total_transfer_size > fuchsia_hardware_i2c::kMaxTransferSize) {
      return ZX_ERR_OUT_OF_RANGE;
    }

    op_it++;
  }
  out_ops.back().stop = true;

  return ZX_OK;
}

zx_status_t Request::SaveRequest(const RequestConverter& converter) {
  ZX_DEBUG_ASSERT_MSG(ops_.empty(), "SaveRequest() called twice");

  ops_ =
      fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp>(arena_, converter.ops().size());

  const size_t write_count = std::ranges::count_if(
      converter.ops(), [](const fuchsia_hardware_i2c::wire::Transaction& transaction) {
        return transaction.has_data_transfer() && transaction.data_transfer().is_write_data();
      });
  write_vectors_ = fidl::VectorView<fidl::VectorView<uint8_t>>(arena_, write_count);

  fidl::VectorView<uint8_t>* write_vector_it = write_vectors_.begin();

  return converter.Convert(
      [this, &write_vector_it](fidl::VectorView<uint8_t>& write_vector) mutable {
        ZX_DEBUG_ASSERT(write_vector_it != nullptr && write_vector_it != write_vectors_.end());
        *write_vector_it = fidl::VectorView<uint8_t>(arena_, write_vector.get());
        return fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(write_vector_it++);
      },
      ops_.get());
}

}  // namespace i2c
