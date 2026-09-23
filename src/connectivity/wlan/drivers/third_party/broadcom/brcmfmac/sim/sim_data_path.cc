// Copyright (c) 2023 The Fuchsia Authors
//
// Permission to use, copy, modify, and/or distribute this software for any purpose with or without
// fee is hereby granted, provided that the above copyright notice and this permission notice
// appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
// SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
// AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
// NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE
// OF THIS SOFTWARE.

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_data_path.h"

#include <fidl/fuchsia.hardware.network.driver/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/env.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <wlan/drivers/macaddr.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_device.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim_utils.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {

namespace {

// NOTE: These can't conflict with the vmo id used internally by sim_fw.cc
constexpr uint8_t kTxVmoId = fuchsia_hardware_network::wire::kMaxDataVmos - 1;
constexpr uint8_t kRxVmoId = fuchsia_hardware_network::wire::kMaxDataVmos - 2;

zx::result<cpp20::span<uint8_t>> CreateAndMapVmo(zx::vmo& vmo, uint64_t req_size) {
  zx_status_t status = zx::vmo::create(req_size, 0, &vmo);
  if (status != ZX_OK) {
    return zx::error{status};
  }

  uint64_t size = 0;
  status = vmo.get_size(&size);
  if (status != ZX_OK) {
    return zx::error{status};
  }

  zx_vaddr_t addr = 0;
  status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size, &addr);
  if (status != ZX_OK) {
    return zx::error{status};
  }

  cpp20::span<uint8_t> ret = {reinterpret_cast<uint8_t*>(addr), size};
  return zx::success{ret};
}
}  // namespace

SimDataPath::~SimDataPath() {
  if (client_dispatcher_.get()) {
    client_dispatcher_.ShutdownAsync();
    client_dispatcher_shutdown_.Wait();
  }
  if (ifc_dispatcher_.get()) {
    ifc_dispatcher_.ShutdownAsync();
    ifc_dispatcher_shutdown_.Wait();
  }
  if (port_dispatcher_.get()) {
    port_dispatcher_.ShutdownAsync();
    port_dispatcher_shutdown_.Wait();
  }
}

