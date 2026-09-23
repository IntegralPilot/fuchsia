// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/misc/drivers/compat/loader.h"

#include <lib/async/default.h>
#include <lib/zx/vmo.h>

namespace compat {

namespace fio = fuchsia_io;

Loader::Loader(fidl::UnownedClientEnd<fuchsia_ldsvc::Loader> loader, OverrideMap overrides)
    : dispatcher_(async_get_default_dispatcher()),
      client_(loader),
      overrides_(std::move(overrides)) {}

void Loader::Bind(fidl::ServerEnd<fuchsia_ldsvc::Loader> request) {
  bindings_.AddBinding(dispatcher_, std::move(request), this, fidl::kIgnoreBindingClosure);
}

void Loader::Done(DoneCompleter::Sync& completer) { completer.Close(ZX_OK); }

void Loader::LoadObject(LoadObjectRequestView request, LoadObjectCompleter::Sync& completer) {
  // When there is a request for an overriden library return the override instead.
  if (auto file = overrides_.find(std::string(request->object_name.get()));
      file != overrides_.end()) {
    auto result = fidl::Call(file->second)
                      ->GetBackingMemory(fio::VmoFlags::kRead | fio::VmoFlags::kExecute |
                                         fio::VmoFlags::kPrivateClone);
    if (!result.is_ok()) {
      zx_status_t status = result.error_value().is_domain_error()
                               ? result.error_value().domain_error()
                               : result.error_value().framework_error().status();
      completer.Reply(status, zx::vmo());
      return;
    }
    completer.Reply(ZX_OK, std::move(result->vmo()));
    return;
  }

  fidl::WireResult result = fidl::WireCall(client_)->LoadObject(request->object_name);
  if (!result.ok()) {
    completer.Reply(result.status(), {});
    return;
  }
  completer.Reply(result->rv, std::move(result->object));
}

void Loader::Config(ConfigRequestView request, ConfigCompleter::Sync& completer) {
  fidl::WireResult result = fidl::WireCall(client_)->Config(request->config);
  if (!result.ok()) {
    completer.Reply(result.status());
    return;
  }
  completer.Reply(result->rv);
}

void Loader::Clone(CloneRequestView request, CloneCompleter::Sync& completer) {
  bindings_.AddBinding(dispatcher_, std::move(request->loader), this, fidl::kIgnoreBindingClosure);
  completer.Reply(ZX_OK);
}

}  // namespace compat
