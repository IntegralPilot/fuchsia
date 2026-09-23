// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_CONNECTION_DIRECTORY_CONNECTION_H_
#define SRC_STORAGE_LIB_VFS_CPP_CONNECTION_DIRECTORY_CONNECTION_H_

#ifndef __Fuchsia__
#error "Fuchsia-only header"
#endif

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire_messaging.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/availability.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <optional>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/connection/connection.h"
#include "src/storage/lib/vfs/cpp/vfs.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs::internal {

class DirectoryConnection final : public Connection,
                                  public fidl::WireServer<fuchsia_io::Directory> {
 public:
  // Refer to documentation for |Connection::Connection|.
  DirectoryConnection(fs::FuchsiaVfs* vfs, fbl::RefPtr<fs::Vnode> vnode, fuchsia_io::Rights rights,
                      zx_koid_t koid);

 private:
  //
  // |fs::Connection| Implementation
  //

  void BindImpl(zx::channel channel, OnUnbound on_unbound) final;
  void Unbind() final;
  zx::result<> WithRepresentation(
      fit::callback<zx::result<>(fuchsia_io::wire::Representation)> handler,
      std::optional<fuchsia_io::NodeAttributesQuery> query) const final;
#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
  zx_status_t WithNodeInfoDeprecated(
      fit::callback<zx_status_t(fuchsia_io::wire::NodeInfoDeprecated)> handler) const final;
#endif

  //
  // |fuchsia.io/Node| operations.
  //
#if FUCHSIA_API_LEVEL_LESS_THAN(29) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
  void DeprecatedClone(DeprecatedCloneRequestView request,
                       DeprecatedCloneCompleter::Sync& completer) final;
#endif
  void Clone(CloneRequestView request, CloneCompleter::Sync& completer) final;
  void Close(CloseCompleter::Sync& completer) final;
  void Query(QueryCompleter::Sync& completer) final;
  void Sync(SyncCompleter::Sync& completer) final;
#if FUCHSIA_API_LEVEL_AT_LEAST(28)
  void DeprecatedGetAttr(DeprecatedGetAttrCompleter::Sync& completer) final;
  void DeprecatedSetAttr(DeprecatedSetAttrRequestView request,
                         DeprecatedSetAttrCompleter::Sync& completer) final;
#else
  void GetAttr(GetAttrCompleter::Sync& completer) final;
  void SetAttr(SetAttrRequestView request, SetAttrCompleter::Sync& completer) final;
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(28)
  void GetFlags(GetFlagsCompleter::Sync& completer) final;
  void SetFlags(SetFlagsRequestView, SetFlagsCompleter::Sync& completer) final;
  void DeprecatedGetFlags(DeprecatedGetFlagsCompleter::Sync& completer) final;
  void DeprecatedSetFlags(DeprecatedSetFlagsRequestView,
                          DeprecatedSetFlagsCompleter::Sync& completer) final;
  void GetAttributes(fuchsia_io::wire::NodeGetAttributesRequest* request,
                     GetAttributesCompleter::Sync& completer) final;
  void UpdateAttributes(fuchsia_io::wire::MutableNodeAttributes* request,
                        UpdateAttributesCompleter::Sync& completer) final;
  void ListExtendedAttributes(ListExtendedAttributesRequestView request,
                              ListExtendedAttributesCompleter::Sync& completer) final {
    request->iterator.Close(ZX_ERR_NOT_SUPPORTED);
  }
  void GetExtendedAttribute(GetExtendedAttributeRequestView request,
                            GetExtendedAttributeCompleter::Sync& completer) final {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void SetExtendedAttribute(SetExtendedAttributeRequestView request,
                            SetExtendedAttributeCompleter::Sync& completer) final {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void RemoveExtendedAttribute(RemoveExtendedAttributeRequestView request,
                               RemoveExtendedAttributeCompleter::Sync& completer) final {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  //
  // |fuchsia.io/Directory| operations.
  //

  void Open(OpenRequestView request, OpenCompleter::Sync& completer) final;
#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
  void DeprecatedOpen(DeprecatedOpenRequestView request,
                      DeprecatedOpenCompleter::Sync& completer) final;
#endif
  void Unlink(UnlinkRequestView request, UnlinkCompleter::Sync& completer) final;
  void ReadDirents(ReadDirentsRequestView request, ReadDirentsCompleter::Sync& completer) final;
  void Rewind(RewindCompleter::Sync& completer) final;
  void GetToken(GetTokenCompleter::Sync& completer) final;
  void Rename(RenameRequestView request, RenameCompleter::Sync& completer) final;
  void Link(LinkRequestView request, LinkCompleter::Sync& completer) final;
  void Watch(WatchRequestView request, WatchCompleter::Sync& completer) final;
  void QueryFilesystem(QueryFilesystemCompleter::Sync& completer) final;
  void CreateSymlink(fuchsia_io::wire::DirectoryCreateSymlinkRequest* request,
                     CreateSymlinkCompleter::Sync& completer) final {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_io::Directory> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) final {}
  //
  // |fuchsia.io/AdvisoryLocking| operations.
  //

  void AdvisoryLock(AdvisoryLockRequestView request, AdvisoryLockCompleter::Sync& _completer) final;

  std::optional<fidl::ServerBindingRef<fuchsia_io::Directory>> binding_;

  // Directory cookie for readdir operations.
  fs::VdirCookie dircookie_;

  const zx_koid_t koid_;
};

}  // namespace fs::internal

#endif  // SRC_STORAGE_LIB_VFS_CPP_CONNECTION_DIRECTORY_CONNECTION_H_