void SimDataPath::Init(fidl::UnownedClientEnd<fuchsia_io::Directory> outgoing_dir_client,
                       fit::callback<void(zx_status_t)>&& on_complete) {
  auto client_dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      this, fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "datapath-client",
      [this](fdf_dispatcher_t*) { client_dispatcher_shutdown_.Signal(); });
  ZX_ASSERT(client_dispatcher.is_ok());
  client_dispatcher_ = std::move(client_dispatcher.value());

  auto ifc_dispatcher = fdf::SynchronizedDispatcher::Create(
      {}, "datapath-ifc", [this](fdf_dispatcher_t*) { ifc_dispatcher_shutdown_.Signal(); });
  ZX_ASSERT(ifc_dispatcher.is_ok());
  ifc_dispatcher_ = std::move(ifc_dispatcher.value());

  auto port_dispatcher = fdf::SynchronizedDispatcher::Create(
      {}, "datapath-port", [this](fdf_dispatcher_t*) { port_dispatcher_shutdown_.Signal(); });
  ZX_ASSERT(port_dispatcher.is_ok());
  port_dispatcher_ = std::move(port_dispatcher.value());

  auto netdev_client = drivers::components::test::NetworkDeviceClient::Create(
      outgoing_dir_client, client_dispatcher_.get());
  ZX_ASSERT(netdev_client.is_ok());
  netdev_client_ = std::move(*netdev_client);
  ZX_ASSERT(netdev_client_->is_valid());

  async::PostTask(client_dispatcher_.async_dispatcher(), [this, on_complete = std::move(
                                                                    on_complete)]() mutable {
    sim_device_.NetDev().WaitUntilServerConnected();

    test_net_dev_ifc_.add_port_ =
        [this](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcAddPortRequest* request,
               fdf::Arena& arena, auto& completer) {
          OnAddPort(request->id, std::move(request->port));
          completer.buffer(arena).Reply(ZX_OK);
        };
    test_net_dev_ifc_.remove_port_ =
        [this](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcRemovePortRequest* request,
               fdf::Arena& arena, auto& completer) { OnRemovePort(request->id); };
    test_net_dev_ifc_.complete_rx_ =
        [this](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteRxRequest* request,
               fdf::Arena& arena, auto& completer) {
          OnRxComplete(cpp20::span(request->rx.begin(), request->rx.end()));
        };
    test_net_dev_ifc_.complete_tx_ =
        [this](fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteTxRequest* request,
               fdf::Arena& arena, auto& completer) {
          OnTxComplete(cpp20::span(request->tx.begin(), request->tx.end()));
        };

    auto bind_result = test_net_dev_ifc_.Bind(ifc_dispatcher_.get());
    if (bind_result.is_error()) {
      fdf::error("Failed to bind NetworkDeviceIfc: {}", bind_result);
      on_complete(bind_result.status_value());
      return;
    }

    fdf::Arena arena('SMDP');
    auto result = netdev_client_->sync().buffer(arena)->Init(std::move(*bind_result));
    if (!result.ok()) {
      fdf::error("NetDevice init failed: {}", result.FormatDescription());
      on_complete(result.status());
      return;
    }
    if (result->s != ZX_OK) {
      fdf::error("NetDevice init failed: {}", zx_status_get_string(result->s));
      on_complete(result->s);
      return;
    }

    auto info = netdev_client_->sync().buffer(arena)->GetInfo();
    if (!info.ok()) {
      fdf::error("GetInfo failed: {}", info.FormatDescription());
      on_complete(info.status());
      return;
    }
    device_info_ = fidl::ToNatural(info->info);

    // setup tx vmo, which is just a single frame of size kMaxFrameSize
    zx::vmo tx_vmo;
    auto tx_result = CreateAndMapVmo(tx_vmo, kMaxFrameSize);
    if (tx_result.is_error()) {
      fdf::error("Failed to create and map TX VMO: {}", tx_result);
      on_complete(tx_result.status_value());
      return;
    }
    tx_span_ = tx_result.value();

    // setup rx vmo
    zx::vmo rx_vmo;
    auto rx_result = CreateAndMapVmo(rx_vmo, kMaxFrameSize);
    if (rx_result.is_error()) {
      fdf::error("Failed to create and map RX VMO: {}", rx_result);
      on_complete(rx_result.status_value());
      return;
    }
    rx_span_ = rx_result.value();

    // Give both vmos to net device
    auto prepare = netdev_client_->sync().buffer(arena)->PrepareVmo(kTxVmoId, std::move(tx_vmo));
    if (!prepare.ok() || prepare->s != ZX_OK) {
      fdf::error("Failed to prepare TX VMO: {}",
                 prepare.ok() ? zx_status_get_string(prepare->s) : prepare.FormatDescription());
      on_complete(prepare.ok() ? prepare->s : prepare.status());
      return;
    }
    prepare = netdev_client_->sync().buffer(arena)->PrepareVmo(kRxVmoId, std::move(rx_vmo));
    if (!prepare.ok() || prepare->s != ZX_OK) {
      fdf::error("Failed to prepare RX VMO: {}",
                 prepare.ok() ? zx_status_get_string(prepare->s) : prepare.FormatDescription());
      on_complete(prepare.ok() ? prepare->s : prepare.status());
      return;
    }

    auto start = netdev_client_->sync().buffer(arena)->Start();
    if (!start.ok() || start->s != ZX_OK) {
      fdf::error("Failed to start network device: {}",
                 start.ok() ? zx_status_get_string(start->s) : start.FormatDescription());
      on_complete(start.ok() ? start->s : start.status());
      return;
    }

    // Give the rx buffer to the network device on init
    QueueRxBuffer();

    on_complete(ZX_OK);
  });
}

void SimDataPath::TxEthernet(uint16_t id, MacAddr dst, MacAddr src, uint16_t type,
                             cpp20::span<const uint8_t> body) {
  const size_t frame_size = body.size() + sim_utils::kEthernetHeaderSize +
                            device_info_.tx_head_length().value() +
                            device_info_.tx_tail_length().value();
  ZX_ASSERT_MSG(frame_size <= kMaxFrameSize, "Ethernet frame size too large");

  auto data_span = tx_span_.subspan(*device_info_.tx_head_length(),
                                    body.size() + sim_utils::kEthernetHeaderSize);
  ZX_ASSERT(sim_utils::WriteEthernetFrame(data_span, dst, src, type, body) == ZX_OK);

  TxRegion(id, 0, data_span.size());
}

