// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/connection/remote_file_connection.h"

#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/zx/result.h>
#include <stdint.h>
#include <stdlib.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/connection/file_connection.h"
#include "src/storage/lib/vfs/cpp/debug.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fio = fuchsia_io;

namespace fs::internal {

RemoteFileConnection::RemoteFileConnection(fs::FuchsiaVfs* vfs, fbl::RefPtr<fs::Vnode> vnode,
                                           fuchsia_io::Rights rights, bool append, zx_koid_t koid)
    : FileConnection(vfs, std::move(vnode), rights, koid), append_(append) {}

zx_status_t RemoteFileConnection::ReadInternal(void* data, size_t len, size_t* out_actual) {
  FS_PRETTY_TRACE_DEBUG("[FileRead] rights: ", rights());
  if (!(rights() & fuchsia_io::Rights::kReadBytes)) {
    return ZX_ERR_BAD_HANDLE;
  }
  if (len > fio::wire::kMaxTransferSize) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  zx_status_t status = vnode()->Read(data, len, offset_, out_actual);
  if (status == ZX_OK) {
    ZX_DEBUG_ASSERT(*out_actual <= len);
    offset_ += *out_actual;
  }
  return status;
}

void RemoteFileConnection::Read(ReadRequestView request, ReadCompleter::Sync& completer) {
  uint8_t data[fio::wire::kMaxBuf];
  size_t actual = 0;
  zx_status_t status = ReadInternal(data, request->count, &actual);
  if (status != ZX_OK) {
    completer.ReplyError(status);
  } else {
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(data, actual));
  }
}

zx_status_t RemoteFileConnection::ReadAtInternal(void* data, size_t len, size_t offset,
                                                 size_t* out_actual) {
  FS_PRETTY_TRACE_DEBUG("[FileReadAt] rights: ", rights());
  if (!(rights() & fuchsia_io::Rights::kReadBytes)) {
    return ZX_ERR_BAD_HANDLE;
  }
  if (len > fio::wire::kMaxTransferSize) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  zx_status_t status = vnode()->Read(data, len, offset, out_actual);
  if (status == ZX_OK) {
    ZX_DEBUG_ASSERT(*out_actual <= len);
  }
  return status;
}

void RemoteFileConnection::ReadAt(ReadAtRequestView request, ReadAtCompleter::Sync& completer) {
  uint8_t data[fio::wire::kMaxBuf];
  size_t actual = 0;
  zx_status_t status = ReadAtInternal(data, request->count, request->offset, &actual);
  if (status != ZX_OK) {
    completer.ReplyError(status);
  } else {
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(data, actual));
  }
}

zx_status_t RemoteFileConnection::WriteInternal(const void* data, size_t len, size_t* out_actual) {
  FS_PRETTY_TRACE_DEBUG("[FileWrite] rights: ", rights());
  if (!(rights() & fuchsia_io::Rights::kWriteBytes)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zx_status_t status;
  if (append_) {
    size_t end = 0u;
    status = vnode()->Append(data, len, &end, out_actual);
    if (status == ZX_OK) {
      offset_ = end;
    }
  } else {
    status = vnode()->Write(data, len, offset_, out_actual);
    if (status == ZX_OK) {
      offset_ += *out_actual;
    }
  }
  if (status == ZX_OK) {
    ZX_DEBUG_ASSERT(*out_actual <= len);
  }
  return status;
}

void RemoteFileConnection::Write(WriteRequestView request, WriteCompleter::Sync& completer) {
  size_t actual;
  zx_status_t status = WriteInternal(request->data.data(), request->data.size(), &actual);
  if (status != ZX_OK) {
    completer.ReplyError(status);
  } else {
    completer.ReplySuccess(actual);
  }
}

zx_status_t RemoteFileConnection::WriteAtInternal(const void* data, size_t len, size_t offset,
                                                  size_t* out_actual) {
  FS_PRETTY_TRACE_DEBUG("[FileWriteAt] rights: ", rights());
  if (!(rights() & fuchsia_io::Rights::kWriteBytes)) {
    return ZX_ERR_BAD_HANDLE;
  }
  zx_status_t status = vnode()->Write(data, len, offset, out_actual);
  if (status == ZX_OK) {
    ZX_DEBUG_ASSERT(*out_actual <= len);
  }
  return status;
}

void RemoteFileConnection::WriteAt(WriteAtRequestView request, WriteAtCompleter::Sync& completer) {
  size_t actual = 0;
  zx_status_t status =
      WriteAtInternal(request->data.data(), request->data.size(), request->offset, &actual);
  if (status != ZX_OK) {
    completer.ReplyError(status);
  } else {
    completer.ReplySuccess(actual);
  }
}

zx_status_t RemoteFileConnection::SeekInternal(fuchsia_io::wire::SeekOrigin origin,
                                               int64_t requested_offset) {
  FS_PRETTY_TRACE_DEBUG("[FileSeek] rights: ", rights());
  zx::result attr = vnode()->GetAttributes();
  if (!attr.is_ok()) {
    return ZX_ERR_STOP;
  }
  size_t n;
  switch (origin) {
    case fio::wire::SeekOrigin::kStart:
      if (requested_offset < 0) {
        return ZX_ERR_INVALID_ARGS;
      }
      n = requested_offset;
      break;
    case fio::wire::SeekOrigin::kCurrent:
      n = offset_ + requested_offset;
      if (requested_offset < 0) {
        // if negative seek
        if (n > offset_) {
          // wrapped around. attempt to seek before start
          return ZX_ERR_INVALID_ARGS;
        }
      } else {
        // positive seek
        if (n < offset_) {
          // wrapped around. overflow
          return ZX_ERR_INVALID_ARGS;
        }
      }
      break;
    case fio::wire::SeekOrigin::kEnd:
      n = *attr->content_size + requested_offset;
      if (requested_offset < 0) {
        // if negative seek
        if (n > *attr->content_size) {
          // wrapped around. attempt to seek before start
          return ZX_ERR_INVALID_ARGS;
        }
      } else {
        // positive seek
        if (n < *attr->content_size) {
          // wrapped around
          return ZX_ERR_INVALID_ARGS;
        }
      }
      break;
    default:
      return ZX_ERR_INVALID_ARGS;
  }
  offset_ = n;
  return ZX_OK;
}

void RemoteFileConnection::Seek(SeekRequestView request, SeekCompleter::Sync& completer) {
  zx_status_t status = SeekInternal(request->origin, request->offset);
  if (status == ZX_ERR_STOP) {
    completer.Close(ZX_ERR_INTERNAL);
  } else if (status != ZX_OK) {
    completer.ReplyError(status);
  } else {
    completer.ReplySuccess(offset_);
  }
}

}  // namespace fs::internal
