// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/connection/connection.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/object_view.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <limits.h>
#include <zircon/assert.h>
#include <zircon/availability.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/debug.h"
#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fio = fuchsia_io;

static_assert(fio::wire::kOpenFlagsAllowedWithNodeReference ==
                  (fio::wire::OpenFlags::kDirectory | fio::wire::OpenFlags::kNotDirectory |
                   fio::wire::OpenFlags::kDescribe | fio::wire::OpenFlags::kNodeReference),
              "OPEN_FLAGS_ALLOWED_WITH_NODE_REFERENCE value mismatch");
static_assert(PATH_MAX == fio::wire::kMaxPathLength + 1,
              "POSIX PATH_MAX inconsistent with Fuchsia MAX_PATH_LENGTH");
static_assert(NAME_MAX == fio::wire::kMaxNameLength,
              "POSIX NAME_MAX inconsistent with Fuchsia MAX_NAME_LENGTH");

namespace fs::internal {

Connection::Connection(FuchsiaVfs* vfs, fbl::RefPtr<Vnode> vnode, fuchsia_io::Rights rights)
    : vfs_(vfs), vnode_(std::move(vnode)), rights_(rights) {
  ZX_DEBUG_ASSERT(vfs);
  ZX_DEBUG_ASSERT(vnode_);
}

Connection::~Connection() {
  // Release the token associated with this connection's vnode since the connection will be
  // releasing the vnode's reference once this function returns.
  if (auto vfs = vfs_.Upgrade(); vfs && token_) {
    vfs->TokenDiscard(std::move(token_));
  }
}

void Connection::NodeClone(fio::Flags flags, zx::channel object) const {
  FS_PRETTY_TRACE_DEBUG("[NodeClone] reopening with flags: ", flags);
  auto vfs = this->vfs();
  if (!vfs) {
    fidl::ServerEnd<fio::Node>{std::move(object)}.Close(ZX_ERR_CANCELED);
    return;
  }
  // On failure, |Vfs::Serve()| will close the channel with an epitaph.
  [[maybe_unused]] zx_status_t status = vfs->Serve(vnode(), std::move(object), flags);
#if FS_TRACE_DEBUG_ENABLED
  if (status != ZX_OK) {
    FS_PRETTY_TRACE_DEBUG("[NodeClone] serve failed: ", zx_status_get_string(status));
  }
#endif  // FS_TRACE_DEBUG_ENABLED
}

zx::result<> Connection::NodeUpdateAttributes(const VnodeAttributesUpdate& update) {
  FS_PRETTY_TRACE_DEBUG("[NodeSetAttr] our rights: ", rights(),
                        ", setting attributes: ", update.Query(),
                        ", supported attributes: ", vnode_->SupportedMutableAttributes());
  if (!(rights_ & fio::Rights::kUpdateAttributes)) {
    return zx::error(ZX_ERR_BAD_HANDLE);
  }
  // Check that the Vnode allows setting the attributes we are updating.
  if (update.Query() - vnode_->SupportedMutableAttributes()) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  return vnode_->UpdateAttributes(update);
}

zx::result<fio::wire::FilesystemInfo> Connection::NodeQueryFilesystem() const {
  auto vfs = vfs_.Upgrade();
  if (!vfs)
    return zx::error(ZX_ERR_CANCELED);
  zx::result<FilesystemInfo> info = vfs->GetFilesystemInfo();
  if (info.is_error()) {
    return info.take_error();
  }
  return zx::ok(info.value().ToFidl());
}

namespace {
// Helper function to reduce verbosity in |NodeAttributes:Build| impl below by using type deduction.
// Returns an external (non-owning) |fidl::ObjectView| to |obj|.
template <typename T>
fidl::ObjectView<T> ExternalView(T* obj) {
  return fidl::ObjectView<T>::FromExternal(obj);
}
}  // namespace

zx::result<fio::wire::NodeAttributes2*> NodeAttributeBuilder::Build(
    fuchsia_io::NodeAttributesQuery query) {
  if (attributes_.is_error()) {
    return zx::error(attributes_.error_value());
  }
  fs::VnodeAttributes* attributes = &attributes_.value();
  // Immutable attributes:
  auto immutable_builder = ImmutableAttrs::ExternalBuilder(ExternalView(&immutable_frame_));
  if (query & fio::NodeAttributesQuery::kProtocols) {
    immutable_builder.protocols(ExternalView(&protocols_));
  }
  if (query & fio::NodeAttributesQuery::kAbilities) {
    immutable_builder.abilities(ExternalView(&abilities_));
  }
  if (query & fio::NodeAttributesQuery::kContentSize && attributes->content_size) {
    immutable_builder.content_size(ExternalView(&*attributes->content_size));
  }
  if (query & fio::NodeAttributesQuery::kStorageSize && attributes->storage_size) {
    immutable_builder.storage_size(ExternalView(&*attributes->storage_size));
  }
  if (query & fio::NodeAttributesQuery::kLinkCount && attributes->link_count) {
    immutable_builder.link_count(ExternalView(&*attributes->link_count));
  }
  if (query & fio::NodeAttributesQuery::kId && attributes->id) {
    immutable_builder.id(ExternalView(&*attributes->id));
  }
  if (query & fio::NodeAttributesQuery::kChangeTime && attributes->change_time) {
    immutable_builder.change_time(ExternalView(&*attributes->change_time));
  }
  // Mutable attributes:
  auto mutable_builder = MutableAttrs::ExternalBuilder(ExternalView(&mutable_frame_));
  if (query & fio::NodeAttributesQuery::kCreationTime && attributes->creation_time) {
    mutable_builder.creation_time(ExternalView(&*attributes->creation_time));
  }
  if (query & fio::NodeAttributesQuery::kModificationTime && attributes->modification_time) {
    mutable_builder.modification_time(ExternalView(&*attributes->modification_time));
  }
  if (query & fio::NodeAttributesQuery::kMode && attributes->mode) {
    mutable_builder.mode(*attributes->mode);
  }
  if (query & fio::NodeAttributesQuery::kUid && attributes->uid) {
    mutable_builder.uid(*attributes->uid);
  }
  if (query & fio::NodeAttributesQuery::kGid && attributes->gid) {
    mutable_builder.gid(*attributes->gid);
  }
  if (query & fio::NodeAttributesQuery::kRdev && attributes->rdev) {
    mutable_builder.rdev(ExternalView(&*attributes->rdev));
  }
  if (query & fio::NodeAttributesQuery::kAccessTime && attributes->access_time) {
    mutable_builder.access_time(ExternalView(&*attributes->access_time));
  }

  // Build the wire table, which is now valid as long as this object remains in scope.
  wire_table_ = NodeAttributes2{
      .mutable_attributes = mutable_builder.Build(),
      .immutable_attributes = immutable_builder.Build(),
  };
  return zx::ok(&wire_table_);
}

}  // namespace fs::internal
