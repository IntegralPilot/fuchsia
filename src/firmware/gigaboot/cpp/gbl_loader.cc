// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "gbl_loader.h"

#include <zircon/assert.h>

#include <gbl/gbl_efi_protocols.h>
#include <phys/efi/main.h>

#include "backends.h"
#include "boot_zbi_items.h"
#include "input.h"
#include "utils.h"

namespace gigaboot {

namespace {

// Gigaboot is designed to print to both `SystemTable->ConOut` and the first found
// `SerialIoProtocol`. However GBL currently only prints to `SystemTable->ConOut`. It remains to be
// investigated if we should port Gigaboot's behavior to GBL. For now we use a workaround that
// overrides `SystemTable->ConOut->OutputString` with Gigaboot's print behavior before booting GBL.

// Stores the original function pointer from `gEfiSystemTable->ConOut->OutputString`
efi_status (*original_conout_output_string)(struct efi_simple_text_output_protocol* self,
                                            char16_t* string) EFIAPI = nullptr;

// Overrides for `gEfiSystemTable->ConOut->OutputString` for GBL.
EFIAPI efi_status ConAndSerialOutputString(struct efi_simple_text_output_protocol* self,
                                           char16_t* string) {
  ZX_ASSERT(original_conout_output_string);
  self->OutputString = original_conout_output_string;
  for (size_t i = 0; string[i]; i++) {
    putchar(string[i]);
  }
  self->OutputString = ConAndSerialOutputString;
  return EFI_SUCCESS;
}

const efi_guid kEfiDtbTableGuid = {
    0xb1b621d5, 0xf19c, 0x41a5, {0x83, 0x0b, 0xd9, 0x15, 0x2c, 0x69, 0xaa, 0xe0}};

// GUID for GBL variables.
efi_guid kGblEfiVendorGuid = {
    0x5a6d92f3, 0xa2d0, 0x4083, {0x91, 0xa1, 0xa5, 0x0f, 0x6c, 0x3d, 0x98, 0x30}};

efi_status InstallGblProtocols() {
  printf("Installing GBL_EFI_FASTBOOT_PROTOCOL...\n");
  efi_status res = InstallGblEfiFastbootProtocol();
  if (res != EFI_SUCCESS) {
    printf("Failed to install GBL_EFI_FASTBOOT_PROTOCOL: %s\n", EfiStatusToString(res));
    return res;
  }

  printf("Installing GBL_EFI_BOOT_CONTROL_PROTOCOL...\n");
  res = InstallGblEfiBootControlProtocol();
  if (res != EFI_SUCCESS) {
    printf("Failed to install GBL_EFI_BOOT_CONTROL_PROTOCOL: %s\n", EfiStatusToString(res));
    return res;
  }

  return EFI_SUCCESS;
}

}  // namespace

std::span<uint8_t> GblFdt();
std::span<uint8_t> GblFdtZbiBlob();
std::span<uint8_t> GblFdtPermAttr();
std::span<uint8_t> GblFdtPermAttrHash();
std::span<uint8_t> GblFdtStopInFastboot();

const std::span<const uint8_t> GetGblEfiApp();

zx::result<> LaunchGbl(bool stop_in_fastboot) {
  const std::span<const uint8_t> app = GetGblEfiApp();

  // Allocates boot buffer and copies over the embedded GBL EFI app blob. This is because
  // `LoadImage()` requires a non-const pointer for the source buffer parameter.
  efi_physical_addr addr;
  efi_status status = gEfiSystemTable->BootServices->AllocatePages(
      AllocateAnyPages, EfiLoaderData, DivideRoundUp(app.size(), kUefiPageSize), &addr);
  if (status != EFI_SUCCESS) {
    printf("Failed to allocate GBL EFI load buffer: %s\n", EfiStatusToString(status));
    return zx::error(static_cast<int>(status));
  }
  memcpy(reinterpret_cast<void*>(addr), app.data(), app.size());

  // Load EFI image.
  efi_handle app_handle = NULL;
  status = gEfiSystemTable->BootServices->LoadImage(
      false, gEfiImageHandle, nullptr, reinterpret_cast<void*>(addr), app.size(), &app_handle);
  if (status != EFI_SUCCESS) {
    printf("Failed to load image: %s\n", EfiStatusToString(status));
    return zx::error(static_cast<int>(status));
  }

  // Passes board specific data via a device tree in the EFI configuration table.

  // Prepares device ZBI items
  // Constructs the ZBI at an aligned address as required by the APIs.
  uintptr_t fdt_zbi_blob_addr = reinterpret_cast<uintptr_t>(GblFdtZbiBlob().data());
  size_t zbi_alignment = size_t{ZBI_ALIGNMENT};
  uintptr_t aligned_addr = DivideRoundUp(fdt_zbi_blob_addr, zbi_alignment) * zbi_alignment;
  std::span<uint8_t> aligned = GblFdtZbiBlob().subspan(aligned_addr - fdt_zbi_blob_addr);
  zbi_header_t* zbi = reinterpret_cast<zbi_header_t*>(aligned.data());
  size_t capacity = aligned.size();
  if (zbi_result_t res = zbi_init(zbi, capacity); res != ZBI_RESULT_OK) {
    printf("Failed to init ZBI container: %d\n", res);
    return zx::error(static_cast<int>(res));
  }

  ZbiContext context = {};
  if (!AddGigabootZbiItems(zbi, capacity, nullptr, &context)) {
    printf("Failed to add ZBI items\n");
    return zx::error(1);
  }

  // Constructs a ZBI_TYPE_MEM_CONFIG ZBI item that contains only ZBI_MEM_TYPE_PERIPHERAL type
  // memory ranges derived from the ACPI table. GBL is responsible for updating this item with other
  // ZBI_MEM_TYPE_RAM/ZBI_MEM_TYPE_RESERVED memory ranges from the EFI memory map upon calling
  // ExitBootService().
  void* mem_payload = nullptr;
  uint32_t payload_len = 0;
  if (zbi_result_t res = zbi_get_next_entry_payload(zbi, capacity, &mem_payload, &payload_len);
      res != ZBI_RESULT_OK) {
    printf("Failed to get payload buffer for peripheral memory ZBI items: %d\n", res);
    return zx::error(static_cast<int>(res));
  }

  auto mems = std::span{reinterpret_cast<zbi_mem_range_t*>(mem_payload),
                        payload_len / sizeof(zbi_mem_range_t)};
  auto collected = CollectPeripheralMemoryItems(&context, mems);
  if (collected.is_error()) {
    printf("Failed to collect peripheral memory ZBI items: %d\n", collected.error_value());
    return collected.take_error();
  } else if (collected->size()) {
    for (auto ele : *collected) {
      printf("Peripheral memory. paddr = %zu, length = %zu\n", size_t{ele.paddr},
             size_t{ele.length});
    }

    if (zbi_result_t res = zbi_create_entry_with_payload(
            zbi, capacity, ZBI_TYPE_MEM_CONFIG, 0, 0, (*collected).data(),
            (*collected).size() * sizeof(zbi_mem_range_t));
        res != ZBI_RESULT_OK) {
      printf("Failed to add peripheral memory ZBI items: %d\n", res);
      return zx::error(static_cast<int>(res));
    }
  }

  printf("Board ZBI items size: %zu\n", sizeof(zbi_header_t) + zbi->length);
  memmove(GblFdtZbiBlob().data(), zbi, capacity);

  // Prepares permanent attributes.
  ZX_ASSERT(GblFdtPermAttr().size() >= GetPermanentAttributes().size());
  memcpy(GblFdtPermAttr().data(), GetPermanentAttributes().data(), GetPermanentAttributes().size());
  // Prepares permanent attributes hash.
  ZX_ASSERT(GblFdtPermAttrHash().size() >= GetPermanentAttributesHash().size());
  memcpy(GblFdtPermAttrHash().data(), GetPermanentAttributesHash().data(),
         GetPermanentAttributesHash().size());

  // Sets `stop-in-fastboot` value.
  // This can be removed once infra GBL is updated.
  GblFdtStopInFastboot()[0] = stop_in_fastboot ? 1 : 0;
  g_should_stop_in_fastboot = stop_in_fastboot;

  // Installs device tree.
  status =
      gEfiSystemTable->BootServices->InstallConfigurationTable(&kEfiDtbTableGuid, GblFdt().data());
  if (status != EFI_SUCCESS) {
    printf("Failed to load device tree: %s\n", EfiStatusToString(status));
    return zx::error(static_cast<int>(status));
  }

  // Installs GBL EFI protocols
  status = InstallGblProtocols();
  if (status != EFI_SUCCESS) {
    return zx::error(static_cast<int>(status));
  }

  // GBL provides an EFI variable for specifying fuchsia as boot target.
  wchar_t gbl_os_boot_fuchsia[] = {L"gbl_os_boot_fuchsia"};
  // Value of the variable doesn't matter. As long as the variable exists, GBL considers fuchsia.
  // However it must be no more than 1 byte because that is what GBL attempts to read.
  status = gEfiSystemTable->RuntimeServices->SetVariable(
      reinterpret_cast<char16_t*>(gbl_os_boot_fuchsia), &kGblEfiVendorGuid,
      EFI_VARIABLE_BOOTSERVICE_ACCESS, 1, "1");
  if (status != EFI_SUCCESS) {
    printf("Failed to set \"gbl_os_boot_fuchsia\" variable: %s\n", EfiStatusToString(status));
    return zx::error(static_cast<int>(status));
  }

  printf("Starting GBL...\n");

  // Overrides `gEfiSystemTable->ConOut->OutputString`
  original_conout_output_string = gEfiSystemTable->ConOut->OutputString;
  gEfiSystemTable->ConOut->OutputString = ConAndSerialOutputString;

  // Starts EFI image.
  size_t exit_data_size = 0;
  status = gEfiSystemTable->BootServices->StartImage(app_handle, &exit_data_size, nullptr);
  gEfiSystemTable->ConOut->OutputString = original_conout_output_string;
  if (status != EFI_SUCCESS) {
    printf("Failed to start image: %s, %zu\n", EfiStatusToString(status), status);
    return zx::error(static_cast<int>(status));
  }

  // Not expected to return if boot succeeds.
  ZX_ASSERT(false);

  return zx::error(1);
}
}  // namespace gigaboot
