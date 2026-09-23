// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_REMOTE_CONTROL_USB_VSOCK_USB_VSOCK_USB_H_
#define SRC_DEVELOPER_REMOTE_CONTROL_USB_VSOCK_USB_VSOCK_USB_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/fidl.h>
#include <fidl/fuchsia.hardware.vsockbridge/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/wait.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/logging/cpp/structured_logger.h>
#include <lib/fit/function.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/socket.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <memory>
#include <optional>
#include <queue>
#include <utility>
#include <variant>
#include <vector>

#include <bind/fuchsia/google/platform/usb/cpp/bind.h>
#include <fbl/mutex.h>
#include <usb-endpoint/usb-endpoint-client.h>
#include <usb-inspect/usb-inspect.h>
#include <usb/request-cpp.h>
#include <usb/usb-request.h>
#include <usb/usb.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}
namespace fdescriptor = fuchsia_hardware_usb_descriptor;

class VsockUsb;

class VsockUsb : public fdf::DriverBase2,
                 public fidl::WireServer<fuchsia_hardware_vsockbridge::Usb>,
                 public fidl::Server<fuchsia_hardware_usb_function::UsbFunctionInterface> {
 public:
  VsockUsb() : fdf::DriverBase2("vsock-usb") {}

  inspect::ComponentInspector& inspector() { return *inspector_; }
  usb_inspect::ThroughputTracker& GetThroughputTrackerForTesting() { return *throughput_tracker_; }

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  void SetCallback(fuchsia_hardware_vsockbridge::wire::UsbSetCallbackRequest* request,
                   SetCallbackCompleter::Sync& completer) override;

  // UsbFunctionInterface methods.
  void Control(ControlRequest& request, ControlCompleter::Sync& completer) override;
  void SetConfigured(SetConfiguredRequest& request,
                     SetConfiguredCompleter::Sync& completer) override;
  void SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_function::UsbFunctionInterface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // Endpoint address of our IN endpoint.
  uint8_t BulkInAddress() const { return descriptors_.in_ep.b_endpoint_address; }
  // Endpoint address of our OUT endpoint.
  uint8_t BulkOutAddress() const { return descriptors_.out_ep.b_endpoint_address; }

  // Whether there are any pending requests.
  bool HasPendingRequests() { return !bulk_in_ep_.RequestsFull() || !bulk_out_ep_.RequestsFull(); }
  bool HasPendingTxRequests() { return !bulk_in_ep_.RequestsFull(); }
  bool HasPendingRxRequests() { return !bulk_out_ep_.RequestsFull(); }

 private:
  friend class VsockUsbTestHelper;
  // Configures the device's endpoints and sets the device state to Running, if it's in the
  // Unconfigured state.
  zx_status_t ConfigureEndpoints();

  // Disables the device's endpoints, cancels any outstanding requests, and moves the device
  // into the Unconfigured state if it's not already there.
  zx_status_t UnconfigureEndpoints();

  // Cancels all pending requests on bulk IN and OUT endpoints.
  void CancelAllEndpoints();

  // Called whenever the socket from RCS is readable. Reads data out of the socket and places it
  // into bulk IN requests.
  void HandleSocketReadable(async_dispatcher_t*, async::WaitBase*, zx_status_t status,
                            const zx_packet_signal_t*);
  // Called whenever the socket from RCS is writable. Pumps data from Running::socket_out_queue_
  // into the socket.
  void HandleSocketWritable(async_dispatcher_t*, async::WaitBase*, zx_status_t status,
                            const zx_packet_signal_t*);

  // Connect a FIDL interface.
  void FidlConnect(fidl::ServerEnd<fuchsia_hardware_vsockbridge::Usb> request);

  // Internal state machine for this driver. There are three states:
  //
  // Unconfigured, Running, Shutting Down.
  //
  // We can transition from the Unconfigured to the Running state if
  // UsbFunctionInterfaceSetConfigured is called.
  //
  // We transition from Running to Unconfigured if any faults happen while handling the connection
  //
  // We transition from any state to Shutting Down if Stop is called.
  class Unconfigured;
  class Running;
  class ShuttingDown;
  using State = std::variant<Unconfigured, Running, ShuttingDown>;

  // Common interface for states where no socket is available.
  class BaseNoSocketState {
   public:
    static bool WritesWaiting() { return false; }
    static bool ReadsWaiting() { return false; }
  };

  // Unconfigured state. We have no transactions queued and cannot receive data.
  class Unconfigured : public BaseNoSocketState {
   public:
    // Called when we receive data from the host while  in this state. Warns and discards it.
    State ReceiveData(uint8_t* data, size_t len, std::optional<zx::socket>* peer_socket,
                      VsockUsb* owner) &&;
    // Called when we are asked to send data from this state. Should never be called.
    State SendData(uint8_t*, size_t, size_t*, zx_status_t* status) && {
      *status = ZX_ERR_SHOULD_WAIT;
      return std::move(*this);
    }
    State Writable() && { return std::move(*this); }
  };

  // Running. We will send any data we get from the RCS socket. Data we receive will be queued
  // on the RCS socket.
  class Running {
   public:
    Running(zx::socket socket, VsockUsb* owner)
        : socket_(std::move(socket)),
          read_waiter_(
              std::make_unique<async::WaitMethod<VsockUsb, &VsockUsb::HandleSocketReadable>>(
                  owner, socket_.get(), ZX_SOCKET_READABLE)),
          write_waiter_(
              std::make_unique<async::WaitMethod<VsockUsb, &VsockUsb::HandleSocketWritable>>(
                  owner, socket_.get(), ZX_SOCKET_WRITABLE)),
          owner_(owner) {}
    Running(Running&&) = default;
    Running& operator=(Running&& other) noexcept {
      this->~Running();
      socket_ = std::move(other.socket_);
      socket_out_queue_ = std::move(other.socket_out_queue_);
      socket_is_new_ = other.socket_is_new_;
      read_waiter_ = std::move(other.read_waiter_);
      write_waiter_ = std::move(other.write_waiter_);
      owner_ = other.owner_;
      return *this;
    }
    // Called when we receive data from the host while in this state. Pushes the data into socket_.
    State ReceiveData(uint8_t* data, size_t len, std::optional<zx::socket>* peer_socket,
                      VsockUsb* owner) &&;
    // Called when we ware asked to send data from this state. Populates the given buffer with data
    // read from socket_.
    State SendData(uint8_t* data, size_t len, size_t* actual, zx_status_t* status) &&;
    // Whether we have data waiting to be written to socket_.
    bool WritesWaiting() const { return !socket_out_queue_.empty(); }
    // Whether we are waiting to read data from the host.
    static bool ReadsWaiting() { return true; }

    // Called when socket_ is writable. Pumps socket_out_queue_ into socket_.
    State Writable() &&;

    zx::socket* socket() { return &socket_; }
    async::WaitMethod<VsockUsb, &VsockUsb::HandleSocketReadable>* read_waiter() {
      return read_waiter_.get();
    }
    async::WaitMethod<VsockUsb, &VsockUsb::HandleSocketWritable>* write_waiter() {
      return write_waiter_.get();
    }

    ~Running() {
      async::PostTask(owner_->dispatcher_,
                      [read_waiter = std::move(read_waiter_),
                       write_waiter = std::move(write_waiter_), socket = std::move(socket_)]() {
                        if (read_waiter) {
                          read_waiter->Cancel();
                        }
                        if (write_waiter) {
                          write_waiter->Cancel();
                        }
                        (void)socket;
                      });
    }

   private:
    zx::socket socket_;
    std::queue<std::vector<uint8_t>> socket_out_queue_;
    bool socket_is_new_ = true;
    std::unique_ptr<async::WaitMethod<VsockUsb, &VsockUsb::HandleSocketReadable>> read_waiter_;
    std::unique_ptr<async::WaitMethod<VsockUsb, &VsockUsb::HandleSocketWritable>> write_waiter_;
    VsockUsb* owner_;
  };
  class ShuttingDown : public BaseNoSocketState {
   public:
    explicit ShuttingDown(fit::function<void()> callback) {
      if (callback) {
        callbacks_.push_back(std::move(callback));
      }
    }
    // Called when we receive data from the host while in this state. Warns and discards it.
    State ReceiveData(uint8_t* data, size_t len, std::optional<zx::socket>* peer_socket,
                      VsockUsb* owner) &&;
    // Called when we receive data from the host while in this state. Ignores with SHOULD_WAIT.
    State SendData(uint8_t*, size_t, size_t*, zx_status_t* status) && {
      *status = ZX_ERR_SHOULD_WAIT;
      return std::move(*this);
    }
    // Called when a socket is writable. Shouldn't happen.
    State Writable() && { return std::move(*this); }
    // Called when shutdown has been successful.
    void FinishWithCallback() {
      if (finished_) {
        return;
      }
      finished_ = true;
      std::vector<fit::function<void()>> callbacks;
      callbacks.swap(callbacks_);
      for (auto& callback : callbacks) {
        callback();
      }
    }

    void AddCallback(fit::function<void()> callback) {
      if (!callback) {
        return;
      }
      if (finished_) {
        callback();
      } else {
        callbacks_.push_back(std::move(callback));
      }
    }

   private:
    std::vector<fit::function<void()>> callbacks_;
    bool finished_ = false;
  };

  // Callback called when we start a new connection. Dispatches the other end of the socket we
  // create to RCS.
  class Callback {
   public:
    explicit Callback(fidl::WireSharedClient<fuchsia_hardware_vsockbridge::Callback> fidl)
        : fidl_(std::move(fidl)) {}
    void operator()(zx::socket socket);

   private:
    fidl::WireSharedClient<fuchsia_hardware_vsockbridge::Callback> fidl_;
  };

  template <class... Ts>
  struct Overloaded : Ts... {
    using Ts::operator()...;
  };

  // Whether we are in a state that is actively receiving data.
  bool Online() const {
    return std::visit(Overloaded{
                          [](const Running&) { return true; },
                          [](const ShuttingDown&) { return false; },
                          [](const Unconfigured&) { return false; },
                      },
                      state_);
  }

  // Synchronize inspect metrics with current driver state.
  void SyncInspectState() {
    auto [name, online] = std::visit(
        Overloaded{
            [](const Running&) -> std::pair<const char*, bool> { return {"Running", true}; },
            [](const ShuttingDown&) -> std::pair<const char*, bool> {
              return {"ShuttingDown", false};
            },
            [](const Unconfigured&) -> std::pair<const char*, bool> {
              return {"Unconfigured", false};
            },
        },
        state_);
    if (state_property_) {
      state_property_.Set(name);
    }
    if (online_property_) {
      online_property_.Set(online);
    }
  }

  // Transition from Running to Unconfigured, usually due to a connection error.
  void ResetState() {
    if (std::holds_alternative<Running>(state_)) {
      state_ = Unconfigured();
      SyncInspectState();
    }
  }

  // Get an IN request ready for use.
  std::optional<usb::FidlRequest> PrepareTx();

  // Handle when RCS connects to us and is ready to receive a socket, or when we have a socket and
  // need to hand it to RCS.
  void HandleSocketAvailable();

  // Transition to the ShuttingDown state and begin cleaning up driver resources (cancel and wait
  // for all pending transactions).
  void Shutdown(fit::function<void()> callback);

  // Finishes shutting down by calling the shutdown callback.
  void ShutdownComplete();

  // Handle the completion of a single outstanding USB read request.
  void ReadComplete(fuchsia_hardware_usb_endpoint::Completion completion);
  // Handle the completion of a batch of USB read requests.
  void ReadBatchComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completion);
  // Handle the completion of a single outstanding USB write request.
  void WriteComplete(fuchsia_hardware_usb_endpoint::Completion completion);
  // Handle the completion of a batch of USB write requests.
  void WriteBatchComplete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completion);

  // Start watching our RCS socket for readability and call HandleSocketReadable when it is
  // readable.
  void ProcessReadsFromSocket() {
    async::PostTask(dispatcher(), [this]() {
      if (auto state = std::get_if<Running>(&state_)) {
        auto status = state->read_waiter()->Begin(dispatcher());
        if (status != ZX_OK && status != ZX_ERR_ALREADY_EXISTS) {
          FDF_SLOG(ERROR, "Failed to wait on socket", KV("status", zx_status_get_string(status)));
          ResetState();
        }
      }
    });
  }

  // Start watching our RCS socket for writability and call HandleSocketReadable when it is
  // writable.
  void ProcessWritesToSocket() {
    async::PostTask(dispatcher(), [this]() {
      if (auto state = std::get_if<Running>(&state_)) {
        auto status = state->write_waiter()->Begin(dispatcher());
        if (status != ZX_OK && status != ZX_ERR_ALREADY_EXISTS) {
          FDF_SLOG(ERROR, "Failed to wait on socket", KV("status", zx_status_get_string(status)));
          ResetState();
        }
      }
    });
  }

  // Number of USB requests in both free_read_pool_ and free_write_pool_.
  static constexpr size_t kRequestPoolSize = 8;

  // Amount of data buffer allocated for requests in free_read_pool_ and free_write_pool_.
  static constexpr size_t kMtu = 1024;

  // USB max packet size for our interface descriptor.
  static constexpr uint16_t kMaxPacketSize = 512;

  std::optional<Callback> callback_;
  uint64_t callback_id_ = 0;
  std::optional<zx::socket> peer_socket_;

  fidl::SyncClient<fuchsia_driver_framework::NodeController> node_controller_;
  fidl::ServerBindingGroup<fuchsia_hardware_vsockbridge::Usb> device_binding_group_;

  fidl::SyncClient<fuchsia_hardware_usb_function::UsbFunction> function_;

  State state_ = Unconfigured();

  usb::EndpointClient<VsockUsb> bulk_out_ep_{usb::EndpointType::BULK, this,
                                             std::mem_fn(&VsockUsb::ReadBatchComplete)};
  usb::EndpointClient<VsockUsb> bulk_in_ep_{usb::EndpointType::BULK, this,
                                            std::mem_fn(&VsockUsb::WriteBatchComplete)};

  std::optional<inspect::ComponentInspector> inspector_;
  inspect::Node inspect_node_;
  inspect::BoolProperty online_property_;
  inspect::StringProperty state_property_;
  usb_inspect::EndpointInspect bulk_in_inspect_;
  usb_inspect::EndpointInspect bulk_out_inspect_;
  std::optional<usb_inspect::ThroughputTracker> throughput_tracker_;

  struct {
    usb_interface_descriptor_t data_interface;
    usb_endpoint_descriptor_t out_ep;
    usb_endpoint_descriptor_t in_ep;
  } __PACKED descriptors_ [[maybe_unused]] = {
      .data_interface =
          {
              .b_length = sizeof(usb_interface_descriptor_t),
              .b_descriptor_type = USB_DT_INTERFACE,
              .b_interface_number = 0,  // set later
              .b_alternate_setting = 0,
              .b_num_endpoints = 2,
              .b_interface_class = USB_CLASS_VENDOR,
              .b_interface_sub_class =
                  bind_fuchsia_google_platform_usb::BIND_USB_SUBCLASS_VSOCK_BRIDGE,
              .b_interface_protocol =
                  bind_fuchsia_google_platform_usb::BIND_USB_PROTOCOL_VSOCK_BRIDGE,
              .i_interface = 0,
          },
      .out_ep =
          {
              .b_length = sizeof(usb_endpoint_descriptor_t),
              .b_descriptor_type = USB_DT_ENDPOINT,
              .b_endpoint_address = 0,  // set later
              .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
              .w_max_packet_size = htole16(kMaxPacketSize),
              .b_interval = 0,
          },
      .in_ep = {
          .b_length = sizeof(usb_endpoint_descriptor_t),
          .b_descriptor_type = USB_DT_ENDPOINT,
          .b_endpoint_address = 0,  // set later
          .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
          .w_max_packet_size = htole16(kMaxPacketSize),
          .b_interval = 0,
      }};

  // Tracks whether the hardware USB endpoints have been configured via the function driver.
  // Kept distinct from `state_` because `state_` tracks socket connection and data streaming
  // lifecycle (which resets to Unconfigured when a client socket disconnects while hardware
  // endpoints remain configured).
  bool endpoints_configured_ = false;
  async_dispatcher_t* dispatcher_ = fdf::Dispatcher::GetCurrent()->async_dispatcher();
};

#endif  // SRC_DEVELOPER_REMOTE_CONTROL_USB_VSOCK_USB_VSOCK_USB_H_
