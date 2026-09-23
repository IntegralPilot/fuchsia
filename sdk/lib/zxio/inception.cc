// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/channel.h>
#include <lib/zx/stream.h>
#include <lib/zx/vmo.h>
#include <lib/zxio/cpp/inception.h>
#include <zircon/availability.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include "private.h"

namespace fio = fuchsia_io;
namespace funknown = fuchsia_unknown;

zx_status_t zxio_create_with_allocator(zx::handle handle, zxio_storage_alloc allocator,
                                       void** out_context) {
  zx_info_handle_basic_t handle_info = {};
  if (const zx_status_t status = handle.get_info(ZX_INFO_HANDLE_BASIC, &handle_info,
                                                 sizeof(handle_info), nullptr, nullptr);
      status != ZX_OK) {
    return status;
  }
  zxio_storage_t* storage = nullptr;
  zxio_object_type_t type = ZXIO_OBJECT_TYPE_NONE;
  switch (handle_info.type) {
    case ZX_OBJ_TYPE_CHANNEL: {
      fidl::ClientEnd<funknown::Queryable> queryable(zx::channel(std::move(handle)));
      zx::result type = zxio_get_object_type(queryable);
      if (type.is_error()) {
        return type.error_value();
      }
      zx_status_t status = allocator(type.value(), &storage, out_context);
      if (status != ZX_OK || storage == nullptr) {
        return ZX_ERR_NO_MEMORY;
      }
      return zxio_create_with_info(queryable.TakeChannel().release(), &handle_info, storage);
    }
    case ZX_OBJ_TYPE_LOG: {
      type = ZXIO_OBJECT_TYPE_DEBUGLOG;
      break;
    }
    case ZX_OBJ_TYPE_SOCKET: {
      type = ZXIO_OBJECT_TYPE_PIPE;
      break;
    }
    case ZX_OBJ_TYPE_VMO: {
      type = ZXIO_OBJECT_TYPE_VMO;
      break;
    }
  }
  if (const zx_status_t status = allocator(type, &storage, out_context);
      status != ZX_OK || storage == nullptr) {
    return ZX_ERR_NO_MEMORY;
  }
  return zxio_create_with_info(handle.release(), &handle_info, storage);
}

zx_status_t zxio_create_with_allocator(fidl::ClientEnd<fuchsia_io::Node> node,
                                       fuchsia_io::wire::Representation& representation,
                                       zxio_storage_alloc allocator, void** out_context) {
  zxio_storage_t* storage = nullptr;
  zxio_object_type_t type = ZXIO_OBJECT_TYPE_NONE;
  switch (representation.Which()) {
    case fio::wire::Representation::Tag::kDirectory:
      type = ZXIO_OBJECT_TYPE_DIR;
      break;
    case fio::wire::Representation::Tag::kFile:
      type = ZXIO_OBJECT_TYPE_FILE;
      break;
    case fio::wire::Representation::Tag::kNode:
      type = ZXIO_OBJECT_TYPE_NODE;
      break;
    case fio::wire::Representation::Tag::kSymlink:
      type = ZXIO_OBJECT_TYPE_SYMLINK;
      break;
    default:
      return ZX_ERR_NOT_SUPPORTED;
  }
  zx_status_t status = allocator(type, &storage, out_context);
  if (status != ZX_OK || storage == nullptr) {
    return ZX_ERR_NO_MEMORY;
  }
  return zxio_create_with_representation(std::move(node), representation, nullptr, storage);
}
