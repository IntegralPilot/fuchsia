// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/standalone-test/standalone.h>
#include <lib/zbi-format/kernel.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/item.h>
#include <lib/zx/channel.h>
#include <lib/zx/debuglog.h>
#include <lib/zx/event.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/pager.h>
#include <lib/zx/port.h>
#include <lib/zx/resource.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/log.h>
#include <zircon/syscalls/object.h>
#include <zircon/syscalls/port.h>
#include <zircon/syscalls/resource.h>
#include <zircon/types.h>

#include <thread>

#include <zxtest/zxtest.h>

constexpr zx_rsrc_kind_t kDeprecatedRootKind = 3;

static const size_t mmio_test_size = (zx_system_get_page_size() * 4);
static uint64_t mmio_test_base;

const zx::unowned_resource get_ioport() { return standalone::GetIoportResource(); }

const zx::unowned_resource get_mmio() { return standalone::GetMmioResource(); }

const zx::unowned_resource get_system() { return standalone::GetSystemResource(); }

// Physical memory is reserved during boot and its location varies based on
// system and architecture. What this 'test' does is scan MMIO space looking
// for a valid region to test against, ensuring that the only errors it sees
// are 'ZX_ERR_NOT_FOUND', which indicates that it is missing from the
// region allocator.
//
// TODO(https://fxbug.dev/42107339): Figure out a way to test IRQs in the same manner, without
// hardcoding target-specific IRQ vectors in these tests. That information is
// stored in the kernel and is not exposed to userspace, so we can't simply
// guess/probe valid vectors like we can MMIO and still assume the tests are
// valid.

TEST(Resource, ProbeAddressSpace) {
  zx_status_t status;
  // Scan mmio in chunks until we find a gap that isn't exclusively reserved physical memory.
  uint64_t step = 0x100000000;
  for (uint64_t base = 0; base < UINT64_MAX - step; base += step) {
    zx::resource handle;
    status = zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, base, mmio_test_size, NULL, 0,
                                  &handle);
    if (status == ZX_OK) {
      mmio_test_base = base;
      break;
    }

    // If ZX_OK wasn't returned, then we should see either ZX_ERR_NOT_FOUND or
    // ZX_ERR_ACCESS_DENIED.
    ASSERT_TRUE(status == ZX_ERR_NOT_FOUND || status == ZX_ERR_ACCESS_DENIED);
  }
}

// This is a basic smoketest for creating resources and verifying the internals
// returned by zx_object_get_info match what the caller passed for creation.
TEST(Resource, BasicActions) {
  zx::resource new_resource;
  zx_info_resource_t info;
  char resource_name[] = "resource";

  // Create a resource and verify the fields are still zero, but the name matches.
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                 resource_name, sizeof(resource_name), &new_resource),
            ZX_OK);
  ASSERT_EQ(new_resource.get_info(ZX_INFO_RESOURCE, &info, sizeof(info), NULL, NULL), ZX_OK);
  EXPECT_EQ(info.kind, ZX_RSRC_KIND_MMIO);
  EXPECT_EQ(info.base, mmio_test_base);
  EXPECT_EQ(info.size, mmio_test_size);
  EXPECT_EQ(info.flags, 0u);
  EXPECT_EQ(0, strncmp(resource_name, info.name, ZX_MAX_NAME_LEN));

  // Check that a resource is created with all the parameters passed to the syscall, and use
  // the new resource created for good measure.
  zx::resource mmio;
  uint32_t kind = ZX_RSRC_KIND_MMIO;
  char mmio_name[] = "test_resource_name";
  ASSERT_EQ(zx::resource::create(new_resource, kind, mmio_test_base, mmio_test_size, mmio_name,
                                 sizeof(mmio_name), &mmio),
            ZX_OK);
  ASSERT_EQ(mmio.get_info(ZX_INFO_RESOURCE, &info, sizeof(info), NULL, NULL), ZX_OK);
  EXPECT_EQ(info.kind, kind);
  EXPECT_EQ(info.flags, 0u);
  EXPECT_EQ(info.base, mmio_test_base);
  EXPECT_EQ(info.size, mmio_test_size);
  EXPECT_EQ(0, strncmp(info.name, mmio_name, ZX_MAX_NAME_LEN));
}

// This test covers every path that returns ZX_ERR_INVALID_ARGS from the syscall.
TEST(Resource, InvalidArgs) {
  zx::resource temp;
  zx::resource fail_hnd;
  // test privilege inversion by seeing if an MMIO resource can create other resources.
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                 NULL, 0, &temp),
            ZX_OK);
  EXPECT_EQ(zx::resource::create(temp, ZX_RSRC_KIND_IRQ, 0, 0, NULL, 0, &fail_hnd),
            ZX_ERR_ACCESS_DENIED);

  // test invalid kind
  EXPECT_EQ(zx::resource::create(*get_mmio(), kDeprecatedRootKind, mmio_test_base, mmio_test_size,
                                 NULL, 0, &temp),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_COUNT, mmio_test_base, mmio_test_size,
                                 NULL, 0, &temp),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_COUNT + 1, mmio_test_base,
                                 mmio_test_size, NULL, 0, &temp),
            ZX_ERR_INVALID_ARGS);

  // test invalid base
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, UINT64_MAX, 1024, NULL, 0, &temp),
            ZX_ERR_INVALID_ARGS);
  // test invalid size
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, 1024, UINT64_MAX, NULL, 0, &temp),
            ZX_ERR_INVALID_ARGS);
  // test invalid options
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO | 0xFF0000, mmio_test_base,
                                 mmio_test_size, NULL, 0, &temp),
            ZX_ERR_INVALID_ARGS);
}

TEST(Resource, ExclusiveShared) {
  // Try to create a shared  resource and ensure it blocks an exclusive
  // resource.
  zx::resource mmio_1, mmio_2;
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE,
                                 mmio_test_base, mmio_test_size, NULL, 0, &mmio_1),
            ZX_OK);
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                 NULL, 0, &mmio_2),
            ZX_ERR_NOT_FOUND);
}

TEST(Resource, SharedExclusive) {
  // Try to create a shared resource and ensure it blocks an exclusive
  // resource.
  zx::resource mmio_1, mmio_2;
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                 NULL, 0, &mmio_1),
            ZX_OK);
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE,
                                 mmio_test_base, mmio_test_size, NULL, 0, &mmio_2),
            ZX_ERR_NOT_FOUND);
}

TEST(Resource, CreateFromRangedRoot) {
  // Try to create an exclusive resource from a ranged resource.
  zx::resource mmio, mmio_dup;
  EXPECT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE,
                                 mmio_test_base, mmio_test_size, NULL, 0, &mmio),
            ZX_OK);
  // Try to duplicate a ranged resource.
  EXPECT_EQ(get_mmio()->duplicate(ZX_RIGHT_SAME_RIGHTS, &mmio_dup), ZX_OK);
}

