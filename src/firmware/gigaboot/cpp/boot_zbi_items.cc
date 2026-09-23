// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "boot_zbi_items.h"

#include <lib/ddk/platform-defs.h>
#include <lib/zbi-format/board.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbi-format/graphics.h>
#include <lib/zbi-format/memory.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbi/zbi.h>
#include <lib/zircon_boot/zbi_utils.h>
#include <lib/zx/result.h>
#include <stdio.h>
#include <stdlib.h>
#include <zircon/errors.h>
#include <zircon/limits.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <span>

#include <efi/boot-services.h>
#include <efi/protocol/graphics-output.h>
#include <efi/system-table.h>
#include <efi/types.h>
#include <fbl/vector.h>
#include <phys/efi/main.h>

#include "acpi.h"
#include "page_size.h"
#include "utils.h"

namespace gigaboot {

const efi_guid kSmbiosTableGUID = SMBIOS_TABLE_GUID;
const efi_guid kSmbios3TableGUID = SMBIOS3_TABLE_GUID;
const uint8_t kSmbiosAnchor[4] = {'_', 'S', 'M', '_'};
const uint8_t kSmbios3Anchor[5] = {'_', 'S', 'M', '3', '_'};

extern "C" efi_status generate_efi_memory_attributes_table_item(
    void* ramdisk, const size_t ramdisk_size, efi_system_table* sys, const void* mmap,
    size_t memory_map_size, size_t dsize);

namespace {

constexpr size_t kBufferSize = (static_cast<size_t>(32) * 1024) / 2;

template <typename T>
using MemArray = std::array<T, kBufferSize / sizeof(T)>;

template <typename T>
class MemDynamicViewer {
 public:
  class Iterator {
   public:
    const T& operator*() const { return *item_; }
    const T* operator->() const { return item_; }
    Iterator operator++() {
      item_ = reinterpret_cast<const T*>(reinterpret_cast<const uint8_t*>(item_) + item_size_);
      return *this;
    }

    friend bool operator==(const Iterator& a, const Iterator& b) { return a.item_ == b.item_; }
    friend bool operator!=(const Iterator& a, const Iterator& b) { return !(a == b); }

   private:
    friend MemDynamicViewer<T>;
    Iterator(const T* item, size_t item_size) : item_(item), item_size_(item_size) {}

    const T* item_;
    size_t item_size_;
  };

  MemDynamicViewer(const MemArray<T>& base, size_t num_items, size_t item_size)
      : base_(base.data()), num_items_(num_items), item_size_(item_size) {
    // Verify that iteration won't overrun the end of the backing array.
    ZX_ASSERT(num_items * item_size <= base.size() * sizeof(T));

    // Verify that we aren't violating alignment requirements.
    ZX_ASSERT(item_size % std::alignment_of_v<T> == 0);
  }

  Iterator begin() const { return Iterator(base_, item_size_); }
  Iterator end() const {
    // The casting and pointer arithmetic on uint8_t* is necessary
    // because item_size_ may not equal sizeof(T).
    // That is the whole point of this custom iterator.
    return Iterator(reinterpret_cast<const T*>(reinterpret_cast<const uint8_t*>(base_) +
                                               num_items_ * item_size_),
                    item_size_);
  }

