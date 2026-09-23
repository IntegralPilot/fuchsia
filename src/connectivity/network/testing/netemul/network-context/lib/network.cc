// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "network.h"

#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <utility>

#include "consumer.h"
#include "fake_endpoint.h"
#include "interceptors/latency.h"
#include "interceptors/packet_loss.h"
#include "interceptors/reorder.h"
#include "lib/async/dispatcher.h"
#include "lib/syslog/cpp/macros.h"
#include "netdump.h"
#include "network_context.h"
#include "src/connectivity/lib/network-device/cpp/network_device_client.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace std {

template <typename T>
struct hash<fxl::WeakPtr<T>> {
  std::size_t operator()(fxl::WeakPtr<T> const& p) const noexcept {
    return std::hash<T*>{}(p.get());
  }
};

template <typename T>
struct equal_to<fxl::WeakPtr<T>> {
  bool operator()(fxl::WeakPtr<T> const& lhs, fxl::WeakPtr<T> const& rhs) const noexcept {
    return lhs.get() == rhs.get();
  }
};

}  // namespace std

namespace netemul {
namespace impl {

class Guest : public fuchsia::net::virtualization::Interface, public data::Consumer {
 public:
  explicit Guest(fidl::InterfaceRequest<fuchsia::net::virtualization::Interface> interface,
                 fidl::InterfaceHandle<fuchsia::hardware::network::Port> port,
                 async_dispatcher_t* dispatcher,
                 fidl::ClientEnd<fuchsia_hardware_network::Device> device)
      : binding_(this, std::move(interface), dispatcher),
        client_(std::move(device), dispatcher),
        weak_ptr_factory_(this) {
    port_.Bind(std::move(port), dispatcher);
  }

  // The binding retains a pointer to |this|, so |this| must never move.
  Guest(Guest&&) = delete;
  Guest& operator=(Guest&&) = delete;

  fidl::Binding<fuchsia::net::virtualization::Interface>& binding() { return binding_; }
  network::client::NetworkDeviceClient& client() { return client_; }
  fuchsia::hardware::network::PortPtr& port() { return port_; }
  void SetAttachedPort(fuchsia_hardware_network::wire::PortId port_id) {
    attached_port_id_ = port_id;
  }

  void Consume(const void* data, size_t len) override {
    if (!attached_port_id_.has_value()) {
      // Not yet attached to any ports.
      return;
    }
    const fuchsia_hardware_network::wire::PortId& port_id = attached_port_id_.value();
    network::client::NetworkDeviceClient::Buffer buffer = client_.AllocTx();
    if (!buffer.is_valid()) {
      FX_LOGS(ERROR) << "network device client TX buffers depleted, dropping " << len << " bytes";
      return;
    }
    buffer.data().SetPortId(port_id);
    buffer.data().SetFrameType(fuchsia_hardware_network::wire::FrameType::kEthernet);
    const size_t n = buffer.data().Write(data, len);
    if (n != len) {
      FX_LOGS(ERROR) << "network device client TX MTU (" << n << " bytes) exceeded, dropping "
                     << len << " bytes";
      return;
    }
    if (zx_status_t status = buffer.Send(); status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "network device client TX send failed; dropping " << len
                              << " bytes";
      return;
    }
  }

  std::string GetName(uint32_t idx) override { return "guest-" + std::to_string(idx); }

  fxl::WeakPtr<data::Consumer> GetPointer() { return weak_ptr_factory_.GetWeakPtr(); }

 private:
  // NB: Netdevice client uses LLCPP types so we store the port ID in the
  // compatible type.
  std::optional<fuchsia_hardware_network::wire::PortId> attached_port_id_;
  fuchsia::hardware::network::PortPtr port_;
  fidl::Binding<fuchsia::net::virtualization::Interface> binding_;
  network::client::NetworkDeviceClient client_;
  fxl::WeakPtrFactory<data::Consumer> weak_ptr_factory_;
};

class NetworkBus : public data::BusConsumer {
 public:
  NetworkBus() : weak_ptr_factory_(this) {}
  ~NetworkBus() override { StopCapture(); }