TEST(Resource, VmoCreation) {
  // Attempt to create a resource and then a vmo using that resource.
  zx::resource mmio;
  zx::vmo vmo;
  ASSERT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                 NULL, 0, &mmio),
            ZX_OK);
  EXPECT_EQ(zx_vmo_create_physical(mmio.get(), mmio_test_base, zx_system_get_page_size(),
                                   vmo.reset_and_get_address()),
            ZX_OK);
}

TEST(Resource, VmoCreationSmaller) {
  // Attempt to create a resource smaller than a page and ensure it still expands access to the
  // entire page.
  zx::resource mmio;
  zx::vmo vmo;
  ASSERT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base,
                                 zx_system_get_page_size() / 2, NULL, 0, &mmio),
            ZX_OK);
  EXPECT_EQ(zx_vmo_create_physical(mmio.get(), mmio_test_base, zx_system_get_page_size(),
                                   vmo.reset_and_get_address()),
            ZX_OK);
}

TEST(Resource, VmoCreationUnaligned) {
  // Attempt to create an unaligned resource and ensure that the bounds are rounded appropriately
  // to the proper zx_system_get_page_size().
  zx::resource mmio;
  zx::vmo vmo;
  ASSERT_EQ(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base + 0x7800, 0x2000,
                                 NULL, 0, &mmio),
            ZX_OK);
  EXPECT_EQ(zx_vmo_create_physical(mmio.get(), mmio_test_base + 0x7000, 0x2000,
                                   vmo.reset_and_get_address()),
            ZX_OK);
}

// Returns zero on failure.
static zx_rights_t get_vmo_rights(const zx::vmo& vmo) {
  zx_info_handle_basic_t info;
  zx_status_t s =
      zx_object_get_info(vmo.get(), ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (s != ZX_OK) {
    EXPECT_EQ(s, ZX_OK);  // Poison the test
    return 0;
  }
  return info.rights;
}

TEST(Resource, VmoReplaceAsExecutable) {
  zx::resource vmex;
  zx::vmo vmo, vmo2, vmo3;

  // allocate an object
  ASSERT_EQ(ZX_OK, zx_vmo_create(zx_system_get_page_size(), 0, vmo.reset_and_get_address()));

  // set-exec with valid VMEX resource
  ASSERT_EQ(ZX_OK, zx::resource::create(*get_system(), ZX_RSRC_KIND_SYSTEM,
                                        ZX_RSRC_SYSTEM_VMEX_BASE, 1, NULL, 0, &vmex));
  ASSERT_EQ(ZX_OK, zx_handle_duplicate(vmo.get(), ZX_RIGHT_READ, vmo2.reset_and_get_address()));
  ASSERT_EQ(ZX_OK,
            zx_vmo_replace_as_executable(vmo2.release(), vmex.get(), vmo3.reset_and_get_address()));
  EXPECT_EQ(ZX_RIGHT_READ | ZX_RIGHT_EXECUTE, get_vmo_rights(vmo3));

  // set-exec with ZX_HANDLE_INVALID
  // TODO(mdempsky): Disallow.
  ASSERT_EQ(ZX_OK, zx_handle_duplicate(vmo.get(), ZX_RIGHT_READ, vmo2.reset_and_get_address()));
  ASSERT_EQ(ZX_OK, zx_vmo_replace_as_executable(vmo2.release(), ZX_HANDLE_INVALID,
                                                vmo3.reset_and_get_address()));
  EXPECT_EQ(ZX_RIGHT_READ | ZX_RIGHT_EXECUTE, get_vmo_rights(vmo3));

  // verify invalid handle fails
  ASSERT_EQ(ZX_OK, zx_handle_duplicate(vmo.get(), ZX_RIGHT_READ, vmo2.reset_and_get_address()));
  EXPECT_EQ(ZX_ERR_WRONG_TYPE,
            zx_vmo_replace_as_executable(vmo2.release(), vmo.get(), vmo3.reset_and_get_address()));
}

TEST(Resource, CreateResourceSlice) {
  {
    zx::resource mmio, smaller_mmio;
    ASSERT_EQ(ZX_OK, zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base,
                                          zx_system_get_page_size(), NULL, 0, &mmio));
    // Creating an identically sized resource with the wrong kind should fail.
    EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
              zx::resource::create(mmio, ZX_RSRC_KIND_IRQ, mmio_test_base,
                                   zx_system_get_page_size(), NULL, 0, &smaller_mmio));
    // Creating a resource with a different base and the same size should fail.
    EXPECT_EQ(
        ZX_ERR_ACCESS_DENIED,
        zx::resource::create(mmio, ZX_RSRC_KIND_MMIO, mmio_test_base + zx_system_get_page_size(),
                             zx_system_get_page_size(), NULL, 0, &smaller_mmio));
    // Creating a resource with the same base and a different size should fail.
    EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
              zx::resource::create(mmio, ZX_RSRC_KIND_MMIO, mmio_test_base,
                                   zx_system_get_page_size() + 34u, NULL, 0, &smaller_mmio));
  }
  {
    // Try to make a slice going from exclusive -> shared. This should fail.
    zx::resource mmio, smaller_mmio;
    ASSERT_EQ(ZX_OK,
              zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE,
                                   mmio_test_base, zx_system_get_page_size(), NULL, 0, &mmio));
    EXPECT_EQ(ZX_ERR_INVALID_ARGS,
              zx::resource::create(mmio, ZX_RSRC_KIND_MMIO, mmio_test_base,
                                   zx_system_get_page_size(), NULL, 0, &smaller_mmio));
  }
  {
    // Try to make a slice going from shared -> exclusive. This should fail.
    zx::resource mmio, smaller_mmio;
    ASSERT_EQ(ZX_OK, zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base,
                                          zx_system_get_page_size(), NULL, 0, &mmio));
    EXPECT_EQ(ZX_ERR_INVALID_ARGS,
              zx::resource::create(mmio, ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE, mmio_test_base,
                                   zx_system_get_page_size(), NULL, 0, &smaller_mmio));
  }
  {
    // Try to make a slice going from exclusive -> exclusive. This should fail.
    zx::resource mmio, smaller_mmio;
    ASSERT_EQ(ZX_OK,
              zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE,
                                   mmio_test_base, zx_system_get_page_size(), NULL, 0, &mmio));
    EXPECT_EQ(ZX_ERR_INVALID_ARGS,
              zx::resource::create(mmio, ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE, mmio_test_base,
                                   zx_system_get_page_size(), NULL, 0, &smaller_mmio));
  }
  {
    // Creating a identically sized resource should succeed.
    zx::resource mmio, smaller_mmio;
    ASSERT_EQ(ZX_OK, zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base,
                                          zx_system_get_page_size(), NULL, 0, &mmio));
    EXPECT_EQ(ZX_OK, zx::resource::create(mmio, ZX_RSRC_KIND_MMIO, mmio_test_base,
                                          zx_system_get_page_size(), NULL, 0, &smaller_mmio));
  }
  {
    // Creating an smaller resource should succeed.
    zx::vmo vmo;
    zx::resource mmio, smaller_mmio;
    EXPECT_EQ(ZX_OK, zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base,
                                          zx_system_get_page_size() * 2, NULL, 0, &mmio));
    // This will succeed at creating an MMIO resource that is a single page size.
    EXPECT_EQ(ZX_OK, zx::resource::create(mmio, ZX_RSRC_KIND_MMIO, mmio_test_base,
                                          zx_system_get_page_size(), NULL, 0, &smaller_mmio));
    // Trying to create a VMO of the original size will fail
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
              zx_vmo_create_physical(smaller_mmio.get(), mmio_test_base,
                                     zx_system_get_page_size() * 2, vmo.reset_and_get_address()));
    // Trying to create VMO that fits in the resource will succeed.
    EXPECT_EQ(ZX_OK,
              zx_vmo_create_physical(smaller_mmio.get(), mmio_test_base, zx_system_get_page_size(),
                                     vmo.reset_and_get_address()));
  }
}

