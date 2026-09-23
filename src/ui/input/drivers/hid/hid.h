// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_HID_HID_H_
#define SRC_UI_INPUT_DRIVERS_HID_HID_H_

#include <fidl/fuchsia.hardware.hidbus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.hidbus/cpp/wire.h>
#include <fidl/fuchsia.hardware.input/cpp/wire.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/hid-parser/item.h>
#include <lib/hid-parser/parser.h>
#include <lib/hid-parser/usages.h>

#include <vector>

#include <fbl/intrusive_double_list.h>
#include <fbl/ref_ptr.h>

#include "hid-instance.h"

namespace hid_driver {

using input_report_id_t = uint8_t;
struct HidPageUsage {
  uint16_t page;
  uint32_t usage;

  friend bool operator<(const HidPageUsage& l, const HidPageUsage& r) {
    return std::tie(l.page, l.usage) < std::tie(r.page, r.usage);
  }
};

class HidDevice : public fidl::WireServer<fuchsia_hardware_input::Controller>,
                  public fidl::WireAsyncEventHandler<fuchsia_hardware_hidbus::Hidbus> {
 public:
  explicit HidDevice(fidl::ClientEnd<fuchsia_hardware_hidbus::Hidbus> hidbus)
      : hidbus_(std::move(hidbus), fdf::Dispatcher::GetCurrent()->async_dispatcher(), this) {}
  ~HidDevice() override {
    instance_list_.clear();
    ReleaseReassemblyBuffer();
    if (parsed_hid_desc_) {
      FreeDeviceDescriptor(parsed_hid_desc_);
    }
  }

  zx::result<std::vector<fuchsia_driver_framework::NodeProperty2>> Init();

  void OpenSession(OpenSessionRequestView request, OpenSessionCompleter::Sync& completer) override;

  // fidl::WireAsyncEventHandler<fuchsia_hardware_hidbus::Hidbus> Methods.
  void OnReportReceived(
      fidl::WireEvent<fuchsia_hardware_hidbus::Hidbus::OnReportReceived>* event) override;

  size_t GetMaxInputReportSize();

  size_t GetReportSizeById(input_report_id_t id, fuchsia_hardware_hidbus::wire::ReportType type);
  // Owned by HidDevice. Will be destructed when HidDevice is destructed.
  const fuchsia_hardware_hidbus::HidInfo& GetHidInfo() { return info_; }

  fidl::WireClient<fuchsia_hardware_hidbus::Hidbus>& GetHidbusProtocol() { return hidbus_; }

  zx::result<> CreateInstance(fidl::ServerEnd<fuchsia_hardware_input::Device> session);

  size_t GetReportDescLen() { return hid_report_desc_.size(); }
  const uint8_t* GetReportDesc() { return hid_report_desc_.data(); }

  void RemoveInstance(HidInstance& instance);

 private:
  zx_status_t InitReassemblyBuffer();
  void ReleaseReassemblyBuffer();
  zx_status_t SetReportDescriptor();

  fuchsia_hardware_hidbus::HidInfo info_;
  fidl::WireClient<fuchsia_hardware_hidbus::Hidbus> hidbus_;

  // Reassembly buffer for input events too large to fit in a single interrupt
  // transaction.
  uint8_t* rbuf_ = nullptr;
  size_t rbuf_size_ = 0;
  size_t rbuf_filled_ = 0;
  size_t rbuf_needed_ = 0;

  std::vector<uint8_t> hid_report_desc_;
  hid::DeviceDescriptor* parsed_hid_desc_ = nullptr;

  fbl::DoublyLinkedList<fbl::RefPtr<HidInstance>> instance_list_;
};

class HidDriver : public fdf::DriverBase2 {
 private:
  static constexpr char kDeviceName[] = "hid-device";

 public:
  explicit HidDriver()
      : fdf::DriverBase2(kDeviceName),
        devfs_connector_(fit::bind_member<&HidDriver::Serve>(this)) {}

  zx::result<> Start(fdf::DriverContext context) override;

  HidDevice& hiddev() { return *hiddev_; }

 private:
  void Serve(fidl::ServerEnd<fuchsia_hardware_input::Controller> server) {
    bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(server),
                         hiddev_.get(), fidl::kIgnoreBindingClosure);
  }

  std::unique_ptr<HidDevice> hiddev_;

  compat::SyncInitializedDeviceServer compat_server_;
  fidl::ServerBindingGroup<fuchsia_hardware_input::Controller> bindings_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  driver_devfs::Connector<fuchsia_hardware_input::Controller> devfs_connector_;
};

}  // namespace hid_driver

#endif  // SRC_UI_INPUT_DRIVERS_HID_HID_H_
