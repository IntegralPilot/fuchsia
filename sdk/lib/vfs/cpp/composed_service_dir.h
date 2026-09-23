// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_VFS_CPP_COMPOSED_SERVICE_DIR_H_
#define LIB_VFS_CPP_COMPOSED_SERVICE_DIR_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fuchsia/io/cpp/fidl.h>
#include <lib/fidl/cpp/client.h>
#include <lib/vfs/cpp/node.h>
#include <lib/vfs/cpp/service.h>
#include <zircon/assert.h>

#include <string>

namespace vfs {

// A directory-like object which created a composed `PseudoDir` on top of a `fallback_dir`. It can
// be used to connect to services in `fallback_dir`, but will not enumerate them.
//
// This class is thread-safe.
class ComposedServiceDir final : public Node {
 public:
  ComposedServiceDir() : Node(MakeComposedServiceDir()) {}

  // Serve a new connection to this directory on `server_end` using specified `flags`.
  //
  // This method must only be used with a single-threaded asynchronous dispatcher. If `dispatcher`
  // is `nullptr`, the current thread's default dispatcher will be used via
  // `async_get_default_dispatcher`. The same `dispatcher` must be used if multiple connections are
  // served for the same node, otherwise `ZX_ERR_INVALID_ARGS` will be returned.
  zx_status_t Serve(fuchsia_io::Flags flags, fidl::ServerEnd<fuchsia_io::Directory> server_end,
                    async_dispatcher_t* dispatcher = nullptr) const {
    if (flags & (fuchsia_io::wire::kMaskKnownProtocols ^ fuchsia_io::Flags::kProtocolDirectory)) {
      return ZX_ERR_INVALID_ARGS;  // Only the directory protocol is allowed with this signature.
    }
    return ServeInternal(flags | fuchsia_io::Flags::kProtocolDirectory, server_end.TakeChannel(),
                         dispatcher);
  }

  // Sets the fallback directory for services. Services in this directory can be connected to, but
  // will not be enumerated. This method may only be called once.
  void SetFallback(fidl::ClientEnd<fuchsia_io::Directory> fallback_dir) {
    ZX_ASSERT(vfs_internal_composed_svc_dir_set_fallback(
                  handle(), fallback_dir.TakeChannel().release()) == ZX_OK);
  }

  // Adds a service to this directory. Services added will be preferred over those in the fallback
  // directory, and can be enumerated.
  void AddService(const std::string& service_name, std::unique_ptr<vfs::Service> service) {
    ZX_ASSERT(vfs_internal_composed_svc_dir_add(handle(), service->handle(), service_name.data()) ==
              ZX_OK);
  }

#if FUCHSIA_API_LEVEL_LESS_THAN(31)
  // Sets the fallback directory for services. Services in this directory can be connected to, but
  // will not be enumerated. This method may only be called once.
  void set_fallback(fidl::InterfaceHandle<fuchsia::io::Directory> fallback_dir)
      ZX_REMOVED_SINCE(1, 25, 29, "Replaced by SetFallback().") {
    ZX_ASSERT(vfs_internal_composed_svc_dir_set_fallback(
                  handle(), fallback_dir.TakeChannel().release()) == ZX_OK);
  }
#endif

 private:
  static vfs_internal_node_t* MakeComposedServiceDir() {
    vfs_internal_node_t* dir;
    ZX_ASSERT(vfs_internal_composed_svc_dir_create(&dir) == ZX_OK);
    return dir;
  }
};

}  // namespace vfs

#endif  // LIB_VFS_CPP_COMPOSED_SERVICE_DIR_H_