#if defined(__x86_64__)

static inline void outb(uint16_t port, uint8_t data) {
  __asm__ __volatile__("outb %1, %0" : : "dN"(port), "a"(data));
}

TEST(Resource, Ioports) {
  // On x86 create an ioport resource and attempt to have the privilege bits
  // set for the process.
  zx::resource io;
  uint16_t io_base = 0xCF8;
  uint32_t io_size = 8;  // CF8 - CFC (inclusive to 4 bytes each)
  char io_name[] = "ports!";
  ASSERT_EQ(zx::resource::create(*get_ioport(), ZX_RSRC_KIND_IOPORT, io_base, io_size, io_name,
                                 sizeof(io_name), &io),
            ZX_OK);
  EXPECT_EQ(zx_ioports_request(io.get(), io_base, io_size), ZX_OK);

  EXPECT_EQ(zx_ioports_release(io.get(), io_base, io_size), ZX_OK);

  zx::resource one_io;
  char one_io_name[] = "one";
  ASSERT_EQ(zx::resource::create(*get_ioport(), ZX_RSRC_KIND_IOPORT, 0x80, 1, one_io_name,
                                 strlen(one_io_name), &one_io),
            ZX_OK);
  // Ask for the wrong port. Should fail.
  EXPECT_EQ(zx_ioports_request(one_io.get(), io_base, io_size), ZX_ERR_OUT_OF_RANGE);
  // Lets get the right one.
  EXPECT_EQ(zx_ioports_request(one_io.get(), 0x80, 1), ZX_OK);

  outb(/*port=*/0x80, /*data=*/1);  // If we failed to get the port, this will #GP.

  // Try to release the wrong one.
  EXPECT_EQ(zx_ioports_release(one_io.get(), io_base, io_size), ZX_ERR_OUT_OF_RANGE);

  EXPECT_EQ(zx_ioports_release(one_io.get(), 0x80, 1), ZX_OK);
}

TEST(Resource, IoportDeniedByDefault) {
  // Ensure that access without requesting it via resource leads to a general protection fault
  // (#GP).
  ASSERT_DEATH([]() { outb(/*port=*/0x80, /*data=*/1); },
               "I/O port access should be forbidden by default and trigger a crash");
}

// Regression test for https://fxbug.dev/502277149
TEST(Resource, MexecPagerPanic) {
  const zx::unowned_resource system = get_system();
  if (!system->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  // Create a valid-looking ZBI for kernel_vmo
  zx::vmo kernel_vmo;
  ASSERT_OK(zx::vmo::create(8192, 0, &kernel_vmo));

  zbi_header_t header = {
      .type = ZBI_TYPE_CONTAINER,
      .length = static_cast<uint32_t>(sizeof(zbi_header_t) + sizeof(zbi_kernel_t)),
      .extra = ZBI_CONTAINER_MAGIC,
      .flags = ZBI_FLAGS_VERSION,
      .reserved0 = 0,
      .reserved1 = 0,
      .magic = ZBI_ITEM_MAGIC,
      .crc32 = ZBI_ITEM_NO_CRC32,
  };
  zbi_header_t kheader = {
      .type = ZBI_TYPE_KERNEL_X64,
      .length = static_cast<uint32_t>(sizeof(zbi_kernel_t)),
      .extra = 0,
      .flags = ZBI_FLAGS_VERSION,
      .reserved0 = 0,
      .reserved1 = 0,
      .magic = ZBI_ITEM_MAGIC,
      .crc32 = ZBI_ITEM_NO_CRC32,
  };
  zbi_kernel_t kernel = {
      .entry = 0x100,
      .reserve_memory_size = 0,
  };
  ASSERT_OK(kernel_vmo.write(&header, 0, sizeof(header)));
  ASSERT_OK(kernel_vmo.write(&kheader, sizeof(header), sizeof(kheader)));
  ASSERT_OK(kernel_vmo.write(&kernel, sizeof(header) + sizeof(kheader), sizeof(kernel)));

  zx::pager pager;
  zx::port port;
  zx::vmo vmo;
  ASSERT_OK(zx::pager::create(0, &pager));
  ASSERT_OK(zx::port::create(0, &port));
  ASSERT_OK(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &vmo));

  std::thread t([&pager, &port, &vmo]() {
    for (;;) {
      zx_port_packet_t packet;
      zx_status_t status = port.wait(zx::time::infinite(), &packet);
      if (status != ZX_OK)
        break;

      if (packet.type == ZX_PKT_TYPE_USER)
        break;

      if (packet.type == ZX_PKT_TYPE_PAGE_REQUEST &&
          packet.page_request.command == ZX_PAGER_VMO_READ) {
        pager.op_range(ZX_PAGER_OP_FAIL, vmo, packet.page_request.offset,
                       packet.page_request.length, ZX_ERR_IO);
      }
    }
  });

  // Call mexec.
  // This should return an error, but not panic!
  zx_status_t status = zx_system_mexec(system->get(), kernel_vmo.get(), vmo.get());

  EXPECT_NE(status, ZX_OK);

  // Cleanup
  zx_port_packet_t packet = {};
  packet.type = ZX_PKT_TYPE_USER;
  port.queue(&packet);
  t.join();
}

#endif  // defined(__x86_64__)

// Regression test for https://fxbug.dev/503716683
TEST(Resource, MexecZeroSizedVmo) {
  // This test requires the MEXEC resource.
  zx::unowned_resource system_resource = standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> mexec_rsrc_result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_MEXEC_BASE);

  if (mexec_rsrc_result.is_error()) {
    ZXTEST_SKIP("MEXEC resource not available or failed to derive");
  }
  zx::resource mexec_resource = std::move(mexec_rsrc_result.value());

  zx::vmo vmo1, vmo2;
  ASSERT_OK(zx::vmo::create(0, 0, &vmo1));
  ASSERT_OK(zx::vmo::create(0, 0, &vmo2));

  // Zero sized kernel or bootimage is never valid.
  EXPECT_EQ(zx_system_mexec(mexec_resource.get(), vmo1.get(), vmo2.get()), ZX_ERR_BAD_STATE);
}

