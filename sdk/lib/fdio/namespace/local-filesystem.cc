// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "local-filesystem.h"

#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>
#include <lib/fdio/zxio.h>
#include <lib/fit/defer.h>
#include <lib/stdcompat/string_view.h>
#include <lib/zx/channel.h>
#include <lib/zxio/cpp/create_with_type.h>
#include <lib/zxio/types.h>
#include <zircon/processargs.h>

#include <utility>

#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <fbl/string.h>

#include "local-connection.h"
#include "local-vnode.h"

namespace fio = fuchsia_io;

namespace {

std::pair<std::string_view, bool> FindNextPathSegment(std::string_view path) {
  const size_t next_slash = path.find('/');
  return {path.substr(0, next_slash), next_slash == std::string_view::npos};
}

zx::result<fbl::RefPtr<fdio_internal::LocalVnode>> CreateRemoteVnode(
    std::optional<fdio_internal::LocalVnode::ParentAndId> parent_and_id,
    fidl::ClientEnd<fio::Directory> remote) {
  zxio_storage_t remote_storage;
  if (zx_status_t status = zxio::CreateDirectory(&remote_storage, std::move(remote));
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(fbl::MakeRefCounted<fdio_internal::LocalVnode>(
      parent_and_id, std::in_place_type_t<fdio_internal::LocalVnode::Remote>(), remote_storage));
}

}  // namespace

fdio_namespace::fdio_namespace() { ResetRoot(); }

void fdio_namespace::ResetRoot() {
  root_ = fbl::MakeRefCounted<LocalVnode>(std::nullopt,
                                          std::in_place_type_t<LocalVnode::Intermediate>());
}

zx_status_t fdio_namespace::WalkLocked(fbl::RefPtr<LocalVnode>* in_out_vn,
                                       std::string_view* in_out_path) const {
  fbl::RefPtr<LocalVnode> vn = *in_out_vn;
  std::string_view path_remaining = *in_out_path;

  // Empty path or "." matches initial node.
  if (path_remaining.empty() || path_remaining == ".") {
    return ZX_OK;
  }

  for (;;) {
    auto [next_path_segment, is_last_segment] = FindNextPathSegment(path_remaining);

    // Path segments may not longer than NAME_MAX.
    if (next_path_segment.length() > NAME_MAX) {
      return ZX_ERR_BAD_PATH;
    }

    // "." matches current node.
    if (next_path_segment != ".") {
      // The outcome of this visit is a ternary that either communicates a success/failure in
      // walking a path within the namespace, or a need to continue parsing the path to
      // find the end of the path within the namespace.
      std::optional walk_status_opt =
          std::visit(fdio_internal::overloaded{
                         [](LocalVnode::Local& c) -> std::optional<zx_status_t> {
                           // Local file are never directories.
                           return ZX_ERR_NOT_FOUND;
                         },
                         [&, next_path_segment = next_path_segment](
                             LocalVnode::Intermediate& c) -> std::optional<zx_status_t> {
                           fbl::RefPtr<LocalVnode> child = c.Lookup(next_path_segment);

                           // If we didn't find the next valid child, there's no way to proceed.
                           if (child == nullptr) {
                             return ZX_ERR_NOT_FOUND;
                           }

                           // Proceed to parse the path, with the new child.
                           vn = child;
                           return std::nullopt;
                         },
                         [&](LocalVnode::Remote& s) -> std::optional<zx_status_t> {
                           *in_out_vn = vn;
                           *in_out_path = path_remaining;
                           return ZX_OK;
                         },
                     },
                     vn->NodeType());

      if (walk_status_opt.has_value()) {
        return walk_status_opt.value();
      }
    }

    if (is_last_segment) {
      // The full path is contained within the fdio_namespace. Return
      // the terminal local_vnode, along with a self-referential
      // remaining path.
      *in_out_vn = vn;
      *in_out_path = ".";
      return ZX_OK;
    }

    // Lookup completed successfully, but more segments exist.
    path_remaining.remove_prefix(next_path_segment.length() + 1);
  }
}

zx::result<fdio_ptr> fdio_namespace::OpenAt(fbl::RefPtr<LocalVnode> vn, std::string_view path,
                                            fio::Flags flags) const {
  {
    std::lock_guard lock(lock_);
    zx_status_t status = WalkLocked(&vn, &path);
    if (status != ZX_OK) {
      return zx::error(status);
    }
  }

  return std::visit(
      fdio_internal::overloaded{
          [&](LocalVnode::Local& l) -> zx::result<fdio_ptr> { return l.Open(); },
          [&](LocalVnode::Intermediate& c) -> zx::result<fdio_ptr> { return CreateConnection(vn); },
          [&](LocalVnode::Remote& s) -> zx::result<fdio_ptr> {
            if ((flags & fio::Flags::kFlagMustCreate) && path == ".") {
              return zx::error(ZX_ERR_ALREADY_EXISTS);
            }
            fdio_ptr io = fbl::MakeRefCounted<fdio_internal::remote>();
            return zx::make_result(
                zxio_open(s.Connection(), path.data(), path.length(), zxio_open_flags_t{flags},
                          /*options*/ {}, &io->zxio_storage()),
                io);
          },
      },
      vn->NodeType());
}

zx_status_t fdio_namespace::Readdir(const LocalVnode& vn, DirentIteratorState* state,
                                    zxio_dirent_t* inout_entry) const {
  std::lock_guard lock(lock_);

  auto populate_entry = [](zxio_dirent_t* inout_entry, std::string_view name) {
    if (name.size() > NAME_MAX) {
      return ZX_ERR_INVALID_ARGS;
    }
    ZXIO_DIRENT_SET(*inout_entry, protocols, ZXIO_NODE_PROTOCOL_DIRECTORY);
    uint8_t name_size = static_cast<uint8_t>(name.size());
    inout_entry->name_length = name_size;
    memcpy(inout_entry->name, name.data(), name_size);
    return ZX_OK;
  };

  if (!state->encountered_dot) {
    zx_status_t status = populate_entry(inout_entry, std::string_view("."));
    if (status != ZX_OK) {
      return status;
    }
    state->encountered_dot = true;
    return ZX_OK;
  }
  zx::result name = vn.Readdir(&state->last_seen);
  if (name.is_error()) {
    return name.error_value();
  }
  return populate_entry(inout_entry, name.value());
}

zx::result<fdio_ptr> fdio_namespace::CreateConnection(fbl::RefPtr<LocalVnode> vn) const {
  return fdio_internal::CreateLocalConnection(fbl::RefPtr(this), std::move(vn));
}

#if FUCHSIA_API_LEVEL_LESS_THAN(31)
zx_status_t fdio_namespace::OpenRemoteDeprecated(std::string_view path, fio::wire::OpenFlags flags,
                                                 fidl::ServerEnd<fio::Node> server_end) const {
  // Require that we start at /
  if (!cpp20::starts_with(path, '/')) {
    return ZX_ERR_NOT_FOUND;
  }
  // Skip leading slash.
  path.remove_prefix(1);

  fbl::RefPtr<LocalVnode> vn;
  {
    std::lock_guard lock(lock_);
    vn = root_;
    zx_status_t status = WalkLocked(&vn, &path);
    if (status != ZX_OK) {
      return status;
    }
  }

  return std::visit(fdio_internal::overloaded{
                        [](LocalVnode::Local& l) {
                          // Cannot connect to non-mount-points.
                          return ZX_ERR_NOT_SUPPORTED;
                        },
                        [](LocalVnode::Intermediate& c) {
                          // Cannot connect to non-mount-points.
                          return ZX_ERR_NOT_SUPPORTED;
                        },
                        [&](LocalVnode::Remote& s) {
                          zx_handle_t borrowed_handle = ZX_HANDLE_INVALID;
                          zx_status_t status = zxio_borrow(s.Connection(), &borrowed_handle);
                          if (status != ZX_OK) {
                            return status;
                          }
                          fidl::UnownedClientEnd<fio::Directory> directory(borrowed_handle);

#if FUCHSIA_API_LEVEL_AT_LEAST(27)
                          return fidl::WireCall(directory)
                              ->DeprecatedOpen(flags, {}, fidl::StringView::FromExternal(path),
                                               std::move(server_end))
                              .status();
#else
                          return fidl::WireCall(directory)
                              ->Open(flags, {}, fidl::StringView::FromExternal(path),
                                     std::move(server_end))
                              .status();
#endif
                        },
                    },
                    vn->NodeType());
}
#endif

zx_status_t fdio_namespace::OpenRemote(std::string_view path, fio::Flags flags,
                                       zx::channel object) const {
  // Require that we start at /
  if (!cpp20::starts_with(path, '/')) {
    return ZX_ERR_NOT_FOUND;
  }
  // Skip leading slash.
  path.remove_prefix(1);

  fbl::RefPtr<LocalVnode> vn;
  {
    std::lock_guard lock(lock_);
    vn = root_;
    zx_status_t status = WalkLocked(&vn, &path);
    if (status != ZX_OK) {
      return status;
    }
  }

  return std::visit(fdio_internal::overloaded{
                        [](LocalVnode::Local& l) {
                          // Cannot connect to non-mount-points.
                          return ZX_ERR_NOT_SUPPORTED;
                        },
                        [](LocalVnode::Intermediate& c) {
                          // Cannot connect to non-mount-points.
                          return ZX_ERR_NOT_SUPPORTED;
                        },
                        [&](LocalVnode::Remote& s) {
                          zx_handle_t borrowed_handle = ZX_HANDLE_INVALID;
                          zx_status_t status = zxio_borrow(s.Connection(), &borrowed_handle);
                          if (status != ZX_OK) {
                            return status;
                          }
                          fidl::UnownedClientEnd<fio::Directory> directory(borrowed_handle);
#if FUCHSIA_API_LEVEL_AT_LEAST(27)
                          return fidl::WireCall(directory)
                              ->Open(fidl::StringView::FromExternal(path), flags, {},
                                     std::move(object))
                              .status();
#else
                          return fidl::WireCall(directory)
                              ->Open3(fidl::StringView::FromExternal(path), flags, {},
                                      std::move(object))
                              .status();
#endif
                        },
                    },
                    vn->NodeType());
}

zx_status_t fdio_namespace::Unbind(std::string_view path) {
  if (!cpp20::starts_with(path, '/')) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Skip leading slash.
  path.remove_prefix(1);

  std::lock_guard lock(lock_);
  fbl::RefPtr<LocalVnode> vn = root_;

  if (path.empty()) {
    if (zx_status_t status =
            std::visit(fdio_internal::overloaded{
                           [&](LocalVnode::Local&) -> zx_status_t { return ZX_OK; },
                           [](LocalVnode::Intermediate&) -> zx_status_t {
                             // The node identified by the path is not a mount point,
                             // so unbinding makes no sense.
                             return ZX_ERR_NOT_FOUND;
                           },
                           [&](LocalVnode::Remote&) -> zx_status_t { return ZX_OK; },
                       },
                       vn->NodeType());
        status != ZX_OK) {
      return status;
    }
    ResetRoot();
    return ZX_OK;
  }

  // This node denotes the "highest" node in a lineage of nodes with
  // one or fewer children. It is tracked to ensure that when the target
  // node identified by `path` is identified, we unbind it and all
  // child-less parents that its removal would have created.
  fbl::RefPtr<LocalVnode> removable_origin_vn;

  for (;;) {
    auto [next_path_segment, is_last_segment] = FindNextPathSegment(path);

    if (next_path_segment.length() > NAME_MAX) {
      return ZX_ERR_BAD_PATH;
    }

    // Check to see if the working node contains a child identified by the next path segment.
    zx::result next_vn =
        std::visit(fdio_internal::overloaded{
                       [](LocalVnode::Local&) -> zx::result<fbl::RefPtr<LocalVnode>> {
                         // At the end of each iteration, its considered a failure for the "next"
                         // working node to not be intermediate if more segments remain, so the only
                         // way to arrive here is if our first working node is a mount point. Our
                         // first working node is always root, and unbinding root is not supported.
                         return zx::error(ZX_ERR_BAD_PATH);
                       },
                       [&, next_path_segment = next_path_segment](
                           LocalVnode::Intermediate& c) -> zx::result<fbl::RefPtr<LocalVnode>> {
                         fbl::RefPtr<LocalVnode> next_vn = c.Lookup(next_path_segment);
                         if (next_vn == nullptr) {
                           // The working node was an intermediate node, and Lookup failed to find
                           // the relevant next path segment.
                           return zx::error(ZX_ERR_NOT_FOUND);
                         }
                         return zx::ok(next_vn);
                       },
                       [](LocalVnode::Remote&) -> zx::result<fbl::RefPtr<LocalVnode>> {
                         // At the end of each iteration, its considered a failure for the "next"
                         // working node to not be intermediate if more segments remain, so the only
                         // way to arrive here is if our first working node is a mount point. Our
                         // first working node is always root, and unbinding root is not supported.
                         return zx::error(ZX_ERR_BAD_PATH);
                       },
                   },
                   vn->NodeType());

    if (next_vn.is_error()) {
      return next_vn.error_value();
    }

    vn = std::move(next_vn.value());

    auto handle_terminal_node = [&removable_origin_vn, &vn, &is_last_segment]() -> zx_status_t {
      if (!is_last_segment) {
        // If the non-final segment of a namespace path has a Local node,
        // then the path is invalid, since this Vnode has no children, and future
        // segments cannot exist in the namespace.
        return ZX_ERR_NOT_FOUND;
      }

      // There is no higher parent to unlink than our target node.
      if (removable_origin_vn == nullptr) {
        removable_origin_vn = vn;
      }

      removable_origin_vn->UnlinkFromParent();
      return ZX_OK;
    };

    // The outcome of this visit is a ternary that either communicates a success/failure in
    // an unbind attempt, or a need to continue parsing the path to find the bind location.
    std::optional status_opt = std::visit(
        fdio_internal::overloaded{
            [&handle_terminal_node](const LocalVnode::Local&) -> std::optional<zx_status_t> {
              return handle_terminal_node();
            },
            [&, is_last_segment = is_last_segment, next_path_segment = next_path_segment](
                const LocalVnode::Intermediate& c) -> std::optional<zx_status_t> {
              if (is_last_segment) {
                // The node identified by the path is not a mount point, so unbinding
                // makes no sense.
                return ZX_ERR_NOT_FOUND;
              }

              if (c.num_children() > 1) {
                // If this node has multiple children (including something OTHER than the
                // node we're potentially unbinding), we shouldn't try to remove it while
                // deleting childless intermediate nodes.
                removable_origin_vn = nullptr;
              } else if (removable_origin_vn == nullptr) {
                // If this node has one or fewer children, it's a viable candidate for
                // removal. Only set this if it's the "highest" node we've seen
                // satisfying this property.
                removable_origin_vn = vn;
              }

              // We only remove the prefix if children are present, as this is the only
              // case in which future iterations will find a new node.
              path.remove_prefix(next_path_segment.length() + 1);

              return std::nullopt;
            },
            [&handle_terminal_node](const LocalVnode::Remote&) -> std::optional<zx_status_t> {
              return handle_terminal_node();
            },
        },
        vn->NodeType());

    if (status_opt.has_value()) {
      return status_opt.value();
    }
  }
}

bool fdio_namespace::IsBound(std::string_view path) {
  if (!cpp20::starts_with(path, '/')) {
    return false;
  }
  path.remove_prefix(1);

  std::lock_guard lock(lock_);
  fbl::RefPtr<LocalVnode> vn = root_;
  zx_status_t status = WalkLocked(&vn, &path);
  if (status != ZX_OK) {
    return false;
  }

  return std::visit(fdio_internal::overloaded{
                        [&](LocalVnode::Local& l) { return path == "."; },
                        [](LocalVnode::Intermediate& c) { return false; },
                        [&](LocalVnode::Remote& s) { return path == "."; },
                    },
                    vn->NodeType());
}

zx_status_t fdio_namespace::Bind(std::string_view path, fidl::ClientEnd<fio::Directory> remote) {
  if (!remote.is_valid()) {
    return ZX_ERR_BAD_HANDLE;
  }
  return Bind(path, [remote = std::move(remote)](
                        std::optional<LocalVnode::ParentAndId> parent_and_id) mutable {
    return CreateRemoteVnode(parent_and_id, std::move(remote));
  });
}

zx_status_t fdio_namespace::Bind(std::string_view path, fdio_open_local_func_t on_open,
                                 void* context) {
  return Bind(path, [on_open, context](std::optional<LocalVnode::ParentAndId> parent_and_id) {
    return zx::ok(fbl::MakeRefCounted<LocalVnode>(
        parent_and_id, std::in_place_type_t<LocalVnode::Local>(), on_open, context));
  });
}

zx_status_t fdio_namespace::Bind(
    std::string_view path,
    fit::function<zx::result<fbl::RefPtr<LocalVnode>>(std::optional<LocalVnode::ParentAndId>)>
        builder) {
  if (!cpp20::starts_with(path, '/')) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Skip leading slash.
  path.remove_prefix(1);

  std::lock_guard lock(lock_);
  if (path.empty()) {
    // We've been asked to bind the namespace root. In this function, we will not
    // bind root if:
    //   A) root was previously an intermediate node, and already has any children.
    //   B) root was previously a remote or local node.
    return std::visit(fdio_internal::overloaded{
                          [](LocalVnode::Local& l) {
                            // Root is already a local node. Bind must fail.
                            return ZX_ERR_ALREADY_EXISTS;
                          },
                          [&](LocalVnode::Intermediate& c) {
                            // Convince the compiler that the lock is held. This is safe to
                            // perform because the lock's scope extends over the synchronous
                            // std::visit call, so the lifetime of the lambda cannot be extended.
                            //
                            // This function is required because std::visit has no way to provide
                            // TA_REQ annotations for lock_, and annotating the lambdas with
                            // NO_TA will prevent future internal locks from being scrutinized.
                            []() __TA_ASSERT(lock_) {}();
                            if (c.num_children() != 0) {
                              // Overlay remotes are disallowed.
                              return ZX_ERR_NOT_SUPPORTED;
                            }

                            // The path was "/" so we're trying to bind to the root vnode.
                            zx::result vn_res = builder({});
                            if (vn_res.is_error()) {
                              return vn_res.error_value();
                            }

                            root_ = std::move(vn_res.value());
                            return ZX_OK;
                          },
                          [](LocalVnode::Remote& s) {
                            // Root is already a remote node. Bind must fail.
                            return ZX_ERR_ALREADY_EXISTS;
                          },
                      },
                      root_->NodeType());
  }

  fbl::RefPtr<LocalVnode> vn = root_;
  fbl::RefPtr<LocalVnode> first_new_node = nullptr;

  // If we fail, but leave any intermediate nodes, we need to clean them up
  // before unlocking and returning.
  auto cleanup = fit::defer([&first_new_node]() {
    if (first_new_node != nullptr) {
      first_new_node->UnlinkFromParent();
    }
  });

  for (;;) {
    auto [next_path_segment, is_last_segment] = FindNextPathSegment(path);

    if (next_path_segment.length() > NAME_MAX) {
      return ZX_ERR_BAD_PATH;
    }

    auto next_path_segment_builder =
        [&](LocalVnode::ParentAndId parent_and_id) -> zx::result<fbl::RefPtr<LocalVnode>> {
      if (is_last_segment) {
        return builder(parent_and_id);
      }
      return zx::ok(fbl::MakeRefCounted<LocalVnode>(
          parent_and_id, std::in_place_type_t<LocalVnode::Intermediate>()));
    };

    // The outcome of this visit is a ternary that either communicates a success/failure in
    // a bind attempt, or a need to continue parsing the path to find the bind location.
    std::optional walk_status_opt = std::visit(
        fdio_internal::overloaded{
            [](LocalVnode::Local& l) -> std::optional<zx_status_t> {
              // Encountering a valid storage end at any point in the bind path
              // implies shadowing, which is not supported.
              return ZX_ERR_NOT_SUPPORTED;
            },
            [&](LocalVnode::Intermediate& c) -> std::optional<zx_status_t> {
              zx::result res = c.LookupOrInsert(next_path_segment, next_path_segment_builder);
              if (res.is_error()) {
                return res.error_value();
              }
              auto& [child, created] = res.value();
              vn = std::move(child);
              if (is_last_segment) {
                // If the final segment already exists as a child on our working node,
                // we cannot overwrite.
                if (!created) {
                  return ZX_ERR_ALREADY_EXISTS;
                }
                return ZX_OK;
              }
              // Keep track of the first node we create. If any subsequent
              // operation fails during bind, we will need to delete all nodes
              // in this subtree.
              if (created && first_new_node == nullptr) {
                first_new_node = vn;
              }

              // Our working node is our new intermediate node. Let's continue
              // the bind.
              return std::nullopt;
            },
            [](LocalVnode::Remote& s) -> std::optional<zx_status_t> {
              // Encountering a valid storage end at any point in the bind path
              // implies shadowing, which is not supported.
              return ZX_ERR_NOT_SUPPORTED;
            },
        },
        vn->NodeType());

    if (walk_status_opt.has_value()) {
      zx_status_t walk_status = walk_status_opt.value();
      // Make sure to cancel our deferred cleanup if
      // the bind succeeded.
      if (walk_status == ZX_OK) {
        cleanup.cancel();
      }
      return walk_status;
    }

    // Proceed to loop onto subpath.
    path.remove_prefix(next_path_segment.length() + 1);
  }
}

zx::result<fdio_ptr> fdio_namespace::OpenRoot() const {
  fbl::RefPtr<LocalVnode> vn = [this]() {
    std::lock_guard lock(lock_);
    return root_;
  }();

  return std::visit(
      fdio_internal::overloaded{
          [&](LocalVnode::Local&) -> zx::result<fdio_ptr> {
            // The root node should never be a local node.
            return zx::error(ZX_ERR_NOT_SUPPORTED);
          },
          [&](LocalVnode::Intermediate&) -> zx::result<fdio_ptr> { return CreateConnection(vn); },
          [](LocalVnode::Remote& s) -> zx::result<fdio_ptr> {
            zx::result endpoints = fidl::CreateEndpoints<fio::Node>();
            if (endpoints.is_error()) {
              return endpoints.take_error();
            }

            zx::channel clone;
            zx_status_t status = zxio_clone(s.Connection(), clone.reset_and_get_address());

            if (status != ZX_OK) {
              return zx::error(status);
            }
            // We know this is a Directory.
            fio::wire::DirectoryInfo info;
            return fdio::create(fidl::ClientEnd<fio::Node>(std::move(clone)),
                                fio::wire::Representation::WithDirectory(
                                    fidl::ObjectView<decltype(info)>::FromExternal(&info)));
          },
      },
      vn->NodeType());
}

zx_status_t fdio_namespace::SetRoot(fdio_t* io) {
  fbl::RefPtr<LocalVnode> vn = fdio_internal::GetLocalNodeFromConnectionIfAny(io);

  if (!vn) {
    fidl::ClientEnd<fio::Directory> client_end;
    zx_status_t status = io->clone(client_end.channel().reset_and_get_address());
    if (status != ZX_OK) {
      return status;
    }

    zx::result vn_res = CreateRemoteVnode({}, std::move(client_end));
    if (vn_res.is_error()) {
      return vn_res.error_value();
    }

    vn = std::move(vn_res.value());
  }

  std::lock_guard lock(lock_);
  if (vn == root_) {
    // Nothing to do.
    return ZX_OK;
  }

  vn->UnlinkFromParent();
  std::swap(root_, vn);
  return ZX_OK;
}

zx_status_t fdio_namespace::Export(fdio_flat_namespace_t** out) const {
  std::lock_guard lock(lock_);

  size_t count = 0;
  size_t buffer_size = 0;
  auto count_callback = [&](std::string_view path, zxio_t* remote) {
    count += 1;
    buffer_size += path.size() + 1;
    return ZX_OK;
  };
  if (zx_status_t status = root_->EnumerateRemotes(count_callback); status != ZX_OK) {
    return status;
  }
  // Allocate enough space for a hypothetical:
  // struct {
  //   fdio_flat_namespace_t flat;
  //   zx_handle_t handle[count];
  //   char* path[count];
  //   char buffer[buffer_size];
  // };
  // Insert padding where needed for alignment.
  auto padding = [](size_t offset, size_t alignment) {
    return (alignment - (offset % alignment)) % alignment;
  };
  size_t offset = 0;
  offset += sizeof(fdio_flat_namespace_t);
  offset += padding(offset, alignof(zx_handle_t));
  const size_t handle_offset = offset;
  offset += sizeof(zx_handle_t) * count;
  offset += padding(offset, alignof(char*));
  const size_t path_offset = offset;
  offset += sizeof(char*) * count;
  const size_t buffer_offset = offset;
  offset += buffer_size;

  std::byte* ptr = static_cast<std::byte*>(malloc(offset));
  if (ptr == nullptr) {
    return ZX_ERR_NO_MEMORY;
  }
  fdio_flat_namespace_t& flat = *reinterpret_cast<fdio_flat_namespace_t*>(ptr);
  flat = {
      .count = 0,
      .handle = reinterpret_cast<zx_handle_t*>(ptr + handle_offset),
      .path = reinterpret_cast<char**>(ptr + path_offset),
  };
  char* buffer = reinterpret_cast<char*>(ptr + buffer_offset);
  auto cleanup = fit::defer([flat = &flat]() { fdio_ns_free_flat_ns(flat); });

  auto export_callback = [&](std::string_view path, zxio_t* remote) {
    zx::channel remote_clone;
    zx_status_t status = zxio_clone(remote, remote_clone.reset_and_get_address());
    if (status != ZX_OK) {
      return status;
    }
    flat.handle[flat.count] = remote_clone.release();
    const_cast<char**>(flat.path)[flat.count] = buffer;
    flat.count += 1;

    memcpy(buffer, path.data(), path.length());
    buffer += path.length();
    *buffer++ = 0;
    return ZX_OK;
  };

  if (zx_status_t status = root_->EnumerateRemotes(export_callback); status != ZX_OK) {
    return status;
  }

  cleanup.cancel();
  *out = &flat;
  return ZX_OK;
}
