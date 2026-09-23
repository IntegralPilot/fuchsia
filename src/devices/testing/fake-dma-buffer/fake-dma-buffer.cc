// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/dma-buffer/buffer.h>
#include <lib/zx/vmar.h>

#include <string_view>

#include <fake-dma-buffer/fake-dma-buffer.h>

#include "lib/zx/vmo.h"

class ContiguousBufferImpl : public dma_buffer::ContiguousBuffer {
 public:
  ContiguousBufferImpl(size_t size, zx::vmo vmo, void* virt, zx_paddr_t phys, zx::pmt pmt,
                       dma_buffer::CacheOptions cache_options)
      : buffer_(virt, size, cache_options),
        phys_(phys),
        vmo_(std::move(vmo)),
        pmt_(std::move(pmt)) {}

  const dma_buffer::Buffer& buffer() const override { return buffer_; }
  zx_paddr_t phys() const override { return phys_; }
  zx::unowned_vmo vmo() const override { return vmo_.borrow(); }

  ~ContiguousBufferImpl() override {
    if (vmo_.is_valid()) {
      delete reinterpret_cast<ddk_fake::FakePage*>(phys_);
    }
  }

 private:
  dma_buffer::Buffer buffer_;
  zx_paddr_t phys_;
  zx::vmo vmo_;
  zx::pmt pmt_;
};

class PagedBufferImpl : public dma_buffer::PagedBuffer {
 public:
  PagedBufferImpl(size_t size, zx::vmo vmo, void* virt, std::vector<zx_paddr_t> phys, zx::pmt pmt,
                  dma_buffer::CacheOptions cache_options)
      : buffer_(virt, size, cache_options),
        phys_(std::move(phys)),
        vmo_(std::move(vmo)),
        pmt_(std::move(pmt)) {}

  const dma_buffer::Buffer& buffer() const override { return buffer_; }
  const zx_paddr_t* phys() const override { return phys_.data(); }

  ~PagedBufferImpl() override {
    if (vmo_.is_valid()) {
      static_assert(sizeof(phys_[0]) == sizeof(ddk_fake::FakePage*));
      for (auto paddr : phys_) {
        delete reinterpret_cast<ddk_fake::FakePage*>(paddr);
      }
    }
  }

 private:
  dma_buffer::Buffer buffer_;
  std::vector<zx_paddr_t> phys_;
  zx::vmo vmo_;
  zx::pmt pmt_;
};

class BufferFactoryImpl : public dma_buffer::BufferFactory {
  zx_status_t CreateContiguous(const zx::bti& bti, size_t size, uint32_t alignment_log2,
                               dma_buffer::CacheOptions cache_options,
                               std::unique_ptr<dma_buffer::ContiguousBuffer>* out) const override {
    if (size > zx_system_get_page_size()) {
      // TODO(https://fxbug.dev/42121490): We don't currently support contiguous buffers > 1 page.
      return ZX_ERR_NOT_SUPPORTED;
    }
    zx::vmo real_vmo;
    zx_status_t status = zx::vmo::create(size, 0, &real_vmo);
    if (status != ZX_OK) {
      return status;
    }

    constexpr std::string_view kVmoName = "fake-dma-buffer-contiguous";
    real_vmo.set_property(ZX_PROP_NAME, kVmoName.data(), kVmoName.length());

    void* virt;
    status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, real_vmo, 0, size,
                                        reinterpret_cast<zx_vaddr_t*>(&virt));
    if (status != ZX_OK) {
      return status;
    }
    auto fake = new ddk_fake::FakePage();
    fake->alignment_log2 = alignment_log2;
    fake->enable_cache = (cache_options == dma_buffer::CacheOptions::kEnabled);
    fake->size = size;
    status = real_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &fake->backing_storage);
    if (status != ZX_OK) {
      return status;
    }
    fake->virt = virt;
    fake->contiguous = true;
    fake->bti = bti.get();

    auto buffer = std::make_unique<ContiguousBufferImpl>(size, std::move(real_vmo), virt,
                                                         reinterpret_cast<zx_paddr_t>(fake),
                                                         zx::pmt(ZX_HANDLE_INVALID), cache_options);
    *out = std::move(buffer);
    return ZX_OK;
  }
  zx_status_t CreatePaged(const zx::bti& bti, size_t size, dma_buffer::CacheOptions cache_options,
                          std::unique_ptr<dma_buffer::PagedBuffer>* out) const override {
    zx::vmo real_vmo;
    zx_status_t status = zx::vmo::create(size, 0, &real_vmo);
    if (status != ZX_OK) {
      return status;
    }

    constexpr std::string_view kVmoName = "fake-dma-buffer-paged";
    real_vmo.set_property(ZX_PROP_NAME, kVmoName.data(), kVmoName.length());

    uint8_t* virt;
    status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, real_vmo, 0, size,
                                        reinterpret_cast<zx_vaddr_t*>(&virt));
    if (status != ZX_OK) {
      return status;
    }
    size_t pages = fbl::round_up(size, zx_system_get_page_size()) / zx_system_get_page_size();
    std::vector<zx_paddr_t> physvec;
    physvec.resize(pages);
    for (size_t i = 0; i < pages; i++) {
      auto phys = new ddk_fake::FakePage();
      phys->enable_cache = (cache_options == dma_buffer::CacheOptions::kEnabled);
      phys->size = size;
      status = real_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &phys->backing_storage);
      if (status != ZX_OK) {
        return status;
      }
      phys->virt = virt + (zx_system_get_page_size() * i);
      phys->bti = bti.get();
      phys->contiguous = false;
      physvec[i] = reinterpret_cast<zx_paddr_t>(phys);
    }
    auto buffer =
        std::make_unique<PagedBufferImpl>(size, std::move(real_vmo), virt, std::move(physvec),
                                          zx::pmt(ZX_HANDLE_INVALID), cache_options);
    *out = std::move(buffer);
    return ZX_OK;
  }
};

namespace ddk_fake {

void* PhysToVirt(zx_paddr_t phys) { return PhysToVirt<void*>(phys); }

const FakePage& GetPage(zx_paddr_t phys) {
  size_t start = fbl::round_down(phys, zx_system_get_page_size());
  return *reinterpret_cast<FakePage*>(start);
}

std::unique_ptr<dma_buffer::BufferFactory> CreateBufferFactory() {
  return std::make_unique<BufferFactoryImpl>();
}

}  // namespace ddk_fake
