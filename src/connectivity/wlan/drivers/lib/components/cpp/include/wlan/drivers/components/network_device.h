// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_INCLUDE_WLAN_DRIVERS_COMPONENTS_NETWORK_DEVICE_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_INCLUDE_WLAN_DRIVERS_COMPONENTS_NETWORK_DEVICE_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network.driver/cpp/driver/fidl.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/stdcompat/span.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/compiler.h>

#include <mutex>
#include <optional>
#include <vector>

#include <wlan/drivers/components/frame.h>
#include <wlan/drivers/components/frame_container.h>
#include <wlan/drivers/components/internal/async_txn.h>

namespace wlan::drivers::components {

using fuchsia_driver_framework::NodeController;

// This class is intended to make it easier to construct and work with a
// fuchsia.hardware.network.device device (called netdev here) and its interfaces. The user should
// instantiate an object of this class and provide an implementation of NetworkDevice::Callbacks to
// handle the various calls that are made to it.
//
// The actual network device is created when Initialize() is called. As part of the creation of the
// device Callbacks::NetDevInit will be called and is a suitable place to perform any setup that
// could fail and prevent the creation of the device by returning an error code. When the device
// is destroyed Callbacks::NetDevRelease will be called which should be used to perform any cleanup.
class NetworkDevice final
    : public fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceImpl>,
      public fidl::WireAsyncEventHandler<NodeController> {
 public:
  class Callbacks {
   public:
    // Takes zx_status_t as parameter, call txn.Reply(status) to complete transaction.
    using InitTxn = AsyncTxn<InitCompleter::Sync, zx_status_t>;
    using StartTxn = AsyncTxn<StartCompleter::Sync, zx_status_t>;
    // Does not take a parameter, call txn.Reply() to complete transaction.
    using StopTxn = AsyncTxn<StopCompleter::Sync>;

    virtual ~Callbacks();

    // Called when the underlying device is released.
    virtual void NetDevRelease() = 0;

    // Called as part of the initialization of the device. The device is considered initialized
    // after txn.Reply() has been called with a ZX_OK status as parameter. The device can call
    // txn.Reply() at any time, either during the invocation of NetDevInit or at a later time from
    // the same thread or another thread. Call txn.Reply() with anything but ZX_OK will prevent the
    // device from being created.
    virtual void NetDevInit(InitTxn txn) = 0;

    // Start the device's data path. The data path is considered started after txn.Reply() has been
    // called with a ZX_OK status as parameter. The device can call txn.Reply() at any time, either
    // during the invocation of NetDevStart or at a later time from the same thread or another
    // thread. To indicate that the data path has already started, call txn.Reply() with
    // ZX_ERR_ALREADY_BOUND, any other error code indicates a failure to start the data path.
    virtual void NetDevStart(StartTxn txn) = 0;

    // Stop the device's data path. The data path will be considered stopped after txn.Reply() has
    // been called. The device can call txn.Reply() at any time, either during the invocation of
    // NetDevStop or at a later time from the same thread or another thread.
    //
    // When the device receives this call it must return all TX and RX frames previously provided
    // and any further TX and RX frames received while in a stopped state must be immediately
    // returned. TX frames should be returned using NetworkDevice::CompleteTx with status
    // ZX_ERR_UNAVAILABLE and RX frames should be returned using NetworkDevice::CompleteRx with a
    // size of zero.
    virtual void NetDevStop(StopTxn txn) = 0;

    // Get information from the device about the underlying device. This includes details such as RX
    // depth, TX depths, features supported any many others. See the device_impl_info_t struct for
    // more information.
    virtual void NetDevGetInfo(fuchsia_hardware_network_driver::DeviceImplInfo* out_info) = 0;

    // Enqueue frames for transmission. A span of frames is provided which represent all the frames
    // to be sent. These frames point to the payload to be transmitted and will have any additional
    // headroom and tailspace specified in device_impl_info_t available. So in order to populate
    // headers for example the driver must first call GrowHead on a frame to place the data pointer
    // at the location where the header should be, then populate the header. Note that the lifetime
    // of the data pointed to by the span is limited to this method call. Once this method
    // implementation returns, the frame objects (but not the underlying data) will be lost. The
    // driver therefore needs to make a copy of these frame objects, for example by placing them in
    // a queue or submitting them to hardware before the method returns.
    virtual void NetDevQueueTx(cpp20::span<Frame> frames) = 0;

    // Enqueue available space to the device, for receiving data into. The device will provide any
    // number of buffers up to a total maximum specified during the NetDevGetInfo call for RX depth.
    // As the device completes RX buffers, thereby giving them back to NetworkDevice, the device
    // will gradually pass them back to the device through this call. The device is expected to
    // store all these space buffers and then use them to receive data. FrameStorage is intended for
    // such usage. As a convenience an array containing the virtual addresses where each VMO is
    // mapped is provided. The VMO id in the buffers can be used as a direct index into this array
    // to obtain the virtual address where the VMO has been mapped into memory. Note that in order
    // to get the address for a specific buffer the offset in the buffer needs to be added to the
    // VMO address. The number of addresses in this array matches the maximum number of possible
    // VMOs, defined by MAX_VMOS.
    virtual void NetDevQueueRxSpace(
        cpp20::span<const fuchsia_hardware_network_driver::wire::RxSpaceBuffer> buffers,
        uint8_t* vmo_addrs[]) = 0;

    // Inform the device that a new VMO is being used for data transfer. Each frame simply points to
    // a VMO provided through this call. Usually this VMO is shared among multiple frames and each
    // frame has an offset into the VMO indicating where its data is located. The device may need to
    // perform additional operations at the bus level to make sure that these VMOs are ready for
    // use with the bus, examples of this include making the VMO available for DMA operations. In
    // addition to providing the VMO the method call also provides a location in virtual memory
    // where the VMO has been memory mapped and the size of the mapping.
    virtual zx_status_t NetDevPrepareVmo(uint8_t vmo_id, zx::vmo vmo, uint8_t* mapped_address,
                                         size_t mapped_size) = 0;

    // Inform the device that a VMO will no longer be used for data transfers. The device may have
    // to ensure that any underlying bus is made aware of this to complete the release. The vmo_id
    // is one of the IDs previously supplied in the prepare call. It is guaranteed that this will
    // not be called until the device has returned all TX and RX frames through CompleteTx and
    // CompleteRx. This means that the device does not need to attempt any cleanup and return of
    // frames as a result of this call.
    virtual void NetDevReleaseVmo(uint8_t vmo_id) = 0;
  };

  explicit NetworkDevice(Callbacks* callbacks);

  ~NetworkDevice() override;
  NetworkDevice(const NetworkDevice&) = delete;
  NetworkDevice& operator=(const NetworkDevice&) = delete;

  // Initialize the NetworkDevice. This should be called by the device driver when it's ready for
  // the NetworkDevice to start. The NetworkDeviceImpl service will be added to the |outgoing|
  // directory. A netdev child node named |device_name| will be added to |parent|. Requests will be
  // served on |dispatcher|. This MUST be called on a dispatcher that can safely make calls to
  // |parent| and |outgoing|. The |outgoing| object MUST be kept alive for the duration of the
  // NetworkDevice object's lifetime. It will be used again as part of the Remove call.
  zx_status_t Initialize(fidl::WireClient<fuchsia_driver_framework::Node>& parent,
                         fdf_dispatcher_t* dispatcher, fdf::OutgoingDirectory& outgoing,
                         const char* device_name) __TA_EXCLUDES(mutex_);

  // Remove the NetworkDevice, this calls Remove on the network device child node controller. The
  // removal is not complete until NetDevRelease is called on the Callbacks interface. Returns true
  // if removal was initiated, false if removal is not needed because Initialize was never called or
  // the device was already removed. Note that multiple concurrent calls to Remove may all return
  // true, don't rely on it to determine if removal has been initiated. It only indicates that the
  // removal is completed.
  bool Remove() __TA_EXCLUDES(mutex_);

  // Wait until the server is connected to a client. Intended for use in testing.
  void WaitUntilServerConnected();

  // Access the NetworkDeviceIfc client that is created when NetworkDeviceImpl::Init is called by
  // the network-device driver. This client will not be valid until the NetDevInit callback has been
  // called on the Callbacks interface.
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc>& NetDevIfcClient();

  // Notify NetworkDevice of a single incoming RX frame. This method exists for convenience but the
  // driver should prefer to complete as many frames as possible in one call instead of making
  // multiple calls to this method. It is safe to complete a frame with a size of zero, such a frame
  // will be considered unfulfilled. An unfulfilled frame will not be passed up through the network
  // stack and its receive space can be made available to the device again. After this call the
  // device must not expect to be able to use the frame again.
  void CompleteRx(Frame&& frame);
  // Notify NetworkDevice of multiple incoming RX frames. This is the preferred way of receiving
  // frames as receiving multiple frames in one call is more efficient. It is safe to complete
  // frames with a size of zero, such frames will be considered unfulfilled. Unfulfilled frames will
  // not be passed up through the network stack and its receive space can be made available to the
  // device again. This allows the driver to indicate that individual frames in a FrameContainer do
  // not need to be received without having to remove them from the FrameContainer. After this call
  // the device must not expect to be able to use the frames again.
  void CompleteRx(FrameContainer&& frames);
  // Notify NetworkDevice that TX frames have been completed. This can be either because they were
  // successfully received, indicated by a ZX_OK status, or failed somehow, indicated by anything
  // other than ZX_OK. All frames in the sequence will be completed with the same status, if the
  // driver has partially completed a transmission where some frames succeeded and some failed, the
  // driver will have to create separate spans and make multiple calls to this method. After this
  // call the device must not expect to be able to use the frames again.
  void CompleteTx(cpp20::span<Frame> frames, zx_status_t status);

 private:
  // NetworkDeviceImpl implementation
  void Init(fuchsia_hardware_network_driver::wire::NetworkDeviceImplInitRequest* request,
            fdf::Arena& arena, InitCompleter::Sync& completer) override;
  void Start(fdf::Arena& arena, StartCompleter::Sync& completer) override;
  void Stop(fdf::Arena& arena, StopCompleter::Sync& completer) override;
  void GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) override;
  void QueueTx(fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueTxRequest* request,
               fdf::Arena& arena, QueueTxCompleter::Sync& completer) override;
  void QueueRxSpace(
      fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueRxSpaceRequest* request,
      fdf::Arena& arena, QueueRxSpaceCompleter::Sync& completer) override;
  void PrepareVmo(
      fuchsia_hardware_network_driver::wire::NetworkDeviceImplPrepareVmoRequest* request,
      fdf::Arena& arena, PrepareVmoCompleter::Sync& completer) override;
  void ReleaseVmo(
      fuchsia_hardware_network_driver::wire::NetworkDeviceImplReleaseVmoRequest* request,
      fdf::Arena& arena, ReleaseVmoCompleter::Sync& completer) override;

