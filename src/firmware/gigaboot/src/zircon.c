// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "zircon.h"

#include <inttypes.h>
#include <lib/zbi-format/board.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbi-format/internal/efi.h>
#include <lib/zbi-format/kernel.h>
#include <lib/zbi-format/memory.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbi/zbi.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <xefi.h>
#include <zircon/limits.h>

#include <efi/boot-services.h>
#include <efi/runtime-services.h>
#include <efi/types.h>

#include "page_size.h"

#define MAX_CPU_COUNT 16

#if __x86_64__
const uint32_t MY_ARCH_KERNEL_TYPE = ZBI_TYPE_KERNEL_X64;
#elif __aarch64__
const uint32_t MY_ARCH_KERNEL_TYPE = ZBI_TYPE_KERNEL_ARM64;
#endif

// Tries to generate an EFI memory attributes table ZBI item and insert it in the given ramdisk.
efi_status generate_efi_memory_attributes_table_item(void* ramdisk, const size_t ramdisk_size,
                                                     efi_system_table* sys, const void* mmap,
                                                     size_t memory_map_size, size_t dsize) {
  const efi_memory_attributes_table_header* efi_provided_table = NULL;

  const efi_guid kMemoryAttributesGuid = EFI_MEMORY_ATTRIBUTES_GUID;
  for (size_t i = 0; i < sys->NumberOfTableEntries; i++) {
    if (!memcmp(&kMemoryAttributesGuid, &sys->ConfigurationTable[i].VendorGuid, sizeof(efi_guid))) {
      efi_provided_table = sys->ConfigurationTable[i].VendorTable;
      break;
    }
  }

  efi_memory_attributes_table_header* generated_header;
  uint32_t max_payload_length;

  zbi_result_t zbi_result = zbi_get_next_entry_payload(
      ramdisk, ramdisk_size, (void**)&generated_header, &max_payload_length);
  if (zbi_result != ZBI_RESULT_OK) {
    printf("Failed to create next entry payload: 0x%x", zbi_result);
    return EFI_ABORTED;
  }

  const size_t max_items = (max_payload_length - sizeof(efi_memory_attributes_table_header)) /
                           sizeof(efi_memory_descriptor);

  efi_memory_descriptor* generated_items = (efi_memory_descriptor*)&generated_header[1];

  size_t items = memory_map_size / dsize;

  size_t item_count = 0;
  for (size_t i = 0; i < items; i++) {
    efi_memory_descriptor* item = (efi_memory_descriptor*)&mmap[dsize * i];
    if (item->Attribute & EFI_MEMORY_RUNTIME) {
      // Remove any ranges that are handled by the actual EFI memory attributes table.
      size_t base = item->PhysicalStart;
      size_t size = item->NumberOfPages * PAGE_SIZE;

      for (size_t j = 0;
           efi_provided_table != NULL && j < efi_provided_table->number_of_entries && size != 0;
           j++) {
        // This EMAT entry is either a sub-region or a full copy of the memory map region, per
        // EFI 2.10 4.6.4: "Additionally, every memory region described by a Descriptor in
        // EFI_MEMORY_ATTRIBUTES_TABLE must be a sub-region of, or equal to, a descriptor in the
        // table produced by GetMemoryMap()."
        //
        // This means that we do not have to consider the case where the EMAT entry only overlaps
        // the end of the memory map entry.

        efi_memory_descriptor* emat_item =
            (efi_memory_descriptor*)(((uint8_t*)&efi_provided_table[1]) +
                                     (j * efi_provided_table->descriptor_size));
        if (emat_item->PhysicalStart < base) {
          continue;
        }

        // EMAT items are ordered by physical address, so once we go past |base| we can quit the
        // loop.
        if (emat_item->PhysicalStart >= (base + size)) {
          break;
        }

        if (emat_item->PhysicalStart > base) {
          // Create a region for [base ... emat_item->PhysicalStart), because that region is not
          // covered by the EMAT.
          generated_items[item_count].PhysicalStart = base;
          generated_items[item_count].NumberOfPages = (emat_item->PhysicalStart - base) / PAGE_SIZE;
          generated_items[item_count].VirtualStart = 0;
          generated_items[item_count].Attribute = EFI_MEMORY_RUNTIME;
          generated_items[item_count].Type = item->Type;

          // Adjust base and size forward.
          size -= emat_item->PhysicalStart - base;
          base = emat_item->PhysicalStart;

          item_count++;
          if (item_count >= max_items) {
            printf("ran out of zbi scratch space!\n");
            return EFI_OUT_OF_RESOURCES;
          }
        }

        if (emat_item->PhysicalStart == base) {
          // Create a region for [base ... emat_item->NumberOfPages * PAGE_SIZE)
          generated_items[item_count] = *emat_item;

          // Adjust base and size forward.
          base += emat_item->NumberOfPages * PAGE_SIZE;
          size -= emat_item->NumberOfPages * PAGE_SIZE;
          item_count++;
          if (item_count >= max_items) {
            printf("ran out of zbi scratch space!\n");
            return EFI_OUT_OF_RESOURCES;
          }
        }
      }

      if (size != 0) {
        // Create a region for the remaining space represented by this mmap region.
        generated_items[item_count].PhysicalStart = base;
        generated_items[item_count].NumberOfPages = size / PAGE_SIZE;
        generated_items[item_count].VirtualStart = 0;
        generated_items[item_count].Attribute = EFI_MEMORY_RUNTIME;
        generated_items[item_count].Type = item->Type;
        item_count++;
        if (item_count >= max_items) {
          printf("ran out of scratch space!\n");
          return EFI_OUT_OF_RESOURCES;
        }
      }
    }
  }

  generated_header->descriptor_size = sizeof(efi_memory_descriptor);
  generated_header->number_of_entries = (uint32_t)item_count;
  generated_header->reserved = 0;
  generated_header->version = 1;

  zbi_result_t result = zbi_create_entry_with_payload(
      ramdisk, ramdisk_size, ZBI_TYPE_EFI_MEMORY_ATTRIBUTES_TABLE, 0, 0, generated_header,
      sizeof(*generated_header) + (item_count * sizeof(efi_memory_descriptor)));
  if (result != ZBI_RESULT_OK) {
    printf(
        "warning: failed to create EFI memory attributes ZBI item."
        " EFI runtime services won't work.\n");
    return EFI_INVALID_PARAMETER;
  }

  return EFI_SUCCESS;
}
