// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/connection/directory_connection.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/object_view.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/file-lock/file-lock.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>
#include <lib/zx/event.h>
#include <lib/zx/handle.h>
#include <lib/zx/result.h>
#include <stdint.h>
#include <stdlib.h>
#include <zircon/assert.h>
#include <zircon/availability.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <memory>
#include <optional>
#include <string_view>
#include <tuple>
#include <type_traits>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/connection/advisory_lock.h"
#include "src/storage/lib/vfs/cpp/connection/connection.h"
#include "src/storage/lib/vfs/cpp/connection/directory_connection.h"
#include "src/storage/lib/vfs/cpp/debug.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fio = fuchsia_io;

namespace fs::internal {

namespace {

constexpr zx::result<std::tuple<fio::Rights, fio::Rights>> ValidateRequestRights(
    fio::Flags flags, fio::Rights parent_rights) {
  // If the request will create a new object, ensure this connection allows it.
  if (internal::CreationModeFromFidl(flags) != CreationMode::kNever &&
      !(parent_rights & fio::Rights::kModifyDirectory)) {
    return zx::error(ZX_ERR_ACCESS_DENIED);
  }
  // If the request attempts to truncate a file, it must also request write permissions.
  if ((flags & fio::Flags::kFileTruncate) && !(flags & fio::Flags::kPermWriteBytes)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  fio::Rights requested_rights = internal::FlagsToRights(flags);
  // If the requested rights exceed those of the parent connection, reject the request.
  if (requested_rights - parent_rights) {
    return zx::error(ZX_ERR_ACCESS_DENIED);
  }
  fio::Rights optional_rights;
  if (flags & fio::Flags::kPermInheritWrite) {
    optional_rights |= fio::kInheritedWritePermissions;
  }
  if (flags & fio::Flags::kPermInheritExecute) {
    optional_rights |= fio::Rights::kExecute;
  }
  // *CAUTION*: The resulting connection rights must *never* exceed those of |parent_rights|.
  return zx::ok(std::tuple{requested_rights & parent_rights, optional_rights & parent_rights});
}

#if FUCHSIA_API_LEVEL_AT_LEAST(31)
void ForwardRequestToRemote(fio::wire::OpenableOpenRequest* request, Vfs::OpenResult open_result,
                            fio::Rights parent_rights) {
#else
void ForwardRequestToRemote(fio::wire::DirectoryOpenRequest* request, Vfs::OpenResult open_result,
                            fio::Rights parent_rights) {
#endif
  ZX_DEBUG_ASSERT(open_result.vnode()->IsRemote());
  // Update the request path to point only to the remaining segment.
  request->path = fidl::StringView::FromExternal(open_result.path());
  // Remove optional rights from the request that the parent lacks.
  if ((fio::kInheritedWritePermissions)-parent_rights) {
    request->flags &= ~fio::Flags::kPermInheritWrite;
  }
  if (fio::Rights::kExecute - parent_rights) {
    request->flags &= ~fio::Flags::kPermInheritExecute;
  }
  open_result.TakeVnode()->OpenRemote(std::move(*request));
}

}  // namespace

DirectoryConnection::DirectoryConnection(fs::FuchsiaVfs* vfs, fbl::RefPtr<fs::Vnode> vnode,
                                         fuchsia_io::Rights rights, zx_koid_t koid)
    : Connection(vfs, std::move(vnode), rights), koid_(koid) {
  // Ensure the VFS does not create connections that have privileges which cannot be used.
  ZX_DEBUG_ASSERT(internal::DownscopeRights(rights, VnodeProtocol::kDirectory) == rights);
}

void DirectoryConnection::BindImpl(zx::channel channel, OnUnbound on_unbound) {
  ZX_DEBUG_ASSERT(!binding_);
  binding_.emplace(fidl::BindServer(
      vfs()->dispatcher(), fidl::ServerEnd<fuchsia_io::Directory>{std::move(channel)}, this,
      [on_unbound = std::move(on_unbound)](DirectoryConnection* self, fidl::UnbindInfo,
                                           fidl::ServerEnd<fuchsia_io::Directory>) {
        [[maybe_unused]] zx::result<> result = self->CloseVnode(self->koid_);
        on_unbound(self);
      }));
}

void DirectoryConnection::Unbind() {
  // NOTE: This needs to be thread-safe!
  if (binding_)
    binding_->Unbind();
}

#if FUCHSIA_API_LEVEL_LESS_THAN(29) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
void DirectoryConnection::DeprecatedClone(DeprecatedCloneRequestView request,
                                          DeprecatedCloneCompleter::Sync& completer) {
  fidl::ServerEnd<fio::Node> server_end{std::move(request->object)};
  if (request->flags & fio::wire::OpenFlags::kDescribe) {
    // Ignore errors since there is nothing we can do if this fails.
    [[maybe_unused]] auto result =
        fidl::WireSendEvent(server_end)->OnOpen(ZX_ERR_NOT_SUPPORTED, {});
  }
  server_end.Close(ZX_ERR_NOT_SUPPORTED);
}
#endif

void DirectoryConnection::Clone(CloneRequestView request, CloneCompleter::Sync& completer) {
  Connection::NodeClone(fio::Flags::kProtocolDirectory | fs::internal::RightsToFlags(rights()),
                        request->request.TakeChannel());
}

void DirectoryConnection::Close(CloseCompleter::Sync& completer) {
  completer.Reply(CloseVnode(koid_));
  Unbind();
}

void DirectoryConnection::Query(QueryCompleter::Sync& completer) {
  const std::string_view kProtocolName = fidl::DiscoverableProtocolName<fio::Directory>;
  // TODO(https://fxbug.dev/42052765): avoid the const cast.
  uint8_t* data = reinterpret_cast<uint8_t*>(const_cast<char*>(kProtocolName.data()));
  completer.Reply(fidl::VectorView<uint8_t>::FromExternal(data, kProtocolName.size()));
}

void DirectoryConnection::Sync(SyncCompleter::Sync& completer) {
  vnode()->Sync([completer = completer.ToAsync()](zx_status_t sync_status) mutable {
    if (sync_status != ZX_OK) {
      completer.ReplyError(sync_status);
    } else {
      completer.ReplySuccess();
    }
  });
}

#if FUCHSIA_API_LEVEL_AT_LEAST(28)
void DirectoryConnection::DeprecatedGetAttr(DeprecatedGetAttrCompleter::Sync& completer) {
#else
void DirectoryConnection::GetAttr(GetAttrCompleter::Sync& completer) {
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(28)
  if (!(rights() & fio::Rights::kGetAttributes)) {
    completer.Reply(ZX_ERR_BAD_HANDLE, {});
    return;
  }
  zx::result attrs = vnode()->GetAttributes();
  if (attrs.is_ok()) {
    completer.Reply(ZX_OK, attrs->ToIoV1NodeAttributes(*vnode()));
  } else {
    completer.Reply(attrs.error_value(), {});
  }
}

#if FUCHSIA_API_LEVEL_AT_LEAST(28)
void DirectoryConnection::DeprecatedSetAttr(DeprecatedSetAttrRequestView request,
                                            DeprecatedSetAttrCompleter::Sync& completer) {
#else
void DirectoryConnection::SetAttr(SetAttrRequestView request, SetAttrCompleter::Sync& completer) {
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(28)
  VnodeAttributesUpdate update =
      VnodeAttributesUpdate::FromIo1(request->attributes, request->flags);
  completer.Reply(Connection::NodeUpdateAttributes(update).status_value());
}

void DirectoryConnection::GetAttributes(fio::wire::NodeGetAttributesRequest* request,
                                        GetAttributesCompleter::Sync& completer) {
  if (!(rights() & fuchsia_io::Rights::kGetAttributes)) {
    completer.ReplyError(ZX_ERR_ACCESS_DENIED);
    return;
  }
  internal::NodeAttributeBuilder builder(vnode());
  completer.Reply(builder.Build(request->query));
}

void DirectoryConnection::UpdateAttributes(fio::wire::MutableNodeAttributes* request,
                                           UpdateAttributesCompleter::Sync& completer) {
  VnodeAttributesUpdate update = VnodeAttributesUpdate::FromIo2(*request);
  completer.Reply(Connection::NodeUpdateAttributes(update));
}

void DirectoryConnection::GetFlags(GetFlagsCompleter::Sync& completer) {
  completer.ReplySuccess(fio::Flags::kProtocolDirectory | RightsToFlags(rights()));
}
void DirectoryConnection::SetFlags(SetFlagsRequestView, SetFlagsCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void DirectoryConnection::DeprecatedGetFlags(DeprecatedGetFlagsCompleter::Sync& completer) {
  completer.Reply(ZX_OK, RightsToOpenFlags(rights()));
}

void DirectoryConnection::DeprecatedSetFlags(DeprecatedSetFlagsRequestView,
                                             DeprecatedSetFlagsCompleter::Sync& completer) {
  completer.Reply(ZX_ERR_NOT_SUPPORTED);
}

#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
void DirectoryConnection::DeprecatedOpen(DeprecatedOpenRequestView request,
                                         DeprecatedOpenCompleter::Sync& completer) {
  zx_status_t status = [&]() -> zx_status_t {
    std::string_view path(request->path.data(), request->path.size());
    fio::OpenFlags flags = request->flags;
    if (path.empty() || ((path == "." || path == "/") && (flags & fio::OpenFlags::kNotDirectory))) {
      return ZX_ERR_INVALID_ARGS;
    }
    if (path != "." && path != "/" && !(rights() & fio::Rights::kTraverse)) {
      return ZX_ERR_ACCESS_DENIED;
    }
    if (path.back() == '/') {
      flags |= fio::OpenFlags::kDirectory;
    }
    zx::result open_options = DeprecatedOptions::FromOpen1Flags(flags);
    if (open_options.is_error()) {
      FS_PRETTY_TRACE_DEBUG("[DirectoryDeprecatedOpen] invalid flags: ", request->flags,
                            ", path: ", request->path);
      return open_options.error_value();
    }
    FS_PRETTY_TRACE_DEBUG("[DirectoryDeprecatedOpen] our rights ", rights(),
                          ", incoming options: ", *open_options, ", path: ", path);
    // The POSIX compatibility flags allow the child directory connection to inherit the writable
    // and executable rights.  If there exists a directory without the corresponding right along
    // the Open() chain, we remove that POSIX flag preventing it from being inherited down the line
    // (this applies both for local and remote mount points, as the latter may be served using
    // a connection with vastly greater rights).
    if (!(rights() & fio::Rights::kWriteBytes)) {
      open_options->flags &= ~fio::OpenFlags::kPosixWritable;
    }
    if (!(rights() & fio::Rights::kExecute)) {
      open_options->flags &= ~fio::OpenFlags::kPosixExecutable;
    }
    // Return ACCESS_DENIED if the client asked for a right the parent connection doesn't have.
    // TODO(https://fxbug.dev/346585458): GET_ATTRIBUTES was unprivileged in io1 so we cannot
    // enforce that correctly here.
    if ((open_options->rights & ~fio::Rights::kGetAttributes) - rights()) {
      return ZX_ERR_ACCESS_DENIED;
    }
    // If the request attempts to create a file, ensure this connection allows it.
    if (internal::CreationModeFromFidl(open_options->flags) != CreationMode::kNever &&
        !(rights() & fio::Rights::kModifyDirectory)) {
      return ZX_ERR_ACCESS_DENIED;
    }

    auto fs = vfs();
    if (!fs)
      return ZX_ERR_CANCELED;

    return fs->DeprecatedOpen(vnode(), path, *open_options, rights())
        .visit([&](auto&& result) -> zx_status_t {
          using ResultT = std::decay_t<decltype(result)>;
          if constexpr (std::is_same_v<ResultT, Vfs::DeprecatedOpenResult::Error>) {
            return result;
          } else if constexpr (std::is_same_v<ResultT, Vfs::DeprecatedOpenResult::Remote>) {
            result.vnode->DeprecatedOpenRemote(open_options->ToIoV1Flags(), {},
                                               fidl::StringView::FromExternal(result.path),
                                               std::move(request->object));
            return ZX_OK;
          } else if constexpr (std::is_same_v<ResultT, Vfs::DeprecatedOpenResult::Ok>) {
            return fs->ServeDeprecated(result.vnode, request->object.TakeChannel(), result.options);
          }
        });
  }();

  // On any errors, if the channel wasn't consumed, we send an OnOpen event if required, and try
  // closing the channel with an epitaph describing the error.
  if (status != ZX_OK) {
    FS_PRETTY_TRACE_DEBUG("[DirectoryDeprecatedOpen] error: ", zx_status_get_string(status));
    if (request->object.is_valid()) {
      if (request->flags & fio::wire::OpenFlags::kDescribe) {
        // Ignore errors since there is nothing we can do if this fails.
        [[maybe_unused]] auto result = fidl::WireSendEvent(request->object)->OnOpen(status, {});
      }
      request->object.Close(status);
    }
  }
}
#endif

void DirectoryConnection::Open(OpenRequestView request, OpenCompleter::Sync& completer) {
  FS_PRETTY_TRACE_DEBUG("[DirectoryConnection::Open] our rights: ", rights(), ", path: '",
                        request->path, "', flags: ", request->flags, "options: ", request->options);
  // Attempt to open/create the target vnode, and serve a connection to it.
  zx::result handled = [&]() -> zx::result<> {
    std::string_view path(request->path.data(), request->path.size());
    if (!path.empty() && path != "." && path != "/" && !(this->rights() & fio::Rights::kTraverse)) {
      return zx::error(ZX_ERR_ACCESS_DENIED);
    }
    // If the request attempts to query attributes, this connection must allow it.
    if (request->options.has_attributes() && request->options.attributes() &&
        !(this->rights() & fio::Rights::kGetAttributes)) {
      return zx::error(ZX_ERR_ACCESS_DENIED);
    }
    // Calculate the set of rights the connection should have.
    zx::result resulting_rights = ValidateRequestRights(request->flags, this->rights());
    if (resulting_rights.is_error()) {
      return resulting_rights.take_error();
    }
    auto [rights, optional_rights] = *resulting_rights;
    // The rights for the new connection must never exceed those of this connection.
    ZX_DEBUG_ASSERT((rights - this->rights()) == fio::Rights());
    ZX_DEBUG_ASSERT((optional_rights - this->rights()) == fio::Rights());
    auto fs = vfs();
    if (!fs)
      return zx::error(ZX_ERR_CANCELED);
    // Handle opening (or creating) the vnode.
    zx::result open_result = fs->Open(vnode(), path, request->flags, &request->options, rights);
    if (open_result.is_error()) {
      FS_PRETTY_TRACE_DEBUG("[DirectoryConnection::Open] Vfs::Open failed: ",
                            open_result.status_string());
      return open_result.take_error();
    }
    // If we encountered a remote node, forward the remainder of the request there.
    if (open_result->vnode()->IsRemote()) {
      ForwardRequestToRemote(request, *std::move(open_result), /*parent_rights*/ this->rights());
      return zx::ok();
    }
    // Expand optional rights if we negotiated the directory protocol.
    if (open_result->protocol() == fs::VnodeProtocol::kDirectory && optional_rights) {
      if (fs->IsReadonly()) {
        // Ensure that we don't grant the ability to modify the filesystem if it's read only.
        optional_rights &= ~fs::kAllMutableIo2Rights;
      }
      rights |= optional_rights;
    }
    // If the target negotiated to a service, ensure this directory connection permits connecting.
    if (open_result->protocol() == fs::VnodeProtocol::kService &&
        !(this->rights() & fio::Rights::kConnect)) {
      return zx::error(ZX_ERR_ACCESS_DENIED);
    }
    // Serve a new connection to the vnode.
    return fs->ServeResult(*std::move(open_result), rights, request->object, request->flags,
                           request->options);
  }();

  // On any errors above, the object request channel should remain usable, so that we can close it
  // with the corresponding error epitaph.
  if (handled.is_error()) {
    FS_PRETTY_TRACE_DEBUG("[DirectoryConnection::Open] Error: ", handled.status_string());
    if (request->object.is_valid()) {
      fidl::ServerEnd<fio::Node>{std::move(request->object)}.Close(handled.error_value());
    }
    return;
  }
}

void DirectoryConnection::Unlink(UnlinkRequestView request, UnlinkCompleter::Sync& completer) {
  FS_PRETTY_TRACE_DEBUG("[DirectoryUnlink] our rights: ", rights(), ", name: ", request->name);
  // TODO(https://fxbug.dev/346585458): This operation should require ENUMERATE and MODIFY_DIRECTORY
  // rights, instead of WRITE_BYTES.
  if (!(rights() & fuchsia_io::Rights::kWriteBytes)) {
    completer.ReplyError(ZX_ERR_BAD_HANDLE);
    return;
  }
  std::string_view name_str(request->name.data(), request->name.size());
  if (!IsValidName(name_str)) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto fs = vfs();
  zx_status_t status =
      fs ? fs->Unlink(vnode(), name_str,
                      request->options.has_flags() &&
                          static_cast<bool>((request->options.flags() &
                                             fuchsia_io::wire::UnlinkFlags::kMustBeDirectory)))
         : ZX_ERR_CANCELED;
  if (status == ZX_OK) {
    completer.ReplySuccess();
  } else {
    completer.ReplyError(status);
  }
}

void DirectoryConnection::ReadDirents(ReadDirentsRequestView request,
                                      ReadDirentsCompleter::Sync& completer) {
  FS_PRETTY_TRACE_DEBUG("[DirectoryReadDirents] our rights: ", rights());
  if (!(rights() & fuchsia_io::Rights::kEnumerate)) {
    completer.Reply(ZX_ERR_ACCESS_DENIED, fidl::VectorView<uint8_t>());
    return;
  }
  if (request->max_bytes > fio::wire::kMaxBuf) {
    completer.Reply(ZX_ERR_BAD_HANDLE, fidl::VectorView<uint8_t>());
    return;
  }
  auto data = std::make_unique<uint8_t[]>(request->max_bytes);
  size_t actual = 0;
  auto fs = vfs();
  zx_status_t status =
      fs ? fs->Readdir(vnode().get(), &dircookie_, data.get(), request->max_bytes, &actual)
         : ZX_ERR_CANCELED;
  completer.Reply(status, fidl::VectorView<uint8_t>::FromExternal(data.get(), actual));
}

void DirectoryConnection::Rewind(RewindCompleter::Sync& completer) {
  FS_PRETTY_TRACE_DEBUG("[DirectoryRewind] our rights: ", rights());
  if (!(rights() & fuchsia_io::Rights::kEnumerate)) {
    completer.Reply(ZX_ERR_ACCESS_DENIED);
    return;
  }
  dircookie_ = VdirCookie();
  completer.Reply(ZX_OK);
}

void DirectoryConnection::GetToken(GetTokenCompleter::Sync& completer) {
  FS_PRETTY_TRACE_DEBUG("[DirectoryGetToken] our rights: ", rights());
  // GetToken exists to support linking, so we must make sure the connection has the permission to
  // modify the directory.
  if (!(rights() & fuchsia_io::Rights::kModifyDirectory)) {
    completer.Reply(ZX_ERR_BAD_HANDLE, zx::handle());
    return;
  }
  zx::event returned_token;
  auto fs = vfs();
  zx_status_t status = fs ? fs->VnodeToToken(vnode(), &token(), &returned_token) : ZX_ERR_CANCELED;
  completer.Reply(status, std::move(returned_token));
}

void DirectoryConnection::Rename(RenameRequestView request, RenameCompleter::Sync& completer) {
  FS_PRETTY_TRACE_DEBUG("[DirectoryRename] our rights: ", rights(), ", src: ", request->src,
                        ", dst: ", request->dst);
  if (request->src.empty() || request->dst.empty()) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  if (!(rights() & fuchsia_io::Rights::kModifyDirectory)) {
    completer.ReplyError(ZX_ERR_ACCESS_DENIED);
    return;
  }
  auto fs = vfs();
  zx_status_t status =
      fs ? fs->Rename(std::move(request->dst_parent_token), vnode(),
                      std::string_view(request->src.data(), request->src.size()),
                      std::string_view(request->dst.data(), request->dst.size()), rights())
         : ZX_ERR_CANCELED;
  if (status == ZX_OK) {
    completer.ReplySuccess();
  } else {
    completer.ReplyError(status);
  }
}

void DirectoryConnection::Link(LinkRequestView request, LinkCompleter::Sync& completer) {
  FS_PRETTY_TRACE_DEBUG("[DirectoryLink] our rights: ", rights(), ", src: ", request->src,
                        ", dst: ", request->dst);
  // |fuchsia.io/Directory.Rename| only specified the token to be a generic handle; casting it here.
  zx::event token(request->dst_parent_token.release());
  if (request->src.empty() || request->dst.empty()) {
    completer.Reply(ZX_ERR_INVALID_ARGS);
    return;
  }

  // To avoid rights escalation, we must make sure that the connection to the source directory has
  // the maximal set of file rights.  We do not check for EXECUTE because mutable filesystems that
  // support link don't currently support EXECUTE rights.  The destination rights are verified by
  // virtue of the fact that it is not possible to get a token without the MODIFY_DIRECTORY right.
  if ((rights() & fuchsia_io::kRwStarDir) != fuchsia_io::kRwStarDir) {
    completer.Reply(ZX_ERR_BAD_HANDLE);
    return;
  }
  auto fs = vfs();
  zx_status_t status = fs ? fs->Link(std::move(token), vnode(),
                                     std::string_view(request->src.data(), request->src.size()),
                                     std::string_view(request->dst.data(), request->dst.size()))
                          : ZX_ERR_CANCELED;
  completer.Reply(status);
}

void DirectoryConnection::Watch(WatchRequestView request, WatchCompleter::Sync& completer) {
  FS_PRETTY_TRACE_DEBUG("[DirectoryWatch] our rights: ", rights());
  if (!(rights() & fuchsia_io::Rights::kEnumerate)) {
    completer.Reply(ZX_ERR_ACCESS_DENIED);
    return;
  }
  auto fs = vfs();
  zx_status_t status =
      fs ? vnode()->WatchDir(fs.get(), request->mask, request->options, std::move(request->watcher))
         : ZX_ERR_CANCELED;
  completer.Reply(status);
}

void DirectoryConnection::QueryFilesystem(QueryFilesystemCompleter::Sync& completer) {
  FS_PRETTY_TRACE_DEBUG("[DirectoryQueryFilesystem] our rights: ", rights());

  zx::result result = Connection::NodeQueryFilesystem();
  completer.Reply(result.status_value(),
                  result.is_ok() ? fidl::ObjectView<fuchsia_io::wire::FilesystemInfo>::FromExternal(
                                       &result.value())
                                 : nullptr);
}

void DirectoryConnection::AdvisoryLock(AdvisoryLockRequestView request,
                                       AdvisoryLockCompleter::Sync& completer) {
  // advisory_lock replies to the completer
  auto async_completer = completer.ToAsync();
  fit::callback<void(zx_status_t)> callback = file_lock::lock_completer_t(
      [lock_completer = std::move(async_completer)](zx_status_t status) mutable {
        lock_completer.ReplyError(status);
      });

  advisory_lock(koid_, vnode(), false, request->request, std::move(callback));
}

zx::result<> DirectoryConnection::WithRepresentation(
    fit::callback<zx::result<>(fuchsia_io::wire::Representation)> handler,
    std::optional<fuchsia_io::NodeAttributesQuery> query) const {
  using DirectoryRepresentation = fio::wire::DirectoryInfo;
  fidl::WireTableFrame<DirectoryRepresentation> representation_frame;
  auto builder = DirectoryRepresentation::ExternalBuilder(
      fidl::ObjectView<fidl::WireTableFrame<DirectoryRepresentation>>::FromExternal(
          &representation_frame));
  std::optional<NodeAttributeBuilder> attributes_builder;
  if (query) {
    attributes_builder.emplace(vnode());
    zx::result<fio::wire::NodeAttributes2*> attributes = attributes_builder->Build(*query);
    if (attributes.is_error()) {
      return attributes.take_error();
    }
    builder.attributes(fidl::ObjectView<fio::wire::NodeAttributes2>::FromExternal(*attributes));
  }
  auto representation = builder.Build();
  return handler(fuchsia_io::wire::Representation::WithDirectory(
      fidl::ObjectView<DirectoryRepresentation>::FromExternal(&representation)));
}

#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
zx_status_t DirectoryConnection::WithNodeInfoDeprecated(
    fit::callback<zx_status_t(fuchsia_io::wire::NodeInfoDeprecated)> handler) const {
  return handler(fuchsia_io::wire::NodeInfoDeprecated::WithDirectory({}));
}
#endif

}  // namespace fs::internal