void SimDataPath::TxRaw(uint16_t id, const std::vector<uint8_t>& body) {
  const size_t frame_size =
      body.size() + device_info_.tx_head_length().value() + device_info_.tx_tail_length().value();
  ZX_ASSERT_MSG(frame_size <= kMaxFrameSize, "Frame size too large");

  auto data_span = tx_span_.subspan(*device_info_.tx_head_length(), body.size());
  memcpy(data_span.data(), body.data(), body.size());
  TxRegion(id, 0, data_span.size());
}

void SimDataPath::OnTxComplete(cpp20::span<fuchsia_hardware_network_driver::wire::TxResult> tx) {
  // Currently only one frame is sent at a time by the sim driver.
  // It is possible for this to be called with tx_count == 0 if frames owned internally by the
  // driver (not the network device) were transmitted.
  ZX_ASSERT(tx.size() <= 1);

  if (!tx.empty()) {
    tx_results_.push_back(tx[0]);
    complete_tx_called_.Signal();
  }
}

void SimDataPath::OnRxComplete(cpp20::span<fuchsia_hardware_network_driver::wire::RxBuffer> rx) {
  ZX_ASSERT(rx.size() == 1);

  ZX_ASSERT(!rx[0].data.empty());
  auto& region = rx[0].data[0];

  ZX_ASSERT(region.offset + region.length <= rx_span_.size());

  // Copy rx data out so that it can be verified by the test.
  rx_data_.emplace_back();
  rx_data_.back().resize(region.length);
  memcpy(rx_data_.back().data(), rx_span_.subspan(region.offset, region.length).data(),
         region.length);

  // Immediately return the rx buffer to network device so users can rx more data
  QueueRxBuffer();
}

void SimDataPath::OnAddPort(uint8_t id,
                            fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkPort>&& port) {
  ZX_ASSERT(port.is_valid());
  ZX_ASSERT(port_clients_.find(id) == port_clients_.end());
  port_clients_[id].Bind(std::move(port), port_dispatcher_.get());
}

void SimDataPath::OnRemovePort(uint8_t id) {
  auto client = port_clients_.find(id);
  ZX_ASSERT(client != port_clients_.end());
  fdf::Arena arena('SMDP');
  ZX_ASSERT(client->second.sync().buffer(arena)->Removed().ok());
  port_clients_.erase(client);
}

void SimDataPath::TxRegion(uint16_t id, uint64_t offset, uint64_t data_size) {
  fuchsia_hardware_network_driver::wire::BufferRegion regions[] = {{
      .vmo = kTxVmoId,
      .offset = offset,
      .length =
          data_size + device_info_.tx_head_length().value() + device_info_.tx_tail_length().value(),
  }};

  fdf::Arena arena('SMDP');
  fuchsia_hardware_network_driver::wire::TxBuffer buffers[] = {{
      .id = id,
      .data = {arena, regions},
      .meta =
          {
              .frame_type = fuchsia_hardware_network::wire::FrameType::kEthernet,
          },
      .head_length = *device_info_.tx_head_length(),
      .tail_length = *device_info_.tx_tail_length(),
  }};

  // Calling QueueTx should eventually result in a CompleteTx, reset the event to make sure we catch
  // this one. Since the SimDataPath only has one frame ever in flight it has to be the correct one.
  // If more frames are ever added this would need to trigger on the correct buffer ID only.
  complete_tx_called_.Reset();

  auto status = netdev_client_->sync().buffer(arena)->QueueTx({arena, buffers});
  ZX_ASSERT(status.ok());

  // Wait until TX has completed to make this call synchronous.
  complete_tx_called_.Wait();
}

void SimDataPath::QueueRxBuffer() {
  fuchsia_hardware_network_driver::wire::RxSpaceBuffer buffers[] = {
      {.id = 0, .region = {.vmo = kRxVmoId, .offset = 0, .length = kMaxFrameSize}}};

  fdf::Arena arena('SMDP');
  ZX_ASSERT(netdev_client_->sync().buffer(arena)->QueueRxSpace({arena, buffers}).ok());
}

}  // namespace wlan::brcmfmac