  fxl::WeakPtr<NetworkBus> GetPointer() { return weak_ptr_factory_.GetWeakPtr(); }

  void Consume(const void* data, size_t len, const data::Consumer::Ptr& sender) override {
    if (packet_capture_) {
      packet_capture_->Dump().WritePacket(data, len, sinks_[sender]);
    }
    if (interceptors_.empty()) {
      ForwardData(data, len, sender);
    } else {
      interceptors_.back()->Intercept(InterceptPacket(data, len, sender));
    }
  }

  std::unordered_map<zx_handle_t, FakeEndpoint>& fake_endpoints() { return fake_endpoints_; }
  std::unordered_map<zx_handle_t, Guest>& guests() { return guests_; }

  std::pair<std::unordered_map<data::Consumer::Ptr, uint32_t>::iterator, bool> AddSink(
      data::Consumer::Ptr sink) {
    uint32_t interface_id = 0;
    if (packet_capture_) {
      if (sink) {
        interface_id = packet_capture_->Dump().AddInterfaceWith(
            [sink](uint32_t idx) { return sink->GetName(idx); });
      }
    }
    return sinks_.insert(std::make_pair(sink, interface_id));
  }

  void RemoveSink(data::Consumer::Ptr sink) { sinks_.erase(sink); }

  void UpdateConfiguration(const Network::Config& config) {
    // first, flush all packets that are currently in interceptors:
    for (auto& i : interceptors_) {
      auto packets = i->Flush();
      for (auto& packet : packets) {
        ForwardData(packet.data().data(), packet.data().size(), packet.origin());
      }
    }
    // clear all interceptors
    interceptors_.clear();

    // rebuild interceptors based on configuration

    // reordering interceptor:
    if (config.has_reorder()) {
      interceptors_.emplace_back(new interceptor::Reorder(
          config.reorder().store_buff, zx::msec(config.reorder().tick), MakeInterceptorCallback()));
    }
    // latency interceptor:
    if (config.has_latency()) {
      interceptors_.emplace_back(new interceptor::Latency(
          config.latency().average, config.latency().std_dev, MakeInterceptorCallback()));
    }
    // packet loss interceptor:
    if (config.has_packet_loss()) {
      if (config.packet_loss().is_random_rate()) {
        interceptors_.emplace_back(new interceptor::PacketLoss(config.packet_loss().random_rate(),
                                                               MakeInterceptorCallback()));
      }
    }
  }

  zx_status_t StartCapture(const std::string& pcap_name) {
    if (packet_capture_) {
      return ZX_ERR_ALREADY_EXISTS;
    }
    packet_capture_ = std::make_unique<PacketCapture>(GetPointer(), pcap_name);
    for (auto it = sinks_.begin(); it != sinks_.end();) {
      auto& [sink, interface_id] = *it;
      if (sink) {
        interface_id = packet_capture_->Dump().AddInterfaceWith([it](uint32_t idx) {
          auto& [sink, _] = *it;
          return sink->GetName(idx);
        });
        ++it;
      } else {
        it = sinks_.erase(it);
      }
    }
    return ZX_OK;
  }

  void StopCapture() { packet_capture_ = nullptr; }

 private:
  Interceptor::ForwardPacketCallback MakeInterceptorCallback() {
    if (interceptors_.empty()) {
      // if no interceptors inside just forward packet
      return [this](InterceptPacket packet) {
        ForwardData(packet.data().data(), packet.data().size(), packet.origin());
      };
    }
    // if interceptors already have members, point onto the back for chaining
    auto* next = interceptors_.back().get();
    return [next](InterceptPacket packet) { next->Intercept(std::move(packet)); };
  }

  void ForwardData(const void* data, size_t len, const data::Consumer::Ptr& sender) {
    for (auto i = sinks_.begin(); i != sinks_.end();) {
      auto& [sink, interface_id] = *i;
      if (sink) {
        if (sink.get() != sender.get()) {  // flood all sinks other than sender
          sink->Consume(data, len);
        }
        ++i;
      } else {
        // sink has been freed, erase it
        i = sinks_.erase(i);
      }
    }
  }

