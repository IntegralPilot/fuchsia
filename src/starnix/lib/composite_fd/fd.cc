// Copyright 2026 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/lib/composite_fd/fd.h"

#include <lib/fdio/fdio.h>
#include <lib/fdio/unsafe.h>
#include <lib/fit/defer.h>
#include <lib/zx/handle.h>
#include <lib/zxio/null.h>
#include <lib/zxio/zxio.h>

#include <span>
#include <vector>

namespace {

class CompositeFd {
 public:
  CompositeFd(zx_handle_t* handles, size_t size);
  std::vector<zx::handle>& handles() { return handles_; }
  inline bool valid();

 private:
  zxio_t io_;
  std::vector<zx::handle> handles_;
};

constexpr zxio_ops_t kCompositeFdOps = [] {
  zxio_ops_t ops = zxio_default_ops;
  ops.destroy = [](zxio_t* io) { reinterpret_cast<CompositeFd*>(io)->~CompositeFd(); };
  return ops;
}();

CompositeFd::CompositeFd(zx_handle_t* handles, size_t size) {
  zxio_init(&io_, &kCompositeFdOps);
  handles_.reserve(size);
  for (zx_handle_t handle : std::span(handles, size)) {
    handles_.emplace_back(handle);
  }
}

bool CompositeFd::valid() { return zxio_get_ops(&io_) == &kCompositeFdOps; }

}  // namespace

zx_status_t composite_fd_create(zx_handle_t* handles, size_t size, fdio_t** out_fdio) {
  zxio_storage_t* storage;
  fdio_t* fdio = fdio_zxio_create(&storage);
  if (fdio == nullptr) {
    return ZX_ERR_NO_MEMORY;
  }
  new (storage) CompositeFd(handles, size);
  *out_fdio = fdio;
  return ZX_OK;
}

void composite_fd_release(fdio_t* fdio, size_t size, zx_handle_t* out_handles) {
  zxio_t* zxio = fdio_get_zxio(fdio);
  std::vector<zx::handle>& handles = reinterpret_cast<CompositeFd*>(zxio)->handles();

  size = std::min(size, handles.size());
  for (size_t i = 0; i < size; i++) {
    out_handles[i] = handles[i].release();
  }

  handles.clear();
}

size_t composite_fd_size(fdio_t* fdio) {
  zxio_t* zxio = fdio_get_zxio(fdio);
  return reinterpret_cast<CompositeFd*>(zxio)->handles().size();
}

bool composite_fd_valid(fdio_t* fdio) {
  zxio_t* zxio = fdio_get_zxio(fdio);
  return reinterpret_cast<CompositeFd*>(zxio)->valid();
}
