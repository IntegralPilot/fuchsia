// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_TEST_UTIL_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_TEST_UTIL_H_

#include <lib/sync/completion.h>
#include <lib/zx/event.h>

#include <memory>
#include <vector>

#include <fbl/intrusive_double_list.h>
#include <gtest/gtest.h>

#include "definitions.h"
#include "device_interface.h"
#include "src/lib/testing/predicates/status.h"

namespace network::testing {

constexpr uint16_t kDefaultRxDepth = 16;
constexpr uint16_t kDefaultTxDepth = 16;
constexpr uint16_t kDefaultDescriptorCount = 256;
constexpr uint64_t kDefaultBufferLength = 2048;
constexpr uint64_t kBufferAlignment = 4096;
constexpr uint32_t kAutoReturnRxLength = 512;

class RxReturnTransaction;
class TxReturnTransaction;
using VmoProvider = fit::function<zx::unowned_vmo(uint8_t)>;

class TxBuffer : public fbl::DoublyLinkedListable<std::unique_ptr<TxBuffer>> {
 public:
  explicit TxBuffer(const fuchsia_hardware_network_driver::wire::TxBuffer& buffer)
      : buffer_(buffer) {
    auto regions = buffer.data.get();
    for (size_t i = 0; i < regions.size(); ++i) {
      parts_[i] = regions[i];
    }
    buffer_.data =
        fidl::VectorView<fuchsia_hardware_network_driver::wire::BufferRegion>::FromExternal(
            parts_.data(), regions.size());
  }

  zx_status_t status() const { return status_; }

  void set_status(zx_status_t status) { status_ = status; }

  zx::result<std::vector<uint8_t>> GetData(const VmoProvider& vmo_provider);

  fuchsia_hardware_network_driver::wire::TxResult result() {
    return {
        .id = buffer_.id,
        .status = status_,
    };
  }

  fuchsia_hardware_network_driver::wire::TxBuffer& buffer() { return buffer_; }

 private:
  fuchsia_hardware_network_driver::wire::TxBuffer buffer_{};
  internal::BufferParts<fuchsia_hardware_network_driver::wire::BufferRegion> parts_{};
  zx_status_t status_ = ZX_OK;
};

class RxBuffer : public fbl::DoublyLinkedListable<std::unique_ptr<RxBuffer>> {
 public:
  explicit RxBuffer(const fuchsia_hardware_network_driver::wire::RxSpaceBuffer& space)
      : space_(space),
        return_part_({
            .id = space.id,
        }) {}

  zx_status_t WriteData(const std::vector<uint8_t>& data, const VmoProvider& vmo_provider) {
    return WriteData(cpp20::span(data.data(), data.size()), vmo_provider);
  }

  zx_status_t WriteData(cpp20::span<const uint8_t> data, const VmoProvider& vmo_provider);

  fuchsia_hardware_network_driver::wire::RxBufferPart& return_part() { return return_part_; }
  fuchsia_hardware_network_driver::wire::RxSpaceBuffer& space() { return space_; }

  void SetReturnLength(uint32_t length) { return_part_.length = length; }

 private:
  fuchsia_hardware_network_driver::wire::RxSpaceBuffer space_{};
  fuchsia_hardware_network_driver::wire::RxBufferPart return_part_{};
};

class RxFidlReturn : public fbl::DoublyLinkedListable<std::unique_ptr<RxFidlReturn>> {
 public:
  RxFidlReturn()
      : buffer_({
            .meta =
                {
                    .frame_type = netdev::wire::FrameType::kEthernet,
                },
            .data =
                fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBufferPart>::FromExternal(
                    parts_.data(), 0),
        }) {}
  // RxReturn can't be moved because it keeps pointers to the return buffer internally.
  RxFidlReturn(RxFidlReturn&&) = delete;
  RxFidlReturn(std::unique_ptr<RxBuffer> buffer, uint8_t port_id) : RxFidlReturn() {
    PushPart(std::move(buffer));
    buffer_.meta.port = port_id;
  }

  // Pushes buffer space into the return buffer.
  //
  // NB: We don't really need the unique pointer here, we just copy the information we need. But
  // requiring the unique pointer to be passed enforces the buffer ownership semantics. Also
  // RxBuffers usually sit in the available queue as a pointer already.
  void PushPart(std::unique_ptr<RxBuffer> buffer) {
    ZX_ASSERT(buffer_.data.size() < parts_.size());
    parts_[buffer_.data.size()] = buffer->return_part();
    buffer_.data.set_size(buffer_.data.size() + 1);
  }

  const fuchsia_hardware_network_driver::wire::RxBuffer& buffer() const { return buffer_; }
  fuchsia_hardware_network_driver::wire::RxBuffer& buffer() { return buffer_; }