// Regression test for https://fxbug.dev/503703423
TEST(Resource, MexecPayloadTooSmall) {
  // This test requires the MEXEC resource.
  zx::unowned_resource system_resource = get_system();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::resource mexec_resource;
  zx_status_t status =
      zx::resource::create(*system_resource, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_MEXEC_BASE, 1,
                           nullptr, 0, &mexec_resource);

  if (status != ZX_OK) {
    ZXTEST_SKIP("MEXEC resource not available or failed to derive");
  }

  // Buffer small enough to cause Extend to fail.
  uint8_t buffer[32];

  status = zx_system_mexec_payload_get(mexec_resource.get(), buffer, sizeof(buffer));

  // If we reach here, it didn't crash!
  EXPECT_NE(ZX_OK, status);
}

// Regression test for https://fxbug.dev/507415559
TEST(Resource, MexecEmptyZbi) {
  // This test requires the MEXEC resource.
  zx::unowned_resource system_resource = get_system();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::resource mexec_resource;
  if (zx_status_t status =
          zx::resource::create(*system_resource, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_MEXEC_BASE, 1,
                               nullptr, 0, &mexec_resource);
      status != ZX_OK) {
    ZXTEST_SKIP("MEXEC resource not available");
  }

  zx::vmo kernel_vmo, bootimage_vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &kernel_vmo));
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &bootimage_vmo));

  // Valid container, but no kernel item.
  const zbi_header_t header = zbitl::ContainerHeader(0);
  ASSERT_OK(kernel_vmo.write(&header, 0, sizeof(header)));

  EXPECT_EQ(ZX_ERR_IO_DATA_INTEGRITY,
            zx_system_mexec(mexec_resource.get(), kernel_vmo.get(), bootimage_vmo.get()));
}

// Regression test for HAZARD-ZIRCON-01: Verify that failed sys_system_mexec calls
// do not leak wired physical memory pages allocated by vmo_coalesce_pages.
TEST(Resource, MexecMemoryLeakOnFailure) {
  zx::unowned_resource system_resource = get_system();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::resource mexec_resource;
  if (zx_status_t status =
          zx::resource::create(*system_resource, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_MEXEC_BASE, 1,
                               nullptr, 0, &mexec_resource);
      status != ZX_OK) {
    ZXTEST_SKIP("MEXEC resource not available");
  }

  zx::resource info_resource;
  if (zx_status_t status =
          zx::resource::create(*system_resource, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_INFO_BASE, 1,
                               nullptr, 0, &info_resource);
      status != ZX_OK) {
    ZXTEST_SKIP("INFO resource not available");
  }

  zx_info_kmem_stats_t kmem_before;
  ASSERT_OK(zx_object_get_info(info_resource.get(), ZX_INFO_KMEM_STATS, &kmem_before,
                               sizeof(kmem_before), nullptr, nullptr));

  // Allocate a 64KB kernel VMO containing an invalid container header (no bootable kernel).
  const size_t test_vmo_size = 64 * 1024;
  zx::vmo kernel_vmo, bootimage_vmo;
  ASSERT_OK(zx::vmo::create(test_vmo_size, 0, &kernel_vmo));
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &bootimage_vmo));

  const zbi_header_t header = zbitl::ContainerHeader(0);
  ASSERT_OK(kernel_vmo.write(&header, 0, sizeof(header)));

  // Repeatedly invoke mexec expecting failure on zbitl validation.
  // Prior to the fix, each call leaked the entire contiguous physical range (64KB per call, ~38MB
  // total).
  constexpr size_t kIterations = 600;
  for (size_t i = 0; i < kIterations; ++i) {
    EXPECT_EQ(ZX_ERR_IO_DATA_INTEGRITY,
              zx_system_mexec(mexec_resource.get(), kernel_vmo.get(), bootimage_vmo.get()));
  }

  zx_info_kmem_stats_t kmem_after;
  ASSERT_OK(zx_object_get_info(info_resource.get(), ZX_INFO_KMEM_STATS, &kmem_after,
                               sizeof(kmem_after), nullptr, nullptr));

  // If memory were leaked, free_bytes would decrease by at least (kIterations * test_vmo_size) =
  // ~38MB. Allow for modest ambient allocations, but assert we did not leak ~38MB.
  const uint64_t potential_leak_bytes = kIterations * test_vmo_size;
  if (kmem_before.free_bytes > kmem_after.free_bytes) {
    uint64_t bytes_lost = kmem_before.free_bytes - kmem_after.free_bytes;
    EXPECT_LT(bytes_lost, potential_leak_bytes / 2,
              "Wired physical memory was leaked across failed mexec calls");
  }
}

// Regression test for https://fxbug.dev/512234306
// NOTE: If behavior ever regresses, then we expect this
// test to only fail (crash) on KASAN builds of the kernel.
TEST(Resource, MexecBadZbiHeaderLength) {
  zx::unowned_resource system_resource = get_system();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::resource mexec_resource;
  zx_status_t status =
      zx::resource::create(*system_resource, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_MEXEC_BASE, 1,
                           nullptr, 0, &mexec_resource);
  if (status != ZX_OK) {
    ZXTEST_SKIP("MEXEC resource not available");
  }

  zbi_header_t container = {
      .type = ZBI_TYPE_CONTAINER,
      .length = 0x40000000,  // 1 GB
      .extra = ZBI_CONTAINER_MAGIC,
      .flags = ZBI_FLAGS_VERSION,
      .reserved0 = 0,
      .reserved1 = 0,
      .magic = ZBI_ITEM_MAGIC,
      .crc32 = ZBI_ITEM_NO_CRC32,
  };

  zbi_header_t item1 = {
      .type = ZBI_TYPE_DISCARD,
      .length = 0x3FFFFF00,  // Jump 1 GB
      .extra = 0,
      .flags = ZBI_FLAGS_VERSION,
      .reserved0 = 0,
      .reserved1 = 0,
      .magic = ZBI_ITEM_MAGIC,
      .crc32 = ZBI_ITEM_NO_CRC32,
  };

  zx::vmo kernel_vmo, bootimage_vmo;
  const size_t vmo_size = sizeof(container) + sizeof(item1);
  ASSERT_OK(zx::vmo::create(vmo_size, 0, &kernel_vmo));
  ASSERT_OK(zx::vmo::create(vmo_size, 0, &bootimage_vmo));

  ASSERT_OK(kernel_vmo.write(&container, 0, sizeof(container)));
  ASSERT_OK(kernel_vmo.write(&item1, sizeof(container), sizeof(item1)));

  status = zx_system_mexec(mexec_resource.get(), kernel_vmo.get(), bootimage_vmo.get());
  EXPECT_NE(ZX_OK, status);
}