  // fidl::WireAsyncEventHandler<NodeController> implementation
  void handle_unknown_event(fidl::UnknownEventMetadata<NodeController> metadata) override {}
  void on_fidl_error(::fidl::UnbindInfo error) override;

  zx_status_t AddService(fdf::OutgoingDirectory& outgoing) __TA_REQUIRES(mutex_);
  bool ShouldCompleteFrame(const Frame& frame);

  // Removal tasks
  void RemoveLocked() __TA_REQUIRES(mutex_);
  // These removal methods should only ever be called from the parent dispatcher. No locks needed.
  void Unbind();
  void RemoveService();
  void RemoveNode();
  void Release();

  enum class State {
    // Device not yet initialized, this is the initial state after the object has been constructed.
    UnInitialized,
    // The device has been initialized and is actively serving requests.
    Initialized,
    // The device is shutting down and currently removing the network device child node.
    RemovingNode,
    // The device is shutting down and currently removing the NetworkDeviceImpl service.
    RemovingService,
    // The device is shutting down and currently unbinding the NetworkDeviceImpl server.
    Unbinding,
    // The device is shut down and is about to call NetDevRelease on its Callbacks pointer.
    Releasing,
    // The device is fully shut down and the NetworkDevice object can safely be destroyed.
    Released,
  };

