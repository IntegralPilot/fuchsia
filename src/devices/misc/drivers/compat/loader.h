// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_MISC_DRIVERS_COMPAT_LOADER_H_
#define SRC_DEVICES_MISC_DRIVERS_COMPAT_LOADER_H_

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.ldsvc/cpp/wire.h>

#include <string>
#include <unordered_map>

namespace compat {

// Loader is a loader service that is used to override the DFv1 driver library
// with an alternative implementation.
//
// For most requests, it passes them along to a backing loader service, however
// if the DFv1 driver library is requested, it will return the compatibility
// driver's VMO.
class Loader : public fidl::WireServer<fuchsia_ldsvc::Loader> {
 public:
  using OverrideMap = std::unordered_map<std::string, fidl::ClientEnd<fuchsia_io::File>>;

  Loader(fidl::UnownedClientEnd<fuchsia_ldsvc::Loader> loader, OverrideMap overrides);

  void Bind(fidl::ServerEnd<fuchsia_ldsvc::Loader> request);

 private:
  // fidl::WireServer<fuchsia_ldsvc::Loader>
  void Done(DoneCompleter::Sync& completer) override;
  void LoadObject(LoadObjectRequestView request, LoadObjectCompleter::Sync& completer) override;
  void Config(ConfigRequestView request, ConfigCompleter::Sync& completer) override;
  void Clone(CloneRequestView request, CloneCompleter::Sync& completer) override;

  async_dispatcher_t* dispatcher_;
  fidl::UnownedClientEnd<fuchsia_ldsvc::Loader> client_;
  OverrideMap overrides_;
  fidl::ServerBindingGroup<fuchsia_ldsvc::Loader> bindings_;
};

}  // namespace compat

#endif  // SRC_DEVICES_MISC_DRIVERS_COMPAT_LOADER_H_
