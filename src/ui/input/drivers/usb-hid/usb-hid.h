// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_INPUT_DRIVERS_USB_HID_USB_HID_H_
#define SRC_UI_INPUT_DRIVERS_USB_HID_USB_HID_H_

#include <fidl/fuchsia.hardware.hidbus/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb/cpp/fidl.h>
#include <fuchsia/hardware/usb/cpp/banjo.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/sync/completion.h>

#include <thread>

#include <fbl/condition_variable.h>
#include <fbl/mutex.h>
#include <usb-endpoint/usb-endpoint-client.h>
#include <usb/hid.h>
#include <usb/usb.h>

namespace usb_hid {

class UsbHidbus : public fdf::DriverBase2,
                  public fidl::WireServer<fuchsia_hardware_hidbus::Hidbus> {
 public:
  static constexpr std::string_view kDriverName = "usb_hid";
  static constexpr std::string_view kChildNodeName = "usb-hid";

  explicit UsbHidbus();
  ~UsbHidbus() override;

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  using fdf::DriverBase2::Start;

  // fidl::WireServer<fuchsia_hardware_hidbus::Hidbus> implementation.
  void Query(QueryCompleter::Sync& completer) override;
  void Start(
      fidl::WireServer<fuchsia_hardware_hidbus::Hidbus>::StartCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void GetDescriptor(fuchsia_hardware_hidbus::wire::HidbusGetDescriptorRequest* request,
                     GetDescriptorCompleter::Sync& completer) override;
  void SetDescriptor(fuchsia_hardware_hidbus::wire::HidbusSetDescriptorRequest* request,
                     SetDescriptorCompleter::Sync& completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
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
  zx_status_t UsbHidControl(uint8_t req_type, uint8_t request, uint16_t value, uint16_t index,
                            void* data, size_t length, size_t* out_length);
  zx_status_t UsbHidControlIn(uint8_t req_type, uint8_t request, uint16_t value, uint16_t index,
                              void* data, size_t length, size_t* out_length);
  zx_status_t UsbHidControlOut(uint8_t req_type, uint8_t request, uint16_t value, uint16_t index,
                               const void* data, size_t length, size_t* out_length);

  void UsbHidRelease();
  static void FindDescriptors(usb::Interface interface, const usb_hid_descriptor_t** hid_desc,
                              const usb_endpoint_descriptor_t** endptin,
                              const usb_endpoint_descriptor_t** endptout);

 private:
  void StopHidbus();

  void HandleInterrupt(fuchsia_hardware_usb_endpoint::Completion completion);
  void HandleBatchInterrupt(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  void SetReportComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);

  async::Loop dispatcher_loop_{&kAsyncLoopConfigNeverAttachToThread};

  std::optional<fidl::ServerBinding<fuchsia_hardware_hidbus::Hidbus>> binding_;
  std::atomic_bool started_ = false;

  std::optional<usb::InterfaceList> usb_interface_list_;

  // These pointers are valid as long as usb_interface_list_ is valid.
  const usb_hid_descriptor_t* hid_desc_ = nullptr;

  fidl::Arena<> arena_;
  fuchsia_hardware_hidbus::wire::HidInfo info_;

  ddk::UsbProtocolClient usb_;

  uint8_t interface_ = 0;
  usb_desc_iter_t desc_iter_ = {};
  size_t parent_req_size_ = 0;

  std::thread unbind_thread_;
  std::optional<SetReportCompleter::Async> set_report_completer_;

  // Interrupt endpoint
  usb::EndpointClient<UsbHidbus> ep_in_{usb::EndpointType::INTERRUPT, this,
                                        std::mem_fn(&UsbHidbus::HandleBatchInterrupt)};
  std::optional<usb::EndpointClient<UsbHidbus>> ep_out_;

  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_;
};

}  // namespace usb_hid

#endif  // SRC_UI_INPUT_DRIVERS_USB_HID_USB_HID_H_
