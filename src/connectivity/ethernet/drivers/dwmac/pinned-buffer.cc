// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pinned-buffer.h"

#include <climits>
#include <memory>
#include <utility>

fbl::RefPtr<PinnedBuffer> PinnedBuffer::Create(size_t size, const zx::bti& bti,
                                               uint32_t cache_policy) {
  fbl::RefPtr<fzl::VmarManager> vmar_mgr;

  const size_t page_size = zx_system_get_page_size();
  if (!bti.is_valid() || (size & (page_size - 1))) {
    return nullptr;
  }

  // create vmar large enough for rx,tx buffers, and rx,tx dma descriptors
  vmar_mgr = fzl::VmarManager::Create(size, nullptr);
  if (!vmar_mgr) {
    zxlogf(ERROR, "pinned-buffer: Creation of vmar manager failed");
    return nullptr;
  }

  fbl::AllocChecker ac;

  auto pbuf = fbl::AdoptRef(new (&ac) PinnedBuffer());
  if (!ac.check()) {
    return nullptr;
  }

  zx_status_t status = pbuf->vmo_mapper_.CreateAndMap(
      size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, std::move(vmar_mgr), &pbuf->vmo_,
      ZX_RIGHT_READ | ZX_RIGHT_MAP | ZX_RIGHT_WRITE, cache_policy);
  if (status != ZX_OK) {
    zxlogf(ERROR, "pinned-buffer: vmo creation failed %d", status);
    return nullptr;
  }

  uint32_t page_count = static_cast<uint32_t>(size / page_size);

  std::unique_ptr<zx_paddr_t[]> addrs(new (&ac) zx_paddr_t[page_count]);
  if (!ac.check()) {
    return nullptr;
  }

  // Now actually pin the region.

  status = bti.pin(ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE, pbuf->vmo_, 0, size, addrs.get(),
                   page_count, &pbuf->pmt_);
  if (status != ZX_OK) {
    pbuf->UnPin();
    return nullptr;
  }

  pbuf->paddrs_.reset(addrs.release());
  return pbuf;
}

zx_status_t PinnedBuffer::UnPin() {
  if ((paddrs_ == nullptr) || !pmt_.is_valid()) {
    return ZX_ERR_BAD_STATE;
  }
  pmt_.unpin();
  paddrs_.reset();
  return ZX_OK;
}

// We need a status here since it is within the realm of possibility that
// the physical address returned could legitimately be 0x00000000, so
// returning a nullptr for a failure won't cut it.
zx_status_t PinnedBuffer::LookupPhys(zx_off_t offset, zx_paddr_t* out) {
  if (paddrs_ == nullptr) {
    return ZX_ERR_BAD_STATE;
  }
  if (offset >= GetSize()) {
    *out = 0;
    return ZX_ERR_INVALID_ARGS;
  }

  const size_t page_size = zx_system_get_page_size();
  *out = paddrs_[offset / page_size] + (offset % page_size);

  return ZX_OK;
}
