// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_I2C_DRIVERS_I2C_I2C_H_
#define SRC_DEVICES_I2C_DRIVERS_I2C_I2C_H_

#include <fidl/fuchsia.hardware.i2c.businfo/cpp/fidl.h>
#include <fidl/fuchsia.hardware.i2cimpl/cpp/driver/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>

#include <deque>
#include <optional>

#include "request.h"
#include "src/devices/i2c/drivers/i2c/i2c-child-server.h"

namespace i2c {

class I2cDriver : public fdf::DriverBase2 {
  using TransferRequestView = fidl::WireServer<fuchsia_hardware_i2c::Device>::TransferRequestView;
  using TransferCompleter = fidl::WireServer<fuchsia_hardware_i2c::Device>::TransferCompleter;

  static constexpr std::string_view kDriverName = "i2c";

 public:
  explicit I2cDriver() : fdf::DriverBase2(kDriverName) {
    impl_ops_.resize(kInitialOpCount);
    read_vectors_.resize(kInitialOpCount);
  }
  ~I2cDriver() override;

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  void Transact(uint16_t address, TransferRequestView request, TransferCompleter::Sync& completer);

 protected:
 private:
  static constexpr size_t kInitialOpCount = 16;
  // An arbitrary limit on the size of the request queue.
  static constexpr size_t kMaxTransactionsPerChild = 8;

  RequestStorage::Deleter GetCurrentRequestDeleter() {
    return [pending_requests = &pending_requests_](Request* request) {
      // We can't call erase() because `RequestStorage` isn't movable.
      ZX_DEBUG_ASSERT(request == &pending_requests->front());
      pending_requests->pop_front();
    };
  }

  void StartCurrentRequest();

  void CompleteRequest(fdf::WireUnownedResult<fuchsia_hardware_i2cimpl::Device::Transact>& result);

  zx::result<> AddI2cChildren(const fuchsia_hardware_i2c_businfo::I2CBusMetadata& metadata,
                              i2c_config::Config config,
                              const std::optional<std::string>& node_name,
                              const std::shared_ptr<fdf::Namespace>& incoming);

  uint64_t max_transfer_;

  // Ops and read vectors to be used in Transact(). Set to the initial capacities specified above;
  // more space is dynamically allocated if needed.
  std::vector<fuchsia_hardware_i2cimpl::wire::I2cImplOp> impl_ops_;
  std::vector<fidl::VectorView<uint8_t>> read_vectors_;

  fdf::WireClient<fuchsia_hardware_i2cimpl::Device> i2c_;

  std::vector<std::unique_ptr<I2cChildServer>> child_servers_;

  fidl::ClientEnd<fuchsia_driver_framework::Node> i2c_node_;

  std::deque<Request> pending_requests_;
  std::optional<RequestStorage> current_request_;
  bool shutdown_ = false;
};

}  // namespace i2c

#endif  // SRC_DEVICES_I2C_DRIVERS_I2C_I2C_H_
