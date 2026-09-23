// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fdio/limits.h>
#include <lib/fdio/namespace.h>
#include <lib/zx/channel.h>
#include <lib/zxio/cpp/inception.h>
#include <poll.h>

#include <fbl/ref_ptr.h>

#include "sdk/lib/fdio/internal.h"
#include "sdk/lib/fdio/socket.h"
#include "sdk/lib/fdio/zxio.h"

namespace fio = fuchsia_io;

static_assert(FDIO_CHUNK_SIZE >= PATH_MAX, "FDIO_CHUNK_SIZE must be large enough to contain paths");

static_assert(static_cast<uint32_t>(fio::wire::VmoFlags::kRead) == ZX_VM_PERM_READ,
              "Vmar / Vmo flags should be aligned");
static_assert(static_cast<uint32_t>(fio::wire::VmoFlags::kWrite) == ZX_VM_PERM_WRITE,
              "Vmar / Vmo flags should be aligned");
static_assert(static_cast<uint32_t>(fio::wire::VmoFlags::kExecute) == ZX_VM_PERM_EXECUTE,
              "Vmar / Vmo flags should be aligned");

zx_status_t fdio_validate_path(const char* path, size_t* out_length) {
  if (path == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  const size_t length = strnlen(path, PATH_MAX);
  if (length >= PATH_MAX) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (out_length != nullptr) {
    *out_length = length;
  }
  return ZX_OK;
}

// Allocates an fdio_t instance containing storage for a zxio_t object.
zx_status_t fdio::zxio_allocator(zxio_object_type_t type, zxio_storage_t** out_storage,
                                 void** out_context) {
  fdio_ptr io;
  // The type of storage (fdio subclass) depends on the type of the object until
  // https://fxbug.dev/42119552 is resolved, so this has to switch on the type.
  switch (type) {
    case ZXIO_OBJECT_TYPE_DATAGRAM_SOCKET:
    case ZXIO_OBJECT_TYPE_PACKET_SOCKET:
    case ZXIO_OBJECT_TYPE_RAW_SOCKET:
    case ZXIO_OBJECT_TYPE_STREAM_SOCKET:
    case ZXIO_OBJECT_TYPE_SYNCHRONOUS_DATAGRAM_SOCKET:
      io = fdio_socket_allocate();
      break;
    case ZXIO_OBJECT_TYPE_DEBUGLOG:
      io = fbl::MakeRefCounted<fdio_internal::zxio>();
      break;
    case ZXIO_OBJECT_TYPE_DIR:
    case ZXIO_OBJECT_TYPE_FILE:
    case ZXIO_OBJECT_TYPE_SERVICE:
    case ZXIO_OBJECT_TYPE_TTY:
    case ZXIO_OBJECT_TYPE_VMO:
      io = fbl::MakeRefCounted<fdio_internal::remote>();
      break;
    case ZXIO_OBJECT_TYPE_PIPE:
      io = fbl::MakeRefCounted<fdio_internal::pipe>();
      break;
    default:
      // Unknown type - allocate a generic fdio object so that zxio_create can
      // initialize a zxio object holding the object for us.
      io = fbl::MakeRefCounted<fdio_internal::zxio>();
      break;
  }
  if (io == nullptr) {
    return ZX_ERR_NO_MEMORY;
  }
  *out_storage = &io->zxio_storage();
  *out_context = fbl::ExportToRawPtr(&io);
  return ZX_OK;
}

zx::result<fdio_ptr> fdio::create(void*& context, zx_status_t status) {
  // If the status is ZX_ERR_NO_MEMORY, then zxio_create_with_allocator has not allocated
  // anything and we can return immediately with no cleanup.
  if (status == ZX_ERR_NO_MEMORY) {
    ZX_ASSERT(context == nullptr);
    return zx::error(status);
  }

  // Otherwise, fdio_zxio_allocator has allocated an fdio instance that we now own.
  fdio_ptr io = fbl::ImportFromRawPtr(static_cast<fdio*>(context));

  return zx::make_result(status, std::move(io));
}

zx::result<fdio_ptr> fdio::create(zx::handle handle) {
  return fdio::create([&](zxio_storage_alloc allocator, void** out_context) {
    return zxio_create_with_allocator(std::move(handle), allocator, out_context);
  });
}

zx::result<fdio_ptr> fdio::create(fidl::ClientEnd<fio::Node> node,
                                  fio::wire::Representation representation) {
  return fdio::create([&](zxio_storage_alloc allocator, void** out_context) {
    return zxio_create_with_allocator(std::move(node), representation, allocator, out_context);
  });
}
