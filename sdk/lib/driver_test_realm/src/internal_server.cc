// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/component/incoming/cpp/clone.h>
#include <lib/driver_test_realm/src/internal_server.h>

namespace driver_test_realm {

InternalServer::InternalServer(fidl::ClientEnd<fuchsia_io::Directory> boot_dir,
                               fidl::ClientEnd<fuchsia_io::Directory> pkg_dir,
                               fidl::ClientEnd<fuchsia_io::Directory> test_pkg_dir,
                               std::optional<fuchsia_component_resolution::Context> context,
                               std::optional<std::vector<std::string>> boot_driver_components)
    : boot_dir_(std::move(boot_dir)),
      pkg_dir_(std::move(pkg_dir)),
      test_pkg_dir_(std::move(test_pkg_dir)),
      context_(std::move(context)),
      boot_driver_components_(std::move(boot_driver_components)) {}

void InternalServer::Serve(async_dispatcher_t* dispatcher,
                           fidl::ServerEnd<fuchsia_driver_test::Internal> server_end) {
  bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
}

void InternalServer::GetTestPackage(GetTestPackageCompleter::Sync& completer) {
  if (test_pkg_dir_.is_valid()) {
    auto dir_clone = component::Clone(test_pkg_dir_);
    if (dir_clone.is_ok()) {
      completer.ReplySuccess(std::move(*dir_clone));
    } else {
      completer.ReplyError(dir_clone.error_value());
    }

    return;
  }

  if (pkg_dir_.is_valid()) {
    auto dir_clone = component::Clone(pkg_dir_);
    if (dir_clone.is_ok()) {
      completer.ReplySuccess(std::move(*dir_clone));
    } else {
      completer.ReplyError(dir_clone.error_value());
    }

    return;
  }

  completer.ReplyError(ZX_ERR_NOT_FOUND);
}

void InternalServer::GetTestResolutionContext(GetTestResolutionContextCompleter::Sync& completer) {
  fidl::Arena arena;
  if (context_.has_value()) {
    completer.ReplySuccess(fidl::ObjectView<fuchsia_component_resolution::wire::Context>(
        arena, fidl::ToWire(arena, *context_)));
  } else {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
  }
}

void InternalServer::GetBootDirectory(GetBootDirectoryCompleter::Sync& completer) {
  if (!boot_dir_.is_valid()) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }

  auto dir_clone = component::Clone(boot_dir_);
  if (dir_clone.is_ok()) {
    completer.ReplySuccess(std::move(*dir_clone));
  } else {
    completer.ReplyError(dir_clone.error_value());
  }
}

void InternalServer::GetBootDriverOverrides(GetBootDriverOverridesCompleter::Sync& completer) {
  if (!boot_driver_components_.has_value()) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }

  fidl::Arena arena;
  completer.ReplySuccess(fidl::ToWire(arena, *boot_driver_components_));
}

}  // namespace driver_test_realm