#if defined(__x86_64__)

// Regression test for https://fxbug.dev/517585028
TEST(Resource, PcInterruptVectorPoolLeak) {
  zx::unowned_resource irq_res = standalone::GetIrqResource();
  if (!irq_res->is_valid()) {
    ZXTEST_SKIP("IRQ resource not available");
  }
  std::atomic<bool> stop{false};

  // This test spawns two threads repeatedly creating and destroying interrupt dispatcher
  // objects in order to exploit a potential race condition in the pc platform's
  // interrupt manager and leak entries of the vector pool.

  auto repeatedly_create_and_destroy_interrupt = [&]() {
    while (!stop.load()) {
      zx_handle_t h = ZX_HANDLE_INVALID;
      if (zx_interrupt_create(irq_res->get(), 8, ZX_INTERRUPT_MODE_EDGE_HIGH, &h) == ZX_OK) {
        zx_handle_close(h);
      }
    }
  };

  std::thread t1(repeatedly_create_and_destroy_interrupt);
  std::thread t2(repeatedly_create_and_destroy_interrupt);

  zx_nanosleep(zx_deadline_after(ZX_MSEC(50)));
  stop.store(true);
  t1.join();
  t2.join();

  // By this point, if there was a leak, we would expect a crash.
}

#endif  // defined(__x86_64__)

TEST(Resource, CreateParentMissingWriteRightsReturnsAccessDenied) {
  zx::resource reduced;
  ASSERT_OK(get_mmio()->duplicate(ZX_DEFAULT_RESOURCE_RIGHTS & ~ZX_RIGHT_WRITE, &reduced));

  zx::resource out;
  EXPECT_STATUS(zx::resource::create(reduced, ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                     nullptr, 0, &out),
                ZX_ERR_ACCESS_DENIED);
}

TEST(Resource, CreateInvalidParentReturnsBadHandle) {
  zx::resource out;
  EXPECT_STATUS(zx_resource_create(ZX_HANDLE_INVALID, ZX_RSRC_KIND_MMIO, mmio_test_base,
                                   mmio_test_size, nullptr, 0, out.reset_and_get_address()),
                ZX_ERR_BAD_HANDLE);
}

TEST(Resource, CreateBadNamePointerReturnsInvalidArgs) {
  zx::resource out;
  const char* bad_name = reinterpret_cast<const char*>(1);
  EXPECT_STATUS(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO, mmio_test_base,
                                   mmio_test_size, bad_name, 8, out.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);
}

TEST(Resource, ValidateResourceKindBaseWithWrongTypeOrKindReturnsError) {
  // zx_debuglog_create calls validate_resource_kind_base(rsrc, ZX_RSRC_KIND_SYSTEM,
  // ZX_RSRC_SYSTEM_DEBUGLOG_BASE).
  zx::debuglog out;

  // 1. Invalid handle with non-zero options returns ZX_ERR_BAD_HANDLE.
  EXPECT_STATUS(
      zx_debuglog_create(ZX_HANDLE_INVALID, ZX_LOG_FLAG_READABLE, out.reset_and_get_address()),
      ZX_ERR_BAD_HANDLE);

  // 2. Non-resource handle (event) returns ZX_ERR_WRONG_TYPE.
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  EXPECT_STATUS(zx_debuglog_create(event.get(), 0, out.reset_and_get_address()), ZX_ERR_WRONG_TYPE);

  // 3. Wrong resource kind (MMIO passed to debuglog_create) returns ZX_ERR_WRONG_TYPE.
  EXPECT_STATUS(zx_debuglog_create(get_mmio()->get(), 0, out.reset_and_get_address()),
                ZX_ERR_WRONG_TYPE);
}

TEST(Resource, ValidateRangedResourceWithWrongKindReturnsAccessDenied) {
  // Trying to slice a resource using a parent of a different kind returns ZX_ERR_WRONG_TYPE in
  // validate_ranged_resource, which sys_resource_create translates to ZX_ERR_ACCESS_DENIED.
  zx::resource parent_mmio;
  ASSERT_OK(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                 nullptr, 0, &parent_mmio));

  zx::resource out;
  EXPECT_STATUS(zx::resource::create(parent_mmio, ZX_RSRC_KIND_IOPORT, 0, 1, nullptr, 0, &out),
                ZX_ERR_ACCESS_DENIED);
}

TEST(Resource, CreateArithmeticOverflow) {
  zx::resource out;
  // base + size overflows uint64_t.
  EXPECT_STATUS(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO, UINT64_MAX, 1, nullptr, 0,
                                   out.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO, 1, UINT64_MAX, nullptr, 0,
                                   out.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO, UINT64_MAX - 100, 101,
                                   nullptr, 0, out.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO, 0x8000000000000000ULL,
                                   0x8000000000000001ULL, nullptr, 0, out.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);
}

