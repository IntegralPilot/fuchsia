// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fzl/vmar-manager.h>

#include <utility>

#include <fbl/alloc_checker.h>

namespace fzl {

fbl::RefPtr<VmarManager> VmarManager::Create(size_t size, fbl::RefPtr<VmarManager> parent,
                                             zx_vm_option_t options) {
  if (!size || (parent && !parent->vmar().is_valid())) {
    return nullptr;
  }

  fbl::AllocChecker ac;
  fbl::RefPtr<VmarManager> ret = fbl::AdoptRef(new (&ac) VmarManager());

  if (!ac.check()) {
    return nullptr;
  }

  zx_status_t res;
  zx_handle_t p = parent ? parent->vmar().get() : zx::vmar::root_self()->get();
  uintptr_t child_addr;

  res = zx_vmar_allocate(p, options, 0, size, ret->vmar_.reset_and_get_address(), &child_addr);
  if (res != ZX_OK) {
    return nullptr;
  }

  ret->parent_ = std::move(parent);
  ret->start_ = reinterpret_cast<void*>(child_addr);
  ret->size_ = size;
  ret->unowned_vmar_ = false;

  return ret;
}

zx::result<fbl::RefPtr<VmarManager>> VmarManager::Use(const zx::unowned_vmar& vmar) {
  fbl::AllocChecker ac;
  fbl::RefPtr<VmarManager> ret = fbl::AdoptRef(new (&ac) VmarManager());

  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  if (!vmar->is_valid()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx_info_vmar_t info;
  zx_status_t status = vmar->get_info(ZX_INFO_VMAR, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) [[unlikely]] {
    return zx::error(status);
  }

  zx_info_handle_basic_t basic_info;
  status = vmar->get_info(ZX_INFO_HANDLE_BASIC, &basic_info, sizeof(basic_info), nullptr, nullptr);
  if (status != ZX_OK) [[unlikely]] {
    return zx::error(status);
  }

  zx_info_handle_basic_t root_basic_info;
  status = zx::vmar::root_self()->get_info(ZX_INFO_HANDLE_BASIC, &root_basic_info,
                                           sizeof(root_basic_info), nullptr, nullptr);
  if (status != ZX_OK) [[unlikely]] {
    return zx::error(status);
  }

  if (basic_info.koid == root_basic_info.koid) {
    // The nullptr is the sentinel for the root vmar.
    return zx::ok(nullptr);
  }

  status = vmar->duplicate(ZX_RIGHT_SAME_RIGHTS, &ret->vmar_);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  ret->parent_ = nullptr;
  ret->start_ = reinterpret_cast<void*>(info.base);
  ret->size_ = info.len;
  ret->unowned_vmar_ = true;

  return zx::ok(std::move(ret));
}

}  // namespace fzl