 private:
  internal::BufferParts<fuchsia_hardware_network_driver::wire::RxBufferPart> parts_{};
  fuchsia_hardware_network_driver::wire::RxBuffer buffer_{};
};

constexpr zx_signals_t kEventStartInitiated = ZX_USER_SIGNAL_0;
constexpr zx_signals_t kEventStop = ZX_USER_SIGNAL_1;
constexpr zx_signals_t kEventTx = ZX_USER_SIGNAL_2;
constexpr zx_signals_t kEventSessionStarted = ZX_USER_SIGNAL_3;
constexpr zx_signals_t kEventRxAvailable = ZX_USER_SIGNAL_4;
constexpr zx_signals_t kEventStartCompleted = ZX_USER_SIGNAL_5;
constexpr zx_signals_t kEventPortActiveChanged = ZX_USER_SIGNAL_6;
constexpr zx_signals_t kEventSessionDied = ZX_USER_SIGNAL_7;

struct PortInfo {
  netdev::wire::PortClass port_class;
  std::vector<netdev::wire::FrameType> rx_types;
  std::vector<netdev::wire::FrameTypeSupport> tx_types;
};

struct PortStatus {
  uint32_t mtu;
  netdev::wire::StatusFlags flags;
};

class FakeNetworkPortImpl : public fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort> {
 public:
  using OnSetActiveCallback = fit::function<void(bool)>;
  using NetDevIfcClient = fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc>;

  FakeNetworkPortImpl();
  ~FakeNetworkPortImpl() override;

  void GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) override;
  void GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) override;
  void SetActive(fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request,
                 fdf::Arena& arena, SetActiveCompleter::Sync& completer) override;
  void GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) override;
  void Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) override;

  PortInfo& port_info() { return port_info_; }
  const PortStatus& status() const { return status_; }
  zx_status_t AddPort(uint8_t port_id, fdf_dispatcher_t* dispatcher,
                      fidl::WireSyncClient<netdev::Device> device, NetDevIfcClient& netdev_ifc);
  zx_status_t AddPortNoWait(uint8_t port_id, fdf_dispatcher_t* dispatcher,
                            fidl::WireSyncClient<netdev::Device> device,
                            NetDevIfcClient& netdev_ifc);
  void RemoveSync();
  void SetMac(fdf::ClientEnd<fuchsia_hardware_network_driver::MacAddr> client) {
    mac_client_end_ = std::move(client);
  }
  void SetOnSetActiveCallback(OnSetActiveCallback cb) { on_set_active_ = std::move(cb); }
  void SetSupportedRxType(netdev::wire::FrameType frame_type) {
    port_info_.rx_types = {frame_type};
  }
  void SetSupportedTxType(netdev::wire::FrameType frame_type) {
    port_info_.tx_types = {{.type = frame_type,
                            .features = netdev::wire::kFrameFeaturesRaw,
                            .supported_flags = netdev::wire::TxFlags(0)}};
  }

  void WaitPortRemoved();

  void WaitForPortRemoval() {
    ASSERT_OK(sync_completion_wait_deadline(&wait_removed_, zx::time::infinite().get()));
  }

  bool active() const { return port_active_; }
  bool removed() const { return port_removed_; }
  uint8_t id() const { return id_; }

  const zx::event& events() const { return event_; }

  void SetOnline(bool online);
  void SetStatus(const PortStatus& status);

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(FakeNetworkPortImpl);

  std::optional<fdf::ServerBindingRef<fuchsia_hardware_network_driver::NetworkPort>> binding_;
  NetDevIfcClient* netdev_ifc_ = nullptr;
  std::optional<fdf::ClientEnd<fuchsia_hardware_network_driver::MacAddr>> mac_client_end_;
  sync_completion_t wait_removed_;
  OnSetActiveCallback on_set_active_;
  uint8_t id_;
  PortInfo port_info_;
  std::atomic_bool port_active_ = false;
  PortStatus status_;
  zx::event event_;
  bool port_removed_ = false;
  bool port_added_ = false;
  std::optional<fidl::WireSyncClient<netdev::Device>> device_;
};

struct DeviceInfo {
  uint32_t device_features;
  uint16_t tx_depth;
  uint16_t rx_depth;
  uint16_t rx_threshold;
  uint16_t min_rx_buffers;
  uint8_t max_buffer_parts;
  uint32_t max_buffer_length;
  uint32_t buffer_alignment;
  uint32_t min_rx_buffer_length;
  uint32_t min_tx_buffer_length;
  uint16_t tx_head_length;
  uint16_t tx_tail_length;
};