TEST(Resource, CreateParentHandleValidation) {
  zx::resource out;

  // 1. Non-resource handles should return ZX_ERR_WRONG_TYPE.
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  EXPECT_STATUS(zx_resource_create(event.get(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                   nullptr, 0, out.reset_and_get_address()),
                ZX_ERR_WRONG_TYPE);

  zx::channel ch1, ch2;
  ASSERT_OK(zx::channel::create(0, &ch1, &ch2));
  EXPECT_STATUS(zx_resource_create(ch1.get(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                   nullptr, 0, out.reset_and_get_address()),
                ZX_ERR_WRONG_TYPE);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  EXPECT_STATUS(zx_resource_create(vmo.get(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                   nullptr, 0, out.reset_and_get_address()),
                ZX_ERR_WRONG_TYPE);

  // 2. Parent resource handle with ONLY ZX_RIGHT_WRITE should succeed.
  zx::resource write_only_parent;
  ASSERT_OK(get_mmio()->duplicate(ZX_RIGHT_WRITE, &write_only_parent));
  EXPECT_OK(zx_resource_create(write_only_parent.get(), ZX_RSRC_KIND_MMIO, mmio_test_base,
                               mmio_test_size, nullptr, 0, out.reset_and_get_address()));
}

TEST(Resource, CreateInvalidOptionsAndFlags) {
  zx::resource out;

  // Invalid kind values.
  EXPECT_STATUS(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_COUNT, mmio_test_base,
                                   mmio_test_size, nullptr, 0, out.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_COUNT + 5, mmio_test_base,
                                   mmio_test_size, nullptr, 0, out.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(zx_resource_create(get_mmio()->get(), 0x0000FFFF, mmio_test_base, mmio_test_size,
                                   nullptr, 0, out.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);

  // Invalid flag values outside ZX_RSRC_FLAGS_MASK (0x00010000).
  EXPECT_STATUS(
      zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO | 0x00020000, mmio_test_base,
                         mmio_test_size, nullptr, 0, out.reset_and_get_address()),
      ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(
      zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO | 0x00040000, mmio_test_base,
                         mmio_test_size, nullptr, 0, out.reset_and_get_address()),
      ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(
      zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO | 0x80000000, mmio_test_base,
                         mmio_test_size, nullptr, 0, out.reset_and_get_address()),
      ZX_ERR_INVALID_ARGS);

  // Valid exclusive flag should succeed.
  EXPECT_OK(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE,
                               mmio_test_base, mmio_test_size, nullptr, 0,
                               out.reset_and_get_address()));
}

TEST(Resource, CreateNameHandlingAndTruncation) {
  zx::resource out;
  zx_info_resource_t info;
  char prop_name[ZX_MAX_NAME_LEN];

  // 1. name_size == 0 with nullptr name -> empty name.
  ASSERT_OK(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                               nullptr, 0, out.reset_and_get_address()));
  ASSERT_OK(out.get_info(ZX_INFO_RESOURCE, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0, strcmp(info.name, ""));
  ASSERT_OK(out.get_property(ZX_PROP_NAME, prop_name, sizeof(prop_name)));
  EXPECT_EQ(0, strcmp(prop_name, ""));

  // 2. name_size == 0 with non-null name -> empty name.
  ASSERT_OK(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                               "ignored", 0, out.reset_and_get_address()));
  ASSERT_OK(out.get_info(ZX_INFO_RESOURCE, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0, strcmp(info.name, ""));

  // 3. name_size > 0 with nullptr name -> ZX_ERR_INVALID_ARGS.
  EXPECT_STATUS(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO, mmio_test_base,
                                   mmio_test_size, nullptr, 10, out.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);

  // 4. Exact 31-character name (ZX_MAX_NAME_LEN - 1).
  const char name_31[] = "1234567890123456789012345678901";
  static_assert(sizeof(name_31) - 1 == ZX_MAX_NAME_LEN - 1);
  ASSERT_OK(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                               name_31, sizeof(name_31) - 1, out.reset_and_get_address()));
  ASSERT_OK(out.get_info(ZX_INFO_RESOURCE, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0, strcmp(info.name, name_31));
  ASSERT_OK(out.get_property(ZX_PROP_NAME, prop_name, sizeof(prop_name)));
  EXPECT_EQ(0, strcmp(prop_name, name_31));

  // 5. Name longer than ZX_MAX_NAME_LEN is truncated to 31 chars.
  const char long_name[] = "1234567890123456789012345678901_THIS_SHOULD_BE_TRUNCATED";
  ASSERT_OK(zx_resource_create(get_mmio()->get(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                               long_name, sizeof(long_name), out.reset_and_get_address()));
  ASSERT_OK(out.get_info(ZX_INFO_RESOURCE, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0, strncmp(info.name, long_name, ZX_MAX_NAME_LEN - 1));
  EXPECT_EQ('\0', info.name[ZX_MAX_NAME_LEN - 1]);
}

TEST(Resource, CreatedHandlePropertiesAndRights) {
  zx::resource res;
  char name[] = "test_props";
  ASSERT_OK(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                 name, sizeof(name), &res));

  // Basic handle info checks.
  zx_info_handle_basic_t basic_info;
  ASSERT_OK(res.get_info(ZX_INFO_HANDLE_BASIC, &basic_info, sizeof(basic_info), nullptr, nullptr));
  EXPECT_EQ(basic_info.type, ZX_OBJ_TYPE_RESOURCE);
  EXPECT_EQ(basic_info.rights, ZX_DEFAULT_RESOURCE_RIGHTS);
  EXPECT_NE(basic_info.koid, 0u);
  EXPECT_EQ(basic_info.related_koid, 0u);

  // Resources are not waitable and lack ZX_RIGHT_WAIT -> wait_one returns ZX_ERR_ACCESS_DENIED.
  zx_signals_t pending = 0;
  EXPECT_STATUS(res.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite_past(), &pending),
                ZX_ERR_ACCESS_DENIED);

  // Getting ZX_PROP_NAME with sufficient buffer succeeds.
  char prop_name[ZX_MAX_NAME_LEN];
  ASSERT_OK(res.get_property(ZX_PROP_NAME, prop_name, sizeof(prop_name)));
  EXPECT_EQ(0, strcmp(prop_name, name));

  // Getting ZX_PROP_NAME with buffer smaller than ZX_MAX_NAME_LEN returns ZX_ERR_BUFFER_TOO_SMALL.
  char small_buf[ZX_MAX_NAME_LEN - 1];
  EXPECT_STATUS(res.get_property(ZX_PROP_NAME, small_buf, sizeof(small_buf)),
                ZX_ERR_BUFFER_TOO_SMALL);

  // Setting ZX_PROP_NAME fails with ZX_ERR_ACCESS_DENIED (resource handles lack
  // ZX_RIGHT_SET_PROPERTY).
  EXPECT_STATUS(res.set_property(ZX_PROP_NAME, "new_name", sizeof("new_name")),
                ZX_ERR_ACCESS_DENIED);
}

TEST(Resource, GetInfoResourceValidation) {
  zx::resource res;
  char name[] = "info_test";
  ASSERT_OK(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                 name, sizeof(name), &res));

  // Full buffer returns full struct.
  zx_info_resource_t info;
  size_t actual = 0, avail = 0;
  ASSERT_OK(res.get_info(ZX_INFO_RESOURCE, &info, sizeof(info), &actual, &avail));
  EXPECT_EQ(actual, 1u);
  EXPECT_EQ(avail, 1u);
  EXPECT_EQ(info.kind, ZX_RSRC_KIND_MMIO);
  EXPECT_EQ(info.flags, 0u);
  EXPECT_EQ(info.base, mmio_test_base);
  EXPECT_EQ(info.size, mmio_test_size);
  EXPECT_EQ(0, strcmp(info.name, name));

  // Buffer smaller than zx_info_resource_t returns ZX_ERR_BUFFER_TOO_SMALL.
  uint8_t small_buf[sizeof(zx_info_resource_t) - 1];
  EXPECT_STATUS(res.get_info(ZX_INFO_RESOURCE, small_buf, sizeof(small_buf), &actual, &avail),
                ZX_ERR_BUFFER_TOO_SMALL);

  // Resource handle without ZX_RIGHT_INSPECT returns ZX_ERR_ACCESS_DENIED.
  zx::resource no_inspect;
  ASSERT_OK(res.duplicate(ZX_DEFAULT_RESOURCE_RIGHTS & ~ZX_RIGHT_INSPECT, &no_inspect));
  EXPECT_STATUS(no_inspect.get_info(ZX_INFO_RESOURCE, &info, sizeof(info), nullptr, nullptr),
                ZX_ERR_ACCESS_DENIED);
}

TEST(Resource, CreateFromRangedRootKinds) {
  // MMIO resource can create MMIO child.
  zx::resource mmio_child;
  EXPECT_OK(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base, mmio_test_size,
                                 nullptr, 0, &mmio_child));

  // IRQ resource can create IRQ child.
  zx::unowned_resource irq_root = standalone::GetIrqResource();
  if (irq_root->is_valid()) {
    zx::resource irq_child;
    // Scan for a valid vector in the platform's IRQ allocator (e.g. 0 on x86, 32 on ARM GIC).
    for (uint32_t vector = 0; vector < 1024; ++vector) {
      if (zx::resource::create(*irq_root, ZX_RSRC_KIND_IRQ, vector, 1, nullptr, 0, &irq_child) ==
          ZX_OK) {
        break;
      }
    }
    EXPECT_TRUE(irq_child.is_valid());
  }

  // SYSTEM resource can create SYSTEM child.
  zx::unowned_resource sys_root = get_system();
  if (sys_root->is_valid()) {
    zx::resource sys_child;
    EXPECT_OK(zx::resource::create(*sys_root, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_INFO_BASE, 1,
                                   nullptr, 0, &sys_child));
  }

  // Cross-kind creation from resources is denied.
  zx::resource fail_child;
  EXPECT_STATUS(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_IRQ, 0, 1, nullptr, 0, &fail_child),
                ZX_ERR_ACCESS_DENIED);
  EXPECT_STATUS(
      zx::resource::create(*get_mmio(), ZX_RSRC_KIND_SYSTEM, 0, 1, nullptr, 0, &fail_child),
      ZX_ERR_ACCESS_DENIED);
  EXPECT_STATUS(
      zx::resource::create(*get_mmio(), kDeprecatedRootKind, 0, 0, nullptr, 0, &fail_child),
      ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(zx_resource_create(get_mmio()->get(), kDeprecatedRootKind, 100, 10, nullptr, 0,
                                   fail_child.reset_and_get_address()),
                ZX_ERR_INVALID_ARGS);
}

TEST(Resource, HierarchySlicingBoundaries) {
  const size_t page_size = zx_system_get_page_size();
  zx::resource parent;
  ASSERT_OK(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base, page_size * 4,
                                 nullptr, 0, &parent));

  zx::resource child;
  // Slicing exact range -> OK.
  EXPECT_OK(zx::resource::create(parent, ZX_RSRC_KIND_MMIO, mmio_test_base, page_size * 4, nullptr,
                                 0, &child));

  // Sub-slice at start -> OK.
  EXPECT_OK(zx::resource::create(parent, ZX_RSRC_KIND_MMIO, mmio_test_base, page_size, nullptr, 0,
                                 &child));

  // Sub-slice in middle -> OK.
  EXPECT_OK(zx::resource::create(parent, ZX_RSRC_KIND_MMIO, mmio_test_base + page_size,
                                 page_size * 2, nullptr, 0, &child));

  // Sub-slice at end -> OK.
  EXPECT_OK(zx::resource::create(parent, ZX_RSRC_KIND_MMIO, mmio_test_base + page_size * 3,
                                 page_size, nullptr, 0, &child));

  // Zero-sized slice of a non-root parent returns ZX_ERR_ACCESS_DENIED.
  EXPECT_STATUS(
      zx::resource::create(parent, ZX_RSRC_KIND_MMIO, mmio_test_base, 0, nullptr, 0, &child),
      ZX_ERR_ACCESS_DENIED);
  EXPECT_STATUS(zx::resource::create(parent, ZX_RSRC_KIND_MMIO, mmio_test_base + page_size * 4, 0,
                                     nullptr, 0, &child),
                ZX_ERR_ACCESS_DENIED);

  // Multi-tier hierarchy: Parent -> Child -> Grandchild -> Great-Grandchild.
  zx::resource tier1_child, tier2_grandchild, tier3_great_grandchild;
  ASSERT_OK(zx::resource::create(parent, ZX_RSRC_KIND_MMIO, mmio_test_base + page_size,
                                 page_size * 2, nullptr, 0, &tier1_child));

  // Slice starting 1 byte before tier1_child -> ZX_ERR_ACCESS_DENIED.
  // Using (mmio_test_base + page_size - 1) avoids uint64 underflow when mmio_test_base is 0.
  EXPECT_STATUS(zx::resource::create(tier1_child, ZX_RSRC_KIND_MMIO, mmio_test_base + page_size - 1,
                                     page_size, nullptr, 0, &child),
                ZX_ERR_ACCESS_DENIED);

  // Slice extending 1 byte past parent end -> ZX_ERR_ACCESS_DENIED.
  EXPECT_STATUS(zx::resource::create(parent, ZX_RSRC_KIND_MMIO, mmio_test_base + page_size * 3,
                                     page_size + 1, nullptr, 0, &child),
                ZX_ERR_ACCESS_DENIED);

  // Slice completely outside parent -> ZX_ERR_ACCESS_DENIED.
  EXPECT_STATUS(zx::resource::create(parent, ZX_RSRC_KIND_MMIO, mmio_test_base + page_size * 10,
                                     page_size, nullptr, 0, &child),
                ZX_ERR_ACCESS_DENIED);

  ASSERT_OK(zx::resource::create(tier1_child, ZX_RSRC_KIND_MMIO, mmio_test_base + page_size,
                                 page_size, nullptr, 0, &tier2_grandchild));
  ASSERT_OK(zx::resource::create(tier2_grandchild, ZX_RSRC_KIND_MMIO, mmio_test_base + page_size,
                                 page_size / 2, nullptr, 0, &tier3_great_grandchild));

  // Great-grandchild attempting to slice beyond grandchild bounds (even if within tier1_child)
  // must fail.
  zx::resource out_of_bounds;
  EXPECT_STATUS(
      zx::resource::create(tier3_great_grandchild, ZX_RSRC_KIND_MMIO, mmio_test_base + page_size,
                           page_size, nullptr, 0, &out_of_bounds),
      ZX_ERR_ACCESS_DENIED);
}

