// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/stdcompat/source_location.h>
#include <lib/unittest/unittest.h>
#include <lib/zbi-format/kernel.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/view.h>
#include <stdio.h>
#include <zircon/assert.h>

#include <cstdint>

#include <fbl/algorithm.h>
#include <ktl/iterator.h>
#include <phys/address-space.h>
#include <phys/allocation.h>
#include <phys/boot-zbi.h>
#include <phys/zbi.h>

#include "turducken.h"

namespace {

bool gAnyError = false;
bool gTestError = false;

// RAII to mark start/end of a test, including whether it failed or passed.
struct TestTracker {
  explicit TestTracker(const char* name) : name(name) {
    gTestError = false;
    printf("[ RUNNING ] BootZbiLoadTest.%s\n", name);
  }

  ~TestTracker() {
    if (gTestError) {
      gAnyError = true;
      printf("[  FAILED ] BootZbiLoadTest.%s\n", name);
    } else {
      printf("[  PASSED ] BootZbiLoadTest.%s\n", name);
    }
  }

  const char* name = nullptr;
};

// Helper that returns a deferred call to `ignore_error` on a zbitl compatible iterator.
[[nodiscard]] auto zbitl_cleanup(auto& zbi) {
  return fit::defer([&]() { zbi.ignore_error(); });
}

using InputZbi = TurduckenTestBase::Zbi;

// ImageModifier takes as an argument zbitl::Image<ktl::span<ktl::byte>> of the base image
// generated here
Allocation AllocateZbiForTest(InputZbi zbi, size_t extra_capacity = 0,
                              size_t alignment = kArchZbiDataAlignment,
                              size_t prepend_discard_item_size = 0) {
  // Given an InputZbi that may have additional items injected from the environment in which it was
  // built, we want to extract the kernel item and the cmdline item. Additionally we allocate up to
  // `extra_capacity` bytes, and proceed to hand it over to `image_mod` that may append additional
  // content for test specific purposes.

  auto cleanup = zbitl_cleanup(zbi);

  auto kernel_it = zbi.find(kArchZbiKernelType);
  ZX_ASSERT_MSG(kernel_it != zbi.end(), "InputZbi did not contain a KERNEL item.");

  auto cmdline_it = zbi.find(ZBI_TYPE_CMDLINE);
  ZX_ASSERT_MSG(cmdline_it != zbi.end(), "InputZbi did not contain a CMDLINE item.");

  // The sum of the payloads and their headers, plus the ZBI_CONTAINER header and the extra bytes
  // requested. Additionally, make sure that `extra_capacity` counts from the start of the next
  // item.
  size_t zbi_size = fbl::round_up(kernel_it->header->length + cmdline_it->header->length +
                                      (3 * sizeof(zbi_header_t)),
                                  ZBI_ALIGNMENT) +
                    extra_capacity;
  if (prepend_discard_item_size > 0) {
    zbi_size += sizeof(zbi_header_t) + fbl::round_up(prepend_discard_item_size, ZBI_ALIGNMENT);
  }
  fbl::AllocChecker ac;
  Allocation zbi_alloc = Allocation::New(ac, memalloc::Type::kDataZbi, zbi_size, alignment);
  ZX_ASSERT_MSG(ac.check(), "Failed to allocate ZBI.");

  zbitl::Image<ktl::span<ktl::byte>> zbi_for_test(zbi_alloc.data());
  ZX_ASSERT(zbi_for_test.clear().is_ok());
  if (prepend_discard_item_size > 0) {
    ZX_ASSERT(zbi_for_test
                  .Append({
                      .type = ZBI_TYPE_DISCARD,
                      .length = static_cast<uint32_t>(prepend_discard_item_size),
                      .extra = 0,
                      .flags = 0,
                      .magic = ZBI_ITEM_MAGIC,
                  })
                  .is_ok());
  }

  ZX_ASSERT(zbi_for_test.Extend(kernel_it, ktl::next(kernel_it)).is_ok());
  ZX_ASSERT(zbi_for_test.Extend(cmdline_it, ktl::next(cmdline_it)).is_ok());

  return zbi_alloc;
}

// Shrink `zbi_alloc` to the size of the zbi contained. Trailing bytes can be optionally allocated
// by setting `allocate_trailing_bytes` to true, which will return the trailing bytes allocation.
ktl::optional<Allocation> ShrinkZbiToFit(Allocation& zbi_alloc, bool round_to_page = false,
                                         bool allocate_trailing_bytes = false) {
  zbitl::View<ktl::span<ktl::byte>> zbi(zbi_alloc.data());
  size_t alloc_size = zbi_alloc->size_bytes();

  fbl::AllocChecker ac;
  size_t zbi_size_bytes =
      round_to_page ? fbl::round_up(zbi.size_bytes(), Allocation::PageSize()) : zbi.size_bytes();

  zbi_alloc.Resize(ac, zbi_size_bytes);
  ZX_ASSERT_MSG(
      ac.check(),
      "Failed to shrink zbi allocation(%#zx) to match the size of the zbi(%#zx) rounded to %zx.\n",
      zbi_alloc->size_bytes(), zbi.size_bytes(), zbi_size_bytes);

  if (allocate_trailing_bytes) {
    size_t trailing_bytes_length = alloc_size - zbi.size_bytes();
    uint64_t trailing_bytes_addr =
        reinterpret_cast<uint64_t>(zbi_alloc->data() + zbi_alloc.size_bytes());
    Allocation trailing_bytes_alloc =
        Allocation::New(ac, memalloc::Type::kPhysScratch, trailing_bytes_length, 8,
                        trailing_bytes_addr, trailing_bytes_addr + trailing_bytes_length);
    ZX_ASSERT_MSG(ac.check(),
                  "Failed to allocated trailing bytes of ZBI Alloc after shrink to fit.");
    return trailing_bytes_alloc;
  }
  return ktl::nullopt;
}

// Just returns the iterators to the kernel and data items in the input zbi, and calls Init and
// Load. Just wraps iterator cleanup and shared setup among all tests cases.
BootZbi BootZbiInitAndLoad(BootZbi::InputZbi input_zbi, uint32_t extra_bytes) {
  {
    auto clean_input_zbi_iters = zbitl_cleanup(input_zbi);

    auto kernel_item_it = input_zbi.find(kArchZbiKernelType);
    ZX_ASSERT(kernel_item_it != input_zbi.end());

    auto data_item_it = ktl::next(kernel_item_it);
    ZX_ASSERT(data_item_it != input_zbi.end());
  }
  BootZbi boot_zbi;
  ZX_ASSERT(boot_zbi.Init(input_zbi).is_ok());
  ZX_ASSERT(boot_zbi.Load(extra_bytes).is_ok());
  return boot_zbi;
}

size_t alloc_end(ktl::span<const ktl::byte> alloc) {
  return reinterpret_cast<size_t>(alloc.data() + alloc.size_bytes());
}

size_t alloc_end(const Allocation& alloc) { return alloc_end(alloc.data()); }

bool CannotLoadKernelInPlaceDataZbiFitsWithinAllocation(InputZbi zbi) {
  BEGIN_TEST;
  TestTracker tracker("CannotLoadKernelInPlaceDataZbiFitsWithinAllocation");
  // By using the data alignment, the kernel alignment wont work, leading to a forced relocation of
  // the kernel.
  Allocation zbi_alloc;
  // Prepend 1 discard item of 1 byte long, so the kernel item is no longer aligned, and forces
  // relocation.
  zbi_alloc = AllocateZbiForTest(zbi, Allocation::PageSize(), kArchZbiKernelAlignment, 1);
  // No allocation should be returned, since we did not request the trailing bytes to be allocated.
  ZX_ASSERT(!ShrinkZbiToFit(zbi_alloc, true).has_value());
  // `zbi_alloc` contains a valid ZBI whose current allocation capacity should allow it to include
  // at most one extra page.
  zbitl::Image<ktl::span<ktl::byte>> image(zbi_alloc.data());
  // At most PageSize() - 1 extra bytes.
  ASSERT_TRUE(zbi_alloc->size_bytes() - image.size_bytes() < Allocation::PageSize());
  ASSERT_TRUE(zbi_alloc->size_bytes() >= image.size_bytes());
  // Before any modifications are applied to input zbi.
  auto image_cleanup = zbitl_cleanup(image);
  auto input_kernel_it = image.find(kArchZbiKernelType);
  ASSERT_TRUE(input_kernel_it != image.end());
  auto input_data_it = ktl::next(input_kernel_it);
  ASSERT_TRUE(input_data_it != image.end());

  ktl::byte* input_zbi_data_address = input_data_it->payload.data();
  // From the start of the data item, there this many bytes left.
  size_t input_alloc_end = fbl::round_up(alloc_end(zbi_alloc), Allocation::PageSize());

  // Loading the ZBI and asking for at least an extra page, should not result in a reallocation,
  // meaning the data zbi should be in place, with proper container headers added, etc.
  BootZbi boot_zbi = BootZbiInitAndLoad(BootZbi::InputZbi{zbi_alloc.release()}, 0);
  // Check the kernel if the kernel is relocated, outside the Data ZBI.
  auto input_zbi = boot_zbi.input_zbi();
  auto& loaded_zbi = boot_zbi.DataZbi();
  uint64_t kernel_load_address = boot_zbi.KernelLoadAddress();
  // This should not be within the input zbi's memory range.
  EXPECT_TRUE(kernel_load_address < reinterpret_cast<uint64_t>(input_zbi.storage().data()) ||
              (reinterpret_cast<uint64_t>(input_zbi.storage().data()) +
               input_zbi.storage().size_bytes()) <= kernel_load_address);

  // Data ZBI is in the same place.
  auto loaded_zbi_cleanup = zbitl_cleanup(loaded_zbi);
  auto loaded_data_it = loaded_zbi.find(ZBI_TYPE_CMDLINE);
  ASSERT_TRUE(loaded_data_it != loaded_zbi.end());
  ASSERT_TRUE(input_zbi_data_address == loaded_data_it->payload.data());
  // The zbi_alloc must not increase beyond the containing page bound.
  ASSERT_TRUE(alloc_end(loaded_zbi.storage()) == input_alloc_end);
  END_TEST;
}

bool CannotLoadKernelInPlaceDataZbiDoesNotFitsWithinAllocation(InputZbi zbi) {
  BEGIN_TEST;
  TestTracker tracker("CannotLoadKernelInPlaceDataZbiDoesNotFitsWithinAllocation");
  // By using the data alignment, the kernel alignment wont work, leading to a forced relocation of
  // the kernel.
  Allocation zbi_alloc;
  // Prepend 1 discard item of 1 byte long, so the kernel item is no longer aligned, and forces
  // relocation.
  zbi_alloc = AllocateZbiForTest(zbi, 2 * Allocation::PageSize(), kArchZbiKernelAlignment, 1);
  // No allocation should be returned, since we did not request the trailing bytes to be allocated.
  ZX_ASSERT(!ShrinkZbiToFit(zbi_alloc, true));

  // `zbi_alloc` contains a valid ZBI whose current allocation capacity should allow it to include
  // at most one extra page.
  zbitl::Image<ktl::span<ktl::byte>> image(zbi_alloc.data());
  // At most PageSize() - 1 extra bytes.
  ASSERT_TRUE(zbi_alloc->size_bytes() - image.size_bytes() < Allocation::PageSize());
  ASSERT_TRUE(zbi_alloc->size_bytes() >= image.size_bytes());
  // Before any modifications are applied to input zbi.
  auto image_cleanup = zbitl_cleanup(image);
  auto input_kernel_it = image.find(kArchZbiKernelType);
  ASSERT_TRUE(input_kernel_it != image.end());
  auto input_data_it = ktl::next(input_kernel_it);
  ASSERT_TRUE(input_data_it != image.end());

  ktl::byte* input_zbi_data_address = input_data_it->payload.data();
  // From the start of the data item, there this many bytes left.
  size_t input_alloc_end = fbl::round_up(alloc_end(zbi_alloc), Allocation::PageSize());

  // Loading the ZBI and asking for at least an extra page, should not result in a reallocation,
  // meaning the data zbi should be in place, with proper container headers added, etc.
  BootZbi boot_zbi = BootZbiInitAndLoad(BootZbi::InputZbi{zbi_alloc.release()},
                                        static_cast<uint32_t>(2 * Allocation::PageSize()));

  // Check the kernel if the kernel is relocated, outside the Data ZBI.
  auto input_zbi = boot_zbi.input_zbi();
  auto& loaded_zbi = boot_zbi.DataZbi();

  uint64_t kernel_load_address = boot_zbi.KernelLoadAddress();

  // This should not be within the input zbi's memory range.
  EXPECT_TRUE(kernel_load_address < reinterpret_cast<uint64_t>(input_zbi.storage().data()) ||
              (reinterpret_cast<uint64_t>(input_zbi.storage().data()) +
               input_zbi.storage().size_bytes()) <= kernel_load_address);

  // Data ZBI is in the same place.
  auto loaded_zbi_cleanup = zbitl_cleanup(loaded_zbi);
  auto loaded_data_it = loaded_zbi.find(ZBI_TYPE_CMDLINE);
  ASSERT_TRUE(loaded_data_it != loaded_zbi.end());
  ASSERT_TRUE(input_zbi_data_address == loaded_data_it->payload.data());

  // The zbi_alloc must not increase beyond the containing page bound plus the two extra pages
  // allcated at the end, as requested for `BootZbiInitAndLoad`.
  ASSERT_TRUE(alloc_end(loaded_zbi.storage()) == input_alloc_end + 2 * Allocation::PageSize());
  END_TEST;
}

bool CannotLoadKernelInPlaceDataZbiDoesNotFitInPlace(InputZbi zbi) {
  BEGIN_TEST;
  TestTracker tracker("CannotLoadKernelInPlaceDataZbiDoesNotFitInPlace");
  // By using the data alignment, the kernel alignment wont work, leading to a forced relocation of
  // the kernel.
  Allocation zbi_alloc;
  // Prepend 1 discard item of 1 byte long, so the kernel item is no longer aligned, and forces
  // relocation.
  zbi_alloc = AllocateZbiForTest(zbi, 2 * Allocation::PageSize(), kArchZbiKernelAlignment, 1);
  // By allocating the tail, we will fail to allocate in place (at least PageSize extra bytes)
  // and the data should be reallocated as well.
  ktl::optional tail = ShrinkZbiToFit(zbi_alloc, true, true);
  ASSERT_TRUE(tail.has_value());
  // `zbi_alloc` contains a valid ZBI whose current allocation capacity should allow it to include
  // at most one extra page.
  zbitl::Image<ktl::span<ktl::byte>> image(zbi_alloc.data());
  // At most PageSize() - 1 extra bytes.
  ASSERT_TRUE(zbi_alloc->size_bytes() - image.size_bytes() < Allocation::PageSize());
  ASSERT_TRUE(zbi_alloc->size_bytes() >=
              image.size_bytes());  // Before any modifications are applied to input zbi.
  auto image_cleanup = zbitl_cleanup(image);
  auto input_kernel_it = image.find(kArchZbiKernelType);
  ASSERT_TRUE(input_kernel_it != image.end());
  auto input_data_it = ktl::next(input_kernel_it);
  ASSERT_TRUE(input_data_it != image.end());

  ktl::byte* input_zbi_data_address = input_data_it->payload.data();
  // Loading the ZBI and asking for at least an extra page, should not result in a reallocation,
  // meaning the data zbi should be in place, with proper container headers added, etc.
  BootZbi boot_zbi = BootZbiInitAndLoad(BootZbi::InputZbi{zbi_alloc.release()},
                                        static_cast<uint32_t>(Allocation::PageSize()));

  // Check the kernel if the kernel is relocated, outside the Data ZBI.
  auto input_zbi = boot_zbi.input_zbi();
  auto& loaded_zbi = boot_zbi.DataZbi();

  uint64_t kernel_load_address = boot_zbi.KernelLoadAddress();

  // This should not be within the input zbi's memory range.
  EXPECT_TRUE(kernel_load_address < reinterpret_cast<uint64_t>(input_zbi.storage().data()) ||
              (reinterpret_cast<uint64_t>(input_zbi.storage().data()) +
               input_zbi.storage().size_bytes()) <= kernel_load_address);
  // Data ZBI is not in the same place.
  auto loaded_zbi_cleanup = zbitl_cleanup(loaded_zbi);
  auto loaded_data_it = loaded_zbi.find(ZBI_TYPE_CMDLINE);
  ASSERT_TRUE(loaded_data_it != loaded_zbi.end());
  ASSERT_TRUE(input_zbi_data_address != loaded_data_it->payload.data());

  // Now check payloads match.
  ASSERT_TRUE(input_data_it->header->length == loaded_data_it->header->length);
  EXPECT_TRUE(memcmp(input_data_it->payload.data(), loaded_data_it->payload.data(),
                     loaded_data_it->payload.size()) == 0);
  END_TEST;
}

bool CanLoadKernelInPlaceDataZbiDoesNotFitInPlace(InputZbi zbi) {
  BEGIN_TEST;
  TestTracker tracker("CannotLoadKernelInPlaceDataZbiDoesNotFitInPlace");
  // By using the data alignment, the kernel alignment wont work, leading to a forced relocation of
  // the kernel.
  Allocation zbi_alloc;
  // The kernels is properly aligned, lets include some extra items so the the memory requirements
  // can be met with this allocation. We can do that by:
  // * Adding extra items on the tail to make data big enough to contain the kernel load size.
  // * Allocate a tail to prevent extending the data zbi.
  // * Request at least 1 extra page when calling `BootZbi::Load`, this will require extenidng in
  //   place which is prevented by the allocated tail.
  uint32_t kernel_reserved_memory_size = 0;
  {
    auto cleanup_zbi = zbitl_cleanup(zbi);
    auto kernel_it = zbi.find(kArchZbiKernelType);
    ASSERT_TRUE(kernel_it != zbi.end());
    kernel_reserved_memory_size = static_cast<uint32_t>(
        reinterpret_cast<zbi_kernel_t*>(kernel_it->payload.data())->reserve_memory_size);
  }

  zbi_alloc = AllocateZbiForTest(zbi, kernel_reserved_memory_size + Allocation::PageSize(),
                                 kArchZbiKernelAlignment);

  {
    // Before shrink to fit, append no discard item of the memory size.
    zbitl::Image<ktl::span<ktl::byte>> image(zbi_alloc.data());
    ZX_ASSERT(image
                  .Append({
                      .type = ZBI_TYPE_DISCARD,
                      .length = kernel_reserved_memory_size,
                      .extra = 0,
                      .flags = 0,
                      .magic = ZBI_ITEM_MAGIC,
                  })
                  .is_ok());
  }

  // By allocating the tail, we will fail to allocate in place (at least PageSize extra bytes)
  // and the data should be reallocated as well.
  ktl::optional tail = ShrinkZbiToFit(zbi_alloc, true, true);
  ASSERT_TRUE(tail.has_value());
  // `zbi_alloc` contains a valid ZBI whose current allocation capacity should allow it to include
  // at most one extra page.
  zbitl::Image<ktl::span<ktl::byte>> image(zbi_alloc.data());
  // At most PageSize() - 1 extra bytes.
  ASSERT_TRUE(zbi_alloc->size_bytes() - image.size_bytes() < Allocation::PageSize());
  ASSERT_TRUE(zbi_alloc->size_bytes() >=
              image.size_bytes());  // Before any modifications are applied to input zbi.
  auto image_cleanup = zbitl_cleanup(image);
  auto input_kernel_it = image.find(kArchZbiKernelType);
  ASSERT_TRUE(input_kernel_it != image.end());
  ktl::byte* input_kernel_address = input_kernel_it->payload.data();
  auto input_data_it = ktl::next(input_kernel_it);
  ASSERT_TRUE(input_data_it != image.end());

  ktl::byte* input_zbi_data_address = input_data_it->payload.data();

  BootZbi boot_zbi = BootZbiInitAndLoad(
      BootZbi::InputZbi{zbi_alloc.release()},
      static_cast<uint32_t>(kernel_reserved_memory_size + Allocation::PageSize()));

  // Check the kernel is loaded in place.
  auto& loaded_zbi = boot_zbi.DataZbi();

  // This is a "container header" + kernel item header + the kernel payload.
  auto loaded_zbi_cleanup = zbitl_cleanup(loaded_zbi);
  auto loaded_kernel_address = reinterpret_cast<const ktl::byte*>(boot_zbi.KernelHeader());
  ASSERT_TRUE(loaded_kernel_address == input_kernel_address);

  auto loaded_data_it = loaded_zbi.find(ZBI_TYPE_CMDLINE);
  ASSERT_TRUE(loaded_data_it != loaded_zbi.end());
  ASSERT_TRUE(input_zbi_data_address != loaded_data_it->payload.data());

  // Now check payloads match.
  ASSERT_TRUE(input_data_it->header->length == loaded_data_it->header->length);
  EXPECT_TRUE(memcmp(input_data_it->payload.data(), loaded_data_it->payload.data(),
                     loaded_data_it->payload.size()) == 0);
  END_TEST;
}

bool CanLoadKernelInPlaceDataZbiDoesFitInPlaceDataZbiIsSmaller(InputZbi zbi) {
  BEGIN_TEST;
  TestTracker tracker("CanLoadKernelInPlaceDataZbiDoesFitInPlaceDataZbiIsSmaller");
  // By using the data alignment, the kernel alignment wont work, leading to a forced relocation of
  // the kernel.
  Allocation zbi_alloc;
  // The kernels is properly aligned, lets include some extra items so the the memory requirements
  // can be met with this allocation. We can do that by:
  // * Adding extra items on the tail to make data big enough to contain the kernel load size.
  // * Allocate a tail to prevent extending the data zbi.
  // * Request at least 1 extra page when calling `BootZbi::Load`, this will require extenidng in
  //   place which is prevented by the allocated tail.
  uint32_t kernel_reserved_memory_size = 0;
  {
    auto cleanup_zbi = zbitl_cleanup(zbi);
    auto kernel_it = zbi.find(kArchZbiKernelType);
    ASSERT_TRUE(kernel_it != zbi.end());
    kernel_reserved_memory_size = static_cast<uint32_t>(
        reinterpret_cast<zbi_kernel_t*>(kernel_it->payload.data())->reserve_memory_size);
  }

  zbi_alloc = AllocateZbiForTest(zbi, kernel_reserved_memory_size + Allocation::PageSize(),
                                 kArchZbiKernelAlignment);

  {
    // Before shrink to fit, append no discard item of the memory size, the kernel is still bigger
    // than the DataZBI.
    zbitl::Image<ktl::span<ktl::byte>> image(zbi_alloc.data());
    ZX_ASSERT(image
                  .Append({
                      .type = ZBI_TYPE_DISCARD,
                      .length = kernel_reserved_memory_size,
                      .extra = 0,
                      .flags = 0,
                      .magic = ZBI_ITEM_MAGIC,
                  })
                  .is_ok());
  }

  ASSERT_TRUE(!ShrinkZbiToFit(zbi_alloc, true));

  // `zbi_alloc` contains a valid ZBI whose current allocation capacity should allow it to include
  // at most one extra page.
  zbitl::Image<ktl::span<ktl::byte>> image(zbi_alloc.data());
  // At most PageSize() - 1 extra bytes.
  ASSERT_TRUE(zbi_alloc->size_bytes() - image.size_bytes() < Allocation::PageSize());
  ASSERT_TRUE(zbi_alloc->size_bytes() >= image.size_bytes());
  // Before any modifications are applied to input zbi.
  auto image_cleanup = zbitl_cleanup(image);
  auto input_kernel_it = image.find(kArchZbiKernelType);
  ASSERT_TRUE(input_kernel_it != image.end());
  ktl::byte* input_kernel_address = input_kernel_it->payload.data();
  auto input_data_it = ktl::next(input_kernel_it);
  ASSERT_TRUE(input_data_it != image.end());

  ktl::byte* input_zbi_data_address = input_data_it->payload.data();

  // Do not request any extra space, the data ZBI is smaller than the kernel plus
  // it's reserved memory size. The result should be the same as if the data zbi would
  // not fit in place.
  BootZbi boot_zbi = BootZbiInitAndLoad(BootZbi::InputZbi{zbi_alloc.release()}, 0);

  // Check the kernel is loaded in place.
  auto& loaded_zbi = boot_zbi.DataZbi();

  // This is a "container header" + kernel item header + the kernel payload.
  auto loaded_zbi_cleanup = zbitl_cleanup(loaded_zbi);
  auto loaded_kernel_address = reinterpret_cast<const ktl::byte*>(boot_zbi.KernelHeader());
  ASSERT_TRUE(loaded_kernel_address == input_kernel_address);

  auto loaded_data_it = loaded_zbi.find(ZBI_TYPE_CMDLINE);
  ASSERT_TRUE(loaded_data_it != loaded_zbi.end());
  ASSERT_TRUE(input_zbi_data_address != loaded_data_it->payload.data());

  // Now check payloads match.
  ASSERT_TRUE(input_data_it->header->length == loaded_data_it->header->length);
  EXPECT_TRUE(memcmp(input_data_it->payload.data(), loaded_data_it->payload.data(),
                     loaded_data_it->payload.size()) == 0);
  END_TEST;
}

bool CanLoadKernelInPlaceDataZbiDoesFitInPlaceDataZbiIsBigger(InputZbi zbi) {
  BEGIN_TEST;
  TestTracker tracker("CanLoadKernelInPlaceDataZbiDoesFitInPlaceDataZbiIsBigger");
  // By using the data alignment, the kernel alignment wont work, leading to a forced relocation of
  // the kernel.
  Allocation zbi_alloc;
  // The kernels is properly aligned, lets include some extra items so the the memory requirements
  // can be met with this allocation. We can do that by:
  // * Adding extra items on the tail to make data big enough to contain the kernel load size.
  // * Allocate a tail to prevent extending the data zbi.
  // * Request at least 1 extra page when calling `BootZbi::Load`, this will require extenidng in
  //   place which is prevented by the allocated tail.
  uint32_t kernel_reserved_memory_size = 0;
  uint32_t kernel_size = 0;
  {
    auto cleanup_zbi = zbitl_cleanup(zbi);
    auto kernel_it = zbi.find(kArchZbiKernelType);
    ASSERT_TRUE(kernel_it != zbi.end());
    kernel_size = kernel_it->header->length;
    kernel_reserved_memory_size = static_cast<uint32_t>(
        reinterpret_cast<zbi_kernel_t*>(kernel_it->payload.data())->reserve_memory_size);
  }

  zbi_alloc =
      AllocateZbiForTest(zbi, kernel_reserved_memory_size + kernel_size + Allocation::PageSize(),
                         kArchZbiKernelAlignment);

  {
    // Before shrink to fit, append an item of kernel size + memory size.
    // Now the Data ZBI is guaranteed to be larger than the kernel load size.
    zbitl::Image<ktl::span<ktl::byte>> image(zbi_alloc.data());
    ZX_ASSERT(image
                  .Append({
                      .type = ZBI_TYPE_CMDLINE,
                      .length = kernel_reserved_memory_size + kernel_size,
                      .extra = 0,
                      .flags = 0,
                      .magic = ZBI_ITEM_MAGIC,
                  })
                  .is_ok());
  }

  // By allocating the tail, we will fail to allocate in place (at least PageSize extra bytes)
  // and the data should be reallocated as well.
  ASSERT_TRUE(!ShrinkZbiToFit(zbi_alloc, true));
  // `zbi_alloc` contains a valid ZBI whose current allocation capacity should allow it to include
  // at most one extra page.
  zbitl::Image<ktl::span<ktl::byte>> image(zbi_alloc.data());
  // At most PageSize() - 1 extra bytes.
  ASSERT_TRUE(zbi_alloc->size_bytes() - image.size_bytes() < Allocation::PageSize());
  ASSERT_TRUE(zbi_alloc->size_bytes() >= image.size_bytes());

  // Before any modifications are applied to input zbi.
  auto image_cleanup = zbitl_cleanup(image);
  auto input_kernel_it = image.find(kArchZbiKernelType);
  ASSERT_TRUE(input_kernel_it != image.end());
  ktl::byte* input_kernel_address = input_kernel_it->payload.data();
  auto input_data_it = ktl::next(input_kernel_it);
  ASSERT_TRUE(input_data_it != image.end());
  size_t input_alloc_end = fbl::round_up(alloc_end(zbi_alloc), Allocation::PageSize());

  ktl::byte* input_zbi_data_address = input_data_it->payload.data();

  // Do not request any extra space, the data ZBI is smaller than the kernel plus
  // it's reserved memory size. The result should be the same as if the data zbi would
  // not fit in place.
  BootZbi boot_zbi = BootZbiInitAndLoad(BootZbi::InputZbi{zbi_alloc.release()}, 0);

  // Check the kernel is not loaded in place.
  auto& loaded_zbi = boot_zbi.DataZbi();

  // This is a "container header" + kernel item header + the kernel payload.
  auto loaded_zbi_cleanup = zbitl_cleanup(loaded_zbi);
  auto loaded_kernel_address = reinterpret_cast<const ktl::byte*>(boot_zbi.KernelHeader());
  ASSERT_TRUE(loaded_kernel_address != input_kernel_address);

  // DataZBI is bigger, and can should be loaded in place.
  auto loaded_data_it = loaded_zbi.find(ZBI_TYPE_CMDLINE);
  ASSERT_TRUE(loaded_data_it != loaded_zbi.end());
  ASSERT_TRUE(input_zbi_data_address == loaded_data_it->payload.data());
  ASSERT_TRUE(alloc_end(loaded_zbi.storage()) == input_alloc_end);
  END_TEST;
}

}  // namespace

const char* kTestName = "boot-zbi-load-test";

int TurduckenTest::Main(Zbi::iterator kernel_item) {
  // `loaded_zbi` is now a bootable ZBI.
  Load(kernel_item, ktl::next(kernel_item), boot_zbi().end());
  boot_zbi().ignore_error();

  CannotLoadKernelInPlaceDataZbiFitsWithinAllocation(loaded_zbi());
  CannotLoadKernelInPlaceDataZbiDoesNotFitsWithinAllocation(loaded_zbi());
  CannotLoadKernelInPlaceDataZbiDoesNotFitInPlace(loaded_zbi());

  CanLoadKernelInPlaceDataZbiDoesNotFitInPlace(loaded_zbi());
  CanLoadKernelInPlaceDataZbiDoesFitInPlaceDataZbiIsSmaller(loaded_zbi());
  CanLoadKernelInPlaceDataZbiDoesFitInPlaceDataZbiIsBigger(loaded_zbi());

  return gAnyError ? -1 : 0;
}