class FakeNetworkDeviceImpl
    : public fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceImpl> {
 public:
  using PrepareVmoHandler =
      fit::function<bool(uint8_t, const zx::vmo&, PrepareVmoCompleter::Sync&)>;
  using ReleaseVmoHandler = fit::function<void(uint8_t)>;
  // |dispatcher| is optional, if it is not provided the impl dispatcher from a call to CreateChild
  // is used instead. This is provided for tests that don't call CreateChild.
  explicit FakeNetworkDeviceImpl(fdf_dispatcher_t* dispatcher = nullptr);
  ~FakeNetworkDeviceImpl() override;

  zx::result<std::unique_ptr<NetworkDeviceInterface>> CreateChild(
      DeviceInterfaceDispatchers dispatchers);

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

  fit::function<zx::unowned_vmo(uint8_t)> VmoGetter();

  const zx::event& events() const { return event_; }

  DeviceInfo& info() { return info_; }

  std::unique_ptr<RxBuffer> PopRxBuffer() __TA_EXCLUDES(lock_) {
    fbl::AutoLock lock(&lock_);
    return rx_buffers_.pop_front();
  }

  std::unique_ptr<TxBuffer> PopTxBuffer() __TA_EXCLUDES(lock_) {
    fbl::AutoLock lock(&lock_);
    return tx_buffers_.pop_front();
  }

  fbl::SizedDoublyLinkedList<std::unique_ptr<TxBuffer>> TakeTxBuffers() __TA_EXCLUDES(lock_) {
    fbl::AutoLock lock(&lock_);
    fbl::SizedDoublyLinkedList<std::unique_ptr<TxBuffer>> r;
    tx_buffers_.swap(r);
    return r;
  }

  fbl::SizedDoublyLinkedList<std::unique_ptr<RxBuffer>> TakeRxBuffers() __TA_EXCLUDES(lock_) {
    fbl::AutoLock lock(&lock_);
    fbl::SizedDoublyLinkedList<std::unique_ptr<RxBuffer>> r;
    rx_buffers_.swap(r);
    return r;
  }

  size_t rx_buffer_count() __TA_EXCLUDES(lock_) {
    fbl::AutoLock lock(&lock_);
    return rx_buffers_.size();
  }

  size_t tx_buffer_count() __TA_EXCLUDES(lock_) {
    fbl::AutoLock lock(&lock_);
    return tx_buffers_.size();
  }

  size_t queue_rx_space_called() __TA_EXCLUDES(lock_) {
    fbl::AutoLock lock(&lock_);
    ZX_ASSERT(!queue_rx_space_called_.empty());
    const size_t front = queue_rx_space_called_.front();
    queue_rx_space_called_.pop_front();
    return front;
  }

  size_t queue_tx_called() __TA_EXCLUDES(lock_) {
    fbl::AutoLock lock(&lock_);
    ZX_ASSERT(!queue_tx_called_.empty());
    const size_t front = queue_tx_called_.front();
    queue_tx_called_.pop_front();
    return front;
  }

  std::optional<uint8_t> first_vmo_id() {
    for (size_t i = 0; i < vmos_.size(); i++) {
      if (vmos_[i].is_valid()) {
        return i;
      }
    }
    return std::nullopt;
  }

  void set_auto_start(std::optional<zx_status_t> auto_start) { auto_start_ = auto_start; }

  void set_auto_stop(bool auto_stop) { auto_stop_ = auto_stop; }

  bool TriggerStart();
  bool TriggerStop();

  void set_immediate_return_tx(bool auto_return) { immediate_return_tx_ = auto_return; }
  void set_immediate_return_rx(bool auto_return) { immediate_return_rx_ = auto_return; }
  void set_prepare_vmo_handler(PrepareVmoHandler handler) {
    prepare_vmo_handler_ = std::move(handler);
  }
  void set_release_vmo_handler(ReleaseVmoHandler handler) {
    release_vmo_handler_ = std::move(handler);
  }

  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc>& client() {
    return device_client_;
  }

  void WaitReleased() {
    // TODO(nahurley): Figure out why we need to wait based on a signal. Why
    // isn't shutting down the server/client sufficient?
    bool all_released = true;
    for (auto& vmo : vmos_) {
      if (vmo.is_valid()) {
        all_released = false;
      }
    }

    if (!all_released) {
      ASSERT_OK(sync_completion_wait_deadline(&released_completer_, zx::time::infinite().get()));
    }
  }

  // Note that |on_start| will be called with the internal lock held, be careful of deadlocks.
  void SetOnStart(fit::function<void()>&& on_start) __TA_EXCLUDES(lock_) {
    fbl::AutoLock lock(&lock_);
    on_start_ = std::move(on_start);
  }

  cpp20::span<const zx::vmo> vmos() { return cpp20::span(vmos_.begin(), vmos_.end()); }

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(FakeNetworkDeviceImpl);

  class Binder : public NetworkDeviceImplBinder {
   public:
    Binder(FakeNetworkDeviceImpl* parent, fdf_dispatcher_t* dispatcher)
        : parent_(parent), dispatcher_(dispatcher) {}
    zx::result<fdf::ClientEnd<netdriver::NetworkDeviceImpl>> Bind() override;

   private:
    DISALLOW_COPY_ASSIGN_AND_MOVE(Binder);

    FakeNetworkDeviceImpl* parent_ = nullptr;
    std::optional<fdf::ServerBindingRef<netdriver::NetworkDeviceImpl>> binding_;
    fdf_dispatcher_t* dispatcher_ = nullptr;
  };

  fbl::Mutex lock_;
  sync_completion_t released_completer_;
  fdf_dispatcher_t* dispatcher_ = nullptr;
  std::array<zx::vmo, fuchsia_hardware_network::wire::kMaxDataVmos> vmos_;
  DeviceInfo info_{};
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> device_client_;
  fbl::SizedDoublyLinkedList<std::unique_ptr<RxBuffer>> rx_buffers_ __TA_GUARDED(lock_);
  fbl::SizedDoublyLinkedList<std::unique_ptr<TxBuffer>> tx_buffers_ __TA_GUARDED(lock_);
  std::deque<size_t> queue_tx_called_ __TA_GUARDED(lock_);
  std::deque<size_t> queue_rx_space_called_ __TA_GUARDED(lock_);
  zx::event event_;
  std::optional<zx_status_t> auto_start_ = ZX_OK;
  bool auto_stop_ = true;
  bool immediate_return_tx_ = false;
  bool immediate_return_rx_ = false;
  bool device_started_ __TA_GUARDED(lock_) = false;
  fit::function<void()> pending_start_callback_ __TA_GUARDED(lock_);
  fit::function<void()> pending_stop_callback_ __TA_GUARDED(lock_);
  fit::function<void()> on_start_ __TA_GUARDED(lock_);
  PrepareVmoHandler prepare_vmo_handler_;
  ReleaseVmoHandler release_vmo_handler_;
};