TEST(Resource, StrictMmioRangeValidation) {
  const size_t page_size = zx_system_get_page_size();
  // Create an unaligned parent MMIO slice covering [mmio_test_base + 0x200, mmio_test_base +
  // 0x600).
  zx::resource parent;
  ASSERT_OK(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base + 0x200, 0x400,
                                 nullptr, 0, &parent));

  // Strict validation on sys_resource_create rejects child outside [base, base + size) even if on
  // the same page.
  zx::resource child;
  EXPECT_STATUS(
      zx::resource::create(parent, ZX_RSRC_KIND_MMIO, mmio_test_base, 0x200, nullptr, 0, &child),
      ZX_ERR_ACCESS_DENIED);

  // Child strictly contained inside [mmio_test_base + 0x200, mmio_test_base + 0x600) succeeds.
  EXPECT_OK(zx::resource::create(parent, ZX_RSRC_KIND_MMIO, mmio_test_base + 0x200, 0x200, nullptr,
                                 0, &child));

  // Non-strict validation in zx_vmo_create_physical allows mapping the page.
  zx::vmo vmo;
  EXPECT_OK(
      zx_vmo_create_physical(parent.get(), mmio_test_base, page_size, vmo.reset_and_get_address()));
}

TEST(Resource, ExclusiveRegionReleaseLifecycle) {
  const size_t page_size = zx_system_get_page_size();

  // 1. Allocate exclusive region A.
  zx::resource res_a;
  ASSERT_OK(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE,
                                 mmio_test_base, page_size, nullptr, 0, &res_a));

  // 2. Allocate adjacent disjoint exclusive region B -> OK.
  zx::resource res_b;
  EXPECT_OK(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE,
                                 mmio_test_base + page_size, page_size, nullptr, 0, &res_b));

  // 3. Attempt to allocate overlapping exclusive region C -> ZX_ERR_NOT_FOUND.
  zx::resource res_c;
  EXPECT_STATUS(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE,
                                     mmio_test_base, page_size, nullptr, 0, &res_c),
                ZX_ERR_NOT_FOUND);

  // 4. Release region A by closing its handle.
  res_a.reset();

  // 5. Now allocating region C succeeds because region A was returned to the allocator.
  EXPECT_OK(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO | ZX_RSRC_FLAG_EXCLUSIVE,
                                 mmio_test_base, page_size, nullptr, 0, &res_c));

  // 6. Release region C.
  res_c.reset();

  // 7. Allocating shared region D in the same space succeeds.
  zx::resource res_d;
  EXPECT_OK(zx::resource::create(*get_mmio(), ZX_RSRC_KIND_MMIO, mmio_test_base, page_size, nullptr,
                                 0, &res_d));
}

