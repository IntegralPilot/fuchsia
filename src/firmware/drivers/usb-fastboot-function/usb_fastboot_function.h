// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_DRIVERS_USB_FASTBOOT_FUNCTION_USB_FASTBOOT_FUNCTION_H_
#define SRC_FIRMWARE_DRIVERS_USB_FASTBOOT_FUNCTION_USB_FASTBOOT_FUNCTION_H_

#include <fidl/fuchsia.hardware.fastboot/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/inspect/cpp/inspect.h>
#include <zircon/compiler.h>

#include <usb-endpoint/usb-endpoint-client.h>
#include <usb-inspect/usb-inspect.h>
#include <usb/request-cpp.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

namespace usb_fastboot_function {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

// The higher the value of `kBulkRequestSize`, the higher the speed. But if set too high, the driver
// will start crashing more often due to memory error. Note that we allow up to 16 requests to be
// queued at a time, so we will consume up to 16 * 4k = 64KB of memory for each endpoint.
constexpr size_t kBulkRequestSize = 4ul * 1024;
constexpr size_t kPacketSize = 512;
constexpr size_t kMaxRequestCount = 16;

class UsbFastbootFunction
    : public fdf::DriverBase2,
      public fidl::WireServer<fuchsia_hardware_fastboot::FastbootImpl>,
      public fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface> {
 public:
  UsbFastbootFunction() : fdf::DriverBase2("usb_fastboot") {}

  virtual ~UsbFastbootFunction() = default;

  // Driver lifecycle methods.
  inspect::ComponentInspector& inspector() { return *inspector_; }

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  // For inspect test.
  zx::vmo inspect_vmo() { return inspector().inspector().DuplicateVmo(); }
  usb_inspect::ThroughputTracker& GetThroughputTrackerForTesting() { return *throughput_tracker_; }

  // UsbFunctionInterface methods.
  void Control(ControlRequest& request, ControlCompleter::Sync& completer) override;
  void SetConfigured(SetConfiguredRequest& request,
                     SetConfiguredCompleter::Sync& completer) override;
  void SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void Send(::fuchsia_hardware_fastboot::wire::FastbootImplSendRequest* request,
            SendCompleter::Sync& completer) override;
  void Receive(::fuchsia_hardware_fastboot::wire::FastbootImplReceiveRequest* request,
               ReceiveCompleter::Sync& completer) override;

  uint8_t bulk_out_addr() const { return descriptors_.bulk_out_ep.b_endpoint_address; }
  uint8_t bulk_in_addr() const { return descriptors_.bulk_in_ep.b_endpoint_address; }

 private:
  zx_status_t ConfigureEndpoints(bool enable);

  std::atomic<bool> configured_ = false;

  std::optional<inspect::ComponentInspector> inspector_;
  inspect::Node inspect_node_;
  usb_inspect::EndpointInspect bulk_in_inspect_;
  usb_inspect::EndpointInspect bulk_out_inspect_;
  std::optional<usb_inspect::ThroughputTracker> throughput_tracker_;

  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunction> function_;

  size_t total_to_send_ = 0;
  size_t sent_size_ = 0;
  size_t queued_tx_size_ = 0;
  fzl::OwnedVmoMapper send_vmo_;
  std::optional<SendCompleter::Async> send_completer_;
  // In-direction (TX to host).
  usb::EndpointClient<UsbFastbootFunction> bulk_in_ep_{
      usb::EndpointType::BULK, this, std::mem_fn(&UsbFastbootFunction::TxBatchComplete)};

  fzl::OwnedVmoMapper receive_vmo_;
  size_t received_size_ = 0;
  size_t requested_size_ = 0;
  size_t queued_rx_size_ = 0;
  std::optional<ReceiveCompleter::Async> receive_completer_;
  // Out-direction (RX from host).
  usb::EndpointClient<UsbFastbootFunction> bulk_out_ep_{
      usb::EndpointType::BULK, this, std::mem_fn(&UsbFastbootFunction::RxBatchComplete)};

  fidl::ServerBindingGroup<fuchsia_hardware_fastboot::FastbootImpl> bindings_;

  // USB request completion callback methods.
  void RxComplete(fuchsia_hardware_usb_endpoint::Completion completion);
  void TxComplete(fuchsia_hardware_usb_endpoint::Completion completion);
  void RxBatchComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  void TxBatchComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  void CleanUpRx(zx_status_t status, usb::FidlRequest req);
  void CleanUpTx(zx_status_t status, usb::FidlRequest req);
  void QueueTx();
  void QueueRx();

  // USB Fastboot interface descriptor.
  struct {
    usb_interface_descriptor_t fastboot_intf;
    usb_endpoint_descriptor_t bulk_out_ep;
    usb_endpoint_descriptor_t bulk_in_ep;

    // Fastboot tool checks only up to `bNumInterfaces` of interfaces in each USB device's
    // descriptor to see if it supports fastboot. One issue is that `alternate` interfaces don't
    // count in  `bNumInterfaces`. But it will still be listed in device descriptors. That is, when
    // device has `alternate` interfaces, total number of interfaces in device descriptor is
    // greater than `bNumInterfaces`. Fastboot tool doesn't know to skip them and therefore might
    // miss some of the interfaces.
    //
    // This is the case when fastboot is used with CDC Ethernet, which has an `alternate` interface
    // causing the fastboot tool to miss the fastboot interface. Therefore, we add a placeholder
    // interface as a workaround to increase `bNumInterfaces` by 1 so that it can cover right
    // at `fastboot_intf`.
    //
    // This should be changed if/when the fastboot CLI logic (ffx and upstream fastboot tool) knows
    // how to handle interface alt-configs.
    usb_interface_descriptor_t placehodler_intf;
  } descriptors_ [[maybe_unused]] = {
      .fastboot_intf =
          {
              .b_length = sizeof(usb_interface_descriptor_t),
              .b_descriptor_type = USB_DT_INTERFACE,
              .b_interface_number = 0,  // set later
              .b_alternate_setting = 0,
              .b_num_endpoints = 2,
              .b_interface_class = USB_CLASS_VENDOR,
              .b_interface_sub_class = USB_SUBCLASS_FASTBOOT,
              .b_interface_protocol = USB_PROTOCOL_FASTBOOT,
              .i_interface = 0,
          },
      .bulk_out_ep =
          {
              .b_length = sizeof(usb_endpoint_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              .b_endpoint_address = 0,  // set later during AllocEp
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
              .w_max_packet_size = htole16(uint16_t{kPacketSize}),
              .b_interval = 0,
          },
      .bulk_in_ep =
          {
              .b_length = sizeof(usb_endpoint_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              .b_endpoint_address = 0,  // set later during AllocEp
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
              .w_max_packet_size = htole16(uint16_t{kPacketSize}),
              .b_interval = 0,
          },
      .placehodler_intf =
          {
              .b_length = sizeof(usb_interface_descriptor_t),
              .b_descriptor_type = USB_DT_INTERFACE,
              .b_interface_number = 0,
              .b_alternate_setting = 0,
              .b_num_endpoints = 0,
              .b_interface_class = USB_CLASS_VENDOR,
              .b_interface_sub_class = 0,
              .b_interface_protocol = 0,
              .i_interface = 0,
          },
  };
};

}  // namespace usb_fastboot_function

#endif  // SRC_FIRMWARE_DRIVERS_USB_FASTBOOT_FUNCTION_USB_FASTBOOT_FUNCTION_H_
