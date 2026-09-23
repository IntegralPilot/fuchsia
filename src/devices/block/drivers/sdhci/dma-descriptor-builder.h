// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_SDHCI_DMA_DESCRIPTOR_BUILDER_H_
#define SRC_DEVICES_BLOCK_DRIVERS_SDHCI_DMA_DESCRIPTOR_BUILDER_H_

#include <fidl/fuchsia.hardware.sdmmc/cpp/driver/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fzl/pinned-vmo.h>
#include <lib/sdmmc/hw.h>
#include <lib/zx/bti.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <stddef.h>
#include <stdint.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <array>
#include <span>

#include <fbl/algorithm.h>
#include <hwreg/bitfields.h>
#include <sdk/lib/driver/logging/cpp/logger.h>

#include "src/lib/vmo_store/vmo_store.h"

namespace sdhci {

// Maintains a list of physical memory regions to be used for DMA. Input buffers are pinned if
// needed, and unpinned upon destruction.
template <typename VmoInfoType>
class DmaDescriptorBuilder {
 public:
  using VmoStore = vmo_store::VmoStore<vmo_store::HashTableStorage<uint32_t, VmoInfoType>>;

  DmaDescriptorBuilder(const fuchsia_hardware_sdmmc::wire::SdmmcReq& request,
                       VmoStore& registered_vmos, uint64_t dma_boundary_alignment,
                       zx::unowned_bti bti)
      : request_(request),
        registered_vmos_(registered_vmos),
        dma_boundary_alignment_(dma_boundary_alignment),
        bti_(std::move(bti)) {}
  ~DmaDescriptorBuilder() {
    for (size_t i = 0; i < pmt_count_; i++) {
      pmts_[i].unpin();
    }
  }

  // Appends the physical memory regions for this buffer to the end of the list.
  zx_status_t ProcessBuffer(const fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion& buffer);

  // Builds DMA descriptors of the template type in the array provided.
  template <typename DescriptorType>
  zx_status_t BuildDmaDescriptors(cpp20::span<DescriptorType> out_descriptors);

  size_t block_count() const {
    ZX_DEBUG_ASSERT(request_.blocksize != 0);
    return total_size_ / request_.blocksize;
  }
  size_t descriptor_count() const { return descriptor_count_; }

 private:
  // 64k max per descriptor
  static constexpr size_t kMaxDescriptorLength = 0x1'0000;

  static constexpr uint32_t Hi32(zx_paddr_t val) {
    return static_cast<uint32_t>((val >> 32) & 0xffffffff);
  }
  static constexpr uint32_t Lo32(zx_paddr_t val) { return val & 0xffffffff; }

  // Pins the buffer if needed, and fills out_regions with the physical addresses corresponding to
  // the (owned or unowned) input buffer. Contiguous runs of pages are condensed into single
  // regions so that the minimum number of DMA descriptors are required.
  zx::result<size_t> GetPinnedRegions(uint32_t vmo_id,
                                      const fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion& buffer,
                                      cpp20::span<fzl::PinnedVmo::Region> out_regions);
  zx::result<size_t> GetPinnedRegions(const zx::vmo& vmo,
                                      const fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion& buffer,
                                      cpp20::span<fzl::PinnedVmo::Region> out_regions);

  // Appends the regions to the current list of regions being tracked by this object. Regions are
  // split if needed according to hardware restrictions on size or alignment.
  zx_status_t AppendRegions(cpp20::span<const fzl::PinnedVmo::Region> regions);

  const fuchsia_hardware_sdmmc::wire::SdmmcReq& request_;
  VmoStore& registered_vmos_;
  const uint64_t dma_boundary_alignment_;
  std::array<fzl::PinnedVmo::Region, fuchsia_hardware_sdmmc::wire::kSdmmcPagesCount> regions_ = {};
  size_t region_count_ = 0;
  size_t total_size_ = 0;
  size_t descriptor_count_ = 0;
  std::array<zx::pmt, fuchsia_hardware_sdmmc::wire::kSdmmcPagesCount> pmts_ = {};
  size_t pmt_count_ = 0;
  const zx::unowned_bti bti_;
};

class Adma2DescriptorAttributes : public hwreg::RegisterBase<Adma2DescriptorAttributes, uint16_t> {
 public:
  static constexpr uint16_t kTypeData = 0b10;

