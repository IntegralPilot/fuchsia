// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/transport.h"

#include "tools/fidl/fidlc/src/flat_ast.h"

namespace fidlc {

std::optional<HandleClass> HandleClassFromName(const Name& name) {
  auto& library = name.library()->name;
  auto decl = name.decl_name();
  if (library == "zx" && decl == "Handle")
    return HandleClass::kZircon;
  if (library == "fdf" && decl == "handle")
    return HandleClass::kDriver;
  return std::nullopt;
}

bool Transport::IsCompatible(HandleClass handle_class) const {
  return compatible_handle_classes.find(handle_class) != compatible_handle_classes.end();
}

const Transport* Transport::FromTransportName(std::string_view transport_name) {
  for (const Transport& transport : transports) {
    if (transport.name == transport_name) {
      return &transport;
    }
  }
  return nullptr;
}

std::set<std::string_view> Transport::AllTransportNames() {
  std::set<std::string_view> names;
  for (const auto& entry : transports) {
    names.insert(entry.name);
  }
  return names;
}

std::vector<Transport> Transport::transports = {
    Transport{
        .kind = Kind::kZirconChannel,
        .name = "Channel",
        .handle_class = HandleClass::kZircon,
        .compatible_handle_classes = {HandleClass::kZircon},
    },
    Transport{
        .kind = Kind::kDriverChannel,
        .name = "Driver",
        .handle_class = HandleClass::kDriver,
        .compatible_handle_classes = {HandleClass::kZircon, HandleClass::kDriver},
    },
    Transport{
        .kind = Kind::kBanjo,
        .name = "Banjo",
        .handle_class = HandleClass::kBanjo,
        .compatible_handle_classes = {HandleClass::kZircon, HandleClass::kBanjo},
    },
    Transport{
        .kind = Kind::kSyscall,
        .name = "Syscall",
        .compatible_handle_classes = {HandleClass::kZircon},
    },
};

}  // namespace fidlc
