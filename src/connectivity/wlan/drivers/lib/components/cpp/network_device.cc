// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/connectivity/wlan/drivers/lib/components/cpp/include/wlan/drivers/components/network_device.h"

#include <fidl/fuchsia.hardware.network.driver/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network/cpp/wire.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fidl/cpp/wire_natural_conversions.h>
#include <lib/zx/vmar.h>

namespace netdriver = fuchsia_hardware_network_driver;

namespace {

void PopulateRxBuffer(netdriver::wire::RxBuffer& buffer, netdriver::wire::RxBufferPart& buffer_part,
                      const wlan::drivers::components::Frame& frame) {
  buffer.data = ::fidl::VectorView<::netdriver::wire::RxBufferPart>::FromExternal(&buffer_part, 1);
  buffer.meta.port = frame.PortId();
  buffer.meta.frame_type = ::fuchsia_hardware_network::wire::FrameType::kEthernet;
  buffer_part.id = frame.BufferId();
  buffer_part.length = frame.Size();
  buffer_part.offset = frame.Headroom();
}

}  // anonymous namespace

namespace wlan::drivers::components {

NetworkDevice::Callbacks::~Callbacks() = default;

NetworkDevice::NetworkDevice(Callbacks* callbacks) : callbacks_(callbacks) {}

NetworkDevice::~NetworkDevice() {
  ZX_ASSERT_MSG(!binding_.has_value(),
                "NetworkDevice is bound. Call Remove and wait for NetDevRelease first.");
  ZX_ASSERT_MSG(outgoing_directory_ == nullptr,
                "NetworkDevice service registered. Call Remove and wait for NetDevRelease first.");
  ZX_ASSERT_MSG(!node_controller_.is_valid(),
                "NetworkDevice node valid. Call Remove and wait for NetDevRelease first.");
  ZX_ASSERT_MSG(state_ == State::UnInitialized || state_ == State::Released,
                "NetworkDevice not released yet. Call Remove and wait for NetDevRelease first.");
}

zx_status_t NetworkDevice::Initialize(fidl::WireClient<fuchsia_driver_framework::Node>& parent,
                                      fdf_dispatcher_t* dispatcher,
                                      fdf::OutgoingDirectory& outgoing, const char* device_name) {
  std::lock_guard lock(mutex_);
  if (state_ != State::UnInitialized) {
    fdf::error("NetworkDevice already initialized");
    return ZX_ERR_ALREADY_BOUND;
  }
  state_ = State::Initialized;

  netdev_dispatcher_ = dispatcher;

  if (zx_status_t status = AddService(outgoing); status != ZX_OK) {
    // AddService should have logged an error message already.
    return status;
  }

  fidl::Arena arena;
  auto offer = fdf::MakeOffer2<netdriver::Service>(arena, component::kDefaultInstance);
  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(device_name)
                  .offers2(arena, std::vector{offer})
                  .Build();

  auto [client, server] = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  auto result = parent.sync()->AddChild(args, std::move(server), {});
  if (!result.ok()) {
    fdf::error("Failed to make add child call: {}", result.FormatDescription());
    return result.status();
  }
  if (result->is_error()) {
    fdf::error("Failed to add child: {}", static_cast<uint32_t>(result->error_value()));
    return ZX_ERR_INTERNAL;
  }
  node_controller_.Bind(std::move(client), fdf::Dispatcher::GetCurrent()->async_dispatcher(), this);

  return ZX_OK;
}

bool NetworkDevice::Remove() {
  std::lock_guard lock(mutex_);
  if (state_ == State::UnInitialized || state_ == State::Released) {
    // Never initialized or already removed. No need for a removal.
    return false;
  }
  if (state_ != State::Initialized) {
    // Removal is already in progress. Completion will be signaled through NetDevRelease.
    return true;
  }
  RemoveLocked();
  return true;
}

void NetworkDevice::RemoveLocked() {
  // Attempt to remove the network device child node first. This is to allow the network device
  // driver to complete shutdown properly which involves calling into our NetworkDeviceImpl server.
  // Keep the server alive until after the node is gone.
  if (node_controller_.is_valid()) {
    // Remove the child node first as mentioned above. The next step in the removal process will be
    // triggered by the on_fidl_error handler which is called when the node removal is complete.
    TransitionToState(State::RemovingNode, [this] { RemoveNode(); });
  } else if (outgoing_directory_) {
    // Remove the service. The next step will be triggered by the RemoveService call itself as
    // service removal is a synchronous process.
    TransitionToState(State::RemovingService, [this] { RemoveService(); });
  } else if (binding_.has_value()) {
    // The service has been removed, it's now safe to unbind the server. The remaining removal will
    // be triggered by the unbind handler.
    TransitionToState(State::Unbinding, [this] { Unbind(); });
  } else {
    // Server has been unbound, call release. This will trigger the transition to the final state,
    // Released, and call NetDevRelease on the callbacks.
    TransitionToState(State::Releasing, [this] { Release(); });
  }
}

void NetworkDevice::TransitionToState(State new_state, fit::function<void()>&& task) {
  if (state_ >= new_state) {
    // Don't perform the task if the desired or a subsequent state has already been reached.
    return;
  }
  state_ = new_state;
  async::PostTask(parent_dispatcher_, [task = std::move(task)] { task(); });
}

void NetworkDevice::WaitUntilServerConnected() { server_connected_.Wait(); }

fdf::WireSharedClient<netdriver::NetworkDeviceIfc>& NetworkDevice::NetDevIfcClient() {
  return netdev_ifc_;
}

void NetworkDevice::CompleteRx(Frame&& frame) {
  if (!ShouldCompleteFrame(frame)) {
    return;
  }

  // Use a FIDL arena here as it can allocate storage on the stack. This avoid any memory
  // allocations that would happen with an fdf::Arena. The default FIDL arena size should be plenty.
  fidl::Arena arena;
  // Still need an fdf::Arena for the actual call.
  fdf::Arena fdf_arena('NETD');

  fidl::VectorView<netdriver::wire::RxBuffer> buffers(arena, 1u);
  fidl::VectorView<netdriver::wire::RxBufferPart> buffer_part(arena, 1u);

  PopulateRxBuffer(buffers[0], buffer_part[0], frame);
  frame.ReleaseFromStorage();

  auto status = netdev_ifc_.buffer(fdf_arena)->CompleteRx(buffers);
  if (!status.ok()) {
    fdf::error("Failed to complete RX: {}", status.FormatDescription());
    return;
  }
}

void NetworkDevice::CompleteRx(FrameContainer&& frames) {
  // Use a FIDL arena here as it can allocate storage on the stack. This avoid any memory
  // allocations that would happen with an fdf::Arena. Figure out the maximum possible arena size
  // needed and use that.
  constexpr size_t kRxBuffersPerBatch = netdriver::wire::kMaxRxBuffers;
  constexpr size_t kArenaSize =
      fidl::MaxSizeInChannel<netdriver::wire::NetworkDeviceIfcCompleteRxRequest,
                             fidl::MessageDirection::kSending>();
  fidl::Arena<kArenaSize> arena;
  // Still need an fdf::Arena for the actual call.
  fdf::Arena fdf_arena('NETD');

  const size_t buffers_to_fill = std::min(kRxBuffersPerBatch, frames.size());
  fidl::VectorView<netdriver::wire::RxBuffer> buffers(arena, buffers_to_fill);
  fidl::VectorView<netdriver::wire::RxBufferPart> buffer_parts(arena, buffers_to_fill);

  for (auto frame = frames.begin(); frame != frames.end();) {
    size_t count = 0;
    for (; count < kRxBuffersPerBatch && frame != frames.end(); ++frame) {
      if (!ShouldCompleteFrame(*frame)) [[unlikely]] {
        continue;
      }
      PopulateRxBuffer(buffers[count], buffer_parts[count], *frame);
      frame->ReleaseFromStorage();
      ++count;
    }
    if (count > 0) [[likely]] {
      // The number of buffers might have changed as some frames should not be completed.
      buffers.set_size(count);
      auto status = netdev_ifc_.buffer(fdf_arena)->CompleteRx(buffers);
      if (!status.ok()) {
        fdf::error("Failed to complete RX: {}", status.FormatDescription());
        return;
      }
    }
  }
}

void NetworkDevice::CompleteTx(cpp20::span<Frame> frames, zx_status_t status) {
  // Use a FIDL arena here as it can allocate storage on the stack. This avoid any memory
  // allocations that would happen with an fdf::Arena. Figure out the maximum possible arena size
  // needed and use that.
  constexpr size_t kTxBuffersPerBatch = netdriver::wire::kMaxTxBuffers;
  constexpr size_t kArenaSize =
      fidl::MaxSizeInChannel<netdriver::wire::NetworkDeviceIfcCompleteTxRequest,
                             fidl::MessageDirection::kSending>();
  fidl::Arena<kArenaSize> arena;
  // Still need an fdf::Arena for the actual call.
  fdf::Arena fdf_arena('NETD');

  fidl::VectorView<netdriver::wire::TxResult> results(arena, kTxBuffersPerBatch);

  const size_t results_to_fill = std::min(results.size(), frames.size());
  for (auto frame = frames.begin(); frame != frames.end();) {
    size_t count = 0;
    for (; count < results_to_fill && frame != frames.end(); ++frame) {
      if (vmo_addrs_[frame->VmoId()] == nullptr) [[unlikely]] {
        // Do not complete TX for frames that don't belong to the network device. These could be
        // remnants from after a stop that should not be completed or frames used internally by
        // the driver that made their way to this method.
        continue;
      }
      results[count].id = frame->BufferId();
      results[count].status = status;
      ++count;
    }
    if (count > 0) [[unlikely]] {
      results.set_size(count);
      auto status = netdev_ifc_.buffer(fdf_arena)->CompleteTx(results);
      if (!status.ok()) {
        fdf::error("Failed to complete TX: {}", status.FormatDescription());
        return;
      }
    }
  }
}

// NetworkDeviceImpl implementation
void NetworkDevice::Init(netdriver::wire::NetworkDeviceImplInitRequest* request, fdf::Arena& arena,
                         InitCompleter::Sync& completer) {
  {
    std::lock_guard lock(mutex_);
    netdev_ifc_.Bind(std::move(request->iface), netdev_dispatcher_);
  }
  callbacks_->NetDevInit(Callbacks::InitTxn(completer));
}

void NetworkDevice::Start(fdf::Arena& arena, StartCompleter::Sync& completer) {
  bool expected = false;
  if (!started_.compare_exchange_strong(expected, true)) {
    fdf::error("NetworkDevice already started, unexpected Start call");
    return;
  }
  callbacks_->NetDevStart(Callbacks::StartTxn(completer));
}

void NetworkDevice::Stop(fdf::Arena& arena, StopCompleter::Sync& completer) {
  bool expected = true;
  if (!started_.compare_exchange_strong(expected, false)) {
    fdf::error("NetworkDevice not started, unexpected Stop call");
    return;
  }
  callbacks_->NetDevStop(Callbacks::StopTxn(completer));
}

void NetworkDevice::GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) {
  netdriver::DeviceImplInfo info;
  callbacks_->NetDevGetInfo(&info);

  completer.buffer(arena).Reply(fidl::ToWire(arena, std::move(info)));
}