TEST(Resource, SystemResourceSubBasesAndSyscallValidation) {
  const zx::unowned_resource system = get_system();
  if (!system->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  const zx_rsrc_system_base_t all_bases[] = {
      ZX_RSRC_SYSTEM_HYPERVISOR_BASE, ZX_RSRC_SYSTEM_VMEX_BASE,        ZX_RSRC_SYSTEM_DEBUG_BASE,
      ZX_RSRC_SYSTEM_INFO_BASE,       ZX_RSRC_SYSTEM_CPU_BASE,         ZX_RSRC_SYSTEM_POWER_BASE,
      ZX_RSRC_SYSTEM_MEXEC_BASE,      ZX_RSRC_SYSTEM_ENERGY_INFO_BASE, ZX_RSRC_SYSTEM_IOMMU_BASE,
      ZX_RSRC_SYSTEM_PROFILE_BASE,    ZX_RSRC_SYSTEM_MSI_BASE,         ZX_RSRC_SYSTEM_DEBUGLOG_BASE,
      ZX_RSRC_SYSTEM_STALL_BASE,      ZX_RSRC_SYSTEM_TRACING_BASE,     ZX_RSRC_SYSTEM_SAMPLING_BASE,
  };

  for (zx_rsrc_system_base_t base : all_bases) {
    zx::resource child;
    ASSERT_OK(zx::resource::create(*system, ZX_RSRC_KIND_SYSTEM, base, 1, nullptr, 0, &child));

    zx_info_resource_t info;
    ASSERT_OK(child.get_info(ZX_INFO_RESOURCE, &info, sizeof(info), nullptr, nullptr));
    EXPECT_EQ(info.kind, ZX_RSRC_KIND_SYSTEM);
    EXPECT_EQ(info.base, base);
    EXPECT_EQ(info.size, 1u);
  }

  // Verify syscall/topic enforcement with specific system resource sub-bases:
  zx::resource debuglog_res, vmex_res, info_res, stall_res;
  ASSERT_OK(zx::resource::create(*system, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_DEBUGLOG_BASE, 1,
                                 nullptr, 0, &debuglog_res));
  ASSERT_OK(zx::resource::create(*system, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_VMEX_BASE, 1, nullptr,
                                 0, &vmex_res));
  ASSERT_OK(zx::resource::create(*system, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_INFO_BASE, 1, nullptr,
                                 0, &info_res));
  ASSERT_OK(zx::resource::create(*system, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_STALL_BASE, 1,
                                 nullptr, 0, &stall_res));

  // 1. zx_debuglog_create uses validate_resource_kind_base: debuglog_res -> OK, vmex_res ->
  // ZX_ERR_WRONG_TYPE.
  zx::debuglog log;
  EXPECT_OK(zx_debuglog_create(debuglog_res.get(), 0, log.reset_and_get_address()));
  EXPECT_STATUS(zx_debuglog_create(vmex_res.get(), 0, log.reset_and_get_address()),
                ZX_ERR_WRONG_TYPE);

  // 2. zx_vmo_replace_as_executable uses validate_ranged_resource: vmex_res -> OK, debuglog_res
  // (wrong base) -> ZX_ERR_OUT_OF_RANGE.
  zx::vmo vmo, vmo_dup, exec_vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_READ, &vmo_dup));
  EXPECT_OK(zx_vmo_replace_as_executable(vmo_dup.release(), vmex_res.get(),
                                         exec_vmo.reset_and_get_address()));
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_READ, &vmo_dup));
  EXPECT_STATUS(zx_vmo_replace_as_executable(vmo_dup.release(), debuglog_res.get(),
                                             exec_vmo.reset_and_get_address()),
                ZX_ERR_OUT_OF_RANGE);

  // 3. ZX_INFO_KMEM_STATS uses validate_ranged_resource: info_res -> OK, debuglog_res ->
  // ZX_ERR_OUT_OF_RANGE.
  zx_info_kmem_stats_t kmem_stats;
  EXPECT_OK(
      info_res.get_info(ZX_INFO_KMEM_STATS, &kmem_stats, sizeof(kmem_stats), nullptr, nullptr));
  EXPECT_STATUS(
      debuglog_res.get_info(ZX_INFO_KMEM_STATS, &kmem_stats, sizeof(kmem_stats), nullptr, nullptr),
      ZX_ERR_OUT_OF_RANGE);

  // 4. ZX_INFO_MEMORY_STALL uses validate_ranged_resource: stall_res -> OK, info_res ->
  // ZX_ERR_OUT_OF_RANGE.
  zx_info_memory_stall_t stall_stats;
  EXPECT_OK(stall_res.get_info(ZX_INFO_MEMORY_STALL, &stall_stats, sizeof(stall_stats), nullptr,
                               nullptr));
  EXPECT_STATUS(
      info_res.get_info(ZX_INFO_MEMORY_STALL, &stall_stats, sizeof(stall_stats), nullptr, nullptr),
      ZX_ERR_OUT_OF_RANGE);

  // 5. Passing a resource of the wrong kind (e.g. MMIO) to get_info topic expecting SYSTEM returns
  // ZX_ERR_WRONG_TYPE.
  EXPECT_STATUS(
      get_mmio()->get_info(ZX_INFO_KMEM_STATS, &kmem_stats, sizeof(kmem_stats), nullptr, nullptr),
      ZX_ERR_WRONG_TYPE);
}
