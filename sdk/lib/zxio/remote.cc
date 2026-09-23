// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.pty/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/zx/channel.h>
#include <lib/zxio/null.h>
#include <lib/zxio/ops.h>
#include <lib/zxio/types.h>
#include <sys/stat.h>
#include <zircon/availability.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/fidl.h>
#include <zircon/syscalls.h>

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>

#include "sdk/lib/zxio/private.h"
#include "sdk/lib/zxio/vector.h"

namespace fio = fuchsia_io;

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
// Can't import fio in the main header. Just ensure that they're equal.
static_assert(ZXIO_SELINUX_CONTEXT_MAX_ATTR_LEN == fio::wire::kMaxSelinuxContextAttributeLen);
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

zx_status_t RemoteReadv(const fidl::UnownedClientEnd<fio::Readable>& client_end,
                        const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                        size_t* out_actual) {
  return zxio_chunked_do_vector<fio::wire::kMaxBuf>(
      vector, vector_count, flags, out_actual,
      [&client_end](uint8_t* buffer, size_t capacity, size_t* out_actual) {
        // Explicitly allocating message buffers to avoid heap allocation.
        fidl::SyncClientBuffer<fio::Readable::Read> fidl_buffer;
        const auto result = fidl::WireCall(client_end).buffer(fidl_buffer.view())->Read(capacity);
        if (!result.ok()) {
          return result.status();
        }
        const auto& response = result.value();
        if (response.is_error()) {
          return response.error_value();
        }
        const fidl::VectorView data = response.value()->data;
        const size_t actual = data.size();
        if (actual > capacity) {
          return ZX_ERR_IO;
        }
        memcpy(buffer, data.begin(), actual);
        *out_actual = actual;
        return ZX_OK;
      });
}

zx_status_t RemoteWritev(const fidl::UnownedClientEnd<fio::Writable>& client_end,
                         const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                         size_t* out_actual) {
  return zxio_chunked_do_vector<fio::wire::kMaxBuf>(
      vector, vector_count, flags, out_actual,
      [&client_end](uint8_t* buffer, size_t capacity, size_t* out_actual) {
        // Explicitly allocating message buffers to avoid heap allocation.
        fidl::SyncClientBuffer<fio::File::Write> fidl_buffer;
        const fidl::WireUnownedResult result =
            fidl::WireCall(client_end)
                .buffer(fidl_buffer.view())
                ->Write(fidl::VectorView<uint8_t>::FromExternal(buffer, capacity));
        if (!result.ok()) {
          return result.status();
        }
        const auto& response = result.value();
        if (response.is_error()) {
          return response.error_value();
        }
        const size_t actual = response.value()->actual_count;
        if (actual > capacity) {
          return ZX_ERR_IO;
        }
        *out_actual = actual;
        return ZX_OK;
      });
}

namespace {

zx_status_t FileReadvAt(const fidl::WireSyncClient<fio::File>& client, zx_off_t offset,
                        const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                        size_t* out_actual) {
  return zxio_chunked_do_vector<fio::wire::kMaxBuf>(
      vector, vector_count, flags, out_actual,
      [&client, &offset](uint8_t* buffer, size_t capacity, size_t* out_actual) {
        // Explicitly allocating message buffers to avoid heap allocation.
        fidl::SyncClientBuffer<fio::File::ReadAt> fidl_buffer;
        const fidl::WireUnownedResult result =
            client.buffer(fidl_buffer.view())->ReadAt(capacity, offset);
        if (!result.ok()) {
          return result.status();
        }
        const auto& response = result.value();
        if (response.is_error()) {
          return response.error_value();
        }
        const fidl::VectorView data = response.value()->data;
        const size_t actual = data.size();
        if (actual > capacity) {
          return ZX_ERR_IO;
        }
        offset += actual;
        memcpy(buffer, data.begin(), actual);
        *out_actual = actual;
        return ZX_OK;
      });
}

zx_status_t FileWritevAt(const fidl::WireSyncClient<fio::File>& client, zx_off_t offset,
                         const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                         size_t* out_actual) {
  return zxio_chunked_do_vector<fio::wire::kMaxBuf>(
      vector, vector_count, flags, out_actual,
      [&client, &offset](uint8_t* buffer, size_t capacity, size_t* out_actual) {
        // Explicitly allocating message buffers to avoid heap allocation.
        fidl::SyncClientBuffer<fio::File::WriteAt> fidl_buffer;
        const fidl::WireUnownedResult result =
            client.buffer(fidl_buffer.view())
                ->WriteAt(fidl::VectorView<uint8_t>::FromExternal(buffer, capacity), offset);
        if (!result.ok()) {
          return result.status();
        }
        const auto& response = result.value();
        if (response.is_error()) {
          return response.error_value();
        }
        const size_t actual = response.value()->actual_count;
        if (actual > capacity) {
          return ZX_ERR_IO;
        }
        offset += actual;
        *out_actual = actual;
        return ZX_OK;
      });
}

template <typename Protocol>
constexpr zxio_object_type_t ProtocolToObjectType() {
  if constexpr (std::is_same_v<Protocol, fio::Directory>) {
    return ZXIO_OBJECT_TYPE_DIR;
  } else if constexpr (std::is_same_v<Protocol, fio::Node>) {
    return ZXIO_OBJECT_TYPE_NODE;
  } else if constexpr (std::is_same_v<Protocol, fio::File>) {
    return ZXIO_OBJECT_TYPE_FILE;
  } else if constexpr (std::is_same_v<Protocol, fio::Symlink>) {
    return ZXIO_OBJECT_TYPE_SYMLINK;
  } else {
    static_assert(false, "Unmapped protocol type!");
  }
}

class Directory;

// Implementation of |zxio_dirent_iterator_t| for |fuchsia.io| v1.
class DirentIteratorImpl {
 public:
  explicit DirentIteratorImpl(const zxio_t* io) : io_(reinterpret_cast<const Directory*>(io)) {
    static_assert(offsetof(DirentIteratorImpl, io_) == 0,
                  "zxio_dirent_iterator_t requires first field of implementation to be zxio_t");
  }

  zx_status_t Next(zxio_dirent_t* inout_entry) {
    if (remaining_dirents_.empty()) {
      const zx_status_t status = RemoteReadDirents();
      if (status != ZX_OK) {
        return status;
      }
      if (remaining_dirents_.empty()) {
        return ZX_ERR_NOT_FOUND;
      }
    }

    // The format of the packed dirent structure, taken from io.fidl.
    struct dirent {
      // Describes the inode of the entry.
      uint64_t ino;
      // Describes the length of the dirent name in bytes.
      uint8_t size;
      // Describes the type of the entry. Aligned with the
      // POSIX d_type values. Use `DIRENT_TYPE_*` constants.
      uint8_t type;
      // Unterminated name of entry.
      char name[0];
    } __PACKED;

    // Check if we can read the entry size.
    if (remaining_dirents_.size_bytes() < sizeof(dirent)) {
      // Should not happen
      return ZX_ERR_INTERNAL;
    }

    const dirent& packed_entry = *reinterpret_cast<const dirent*>(remaining_dirents_.data());
    const size_t packed_entry_size = sizeof(dirent) + packed_entry.size;

    // Check if we can read the whole entry.
    if (remaining_dirents_.size_bytes() < packed_entry_size) {
      // Should not happen
      return ZX_ERR_INTERNAL;
    }

    // Check that the name length is within bounds.
    if (packed_entry.size > fio::wire::kMaxNameLength) {
      return ZX_ERR_INVALID_ARGS;
    }

    remaining_dirents_ = remaining_dirents_.subspan(packed_entry_size);

    ZXIO_DIRENT_SET(*inout_entry, protocols, DTypeToProtocols(packed_entry.type));
    ZXIO_DIRENT_SET(*inout_entry, id, packed_entry.ino);
    inout_entry->name_length = packed_entry.size;
    if (inout_entry->name != nullptr) {
      memcpy(inout_entry->name, packed_entry.name, packed_entry.size);
    }

    return ZX_OK;
  }