void NetworkDevice::QueueTx(netdriver::wire::NetworkDeviceImplQueueTxRequest* request,
                            fdf::Arena& arena, QueueTxCompleter::Sync& completer) {
  if (!started_.load(std::memory_order_relaxed)) {
    // Do not queue TX on a device that is not started. This can happen when a device is being
    // stopped. For threading reasons the Network Device cannot hold locks when its calling into
    // our implementation so it's possible that it will attempt to queue TX after it has called
    // stop. These buffers will need to be completed with an error status.
    fidl::VectorView<netdriver::wire::TxResult> results(arena, request->buffers.size());
    for (size_t i = 0; i < request->buffers.size(); ++i) {
      const netdriver::wire::TxBuffer& buffer = request->buffers[i];
      results[i].id = buffer.id;
      results[i].status = ZX_ERR_UNAVAILABLE;
    }
    auto status = netdev_ifc_.buffer(arena)->CompleteTx(results);
    if (!status.ok()) {
      fdf::error("Failed to reject TX: {}", status.FormatDescription());
      return;
    }
    return;
  }
  tx_frames_.reserve(request->buffers.size());
  for (size_t i = 0; i < request->buffers.size(); ++i) {
    const netdriver::wire::TxBuffer& buffer = request->buffers[i];
    const netdriver::wire::BufferRegion& region = buffer.data[0];
    tx_frames_.emplace_back(nullptr, region.vmo, region.offset, buffer.id,
                            vmo_addrs_[region.vmo] + region.offset, region.length,
                            buffer.meta.port);
    tx_frames_.back().ShrinkHead(buffer.head_length);
    tx_frames_.back().ShrinkTail(buffer.tail_length);
  }
  callbacks_->NetDevQueueTx(tx_frames_);
  tx_frames_.clear();
}

