// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_ETHERNET_DRIVERS_RNDIS_FUNCTION_RNDIS_FUNCTION_H_
#define SRC_CONNECTIVITY_ETHERNET_DRIVERS_RNDIS_FUNCTION_RNDIS_FUNCTION_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.network/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/fit/function.h>
#include <lib/inspect/cpp/inspect.h>

#include <array>
#include <queue>

#include <fbl/mutex.h>
#include <usb-endpoint/usb-endpoint-client.h>
#include <usb-inspect/usb-inspect.h>
#include <usb/cdc.h>
#include <usb/request-fidl.h>
#include <usb/usb.h>

#include "src/connectivity/ethernet/lib/rndis/rndis.h"
#include "src/lib/vmo_store/vmo_store.h"

namespace fnetdev = fuchsia_hardware_network_driver;

class RndisFunction : public fdf::DriverBase2,
                      public fdf::WireServer<fnetdev::NetworkDeviceImpl>,
                      public fdf::WireServer<fnetdev::NetworkPort>,
                      public fdf::WireServer<fnetdev::MacAddr>,
                      public fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface> {
 public:
  static constexpr std::string_view kDriverName = "rndis-function";
  static constexpr std::string_view kChildNodeName = "rndis-function";
  static constexpr size_t kEthMacSize = 6;
  static constexpr size_t kNotificationMaxPacketSize = 8;
  static constexpr size_t kRequestPoolSize = 8;
  static constexpr size_t kMtu = RNDIS_MAX_XFER_SIZE - sizeof(rndis_packet_header);

  static constexpr uint32_t kVendorId = 0x44070b00;
  static constexpr char kVendorDescription[] = "Google";
  static constexpr uint16_t kVendorDriverVersionMajor = 1;
  static constexpr uint16_t kVendorDriverVersionMinor = 0;
  static constexpr uint8_t kPortId = 1;

  RndisFunction()
      : fdf::DriverBase2(kDriverName),
        vmo_store_({
            .map =
                vmo_store::MapOptions{
                    .vm_option = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_REQUIRE_NON_RESIZABLE,
                    .vmar = nullptr,
                },
        }) {}

  using fdf::DriverBase2::Start;
  using fdf::DriverBase2::Stop;

  inspect::ComponentInspector &inspector() { return *inspector_; }
  usb_inspect::ThroughputTracker &GetThroughputTrackerForTesting() { return *throughput_tracker_; }

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  // UsbFunctionInterface methods.
  void Control(ControlRequest &request, ControlCompleter::Sync &completer) override;
  void SetConfigured(SetConfiguredRequest &request,
                     SetConfiguredCompleter::Sync &completer) override;
  void SetInterface(SetInterfaceRequest &request, SetInterfaceCompleter::Sync &completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
      fidl::UnknownMethodCompleter::Sync &completer) override;

  // NetworkDeviceImpl protocol:
  void Init(InitRequestView request, fdf::Arena &arena, InitCompleter::Sync &completer) override;
  void Start(fdf::Arena &arena, StartCompleter::Sync &completer) override;
  void Stop(fdf::Arena &arena, StopCompleter::Sync &completer) override;
  void GetInfo(
      fdf::Arena &arena,
      fdf::WireServer<fnetdev::NetworkDeviceImpl>::GetInfoCompleter::Sync &completer) override;
  void QueueTx(QueueTxRequestView request, fdf::Arena &arena,
               QueueTxCompleter::Sync &completer) override;
  void QueueRxSpace(QueueRxSpaceRequestView request, fdf::Arena &arena,
                    QueueRxSpaceCompleter::Sync &completer) override;
  void PrepareVmo(PrepareVmoRequestView request, fdf::Arena &arena,
                  PrepareVmoCompleter::Sync &completer) override;
  void ReleaseVmo(ReleaseVmoRequestView request, fdf::Arena &arena,
                  ReleaseVmoCompleter::Sync &completer) override;

  // NetworkPort protocol:
  void GetInfo(fdf::Arena &arena,
               fdf::WireServer<fnetdev::NetworkPort>::GetInfoCompleter::Sync &completer) override;
  void GetStatus(fdf::Arena &arena, GetStatusCompleter::Sync &completer) override;
  void SetActive(SetActiveRequestView request, fdf::Arena &arena,
                 SetActiveCompleter::Sync &completer) override;
  void GetMac(fdf::Arena &arena, GetMacCompleter::Sync &completer) override;
  void Removed(fdf::Arena &arena, RemovedCompleter::Sync &completer) override;

  // MacAddr protocol:
  void GetAddress(fdf::Arena &arena, GetAddressCompleter::Sync &completer) override;
  void GetFeatures(fdf::Arena &arena, GetFeaturesCompleter::Sync &completer) override;
  void SetMode(SetModeRequestView request, fdf::Arena &arena,
               SetModeCompleter::Sync &completer) override;

  uint8_t NotificationAddress() { return descriptors_.notification_ep.b_endpoint_address; }
  uint8_t BulkInAddress() { return descriptors_.in_ep.b_endpoint_address; }
  uint8_t BulkOutAddress() { return descriptors_.out_ep.b_endpoint_address; }

 private:
  zx_status_t HandleCommand(const void *buffer, size_t size);
  zx::result<std::vector<uint8_t>> HandleResponse(size_t size);
  zx_status_t Halt();
  void Reset();

  std::optional<std::vector<uint8_t>> QueryOid(uint32_t oid, void *input, size_t length);
  zx_status_t SetOid(uint32_t oid, const uint8_t *input, size_t length);

  void ContinueStop();
  void CancelAllRequests();
  void Notify();
  void IndicateConnectionStatus(bool connected);

  void DiscardPendingTxBuffers(zx_status_t status);
  void ReturnPendingRxSpace();

  fuchsia_hardware_network::PortStatus ReadStatus() const;

  void UpdatePortStatus();

  bool Online() const { return netdevice_ifc_.is_valid() && rndis_ready_; }

  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};

  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunction> function_;

  fdf::WireSharedClient<fnetdev::NetworkDeviceIfc> netdevice_ifc_;
  bool rndis_ready_ = false;
  bool shutting_down_ = false;
  uint32_t link_speed_ = 0;
  std::array<uint8_t, kEthMacSize> mac_addr_;

  // Stats.
  std::atomic<uint32_t> transmit_ok_ = 0;
  std::atomic<uint32_t> receive_ok_ = 0;
  std::atomic<uint32_t> transmit_errors_ = 0;
  std::atomic<uint32_t> receive_errors_ = 0;
  std::atomic<uint32_t> transmit_no_buffer_ = 0;

  std::queue<std::vector<uint8_t>> control_responses_;

  void NotifyComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  void RxComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  void TxComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  void ProcessRxCompletions(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);

  struct EndpointInfo {
    std::string_view name;
    uint8_t address;
    usb::EndpointClient<RndisFunction> &ep;
  };

  std::array<EndpointInfo, 3> GetEndpoints() {
    return {{
        {.name = "bulk in", .address = BulkInAddress(), .ep = bulk_in_ep_},
        {.name = "bulk out", .address = BulkOutAddress(), .ep = bulk_out_ep_},
        {.name = "notification", .address = NotificationAddress(), .ep = notification_ep_},
    }};
  }

  std::optional<fdf::StopCompleter> stop_completer_;

  // In-direction (TX to host).
  usb::EndpointClient<RndisFunction> notification_ep_{usb::EndpointType::INTERRUPT, this,
                                                      std::mem_fn(&RndisFunction::NotifyComplete)};

  // Out-direction (RX from host).
  usb::EndpointClient<RndisFunction> bulk_out_ep_{usb::EndpointType::BULK, this,
                                                  std::mem_fn(&RndisFunction::RxComplete)};

  // In-direction (TX to host).
  usb::EndpointClient<RndisFunction> bulk_in_ep_{usb::EndpointType::BULK, this,
                                                 std::mem_fn(&RndisFunction::TxComplete)};

  using VmoStore = vmo_store::VmoStore<vmo_store::SlabStorage<uint8_t>>;
  VmoStore vmo_store_;

  std::queue<fnetdev::wire::RxSpaceBuffer> rx_space_buffers_;
  std::queue<uint32_t> tx_completion_queue_;
  std::vector<fuchsia_hardware_usb_endpoint::Completion> rx_completion_queue_;

  struct {
    usb_interface_assoc_descriptor_t assoc;
    usb_interface_descriptor_t communication_interface;
    usb_cs_header_interface_descriptor_t cdc_header;
    usb_cs_call_mgmt_interface_descriptor_t call_mgmt;
    usb_cs_abstract_ctrl_mgmt_interface_descriptor_t acm;
    usb_cs_union_interface_descriptor_1_t cdc_union;
    usb_endpoint_descriptor_t notification_ep;

    usb_interface_descriptor_t data_interface;
    usb_endpoint_descriptor_t out_ep;
    usb_endpoint_descriptor_t in_ep;
  } __PACKED descriptors_;

  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_;

  std::optional<inspect::ComponentInspector> inspector_;
  inspect::Node inspect_node_;
  usb_inspect::EndpointInspect bulk_in_inspect_;
  usb_inspect::EndpointInspect bulk_out_inspect_;
  usb_inspect::EndpointInspect notification_inspect_;
  std::optional<usb_inspect::ThroughputTracker> throughput_tracker_;
};

#endif  // SRC_CONNECTIVITY_ETHERNET_DRIVERS_RNDIS_FUNCTION_RNDIS_FUNCTION_H_