  zx_status_t Rewind() {
    const fidl::WireResult result = client()->Rewind();
    if (!result.ok()) {
      return result.status();
    }
    if (result->s != ZX_OK) {
      return result->s;
    }

    // Reset the state of the iterator, forcing an update the next time |Next| is called.
    remaining_dirents_ = {};
    return ZX_OK;
  }

 private:
  const fidl::WireSyncClient<fio::Directory>& client() const;

  zx_status_t RemoteReadDirents() {
    fidl::BufferSpan fidl_buffer(buffer_, sizeof(buffer_));
    const fidl::WireUnownedResult result = client().buffer(fidl_buffer)->ReadDirents(kBufferSize);
    if (!result.ok()) {
      return result.status();
    }
    const auto& response = result.value();
    if (const zx_status_t status = response.s; status != ZX_OK) {
      return status;
    }
    const fidl::VectorView dirents = response.dirents;
    if (dirents.size() > kBufferSize) {
      return ZX_ERR_IO;
    }

    remaining_dirents_ = dirents.get();
    return ZX_OK;
  }

  static zxio_node_protocols_t DTypeToProtocols(uint8_t type) {
    switch (type) {
      case DT_DIR:
        return ZXIO_NODE_PROTOCOL_DIRECTORY;
      case DT_REG:
        return ZXIO_NODE_PROTOCOL_FILE;
      case DT_LNK:
        return ZXIO_NODE_PROTOCOL_SYMLINK;
      case DT_BLK:
        // Not supported.
      case DT_CHR:
        // Not supported.
      case DT_FIFO:
        // Not supported.
      case DT_SOCK:
        // Not supported.
      default:
        return ZXIO_NODE_PROTOCOL_NONE;
    }
  }

  // The maximum buffer size that is supported by |fuchsia.io/Directory.ReadDirents|.
  static constexpr size_t kBufferSize = fio::wire::kMaxBuf;

  const Directory* const io_;

  // Issuing a FIDL call requires storage for both the request (16 bytes) and the largest possible
  // response message (8192 bytes of payload).
  // TODO(https://fxbug.dev/42166787): Once overlapping request and response is allowed, reduce
  // this allocation to a single channel message size.
  FIDL_ALIGNDECL uint8_t
      buffer_[fidl::SyncClientMethodBufferSizeInChannel<fio::Directory::ReadDirents>()];

  // Tracks and holds a view into |buffer_| of remaining unread dirents.
  cpp20::span<uint8_t> remaining_dirents_;
};

static_assert(sizeof(DirentIteratorImpl) <= sizeof(zxio_dirent_iterator_t),
              "DirentIteratorImpl should fit within a zxio_dirent_iterator_t");

// POSIX expects `EBADF` for access denied errors, which is mapped from `ZX_ERR_BAD_HANDLE`.
// `ZX_ERR_ACCESS_DENIED` maps to `EACCES` which should only be used for sockets.
zx_status_t MapToPosixStatus(zx_status_t status) {
  return status == ZX_ERR_ACCESS_DENIED ? ZX_ERR_BAD_HANDLE : status;
}

template <typename Protocol>
class Remote : public HasIo {
 protected:
  Remote(fidl::ClientEnd<Protocol> client_end, const zxio_ops_t& ops)
      : HasIo(ops), client_(std::move(client_end)) {}

  zx_status_t Sync();

  zx_status_t AttrGet(zxio_node_attributes_t* inout_attr);

  zx_status_t AttrSet(const zxio_node_attributes_t* attr);

  zx_status_t AdvisoryLock(advisory_lock_req* req);

  zx_status_t FlagsGet(uint64_t* out_flags);

  zx_status_t FlagsSet(uint64_t flags);

  zx_status_t LinkInto(zx_handle_t dst_token, const char* dst_path, size_t dst_path_len);

  zx_status_t XattrList(void (*callback)(void* context, const uint8_t* name, size_t name_len),
                        void* context);

  zx_status_t XattrGet(const uint8_t* name, size_t name_len,
                       zx_status_t (*callback)(void* context, zxio_xattr_data_t data),
                       void* context);

  zx_status_t XattrSet(const uint8_t* name, size_t name_len, const uint8_t* value, size_t value_len,
                       zxio_xattr_set_mode_t mode);

  zx_status_t XattrRemove(const uint8_t* name, size_t name_len);

  const fidl::WireSyncClient<Protocol>& client() const { return client_; }

  template <typename T>
    requires std::is_base_of_v<Remote, T>
  static constexpr zxio_ops_t CommonRemoteOps() {
    zxio_ops_t ops = zxio_default_ops;

    // Operations we disallow "overloading" as this class manages the associated object's resources.
    ops.destroy = Remote::Destroy<T>;
    ops.close = Adaptor<T>::template From<&T::Close>;
    ops.release = Adaptor<T>::template From<&T::Release>;
    ops.borrow = Adaptor<T>::template From<&T::Borrow>;
    ops.clone = Adaptor<T>::template From<&T::Clone>;

    // Common `fuchsia.io/Node` operations. Note that derived classes may "overload" these
    // operations by providing a method in their public interface with the same name/type.
    ops.attr_get = Adaptor<T>::template From<&T::AttrGet>;
    ops.attr_set = Adaptor<T>::template From<&T::AttrSet>;
    ops.flags_get = Adaptor<T>::template From<&T::FlagsGet>;
    ops.flags_set = Adaptor<T>::template From<&T::FlagsSet>;
    ops.sync = Adaptor<T>::template From<&T::Sync>;
    ops.xattr_list = Adaptor<T>::template From<&T::XattrList>;
    ops.xattr_get = Adaptor<T>::template From<&T::XattrGet>;
    ops.xattr_set = Adaptor<T>::template From<&T::XattrSet>;
    ops.xattr_remove = Adaptor<T>::template From<&T::XattrRemove>;

    return ops;
  }

 private:
  zx_status_t Release(zx_handle_t* out_handle) {
    *out_handle = client_.TakeClientEnd().TakeChannel().release();
    return ZX_OK;
  }

  zx_status_t Borrow(zx_handle_t* out_handle) {
    *out_handle = client_.client_end().channel().get();
    return ZX_OK;
  }

  zx_status_t Clone(zx_handle_t* out_handle) {
    // fuchsia.io/Node composes fuchsia.unknown/Cloneable. The client end below will speak whatever
    // protocol was negotiated for the current connection.
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_unknown::Cloneable>::Create();
    const fidl::Status result = client_->Clone(std::move(server_end));
    if (!result.ok()) {
      return result.status();
    }
    *out_handle = client_end.TakeChannel().release();
    return ZX_OK;
  }