void NetworkDevice::QueueRxSpace(netdriver::wire::NetworkDeviceImplQueueRxSpaceRequest* request,
                                 fdf::Arena& arena, QueueRxSpaceCompleter::Sync& completer) {
  callbacks_->NetDevQueueRxSpace(cpp20::span(request->buffers.begin(), request->buffers.end()),
                                 vmo_addrs_);
}

void NetworkDevice::PrepareVmo(netdriver::wire::NetworkDeviceImplPrepareVmoRequest* request,
                               fdf::Arena& arena, PrepareVmoCompleter::Sync& completer) {
  const zx_status_t status = [&]() {
    uint64_t size = 0;
    const uint8_t id = request->id;
    zx::vmo& vmo = request->vmo;
    zx_status_t status = vmo.get_size(&size);
    if (status != ZX_OK) {
      fdf::error("Failed to get VMO size on prepare: {}", zx_status_get_string(status));
      return status;
    }

    zx_vaddr_t addr = 0;
    status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size, &addr);
    if (status != ZX_OK) {
      fdf::error("Failed to map VMO on prepare: {}", zx_status_get_string(status));
      return status;
    }
    vmo_addrs_[id] = reinterpret_cast<uint8_t*>(addr);

    vmo_lengths_[id] = size;

    return callbacks_->NetDevPrepareVmo(id, std::move(vmo), vmo_addrs_[id], size);
  }();

  completer.buffer(arena).Reply(status);
}

