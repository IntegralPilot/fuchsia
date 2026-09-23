// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_USB_FUNCTION_H_
#define SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_USB_FUNCTION_H_

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.peripheral/cpp/fidl.h>
#include <lib/async/cpp/wait.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/compat/cpp/compat.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/trace/event.h>

#include <format>

#include <fbl/array.h>
#include <usb-inspect/usb-inspect.h>
#include <usb/usb.h>

namespace usb_peripheral {

class UsbPeripheral;

// This class represents a USB function in the peripheral role configurations.
// USB function drivers bind to this.
class UsbFunction : public fidl::Server<fuchsia_hardware_usb_function::UsbFunction>,
                    public std::enable_shared_from_this<UsbFunction> {
 public:
  UsbFunction(size_t index, UsbPeripheral* peripheral,
              fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor desc, uint8_t configuration,
              uint64_t config_generation, async_dispatcher_t* dispatcher)
      : index_(index),
        config_generation_(config_generation),
        configuration_(configuration),
        peripheral_(peripheral),
        function_descriptor_(desc),
        dispatcher_(dispatcher),
        name_(std::format("function-{:03d}", index)) {}
  ~UsbFunction() override;
  uint64_t config_generation() const { return config_generation_; }

  // If SetConfigured(true, ...) is called from an already configured state,
  // then a deconfigure/reconfigure sequence is performed to reset the function
  // state.
  void SetConfigured(bool configured, usb_speed_t speed,
                     fit::callback<void(zx_status_t)> completer);
  void SetInterface(uint8_t interface, uint8_t alt_setting,
                    fit::callback<void(zx_status_t)> completer);
  zx::result<std::vector<uint8_t>> Control(
      const fuchsia_hardware_usb_descriptor::wire::UsbSetup& setup,
      cpp20::span<uint8_t> write_buffer);
  size_t function_index() const { return index_; }
  std::string name() const { return name_; }
  void RequestRemoval();
  uint8_t configuration() const { return configuration_; }

  void OnNodeControllerUnbound(fidl::UnbindInfo info);

  inline const usb_descriptor_header_t* GetDescriptors(size_t* out_length) const {
    *out_length = descriptors_.size();
    return reinterpret_cast<usb_descriptor_header_t*>(descriptors_.data());
  }

  inline const fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor& GetFunctionDescriptor()
      const {
    return function_descriptor_;
  }

  inline uint8_t GetNumInterfaces() const { return num_interfaces_; }

  // Returns all endpoint addresses associated with the given interface and alternate setting
  // by parsing the function's descriptors. Returns std::nullopt if the function has no descriptors
  // (e.g. mock test functions).
  std::optional<std::vector<uint8_t>> GetEndpointsForInterface(uint8_t interface_num,
                                                               uint8_t alt_setting) const;

  zx_status_t UsbFunctionCancelAll(uint8_t ep_address);

  // fuchsia_hardware_usb_function.UsbFunction protocol implementation.
  void ConnectToEndpoint(ConnectToEndpointRequest& request,
                         ConnectToEndpointCompleter::Sync& completer) override;
  void Configure(ConfigureRequest& request, ConfigureCompleter::Sync& completer) override;
  void AllocResources(AllocResourcesRequest& request,
                      AllocResourcesCompleter::Sync& completer) override;
  void EndpointSetStall(EndpointSetStallRequest& request,
                        EndpointSetStallCompleter::Sync& completer) override;
  void EndpointClearStall(EndpointClearStallRequest& request,
                          EndpointClearStallCompleter::Sync& completer) override;
  void ConfigureEndpoint(ConfigureEndpointRequest& request,
                         ConfigureEndpointCompleter::Sync& completer) override;
  void DisableEndpoint(DisableEndpointRequest& request,
                       DisableEndpointCompleter::Sync& completer) override;
  void Deconfigure(DeconfigureCompleter::Sync& completer) override;

  zx::result<> AddChild(fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent,
                        const std::shared_ptr<fdf::Namespace>& incoming,
                        const std::shared_ptr<fdf::OutgoingDirectory>& outgoing);
  bool registered() const { return function_intf_.is_valid(); }

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(UsbFunction);

  zx_status_t CommonEndpointSetStall(uint8_t ep_address);
  zx_status_t CommonEndpointClearStall(uint8_t ep_address);
  zx_status_t CommonEndpointConfigure(
      uint8_t ep_address,
      fuchsia_hardware_usb_function::EndpointConfiguration endpoint_configuration);
  zx_status_t CommonEndpointDisable(uint8_t ep_address);
  void CloseFunctionInterface();
  void SetDescriptors(uint8_t* descriptors, size_t length);
  void ClearDescriptors();

  // fidl::WireAsyncEventHandler<fuchsia_hardware_usb_function::UsbFunctionInterface>
  class FunctionEventHandler
      : public fidl::WireAsyncEventHandler<fuchsia_hardware_usb_function::UsbFunctionInterface> {
   public:
    explicit FunctionEventHandler(UsbFunction* parent) : parent_(parent->weak_from_this()) {}
    ~FunctionEventHandler();
    void on_fidl_error(fidl::UnbindInfo info) override;
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata)
        override;

   private:
    std::weak_ptr<UsbFunction> parent_;
  };

  class NodeControllerEventHandler
      : public fidl::WireAsyncEventHandler<fuchsia_driver_framework::NodeController> {
   public:
    explicit NodeControllerEventHandler(UsbFunction* parent) : parent_(parent->weak_from_this()) {}
    void on_fidl_error(fidl::UnbindInfo info) override {
      if (std::shared_ptr parent = parent_.lock()) {
        parent->OnNodeControllerUnbound(info);
      }
    }
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_driver_framework::NodeController> metadata) override {}

   private:
    std::weak_ptr<UsbFunction> parent_;
  };

  const size_t index_;
  const uint64_t config_generation_;
  uint8_t configuration_;

  std::optional<bool> last_configured_;
  UsbPeripheral* peripheral_;

  fidl::WireSharedClient<fuchsia_hardware_usb_function::UsbFunctionInterface> function_intf_;
  int CompletionThread();
  const fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor function_descriptor_;

  uint8_t num_interfaces_ = 0;
  fbl::Array<uint8_t> descriptors_;

  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fuchsia_hardware_usb_function::UsbFunction> bindings_;
  fidl::WireSharedClient<fuchsia_driver_framework::NodeController> child_;
  compat::SyncInitializedDeviceServer compat_server_;
  std::optional<fdf_metadata::MetadataServer<fuchsia_boot_metadata::MacAddressMetadata>>
      mac_address_metadata_server_;
  std::optional<fdf_metadata::MetadataServer<fuchsia_boot_metadata::SerialNumberMetadata>>
      serial_number_metadata_server_;
  std::optional<DeconfigureCompleter::Async> deconfigure_completer_;
  std::shared_ptr<fdf::OutgoingDirectory> outgoing_;
  usb_inspect::FunctionInspect inspect_;
  std::string name_;
};

}  // namespace usb_peripheral

#endif  // SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_USB_FUNCTION_H_
