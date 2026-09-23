// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test_session.h"

namespace network::testing {

zx_status_t TestSession::Open(fidl::WireSyncClient<netdev::Device>& netdevice, const char* name,
                              netdev::wire::SessionFlags flags, uint16_t num_descriptors,
                              uint64_t buffer_size, std::vector<VmoConfig> vmos,
                              bool register_for_tx) {
  if (zx_status_t status = Init(num_descriptors, buffer_size, std::move(vmos)); status != ZX_OK) {
    return status;
  }
  zx::result info_status = GetInfo(flags);
  if (info_status.is_error()) {
    return info_status.status_value();
  }
  netdev::wire::SessionInfo& info = info_status.value();

  auto session_name = fidl::StringView::FromExternal(name);

  auto res = netdevice->OpenSession(session_name, info);
  if (res.status() != ZX_OK) {
    return res.status();
  }
  if (res->is_error()) {
    return res->error_value();
  }

  Setup(std::move(res->value()->session), std::move(res->value()->fifos));

  if (register_for_tx) {
    for (size_t i = 0; i < vmo_configs_.size(); ++i) {
      if (vmo_configs_[i].num_rx_buffers > 0) {
        continue;
      }
      uint8_t tx_vmo = static_cast<uint8_t>(i);
      auto reg_res = session_->RegisterForTx(fidl::VectorView<uint8_t>::FromExternal(&tx_vmo, 1));
      if (reg_res.status() != ZX_OK) {
        return reg_res.status();
      }
      if (reg_res->status != ZX_OK) {
        return reg_res->status;
      }
    }
  }

  return ZX_OK;
}

zx_status_t TestSession::Init(uint16_t descriptor_count, uint64_t buffer_size,
                              std::vector<VmoConfig> vmo_configs) {
  if (descriptors_vmo_.is_valid() || !vmo_configs_.empty() || session_.is_valid()) {
    return ZX_ERR_BAD_STATE;
  }

  if (vmo_configs.empty()) {
    zx::vmo data;
    if (zx_status_t status = zx::vmo::create(descriptor_count * buffer_size, 0, &data);
        status != ZX_OK) {
      return status;
    }
    vmo_configs.push_back(
        {.vmo = std::move(data), .num_rx_buffers = static_cast<uint16_t>(descriptor_count / 2)});
  }
  if (zx_status_t status =
          descriptors_.CreateAndMap(descriptor_count * sizeof(buffer_descriptor_t),
                                    ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &descriptors_vmo_);
      status != ZX_OK) {
    return status;
  }

  for (auto& config : vmo_configs) {
    fzl::VmoMapper mapper;
    if (zx_status_t status = mapper.Map(config.vmo, 0, 0, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE);
        status != ZX_OK) {
      return status;
    }
    vmo_configs_.push_back(std::move(config));
    data_mappers_.push_back(std::move(mapper));
  }
  descriptors_count_ = descriptor_count;
  buffer_length_ = buffer_size;
  return ZX_OK;
}

zx::result<netdev::wire::SessionInfo> TestSession::GetInfo(
    std::optional<netdev::wire::SessionFlags> with_flags) {
  if (vmo_configs_.empty() || !descriptors_vmo_.is_valid()) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  std::vector<netdev::wire::DataVmo> data_vmos;
  for (size_t i = 0; i < vmo_configs_.size(); ++i) {
    zx::vmo duplicate_vmo;
    if (zx_status_t status = vmo_configs_[i].vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate_vmo);
        status != ZX_OK) {
      return zx::error(status);
    }
    auto vmo = netdev::wire::DataVmo::Builder(alloc_)
                   .id(static_cast<uint8_t>(i))
                   .vmo(std::move(duplicate_vmo))
                   .num_rx_buffers(vmo_configs_[i].num_rx_buffers)
                   .Build();
    data_vmos.push_back(vmo);
  }

  zx::vmo descriptors_vmo;
  if (zx_status_t status = descriptors_vmo_.duplicate(ZX_RIGHT_SAME_RIGHTS, &descriptors_vmo);
      status != ZX_OK) {
    return zx::error(status);
  }

  auto builder =
      netdev::wire::SessionInfo::Builder(alloc_)
          .data(fidl::VectorView<netdev::wire::DataVmo>(alloc_, data_vmos))
          .descriptors(std::move(descriptors_vmo))
          .descriptor_version(NETWORK_DEVICE_DESCRIPTOR_VERSION)
          .descriptor_length(static_cast<uint8_t>(sizeof(buffer_descriptor_t) / sizeof(uint64_t)))
          .descriptor_count(descriptors_count_);
  if (with_flags.has_value()) {
    builder.options(with_flags.value());
  }
  return zx::ok(builder.Build());
}