void NetworkDevice::ReleaseVmo(netdriver::wire::NetworkDeviceImplReleaseVmoRequest* request,
                               fdf::Arena& arena, ReleaseVmoCompleter::Sync& completer) {
  const uint8_t id = request->id;
  callbacks_->NetDevReleaseVmo(id);
  zx_status_t status =
      zx::vmar::root_self()->unmap(reinterpret_cast<zx_vaddr_t>(vmo_addrs_[id]), vmo_lengths_[id]);
  if (status != ZX_OK) {
    fdf::error("Failed to unmap VMO on release: {}", zx_status_get_string(status));
  }
  vmo_addrs_[id] = nullptr;
  vmo_lengths_[id] = 0;

  completer.buffer(arena).Reply();
}

void NetworkDevice::on_fidl_error(fidl::UnbindInfo error) {
  if (error.status() != ZX_OK) {
    fdf::error("NetworkDevice fidl error: {}", error.FormatDescription());
    return;
  }
  // Begin or continue removal. Either the child node was removed because we initiated the removal
  // or because the node unexpectedly went away.
  std::lock_guard lock(mutex_);
  node_controller_ = {};
  RemoveLocked();
}

zx_status_t NetworkDevice::AddService(fdf::OutgoingDirectory& outgoing) {
  outgoing_directory_ = &outgoing;
  parent_dispatcher_ = fdf::Dispatcher::GetCurrent()->async_dispatcher();
  netdriver::Service::InstanceHandler handler(
      {.network_device_impl = [this](fdf::ServerEnd<netdriver::NetworkDeviceImpl> server) mutable {
        std::lock_guard lock(mutex_);
        binding_ = fdf::BindServer(
            netdev_dispatcher_, std::move(server), this,
            [this](NetworkDevice*, fidl::UnbindInfo info,
                   fdf::ServerEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl>) {
              if (!info.is_user_initiated()) {
                fdf::warn("NetworkDevice server unbound: {}", info.FormatDescription());
              }
              std::lock_guard lock(mutex_);
              binding_.reset();
              // Remove the entire network device at this point as it can no longer be used. This
              // will trigger NetDevRelease to indicate to the owner of this object that the
              // NetworkDevice is going away. It should be safe to do this even if this unbind is
              // part of another Remove call as that is part of the removal process.
              RemoveLocked();
            });
        server_connected_.Signal();
      }});

  auto status = outgoing.AddService<netdriver::Service>(std::move(handler));
  if (status.is_error()) {
    fdf::error("Failed to add outgoing service: {}", status.status_string());
    return status.status_value();
  }

  return ZX_OK;
}

