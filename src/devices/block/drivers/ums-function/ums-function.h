// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UMS_FUNCTION_UMS_FUNCTION_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UMS_FUNCTION_UMS_FUNCTION_H_

#include <endian.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>

#include <atomic>
#include <format>
#include <optional>
#include <queue>
#include <string>
#include <utility>

#include <usb-endpoint/usb-endpoint-client.h>
#include <usb/descriptors.h>
#include <usb/request-fidl.h>
#include <usb/ums.h>

namespace ums {
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

class UmsFunction : public fdf::DriverBase2,
                    public fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface> {
 public:
  static constexpr char kDriverName[] = "usb-ums-function";
  static constexpr uint32_t kBlockSize = 512;
  static constexpr size_t kStorageSize = 4L * 1024L * 1024L * 1024L;
  static constexpr uint64_t kBlockCount = kStorageSize / kBlockSize;
  static constexpr size_t kDataReqSize = 16384;
  static constexpr uint16_t kBulkMaxPacket = 512;

  explicit UmsFunction() : fdf::DriverBase2(kDriverName) {}
  ~UmsFunction() = default;

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  // fuchsia_hardware_usb_function::UsbFunctionInterface impl.
  void Control(ControlRequest& req, ControlCompleter::Sync& completer) override;
  void SetConfigured(SetConfiguredRequest& req, SetConfiguredCompleter::Sync& completer) override;
  void SetInterface(SetInterfaceRequest& req, SetInterfaceCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::error("Unknown method %ld", metadata.method_ordinal);
  }

 private:
  enum DataState {
    DATA_STATE_NONE,
    DATA_STATE_READ,
    DATA_STATE_WRITE,
    DATA_STATE_UNMAP,
    DATA_STATE_FAILED
  };
  friend struct std::formatter<DataState>;

  void RequestQueue(usb::FidlRequest* req);
  void InEpCallback(std::vector<fuchsia_hardware_usb_endpoint::Completion> completion);
  void OutEpCallback(std::vector<fuchsia_hardware_usb_endpoint::Completion> completion);

  zx::result<> ConfigureEndpoint(const usb_endpoint_info_descriptor_t& desc);

  // Main driver initialization.
  zx_status_t Init(fdf::DriverContext& context);

  void QueueData(usb::FidlRequest* req);
  void QueueCsw(uint8_t status, bool also_cbw = true);
  void ContinueTransfer();
  void StartTransferBlocks(DataState state, uint32_t transfer_blocks, uint64_t lba);
  void StartTransferBytes(DataState state, size_t transfer_bytes);
  zx::result<> CancelAll(usb::EndpointClient<UmsFunction>& ep);

  void HandleInquiry(ums_cbw_t* cbw);
  void HandleTestUnitReady(ums_cbw_t* cbw);
  void HandleRequestSense(ums_cbw_t* cbw);
  void HandleReadCapacity10(ums_cbw_t* cbw);
  void HandleReadCapacity16(ums_cbw_t* cbw);
  void HandleModeSense6(ums_cbw_t* cbw);
  void HandleRead10(ums_cbw_t* cbw);
  void HandleRead12(ums_cbw_t* cbw);
  void HandleRead16(ums_cbw_t* cbw);
  void HandleWrite10(ums_cbw_t* cbw);
  void HandleWrite12(ums_cbw_t* cbw);
  void HandleWrite16(ums_cbw_t* cbw);
  void HandleUnmap(ums_cbw_t* cbw);

  void HandleCbw(ums_cbw_t* cbw);
  void CbwComplete(fuchsia_hardware_usb_endpoint::Completion completion);
  void CswComplete(fuchsia_hardware_usb_endpoint::Completion completion);
  void DataComplete(fuchsia_hardware_usb_endpoint::Completion completion);

  bool IsInData() const { return current_cbw_.bmCBWFlags & USB_DIR_IN; }
  bool IsOutData() const { return !IsInData(); }

  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunction> function_;
  fidl::ServerBindingGroup<fuchsia_hardware_usb_function::UsbFunctionInterface> bindings_;
  bool configured_ = false;

  std::optional<usb::FidlRequest> cbw_req_;
  uint64_t cbw_vmo_id_;

  std::optional<usb::FidlRequest> data_in_req_;
  std::optional<usb::FidlRequest> data_out_req_;

  std::optional<usb::FidlRequest> csw_req_;
  bool csw_in_flight_ = false;
  uint64_t csw_vmo_id_;
  std::queue<uint8_t> csw_q_;

  // Command status wrapper (csw) and in-data to host.
  usb::EndpointClient<UmsFunction> in_ep_{usb::EndpointType::BULK, this,
                                          std::mem_fn(&UmsFunction::InEpCallback)};

  // Command block wrapper (cbw) and out-data from host.
  usb::EndpointClient<UmsFunction> out_ep_{usb::EndpointType::BULK, this,
                                           std::mem_fn(&UmsFunction::OutEpCallback)};

  fdf::SynchronizedDispatcher dispatcher_;

  struct Config {
    usb_interface_info_descriptor_t intf;
    usb_endpoint_info_descriptor_t out_ep;
    usb_endpoint_info_descriptor_t in_ep;
  };
  static inline Config config_ = {
      .intf =
          {
              .b_length = sizeof(usb_interface_info_descriptor_t),
              .b_descriptor_type = USB_DT_INTERFACE,
              //      .b_interface_number set later
              .b_alternate_setting = 0,
              .b_num_endpoints = 2,
              .b_interface_class = USB_CLASS_MSC,
              .b_interface_sub_class = USB_SUBCLASS_MSC_SCSI,
              .b_interface_protocol = USB_PROTOCOL_MSC_BULK_ONLY,
              .i_interface = 0,
          },
      .out_ep =
          {
              .b_length = sizeof(usb_endpoint_info_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              //      .b_endpoint_address set later
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
              .w_max_packet_size = htole16(UmsFunction::kBulkMaxPacket),
              .b_interval = 0,
          },
      .in_ep =
          {
              .b_length = sizeof(usb_endpoint_info_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              //      .b_endpoint_address set later
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
              .w_max_packet_size = htole16(UmsFunction::kBulkMaxPacket),
              .b_interval = 0,
          },
  };

  // vmo for backing storage
  static zx::vmo vmo_;
  void* storage_;

  // command we are currently handling
  ums_cbw_t current_cbw_ = {};
  // data transferred for the current command
  size_t data_length_ = 0;

  // state for data transfers
  DataState data_state_;
  // state for reads and writes
  zx_off_t data_offset_ = 0;
  size_t data_remaining_ = 0;

  std::atomic_bool active_;
  std::atomic_int pending_request_count_ = 0;
  std::optional<fdf::StopCompleter> stop_completer_;
};

}  // namespace ums

template <>
struct std::formatter<ums::UmsFunction::DataState> : std::formatter<std::string> {
  auto format(const ums::UmsFunction::DataState state, format_context& ctx) const {
    std::string fmt;
    switch (state) {
      case ums::UmsFunction::DataState::DATA_STATE_NONE:
        fmt = "None";
        break;
      case ums::UmsFunction::DataState::DATA_STATE_READ:
        fmt = "Read";
        break;
      case ums::UmsFunction::DataState::DATA_STATE_WRITE:
        fmt = "Write";
        break;
      case ums::UmsFunction::DataState::DATA_STATE_UNMAP:
        fmt = "Unmap";
        break;
      case ums::UmsFunction::DataState::DATA_STATE_FAILED:
        fmt = "Failed";
        break;
      default:
        fmt = "<unknown>";
        break;
    };
    return std::formatter<std::string>::format(fmt, ctx);
  }
};

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UMS_FUNCTION_UMS_FUNCTION_H_
