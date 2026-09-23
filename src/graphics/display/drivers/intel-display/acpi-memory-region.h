// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_ACPI_MEMORY_REGION_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_ACPI_MEMORY_REGION_H_

#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <span>

namespace intel_display {

// A subset of an ACPI custom Operation Region mapped into this process.
class AcpiMemoryRegion {
 public:
  // Creates a memory region of `region_size` bytes starting at `region_base`.
  //
  // `mmio_resource` must be of kind `ZX_RSRC_KIND_MMIO` and must have access
  // to [region_base, region_base + region_size).
  //
  // The `region_base` and `region_size` should refer to memory that is entirely
  // contained within a memory region in the system's ACPI (Advanced
  // Configuration and Power Interface) tables that is marked as NVS (saved
  // during the Non-Volatile Sleep state)
  static zx::result<AcpiMemoryRegion> Create(zx::unowned_resource mmio_resource,
                                             zx_paddr_t region_base, size_t region_size);

  // Creates an empty memory region without any backing VMO.
  constexpr AcpiMemoryRegion() = default;

  // Creates a representation of an already-mapped memory region.
  //
  // This constructor is exposed for testing convenience. Production usage
  // should prefer Create(), which handles mapping physical memory.
  //
  // If `region_vmo` is a valid VMO, the newly created instance keeps the VMO
  // alive throughout its life, and unmaps the pages that contain `region_data`
  // upon destruction.
  //
  // If `region_vmo` is not a valid VMO, the caller must ensure that the memory
  // backing `region_data` stays alive while the newly created instance exists.
  //
  // `region_data` must not be empty.
  explicit AcpiMemoryRegion(zx::vmo region_vmo, std::span<uint8_t> region_data);

  // Copying is not allowed.
  AcpiMemoryRegion(const AcpiMemoryRegion&) = delete;
  AcpiMemoryRegion& operator=(const AcpiMemoryRegion&) = delete;

  // Moving is allowed so AcpiMemoryRegion can be used as a return type.
  AcpiMemoryRegion(AcpiMemoryRegion&& rhs) noexcept;
  AcpiMemoryRegion& operator=(AcpiMemoryRegion&& rhs) noexcept;

  ~AcpiMemoryRegion();

  bool is_empty() const { return region_data_.empty(); }

  // The mapped memory. Empty iff this is an empty memory region.
  std::span<uint8_t> data() { return region_data_; }
  std::span<const uint8_t> data() const { return region_data_; }

  zx::unowned_vmo vmo_for_testing() const { return region_vmo_.borrow(); }

 private:
  std::span<uint8_t> region_data_;

  // Holds onto the VMO backing `region_data_`.
  //
  // The VMO may be invalid if `region_data_` is empty, or if the
  // AcpiMemoryRegion instance is created without a backing VMO.
  zx::vmo region_vmo_;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_ACPI_MEMORY_REGION_H_