bool NetworkDevice::ShouldCompleteFrame(const Frame& frame) {
  if (vmo_addrs_[frame.VmoId()] == nullptr) [[unlikely]] {
    // Do not return frames with VMOs we are not aware of. This can happen if the frame
    // belongs to some internal VMO in the driver, by skipping it here we allow the driver
    // some relaxation in what it passes to us. It can also happen if we are attempting to
    // complete receives after a VMO has been released, at that point returning the frame
    // serves no purpose.
    return false;
  }
  return true;
}

void NetworkDevice::Unbind() {
  std::lock_guard lock(mutex_);
  if (binding_.has_value()) {
    binding_->Unbind();
  }
}

void NetworkDevice::RemoveService() {
  std::lock_guard lock(mutex_);
  if (outgoing_directory_) {
    auto result = outgoing_directory_->RemoveService<netdriver::Service>(fdf::kDefaultInstance);
    if (result.is_error()) {
      fdf::error("Failed to remove service from outgoing directory: {}", result.status_string());
    }
    outgoing_directory_ = nullptr;
  }
  // Because service removal is synchronous there is no asynchronous notification of completion.
  // The removal process has to be continued here instead.
  RemoveLocked();
}

void NetworkDevice::RemoveNode() {
  std::lock_guard lock(mutex_);
  if (node_controller_.is_valid()) {
    auto result = node_controller_->Remove();
    if (!result.ok()) {
      fdf::error("Controller node remove failed, FIDL error: {}", result.FormatDescription());
    }
    // Don't destroy the node controller here, it needs to stay alive to call on_fidl_error to
    // continue the removal procedure.
  }
}

void NetworkDevice::Release() {
  // Now that everything is shut down and removed, move back to the Released state. Don't hold any
  // locks during the callback. The NetworkDevice object might be destroyed in the callback so any
  // lock would then attempt to unlock a destroyed mutex when we exit this scope. The state
  // transition happens before the callback for the same reason.
  {
    std::lock_guard lock(mutex_);
    state_ = State::Released;
  }
  callbacks_->NetDevRelease();
}

}  // namespace wlan::drivers::components