  void TransitionToState(State new_state, fit::function<void()>&& task) __TA_REQUIRES(mutex_);

  // Device management members.
  fdf_dispatcher_t* netdev_dispatcher_ __TA_GUARDED(mutex_) = nullptr;
  async_dispatcher_t* parent_dispatcher_ __TA_GUARDED(mutex_) = nullptr;
  fdf::OutgoingDirectory* outgoing_directory_ __TA_GUARDED(mutex_) = nullptr;
  fidl::WireClient<NodeController> node_controller_ __TA_GUARDED(mutex_);
  State state_ __TA_GUARDED(mutex_) = State::UnInitialized;
  // To be able to use unsynchronized dispatchers the binding has to be a ServerBindingRef. Other
  // binding types require a synchronized dispatcher.
  std::optional<fdf::ServerBindingRef<fuchsia_hardware_network_driver::NetworkDeviceImpl>> binding_
      __TA_GUARDED(mutex_);
  std::mutex mutex_;
  libsync::Completion server_connected_;

  // Regular operations members.
  Callbacks* callbacks_;
  std::atomic<bool> started_ = false;
  // This needs to be a shared client because it's going to be used with an unsynchronized
  // dispatcher and it's going to be cloned for each NetworkPort.
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> netdev_ifc_;
  uint8_t* vmo_addrs_[fuchsia_hardware_network::wire::kMaxDataVmos] = {};
  uint64_t vmo_lengths_[fuchsia_hardware_network::wire::kMaxDataVmos] = {};
  std::vector<Frame> tx_frames_;
};

}  // namespace wlan::drivers::components

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_INCLUDE_WLAN_DRIVERS_COMPONENTS_NETWORK_DEVICE_H_
