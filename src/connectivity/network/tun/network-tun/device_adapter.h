// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_DEVICE_ADAPTER_H_
#define SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_DEVICE_ADAPTER_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>

#include <array>
#include <queue>

#include <fbl/mutex.h>

#include "buffer.h"
#include "config.h"
#include "port_adapter.h"
#include "src/connectivity/network/drivers/network-device/device/public/network_device.h"
#include "state.h"

namespace network {
namespace tun {

namespace testing {
class TunTest;
}  // namespace testing

class DeviceAdapter;

// An abstract DeviceAdapter parent.
//
// This abstract class allows the owner of a `DeviceAdapter` to change its behavior and be notified
// of important events.
class DeviceAdapterParent {
 public:
  virtual ~DeviceAdapterParent() = default;

  /// Requests that the device adapter be destroyed, it encountered an
  /// unrecoverable error.
  virtual void RequestErrorUnbind() = 0;
  // Gets the DeviceAdapter's configuration.
  virtual const BaseDeviceConfig& config() const = 0;
  // Called when transmit buffers become available.
  virtual void OnTxAvail(DeviceAdapter* device) = 0;
  // Called when receive buffers become available.
  virtual void OnRxAvail(DeviceAdapter* device) = 0;
};

// An entity that instantiates a `NetworkDeviceInterface` and provides an implementations of
// `fuchsia.hardware.network.device.NetworkDeviceImpl` that grants access to the buffers exchanged
// with the interface.
//
// `DeviceAdapter` is used to provide the business logic of virtual NetworkDevice implementations
// both for `tun.Device` and `tun.DevicePair` device classes.
// `DeviceAdapter` maintains the buffer nomenclature used by the DeviceInterface, that is: A "Tx"
// buffer is a buffer that contains data that is expected to be sent over a link, and an "Rx" buffer
// is free space that can be used to write data received over a link and push it back to
// applications.
class DeviceAdapter : public fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceImpl> {
 public:
  // Creates a new `DeviceAdapter` with  `parent`, that will serve its requests through
  // `netdev_dispatcher`. `dispatchers` is passed on to the the network-device library.
  static zx::result<std::unique_ptr<DeviceAdapter>> Create(
      const DeviceInterfaceDispatchers& dispatchers,
      fdf::UnownedUnsynchronizedDispatcher&& netdev_dispatcher, DeviceAdapterParent* parent);

  // Binds `req` to this adapter's `NetworkDeviceInterface`.
  zx_status_t Bind(fidl::ServerEnd<fuchsia_hardware_network::Device> req);
  // Binds `req` to the port with `port_id` in this adapter's `NetworkDeviceInterface`.
  zx_status_t BindPort(uint8_t port_id, fidl::ServerEnd<fuchsia_hardware_network::Port> req);
  // Binds a new driver connection and returns the client end.
  fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl> BindDriver();

  // Tears down this adapter and calls `callback` when teardown is finished.
  // Tearing down causes all client channels to be closed.
  // There are no guarantees over which thread `callback` is called.
  // It is invalid to attempt to tear down a device that is already tearing down or is already torn
  // down.
  void Teardown(fit::function<void()> callback);
  // Same as `Teardown`, but blocks until teardown is complete.
  void TeardownSync();

  // NetworkDeviceImpl protocol:
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

  // Attempts to get a pending transmit buffer containing data expected to reach the network from
  // the pool of pending buffers.
  // The second argument given to `callback` is the number of remaining pending buffers (not
  // including the one given to it).
  // Returns `true` if a buffer was successfully allocated. The buffer given to `callback` is
  // discarded from the list of pending buffers and marked as pending for return.
  bool TryGetTxBuffer(fit::callback<zx_status_t(TxBuffer&, size_t)> callback);
  // Calls `func` with all currently enqueued tx buffers.
  // If `func` returns a value different than `ZX_OK`, the buffer is returned to the device
  // implementation with that error.
  void RetainTxBuffers(fit::function<zx_status_t(TxBuffer&)> func);
  // Attempts to write `data` and `meta` into an available rx buffer and return it to the
  // `NetworkDeviceInterface`.
  // Returns the number of remaining available buffers.
  // Returns `ZX_ERR_BAD_STATE` if the device is offline, or `ZX_ERR_SHOULD_WAIT` if there are no
  // buffers available to write `data` into
  zx::result<size_t> WriteRxFrame(PortAdapter& port,
                                  fuchsia_hardware_network::wire::FrameType frame_type,
                                  const uint8_t* data, size_t count,
                                  const std::optional<fuchsia_net_tun::wire::FrameMetadata>& meta);
  zx::result<size_t> WriteRxFrame(PortAdapter& port,
                                  fuchsia_hardware_network::wire::FrameType frame_type,
                                  const fidl::VectorView<uint8_t>& data,
                                  const std::optional<fuchsia_net_tun::wire::FrameMetadata>& meta);
  zx::result<size_t> WriteRxFrame(PortAdapter& port,
                                  fuchsia_hardware_network::wire::FrameType frame_type,
                                  const std::vector<uint8_t>& data,
                                  const std::optional<fuchsia_net_tun::wire::FrameMetadata>& meta);
  // Copies all pending tx buffers from `this` consuming any available rx buffers from `other`.
  // If `return_failed_buffers` is `true`, all buffers from `this` that couldn't be immediately
  // copied into available buffers from `other` will be returned to applications in a failure state,
  // otherwise buffers from `this` will remain in the available buffer pool.
  void CopyTo(DeviceAdapter* other, bool return_failed_buffers);

