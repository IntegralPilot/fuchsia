// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/sync/completion.h>
#include <lib/zx/channel.h>
#include <lib/zx/event.h>
#include <lib/zx/object.h>
#include <lib/zx/result.h>
#include <lib/zx/stream.h>
#include <zircon/assert.h>
#include <zircon/availability.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>
#include <mutex>
#include <optional>
#include <string_view>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/connection/connection.h"
#include "src/storage/lib/vfs/cpp/connection/directory_connection.h"
#include "src/storage/lib/vfs/cpp/connection/node_connection.h"
#include "src/storage/lib/vfs/cpp/connection/remote_file_connection.h"
#include "src/storage/lib/vfs/cpp/connection/stream_file_connection.h"
#include "src/storage/lib/vfs/cpp/debug.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fio = fuchsia_io;

namespace fs {
namespace {

zx::result<zx_koid_t> GetObjectKoid(const zx::object_base& object) {
  zx_info_handle_basic_t info = {};
  if (zx_status_t status =
          object.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(info.koid);
}

uint32_t ToStreamOptions(fuchsia_io::Rights rights, bool append) {
  uint32_t stream_options = 0u;
  if (rights & fuchsia_io::Rights::kReadBytes) {
    stream_options |= ZX_STREAM_MODE_READ;
  }
  if (rights & fuchsia_io::Rights::kWriteBytes) {
    stream_options |= ZX_STREAM_MODE_WRITE;
  }
  if (append) {
    stream_options |= ZX_STREAM_MODE_APPEND;
  }
  return stream_options;
}

}  // namespace

void FilesystemInfo::SetFsId(const zx::event& event) {
  zx::result koid = GetObjectKoid(event);
  fs_id = koid.is_ok() ? *koid : ZX_KOID_INVALID;
}

fuchsia_io::wire::FilesystemInfo FilesystemInfo::ToFidl() const {
  fuchsia_io::wire::FilesystemInfo out = {};

  out.total_bytes = total_bytes;
  out.used_bytes = used_bytes;
  out.total_nodes = total_nodes;
  out.used_nodes = used_nodes;
  out.free_shared_pool_bytes = free_shared_pool_bytes;
  out.fs_id = fs_id;
  out.block_size = block_size;
  out.max_filename_size = max_filename_size;
  out.fs_type = static_cast<uint32_t>(fs_type);

  ZX_DEBUG_ASSERT(name.size() < fuchsia_io::wire::kMaxFsNameBuffer);
  out.name[name.copy(reinterpret_cast<char*>(out.name.data()),
                     fuchsia_io::wire::kMaxFsNameBuffer - 1)] = '\0';

  return out;
}

FuchsiaVfs::SharedPtr& FuchsiaVfs::SharedPtr::operator=(const FuchsiaVfs::SharedPtr& other) {
  if (this == &other)
    return *this;
  Reset();
  vfs_ = other.vfs_;
  if (vfs_)
    vfs_->ref_->strong_count.fetch_add(1);
  return *this;
}

void FuchsiaVfs::SharedPtr::Reset() {
  if (vfs_) {
    Ref* ref = vfs_->ref_;
    if (ref->strong_count.fetch_sub(1) == 1) {
      // NOTE: After this, vfs_ can be destroyed.
      sync_completion_signal(&vfs_->done_);

      // And now drop the implicit weak reference.
      WeakPtr weak(ref);
    }
    vfs_ = nullptr;
  }
}

FuchsiaVfs::SharedPtr FuchsiaVfs::WeakPtr::Upgrade() const {
  int current = ref_->strong_count.load();
  while (current > 0) {
    if (ref_->strong_count.compare_exchange_weak(current, current + 1)) {
      return SharedPtr(ref_->vfs);
    }
  }
  return SharedPtr(nullptr);
}

FuchsiaVfs::FuchsiaVfs(async_dispatcher_t* dispatcher)
    // We start with one strong count and one weak count.  The strong count is returned in
    // `WillDestroy` and the weak count is returned when the last strong count is returned.
    : dispatcher_(dispatcher), ref_(new Ref{.strong_count = 1, .weak_count = 1, .vfs = this}) {}

FuchsiaVfs::~FuchsiaVfs() {
  if (!is_terminating_)
    WillDestroy();
}

void FuchsiaVfs::SetDispatcher(async_dispatcher_t* dispatcher) {
  ZX_ASSERT_MSG(!dispatcher_,
                "FuchsiaVfs::SetDispatcher maybe only be called when dispatcher_ is not set.");
  dispatcher_ = dispatcher;
}

zx_status_t FuchsiaVfs::Unlink(fbl::RefPtr<Vnode> vndir, std::string_view name, bool must_be_dir) {
  if (zx_status_t s = Vfs::Unlink(vndir, name, must_be_dir); s != ZX_OK)
    return s;

  vndir->Notify(name, fio::wire::WatchEvent::kRemoved);
  return ZX_OK;
}

void FuchsiaVfs::TokenDiscard(zx::event ios_token) {
  std::lock_guard lock(vfs_lock_);
  if (ios_token) {
    // The token is cleared here to prevent the following race condition:
    // 1) Open
    // 2) GetToken
    // 3) Close + Release Vnode
    // 4) Use token handle to access defunct vnode (or a different vnode, if the memory for it is
    //    reallocated).
    //
    // By cleared the token cookie, any remaining handles to the event will be ignored by the
    // filesystem server.
    //
    // This is called from a destructor, so there isn't much we can do if this fails.
    zx::result koid = GetObjectKoid(ios_token);
    if (koid.is_ok()) {
      vnode_tokens_.erase(*koid);
    }
  }
}

zx_status_t FuchsiaVfs::VnodeToToken(fbl::RefPtr<Vnode> vn, zx::event* ios_token, zx::event* out) {
  std::lock_guard lock(vfs_lock_);
  if (ios_token->is_valid()) {
    // Token has already been set for this iostate
    if (zx_status_t status = ios_token->duplicate(ZX_RIGHTS_BASIC, out); status != ZX_OK) {
      return status;
    }
    return ZX_OK;
  }

  zx::event new_token;
  zx::event new_ios_token;
  if (zx_status_t status = zx::event::create(0, &new_ios_token); status != ZX_OK) {
    return status;
  }
  if (zx_status_t status = new_ios_token.duplicate(ZX_RIGHTS_BASIC, &new_token); status != ZX_OK) {
    return status;
  }
  zx::result koid = GetObjectKoid(new_ios_token);
  if (koid.is_error()) {
    return koid.error_value();
  }
  vnode_tokens_.insert(std::make_unique<VnodeToken>(*koid, std::move(vn)));
  *ios_token = std::move(new_ios_token);
  *out = std::move(new_token);
  return ZX_OK;
}

bool FuchsiaVfs::IsTokenAssociatedWithVnode(zx::event token) {
  std::lock_guard lock(vfs_lock_);
  return TokenToVnode(std::move(token), nullptr) == ZX_OK;
}

zx_status_t FuchsiaVfs::TokenToVnode(zx::event token, fbl::RefPtr<Vnode>* out) {
  zx::result koid = GetObjectKoid(token);
  if (koid.is_error()) {
    return koid.error_value();
  }
  const auto& vnode_token = vnode_tokens_.find(*koid);
  if (vnode_token == vnode_tokens_.end()) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (out) {
    *out = vnode_token->get_vnode();
  }
  return ZX_OK;
}

zx_status_t FuchsiaVfs::Rename(zx::event token, fbl::RefPtr<Vnode> oldparent,
                               std::string_view oldStr, std::string_view newStr,
                               fuchsia_io::Rights source_rights) {
  // Local filesystem
  bool old_must_be_dir;
  {
    zx::result result = TrimName(oldStr);
    if (result.is_error()) {
      return result.status_value();
    }
    old_must_be_dir = result.value();
    if (oldStr == ".") {
      return ZX_ERR_UNAVAILABLE;
    }
    if (oldStr == "..") {
      return ZX_ERR_INVALID_ARGS;
    }
  }
  bool new_must_be_dir;
  {
    zx::result result = TrimName(newStr);
    if (result.is_error()) {
      return result.status_value();
    }
    new_must_be_dir = result.value();
    if (newStr == "." || newStr == "..") {
      return ZX_ERR_INVALID_ARGS;
    }
  }

  fbl::RefPtr<fs::Vnode> newparent;
  {
    std::lock_guard lock(vfs_lock_);
    if (ReadonlyLocked()) {
      return ZX_ERR_ACCESS_DENIED;
    }
    if (zx_status_t status = TokenToVnode(std::move(token), &newparent); status != ZX_OK) {
      return status;
    }

    if (oldparent != newparent &&
        (source_rights & fuchsia_io::kRwStarDir) != fuchsia_io::kRwStarDir) {
      return ZX_ERR_ACCESS_DENIED;
    }

    if (zx_status_t status =
            oldparent->Rename(newparent, oldStr, newStr, old_must_be_dir, new_must_be_dir);
        status != ZX_OK) {
      return status;
    }
  }
  oldparent->Notify(oldStr, fio::wire::WatchEvent::kRemoved);
  newparent->Notify(newStr, fio::wire::WatchEvent::kAdded);
  return ZX_OK;
}

zx::result<FilesystemInfo> FuchsiaVfs::GetFilesystemInfo() {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t FuchsiaVfs::Link(zx::event token, fbl::RefPtr<Vnode> oldparent, std::string_view oldStr,
                             std::string_view newStr) {
  std::lock_guard lock(vfs_lock_);
  fbl::RefPtr<fs::Vnode> newparent;
  if (zx_status_t status = TokenToVnode(std::move(token), &newparent); status != ZX_OK) {
    return status;
  }
  // Local filesystem
  if (ReadonlyLocked()) {
    return ZX_ERR_ACCESS_DENIED;
  }
  {
    zx::result result = TrimName(oldStr);
    if (result.is_error()) {
      return result.status_value();
    }
    if (result.value()) {
      return ZX_ERR_NOT_DIR;
    }
    if (oldStr == ".") {
      return ZX_ERR_UNAVAILABLE;
    }
    if (oldStr == "..") {
      return ZX_ERR_INVALID_ARGS;
    }
  }
  {
    zx::result result = TrimName(newStr);
    if (result.is_error()) {
      return result.status_value();
    }
    if (result.value()) {
      return ZX_ERR_NOT_DIR;
    }
    if (newStr == "." || newStr == "..") {
      return ZX_ERR_INVALID_ARGS;
    }
  }

  // Look up the target vnode
  fbl::RefPtr<Vnode> target;
  if (zx_status_t status = oldparent->Lookup(oldStr, &target); status != ZX_OK) {
    return status;
  }
  if (zx_status_t status = newparent->Link(newStr, target); status != ZX_OK) {
    return status;
  }
  newparent->Notify(newStr, fio::wire::WatchEvent::kAdded);
  return ZX_OK;
}

zx_status_t FuchsiaVfs::ServeDeprecated(const fbl::RefPtr<Vnode>& vnode, zx::channel server_end,
                                        DeprecatedOptions options) {
  zx_status_t status = ServeDeprecatedImpl(vnode, server_end, options);
  if (status != ZX_OK) {
    FS_PRETTY_TRACE_DEBUG("[FuchsiaVfs::Serve] Error: ", zx_status_get_string(status));
    if (!(options.flags & fuchsia_io::OpenFlags::kNodeReference)) {
      vnode->Close();  // Balance open count for non-node reference connections.
    }
    // If |server_end| wasn't consumed, send an event if required and close it with an epitaph.
    if (server_end.is_valid()) {
      fidl::ServerEnd<fio::Node> node{std::move(server_end)};
#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
      if (options.flags & fio::wire::OpenFlags::kDescribe) {
        // Ignore errors since there is nothing we can do if this fails.
        [[maybe_unused]] auto result = fidl::WireSendEvent(node)->OnOpen(status, {});
      }
#else
      if (options.flags & fio::wire::OpenFlags::kDescribe) {
        // OnOpen is removed in 32.
      }
#endif
      // Close the channel with an epitaph indicating the failure mode.
      node.Close(status);
    }
  }
  return status;
}

zx_status_t FuchsiaVfs::ServeDeprecatedImpl(const fbl::RefPtr<Vnode>& vnode,
                                            zx::channel& server_end, DeprecatedOptions options) {
  if (zx::result result = vnode->DeprecatedValidateOptions(options); result.is_error()) {
    return result.error_value();
  }
  // Determine the protocol we should use to serve |vnode| based on |options|.
  VnodeProtocol protocol;
  if (options.flags & fio::OpenFlags::kNodeReference) {
    protocol = VnodeProtocol::kNode;
  } else {
    zx::result negotiated = internal::NegotiateProtocol(vnode->GetProtocols(), options.protocols());
    if (negotiated.is_error()) {
      return negotiated.error_value();
    }
    protocol = *negotiated;
  }
  // Downscope the set of rights granted to this connection to only include those which are relevant
  // for the protocol which will be served.
  options.rights = internal::DownscopeRights(options.rights, protocol);

  std::unique_ptr<internal::Connection> connection;
  switch (protocol) {
    case VnodeProtocol::kFile: {
      zx::result koid = GetObjectKoid(server_end);
      if (koid.is_error()) {
        return koid.error_value();
      }
      bool append = static_cast<bool>(options.flags & fio::OpenFlags::kAppend);
      // Truncate was handled when the Vnode was opened, only append mode applies to this
      // connection. |ToStreamOptions| should handle setting the stream to append mode.
      zx::result<zx::stream> stream = vnode->CreateStream(ToStreamOptions(options.rights, append));
      if (stream.is_ok()) {
        connection = std::make_unique<internal::StreamFileConnection>(
            this, vnode, options.rights, append, std::move(*stream), *koid);
        break;
      }
      if (stream.error_value() != ZX_ERR_NOT_SUPPORTED) {
        return stream.error_value();
      }
      connection = std::make_unique<internal::RemoteFileConnection>(this, vnode, options.rights,
                                                                    append, *koid);
      break;
    }
    case VnodeProtocol::kDirectory: {
      zx::result koid = GetObjectKoid(server_end);
      if (koid.is_error()) {
        return koid.error_value();
      }
      connection =
          std::make_unique<internal::DirectoryConnection>(this, vnode, options.rights, *koid);
      break;
    }
    case VnodeProtocol::kNode: {
      connection = std::make_unique<internal::NodeConnection>(this, vnode, options.rights);
      break;
    }
    case VnodeProtocol::kService: {
      if (options.ToIoV1Flags() & ~(fio::OpenFlags::kDescribe | fio::OpenFlags::kNotDirectory)) {
        return ZX_ERR_INVALID_ARGS;
      }
      return vnode->ConnectService(std::move(server_end));
    }
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    case VnodeProtocol::kSymlink: {
      return ZX_ERR_NOT_SUPPORTED;
    }
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  }

    // Send an |fuchsia.io/OnOpen| event if requested. At this point we know the connection is
    // either a Node connection, or a File/Directory that composes the node protocol.
#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
  if (options.flags & fuchsia_io::OpenFlags::kDescribe) {
    fidl::UnownedServerEnd<fio::Node> node{server_end.borrow()};
    zx_status_t status =
        connection->WithNodeInfoDeprecated([&node](fio::wire::NodeInfoDeprecated info) {
          return fidl::WireSendEvent(node)->OnOpen(ZX_OK, std::move(info)).status();
        });
    if (status != ZX_OK) {
      return status;
    }
  }
#else
  if (options.flags & fuchsia_io::OpenFlags::kDescribe) {
    return ZX_ERR_NOT_SUPPORTED;
  }
#endif

  return RegisterConnection(std::move(connection), server_end).status_value();
}

zx_status_t FuchsiaVfs::ServeDirectory(fbl::RefPtr<fs::Vnode> vn,
                                       fidl::ServerEnd<fuchsia_io::Directory> server_end,
                                       fuchsia_io::Rights rights) {
  return Serve(std::move(vn), server_end.TakeChannel(),
               fio::Flags::kProtocolDirectory | internal::RightsToFlags(rights));
}

zx_status_t FuchsiaVfs::Serve(fbl::RefPtr<Vnode> vn, zx::channel channel, fio::Flags flags) {
  const zx_status_t status = ServeImpl(std::move(vn), channel, flags);
  if (status != ZX_OK && channel.is_valid()) {
    fidl::ServerEnd<fio::Node>(std::move(channel)).Close(status);
  }
  return status;
}

zx_status_t FuchsiaVfs::ServeImpl(fbl::RefPtr<Vnode> vn, zx::channel& server_end,
                                  fio::Flags flags) {
  // Disallow FLAG_*_CREATE since they require a path and type of object to create.
  ZX_DEBUG_ASSERT(!(flags & (fio::Flags::kFlagMaybeCreate | fio::Flags::kFlagMustCreate)));
  // Disallow FILE_TRUNCATE since that will modify the state of |vn|. Callers can call
  // |Vnode::Truncate| on |vn| if this is required.
  ZX_DEBUG_ASSERT(!(flags & (fio::Flags::kFileTruncate)));

  // Determine if the protocols and rights specified in |flags| are compatible with |vn|.
  const zx::result protocol = internal::NegotiateProtocol(flags, vn->GetProtocols());
  if (protocol.is_error()) {
    return protocol.error_value();
  }
  const fio::Rights rights = internal::FlagsToRights(flags);
  if (!vn->ValidateRights(rights)) {
    return ZX_ERR_ACCESS_DENIED;
  }
  // Open and serve |vn|.
  zx::result opened_node = OpenResult::OpenVnode(std::move(vn), *protocol);
  if (opened_node.is_error()) {
    return opened_node.error_value();
  }
  zx::result result = ServeResult(std::move(*opened_node), rights, server_end, flags, {});
  if (result.is_error()) {
    return result.error_value();
  }
  return ZX_OK;
}

zx::result<> FuchsiaVfs::ServeResult(OpenResult open_result, fuchsia_io::Rights rights,
                                     zx::channel& object_request, fuchsia_io::Flags flags,
                                     const fuchsia_io::wire::Options& options) {
  rights = internal::DownscopeRights(rights, open_result.protocol());
  const fbl::RefPtr<fs::Vnode>& vnode = open_result.vnode();
  std::unique_ptr<internal::Connection> connection;

  switch (open_result.protocol()) {
    case fs::VnodeProtocol::kDirectory: {
      zx::result koid = GetObjectKoid(object_request);
      if (koid.is_error()) {
        return koid.take_error();
      }
      connection = std::make_unique<internal::DirectoryConnection>(this, vnode, rights, *koid);
      break;
    }
    case VnodeProtocol::kFile: {
      zx::result koid = GetObjectKoid(object_request);
      if (koid.is_error()) {
        return koid.take_error();
      }
      // |ToStreamOptions| handles setting the stream to append mode.
      const bool append = static_cast<bool>(flags & fio::Flags::kFileAppend);
      zx::result<zx::stream> stream = vnode->CreateStream(ToStreamOptions(rights, append));
      if (stream.is_ok()) {
        connection = std::make_unique<internal::StreamFileConnection>(this, vnode, rights, append,
                                                                      std::move(*stream), *koid);
        break;
      }
      if (stream.error_value() != ZX_ERR_NOT_SUPPORTED) {
        return stream.take_error();
      }
      connection =
          std::make_unique<internal::RemoteFileConnection>(this, vnode, rights, append, *koid);
      break;
    }
    case VnodeProtocol::kNode: {
      connection = std::make_unique<internal::NodeConnection>(this, vnode, rights);
      break;
    }
    case VnodeProtocol::kService: {
      if (flags & ~(fuchsia_io::Flags::kProtocolService)) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      return zx::make_result(vnode->ConnectService(std::move(object_request)));
    }
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    case VnodeProtocol::kSymlink: {
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  }
  ZX_DEBUG_ASSERT(connection != nullptr);

  // Send an OnRepresentation event if the request requires one.
  if (flags & fio::Flags::kFlagSendRepresentation) {
    std::optional<fio::NodeAttributesQuery> attribute_query = std::nullopt;
    if (options.has_attributes()) {
      attribute_query = options.attributes();
    }
    zx::result get_representation = connection->WithRepresentation(
        [&](fio::wire::Representation representation) -> zx::result<> {
          return zx::make_result(
              fidl::WireSendEvent(fidl::UnownedServerEnd<fio::Node>{object_request.borrow()})
                  ->OnRepresentation(std::move(representation))
                  .status());
        },
        attribute_query);
    if (get_representation.is_error()) {
      return get_representation.take_error();
    }
  }

  // Register the connection with the VFS. On success, the connection will be bound to the channel
  // and will start serving requests.
  zx::result result = RegisterConnection(std::move(connection), object_request);
  if (result.is_ok()) {
    // On success, the connection is responsible for closing the vnode via a fuchsia.io/Node.Close
    // request is processed, or when the client end of |object_request| is closed.
    open_result.TakeVnode();
  }
  return result;
}

}  // namespace fs