 private:
  const T* base_;
  size_t num_items_;
  size_t item_size_;
};

bool AppendSmbiosPtr(zbi_header_t* image, size_t capacity) {
  uint64_t smbios = 0;
  std::span<const efi_configuration_table> entries(gEfiSystemTable->ConfigurationTable,
                                                   gEfiSystemTable->NumberOfTableEntries);

  for (const efi_configuration_table& entry : entries) {
    if ((entry.VendorGuid == kSmbiosTableGUID &&
         !memcmp(entry.VendorTable, kSmbiosAnchor, sizeof(kSmbiosAnchor))) ||
        (entry.VendorGuid == kSmbios3TableGUID &&
         !memcmp(entry.VendorTable, kSmbios3Anchor, sizeof(kSmbios3Anchor)))) {
      smbios = reinterpret_cast<uint64_t>(entry.VendorTable);
      break;
    }
  }

  if (smbios == 0) {
    return false;
  }

  return zbi_create_entry_with_payload(image, capacity, ZBI_TYPE_SMBIOS, 0, 0, &smbios,
                                       sizeof(smbios)) == ZBI_RESULT_OK;
}

bool AppendAcpiRsdp(zbi_header_t* image, size_t capacity, const AcpiRsdp& rsdp) {
  const auto* ptr = &rsdp;
  if (zbi_result_t result = zbi_create_entry_with_payload(image, capacity, ZBI_TYPE_ACPI_RSDP, 0, 0,
                                                          &ptr, sizeof(&ptr));
      result != ZBI_RESULT_OK) {
    printf("Failed to create ACPI rsdp entry, %d\n", result);
    return false;
  }

  return true;
}

zbi_pixel_format_t PixelFormatFromBitmask(const efi_pixel_bitmask& bitmask) {
  struct entry {
    efi_pixel_bitmask mask;
    zbi_pixel_format_t pixel_format;
  };
  // Ignore reserved field
  constexpr entry entries[] = {
      {.mask = {.RedMask = 0xFF0000, .GreenMask = 0xFF00, .BlueMask = 0xFF},
       .pixel_format = ZBI_PIXEL_FORMAT_RGB_X888},
      {.mask = {.RedMask = 0xE0, .GreenMask = 0x1C, .BlueMask = 0x3},
       .pixel_format = ZBI_PIXEL_FORMAT_RGB_332},
      {.mask = {.RedMask = 0xF800, .GreenMask = 0x7E0, .BlueMask = 0x1F},
       .pixel_format = ZBI_PIXEL_FORMAT_RGB_565},
      {.mask = {.RedMask = 0xC0, .GreenMask = 0x30, .BlueMask = 0xC},
       .pixel_format = ZBI_PIXEL_FORMAT_RGB_2220},
  };

  auto equal_p = [&bitmask](const entry& e) -> bool {
    // Ignore reserved
    return bitmask.RedMask == e.mask.RedMask && bitmask.GreenMask == e.mask.GreenMask &&
           bitmask.BlueMask == e.mask.BlueMask;
  };

  auto res = std::find_if(std::begin(entries), std::end(entries), equal_p);
  if (res == std::end(entries)) {
    printf("unsupported pixel format bitmask: r %08x / g %08x / b %08x\n", bitmask.RedMask,
           bitmask.GreenMask, bitmask.BlueMask);
    return ZBI_PIXEL_FORMAT_NONE;
  }

  return res->pixel_format;
}

uint32_t GetZbiPixelFormat(efi_graphics_output_mode_information* info) {
  efi_graphics_pixel_format efi_fmt = info->PixelFormat;
  switch (efi_fmt) {
    case PixelBlueGreenRedReserved8BitPerColor:
      return ZBI_PIXEL_FORMAT_RGB_X888;
    case PixelBitMask:
      return PixelFormatFromBitmask(info->PixelInformation);
    default:
      printf("unsupported pixel format %d!\n", efi_fmt);
      return ZBI_PIXEL_FORMAT_NONE;
  }
}

// If the firmware supports graphics, append
// framebuffer information to the list of zbi items.
//
// Returns true if there is no graphics support or if framebuffer information
// was successfully added, and false if appending framebuffer information
// returned an error.
bool AddFramebufferIfSupported(zbi_header_t* image, size_t capacity) {
  auto graphics_protocol = gigaboot::EfiLocateProtocol<efi_graphics_output_protocol>();
  if (graphics_protocol.is_error() || graphics_protocol.value() == nullptr) {
    // Graphics are not strictly necessary.
    printf("No valid graphics output detected\n");
    return true;
  }

  zbi_swfb_t framebuffer = {
      .base = graphics_protocol.value()->Mode->FrameBufferBase,
      .width = graphics_protocol.value()->Mode->Info->HorizontalResolution,
      .height = graphics_protocol.value()->Mode->Info->VerticalResolution,
      .stride = graphics_protocol.value()->Mode->Info->PixelsPerScanLine,
      .format = GetZbiPixelFormat(graphics_protocol.value()->Mode->Info),
  };
  if (zbi_result_t result = zbi_create_entry_with_payload(image, capacity, ZBI_TYPE_FRAMEBUFFER, 0,
                                                          0, &framebuffer, sizeof(framebuffer));
      result != ZBI_RESULT_OK) {
    printf("Failed to add framebuffer zbi item: %d\n", result);
    return false;
  }

  return true;
}

bool AddSystemTable(zbi_header_t* image, size_t capacity) {
  return zbi_create_entry_with_payload(image, capacity, ZBI_TYPE_EFI_SYSTEM_TABLE, 0, 0,
                                       &gEfiSystemTable, sizeof(gEfiSystemTable)) == ZBI_RESULT_OK;
}

bool AddUartDriver(zbi_header_t* image, size_t capacity, const AcpiRsdp& rsdp,
                   ZbiContext* context) {
  const auto* spcr = rsdp.LoadTable<AcpiSpcr>();
  if (spcr == nullptr) {
    printf("%s: no spcr\n", __func__);
    return true;
  }

  uint32_t serial_driver_type = spcr->GetKdrv();
  if (serial_driver_type == 0) {
    printf("%s: no serial driver\n", __func__);
    return true;
  }

  zbi_dcfg_simple_t uart_driver = spcr->DeriveUartDriver();
  if (context) {
    context->uart_mmio_phys = uart_driver.mmio_phys;
  }

  return zbi_create_entry_with_payload(image, capacity, ZBI_TYPE_KERNEL_DRIVER, serial_driver_type,
                                       0, &uart_driver, sizeof(uart_driver)) == ZBI_RESULT_OK;
}

bool AddMadtItems(zbi_header_t* image, size_t capacity, const AcpiRsdp& rsdp, ZbiContext* context) {
  const auto* madt = rsdp.LoadTable<AcpiMadt>();
  if (madt == nullptr) {
    printf("%s: no madt\n", __func__);
    return true;
  }

  std::array<zbi_topology_node_t, 1> nodes;
  auto node_span = madt->GetTopology(nodes);
  if (!node_span.empty() && zbi_create_entry_with_payload(
                                image, capacity, ZBI_TYPE_CPU_TOPOLOGY, sizeof(node_span.front()),
                                0, node_span.data(), node_span.size_bytes()) != ZBI_RESULT_OK) {
    return false;
  }
  if (context) {
    context->num_cpu_nodes = static_cast<uint8_t>(node_span.size());
  }

  std::optional<AcpiMadt::GicDescriptor> gic_cfg = madt->GetGicDriver();
  if (!gic_cfg) {
    printf("%s: no gic cfg\n", __func__);
    return true;
  }

  if (context) {
    context->gic_driver = gic_cfg->driver;
  }

  return std::visit(
             [image, capacity, zbi_type = gic_cfg->zbi_type](const auto& driver) {
               return zbi_create_entry_with_payload(image, capacity, ZBI_TYPE_KERNEL_DRIVER,
                                                    zbi_type, 0, &driver, sizeof(driver));
             },
             gic_cfg->driver) == ZBI_RESULT_OK;
}

bool AddPsciDriver(zbi_header_t* image, size_t capacity, const AcpiRsdp& rsdp) {
  const auto* fadt = rsdp.LoadTable<AcpiFadt>();
  if (!fadt) {
    printf("%s: no fadt\n", __func__);
    return true;
  }

  std::optional<zbi_dcfg_arm_psci_driver_t> psci_driver = fadt->GetPsciDriver();
  if (!psci_driver) {
    printf("%s: no psci\n", __func__);
    return true;
  }

  return zbi_create_entry_with_payload(image, capacity, ZBI_TYPE_KERNEL_DRIVER,
                                       ZBI_KERNEL_DRIVER_ARM_PSCI, 0, &(psci_driver.value()),
                                       sizeof(*psci_driver)) == ZBI_RESULT_OK;
}

bool AddArmTimerDriver(zbi_header_t* image, size_t capacity, const AcpiRsdp& rsdp) {
  const auto* gtdt = rsdp.LoadTable<AcpiGtdt>();
  if (!gtdt) {
    printf("%s: no gtdt\n", __func__);
    return true;
  }

  zbi_dcfg_arm_generic_timer_driver_t timer = gtdt->GetTimer();
  return zbi_create_entry_with_payload(image, capacity, ZBI_TYPE_KERNEL_DRIVER,
                                       ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER, 0, &timer,
                                       sizeof(timer)) == ZBI_RESULT_OK;
}

bool AddPlatformId(zbi_header_t* image, size_t capacity) {
  zbi_platform_id_t platform_id = {
#ifdef __x86_64__
      .vid = PDEV_VID_INTEL,
      .pid = PDEV_PID_X86,
#elif __aarch64__
      .vid = PDEV_VID_ARM,
      .pid = PDEV_PID_ACPI_BOARD,
#else
#error "Uknown platform architecture."
#endif
  };

  return zbi_create_entry_with_payload(image, capacity, ZBI_TYPE_PLATFORM_ID, 0, 0, &platform_id,
                                       sizeof(platform_id)) == ZBI_RESULT_OK;
}

// Bootloader file item for ssh key provisioning
// TODO(b/239088231): Consider using dynamic allocation.
constexpr size_t kZbiFileLength = 4096;
bool zbi_file_is_initialized = false;
uint8_t zbi_files[kZbiFileLength] __attribute__((aligned(ZBI_ALIGNMENT)));

}  // namespace

zbi_result_t AddBootloaderFiles(const char* name, const void* data, size_t len) {
  if (!zbi_file_is_initialized) {
    zbi_result_t result = zbi_init(zbi_files, kZbiFileLength);
    if (result != ZBI_RESULT_OK) {
      printf("Failed to initialize zbi_files: %d\n", result);
      return result;
    }
    zbi_file_is_initialized = true;
  }

  return AppendZbiFile(reinterpret_cast<zbi_header_t*>(zbi_files), kZbiFileLength, name, data, len);
}

void ClearBootloaderFiles() { zbi_file_is_initialized = false; }

std::span<uint8_t> GetZbiFiles() { return zbi_files; }

zx::result<std::span<zbi_mem_range_t>> CollectPeripheralMemoryItems(
    const ZbiContext* context, std::span<zbi_mem_range_t>& out) {
  if (!context) {
    return zx::ok(out.first(0));
  }

  auto current_zbi = out.begin();
  if (context->uart_mmio_phys) {
    if (current_zbi == out.end()) {
      printf("Insufficient memory to add memory items\n");
      return zx::error(ZX_ERR_NO_MEMORY);
    }
    *current_zbi++ = {
        .paddr = context->uart_mmio_phys.value(),
        .length = PAGE_SIZE,
        .type = ZBI_MEM_TYPE_PERIPHERAL,
    };
  }

  if (context->gic_driver) {
    if (const auto* v2_driver =
            std::get_if<zbi_dcfg_arm_gic_v2_driver_t>(&context->gic_driver.value())) {
      // This memory range must encompass the GICC and GICD register ranges.
      // Each of these generally encompass a page, but some systems like QEMU
      // allocate 64K to make it easier when working with 64kb pages. Since we
      // use 4K pages, we allocate 16 pages here just to be safe.
      constexpr uint64_t entry_length = 16 * PAGE_SIZE;
      if (current_zbi == out.end()) {
        printf("Insufficient memory to add memory items\n");
        return zx::error(ZX_ERR_NO_MEMORY);
      }

      *current_zbi++ = {
          .paddr = v2_driver->mmio_phys,
          .length = entry_length,
          .type = ZBI_MEM_TYPE_PERIPHERAL,
      };

      if (current_zbi == out.end()) {
        printf("Insufficient memory to add memory items\n");
        return zx::error(ZX_ERR_NO_MEMORY);
      }

      *current_zbi++ = {
          .paddr = v2_driver->mmio_phys + v2_driver->gicd_offset + v2_driver->gicc_offset,
          .length = entry_length,
          .type = ZBI_MEM_TYPE_PERIPHERAL,
      };

      if (v2_driver->use_msi) {
        if (current_zbi == out.end()) {
          printf("Insufficient memory to add memory items\n");
          return zx::error(ZX_ERR_NO_MEMORY);
        }

        *current_zbi++ = {
            .paddr = v2_driver->msi_frame_phys,
            .length = entry_length,
            .type = ZBI_MEM_TYPE_PERIPHERAL,
        };
      }
    } else if (const auto* v3_driver =
                   std::get_if<zbi_dcfg_arm_gic_v3_driver_t>(&context->gic_driver.value())) {
      // We should never have a GICv3 system with less than one core.
      if (context->num_cpu_nodes < 1) {
        return zx::error(ZX_ERR_INTERNAL);
      }

      if (current_zbi == out.end()) {
        printf("Insufficient memory to add memory items\n");
        return zx::error(ZX_ERR_NO_MEMORY);
      }

      // This memory range must encompass the GICD and GICR register ranges.
      uint64_t gic_mem_size = 0x10000;  // GICD size.
      gic_mem_size += v3_driver->gicr_offset + v3_driver->gicd_offset;
      // Add the GICR size. Each GICR in GICv3 consists of 2 adjacent 64 KiB frames.
      gic_mem_size += static_cast<uint64_t>(context->num_cpu_nodes) * 0x20000;
      // Add any padding between GICRs on multi-core systems.
      gic_mem_size += (context->num_cpu_nodes - 1) * v3_driver->gicr_stride;
      *current_zbi++ = {
          .paddr = v3_driver->mmio_phys,
          .length = gic_mem_size,
          .type = ZBI_MEM_TYPE_PERIPHERAL,
      };
    }
  }

  return zx::ok(out.first(current_zbi - out.begin()));
}

// Add memory related zbi items.
//
// Returns memory map key on success, which will be used for ExitBootService.
zx::result<size_t> AddMemoryItems(void* zbi, size_t capacity, const ZbiContext* context) {
  static MemArray<zbi_mem_range_t> zbi_mem = {};
  static MemArray<efi_memory_descriptor> efi_mem = {};

  uint32_t dversion = 0;
  size_t mkey = 0;
  size_t dsize = 0;
  size_t msize = sizeof(efi_mem);
  // Note: Once memory map is grabbed, do not do anything that can change it, i.e. anything that
  // involves memory allocation/de-allocation, including printf if it is printing to graphics.
  efi_status status =
      gEfiSystemTable->BootServices->GetMemoryMap(&msize, efi_mem.data(), &mkey, &dsize, &dversion);
  if (status != EFI_SUCCESS) {
    printf("boot: cannot GetMemoryMap(). %s\n", EfiStatusToString(status));
    return zx::error(ZX_ERR_INTERNAL);
  }

  // Look for an EFI memory attributes table we can pass to the kernel.
  efi_status mem_attr_res = generate_efi_memory_attributes_table_item(
      zbi, capacity, gEfiSystemTable, efi_mem.data(), msize, dsize);
  if (mem_attr_res != EFI_SUCCESS) {
    printf("failed to generate EFI memory attributes table: %s", EfiStatusToString(mem_attr_res));
    return zx::error(ZX_ERR_INTERNAL);
  }

  // The structures populated by GetMemoryMap may be larger than sizeof(efi_memory_descriptor),
  // and we need to handle that potential difference in a dynamic manner.
  MemDynamicViewer<efi_memory_descriptor> efi_mem_range(efi_mem, msize / dsize, dsize);
  auto current_zbi = zbi_mem.begin();
  for (const efi_memory_descriptor& efi_desc : efi_mem_range) {
    if (current_zbi == zbi_mem.end()) {
      break;
    }

    *current_zbi++ = {
        .paddr = efi_desc.PhysicalStart,
        .length = efi_desc.NumberOfPages * kUefiPageSize,
        .type = EfiToZbiMemRangeType(efi_desc.Type),
    };
  }

  std::span<zbi_mem_range_t> peripheral_mems = std::span{current_zbi, zbi_mem.end()};
  auto res = CollectPeripheralMemoryItems(context, peripheral_mems);
  if (res.is_error()) {
    return res.take_error();
  }
  current_zbi += (*res).size();

  size_t payload_size = (current_zbi - zbi_mem.begin()) * sizeof(*current_zbi);
  zbi_result_t result = zbi_create_entry_with_payload(zbi, capacity, ZBI_TYPE_MEM_CONFIG, 0, 0,
                                                      zbi_mem.data(), payload_size);
  if (result != ZBI_RESULT_OK) {
    printf("Failed to create memory range entry, %d\n", result);
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(mkey);
}

bool AddGigabootZbiItems(zbi_header_t* image, size_t capacity, const AbrSlotIndex* slot,
                         ZbiContext* context) {
  if (slot && AppendCurrentSlotZbiItem(image, capacity, *slot) != ZBI_RESULT_OK) {
    return false;
  }

  const AcpiRsdp* rsdp = FindAcpiRsdp();
  if (rsdp == nullptr) {
    return false;
  }

  if (!AppendAcpiRsdp(image, capacity, *rsdp)) {
    return false;
  }

  if (!AddUartDriver(image, capacity, *rsdp, context)) {
    return false;
  }

  if (!AddMadtItems(image, capacity, *rsdp, context)) {
    return false;
  }

  if (!AddPsciDriver(image, capacity, *rsdp)) {
    return false;
  }

  if (!AddArmTimerDriver(image, capacity, *rsdp)) {
    return false;
  }

  if (!AddFramebufferIfSupported(image, capacity)) {
    return false;
  }

  if (!AddSystemTable(image, capacity)) {
    return false;
  }

  if (!AppendSmbiosPtr(image, capacity)) {
    return false;
  }

  if (!AddPlatformId(image, capacity)) {
    return false;
  }

  if (zbi_file_is_initialized && zbi_extend(image, capacity, zbi_files) != ZBI_RESULT_OK) {
    return false;
  }

  return true;
}

}  // namespace gigaboot
