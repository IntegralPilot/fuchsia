// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_HIDCTL_HIDCTL_H_
#define SRC_UI_INPUT_DRIVERS_HIDCTL_HIDCTL_H_

#include <fidl/fuchsia.hardware.hidbus/cpp/wire.h>
#include <fidl/fuchsia.hardware.hidctl/cpp/wire.h>
#include <lib/async/cpp/wait.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/device.h>
#include <lib/zx/socket.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <optional>

#include <ddktl/device.h>
#include <ddktl/protocol/empty-protocol.h>
#include <fbl/array.h>
#include <fbl/mutex.h>

namespace hidctl {

class HidCtl;
using DeviceType = ddk::Device<HidCtl, ddk::Messageable<fuchsia_hardware_hidctl::Device>::Mixin>;
class HidCtl : public DeviceType {
 public:
  HidCtl(zx_device_t* device);
  static zx_status_t Create(void* ctx, zx_device_t* parent);

  void DdkRelease();

  void MakeHidDevice(MakeHidDeviceRequestView request, MakeHidDeviceCompleter::Sync& completer);
};

class HidDevice : public ddk::Device<HidDevice, ddk::Initializable, ddk::Unbindable>,
                  public ddk::EmptyProtocol<ZX_PROTOCOL_HIDBUS>,
                  public fidl::WireServer<fuchsia_hardware_hidbus::Hidbus> {
 public:
  HidDevice(zx_device_t* device, const fuchsia_hardware_hidctl::wire::HidCtlConfig& config,
            fbl::Array<const uint8_t> report_desc, zx::socket data);

  void DdkRelease();
  void DdkInit(ddk::InitTxn txn);
  void DdkUnbind(ddk::UnbindTxn txn);

  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> ServeOutgoing();

  // fidl::WireServer<fuchsia_hardware_hidbus::Hidbus>:
  void Query(QueryCompleter::Sync& completer) override;
  void Start(StartCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void GetDescriptor(fuchsia_hardware_hidbus::wire::HidbusGetDescriptorRequest* request,
                     GetDescriptorCompleter::Sync& completer) override;
  void SetDescriptor(fuchsia_hardware_hidbus::wire::HidbusSetDescriptorRequest* request,
                     SetDescriptorCompleter::Sync& completer) override;
  void GetReport(fuchsia_hardware_hidbus::wire::HidbusGetReportRequest* request,
                 GetReportCompleter::Sync& completer) override;
  void SetReport(fuchsia_hardware_hidbus::wire::HidbusSetReportRequest* request,
                 SetReportCompleter::Sync& completer) override;
  void GetIdle(fuchsia_hardware_hidbus::wire::HidbusGetIdleRequest* request,
               GetIdleCompleter::Sync& completer) override;
  void SetIdle(fuchsia_hardware_hidbus::wire::HidbusSetIdleRequest* request,
               SetIdleCompleter::Sync& completer) override;
  void GetProtocol(GetProtocolCompleter::Sync& completer) override;
  void SetProtocol(fuchsia_hardware_hidbus::wire::HidbusSetProtocolRequest* request,
                   SetProtocolCompleter::Sync& completer) override;

 private:
  zx_status_t Recv(uint8_t* buffer, uint32_t capacity);
  void Stop();

  component::OutgoingDirectory outgoing_;
  std::optional<fidl::ServerBinding<fuchsia_hardware_hidbus::Hidbus>> binding_;
  std::atomic_bool started_ = false;

  fuchsia_hardware_hidbus::wire::HidBootProtocol boot_protocol_;
  fbl::Array<const uint8_t> report_desc_;
  static constexpr uint32_t mtu_ = 256;  // TODO: set this based on report_desc_

  fbl::Mutex lock_;
  zx::socket data_;
  void HandleData(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                  const zx_packet_signal_t* signal);
  async::WaitMethod<HidDevice, &HidDevice::HandleData> wait_handler_{this};
  std::array<uint8_t, mtu_> buf_;
};

}  // namespace hidctl

#endif  // SRC_UI_INPUT_DRIVERS_HIDCTL_HIDCTL_H_
