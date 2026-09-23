// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/fdio/namespace/namespace.h"

#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/fdio/namespace.h>
#include <lib/zx/channel.h>
#include <zircon/types.h>

#include <cerrno>

#include <fbl/ref_ptr.h>

#include "fidl/fuchsia.io/cpp/wire_types.h"
#include "sdk/lib/fdio/cleanpath.h"
#include "sdk/lib/fdio/fdio_state.h"
#include "sdk/lib/fdio/fdio_unistd.h"
#include "sdk/lib/fdio/internal.h"
#include "sdk/lib/fdio/namespace/local-connection.h"
#include "sdk/lib/fdio/namespace/local-filesystem.h"
#include "sdk/lib/fdio/unistd.h"

namespace fio = fuchsia_io;

zx::result<fbl::RefPtr<fdio>> fdio_ns_open_root(fdio_ns_t* ns) { return ns->OpenRoot(); }

zx_status_t fdio_ns_set_root(fdio_ns_t* ns, fdio_t* io) { return ns->SetRoot(io); }

__BEGIN_CDECLS

#if FUCHSIA_API_LEVEL_LESS_THAN(31)
__EXPORT
zx_status_t fdio_ns_open(fdio_ns_t* ns, const char* path, uint32_t flags, zx_handle_t request) {
  if (path == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  fdio_internal::PathBuffer clean;
  bool is_dir;
  if (!fdio_internal::CleanPath(path, &clean, &is_dir)) {
    return ZX_ERR_BAD_PATH;
  }
  auto remote = fidl::ServerEnd<fio::Node>(zx::channel(request));
  return ns->OpenRemoteDeprecated(clean, static_cast<fio::wire::OpenFlags>(flags),
                                  std::move(remote));
}
#endif

__EXPORT
zx_status_t fdio_ns_open3(fdio_ns_t* ns, const char* path, uint64_t flags, zx_handle_t request) {
  if (path == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  fdio_internal::PathBuffer clean;
  bool is_dir;
  if (!fdio_internal::CleanPath(path, &clean, &is_dir)) {
    return ZX_ERR_BAD_PATH;
  }
  return ns->OpenRemote(clean, static_cast<fio::Flags>(flags), zx::channel(request));
}

__EXPORT
zx_status_t fdio_ns_service_connect(fdio_ns_t* ns, const char* path, zx_handle_t request) {
#if FUCHSIA_API_LEVEL_AT_LEAST(27)
  // TODO(https://fxbug.dev/324080864): We have some CTF tests built with older versions of the
  // driver framework that incorrectly used this function to open the service directory. They now
  // use `fdio_ns_open3` directly with the correct flags (PERM_READABLE). We can remove this
  // conditional when we drop support for the simulate-scan-wlan-hw-sim CTF tests archived prior to
  // API level 23.
  if (path && std::string_view(path) == "/svc") {
    return fdio_ns_open3(ns, path, uint64_t{fio::wire::kPermReadable}, request);
  }
  return fdio_ns_open3(ns, path, uint64_t{fio::Flags::kProtocolService}, request);
#else
  return fdio_ns_open(ns, path, 0, request);
#endif
}

__EXPORT
zx_status_t fdio_ns_create(fdio_ns_t** out) {
  // Create a ref-counted object, and leak the reference that is returned
  // via the C API.
  //
  // This reference is reclaimed in fdio_ns_destroy.
  fbl::RefPtr ns = fbl::MakeRefCounted<fdio_namespace>();
  *out = fbl::ExportToRawPtr(&ns);
  return ZX_OK;
}

__EXPORT
zx_status_t fdio_ns_destroy(fdio_ns_t* raw_ns) {
  // This function reclaims a reference which was leaked in fdio_ns_create.
  [[maybe_unused]] auto ns = fbl::ImportFromRawPtr<fdio_namespace>(raw_ns);
  return ZX_OK;
}

__EXPORT
zx_status_t fdio_ns_bind_local(fdio_ns_t* ns, const char* path, fdio_open_local_func_t on_open,
                               void* context) {
  if (path == nullptr || on_open == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  fdio_internal::PathBuffer clean;
  bool is_dir;
  if (!fdio_internal::CleanPath(path, &clean, &is_dir)) {
    return ZX_ERR_BAD_PATH;
  }
  if (is_dir) {
    // Local binding are always files.
    return ZX_ERR_INVALID_ARGS;
  }
  return ns->Bind(clean, on_open, context);
}

__EXPORT
zx_status_t fdio_ns_bind(fdio_ns_t* ns, const char* path, zx_handle_t remote) {
  if (path == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  fdio_internal::PathBuffer clean;
  bool is_dir;
  if (!fdio_internal::CleanPath(path, &clean, &is_dir)) {
    return ZX_ERR_BAD_PATH;
  }
  return ns->Bind(clean, fidl::ClientEnd<fio::Directory>(zx::channel(remote)));
}

__EXPORT
zx_status_t fdio_ns_unbind(fdio_ns_t* ns, const char* path) {
  if (path == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  fdio_internal::PathBuffer clean;
  bool is_dir;
  if (!fdio_internal::CleanPath(path, &clean, &is_dir)) {
    return ZX_ERR_BAD_PATH;
  }
  return ns->Unbind(clean);
}

__EXPORT
bool fdio_ns_is_bound(fdio_ns_t* ns, const char* path) {
  if (path == nullptr) {
    return false;
  }
  fdio_internal::PathBuffer clean;
  bool is_dir;
  if (!fdio_internal::CleanPath(path, &clean, &is_dir)) {
    return false;
  }
  return ns->IsBound(clean);
}

__EXPORT
zx_status_t fdio_ns_bind_fd(fdio_ns_t* ns, const char* path, int fd) {
  zx_handle_t handle = ZX_HANDLE_INVALID;
  zx_status_t status = fdio_fd_clone(fd, &handle);
  if (status != ZX_OK) {
    return status;
  }

  return fdio_ns_bind(ns, path, handle);
}

__EXPORT
int fdio_ns_opendir(fdio_ns_t* ns) {
  zx::result io = ns->OpenRoot();
  if (io.is_error()) {
    errno = ENOMEM;
    return -1;
  }
  std::optional fd = fdio_global_state().bind_to_fd(io.value());
  if (fd.has_value()) {
    return fd.value();
  }
  return ERRNO(EMFILE);
}

__EXPORT
zx_status_t fdio_ns_chdir(fdio_ns_t* ns) {
  zx::result io = ns->OpenRoot();
  if (io.is_error()) {
    return ZX_ERR_NO_MEMORY;
  }
  fdio_chdir(io.value(), "/");
  return ZX_OK;
}

__EXPORT
zx_status_t fdio_ns_export(fdio_ns_t* ns, fdio_flat_namespace_t** out) { return ns->Export(out); }

__EXPORT
zx_status_t fdio_ns_export_root(fdio_flat_namespace_t** out) {
  fdio_state_t& gstate = fdio_global_state();
  std::lock_guard lock(gstate.lock);
  return fdio_ns_export(gstate.ns, out);
}

__EXPORT
void fdio_ns_free_flat_ns(fdio_flat_namespace_t* ns) {
  zx_handle_close_many(ns->handle, ns->count);
  free(ns);
}

__END_CDECLS