  // Handles status change notifications from port with `port_id`.
  void OnPortStatusChanged(uint8_t port_id, const PortStatus& new_status);
  // Adds |port| to the device.
  zx_status_t AddPort(PortAdapter& port);
  // Removes port with |port_id|.
  void RemovePort(uint8_t port_id);
  // Delegates |lease| up the receive path.
  zx_status_t DelegateRxLease(fuchsia_hardware_network::wire::DelegatedRxLease lease);

  const fdf::UnownedUnsynchronizedDispatcher& netdevice_dispatcher() const {
    return netdev_dispatcher_;
  }

 private:
  friend class network::tun::testing::TunTest;

  static constexpr uint16_t kFifoDepth = fuchsia_net_tun::wire::kFifoDepth;
  explicit DeviceAdapter(DeviceAdapterParent* parent, const DeviceInterfaceDispatchers& dispatchers,
                         fdf::UnownedUnsynchronizedDispatcher&& netdev_dispatcher);

  // Enqueues a single fulfilled rx frame.
  void EnqueueRx(uint8_t port_id, fuchsia_hardware_network::wire::FrameType frame_type,
                 RxBuffer buffer, size_t length,
                 const std::optional<fuchsia_net_tun::wire::FrameMetadata>& meta)
      __TA_REQUIRES(rx_lock_);
  // Commits all pending rx buffers, returning them to the `NetworkDeviceInterface`.
  void CommitRx() __TA_REQUIRES(rx_lock_);
  // Enqueues a single consumed tx frame.
  // `status` indicates the success or failure when consuming the frame, as dictated by the
  // `NetworkDeviceInterface` contract.
  void EnqueueTx(uint32_t id, zx_status_t status) __TA_REQUIRES(tx_lock_);
  // Commits all pending tx frames, returning them to the `NetworkDeviceInterface`.
  void CommitTx() __TA_REQUIRES(tx_lock_);
  // Allocates rx buffer space with at least `length` bytes.
  zx::result<RxBuffer> AllocRxSpace(size_t length) __TA_REQUIRES(rx_lock_);
  void ReclaimRxSpace(RxBuffer buffer) __TA_REQUIRES(rx_lock_);
  // Requests |kFifoDepth| rx space buffers from the interface.
  void RequestRxSpace() __TA_REQUIRES(rx_lock_);

  // Used in tests to destroy `device_`, which will make `AddPort()` fail.
  void DropNetworkDeviceInterfaceForTests();

  std::unique_ptr<NetworkDeviceInterface> device_;
  DeviceAdapterParent* const parent_;  // pointer to parent, not owned.

  fbl::Mutex rx_lock_;
  fbl::Mutex tx_lock_;
  // NOTE: VmoStore is not thread safe in itself. Our concurrency guarantees come from the
  // `NetworkDeviceInterface` contract, where VMOs are released once we've returned all the buffers.
  // If we wrap it in a lock, or add thread safety to `VmoStore`, we end up with unnecessary added
  // complexity and also we lose the benefit of being able to operate on rx and tx frames without
  // shared locks between them.
  VmoStore vmos_;
  std::queue<TxBuffer> tx_buffers_ __TA_GUARDED(tx_lock_);
  bool tx_available_ __TA_GUARDED(tx_lock_) = false;
  std::queue<fuchsia_hardware_network_driver::wire::RxSpaceBuffer> rx_buffers_
      __TA_GUARDED(rx_lock_);
  bool rx_available_ __TA_GUARDED(rx_lock_) = false;
  bool rx_space_requested_ __TA_GUARDED(rx_lock_) = false;
  std::vector<fuchsia_hardware_network_driver::wire::RxBuffer> return_rx_list_
      __TA_GUARDED(rx_lock_);
  std::array<fuchsia_hardware_network_driver::wire::RxBufferPart, kFifoDepth> return_rx_parts_
      __TA_GUARDED(rx_lock_);
  size_t return_rx_parts_count_ __TA_GUARDED(rx_lock_) = 0;
  std::vector<fuchsia_hardware_network_driver::wire::TxResult> return_tx_list_
      __TA_GUARDED(tx_lock_);
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> device_iface_;
  std::array<std::atomic_bool, fuchsia_hardware_network::wire::kMaxPorts> port_online_status_;
  std::array<std::atomic_bool, fuchsia_hardware_network::wire::kMaxPorts> port_rx_checksum_offload_;
  DeviceInterfaceDispatchers dispatchers_;
  fdf::UnownedUnsynchronizedDispatcher netdev_dispatcher_;
};
}  // namespace tun
}  // namespace network

#endif  // SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_DEVICE_ADAPTER_H_
