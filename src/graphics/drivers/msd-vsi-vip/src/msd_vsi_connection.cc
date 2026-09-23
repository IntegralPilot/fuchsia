// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "msd_vsi_connection.h"

#include "address_space_layout.h"
#include "msd_vsi_buffer.h"
#include "msd_vsi_context.h"

std::unique_ptr<msd::Context> MsdVsiAbiConnection::MsdCreateContext() {
  auto connection = ptr();

  auto context =
      MsdVsiContext::Create(connection, connection->address_space(), connection->GetRingbuffer());
  if (!context) {
    MAGMA_LOG(ERROR, "failed to create new context");
    return nullptr;
  }
  return std::make_unique<MsdVsiAbiContext>(MsdVsiAbiContext(context));
}

magma_status_t MsdVsiAbiConnection::MsdMapBuffer(msd::Buffer& buff, uint64_t gpu_va,
                                                 uint64_t offset, uint64_t length, uint64_t flags) {
  if (!magma::is_page_aligned(offset) || !magma::is_page_aligned(length))
    return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Offset or length not page aligned");

  uint64_t page_offset = offset / magma::page_size();
  uint64_t page_count = length / magma::page_size();

  auto connection = ptr().get();
  auto buffer = MsdVsiAbiBuffer::cast(&buff)->ptr();

  magma::Status status = connection->MapBufferGpu(buffer, gpu_va, page_offset, page_count);

  return status.get();
}

void MsdVsiAbiConnection::MsdReleaseBuffer(msd::Buffer& buff, bool shutting_down) {
  auto connection = ptr().get();
  auto buffer = MsdVsiAbiBuffer::cast(&buff)->ptr();

  connection->ReleaseBuffer(buffer->platform_buffer());
}

magma_status_t MsdVsiAbiConnection::MsdUnmapBuffer(msd::Buffer& buff, uint64_t gpu_va) {
  auto connection = ptr().get();
  auto buffer = MsdVsiAbiBuffer::cast(&buff)->ptr();

  if (!connection->ReleaseMapping(buffer->platform_buffer(), gpu_va)) {
    return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Unmap buffer failed");
  }
  return MAGMA_STATUS_OK;
}

void MsdVsiAbiConnection::MsdSetNotificationCallback(msd::NotificationHandler* handler) {
  ptr()->SetNotificationCallback(handler);
}

magma::Status MsdVsiConnection::MapBufferGpu(std::shared_ptr<MsdVsiBuffer> buffer, uint64_t gpu_va,
                                             uint64_t page_offset, uint64_t page_count) {
  uint64_t end_gpu_va = gpu_va + (page_count * magma::page_size());
  if (!AddressSpaceLayout::IsValidClientGpuRange(gpu_va, end_gpu_va)) {
    MAGMA_LOG(ERROR, "failed to map buffer to [0x%lx, 0x%lx), lies outside client region", gpu_va,
              end_gpu_va);
    return MAGMA_STATUS_INVALID_ARGS;
  }
  std::shared_ptr<GpuMapping> mapping;
  magma::Status status = AddressSpace::MapBufferGpu(address_space(), buffer, gpu_va, page_offset,
                                                    page_count, &mapping);
  if (!status.ok()) {
    MAGMA_LOG(ERROR, "MapBufferGpu failed");
    return status.get();
  }
  address_space_dirty_ = true;

  if (!address_space()->AddMapping(mapping)) {
    MAGMA_LOG(ERROR, "failed to add mapping");
    return MAGMA_STATUS_INVALID_ARGS;
  }
  return MAGMA_STATUS_OK;
}

void MsdVsiConnection::QueueReleasedMappings(std::vector<std::shared_ptr<GpuMapping>> mappings) {
  bool killed = false;
  for (const auto& mapping : mappings) {
    size_t use_count = mapping.use_count();
    if (use_count == 1) {
      // Bus mappings are held in the connection and passed through the command stream to
      // ensure the memory isn't released until the tlbs are invalidated, which happens
      // when the MappingReleaseBatch completes.
      std::vector<std::unique_ptr<magma::PlatformBusMapper::BusMapping>> bus_mappings;
      mapping->Release(&bus_mappings);
      for (uint32_t i = 0; i < bus_mappings.size(); i++) {
        mappings_to_release_.emplace_back(std::move(bus_mappings[i]));
      }
    } else {
      // It's an error to release a buffer while it has inflight mappings, as that
      // can fault the gpu.
      MAGMA_LOG(WARNING, "buffer %lu mapping use_count %zd", mapping->BufferId(), use_count);
      if (!killed) {
        SendContextKilled();
        killed = true;
      }
    }
  }
}

bool MsdVsiConnection::ReleaseMapping(magma::PlatformBuffer* buffer, uint64_t gpu_va) {
  std::shared_ptr<GpuMapping> mapping;
  if (!address_space()->ReleaseMapping(buffer, gpu_va, &mapping)) {
    MAGMA_LOG(ERROR, "failed to remove mapping");
    return false;
  }
  std::vector<std::shared_ptr<GpuMapping>> mappings = {std::move(mapping)};
  QueueReleasedMappings(std::move(mappings));
  address_space_dirty_ = true;

  return true;
}

void MsdVsiConnection::ReleaseBuffer(magma::PlatformBuffer* buffer) {
  std::vector<std::shared_ptr<GpuMapping>> mappings;
  address_space()->ReleaseBuffer(buffer, &mappings);
  QueueReleasedMappings(std::move(mappings));
}

bool MsdVsiConnection::SubmitPendingReleaseMappings(std::shared_ptr<MsdVsiContext> context) {
  if (!mappings_to_release_.empty()) {
    magma::Status status =
        SubmitBatch(std::make_unique<MappingReleaseBatch>(context, std::move(mappings_to_release_)),
                    true /* do_flush */);
    mappings_to_release_.clear();
    if (!status.ok()) {
      MAGMA_LOG(ERROR, "Failed to submit mapping release batch: %d", status.get());
      return false;
    }
  }
  return true;
}
