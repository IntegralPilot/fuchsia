// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>

#include <array>
#include <span>
#include <string_view>

#include <gbl/gbl_efi_protocols.h>
#include <gbl/uefi/protocols/gbl_efi_fastboot_protocol.h>
#include <phys/efi/main.h>

#define GBL_EFI_FASTBOOT_PROTOCOL_GUID \
  {0xc67e48a0, 0x5eb8, 0x4127, {0xbe, 0x89, 0xdf, 0x2e, 0xd9, 0x3d, 0x8a, 0x9a}}

namespace {

// It's easier to work with `char` here so we can use `string_view`, but we need to cast between
// `EfiChar8` for the protocol APIs, so double-check that they're the same size. Signedness
// shouldn't matter since in both cases they're expected to be UTF-8.
static_assert(sizeof(char) == sizeof(EfiChar8), "Char size mismatch");

// Contains information such as variable name and value.
constexpr struct Variable {
  const char* var_name;
  // For now we only consider constant variable.
  const char* var_impl;

  /// Gets the name as a string_view.
  std::string_view name() const { return std::string_view(var_name); }

  /// Gets the value as a string_view.
  std::string_view impl() const { return std::string_view(var_impl); }
} kVariables[] = {
    {"hw-revision", BOARD_NAME},
};

/// Gets the list of variables
std::span<const Variable> variables() { return std::span<const Variable>(kVariables); }

efi_status GetVar(struct GblEfiFastbootProtocol* self, size_t num_args, const EfiChar8* const* args,
                  size_t* buffer_size, EfiChar8* buffer) {
  const std::span<const char* const> args_span{reinterpret_cast<const char* const*>(args),
                                               num_args};
  if (args_span.empty() || !buffer_size) {
    return EFI_INVALID_PARAMETER;
  }

  std::span<uint8_t> out{buffer, *buffer_size};
  for (size_t i = 0; i < variables().size(); i++) {
    const Variable& var = variables()[i];
    if (std::string_view(args_span[0]) != var.name()) {
      continue;
    }

    if (out.size() < var.impl().size() + 1) {
      return EFI_BUFFER_TOO_SMALL;
    }
    memcpy(out.data(), var.impl().data(), var.impl().size());
    *buffer_size = var.impl().size();
    out.data()[*buffer_size] = 0;
    return EFI_SUCCESS;
  }
  return EFI_NOT_FOUND;
}

efi_status GetVarAll(struct GblEfiFastbootProtocol* self, void* context, GetVarAllCallback cb) {
  for (size_t i = 0; i < variables().size(); i++) {
    std::array args{variables()[i].name().data()};
    cb(context, args.size(), reinterpret_cast<const EfiChar8**>(args.data()),
       reinterpret_cast<const EfiChar8*>(variables()[i].impl().data()));
  }
  return EFI_SUCCESS;
}

EfiStatus CommandExec(struct GblEfiFastbootProtocol* self, size_t num_args,
                      const EfiChar8* const* args, size_t download_buffer_size,
                      size_t download_buffer_used_size, uint8_t* download_buffer,
                      GblEfiFastbootCommandExecResult* implementation, FastbootMessageSender sender,
                      void* context) {
  *implementation =
      GblEfiFastbootCommandExecResult::GBL_EFI_FASTBOOT_COMMAND_EXEC_RESULT_DEFAULT_IMPL;
  return EFI_SUCCESS;
}

GblEfiFastbootProtocol protocol = {
    .revision = GBL_EFI_FASTBOOT_PROTOCOL_REVISION,
    .get_var = GetVar,
    .get_var_all = GetVarAll,
    .get_staged = nullptr,
    .command_exec = CommandExec,
    .get_partition_type = nullptr,
};

efi_guid guid = GBL_EFI_FASTBOOT_PROTOCOL_GUID;

}  // namespace

namespace gigaboot {
efi_status InstallGblEfiFastbootProtocol() {
  efi_handle out_handle = nullptr;
  return gEfiSystemTable->BootServices->InstallMultipleProtocolInterfaces(&out_handle, &guid,
                                                                          &protocol, NULL);
}
}  // namespace gigaboot
