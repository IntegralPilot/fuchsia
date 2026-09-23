// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/connection/node_connection.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/object_view.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/availability.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstdint>
#include <optional>
#include <string_view>
#include <utility>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/connection/connection.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fio = fuchsia_io;

namespace fs::internal {

NodeConnection::NodeConnection(fs::FuchsiaVfs* vfs, fbl::RefPtr<fs::Vnode> vnode,
                               fuchsia_io::Rights rights)
    : Connection(vfs, std::move(vnode), rights) {
  // Ensure the VFS does not create connections that have privileges which cannot be used.
  ZX_DEBUG_ASSERT(internal::DownscopeRights(rights, VnodeProtocol::kNode) == rights);
}

void NodeConnection::BindImpl(zx::channel channel, OnUnbound on_unbound) {
  ZX_DEBUG_ASSERT(!binding_);
  binding_.emplace(fidl::BindServer(vfs()->dispatcher(),
                                    fidl::ServerEnd<fuchsia_io::Node>{std::move(channel)}, this,
                                    [on_unbound = std::move(on_unbound)](
                                        NodeConnection* self, fidl::UnbindInfo,
                                        fidl::ServerEnd<fuchsia_io::Node>) { on_unbound(self); }));
}

void NodeConnection::Unbind() {
  // NOTE: This needs to be thread-safe!
  if (binding_)
    binding_->Unbind();
}

#if FUCHSIA_API_LEVEL_LESS_THAN(29) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
void NodeConnection::DeprecatedClone(DeprecatedCloneRequestView request,
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

void NodeConnection::Clone(CloneRequestView request, CloneCompleter::Sync& completer) {
  Connection::NodeClone(fio::Flags::kProtocolNode | fs::internal::RightsToFlags(rights()),
                        request->request.TakeChannel());
}

void NodeConnection::Close(CloseCompleter::Sync& completer) {
  completer.Reply(zx::ok());
  Unbind();
}

void NodeConnection::Query(QueryCompleter::Sync& completer) {
  const std::string_view kProtocolName = fidl::DiscoverableProtocolName<fio::Node>;
  // TODO(https://fxbug.dev/42052765): avoid the const cast.
  uint8_t* data = reinterpret_cast<uint8_t*>(const_cast<char*>(kProtocolName.data()));
  completer.Reply(fidl::VectorView<uint8_t>::FromExternal(data, kProtocolName.size()));
}

void NodeConnection::Sync(SyncCompleter::Sync& completer) {
  completer.Reply(zx::make_result(ZX_ERR_BAD_HANDLE));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(28)
void NodeConnection::DeprecatedGetAttr(DeprecatedGetAttrCompleter::Sync& completer) {
#else
void NodeConnection::GetAttr(GetAttrCompleter::Sync& completer) {
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
void NodeConnection::DeprecatedSetAttr(DeprecatedSetAttrRequestView request,
                                       DeprecatedSetAttrCompleter::Sync& completer) {
#else
void NodeConnection::SetAttr(SetAttrRequestView request, SetAttrCompleter::Sync& completer) {
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(28)
  completer.Reply(ZX_ERR_BAD_HANDLE);
}

void NodeConnection::GetAttributes(fio::wire::NodeGetAttributesRequest* request,
                                   GetAttributesCompleter::Sync& completer) {
  if (!(rights() & fuchsia_io::Rights::kGetAttributes)) {
    completer.ReplyError(ZX_ERR_ACCESS_DENIED);
    return;
  }
  internal::NodeAttributeBuilder builder(vnode());
  completer.Reply(builder.Build(request->query));
}

void NodeConnection::UpdateAttributes(fio::wire::MutableNodeAttributes* request,
                                      UpdateAttributesCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_BAD_HANDLE);
}

void NodeConnection::GetFlags(GetFlagsCompleter::Sync& completer) {
  completer.ReplySuccess(fio::Flags::kProtocolNode | RightsToFlags(rights()));
}
void NodeConnection::SetFlags(SetFlagsRequestView, SetFlagsCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void NodeConnection::DeprecatedGetFlags(DeprecatedGetFlagsCompleter::Sync& completer) {
  completer.Reply(ZX_OK, fio::OpenFlags::kNodeReference);
}

void NodeConnection::DeprecatedSetFlags(DeprecatedSetFlagsRequestView,
                                        DeprecatedSetFlagsCompleter::Sync& completer) {
  completer.Reply(ZX_ERR_BAD_HANDLE);
}

void NodeConnection::QueryFilesystem(QueryFilesystemCompleter::Sync& completer) {
  zx::result result = Connection::NodeQueryFilesystem();
  completer.Reply(result.status_value(),
                  result.is_ok()
                      ? fidl::ObjectView<fio::wire::FilesystemInfo>::FromExternal(&result.value())
                      : nullptr);
}

zx::result<> NodeConnection::WithRepresentation(
    fit::callback<zx::result<>(fio::wire::Representation)> handler,
    std::optional<fio::NodeAttributesQuery> query) const {
  using NodeRepresentation = fio::wire::NodeInfo;
  fidl::WireTableFrame<NodeRepresentation> representation_frame;
  auto builder = NodeRepresentation::ExternalBuilder(
      fidl::ObjectView<fidl::WireTableFrame<NodeRepresentation>>::FromExternal(
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
  return handler(fuchsia_io::wire::Representation::WithNode(
      fidl::ObjectView<NodeRepresentation>::FromExternal(&representation)));
}

#if FUCHSIA_API_LEVEL_LESS_THAN(32) || FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
zx_status_t NodeConnection::WithNodeInfoDeprecated(
    fit::callback<zx_status_t(fuchsia_io::wire::NodeInfoDeprecated)> handler) const {
  // In io1, node reference connections are mapped to the service variant of NodeInfoDeprecated.
  return handler(fuchsia_io::wire::NodeInfoDeprecated::WithService({}));
}
#endif

}  // namespace fs::internal
