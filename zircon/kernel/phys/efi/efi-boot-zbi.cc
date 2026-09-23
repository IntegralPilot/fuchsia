// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "phys/efi/efi-boot-zbi.h"

#include <lib/memalloc/range.h>
#include <lib/zbitl/error-stdio.h>
#include <lib/zbitl/image.h>
#include <lib/zbitl/view.h>
#include <zircon/assert.h>

#include <ktl/utility.h>
#include <phys/allocation.h>
#include <phys/efi/page-size.h>
#include <phys/symbolize.h>
#include <phys/zbi.h>

#include <ktl/enforce.h>

fit::result<EfiBootZbi::Zbi::Error, EfiBootZbi::Zbi> EfiBootZbi::Load(
    uint32_t extra_data_capacity) {
  auto file_error = [](EfiFileZbi::Error error) {
    printf("%s: Cannot load ZBI from file: ", ProgramName());
    zbitl::PrintViewError(error);
    return fit::error{Zbi::Error{.zbi_error = error.zbi_error}};
  };

  auto it = file_zbi_.begin();
  if (it == file_zbi_.end()) {
    if (auto result = file_zbi_.take_error(); result.is_error()) {
      return file_error(result.error_value());
    }
    return fit::error{Zbi::Error{.zbi_error = "empty ZBI"}};
  }

  auto kernel_item = it;
  if (kernel_item->header->type != kArchZbiKernelType ||
      kernel_item->header->length < sizeof(zbi_kernel_t)) {
    file_zbi_.ignore_error();
    return fit::error{Zbi::Error{
        .zbi_error = "first item is not a valid kernel item",
    }};
  }

  ++it;

  zbi_kernel_t kernel_header;
  if (auto result = EfiFileZbi::Traits::Read(file_zbi_.storage(), kernel_item->payload,
                                             &kernel_header, sizeof(kernel_header));
      result.is_error()) {
    return file_error({
        .zbi_error = "cannot read kernel header",
        .storage_error = result.error_value(),
    });
  }

  auto zbi_copy = [this](size_t& alloc_size, memalloc::Type type, size_t alignment,
                         EfiFileZbi::iterator first,
                         EfiFileZbi::iterator last) -> fit::result<EfiFileZbi::Error, Allocation> {
    alloc_size = (alloc_size + kEfiPageSize - 1) & -kEfiPageSize;
    fbl::AllocChecker ac;
    Allocation buffer = Allocation::New(ac, type, alloc_size, alignment);
    if (!ac.check()) {
      return fit::error{EfiFileZbi::Error{.zbi_error = "out of memory"}};
    }

    auto result = file_zbi_.Copy(buffer.data(), first, last);
    if (result.is_error()) {
      return fit::error{EfiFileZbi::Error{
          .zbi_error = result.error_value().zbi_error,
          .item_offset = result.error_value().read_offset,
          .storage_error = result.error_value().read_error,
      }};
    }

    return fit::ok(ktl::move(buffer));
  };

  // Set up the in-memory kernel item as the "input ZBI" like Init() would do.
  // But allocate enough extra space after it to guarantee "in-place" loading.
  const size_t kernel_load_size =
      offsetof(ZbiKernelImage, data_kernel) + kernel_item->header->length;
  size_t kernel_alloc_size = kernel_load_size + kernel_header.reserve_memory_size;
  if (auto result = zbi_copy(kernel_alloc_size, memalloc::Type::kKernel, kArchZbiKernelAlignment,
                             kernel_item, it);
      result.is_ok()) {
    InitKernel(ktl::move(result).value());
    // Now the kernel is already deemed loaded "in place" as Load() would do.
    ZX_ASSERT(KernelCanLoadInPlace());
  } else {
    printf("%s: Cannot load kernel (%#zx bytes) into %#zx bytes space.\n", ProgramName(),
           kernel_load_size, kernel_alloc_size);
    file_zbi_.ignore_error();
    return file_error(result.error_value());
  }

  // If there appear to be no data items, it might have been an iteration error
  // after the kernel item.
  size_t data_items_size;
  if (it == file_zbi_.end()) {
    if (auto result = file_zbi_.take_error(); result.is_error()) {
      return file_error(result.error_value());
    }

    // Nope, the data ZBI really will start as an empty container.
    data_items_size = 0;
  } else {
    data_items_size = file_zbi_.size_bytes() - it.item_offset();
  }

  // Set up the in-memory data ZBI like Load() would do.
  const size_t data_zbi_size = sizeof(zbi_header_t) + data_items_size;
  size_t data_capacity = data_zbi_size + extra_data_capacity;
  if (auto result = zbi_copy(data_capacity, memalloc::Type::kDataZbi, kArchZbiDataAlignment, it,
                             file_zbi_.end());
      result.is_ok()) {
    InitData(ktl::move(result).value());
  } else {
    printf("%s: Cannot load data ZBI (%#zx) into %#zx bytes space.\n", ProgramName(), data_zbi_size,
           data_capacity);
    // If there were authentically no data items, then we did take_error()
    // above and can't do it again, even via ignore_error().  Otherwise, we
    // have the original iteration still open that needs to be cleaned up
    // though it won't have interesting error state since the copy's failure
    // includes all the error state we need.
    if (it != file_zbi_.end()) {
      file_zbi_.ignore_error();
    }
    return file_error(result.error_value());
  }

  return fit::ok(DataZbi());
}

fit::result<EfiBootZbi::Zbi::Error> EfiBootZbi::LastChance(const EfiBootZbi::Zbi& zbi) {
  DataZbi() = zbi;
  ZX_ASSERT(reinterpret_cast<zbi_header_t*>(DataZbi().storage().data())->length > 32);
  Log();
  return fit::ok();
}
