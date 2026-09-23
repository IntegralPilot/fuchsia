// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.i2c/cpp/fidl.h>
#include <fidl/fuchsia.hardware.i2cimpl/cpp/fidl.h>
#include <lib/fdf/cpp/arena.h>
#include <lib/fit/function.h>

#include <cstdint>
#include <span>
#include <variant>

#ifndef SRC_DEVICES_I2C_DRIVERS_I2C_REQUEST_H_
#define SRC_DEVICES_I2C_DRIVERS_I2C_REQUEST_H_

namespace i2c {

// Helper class for converting fuchsia.hardware.i2c requests into fuchsia.hardware.i2cimpl requests.
class RequestConverter {
 public:
  using TransferRequestView = fidl::WireServer<fuchsia_hardware_i2c::Device>::TransferRequestView;

  // `request` must outlive this instance.
  RequestConverter(TransferRequestView request, uint16_t address, uint64_t max_transfer_size)
      : request_(request), address_(address), max_transfer_size_(max_transfer_size) {}

  fidl::VectorView<fuchsia_hardware_i2c::wire::Transaction> ops() const {
    return request_->transactions;
  }

  // Converts the request into a series of i2cimpl operations to be used in a request to our parent.
  // `save_write_vector` is called for each write vector, and should be used to persist the write
  // data if needed. `out_ops` holds the converted operations.
  zx_status_t Convert(
      fit::inline_function<fidl::ObjectView<fidl::VectorView<uint8_t>>(fidl::VectorView<uint8_t>&)>
          save_write_vector,
      std::span<fuchsia_hardware_i2cimpl::wire::I2cImplOp> out_ops) const;

 private:
  const TransferRequestView request_;
  const uint16_t address_;
  const uint64_t max_transfer_size_;
};

// Represents an i2cimpl request and associated completer. The request data may be stored in the
// `Request` object itself or separately.
class Request {
 public:
  using TransferCompleter = fidl::WireServer<fuchsia_hardware_i2c::Device>::TransferCompleter;

  explicit Request(TransferCompleter::Async completer) : completer_(std::move(completer)) {}

  // Constructs a `Request` object with operations stored separately.
  Request(fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp> ops,
          TransferCompleter::Async completer)
      : ops_(ops), completer_(std::move(completer)) {
    ZX_DEBUG_ASSERT(!ops_.empty());
  }

  // `Request` objects cannot be copied or moved due to `arena_`.
  Request(const Request&) = delete;
  Request& operator=(const Request&) = delete;

  Request(Request&&) = delete;
  Request& operator=(Request&&) = delete;

  // Converts the request into a series of i2cimpl operations and saves them to this object.
  zx_status_t SaveRequest(const RequestConverter& converter);

  void Complete(fidl::VectorView<fidl::VectorView<uint8_t>> read_vectors) {
    completer_.ReplySuccess(read_vectors);
  }

  void Complete(zx_status_t status) { completer_.ReplyError(status); }

  fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp> ops() const {
    ZX_DEBUG_ASSERT_MSG(!ops_.empty(), "ops() called before SaveRequest");
    return ops_;
  }

 private:
  fidl::Arena<> arena_;
  fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp> ops_;
  fidl::VectorView<fidl::VectorView<uint8_t>> write_vectors_;
  TransferCompleter::Async completer_;
};

// Represents a `Request` object that is either stored locally or in a deque of requests.
class RequestStorage {
 public:
  using Deleter = fit::inline_function<void(Request*)>;

  // Constructs a `RequestStorage` object with a `Request` object stored locally.
  RequestStorage(fidl::VectorView<fuchsia_hardware_i2cimpl::wire::I2cImplOp> ops,
                 Request::TransferCompleter::Async completer)
      : arena_('I2CI'), storage_(std::in_place_type<Request>, ops, std::move(completer)) {}

  // Constructs a `RequestStorage` object with a `Request` object stored externally. `deleter` is
  // invoked with `request` when the `RequestStorage` object is destroyed.
  RequestStorage(Request* request, Deleter deleter)
      : arena_('I2CI'), storage_(request), deleter_(std::move(deleter)) {
    ZX_DEBUG_ASSERT(deleter_);
  }

  ~RequestStorage() {
    if (std::holds_alternative<Request*>(storage_)) {
      deleter_(std::get<Request*>(storage_));
    }
  }

  RequestStorage(const RequestStorage&) = delete;
  RequestStorage& operator=(const RequestStorage&) = delete;

  RequestStorage(RequestStorage&&) = delete;
  RequestStorage& operator=(RequestStorage&&) = delete;

  // Helper to access the underlying `Request`.
  Request* operator->() {
    if (std::holds_alternative<Request>(storage_)) {
      return &std::get<Request>(storage_);
    }
    if (std::holds_alternative<Request*>(storage_)) {
      return &*std::get<Request*>(storage_);
    }
    ZX_DEBUG_ASSERT(false);
    return nullptr;
  }

  fdf::Arena& arena() { return arena_; }

 private:
  fdf::Arena arena_;
  std::variant<Request, Request*> storage_;
  Deleter deleter_;
};

}  // namespace i2c

#endif  // SRC_DEVICES_I2C_DRIVERS_I2C_REQUEST_H_