  static auto Get(uint16_t value = 0) {
    Adma2DescriptorAttributes ret;
    ret.set_reg_value(value);
    return ret;
  }

  DEF_RSVDZ_FIELD(15, 6);
  DEF_FIELD(5, 4, type);
  DEF_RSVDZ_BIT(3);
  DEF_BIT(2, intr);
  DEF_BIT(1, end);
  DEF_BIT(0, valid);
};

template <typename VmoInfoType>
template <typename DescriptorType>
zx_status_t DmaDescriptorBuilder<VmoInfoType>::BuildDmaDescriptors(
    cpp20::span<DescriptorType> out_descriptors) {
  if (total_size_ % request_.blocksize != 0) {
    fdf::error("Total buffer size ({}) is not a multiple of the request block size ({})",
               total_size_, request_.blocksize);
    return ZX_ERR_INVALID_ARGS;
  }

  const cpp20::span<const fzl::PinnedVmo::Region> regions{regions_.data(), region_count_};
  auto desc_it = out_descriptors.begin();
  for (const fzl::PinnedVmo::Region region : regions) {
    if (desc_it == out_descriptors.end()) {
      fdf::error("Not enough DMA descriptors to handle request");
      return ZX_ERR_OUT_OF_RANGE;
    }

    if constexpr (sizeof(desc_it->address) == sizeof(uint32_t)) {
      if (Hi32(region.phys_addr) != 0) {
        fdf::error("64-bit physical address supplied for 32-bit DMA");
        return ZX_ERR_NOT_SUPPORTED;
      }
      desc_it->address = static_cast<uint32_t>(region.phys_addr);
    } else {
      desc_it->address = region.phys_addr;
    }

    // Should be enforced by ProcessBuffer.
    ZX_DEBUG_ASSERT(region.size > 0);
    ZX_DEBUG_ASSERT(region.size <= kMaxDescriptorLength);

    desc_it->length = static_cast<uint16_t>(region.size == kMaxDescriptorLength ? 0 : region.size);
    desc_it->attr = Adma2DescriptorAttributes::Get()
                        .set_valid(1)
                        .set_type(Adma2DescriptorAttributes::kTypeData)
                        .reg_value();
    desc_it++;
  }

  if (desc_it == out_descriptors.begin()) {
    fdf::error("No buffers were provided for the transfer");
    return ZX_ERR_INVALID_ARGS;
  }

  // The above check verifies that we have at least on descriptor. Set the end bit on the last
  // descriptor as per the SDHCI ADMA2 spec.
  desc_it[-1].attr = Adma2DescriptorAttributes::Get(desc_it[-1].attr).set_end(1).reg_value();

  descriptor_count_ = desc_it - out_descriptors.begin();
  return ZX_OK;
}

template <typename VmoInfoType>
zx_status_t DmaDescriptorBuilder<VmoInfoType>::ProcessBuffer(
    const fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion& buffer) {
  total_size_ += buffer.size;

  fzl::PinnedVmo::Region region_buffer[fuchsia_hardware_sdmmc::wire::kSdmmcPagesCount];
  zx::result<size_t> region_count;
  if (buffer.buffer.is_vmo()) {
    region_count =
        GetPinnedRegions(buffer.buffer.vmo(), buffer, {region_buffer, std::size(region_buffer)});
  } else if (buffer.buffer.is_vmo_id()) {
    region_count =
        GetPinnedRegions(buffer.buffer.vmo_id(), buffer, {region_buffer, std::size(region_buffer)});
  } else {
    return ZX_ERR_INVALID_ARGS;
  }

  if (region_count.is_error()) {
    return region_count.error_value();
  }

  return AppendRegions({region_buffer, region_count.value()});
}

template <typename VmoInfoType>
zx::result<size_t> DmaDescriptorBuilder<VmoInfoType>::GetPinnedRegions(
    uint32_t vmo_id, const fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion& buffer,
    cpp20::span<fzl::PinnedVmo::Region> out_regions) {
  vmo_store::StoredVmo<VmoInfoType>* const stored_vmo =
      registered_vmos_.GetVmo(buffer.buffer.vmo_id());
  if (stored_vmo == nullptr) {
    fdf::error("No VMO {} for client {}", buffer.buffer.vmo_id(), request_.client_id);
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  // Make sure that this request would not cause the controller to violate the rights of the VMO,
  // as we may not have an IOMMU to otherwise prevent it.
  if (!(request_.cmd_flags & SDMMC_CMD_READ) &&
      !(stored_vmo->meta().rights & fuchsia_hardware_sdmmc::SdmmcVmoRight::kRead)) {
    // Write request, controller reads from this VMO and writes to the card.
    fdf::error("Request would cause controller to read from write-only VMO");
    return zx::error(ZX_ERR_ACCESS_DENIED);
  }
  if ((request_.cmd_flags & SDMMC_CMD_READ) &&
      !(stored_vmo->meta().rights & fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite)) {
    // Read request, controller reads from the card and writes to this VMO.
    fdf::error("Request would cause controller to write to read-only VMO");
    return zx::error(ZX_ERR_ACCESS_DENIED);
  }

  size_t region_count = 0;
  const zx_status_t status =
      stored_vmo->GetPinnedRegions(buffer.offset + stored_vmo->meta().offset, buffer.size,
                                   out_regions.data(), out_regions.size(), &region_count);
  if (status != ZX_OK) {
    fdf::error("Failed to get pinned regions: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok(region_count);
}

template <typename VmoInfoType>
zx::result<size_t> DmaDescriptorBuilder<VmoInfoType>::GetPinnedRegions(
    const zx::vmo& vmo, const fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion& buffer,
    cpp20::span<fzl::PinnedVmo::Region> out_regions) {
  const uint64_t kPageSize = zx_system_get_page_size();
  const uint64_t kPageMask = kPageSize - 1;

  if (pmt_count_ >= pmts_.size()) {
    fdf::error("Too many unowned VMOs specified, maximum is {}", pmts_.size());
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  const uint64_t page_offset = buffer.offset & kPageMask;
  const uint64_t page_count = fbl::round_up(buffer.size + page_offset, kPageSize) / kPageSize;

  const uint32_t options =
      (request_.cmd_flags & SDMMC_CMD_READ) ? ZX_BTI_PERM_WRITE : ZX_BTI_PERM_READ;

  zx_paddr_t phys[fuchsia_hardware_sdmmc::wire::kSdmmcPagesCount];

  if (page_count == 0) {
    fdf::error("Buffer has no pages");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (page_count > std::size(phys)) {
    fdf::error("Buffer has too many pages, maximum is {}", std::size(phys));
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  zx_status_t status = bti_->pin(options, vmo, buffer.offset - page_offset, page_count * kPageSize,
                                 phys, page_count, &pmts_[pmt_count_]);
  if (status != ZX_OK) {
    fdf::error("Failed to pin unowned VMO: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  pmt_count_++;

  // We don't own this VMO, and therefore cannot make any assumptions about the state of the
  // cache. The cache must be clean and invalidated for reads so that the final clean + invalidate
  // doesn't overwrite main memory with stale data from the cache, and must be clean for writes so
  // that main memory has the latest data.
  if (request_.cmd_flags & SDMMC_CMD_READ) {
    status = vmo.op_range(ZX_VMO_OP_CACHE_CLEAN_INVALIDATE, buffer.offset, buffer.size, nullptr, 0);
  } else {
    status = vmo.op_range(ZX_VMO_OP_CACHE_CLEAN, buffer.offset, buffer.size, nullptr, 0);
  }

  if (status != ZX_OK) {
    fdf::error("Cache op on unowned VMO failed: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  ZX_DEBUG_ASSERT(!out_regions.empty());  // This assumption simplifies the following logic.

  out_regions[0].phys_addr = phys[0] + page_offset;
  out_regions[0].size = kPageSize - page_offset;

  // Check for any pages that happen to be both contiguous and increasing in physical addresses.
  // Such pages, if there are any, can be combined into a single DMA descriptor to enable larger
  // transfers.

  size_t last_region = 0;
  for (size_t paddr_count = 1; paddr_count < page_count; paddr_count++) {
    if ((out_regions[last_region].phys_addr + out_regions[last_region].size) == phys[paddr_count]) {
      // The current region is contiguous with this physical address, increase it by the page size.
      out_regions[last_region].size += kPageSize;
    } else if (++last_region < out_regions.size()) {
      // The current region is not contiguous with this physical address, create a new region.
      out_regions[last_region].phys_addr = phys[paddr_count];
      out_regions[last_region].size = kPageSize;
    } else {
      // Ran out of regions.
      fdf::error("Buffer has too many regions, maximum is {}", out_regions.size());
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
  }

  // Adjust the last region size based on the offset into the first page and the total size of the
  // buffer.
  out_regions[last_region].size -= (page_count * kPageSize) - buffer.size - page_offset;

  return zx::ok(last_region + 1);
}

template <typename VmoInfoType>
zx_status_t DmaDescriptorBuilder<VmoInfoType>::AppendRegions(
    const cpp20::span<const fzl::PinnedVmo::Region> regions) {
  if (regions.empty()) {
    return ZX_ERR_INVALID_ARGS;
  }

  fzl::PinnedVmo::Region current_region{.phys_addr = 0, .size = 0};
  auto vmo_regions_it = regions.begin();
  for (; region_count_ < regions_.size(); region_count_++) {
    // Current region is invalid, fetch a new one from the input list.
    if (current_region.size == 0) {
      // No more regions left to process.
      if (vmo_regions_it == regions.end()) {
        return ZX_OK;
      }
      if (vmo_regions_it->size == 0) {
        return ZX_ERR_INVALID_ARGS;
      }

      current_region = *vmo_regions_it++;
    }

    // Default to an invalid region so that the next iteration fetches another one from the input
    // list. If this region is divided due to a boundary or size restriction, the next region will
    // remain valid so that processing of the original region will continue.
    fzl::PinnedVmo::Region next_region{.phys_addr = 0, .size = 0};

    if (dma_boundary_alignment_) {
      const zx_paddr_t aligned_start =
          fbl::round_down(current_region.phys_addr, dma_boundary_alignment_);
      const zx_paddr_t aligned_end = fbl::round_down(
          current_region.phys_addr + current_region.size - 1, dma_boundary_alignment_);

      if (aligned_start != aligned_end) {
        // Crossing a boundary, split the DMA buffer in two.
        const size_t first_size =
            aligned_start + dma_boundary_alignment_ - current_region.phys_addr;
        next_region.size = current_region.size - first_size;
        next_region.phys_addr = current_region.phys_addr + first_size;
        current_region.size = first_size;
      }
    }

    // The region size is greater than the maximum, split it into two or more smaller regions.
    if (current_region.size > kMaxDescriptorLength) {
      const size_t size_diff = current_region.size - kMaxDescriptorLength;
      if (next_region.size) {
        next_region.phys_addr -= size_diff;
      } else {
        next_region.phys_addr = current_region.phys_addr + kMaxDescriptorLength;
      }
      next_region.size += size_diff;
      current_region.size = kMaxDescriptorLength;
    }

    regions_[region_count_] = current_region;
    current_region = next_region;
  }

  // If processing did not reach the end of the VMO regions or the current region is still valid we
  // must have hit the end of the output region buffer.
  return vmo_regions_it == regions.end() && current_region.size == 0 ? ZX_OK : ZX_ERR_OUT_OF_RANGE;
}

}  // namespace sdhci

#endif  // SRC_DEVICES_BLOCK_DRIVERS_SDHCI_DMA_DESCRIPTOR_BUILDER_H_
