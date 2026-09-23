// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/microfuchsia/tee/lib/dev_urandom_compat/dev_urandom_compat.h"

#include <lib/fdio/namespace.h>
#include <lib/zxio/null.h>
#include <lib/zxio/ops.h>
#include <lib/zxio/zxio.h>
#include <zircon/syscalls.h>

namespace {

zx_status_t CompatReadv(zxio_t* io, const zx_iovec_t* vector, size_t vector_count,
                        zxio_flags_t flags, size_t* out_actual) {
  size_t bytes_read = 0;
  for (size_t i = 0; i < vector_count; ++i) {
    zx_cprng_draw(vector[i].buffer, vector[i].capacity);
    bytes_read += vector[i].capacity;
  }
  *out_actual = bytes_read;
  return ZX_OK;
}

constexpr zxio_ops_t DevUrandomCompatOps = ([]() {
  zxio_ops_t ops = zxio_default_ops;
  ops.readv = CompatReadv;
  return ops;
})();

}  // anonymous namespace

zx_status_t register_dev_urandom_compat() {
  fdio_ns_t* ns = nullptr;
  zx_status_t st = fdio_ns_get_installed(&ns);
  if (st != ZX_OK) {
    return st;
  }
  fdio_open_local_func_t cb = [](zxio_storage_t* storage, void* context, zxio_ops_t const** ops) {
    // We do not keep any local state in this file.
    *ops = &DevUrandomCompatOps;
    return ZX_OK;
  };
  void* context = nullptr;
  return fdio_ns_bind_local(ns, "/dev/urandom", cb, context);
}
