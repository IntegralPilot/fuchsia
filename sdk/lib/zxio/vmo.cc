// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/zxio/null.h>
#include <lib/zxio/ops.h>
#include <sys/stat.h>
#include <zircon/syscalls.h>

#include "private.h"

class Vmo : public HasIo {
 public:
  Vmo(zx::vmo vmo, zx::stream stream)
      : HasIo(kOps), vmo_(std::move(vmo)), stream_(std::move(stream)) {}

 private:
  static const zxio_ops_t kOps;

  // The underlying VMO that stores the data.
  zx::vmo vmo_;

  // The stream through which we will read and write the VMO.
  zx::stream stream_;

 protected:
  void Destroy() { this->~Vmo(); }

  zx_status_t Release(zx_handle_t* out_handle) {
    *out_handle = vmo_.release();
    ;
    return ZX_OK;
  }

  zx_status_t Clone(zx_handle_t* out_handle) {
    zx::vmo vmo;
    if (zx_status_t status = vmo_.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo); status != ZX_OK) {
      return status;
    }
    *out_handle = vmo.release();
    return ZX_OK;
  }

  zx_status_t AttrGet(zxio_node_attributes_t* inout_attr) {
    uint64_t content_size = 0;
    // Return VMO size as stream size the VMO doesn't have associated stream object.
    if (zx_status_t status = stream_ ? vmo_.get_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size,
                                                         sizeof(content_size))
                                     : vmo_.get_size(&content_size);
        status != ZX_OK) {
      return status;
    }
    if (inout_attr->has.protocols) {
      ZXIO_NODE_ATTR_SET(*inout_attr, protocols, ZXIO_NODE_PROTOCOL_FILE);
    }
    if (inout_attr->has.abilities) {
      ZXIO_NODE_ATTR_SET(*inout_attr, abilities,
                         ZXIO_OPERATION_READ_BYTES | ZXIO_OPERATION_GET_ATTRIBUTES);
    }
    if (inout_attr->has.content_size) {
      ZXIO_NODE_ATTR_SET(*inout_attr, content_size, content_size);
    }
    if (inout_attr->has.object_type) {
      ZXIO_NODE_ATTR_SET(*inout_attr, object_type, ZXIO_OBJECT_TYPE_VMO);
    }
    return ZX_OK;
  }

  zx_status_t Readv(const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                    size_t* out_actual) {
    if (flags || !stream_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    return stream_.readv(0, vector, vector_count, out_actual);
  }

  zx_status_t ReadvAt(zx_off_t offset, const zx_iovec_t* vector, size_t vector_count,
                      zxio_flags_t flags, size_t* out_actual) {
    if (flags || !stream_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    return stream_.readv_at(0, offset, vector, vector_count, out_actual);
  }

  zx_status_t Writev(const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                     size_t* out_actual) {
    if (flags || !stream_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    return stream_.writev(0, vector, vector_count, out_actual);
  }

  zx_status_t WritevAt(zx_off_t offset, const zx_iovec_t* vector, size_t vector_count,
                       zxio_flags_t flags, size_t* out_actual) {
    if (flags || !stream_) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    return stream_.writev_at(0, offset, vector, vector_count, out_actual);
  }

  static_assert(ZXIO_SEEK_ORIGIN_START == ZX_STREAM_SEEK_ORIGIN_START, "ZXIO should match ZX");
  static_assert(ZXIO_SEEK_ORIGIN_CURRENT == ZX_STREAM_SEEK_ORIGIN_CURRENT, "ZXIO should match ZX");
  static_assert(ZXIO_SEEK_ORIGIN_END == ZX_STREAM_SEEK_ORIGIN_END, "ZXIO should match ZX");

  zx_status_t Seek(zxio_seek_origin_t start, int64_t offset, size_t* out_offset) {
    if (!stream_) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    return stream_.seek(static_cast<zx_stream_seek_origin_t>(start), offset, out_offset);
  }

  zx_status_t Truncate(uint64_t length) { return vmo_.set_stream_size(length); }

  zx_status_t FlagsGet(uint64_t* out_flags) {
    zx_info_handle_basic_t info;
    zx_status_t get_status =
        vmo_.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
    if (get_status != ZX_OK) {
      // Returns ZX_ERR_NOT_SUPPORTED, because a posix FD doesn't seem to have any way to lack
      // sufficient rights to F_GETFL (AFAICT), so the most accurate description of this situation
      // (AFAICT) is that FlagsGet() isn't supported on this particular zxio_t after all. We could
      // return ZX_ERR_NOT_SUPPORTED here, but if zxio_default_flags_get() changes, we really do
      // want to delegate to say "we don't have that op after all" essentially.
      return zxio_default_flags_get(io(), out_flags);
    }
    ZX_ASSERT(info.type == ZX_OBJ_TYPE_VMO);
    fuchsia_io::wire::Flags flags{};
    if (info.rights & ZX_RIGHT_READ) {
      flags |= fuchsia_io::wire::Flags::kPermReadBytes;
    }
    if (info.rights & ZX_RIGHT_WRITE) {
      flags |= fuchsia_io::wire::Flags::kPermWriteBytes;
    }
    if (info.rights & ZX_RIGHT_EXECUTE) {
      flags |= fuchsia_io::wire::Flags::kPermExecute;
    }
    *out_flags = uint64_t{flags};
    return ZX_OK;
  }

  zx_status_t VmoGet(zxio_vmo_flags_t flags, zx_handle_t* out_vmo) {
    if (out_vmo == nullptr) {
      return ZX_ERR_INVALID_ARGS;
    }

    zx::vmo& vmo = vmo_;

    uint64_t size;
    if (zx_status_t status = vmo.get_prop_content_size(&size); status != ZX_OK) {
      return status;
    }

    // Ensure that we return a VMO handle with only the rights requested by the client.

    zx_rights_t rights = ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY;
    rights |= flags & ZXIO_VMO_READ ? ZX_RIGHT_READ : 0;
    rights |= flags & ZXIO_VMO_WRITE ? ZX_RIGHT_WRITE : 0;
    rights |= flags & ZXIO_VMO_EXECUTE ? ZX_RIGHT_EXECUTE : 0;

    if (flags & ZXIO_VMO_PRIVATE_CLONE) {
      // Allow ZX_RIGHT_SET_PROPERTY only if creating a private child VMO so that the user can set
      // ZX_PROP_NAME (or similar).
      rights |= ZX_RIGHT_SET_PROPERTY;

      uint32_t options = ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE;
      if (flags & ZXIO_VMO_EXECUTE) {
        // Creating a ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE child removes ZX_RIGHT_EXECUTE even if
        // the parent VMO has it, and we can't arbitrarily add ZX_RIGHT_EXECUTE here on the client
        // side. Adding ZX_VMO_CHILD_NO_WRITE still creates a snapshot and a new VMO object, which
        // e.g. can have a unique ZX_PROP_NAME value, but the returned handle lacks ZX_RIGHT_WRITE
        // and maintains ZX_RIGHT_EXECUTE.
        if (flags & ZXIO_VMO_WRITE) {
          return ZX_ERR_NOT_SUPPORTED;
        }
        options |= ZX_VMO_CHILD_NO_WRITE;
      }

      zx::vmo child_vmo;
      zx_status_t status = vmo.create_child(options, 0u, size, &child_vmo);
      if (status != ZX_OK) {
        return status;
      }

      // ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE adds ZX_RIGHT_WRITE automatically, but we shouldn't
      // return a handle with that right unless requested using ZXIO_VMO_WRITE.
      //
      // TODO(https://fxbug.dev/42112453): Supporting ZXIO_VMO_PRIVATE_CLONE & ZXIO_VMO_WRITE for
      // Vmofiles is a bit weird and inconsistent. See bug for more info.
      zx::vmo result;
      status = child_vmo.replace(rights, &result);
      if (status != ZX_OK) {
        return status;
      }
      *out_vmo = result.release();
      return ZX_OK;
    }

    // For !ZXIO_VMO_PRIVATE_CLONE we just duplicate another handle to the Vmofile's VMO with
    // appropriately scoped rights.
    zx::vmo result;
    zx_status_t status = vmo.duplicate(rights, &result);
    if (status != ZX_OK) {
      return status;
    }
    *out_vmo = result.release();
    return ZX_OK;
  }
};

constexpr zxio_ops_t Vmo::kOps = []() {
  using Adaptor = Adaptor<Vmo>;
  zxio_ops_t ops = zxio_default_ops;
  ops.destroy = Adaptor::From<&Vmo::Destroy>;
  ops.release = Adaptor::From<&Vmo::Release>;
  ops.clone = Adaptor::From<&Vmo::Clone>;
  ops.attr_get = Adaptor::From<&Vmo::AttrGet>;
  ops.readv = Adaptor::From<&Vmo::Readv>;
  ops.readv_at = Adaptor::From<&Vmo::ReadvAt>;
  ops.writev = Adaptor::From<&Vmo::Writev>;
  ops.writev_at = Adaptor::From<&Vmo::WritevAt>;
  ops.seek = Adaptor::From<&Vmo::Seek>;
  ops.truncate = Adaptor::From<&Vmo::Truncate>;
  ops.flags_get = Adaptor::From<&Vmo::FlagsGet>;
  ops.vmo_get = Adaptor::From<&Vmo::VmoGet>;
  return ops;
}();

zx_status_t zxio_vmo_init(zxio_storage_t* storage, zx::vmo vmo, zx::stream stream) {
  new (storage) Vmo(std::move(vmo), std::move(stream));
  return ZX_OK;
}
