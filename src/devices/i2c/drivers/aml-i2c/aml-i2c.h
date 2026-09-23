// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_I2C_DRIVERS_AML_I2C_AML_I2C_H_
#define SRC_DEVICES_I2C_DRIVERS_AML_I2C_AML_I2C_H_

#include <fidl/fuchsia.hardware.i2c.businfo/cpp/fidl.h>
#include <fidl/fuchsia.hardware.i2cimpl/cpp/driver/wire.h>
#include <lib/async/cpp/irq.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/zx/event.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/time.h>

namespace aml_i2c {

class AmlI2c : public fdf::DriverBase2, public fdf::WireServer<fuchsia_hardware_i2cimpl::Device> {
 public:
  static constexpr std::string_view kDriverName = "aml-i2c";
  static constexpr std::string_view kChildNodeName = "aml-i2c";

  explicit AmlI2c() : fdf::DriverBase2(kDriverName) {}

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  // fdf::WireServer<fuchsia_hardware_i2cimpl::Device> implementation.
  void GetMaxTransferSize(fdf::Arena& arena, GetMaxTransferSizeCompleter::Sync& completer) override;
  void SetBitrate(SetBitrateRequestView request, fdf::Arena& arena,
                  SetBitrateCompleter::Sync& completer) override;
  void Transact(TransactRequestView request, fdf::Arena& arena,
                TransactCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_i2cimpl::Device> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  void SetTimeout(zx::duration timeout) { timeout_ = timeout; }

 protected:
  // Visible for testing
  virtual zx::result<fdf::MmioBuffer> MapMmio(fdf::PDev& pdev);

 private:
  zx_status_t ServeI2cImpl();
  zx_status_t CreateChildNode();

  void SetTargetAddr(uint16_t addr) const;
  void StartXfer() const;
  zx_status_t WaitTransferComplete() const;

  zx_status_t Read(cpp20::span<uint8_t> dst, bool stop) const;
  zx_status_t Write(cpp20::span<uint8_t> src, bool stop) const;

  zx_status_t StartIrqThread();
  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt);

  const fdf::MmioBuffer& regs_iobuff() const;

  zx::interrupt irq_;
  zx::event event_;
  std::optional<fdf::MmioBuffer> regs_iobuff_;
  zx::duration timeout_ = zx::sec(1);
  fdf::ServerBindingGroup<fuchsia_hardware_i2cimpl::Device> i2cimpl_bindings_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> child_controller_;
  // Only needed in order to set the role name for the code that waits for irq's.
  std::optional<fdf::Dispatcher> irq_dispatcher_;

  std::optional<fdf::StopCompleter> completer_;
  async::IrqMethod<AmlI2c, &AmlI2c::HandleIrq> irq_handler_{this};
  fdf_metadata::MetadataServer<fuchsia_hardware_i2c_businfo::I2CBusMetadata> metadata_server_;
};

}  // namespace aml_i2c

#endif  // SRC_DEVICES_I2C_DRIVERS_AML_I2C_AML_I2C_H_