class RxFidlReturnTransaction {
 public:
  explicit RxFidlReturnTransaction(FakeNetworkDeviceImpl* impl)
      : return_buffers_(impl->info().rx_depth), client_(impl->client()) {}

  void Enqueue(std::unique_ptr<RxFidlReturn> buffer) {
    ZX_ASSERT(count_ < std::size(return_buffers_));
    return_buffers_[count_++] = buffer->buffer();
    buffers_.push_back(std::move(buffer));
  }

  void Enqueue(std::unique_ptr<RxBuffer> buffer, uint8_t port_id) {
    Enqueue(std::make_unique<RxFidlReturn>(std::move(buffer), port_id));
  }

  void Commit() {
    size_t remaining = count_;
    size_t offset = 0;
    while (remaining > 0) {
      const size_t batch = std::min<size_t>(remaining, netdriver::wire::kMaxRxBuffers);
      fdf::Arena arena('NETD');
      auto results =
          fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBuffer>::FromExternal(
              return_buffers_.data() + offset, batch);

      auto result = client_.buffer(arena)->CompleteRx(results);
      EXPECT_OK(result.status());

      offset += batch;
      remaining -= batch;
    }
    count_ = 0;
    buffers_.clear();
  }

 private:
  std::vector<netdriver::wire::RxBuffer> return_buffers_;
  size_t count_ = 0;
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc>& client_;
  fbl::DoublyLinkedList<std::unique_ptr<RxFidlReturn>> buffers_;

  DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(RxFidlReturnTransaction);
};

class TxFidlReturnTransaction {
 public:
  explicit TxFidlReturnTransaction(FakeNetworkDeviceImpl* impl)
      : return_buffers_(impl->info().tx_depth), client_(impl->client()) {}

  void Enqueue(std::unique_ptr<TxBuffer> buffer) {
    ZX_ASSERT(count_ < std::size(return_buffers_));
    return_buffers_[count_++] = buffer->result();
  }

  void Commit() {
    fdf::Arena arena('NETD');
    auto results = fidl::VectorView<fuchsia_hardware_network_driver::wire::TxResult>::FromExternal(
        return_buffers_.data(), count_);
    EXPECT_TRUE(client_.buffer(arena)->CompleteTx(results).ok());
    count_ = 0;
  }

 private:
  std::vector<netdriver::wire::TxResult> return_buffers_;
  size_t count_ = 0;
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc>& client_;

  DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(TxFidlReturnTransaction);
};

enum class LeaseHandleType : uint8_t {
  kChannel,
  kEventpair,
};

const char* LeaseHandleTypeToString(LeaseHandleType type);

std::tuple<netdev::DelegatedRxLease, zx::handle> CreateDelegatedLease(LeaseHandleType type,
                                                                      uint64_t hold_until_frame);

}  // namespace network::testing

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_TEST_UTIL_H_
