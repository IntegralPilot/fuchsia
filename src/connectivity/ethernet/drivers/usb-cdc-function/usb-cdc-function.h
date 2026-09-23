// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_ETHERNET_DRIVERS_USB_CDC_FUNCTION_USB_CDC_FUNCTION_H_
#define SRC_CONNECTIVITY_ETHERNET_DRIVERS_USB_CDC_FUNCTION_USB_CDC_FUNCTION_H_

#include <endian.h>
#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.network/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.endpoint/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/listnode.h>

#include <array>

#include <usb-endpoint/usb-endpoint-client.h>
#include <usb-inspect/usb-inspect.h>
#include <usb/cdc.h>
#include <usb/request-fidl.h>
#include <usb/usb.h>

#include "src/lib/vmo_store/vmo_store.h"

namespace usb_cdc_function {

namespace fnetdev = fuchsia_hardware_network_driver;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

#define BULK_REQ_SIZE 2048
#define INTR_COUNT 8

#define BULK_MAX_PACKET 512  // FIXME(voydanoff) USB 3.0 support
#define INTR_MAX_PACKET sizeof(usb_cdc_speed_change_notification_t)

// Total Ethernet MTU is 1514 bytes. 1500 bytes for IP and 14 bytes for Ethernet
// header.
#define ETH_MTU 1514
#define ETH_MAC_SIZE 6

// UsbCdcFunction implements the USB CDC (Ethernet Control Model) peripheral function driver.
//
// Threading & Dispatcher Architecture:
// All protocol servers (fuchsia.hardware.usb.function/UsbFunctionInterface,
// fuchsia.hardware.network.driver/NetworkDeviceImpl), endpoint event callbacks,
// and asynchronous teardown/reconfiguration tasks run on the driver's default
// synchronized dispatcher (`dispatcher()`).
//
// Internal driver state (e.g. `online_`, `configured_`, buffer queues, and endpoint
// clients) is managed exclusively within this single dispatch loop, guaranteeing
// serialized execution without requiring explicit mutex locking.
class UsbCdcFunction : public fdf::DriverBase2,
                       public fdf::WireServer<fnetdev::NetworkDeviceImpl>,
                       public fdf::WireServer<fnetdev::NetworkPort>,
                       public fdf::WireServer<fnetdev::MacAddr>,
                       public fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface> {
 public:
  static constexpr std::string_view kDriverName = "usb_cdc_function";
  static constexpr uint8_t kPortId = 1;
  static constexpr size_t kTxDepth = 64;
  static constexpr size_t kRxDepth = 64;
  static constexpr fdf_arena_tag_t kArenaTag = 'CDCE';

  UsbCdcFunction()
      : fdf::DriverBase2(kDriverName),
        vmo_store_({
            .map =
                vmo_store::MapOptions{
                    .vm_option = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_REQUIRE_NON_RESIZABLE,
                    .vmar = nullptr,
                },
        }) {}

  // Unhide the inherited Start and Stop methods to avoid ambiguity with the
  // fnetdev::NetworkDeviceImpl::Start and Stop methods.
  using fdf::DriverBase2::Start;
  using fdf::DriverBase2::Stop;

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  const std::shared_ptr<fdf::Namespace> &incoming() const { return incoming_; }

  // NetworkDeviceImpl protocol:
  void Init(fnetdev::wire::NetworkDeviceImplInitRequest *request, fdf::Arena &arena,
            InitCompleter::Sync &completer) override;
  void Start(fdf::Arena &arena, StartCompleter::Sync &completer) override;
  void Stop(fdf::Arena &arena, StopCompleter::Sync &completer) override;
  void GetInfo(
      fdf::Arena &arena,
      fdf::WireServer<fnetdev::NetworkDeviceImpl>::GetInfoCompleter::Sync &completer) override;
  void QueueTx(fnetdev::wire::NetworkDeviceImplQueueTxRequest *request, fdf::Arena &arena,
               QueueTxCompleter::Sync &completer) override;
  void QueueRxSpace(fnetdev::wire::NetworkDeviceImplQueueRxSpaceRequest *request, fdf::Arena &arena,
                    QueueRxSpaceCompleter::Sync &completer) override;
  void PrepareVmo(fnetdev::wire::NetworkDeviceImplPrepareVmoRequest *request, fdf::Arena &arena,
                  PrepareVmoCompleter::Sync &completer) override;
  void ReleaseVmo(fnetdev::wire::NetworkDeviceImplReleaseVmoRequest *request, fdf::Arena &arena,
                  ReleaseVmoCompleter::Sync &completer) override;

  // NetworkPort protocol:
  void GetInfo(fdf::Arena &arena,
               fdf::WireServer<fnetdev::NetworkPort>::GetInfoCompleter::Sync &completer) override;
  void GetStatus(fdf::Arena &arena, GetStatusCompleter::Sync &completer) override;
  void SetActive(fnetdev::wire::NetworkPortSetActiveRequest *request, fdf::Arena &arena,
                 SetActiveCompleter::Sync &completer) override;
  void GetMac(fdf::Arena &arena, GetMacCompleter::Sync &completer) override;
  void Removed(fdf::Arena &arena, RemovedCompleter::Sync &completer) override;

  // MacAddr protocol:
  void GetAddress(fdf::Arena &arena, GetAddressCompleter::Sync &completer) override;
  void GetFeatures(fdf::Arena &arena, GetFeaturesCompleter::Sync &completer) override;
  void SetMode(fnetdev::wire::MacAddrSetModeRequest *request, fdf::Arena &arena,
               SetModeCompleter::Sync &completer) override;

  // UsbFunctionInterface methods.
  void Control(ControlRequest &request, ControlCompleter::Sync &completer) override;
  void SetConfigured(SetConfiguredRequest &request,
                     SetConfiguredCompleter::Sync &completer) override;
  void SetInterface(SetInterfaceRequest &request, SetInterfaceCompleter::Sync &completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
      fidl::UnknownMethodCompleter::Sync &completer) override;

  zx_status_t cdc_generate_mac_address();
  void CdcIntrComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completion);
  void CdcSendNotifications();
  void CdcRxComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  void CdcTxComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);
  void ProcessRxCompletions(std::vector<fuchsia_hardware_usb_endpoint::Completion> completions);

  // test helpers.
  bool HasPendingRxCompletions();
  bool online() const { return online_; }
  bool configured() const { return configured_; }
  bool IntrEpRequestsFull() { return intr_ep_.RequestsFull(); }
  const std::array<uint8_t, ETH_MAC_SIZE> &mac_addr() const { return mac_addr_; }
  size_t GetInFlightInterruptCountForTesting() { return intr_ep_.GetInFlightCount(); }

  uint8_t BulkInAddress() const { return descriptors_.bulk_in_ep.b_endpoint_address; }
  uint8_t BulkOutAddress() const { return descriptors_.bulk_out_ep.b_endpoint_address; }
  uint8_t InterruptAddress() const { return descriptors_.intr_ep.b_endpoint_address; }

  inspect::ComponentInspector &inspector() { return *inspector_; }
  usb_inspect::ThroughputTracker &GetThroughputTrackerForTesting() { return *throughput_tracker_; }

 private:
  struct SetConfiguredSharedState;
  struct SetInterfaceSharedState;
  void CancelSetInterface();
  zx_status_t AddNetworkDevice();
  fuchsia_hardware_network::PortStatus ReadStatus() const;
  void UpdatePortStatus();
  void DisableAllEndpoints(fit::callback<void(zx_status_t)> callback);

  struct EndpointInfo {
    uint8_t address;
    usb::EndpointClient<UsbCdcFunction> *client;
    std::string_view name;
  };
  std::array<EndpointInfo, 3> GetEndpoints() {
    return {{
        {
            .address = descriptors_.intr_ep.b_endpoint_address,
            .client = &intr_ep_,
            .name = "intr",
        },
        {
            .address = descriptors_.bulk_out_ep.b_endpoint_address,
            .client = &bulk_out_ep_,
            .name = "bulk_out",
        },
        {
            .address = descriptors_.bulk_in_ep.b_endpoint_address,
            .client = &bulk_in_ep_,
            .name = "bulk_in",
        },
    }};
  }

  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunction> function_;
  fidl::Client<fuchsia_hardware_usb_function::UsbFunction> async_function_;
  std::string mac_addr_string_;
  fdf::WireClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> netdevice_ifc_;

  std::optional<inspect::ComponentInspector> inspector_;
  inspect::Node inspect_node_;
  inspect::BoolProperty online_property_;
  usb_inspect::EndpointInspect bulk_in_inspect_;
  usb_inspect::EndpointInspect bulk_out_inspect_;

  std::optional<usb_inspect::ThroughputTracker> throughput_tracker_;

  compat::SyncInitializedDeviceServer child_;
  std::shared_ptr<fdf::Namespace> incoming_;

  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_controller_;

  // In-direction (TX to host).
  usb::EndpointClient<UsbCdcFunction> intr_ep_{usb::EndpointType::INTERRUPT, this,
                                               std::mem_fn(&UsbCdcFunction::CdcIntrComplete)};

  // Out-direction (RX from host).
  usb::EndpointClient<UsbCdcFunction> bulk_out_ep_{usb::EndpointType::BULK, this,
                                                   std::mem_fn(&UsbCdcFunction::CdcRxComplete)};

  // In-direction (TX to host).
  usb::EndpointClient<UsbCdcFunction> bulk_in_ep_{usb::EndpointType::BULK, this,
                                                  std::mem_fn(&UsbCdcFunction::CdcTxComplete)};

  // Queue of buffer IDs that were sent to USB hardware and are awaiting
  // completion. This mirrors the order of requests submitted to bulk_in_ep_.
  std::queue<uint32_t> tx_completion_queue_;
  std::vector<fuchsia_hardware_usb_endpoint::Completion> rx_completion_queue_;
  void DiscardPendingTxBuffers(zx_status_t status);
  void ReturnPendingRxSpace();
  void DrainRxCompletionQueue();
  void CheckStopComplete();
  void CheckSetConfiguredDone();
  void CheckSetInterfaceDone();

  std::atomic_bool unbound_ = false;  // set to true when device is going away.
  bool deconfigure_called_ = false;
  bool deconfigure_completed_ = false;
  bool intr_cancelled_ = false;
  bool bulk_in_cancelled_ = false;
  bool bulk_out_cancelled_ = false;
  std::shared_ptr<SetConfiguredSharedState> set_configured_state_;
  std::shared_ptr<SetInterfaceSharedState> set_interface_state_;

  // Device attributes
  std::array<uint8_t, ETH_MAC_SIZE> mac_addr_;
  // Guarded by the synchronized driver dispatcher.
  bool online_ = false;
  bool configured_ = false;
  bool pending_notification_ = false;
  fuchsia_hardware_usb_descriptor::UsbSpeed speed_ =
      fuchsia_hardware_usb_descriptor::UsbSpeed::kUndefined;

  using VmoStore = vmo_store::VmoStore<vmo_store::SlabStorage<uint8_t>>;
  VmoStore vmo_store_;

  std::queue<fnetdev::wire::RxSpaceBuffer> rx_space_buffers_;
  std::optional<fdf::StopCompleter> stop_completer_;

  struct {
    usb_interface_descriptor_t comm_intf;
    usb_cs_header_interface_descriptor_t cdc_header;
    usb_cs_union_interface_descriptor_1_t cdc_union;
    usb_cs_ethernet_interface_descriptor_t cdc_eth;
    usb_endpoint_descriptor_t intr_ep;
    usb_interface_descriptor_t cdc_intf_0;
    usb_interface_descriptor_t cdc_intf_1;
    usb_endpoint_descriptor_t bulk_out_ep;
    usb_endpoint_descriptor_t bulk_in_ep;
  } descriptors_ = {
      .comm_intf =
          {
              .b_length = sizeof(usb_interface_descriptor_t),
              .b_descriptor_type = USB_DT_INTERFACE,
              .b_interface_number = 0,  // set later
              .b_alternate_setting = 0,
              .b_num_endpoints = 1,
              .b_interface_class = USB_CLASS_COMM,
              .b_interface_sub_class = USB_CDC_SUBCLASS_ETHERNET,
              .b_interface_protocol = 0,
              .i_interface = 0,
          },
      .cdc_header =
          {
              .bLength = sizeof(usb_cs_header_interface_descriptor_t),
              .bDescriptorType = USB_DT_CS_INTERFACE,
              .bDescriptorSubType = USB_CDC_DST_HEADER,
              .bcdCDC = 0x120,
          },
      .cdc_union =
          {
              .bLength = sizeof(usb_cs_union_interface_descriptor_1_t),
              .bDescriptorType = USB_DT_CS_INTERFACE,
              .bDescriptorSubType = USB_CDC_DST_UNION,
              .bControlInterface = 0,      // set later
              .bSubordinateInterface = 0,  // set later
          },
      .cdc_eth =
          {
              .bLength = sizeof(usb_cs_ethernet_interface_descriptor_t),
              .bDescriptorType = USB_DT_CS_INTERFACE,
              .bDescriptorSubType = USB_CDC_DST_ETHERNET,
              .iMACAddress = 0,  // set later
              .bmEthernetStatistics = 0,
              .wMaxSegmentSize = ETH_MTU,
              .wNumberMCFilters = 0,
              .bNumberPowerFilters = 0,
          },
      .intr_ep =
          {
              .b_length = sizeof(usb_endpoint_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              .b_endpoint_address = 0,  // set later
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kInterrupt),
              .w_max_packet_size = htole16(INTR_MAX_PACKET),
              .b_interval = 8,
          },
      .cdc_intf_0 =
          {
              .b_length = sizeof(usb_interface_descriptor_t),
              .b_descriptor_type = USB_DT_INTERFACE,
              .b_interface_number = 0,  // set later
              .b_alternate_setting = 0,
              .b_num_endpoints = 0,
              .b_interface_class = USB_CLASS_CDC,
              .b_interface_sub_class = 0,
              .b_interface_protocol = 0,
              .i_interface = 0,
          },
      .cdc_intf_1 =
          {
              .b_length = sizeof(usb_interface_descriptor_t),
              .b_descriptor_type = USB_DT_INTERFACE,
              .b_interface_number = 0,  // set later
              .b_alternate_setting = 1,
              .b_num_endpoints = 2,
              .b_interface_class = USB_CLASS_CDC,
              .b_interface_sub_class = 0,
              .b_interface_protocol = 0,
              .i_interface = 0,
          },
      .bulk_out_ep =
          {
              .b_length = sizeof(usb_endpoint_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              .b_endpoint_address = 0,  // set later
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
              .w_max_packet_size = htole16(BULK_MAX_PACKET),
              .b_interval = 0,
          },
      .bulk_in_ep =
          {
              .b_length = sizeof(usb_endpoint_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              .b_endpoint_address = 0,  // set later
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
              .w_max_packet_size = htole16(BULK_MAX_PACKET),
              .b_interval = 0,
          },
  };
};

}  // namespace usb_cdc_function

#endif  // SRC_CONNECTIVITY_ETHERNET_DRIVERS_USB_CDC_FUNCTION_USB_CDC_FUNCTION_H_