void TestSession::Setup(fidl::ClientEnd<netdev::Session> session, netdev::wire::Fifos fifos) {
  session_ = fidl::WireSyncClient<netdev::Session>(std::move(session));
  fifos_ = std::move(fifos);
}

zx_status_t TestSession::AttachPort(netdev::wire::PortId port_id,
                                    std::vector<netdev::wire::FrameType> frame_types) {
  fidl::WireResult wire_result = session_->Attach(
      port_id, fidl::VectorView<netdev::wire::FrameType>::FromExternal(frame_types));
  if (!wire_result.ok()) {
    return wire_result.status();
  }

  const auto* res = wire_result.Unwrap();
  if (res->is_error()) {
    return res->error_value();
  }
  return ZX_OK;
}

zx_status_t TestSession::DetachPort(netdev::wire::PortId port_id) {
  fidl::WireResult wire_result = session_->Detach(port_id);
  if (!wire_result.ok()) {
    return wire_result.status();
  }

  const auto* res = wire_result.Unwrap();
  if (res->is_error()) {
    return res->error_value();
  }
  return ZX_OK;
}

zx_status_t TestSession::Close() { return session_->Close().status(); }

zx_status_t TestSession::WaitClosed(zx::time deadline) {
  return session_.client_end().channel().wait_one(ZX_CHANNEL_PEER_CLOSED, deadline, nullptr);
}

buffer_descriptor_t& TestSession::ResetDescriptor(uint16_t index, uint8_t vmo_id, uint64_t offset) {
  buffer_descriptor_t& desc = descriptor(index);
  desc = {
      .frame_type = static_cast<uint8_t>(netdev::wire::FrameType::kEthernet),
      .vmo_id = vmo_id,
      .offset = offset,
      .data_length = static_cast<uint32_t>(buffer_length_),
  };
  return desc;
}

void TestSession::ZeroVmo() {
  for (auto& mapper : data_mappers_) {
    memset(mapper.start(), 0x00, mapper.size());
  }
}

buffer_descriptor_t& TestSession::descriptor(uint16_t index) {
  ZX_ASSERT_MSG(index < descriptors_count_, "descriptor %d out of bounds (count = %d)", index,
                descriptors_count_);
  return *(reinterpret_cast<buffer_descriptor_t*>(descriptors_.start()) + index);
}

uint8_t* TestSession::tx_buffer(uint8_t vmo_id, uint64_t offset) {
  return reinterpret_cast<uint8_t*>(data_mappers_[vmo_id].start()) + offset;
}

zx_status_t TestSession::WaitRxAvailable(zx::time deadline) const {
  return fifos_.rx.wait_one(ZX_FIFO_READABLE, deadline, nullptr);
}

zx_status_t TestSession::FetchRx(uint16_t* descriptors, size_t count, size_t* actual) const {
  return fifos_.rx.read(sizeof(uint16_t), descriptors, count, actual);
}

zx_status_t TestSession::FetchTx(uint16_t* descriptors, size_t count, size_t* actual) const {
  return fifos_.tx.read(sizeof(uint16_t), descriptors, count, actual);
}

zx_status_t TestSession::SendRx(const uint16_t* descriptor, size_t count, size_t* actual) const {
  return fifos_.rx.write(sizeof(uint16_t), descriptor, count, actual);
}

zx_status_t TestSession::SendTx(const uint16_t* descriptor, size_t count, size_t* actual) const {
  return fifos_.tx.write(sizeof(uint16_t), descriptor, count, actual);
}

zx_status_t TestSession::SendTxData(const netdev::wire::PortId& port_id, uint16_t descriptor_index,
                                    uint8_t vmo_id, uint64_t offset,
                                    const std::vector<uint8_t>& data) {
  buffer_descriptor_t& desc = ResetDescriptor(descriptor_index, vmo_id, offset);
  if (zx_status_t status = vmo_configs_[vmo_id].vmo.write(&data.at(0), desc.offset, data.size());
      status != ZX_OK) {
    return status;
  }
  desc.port_id = {
      .base = port_id.base,
      .salt = port_id.salt,
  };
  desc.data_length = static_cast<uint32_t>(data.size());
  return SendTx(descriptor_index);
}

}  // namespace network::testing
