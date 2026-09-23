// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "helpers.h"

#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/vmar.h>

namespace {

zx::result<zx::resource> GetVmexResource() {
  zx::result client = component::Connect<fuchsia_kernel::VmexResource>();
  if (client.is_error()) {
    return client.take_error();
  }
  fidl::WireResult result = fidl::WireCall(client.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  return zx::ok(std::move(result.value().resource));
}

}  // namespace

zx::result<zx_vaddr_t> SetupCodeSegment(zx_handle_t restricted_vmar_handle,
                                        cpp20::span<const std::byte> code_blob) {
  zx::unowned_vmar rvmar(restricted_vmar_handle);
  const uint32_t page_size = zx_system_get_page_size();
  size_t page_aligned_size = (code_blob.size_bytes() + page_size - 1) & ~(page_size - 1);

  zx::vmo cs;
  zx::vmo cs_exe;
  zx_vaddr_t cs_addr;
  zx_status_t status = zx::vmo::create(page_aligned_size, 0, &cs);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  status = cs.write(code_blob.data(), 0, code_blob.size_bytes());
  if (status != ZX_OK) {
    return zx::error(status);
  }
  auto vmex = GetVmexResource();
  status = vmex.status_value();
  if (status != ZX_OK) {
    return zx::error(status);
  }
  status = cs.replace_as_executable(*vmex, &cs_exe);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  status =
      rvmar->map(ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE, 0, cs_exe, 0, page_aligned_size, &cs_addr);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(cs_addr);
}

zx::result<zx_vaddr_t> SetupStack(zx_handle_t restricted_vmar_handle, size_t size,
                                  zx::vmo* stack_vmo) {
  zx::unowned_vmar rvmar(restricted_vmar_handle);

  zx_vaddr_t stack_addr;
  zx_status_t status = zx::vmo::create(size, 0, stack_vmo);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  status = rvmar->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, *stack_vmo, 0, size, &stack_addr);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(stack_addr + size);
}