  /// Helper that implements `zxio_ops_t::destroy` correctly for objects which derive `Remote`.
  /// This method is static since we destroy the object `T` represented by `zxio`, making it more
  /// clear when the object should no longer be used.
  template <typename T>
    requires std::is_base_of_v<Remote, T>
  static void Destroy(zxio_t* zxio) {
    T& instance = *reinterpret_cast<T*>(zxio);
    instance.~T();
  }

  zx_status_t Close() {
    if (client_.is_valid()) {
      const fidl::WireResult result = client_->Close();
      if (!result.ok()) {
        return result.status();
      }
      const auto& response = result.value();
      if (response.is_error()) {
        return response.error_value();
      }
    }
    return ZX_OK;
  }

  fidl::WireSyncClient<Protocol> client_;
};

template <typename Protocol>
zx_status_t Remote<Protocol>::Sync() {
  const fidl::WireResult result = client()->Sync();
  if (!result.ok()) {
    return result.status();
  }
  const auto& response = result.value();
  if (response.is_error()) {
    return response.error_value();
  }
  return ZX_OK;
}

constexpr fio::NodeAttributesQuery BuildAttributeQuery(
    const zxio_node_attributes_t::zxio_node_attr_has_t& attr_has) {
  fio::NodeAttributesQuery query;

  if (attr_has.protocols)
    query |= fio::NodeAttributesQuery::kProtocols;
  if (attr_has.abilities)
    query |= fio::NodeAttributesQuery::kAbilities;
  if (attr_has.content_size)
    query |= fio::NodeAttributesQuery::kContentSize;
  if (attr_has.storage_size)
    query |= fio::NodeAttributesQuery::kStorageSize;
  if (attr_has.link_count)
    query |= fio::NodeAttributesQuery::kLinkCount;
  if (attr_has.id)
    query |= fio::NodeAttributesQuery::kId;
  if (attr_has.creation_time)
    query |= fio::NodeAttributesQuery::kCreationTime;
  if (attr_has.modification_time)
    query |= fio::NodeAttributesQuery::kModificationTime;
  if (attr_has.change_time)
    query |= fio::NodeAttributesQuery::kChangeTime;
  if (attr_has.access_time)
    query |= fio::NodeAttributesQuery::kAccessTime;
  if (attr_has.mode)
    query |= fio::NodeAttributesQuery::kMode;
  if (attr_has.uid)
    query |= fio::NodeAttributesQuery::kUid;
  if (attr_has.gid)
    query |= fio::NodeAttributesQuery::kGid;
  if (attr_has.rdev)
    query |= fio::NodeAttributesQuery::kRdev;
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (attr_has.fsverity_options)
    query |= fio::NodeAttributesQuery::kOptions;
  if (attr_has.fsverity_root_hash)
    query |= fio::NodeAttributesQuery::kRootHash;
  if (attr_has.fsverity_enabled)
    query |= fio::NodeAttributesQuery::kVerityEnabled;
  if (attr_has.casefold)
    query |= fio::NodeAttributesQuery::kCasefold;
  if (attr_has.wrapping_key_id)
    query |= fio::NodeAttributesQuery::kWrappingKeyId;
  if (attr_has.selinux_context)
    query |= fio::NodeAttributesQuery::kSelinuxContext;
  if (attr_has.pending_access_time_update)
    query |= fio::NodeAttributesQuery::kPendingAccessTimeUpdate;
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  return query;
}

// Holds external values for MutableNodeAttributes that need to be held onto until requests are
// complete. These are small view values only, as they will live on the stack.
struct MutableAttributesDataHolder {
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  fio::wire::SelinuxContext selinux_context_wrapper;
  fidl::VectorView<uint8_t> selinux_context_view;
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
};

zx::result<fio::wire::MutableNodeAttributes> BuildMutableAttributes(
    const zxio_node_attributes_t* mutable_attrs,
    fidl::WireTableFrame<fio::wire::MutableNodeAttributes>& mutable_attrs_frame,
    MutableAttributesDataHolder* holder) {
  auto builder = fio::wire::MutableNodeAttributes::ExternalBuilder(
      fidl::ObjectView<fidl::WireTableFrame<fio::wire::MutableNodeAttributes>>::FromExternal(
          &mutable_attrs_frame));

  // Ensure no immutable attributes were specified.
  if (mutable_attrs->has.protocols || mutable_attrs->has.abilities || mutable_attrs->has.id ||
      mutable_attrs->has.content_size || mutable_attrs->has.storage_size ||
      mutable_attrs->has.link_count) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (mutable_attrs->has.change_time) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (mutable_attrs->has.fsverity_enabled || mutable_attrs->has.fsverity_options ||
      mutable_attrs->has.fsverity_root_hash) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (mutable_attrs->has.creation_time) {
    builder.creation_time(fidl::ObjectView<uint64_t>::FromExternal(
        const_cast<uint64_t*>(&mutable_attrs->creation_time)));
  }
  if (mutable_attrs->has.modification_time) {
    builder.modification_time(fidl::ObjectView<uint64_t>::FromExternal(
        const_cast<uint64_t*>(&mutable_attrs->modification_time)));
  }
  if (mutable_attrs->has.access_time) {
    builder.access_time(fidl::ObjectView<uint64_t>::FromExternal(
        const_cast<uint64_t*>(&mutable_attrs->access_time)));
  }
  if (mutable_attrs->has.mode) {
    builder.mode(mutable_attrs->mode);
  }
  if (mutable_attrs->has.uid) {
    builder.uid(mutable_attrs->uid);
  }
  if (mutable_attrs->has.gid) {
    builder.gid(mutable_attrs->gid);
  }
  if (mutable_attrs->has.rdev) {
    builder.rdev(
        fidl::ObjectView<uint64_t>::FromExternal(const_cast<uint64_t*>(&mutable_attrs->rdev)));
  }
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (mutable_attrs->has.selinux_context) {
    size_t len = static_cast<size_t>(mutable_attrs->selinux_context_length);
    if (mutable_attrs->selinux_context_state == ZXIO_SELINUX_CONTEXT_STATE_DATA &&
        mutable_attrs->selinux_context && len <= fio::wire::kMaxSelinuxContextAttributeLen) {
      holder->selinux_context_view = fidl::VectorView<uint8_t>::FromExternal(
          const_cast<uint8_t*>(mutable_attrs->selinux_context), len);
      holder->selinux_context_wrapper = fio::wire::SelinuxContext::WithData(
          fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(&holder->selinux_context_view));
      builder.selinux_context(fidl::ObjectView<fio::wire::SelinuxContext>::FromExternal(
          &holder->selinux_context_wrapper));
    } else {
      // Per the fuchsia.io API, it is never legal to set `USE_EXTENDED_ATTRIBUTES` from the client.
      // Any unknown enum ordinal or an overlong payload is also obviously invalid.
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  }
  if (mutable_attrs->has.casefold) {
    builder.casefold(mutable_attrs->casefold);
  }
  if (mutable_attrs->has.wrapping_key_id) {
    builder.wrapping_key_id(
        fidl::ObjectView<fidl::Array<uint8_t, ZXIO_WRAPPING_KEY_ID_LENGTH>>::FromExternal(
            reinterpret_cast<fidl::Array<uint8_t, ZXIO_WRAPPING_KEY_ID_LENGTH>*>(
                const_cast<uint8_t*>(mutable_attrs->wrapping_key_id))));
  }
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  return zx::ok(builder.Build());
}

template <typename Protocol>
zx_status_t Remote<Protocol>::AttrGet(zxio_node_attributes_t* inout_attr) {
  if (inout_attr->has.object_type) {
    ZXIO_NODE_ATTR_SET(*inout_attr, object_type, ProtocolToObjectType<Protocol>());
  }
  // Construct query from has in inout_attr->has.
  const fio::NodeAttributesQuery query = BuildAttributeQuery(inout_attr->has);
  if (!query) {
    // If we don't have any remaining attributes to query, skip a round trip call to the server.
    return ZX_OK;
  }
  const fidl::WireResult result = client()->GetAttributes(query);
  if (!result.ok()) {
    return result.status();
  }
  const auto& response = result.value();
  if (response.is_error()) {
    return MapToPosixStatus(response.error_value());
  }
  const fio::wire::NodeAttributes2* attributes = response.value();
  return zxio_attr_from_wire(*attributes, inout_attr);
}

template <typename Protocol>
zx_status_t Remote<Protocol>::AttrSet(const zxio_node_attributes_t* attr) {
  MutableAttributesDataHolder mutable_attributes_holder;
  fidl::WireTableFrame<fio::wire::MutableNodeAttributes> mutable_attrs_frame;
  const zx::result mutable_attributes =
      BuildMutableAttributes(attr, mutable_attrs_frame, &mutable_attributes_holder);
  if (mutable_attributes.is_error()) {
    return mutable_attributes.error_value();
  }
  const fidl::WireResult result = client()->UpdateAttributes(*mutable_attributes);
  if (!result.ok()) {
    return result.status();
  }
  const auto& response = result.value();
  if (response.is_error()) {
    return response.error_value();
  }
  return ZX_OK;
}

template <typename Protocol>
zx_status_t Remote<Protocol>::AdvisoryLock(advisory_lock_req* req) {
  fio::wire::AdvisoryLockType lock_type;
  switch (req->type) {
    case ADVISORY_LOCK_SHARED:
      lock_type = fio::wire::AdvisoryLockType::kRead;
      break;
    case ADVISORY_LOCK_EXCLUSIVE:
      lock_type = fio::wire::AdvisoryLockType::kWrite;
      break;
    case ADVISORY_LOCK_UNLOCK:
      lock_type = fio::wire::AdvisoryLockType::kUnlock;
      break;
    default:
      return ZX_ERR_INTERNAL;
  }
  fidl::Arena allocator;
  const fidl::WireResult result = client()->AdvisoryLock(
      fio::wire::AdvisoryLockRequest::Builder(allocator).type(lock_type).wait(req->wait).Build());
  if (!result.ok()) {
    return result.status();
  }
  const auto& response = result.value();
  if (response.is_error()) {
    return response.error_value();
  }
  return ZX_OK;
}

template <typename Protocol>
zx_status_t Remote<Protocol>::FlagsGet(uint64_t* out_flags) {
  const fidl::WireResult result = client()->GetFlags();
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  *out_flags = uint64_t{(*result)->flags};
  return ZX_OK;
}

template <typename Protocol>
zx_status_t Remote<Protocol>::FlagsSet(uint64_t flags) {
  const fidl::WireResult result = client()->SetFlags(fio::wire::Flags{flags});
  if (!result.ok()) {
    return result.status();
  }
  if (result->is_error()) {
    return result->error_value();
  }
  return ZX_OK;
}

template <typename Protocol>
zx_status_t Remote<Protocol>::LinkInto(zx_handle_t dst_token_handle, const char* dst_path,
                                       size_t dst_path_len) {
  zx::event dst_token(dst_token_handle);
  const fidl::WireResult result = client()->LinkInto(
      std::move(dst_token), fidl::StringView::FromExternal(dst_path, dst_path_len));
  if (!result.ok()) {
    return result.status();
  }
  const auto& response = result.value();
  if (response.is_error()) {
    return response.error_value();
  }
  return ZX_OK;
}

template <typename Protocol>
zx_status_t Remote<Protocol>::XattrList(void (*callback)(void* context, const uint8_t* name,
                                                         size_t name_len),
                                        void* context) {
  if (!client().is_valid()) {
    return ZX_ERR_BAD_STATE;
  }

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::ExtendedAttributeIterator>::Create();
  const fidl::OneWayStatus result = client()->ListExtendedAttributes(std::move(server_end));
  if (!result.ok()) {
    return result.status();
  }

  while (true) {
    const fidl::WireResult next_result = fidl::WireCall(client_end)->GetNext();
    if (!next_result.ok()) {
      return next_result.status();
    }
    const auto& response = next_result.value();
    if (response.is_error()) {
      return response.error_value();
    }
    const fidl::VectorView data = response->attributes;
    for (fidl::VectorView<uint8_t>& name : data) {
      callback(context, name.data(), name.size());
    }
    if (response->last) {
      break;
    }
  }

  return ZX_OK;
}

template <typename Protocol>
zx_status_t Remote<Protocol>::XattrGet(const uint8_t* name, size_t name_len,
                                       zx_status_t (*callback)(void* context,
                                                               zxio_xattr_data_t data),
                                       void* context) {
  if (!client().is_valid()) {
    return ZX_ERR_BAD_STATE;
  }

  fidl::Arena arena;
  const fidl::WireResult result =
      client()->GetExtendedAttribute(fidl::VectorView<uint8_t>(arena, cpp20::span(name, name_len)));
  if (!result.ok()) {
    return result.status();
  }
  const auto& response = result.value();
  if (response.is_error()) {
    return response.error_value();
  }

  const fuchsia_io::wire::ExtendedAttributeValue* value_resp = response.value();
  zxio_xattr_data_t data;
  if (value_resp->is_bytes()) {
    const fidl::VectorView value_bytes = value_resp->bytes();
    data = {
        .data = value_bytes.data(),
        .vmo = ZX_HANDLE_INVALID,
        .len = value_bytes.size(),
    };
  } else {
    const zx::vmo& value_vmo = value_resp->buffer();
    uint64_t value_size;
    if (zx_status_t status = value_vmo.get_prop_content_size(&value_size); status != ZX_OK) {
      return status;
    }
    data = {
        .data = nullptr,
        .vmo = value_vmo.get(),
        .len = value_size,
    };
  }

  return callback(context, data);
}

template <typename Protocol>
zx_status_t Remote<Protocol>::XattrSet(const uint8_t* name, size_t name_len, const uint8_t* value,
                                       size_t value_len, zxio_xattr_set_mode_t mode) {
  if (!client().is_valid()) {
    return ZX_ERR_BAD_STATE;
  }

  fidl::Arena arena;
  fuchsia_io::wire::ExtendedAttributeValue attribute_value;
  if (value_len > fuchsia_io::wire::kMaxInlineAttributeValue) {
    zx::vmo val;
    if (zx_status_t status = zx::vmo::create(value_len, 0, &val); status != ZX_OK) {
      return status;
    }
    if (zx_status_t status = val.write(value, 0, value_len); status != ZX_OK) {
      return status;
    }
    attribute_value = fuchsia_io::wire::ExtendedAttributeValue::WithBuffer(std::move(val));
  } else {
    auto value_vec = fidl::VectorView<uint8_t>(arena, cpp20::span(value, value_len));
    fidl::ObjectView value_obj(arena, value_vec);
    attribute_value = fuchsia_io::wire::ExtendedAttributeValue::WithBytes(value_obj);
  }

  fio::SetExtendedAttributeMode mode_fidl;
  switch (mode) {
    case ZXIO_XATTR_SET:
      mode_fidl = fio::SetExtendedAttributeMode::kSet;
      break;
    case ZXIO_XATTR_CREATE:
      mode_fidl = fio::SetExtendedAttributeMode::kCreate;
      break;
    case ZXIO_XATTR_REPLACE:
      mode_fidl = fio::SetExtendedAttributeMode::kReplace;
      break;
    default:
      return ZX_ERR_INVALID_ARGS;
  }

  const fidl::WireResult result =
      client()->SetExtendedAttribute(fidl::VectorView<uint8_t>(arena, cpp20::span(name, name_len)),
                                     std::move(attribute_value), mode_fidl);
  if (!result.ok()) {
    return result.status();
  }
  if (result.value().is_error()) {
    return result.value().error_value();
  }

  return ZX_OK;
}

template <typename Protocol>
zx_status_t Remote<Protocol>::XattrRemove(const uint8_t* name, size_t name_len) {
  if (!client().is_valid()) {
    return ZX_ERR_BAD_STATE;
  }

  fidl::Arena arena;
  const fidl::WireResult result = client()->RemoveExtendedAttribute(
      fidl::VectorView<uint8_t>(arena, cpp20::span(name, name_len)));
  if (!result.ok()) {
    return result.status();
  }
  if (result.value().is_error()) {
    return result.value().error_value();
  }

  return ZX_OK;
}

class Node : public Remote<fio::Node> {
 public:
  explicit Node(fidl::ClientEnd<fio::Node> client_end) : Remote(std::move(client_end), kOps) {}

 private:
  static const zxio_ops_t kOps;
};

constexpr zxio_ops_t Node::kOps = CommonRemoteOps<Node>();

class Directory : public Remote<fio::Directory> {
 public:
  explicit Directory(fidl::ClientEnd<fio::Directory> client_end)
      : Remote(std::move(client_end), kOps) {}

  friend class DirentIteratorImpl;

  zx_status_t Readv(const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                    size_t* out_actual) {
    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    return zxio_do_vector(
        vector, vector_count, out_actual,
        [](void* buffer, size_t capacity, size_t total_so_far, size_t* out_actual) {
          if (capacity > 0) {
            return ZX_ERR_WRONG_TYPE;
          }
          *out_actual = 0;
          return ZX_OK;
        });
  }

  zx_status_t ReadvAt(zx_off_t offset, const zx_iovec_t* vector, size_t vector_count,
                      zxio_flags_t flags, size_t* out_actual) {
    return Readv(vector, vector_count, flags, out_actual);
  }

  zx_status_t WatchDirectory(zxio_watch_directory_cb cb, zx_time_t deadline, void* context) {
    if (cb == nullptr) {
      return ZX_ERR_INVALID_ARGS;
    }
    zx::result endpoints = fidl::CreateEndpoints<fio::DirectoryWatcher>();
    if (endpoints.is_error()) {
      return endpoints.status_value();
    }

    const fidl::WireResult result =
        client()->Watch(fio::wire::WatchMask::kMask, 0, std::move(endpoints->server));

    if (!result.ok()) {
      return result.status();
    }
    const auto& response = result.value();
    if (const zx_status_t status = response.s; status != ZX_OK) {
      return status;
    }

    for (;;) {
      uint8_t bytes[fio::wire::kMaxBuf + 1];  // Extra byte for temporary null terminator.
      uint32_t num_bytes;
      zx_status_t status = endpoints->client.channel().read_etc(0, &bytes, nullptr, sizeof(bytes),
                                                                0, &num_bytes, nullptr);
      if (status != ZX_OK) {
        if (status == ZX_ERR_SHOULD_WAIT) {
          status = endpoints->client.channel().wait_one(
              ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED, zx::time(deadline), nullptr);
          if (status != ZX_OK) {
            return status;
          }
          continue;
        }
        return status;
      }

      // Message Format: { OP, LEN, DATA[LEN] }
      const cpp20::span span(bytes, num_bytes);
      auto it = span.begin();
      for (;;) {
        if (std::distance(it, span.end()) < 2) {
          break;
        }

        const fio::wire::WatchEvent wire_event = static_cast<fio::wire::WatchEvent>(*it++);
        const uint8_t len = *it++;
        uint8_t* name = &*it;

        if (std::distance(it, span.end()) < len) {
          break;
        }
        it += len;

        zxio_watch_directory_event_t event;
        switch (wire_event) {
          case fio::wire::WatchEvent::kAdded:
          case fio::wire::WatchEvent::kExisting:
            event = ZXIO_WATCH_EVENT_ADD_FILE;
            break;
          case fio::wire::WatchEvent::kRemoved:
            event = ZXIO_WATCH_EVENT_REMOVE_FILE;
            break;
          case fio::wire::WatchEvent::kIdle:
            event = ZXIO_WATCH_EVENT_WAITING;
            break;
          default:
            // unsupported event
            continue;
        }

        // The callback expects a null-terminated string.
        const uint8_t tmp = *it;
        *it = 0;
        status = cb(event, reinterpret_cast<const char*>(name), context);
        *it = tmp;
        if (status != ZX_OK) {
          return status;
        }
      }
    }
  }

  zx_status_t CreateSymlink(const char* name, size_t name_len, const uint8_t* target,
                            size_t target_len, zxio_storage_t* storage) {
    fidl::Endpoints<fio::Symlink> endpoints;
    if (storage) {
      if (auto result = fidl::CreateEndpoints<fio::Symlink>(); result.is_error()) {
        return result.status_value();
      } else {
        endpoints = *std::move(result);
      }
    }

    fidl::Arena arena;
    const fidl::WireResult result =
        client()->CreateSymlink(fidl::StringView(arena, std::string_view(name, name_len)),
                                fidl::VectorView<uint8_t>(arena, cpp20::span(target, target_len)),
                                std::move(endpoints.server));
    if (!result.ok()) {
      return result.status();
    }
    const auto& response = result.value();
    if (response.is_error()) {
      return response.error_value();
    }
    if (storage) {
      if (zx_status_t status = zxio_symlink_init(storage, std::move(endpoints.client),
                                                 std::vector(target, target + target_len));
          status != ZX_OK) {
        return status;
      }
    }
    return ZX_OK;
  }

  zx_status_t Open(const char* path, size_t path_len, zxio_open_flags_t flags,
                   const zxio_open_options_t* options, zxio_storage_t* storage) {
    zx::channel client_end, server_end;
    if (zx_status_t status = zx::channel::create(0, &client_end, &server_end); status != ZX_OK) {
      return status;
    }

    fio::NodeAttributesQuery attributes;
    MutableAttributesDataHolder mutable_attributes_holder;
    fidl::WireTableFrame<fio::wire::MutableNodeAttributes> create_attributes_frame;
    zx::result<fio::wire::MutableNodeAttributes> create_attributes;
    fidl::WireTableFrame<fio::wire::Options> options_frame;
    fio::wire::Options open_options;

    if (options && (options->inout_attr || options->create_attr)) {
      auto options_builder = fio::wire::Options::ExternalBuilder(
          fidl::ObjectView<fidl::WireTableFrame<fio::wire::Options>>::FromExternal(&options_frame));

      // -- attributes --
      if (options->inout_attr) {
        attributes = BuildAttributeQuery(options->inout_attr->has);
        if (attributes) {
          options_builder.attributes(
              fidl::ObjectView<fio::wire::NodeAttributesQuery>::FromExternal(&attributes));
        }
      }

      // -- create_attributes --
      if (options->create_attr) {
        create_attributes = BuildMutableAttributes(options->create_attr, create_attributes_frame,
                                                   &mutable_attributes_holder);
        if (create_attributes.is_error()) {
          return create_attributes.error_value();
        }
        options_builder.create_attributes(
            fidl::ObjectView<fio::wire::MutableNodeAttributes>::FromExternal(
                &create_attributes.value()));
      }

      open_options = options_builder.Build();
    }

    const fidl::Status result =
        client()->Open(fidl::StringView::FromExternal(path, path_len),
                       fio::Flags::kFlagSendRepresentation | fio::Flags{flags}, open_options,
                       std::move(server_end));

    if (!result.ok()) {
      return result.status();
    }
    return zxio_create_with_on_representation(client_end.release(),
                                              options ? options->inout_attr : nullptr, storage);
  }

  zx_status_t Unlink(const char* name, size_t name_len, int flags) {
    fidl::Arena allocator;
    auto options = fio::wire::UnlinkOptions::Builder(allocator);
    auto io_flags = fio::wire::UnlinkFlags::kMustBeDirectory;
    if (flags & AT_REMOVEDIR) {
      options.flags(fidl::ObjectView<decltype(io_flags)>::FromExternal(&io_flags));
    }
    const fidl::WireResult result =
        client()->Unlink(fidl::StringView::FromExternal(name, name_len), options.Build());
    if (!result.ok()) {
      return result.status();
    }
    const auto& response = result.value();
    if (response.is_error()) {
      return response.error_value();
    }
    return ZX_OK;
  }

  zx_status_t TokenGet(zx_handle_t* out_token) {
    fidl::WireResult result = client()->GetToken();
    if (!result.ok()) {
      return result.status();
    }
    auto& response = result.value();
    if (const zx_status_t status = response.s; status != ZX_OK) {
      return status;
    }
    *out_token = response.token.release();
    return ZX_OK;
  }

  zx_status_t Rename(const char* old_path, size_t old_path_len, zx_handle_t dst_token,
                     const char* new_path, size_t new_path_len) {
    const fidl::WireResult result = client()->Rename(
        fidl::StringView::FromExternal(old_path, old_path_len), zx::event(dst_token),
        fidl::StringView::FromExternal(new_path, new_path_len));
    if (!result.ok()) {
      return result.status();
    }
    const auto& response = result.value();
    if (response.is_error()) {
      return response.error_value();
    }
    return ZX_OK;
  }

  zx_status_t Link(const char* src_path, size_t src_path_len, zx_handle_t dst_token,
                   const char* dst_path, size_t dst_path_len) {
    const fidl::WireResult result = client()->Link(
        fidl::StringView::FromExternal(src_path, src_path_len), zx::handle(dst_token),
        fidl::StringView::FromExternal(dst_path, dst_path_len));
    if (!result.ok()) {
      return result.status();
    }
    const auto& response = result.value();
    return response.s;
  }

  zx_status_t DirentIteratorInit(zxio_dirent_iterator_t* iterator) {
    new (iterator) DirentIteratorImpl(io());
    return ZX_OK;
  }

  zx_status_t DirentIteratorNext(zxio_dirent_iterator_t* iterator, zxio_dirent_t* inout_entry) {
    return reinterpret_cast<DirentIteratorImpl*>(iterator)->Next(inout_entry);
  }

  zx_status_t DirentIteratorRewind(zxio_dirent_iterator_t* iterator) {
    return reinterpret_cast<DirentIteratorImpl*>(iterator)->Rewind();
  }

  void DirentIteratorDestroy(zxio_dirent_iterator_t* iterator) {
    reinterpret_cast<DirentIteratorImpl*>(iterator)->~DirentIteratorImpl();
  }

  static const zxio_ops_t kOps;
};

const fidl::WireSyncClient<fio::Directory>& DirentIteratorImpl::client() const {
  return io_->client();
}

constexpr zxio_ops_t Directory::kOps = ([]() {
  zxio_ops_t ops = CommonRemoteOps<Directory>();

  using Adaptor = Adaptor<Directory>;
  ops.readv = Adaptor::From<&Directory::Readv>;
  ops.readv_at = Adaptor::From<&Directory::ReadvAt>;
  ops.open = Adaptor::From<&Directory::Open>;
  ops.unlink = Adaptor::From<&Directory::Unlink>;
  ops.token_get = Adaptor::From<&Directory::TokenGet>;
  ops.rename = Adaptor::From<&Directory::Rename>;
  ops.link = Adaptor::From<&Directory::Link>;
  ops.dirent_iterator_init = Adaptor::From<&Directory::DirentIteratorInit>;
  ops.dirent_iterator_next = Adaptor::From<&Directory::DirentIteratorNext>;
  ops.dirent_iterator_rewind = Adaptor::From<&Directory::DirentIteratorRewind>;
  ops.dirent_iterator_destroy = Adaptor::From<&Directory::DirentIteratorDestroy>;
  ops.watch_directory = Adaptor::From<&Directory::WatchDirectory>;
  ops.advisory_lock = Adaptor::From<&Directory::AdvisoryLock>;
  ops.create_symlink = Adaptor::From<&Directory::CreateSymlink>;
  return ops;
})();

class File : public Remote<fio::File> {
 public:
  File(fidl::ClientEnd<fio::File> client_end, zx::event event, zx::stream stream)
      : Remote(std::move(client_end), kOps), event_(std::move(event)), stream_(std::move(stream)) {}

 private:
  zx_status_t EnableVerity(const zxio_fsverity_descriptor_t* descriptor) {
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    fidl::WireTableFrame<fio::wire::VerificationOptions> verification_options_frame;
    fio::wire::VerificationOptions verification_options;
    auto builder = fio::wire::VerificationOptions::ExternalBuilder(
        fidl::ObjectView<fidl::WireTableFrame<fio::wire::VerificationOptions>>::FromExternal(
            &verification_options_frame));
    builder.hash_algorithm(static_cast<fio::wire::HashAlgorithm>(descriptor->hash_algorithm));
    fidl::VectorView<uint8_t> vec = fidl::VectorView<uint8_t>::FromExternal(
        const_cast<uint8_t*>(std::begin(descriptor->salt)), descriptor->salt_size);
    builder.salt(fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(&vec));
    verification_options = builder.Build();

    const fidl::WireResult result = client()->EnableVerity(verification_options);
    if (!result.ok()) {
      return result.status();
    }
    const auto& response = result.value();
    if (response.is_error()) {
      return response.error_value();
    }
    return ZX_OK;
#else
    return ZX_ERR_NOT_SUPPORTED;
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  }

  void WaitBegin(zxio_signals_t zxio_signals, zx_handle_t* out_handle,
                 zx_signals_t* out_zx_signals) {
    *out_handle = event_.get();

    zx_signals_t zx_signals = ZX_SIGNAL_NONE;
    if (zxio_signals & ZXIO_SIGNAL_READABLE) {
      zx_signals |= static_cast<zx_signals_t>(fio::wire::FileSignal::kReadable);
    }
    if (zxio_signals & ZXIO_SIGNAL_WRITABLE) {
      zx_signals |= static_cast<zx_signals_t>(fio::wire::FileSignal::kWritable);
    }
    *out_zx_signals = zx_signals;
  }

  void WaitEnd(zx_signals_t zx_signals, zxio_signals_t* out_zxio_signals) {
    zxio_signals_t zxio_signals = ZXIO_SIGNAL_NONE;
    if (zx_signals & static_cast<zx_signals_t>(fio::wire::FileSignal::kReadable)) {
      zxio_signals |= ZXIO_SIGNAL_READABLE;
    }
    if (zx_signals & static_cast<zx_signals_t>(fio::wire::FileSignal::kWritable)) {
      zxio_signals |= ZXIO_SIGNAL_WRITABLE;
    }
    *out_zxio_signals = zxio_signals;
  }

  zx_status_t Readv(const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                    size_t* out_actual) {
    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    if (stream_.is_valid()) {
      return MapToPosixStatus(stream_.readv(0, vector, vector_count, out_actual));
    }
    // Fallback to fuchsia.io/Readable.Read (File composes Readable).
    fidl::UnownedClientEnd<fio::Readable> readable_client(client().client_end().handle());
    return RemoteReadv(readable_client, vector, vector_count, flags, out_actual);
  }

  zx_status_t ReadvAt(zx_off_t offset, const zx_iovec_t* vector, size_t vector_count,
                      zxio_flags_t flags, size_t* out_actual) {
    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    if (stream_.is_valid()) {
      return MapToPosixStatus(stream_.readv_at(0, offset, vector, vector_count, out_actual));
    }
    // Fallback to fuchsia.io/File.ReadAt.
    return FileReadvAt(client(), offset, vector, vector_count, flags, out_actual);
  }

  zx_status_t Writev(const zx_iovec_t* vector, size_t vector_count, zxio_flags_t flags,
                     size_t* out_actual) {
    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    if (stream_.is_valid()) {
      return MapToPosixStatus(stream_.writev(0, vector, vector_count, out_actual));
    }
    // Fallback to fuchsia.io/Writable.Write (File composes Writable).
    fidl::UnownedClientEnd<fio::Writable> writable_client(client().client_end().handle());
    return RemoteWritev(writable_client, vector, vector_count, flags, out_actual);
  }

  zx_status_t WritevAt(zx_off_t offset, const zx_iovec_t* vector, size_t vector_count,
                       zxio_flags_t flags, size_t* out_actual) {
    if (flags) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    if (stream_.is_valid()) {
      return MapToPosixStatus(stream_.writev_at(0, offset, vector, vector_count, out_actual));
    }
    // Fallback to fuchsia.io/File.WriteAt.
    return FileWritevAt(client(), offset, vector, vector_count, flags, out_actual);
  }

  zx_status_t Seek(zxio_seek_origin_t start, int64_t offset, size_t* out_offset) {
    if (stream_.is_valid()) {
      return MapToPosixStatus(stream_.seek(start, offset, out_offset));
    }
    const fidl::WireResult result =
        client()->Seek(static_cast<fio::wire::SeekOrigin>(start), offset);
    if (!result.ok()) {
      return result.status();
    }
    const auto& response = result.value();
    if (response.is_error()) {
      return response.error_value();
    }
    *out_offset = response.value()->offset_from_start;
    return ZX_OK;
  }

  zx_status_t Truncate(uint64_t length) {
    const fidl::WireResult result = client()->Resize(length);
    if (!result.ok()) {
      return result.status();
    }
    const auto& response = result.value();
    if (response.is_error()) {
      return response.error_value();
    }
    return ZX_OK;
  }

  zx_status_t VmoGet(zxio_vmo_flags_t zxio_flags, zx_handle_t* out_vmo) {
    fio::wire::VmoFlags flags;
    if (zxio_flags & ZXIO_VMO_READ) {
      flags |= fio::wire::VmoFlags::kRead;
    }
    if (zxio_flags & ZXIO_VMO_WRITE) {
      flags |= fio::wire::VmoFlags::kWrite;
    }
    if (zxio_flags & ZXIO_VMO_EXECUTE) {
      flags |= fio::wire::VmoFlags::kExecute;
    }
    if (zxio_flags & ZXIO_VMO_PRIVATE_CLONE) {
      flags |= fio::wire::VmoFlags::kPrivateClone;
    }
    if (zxio_flags & ZXIO_VMO_SHARED_BUFFER) {
      flags |= fio::wire::VmoFlags::kSharedBuffer;
    }
    fidl::WireResult result = client()->GetBackingMemory(flags);
    if (!result.ok()) {
      return result.status();
    }
    const auto& response = result.value();
    if (response.is_error()) {
      return response.error_value();
    }
    zx::vmo& vmo = response.value()->vmo;
    *out_vmo = vmo.release();
    return ZX_OK;
  }

  zx_status_t Allocate(uint64_t offset, uint64_t len, zxio_allocate_mode_t mode) {
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    if (!client().is_valid()) {
      return ZX_ERR_BAD_STATE;
    }

    fio::AllocateMode fidl_mode;
    if (mode & ZXIO_ALLOCATE_KEEP_SIZE) {
      fidl_mode |= fio::AllocateMode::kKeepSize;
    } else if (mode & ZXIO_ALLOCATE_UNSHARE_RANGE) {
      fidl_mode |= fio::AllocateMode::kUnshareRange;
    } else if (mode & ZXIO_ALLOCATE_PUNCH_HOLE) {
      fidl_mode |= fio::AllocateMode::kPunchHole;
    } else if (mode & ZXIO_ALLOCATE_COLLAPSE_RANGE) {
      fidl_mode |= fio::AllocateMode::kCollapseRange;
    } else if (mode & ZXIO_ALLOCATE_ZERO_RANGE) {
      fidl_mode |= fio::AllocateMode::kZeroRange;
    } else if (mode & ZXIO_ALLOCATE_INSERT_RANGE) {
      fidl_mode |= fio::AllocateMode::kInsertRange;
    }
    const fidl::WireResult result = client()->Allocate(offset, len, fidl_mode);
    if (!result.ok()) {
      return result.status();
    }
    if (result.value().is_error()) {
      return result.value().error_value();
    }

    return ZX_OK;
#else
    return ZX_ERR_NOT_SUPPORTED;
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  }

  static const zxio_ops_t kOps;

  const zx::event event_;
  const zx::stream stream_;
};

constexpr zxio_ops_t File::kOps = ([]() {
  zxio_ops_t ops = CommonRemoteOps<File>();

  using Adaptor = Adaptor<File>;
  ops.wait_begin = Adaptor::From<&File::WaitBegin>;
  ops.wait_end = Adaptor::From<&File::WaitEnd>;
  ops.readv = Adaptor::From<&File::Readv>;
  ops.readv_at = Adaptor::From<&File::ReadvAt>;
  ops.writev = Adaptor::From<&File::Writev>;
  ops.writev_at = Adaptor::From<&File::WritevAt>;
  ops.seek = Adaptor::From<&File::Seek>;
  ops.truncate = Adaptor::From<&File::Truncate>;
  ops.vmo_get = Adaptor::From<&File::VmoGet>;
  ops.enable_verity = Adaptor::From<&File::EnableVerity>;
  ops.advisory_lock = Adaptor::From<&File::AdvisoryLock>;
  ops.link_into = Adaptor::From<&File::LinkInto>;
  ops.allocate = Adaptor::From<&File::Allocate>;

  return ops;
})();

class Symlink : public Remote<fio::Symlink> {
 public:
  Symlink(fidl::ClientEnd<fio::Symlink> client_end, std::vector<uint8_t> target)
      : Remote(std::move(client_end), kOps), target_(std::move(target)) {}

 private:
  zx_status_t ReadLink(const uint8_t** out_target, size_t* out_target_len) const {
    *out_target = target_.data();
    *out_target_len = target_.size();
    return ZX_OK;
  }

  static const zxio_ops_t kOps;
  const std::vector<uint8_t> target_;
};

constexpr zxio_ops_t Symlink::kOps = ([]() {
  zxio_ops_t ops = CommonRemoteOps<Symlink>();

  using Adaptor = Adaptor<Symlink>;
  ops.read_link = Adaptor::From<&Symlink::ReadLink>;
  ops.link_into = Adaptor::From<&Symlink::LinkInto>;
  return ops;
})();

}  // namespace

zx_status_t zxio_dir_init(zxio_storage_t* storage, fidl::ClientEnd<fio::Directory> client) {
  new (storage) Directory(std::move(client));
  return ZX_OK;
}

zx_status_t zxio_file_init(zxio_storage_t* storage, zx::event event, zx::stream stream,
                           fidl::ClientEnd<fuchsia_io::File> client) {
  new (storage) File(std::move(client), std::move(event), std::move(stream));
  return ZX_OK;
}

zx_status_t zxio_node_init(zxio_storage_t* storage, fidl::ClientEnd<fio::Node> client) {
  new (storage) Node(std::move(client));
  return ZX_OK;
}

zx_status_t zxio_symlink_init(zxio_storage_t* storage, fidl::ClientEnd<fio::Symlink> client,
                              std::vector<uint8_t> target) {
  new (storage) Symlink(std::move(client), std::move(target));
  return ZX_OK;
}

zx_status_t zxio_attr_from_wire(const fio::wire::NodeAttributes2& in, zxio_node_attributes_t* out) {
  out->has = {};

  if (in.immutable_attributes.has_protocols()) {
    out->protocols = static_cast<uint64_t>(in.immutable_attributes.protocols());
    out->has.protocols = true;
  }

  if (in.immutable_attributes.has_abilities()) {
    out->abilities = static_cast<uint64_t>(in.immutable_attributes.abilities());
    out->has.abilities = true;
  }

  if (in.immutable_attributes.has_id()) {
    out->id = in.immutable_attributes.id();
    out->has.id = true;
  }

  if (in.immutable_attributes.has_content_size()) {
    out->content_size = in.immutable_attributes.content_size();
    out->has.content_size = true;
  }

  if (in.immutable_attributes.has_storage_size()) {
    out->storage_size = in.immutable_attributes.storage_size();
    out->has.storage_size = true;
  }

  if (in.immutable_attributes.has_link_count()) {
    out->link_count = in.immutable_attributes.link_count();
    out->has.link_count = true;
  }

  if (in.mutable_attributes.has_creation_time()) {
    out->creation_time = in.mutable_attributes.creation_time();
    out->has.creation_time = true;
  }

  if (in.mutable_attributes.has_modification_time()) {
    out->modification_time = in.mutable_attributes.modification_time();
    out->has.modification_time = true;
  }
  if (in.mutable_attributes.has_access_time()) {
    out->access_time = in.mutable_attributes.access_time();
    out->has.access_time = true;
  }

  if (in.mutable_attributes.has_mode()) {
    out->mode = in.mutable_attributes.mode();
    out->has.mode = true;
  }

  if (in.mutable_attributes.has_uid()) {
    out->uid = in.mutable_attributes.uid();
    out->has.uid = true;
  }

  if (in.mutable_attributes.has_gid()) {
    out->gid = in.mutable_attributes.gid();
    out->has.gid = true;
  }

  if (in.mutable_attributes.has_rdev()) {
    out->rdev = in.mutable_attributes.rdev();
    out->has.rdev = true;
  }
  if (in.immutable_attributes.has_change_time()) {
    out->change_time = in.immutable_attributes.change_time();
    out->has.change_time = true;
  }

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (in.mutable_attributes.has_casefold()) {
    out->casefold = in.mutable_attributes.casefold();
    out->has.casefold = true;
  }

  if (in.immutable_attributes.has_options()) {
    zxio_verification_options_t out_options{};
    fio::wire::VerificationOptions in_options = in.immutable_attributes.options();
    size_t salt_size = in_options.salt().size();
    if (salt_size > ZXIO_MAX_SALT_SIZE)
      return ZX_ERR_INVALID_ARGS;
    out_options.salt_size = salt_size;
    memcpy(out_options.salt, in_options.salt().data(), salt_size);
    out_options.hash_alg = static_cast<zxio_hash_algorithm_t>(in_options.hash_algorithm());
    out->fsverity_options = out_options;
    out->has.fsverity_options = true;
  }

  if (in.immutable_attributes.has_root_hash()) {
    if (!out->fsverity_root_hash) {
      return ZX_ERR_INVALID_ARGS;  // Caller must provide a pointer to write root hash to.
    }
    if (in.immutable_attributes.root_hash().size() > ZXIO_ROOT_HASH_LENGTH) {
      return ZX_ERR_IO_INVALID;
    }
    memcpy(out->fsverity_root_hash, in.immutable_attributes.root_hash().data(),
           in.immutable_attributes.root_hash().size());
    // Zero out the trailing end of the buffer.
    memset(&out->fsverity_root_hash[in.immutable_attributes.root_hash().size()], 0,
           ZXIO_ROOT_HASH_LENGTH - in.immutable_attributes.root_hash().size());
    out->has.fsverity_root_hash = true;
  }

  if (in.mutable_attributes.has_wrapping_key_id()) {
    memcpy(std::begin(out->wrapping_key_id), in.mutable_attributes.wrapping_key_id().begin(),
           ZXIO_WRAPPING_KEY_ID_LENGTH);
    out->has.wrapping_key_id = true;
  }

  if (in.immutable_attributes.has_verity_enabled()) {
    out->fsverity_enabled = in.immutable_attributes.verity_enabled();
    out->has.fsverity_enabled = true;
  }

  if (in.mutable_attributes.has_selinux_context()) {
    if (in.mutable_attributes.selinux_context().is_data()) {
      if (!out->selinux_context) {
        return ZX_ERR_INVALID_ARGS;
      }
      fidl::VectorView<uint8_t>& data_view = in.mutable_attributes.selinux_context().data();
      out->selinux_context_state = ZXIO_SELINUX_CONTEXT_STATE_DATA;
      static_assert(ZXIO_SELINUX_CONTEXT_MAX_ATTR_LEN < UINT16_MAX);
      out->selinux_context_length = static_cast<uint16_t>(data_view.size());
      memcpy(out->selinux_context, data_view.data(), data_view.size());
    } else if (in.mutable_attributes.selinux_context().is_use_extended_attributes()) {
      out->selinux_context_state = ZXIO_SELINUX_CONTEXT_STATE_USE_XATTRS;
    } else {
      // Returned an invalid or unknown enum.
      return ZX_ERR_IO_INVALID;
    }
    out->has.selinux_context = true;
  }
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

  return ZX_OK;
}