  fxl::WeakPtrFactory<NetworkBus> weak_ptr_factory_;
  // Mapping from sink to their interface ID in the packet capture.
  std::unordered_map<data::Consumer::Ptr, uint32_t> sinks_;
  std::unordered_map<zx_handle_t, FakeEndpoint> fake_endpoints_;
  std::unordered_map<zx_handle_t, Guest> guests_;
  std::vector<std::unique_ptr<Interceptor>> interceptors_;

  std::unique_ptr<PacketCapture> packet_capture_;
};

}  // namespace impl

Network::Network(NetworkContext* context, std::string name, Network::Config config)
    : bus_(new impl::NetworkBus()),
      parent_(context),
      name_(std::move(name)),
      config_(std::move(config)) {
  bindings_.set_empty_set_handler([this]() {
    if (closed_callback_) {
      closed_callback_(*this);
    }
  });
  bus_->UpdateConfiguration(config_);
}

Network::~Network() = default;

void Network::Bind(fidl::InterfaceRequest<FNetwork> req) {
  bindings_.AddBinding(this, std::move(req), parent_->dispatcher());
}

void Network::AddPort(fidl::InterfaceHandle<fuchsia::hardware::network::Port> port,
                      fidl::InterfaceRequest<fuchsia::net::virtualization::Interface> interface) {
  zx::result device_endpoints = fidl::CreateEndpoints<fuchsia_hardware_network::Device>();
  if (device_endpoints.is_error()) {
    FX_PLOGS(ERROR, device_endpoints.status_value()) << "failed to create device endpoints";
    interface.Close(ZX_ERR_INTERNAL);
    return;
  }
  auto [device_client_end, device_server_end] = std::move(device_endpoints.value());
  const zx_handle_t key = interface.channel().get();
  auto [it, inserted] =
      bus_->guests().try_emplace(key, std::move(interface), std::move(port), parent_->dispatcher(),
                                 std::move(device_client_end));
  if (!inserted) {
    interface.Close(ZX_ERR_INTERNAL);
    return;
  }
  impl::Guest& guest = it->second;
  auto cleanup = [this, key, &guest](zx_status_t status) {
    bus_->RemoveSink(guest.GetPointer());
    guest.binding().Close(status);
    // There may be other tasks running on the promise executor; schedule destruction so that it
    // happens after other pending work that may want to access the executor.
    async::PostTask(parent_->dispatcher(), [this, key]() { bus_->guests().erase(key); });
  };

  {
    auto [it, inserted] = bus_->AddSink(guest.GetPointer());
    if (!inserted) {
      cleanup(ZX_ERR_INTERNAL);
      return;
    }
  }
  guest.binding().set_error_handler(cleanup);
  guest.client().SetErrorCallback(cleanup);
  guest.client().SetRxCallback([this, &guest](network::client::NetworkDeviceClient::Buffer buffer) {
    switch (buffer.data().parts()) {
      case 0:
        break;
      case 1: {
        const cpp20::span src = buffer.data().part(0).data();
        bus_->Consume(src.data(), src.size(), guest.GetPointer());
      } break;
      default: {
        std::vector<uint8_t> dst;
        for (uint32_t i = 0; i < buffer.data().parts(); ++i) {
          const cpp20::span src = buffer.data().part(i).data();
          std::copy(src.begin(), src.end(), std::back_inserter(dst));
        }
        bus_->Consume(dst.data(), dst.size(), guest.GetPointer());
      };
    }
  });
  guest.port().set_error_handler(cleanup);
  guest.port()->GetInfo([this, device = std::move(device_server_end), &guest,
                         cleanup](fuchsia::hardware::network::PortInfo info) mutable {
    fuchsia::hardware::network::PortId hlcpp_port_id = info.id();
    fuchsia_hardware_network::wire::PortId port_id = {
        .base = hlcpp_port_id.base,
        .salt = hlcpp_port_id.salt,
    };
    guest.port()->GetDevice(
        fidl::InterfaceRequest<fuchsia::hardware::network::Device>(device.TakeChannel()));
    guest.client().OpenSession(name_, [port_id, &guest, cleanup](zx_status_t status) {
      if (status != ZX_OK) {
        cleanup(status);
        return;
      }
      guest.SetAttachedPort(port_id);
      guest.client().AttachPort(port_id, {fuchsia_hardware_network::wire::FrameType::kEthernet},
                                [cleanup](zx_status_t status) {
                                  if (status != ZX_OK) {
                                    cleanup(status);
                                  }
                                });
    });
  });
}

void Network::GetConfig(Network::GetConfigCallback callback) {
  Config config;
  config_.Clone(&config);
  callback(std::move(config));
}

void Network::GetName(Network::GetNameCallback callback) { callback(name_); }

void Network::SetConfig(fuchsia::netemul::network::NetworkConfig config,
                        Network::SetConfigCallback callback) {
  if (!CheckConfig(config)) {
    callback(ZX_ERR_INVALID_ARGS);
    return;
  }
  config_ = std::move(config);
  bus_->UpdateConfiguration(config_);
  callback(ZX_OK);
}

zx_status_t Network::AttachEndpoint(std::string name) {
  data::Consumer::Ptr src;
  auto status = parent_->endpoint_manager().InstallSink(std::move(name), bus_->GetPointer(), &src);

  if (status != ZX_OK) {
    return status;
  }
  if (!src) {
    return ZX_ERR_INTERNAL;
  }

  auto [it, inserted] = bus_->AddSink(std::move(src));
  if (!inserted) {
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

void Network::AttachEndpoint(::std::string name, Network::AttachEndpointCallback callback) {
  callback(AttachEndpoint(std::move(name)));
}

void Network::RemoveEndpoint(::std::string name, Network::RemoveEndpointCallback callback) {
  data::Consumer::Ptr src;
  auto status = parent_->endpoint_manager().RemoveSink(name, bus_->GetPointer(), &src);
  if (status == ZX_OK && src) {
    bus_->RemoveSink(src);
  }
  callback(status);
}

void Network::CreateFakeEndpoint(
    fidl::InterfaceRequest<fuchsia::netemul::network::FakeEndpoint> ep) {
  const zx_handle_t key = ep.channel().get();
  auto [it, inserted] = bus_->fake_endpoints().try_emplace(key, bus_->GetPointer(), std::move(ep),
                                                           parent_->dispatcher());
  ZX_ASSERT(inserted);
  FakeEndpoint& fep = it->second;

  {
    auto [it, inserted] = bus_->AddSink(fep.GetPointer());
    ZX_ASSERT(inserted);
  }

  fep.SetOnDisconnected([this, key, &fep]() {
    bus_->RemoveSink(fep.GetPointer());
    bus_->fake_endpoints().erase(key);
  });
}

void Network::SetClosedCallback(Network::ClosedCallback cb) { closed_callback_ = std::move(cb); }

bool Network::CheckConfig(const Config& config) {
  if (config.has_packet_loss()) {
    if (config.packet_loss().is_random_rate()) {
      // random rate packet loss must be unsigned byte less or equal to 100
      if (config.packet_loss().random_rate() > 100) {
        return false;
      }
    } else {
      // non random-rate packet loss not supported.
      return false;
    }
  }

  return true;
}

void Network::StartCapture(::std::string name, Network::StartCaptureCallback callback) {
  callback(bus_->StartCapture(name));
}
void Network::StopCapture(Network::StopCaptureCallback callback) {
  bus_->StopCapture();
  callback();
}

PacketCapture::PacketCapture(fxl::WeakPtr<impl::NetworkBus> bus, const std::string& pcap_name)
    : pcap_file_(std::string("/custom_artifacts/") + pcap_name + ".pcapng",
                 std::ios::out | std::ios::binary),
      dump_(&pcap_file_),
      bus_(std::move(bus)) {}

NetworkDump& PacketCapture::Dump() { return dump_; }

void PacketCapture::Stop() {
  if (bus_) {
    bus_->StopCapture();
    bus_ = nullptr;
  }
  pcap_file_.flush();
}

}  // namespace netemul
