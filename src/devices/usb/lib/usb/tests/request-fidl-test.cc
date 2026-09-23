// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usb/request-fidl.h"

#include <lib/fake-bti/bti.h>
#include <lib/fit/defer.h>

#include <limits>

#include <zxtest/zxtest.h>

namespace {

zx::result<std::optional<usb::internal::MappedVmo>> MakeNulloptMock(
    const fuchsia_hardware_usb_request::Buffer&) {
  return zx::ok(std::nullopt);
}

// Verifies default construction and destruction of empty FidlRequest objects.
TEST(RequestFidlTest, EmptyRequestTest) {
  fuchsia_hardware_usb_request::Request request;
  usb::FidlRequest fidl_request(std::move(request));
}

// Verifies that PhysMap on an empty request succeeds without mapping VMOs.
// Disabled: PhysMap() requires zero-size bypass for empty request payloads.
TEST(RequestFidlTest, DISABLED_PhysMapEmptyPayload) {
  usb::FidlRequest fidl_request;
  zx::bti bti;
  ASSERT_OK(fake_bti_create(bti.reset_and_get_address()));
  EXPECT_OK(fidl_request.PhysMap(bti));
}

// Verifies that while add_data pads vector capacity upon request creation, PhysMap acts as a safety
// guard ensuring vector capacity is padded up to offset + size before BTI physical pinning.
// Disabled: PhysMap() requires dynamic offset padding when region offset exceeds vector initial
// length.
TEST(RequestFidlTest, DISABLED_PhysMapOutOfBounds) {
  usb::FidlRequest fidl_request;
  zx::bti bti;
  ASSERT_OK(fake_bti_create(bti.reset_and_get_address()));

  // Offset 20 is completely outside the vector length of 10.
  fidl_request.add_data(std::vector<uint8_t>(10, 0), 5, 20);
  EXPECT_OK(fidl_request.PhysMap(bti));
  ASSERT_TRUE(fidl_request->data().has_value());
  EXPECT_EQ(fidl_request->data()->at(0).buffer()->data()->size(), 25u);
}

// Verifies that PhysMap safely handles zero-capacity and zero-size kData payloads.
// Disabled: PhysMap() requires zero-capacity / zero-size kData payload bypass.
TEST(RequestFidlTest, DISABLED_PhysMapZeroSizeBypassTest) {
  usb::FidlRequest fidl_request;
  // Explicitly allocate a kData payload with 0 capacity and 0 requested size.
  fidl_request.add_data(std::vector<uint8_t>(), 0, 0);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  EXPECT_OK(fidl_request.PhysMap(fake_bti));
  ASSERT_TRUE(fidl_request->data().has_value());
  EXPECT_EQ(fidl_request->data()->at(0).buffer()->data()->size(), 0u);
}

// Verifies that PhysMap rejects inline vector regions where offset + size exceeds kMaxTransferSize
// (1024 bytes) with ZX_ERR_INVALID_ARGS, while permitting exact boundary offset + size == 1024
// bytes.
// Disabled: PhysMap() requires validation guard checking that end_offset does not exceed
// kMaxTransferSize.
TEST(RequestFidlTest, DISABLED_PhysMapMaxTransferSizeExceededTest) {
  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  // Exact boundary: offset (800) + size (224) == 1024 bytes -> OK.
  usb::FidlRequest valid_req;
  valid_req.add_data(std::vector<uint8_t>(800, 0), 224, 800);
  EXPECT_OK(valid_req.PhysMap(fake_bti));

  // Over boundary: offset (800) + size (225) == 1025 bytes -> ZX_ERR_INVALID_ARGS.
  usb::FidlRequest invalid_req;
  invalid_req.add_data(std::vector<uint8_t>(800, 0), 225, 800);
  EXPECT_STATUS(invalid_req.PhysMap(fake_bti), ZX_ERR_INVALID_ARGS);
}

// Verifies total payload length aggregation across mixed kVmoId and kData buffer regions.
TEST(RequestFidlTest, LengthTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_vmo_id(0, 16, 0).add_data({}, 16, 0);

  EXPECT_EQ(fidl_request.length(), 32);
}

// Verifies BTI unpinning and unmapping during Unpin().
TEST(RequestFidlTest, UnpinTest) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(16, 0, &vmo));
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(1))
      .offset(0)
      .size(32);
  request.data()
      ->emplace_back()
      .buffer(
          fuchsia_hardware_usb_request::Buffer::WithData(std::vector<uint8_t>{0x0, 0x0, 0x0, 0x0}))
      .offset(0)
      .size(4);
  usb::FidlRequest fidl_request(std::move(request));

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  EXPECT_OK(fidl_request.PhysMap(fake_bti));
  size_t actual;
  fake_bti_pinned_vmo_info_t info[1];
  EXPECT_OK(fake_bti_get_pinned_vmos(fake_bti.get(), info, 1, &actual));
  EXPECT_EQ(actual, 1);

  void* mapped;
  EXPECT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, zx::vmo(info[0].vmo),
                                       0, info[0].size, reinterpret_cast<uintptr_t*>(&mapped)));
  auto iter3 = fidl_request.phys_iter(1, zx_system_get_page_size());
  EXPECT_EQ((*iter3.begin()).second, 4);
  uint8_t expected_vals[] = {0xA, 0xB, 0xC, 0xC};
  memcpy(mapped, expected_vals, sizeof(expected_vals));
  EXPECT_OK(zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(mapped), info[0].size));

  EXPECT_OK(fidl_request.Unpin());
  EXPECT_OK(fake_bti_get_pinned_vmos(fake_bti.get(), nullptr, 0, &actual));
  EXPECT_EQ(actual, 0);
  EXPECT_BYTES_EQ((*fidl_request->data())[1].buffer()->data()->data(), expected_vals,
                  sizeof(expected_vals));
}

// Verifies that Unpin() copies back IN transfer payload data starting at the region offset for
// kData buffers.
// Disabled: PhysMap() and Unpin() require offset displacement handling for kData IN transfers.
TEST(RequestFidlTest, DISABLED_UnpinWithRegionOffsetCopyBackTest) {
  usb::FidlRequest fidl_request;
  std::vector<uint8_t> initial_data(20, 0);
  fidl_request.add_data(std::move(initial_data), 4, 10);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));
  ASSERT_OK(fidl_request.PhysMap(fake_bti));

  size_t actual;
  fake_bti_pinned_vmo_info_t info[1];
  ASSERT_OK(fake_bti_get_pinned_vmos(fake_bti.get(), info, 1, &actual));
  ASSERT_EQ(actual, 1u);

  void* mapped = nullptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, zx::vmo(info[0].vmo),
                                       0, info[0].size, reinterpret_cast<uintptr_t*>(&mapped)));

  uint8_t expected_vals[] = {0xAA, 0xBB, 0xCC, 0xDD};
  memcpy(static_cast<char*>(mapped) + 10, expected_vals, sizeof(expected_vals));
  ASSERT_OK(zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(mapped), info[0].size));

  ASSERT_OK(fidl_request.Unpin());

  const auto& buf = fidl_request->data()->at(0).buffer()->data().value();
  ASSERT_EQ(buf.size(), 20u);
  EXPECT_EQ(buf[0], 0);
  EXPECT_EQ(buf[9], 0);
  EXPECT_BYTES_EQ(buf.data() + 10, expected_vals, sizeof(expected_vals));
}

// Verifies that Unpin() bounds copy-back size to (mapped.size - offset) when region size >
// (mapped.size - offset).
// Disabled: PhysMap() and Unpin() require bounds validation for offset overruns.
TEST(RequestFidlTest, DISABLED_UnpinOffsetOverrunBoundsTest) {
  usb::FidlRequest fidl_request;
  std::vector<uint8_t> initial_data(1000, 0);
  fidl_request.add_data(std::move(initial_data), 900, 100);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));
  ASSERT_OK(fidl_request.PhysMap(fake_bti));

  // Override payload region size to 5000 (exceeding mapped.size - offset = 4096 - 100 = 3996)
  fidl_request->data()->at(0).size(5000);

  // Unpin must safely limit copy-back size without reading past VMAR mapped bounds.
  ASSERT_OK(fidl_request.Unpin());
}

// Verifies PhysMap behavior for pre-registered kVmoId buffers.
TEST(RequestFidlTest, VmoIdTest) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(0))
      .offset(0)
      .size(16);
  request.data()
      ->emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(1))
      .offset(0)
      .size(16);
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(2))
      .offset(0)
      .size(16);
  usb::FidlRequest fidl_request(std::move(request));

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  EXPECT_OK(fidl_request.PhysMap(fake_bti));
  size_t actual;
  EXPECT_OK(fake_bti_get_pinned_vmos(fake_bti.get(), nullptr, 0, &actual));
  EXPECT_EQ(actual, 0);
}

// Verifies BTI physical pinning, VMO creation, and page mapping for inline kData buffers.
TEST(RequestFidlTest, DataTest) {
  fuchsia_hardware_usb_request::Request request;
  uint8_t expected1[] = {0xF, 0xE, 0xD, 0xC, 0xB, 0xA, 0x9, 0x8,
                         0x7, 0x6, 0x5, 0x4, 0x3, 0x2, 0x1, 0x0};
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithData(
          std::vector<uint8_t>(std::begin(expected1), std::end(expected1))))
      .offset(0)
      .size(16);
  uint8_t expected2[] = {0x0, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8, 0x9, 0xA,
                         0xB, 0xC, 0xD, 0xE, 0xF, 0x0, 0x1, 0x2, 0x3, 0x4, 0x5,
                         0x6, 0x7, 0x8, 0x9, 0xA, 0xB, 0xC, 0xD, 0xE, 0xF};
  request.data()
      ->emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithData(
          std::vector<uint8_t>(std::begin(expected2), std::end(expected2))))
      .offset(0)
      .size(32);
  usb::FidlRequest fidl_request(std::move(request));

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  EXPECT_OK(fidl_request.PhysMap(fake_bti));
  size_t actual;
  fake_bti_pinned_vmo_info_t info[2];
  EXPECT_OK(fake_bti_get_pinned_vmos(fake_bti.get(), info, 2, &actual));
  EXPECT_EQ(actual, 2);

  void* mapped;
  EXPECT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, zx::vmo(info[0].vmo),
                                       0, info[0].size, reinterpret_cast<uintptr_t*>(&mapped)));
  auto iter1 = fidl_request.phys_iter(0, zx_system_get_page_size());
  EXPECT_BYTES_EQ(mapped, expected1, sizeof(expected1));
  EXPECT_EQ((*iter1.begin()).second, 16);
  EXPECT_OK(zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(mapped), info[0].size));

  EXPECT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, zx::vmo(info[1].vmo),
                                       0, info[1].size, reinterpret_cast<uintptr_t*>(&mapped)));
  auto iter2 = fidl_request.phys_iter(1, zx_system_get_page_size());
  EXPECT_BYTES_EQ(mapped, expected2, sizeof(expected2));
  EXPECT_EQ((*iter2.begin()).second, 32);
  EXPECT_OK(zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(mapped), info[1].size));
}

// Verifies that Unpin() safely handles teardown when clear_buffers() shrank payload vectors to size
// 0.
TEST(RequestFidlTest, UnpinAfterClearBuffersTest) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(
          fuchsia_hardware_usb_request::Buffer::WithData(std::vector<uint8_t>{0x1, 0x2, 0x3, 0x4}))
      .offset(0)
      .size(4);
  usb::FidlRequest fidl_request(std::move(request));

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  EXPECT_OK(fidl_request.PhysMap(fake_bti));

  // Wipe the payload vector size to 0
  fidl_request.clear_buffers();

  // fidl_request destructor is called, triggering Unpin().
  // This verifies Unpin() gracefully handles vector capacity shrinkage.
  EXPECT_OK(fidl_request.Unpin());

  auto& restored_data = fidl_request->data()->at(0).buffer()->data().value();
  EXPECT_EQ(restored_data.size(), 0u);
  EXPECT_GE(restored_data.capacity(), 4u);
}

// Verifies that PhysMap gracefully returns ZX_ERR_INVALID_ARGS if a kData region omits its size
// field.
TEST(RequestFidlTest, PhysMapMissingRegionSizeForDataGracefulFailure) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithData(std::vector<uint8_t>(32, 0)))
      .offset(0);
  // Omitted size

  usb::FidlRequest fidl_request(std::move(request));

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  EXPECT_EQ(fidl_request.PhysMap(fake_bti), ZX_ERR_INVALID_ARGS);
}

// Verifies clear_buffers() clearing for kData buffers.
TEST(RequestFidlTest, ClearBuffersTest) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithData(std::vector<uint8_t>(32, 0)))
      .offset(0)
      .size(16);
  usb::FidlRequest fidl_request(std::move(request));

  fidl_request.clear_buffers();
  fidl_request->data()->at(0).size(
      16);  // Explicit logical size provisioning required by architecture

  std::vector<uint8_t> src(16, 0xAB);
  auto copied = fidl_request.CopyTo(0, src.data(), 16, MakeNulloptMock);

  ASSERT_EQ(copied.size(), 1);
  EXPECT_EQ(copied[0], 16);
  for (size_t i = 0; i < 16; ++i) {
    EXPECT_EQ(fidl_request->data()->at(0).buffer()->data()->at(i), 0xAB);
  }
}

// Verifies that clear_buffers() resets payload size to 0 on kVmoId buffers for transfer recycling.
TEST(RequestFidlTest, ClearBuffersVmoIdTest) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  uintptr_t mapped_addr = 0;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                       zx_system_get_page_size(), &mapped_addr));
  auto unmap_guard = fit::defer([mapped_addr]() {
    EXPECT_OK(zx::vmar::root_self()->unmap(mapped_addr, zx_system_get_page_size()));
  });
  auto get_mapped = [mapped_addr](const fuchsia_hardware_usb_request::Buffer& buffer)
      -> zx::result<std::optional<usb::internal::MappedVmo>> {
    return zx::ok(usb::internal::MappedVmo{mapped_addr, zx_system_get_page_size()});
  };

  usb::FidlRequest fidl_request;
  // Transfer #1: Simulate an initial 86-byte transfer on a kVmoId buffer.
  fidl_request.add_vmo_id(0, 86, 0);

  // Transfer #2: Clear buffers and try to copy 235 bytes into the kVmoId request.
  fidl_request.clear_buffers();
  ASSERT_EQ(fidl_request->data()->at(0).size().value_or(999), 0u);

  std::vector<uint8_t> src(235, 0xAB);
  auto copied = fidl_request.CopyTo(0, src.data(), 235, get_mapped);

  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 235u);
}

// Verifies PhysMap behavior on requests combining both kVmoId and kData buffer regions.
TEST(RequestFidlTest, MixedTest) {
  std::vector<uint8_t> tmp(32);
  usb::FidlRequest fidl_request;
  fidl_request.add_vmo_id(3, 16, 0).add_vmo_id(7, 16, 0).add_data(std::move(tmp), 32, 0);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  EXPECT_OK(fidl_request.PhysMap(fake_bti));
  size_t actual;
  EXPECT_OK(fake_bti_get_pinned_vmos(fake_bti.get(), nullptr, 0, &actual));
  EXPECT_EQ(actual, 1);

  auto iter = fidl_request.phys_iter(2, zx_system_get_page_size());
  EXPECT_EQ((*iter.begin()).second, 32);
}

// Verifies FidlRequestPool allocation, recycling, and lifecycle management.
TEST(RequestFidlTest, PoolTest) {
  usb::FidlRequestPool pool;
  EXPECT_TRUE(pool.Empty());

  pool.Add(usb::FidlRequest(usb::EndpointType::BULK));
  EXPECT_TRUE(pool.Full());
  usb::FidlRequest control(usb::EndpointType::CONTROL);
  control.add_vmo_id(9, 2, 0).add_vmo_id(1, 4, 0);
  pool.Add(std::move(control));
  EXPECT_TRUE(pool.Full());

  {
    auto req = pool.Get();
    EXPECT_TRUE(req.has_value());
    EXPECT_FALSE(pool.Empty());
    EXPECT_FALSE(pool.Full());
    EXPECT_EQ(req->request().information()->Which(),
              fuchsia_hardware_usb_request::RequestInfo::Tag::kBulk);

    pool.Put(std::move(*req));
    EXPECT_TRUE(pool.Full());
  }

  {
    auto req = pool.Get();
    EXPECT_TRUE(req.has_value());
    EXPECT_FALSE(pool.Empty());
    EXPECT_FALSE(pool.Full());
    EXPECT_EQ(req->request().information()->Which(),
              fuchsia_hardware_usb_request::RequestInfo::Tag::kControl);
    EXPECT_EQ(req->request().data()->size(), 2);
    EXPECT_EQ(req->request().data()->at(0).buffer()->vmo_id().value(), 9);
    EXPECT_EQ(req->request().data()->at(0).size(), 2);
    EXPECT_EQ(req->request().data()->at(0).offset(), 0);
    EXPECT_EQ(req->request().data()->at(1).buffer()->vmo_id().value(), 1);
    EXPECT_EQ(req->request().data()->at(1).size(), 4);
    EXPECT_EQ(req->request().data()->at(1).offset(), 0);
  }

  {
    auto req = pool.Remove();
    EXPECT_TRUE(req.has_value());
    EXPECT_TRUE(pool.Empty());

    pool.Put(std::move(*req));
    EXPECT_TRUE(pool.Full());
  }

  {
    auto req = pool.Remove();
    EXPECT_TRUE(req.has_value());
    EXPECT_TRUE(pool.Empty());
  }

  {
    auto req = pool.Remove();
    EXPECT_FALSE(req.has_value());
  }

  // Make sure that we are able to destruct even with requests sitting in the pool.
  pool.Add(usb::FidlRequest(usb::EndpointType::BULK));
  EXPECT_TRUE(pool.Full());
}

// Verifies CachedCopyTo, CachedCopyFrom, zero-length copies, and mapping error propagation.
TEST(RequestFidlTest, CopyAndCacheTest) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  uintptr_t mapped_addr = 0;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                       zx_system_get_page_size(), &mapped_addr));
  auto unmap_guard = fit::defer([mapped_addr]() {
    EXPECT_OK(zx::vmar::root_self()->unmap(mapped_addr, zx_system_get_page_size()));
  });
  auto get_mapped = [mapped_addr](const fuchsia_hardware_usb_request::Buffer& buffer)
      -> zx::result<std::optional<usb::internal::MappedVmo>> {
    return zx::ok(usb::internal::MappedVmo{mapped_addr, zx_system_get_page_size()});
  };

  usb::FidlRequest fidl_request;
  fidl_request.add_vmo_id(0, 16, 0);

  uint8_t write_data[32] = {1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15, 16,
                            17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32};
  auto cp_res = fidl_request.CachedCopyTo(0, write_data, 16, get_mapped);
  ASSERT_OK(cp_res.status_value());
  ASSERT_EQ(cp_res.value().size(), 1u);
  EXPECT_EQ(cp_res.value()[0], 16u);

  uint8_t read_data[32] = {};
  cp_res = fidl_request.CachedCopyFrom(0, read_data, 16, get_mapped);
  ASSERT_OK(cp_res.status_value());
  ASSERT_EQ(cp_res.value().size(), 1u);
  EXPECT_EQ(cp_res.value()[0], 16u);
  EXPECT_BYTES_EQ(read_data, write_data, 16);

  // Zero-length copies.
  cp_res = fidl_request.CachedCopyTo(0, write_data, 0, get_mapped);
  ASSERT_OK(cp_res.status_value());
  ASSERT_EQ(cp_res.value().size(), 1u);
  EXPECT_EQ(cp_res.value()[0], 0u);
  cp_res = fidl_request.CachedCopyFrom(0, read_data, 0, get_mapped);
  ASSERT_OK(cp_res.status_value());
  ASSERT_EQ(cp_res.value().size(), 1u);
  EXPECT_EQ(cp_res.value()[0], 0u);

  // Out-of-bounds offset test.
  cp_res = fidl_request.CachedCopyTo(100, write_data, 16, get_mapped);
  ASSERT_STATUS(cp_res.status_value(), ZX_ERR_OUT_OF_RANGE);
  cp_res = fidl_request.CachedCopyFrom(100, read_data, 16, get_mapped);
  ASSERT_STATUS(cp_res.status_value(), ZX_ERR_OUT_OF_RANGE);

  // Scatter-Gather multi-region test.
  usb::FidlRequest sg_request;
  sg_request.add_vmo_id(0, 16, 0).add_vmo_id(0, 16, 16);
  cp_res = sg_request.CachedCopyTo(0, write_data, 32, get_mapped);
  ASSERT_OK(cp_res.status_value());
  ASSERT_EQ(cp_res.value().size(), 2u);
  EXPECT_EQ(cp_res.value()[0], 16u);
  EXPECT_EQ(cp_res.value()[1], 16u);

  memset(read_data, 0, sizeof(read_data));
  cp_res = sg_request.CachedCopyFrom(0, read_data, 32, get_mapped);
  ASSERT_OK(cp_res.status_value());
  ASSERT_EQ(cp_res.value().size(), 2u);
  EXPECT_EQ(cp_res.value()[0], 16u);
  EXPECT_EQ(cp_res.value()[1], 16u);
  EXPECT_BYTES_EQ(read_data, write_data, 32);

  // Inline data buffers test.
  auto get_mapped_inline = [](const fuchsia_hardware_usb_request::Buffer& buffer)
      -> zx::result<std::optional<usb::internal::MappedVmo>> { return zx::ok(std::nullopt); };
  usb::FidlRequest inline_request;
  inline_request.add_data({}, 16, 0).add_data({}, 16, 16);
  cp_res = inline_request.CachedCopyTo(0, write_data, 32, get_mapped_inline);
  ASSERT_OK(cp_res.status_value());
  ASSERT_EQ(cp_res.value().size(), 2u);
  EXPECT_EQ(cp_res.value()[0], 16u);
  EXPECT_EQ(cp_res.value()[1], 16u);

  memset(read_data, 0, sizeof(read_data));
  cp_res = inline_request.CachedCopyFrom(0, read_data, 32, get_mapped_inline);
  ASSERT_OK(cp_res.status_value());
  ASSERT_EQ(cp_res.value().size(), 2u);
  EXPECT_EQ(cp_res.value()[0], 16u);
  EXPECT_EQ(cp_res.value()[1], 16u);
  EXPECT_BYTES_EQ(read_data, write_data, 32);

  // Inline data offset skipping test.
  cp_res = inline_request.CachedCopyTo(16, write_data, 16, get_mapped_inline);
  ASSERT_OK(cp_res.status_value());
  ASSERT_EQ(cp_res.value().size(), 2u);
  EXPECT_EQ(cp_res.value()[0], 0u);
  EXPECT_EQ(cp_res.value()[1], 16u);

  // Mapping error propagation test.
  auto bad_get_mapped = [](const fuchsia_hardware_usb_request::Buffer& buffer)
      -> zx::result<std::optional<usb::internal::MappedVmo>> {
    return zx::error(ZX_ERR_BAD_HANDLE);
  };
  cp_res = fidl_request.CachedCopyTo(0, write_data, 16, bad_get_mapped);
  ASSERT_STATUS(cp_res.status_value(), ZX_ERR_BAD_HANDLE);
  cp_res = fidl_request.CachedCopyFrom(0, read_data, 16, bad_get_mapped);
  ASSERT_STATUS(cp_res.status_value(), ZX_ERR_BAD_HANDLE);
}

// Verifies partial slice flushing, selective get_mapped callbacks, and scatter-gather cache
// flushes.
TEST(RequestFidlTest, RangeCacheFlushTest) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  uintptr_t mapped_addr = 0;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                       zx_system_get_page_size(), &mapped_addr));
  auto unmap_guard = fit::defer([mapped_addr]() {
    EXPECT_OK(zx::vmar::root_self()->unmap(mapped_addr, zx_system_get_page_size()));
  });
  auto get_mapped = [mapped_addr](const fuchsia_hardware_usb_request::Buffer& buffer)
      -> zx::result<std::optional<usb::internal::MappedVmo>> {
    return zx::ok(usb::internal::MappedVmo{mapped_addr, zx_system_get_page_size()});
  };

  // 1. Flushing a partial slice of a VMO buffer.
  usb::FidlRequest fidl_request;
  fidl_request.add_vmo_id(0, 64, 0);
  EXPECT_OK(fidl_request.CacheFlush(get_mapped, 16, 32));
  EXPECT_OK(fidl_request.CacheFlushInvalidate(get_mapped, 16, 32));

  // 2. Selective get_mapped to verify untouched preceding and subsequent regions are skipped.
  usb::FidlRequest sg_request;
  sg_request.add_vmo_id(0, 16, 0).add_vmo_id(1, 16, 16).add_vmo_id(2, 16, 32);
  auto get_mapped_selective = [mapped_addr](const fuchsia_hardware_usb_request::Buffer& buffer)
      -> zx::result<std::optional<usb::internal::MappedVmo>> {
    if (buffer.vmo_id().value() == 1) {
      return zx::ok(usb::internal::MappedVmo{mapped_addr, zx_system_get_page_size()});
    }
    return zx::error(ZX_ERR_BAD_STATE);
  };
  // Only flushing buffer 1 (offset 16 to 32) should succeed without querying buffer 0 or 2.
  EXPECT_OK(sg_request.CacheFlush(get_mapped_selective, 16, 16));
  EXPECT_OK(sg_request.CacheFlushInvalidate(get_mapped_selective, 16, 16));
  // Flushing buffer 0 or 2 should fail.
  EXPECT_STATUS(sg_request.CacheFlush(get_mapped_selective, 0, 16), ZX_ERR_BAD_STATE);
  EXPECT_STATUS(sg_request.CacheFlushInvalidate(get_mapped_selective, 0, 16), ZX_ERR_BAD_STATE);
  EXPECT_STATUS(sg_request.CacheFlush(get_mapped_selective, 32, 16), ZX_ERR_BAD_STATE);
  EXPECT_STATUS(sg_request.CacheFlushInvalidate(get_mapped_selective, 32, 16), ZX_ERR_BAD_STATE);

  // 3. Flushing across multiple scatter-gather VMO regions.
  EXPECT_OK(sg_request.CacheFlush(get_mapped, 8, 32));
  EXPECT_OK(sg_request.CacheFlushInvalidate(get_mapped, 8, 32));

  // 4. Zero-length and out-of-bounds bounds.
  EXPECT_OK(fidl_request.CacheFlush(get_mapped, 0, 0));
  EXPECT_OK(fidl_request.CacheFlushInvalidate(get_mapped, 0, 0));
  EXPECT_OK(fidl_request.CacheFlush(get_mapped, 10, 0));
  EXPECT_OK(fidl_request.CacheFlushInvalidate(get_mapped, 10, 0));
  // Out of bounds offset.
  EXPECT_STATUS(fidl_request.CacheFlush(get_mapped, 1000, 16), ZX_ERR_OUT_OF_RANGE);
  EXPECT_STATUS(fidl_request.CacheFlushInvalidate(get_mapped, 1000, 16), ZX_ERR_OUT_OF_RANGE);
  // Partial out of bounds size (starts in bounds, exceeds total length).
  EXPECT_OK(fidl_request.CacheFlush(get_mapped, 48, 100));
  EXPECT_OK(fidl_request.CacheFlushInvalidate(get_mapped, 48, 100));

  // 5. Zero-length buffer region and inline data region skipping.
  usb::FidlRequest mixed_request;
  mixed_request.add_vmo_id(0, 16, 0)
      .add_vmo_id(0, 0, 16)
      .add_data({}, 16, 16)
      .add_vmo_id(0, 16, 32);
  auto get_mapped_mixed = [mapped_addr](const fuchsia_hardware_usb_request::Buffer& buffer)
      -> zx::result<std::optional<usb::internal::MappedVmo>> {
    if (buffer.Which() == fuchsia_hardware_usb_request::Buffer::Tag::kData) {
      return zx::ok(std::nullopt);
    }
    return zx::ok(usb::internal::MappedVmo{mapped_addr, zx_system_get_page_size()});
  };
  EXPECT_OK(mixed_request.CacheFlush(get_mapped_mixed, 0, 48));
  EXPECT_OK(mixed_request.CacheFlushInvalidate(get_mapped_mixed, 0, 48));
}

// Verifies that CopyTo gracefully fails (0 bytes copied) on invalid default-constructed buffer
// tags.
// Disabled: CopyTo() requires validation for default-constructed invalid buffer tags.
TEST(RequestFidlTest, DISABLED_EmptyBufferCrashTest) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer(
          ::fidl::internal::DefaultConstructPossiblyInvalidObjectTag{}))
      .offset(0)
      .size(16);
  usb::FidlRequest fidl_request(std::move(request));

  std::vector<uint8_t> src(16, 0xAB);
  // This must not crash, it should just return 0 bytes copied or gracefully fail
  auto copied = fidl_request.CopyTo(0, src.data(), 16, MakeNulloptMock);

  ASSERT_EQ(copied.size(), 1);
  EXPECT_EQ(copied[0], 0);
}

// Verifies that PhysMap auto-allocates buffer capacity when add_data supplies an empty vector with
// non-zero size.
// Disabled: PhysMap() requires dynamic vector capacity allocation when add_data supplies empty
// vector with size.
TEST(RequestFidlTest, DISABLED_PhysMapZeroCapacityTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(), 16, 0);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  EXPECT_OK(fidl_request.PhysMap(fake_bti));
  ASSERT_TRUE(fidl_request->data().has_value());
  EXPECT_GE(fidl_request->data()->at(0).buffer()->data()->capacity(), 16u);
}

// Verifies that PhysMap returns ZX_ERR_INVALID_ARGS when offset + size overflows 64-bit integers.
TEST(RequestFidlTest, PhysMapOverflowTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(), 10, std::numeric_limits<uint64_t>::max() - 5);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  EXPECT_EQ(ZX_ERR_INVALID_ARGS, fidl_request.PhysMap(fake_bti));
}

// Verifies that PhysMap returns ZX_ERR_INVALID_ARGS when requested buffer size exceeds
// kMaxTransferSize.
// Disabled: PhysMap() requires ZX_ERR_INVALID_ARGS return code when requested size exceeds maximum
// transfer capacity.
TEST(RequestFidlTest, DISABLED_PhysMapCapacityExceedsMaxHalfTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(), std::numeric_limits<uint64_t>::max(), 0);
  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, fidl_request.PhysMap(fake_bti));
}

// Verifies that CacheFlush returns ZX_ERR_INVALID_ARGS if a kVmoId region omits its size field.
// Disabled: CacheFlush() requires optional size check for kVmoId buffer regions.
TEST(RequestFidlTest, DISABLED_MissingRegionSizeGracefulFailure) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(0))
      .offset(0);
  usb::FidlRequest fidl_request(std::move(request));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            fidl_request.CacheFlush([](const fuchsia_hardware_usb_request::Buffer&)
                                        -> zx::result<std::optional<usb::internal::MappedVmo>> {
              return zx::ok(usb::internal::MappedVmo{0x1000, 16});
            }));
}

// Verifies that add_data infers payload region size by subtracting offset from total vector size.
// Disabled: add_data() requires inferred region payload sizing (data.size() - offset).
TEST(RequestFidlTest, DISABLED_VectorPayloadInferredSizeSubtractsOffsetTest) {
  usb::FidlRequest fidl_request;
  std::vector<uint8_t> data(100);
  fidl_request.add_data(std::move(data), 0, 20);
  EXPECT_TRUE(fidl_request->data().has_value());
  EXPECT_EQ(fidl_request->data()->at(0).buffer()->data()->size(), 100u);
  EXPECT_EQ(fidl_request->data()->at(0).size().value(), 80u);
}

// Verifies that add_data infers 0 region size when vector length is smaller than offset.
// Disabled: add_data() requires offset displacement handling without dynamic length expansion.
TEST(RequestFidlTest, DISABLED_VectorPayloadInfersZeroDynamicPaddingTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(10), 0, 20);

  EXPECT_TRUE(fidl_request->data().has_value());
  EXPECT_EQ(fidl_request->data()->at(0).size().value(), 0u);
  EXPECT_EQ(fidl_request->data()->at(0).buffer()->data()->size(), 20u);
}

// Verifies that fit::defer rollback guard unpins previously pinned VMOs if a subsequent payload
// fails PhysMap.
// Disabled: PhysMap() requires fit::defer rollback unpinning for multi-payload mapping failures.
TEST(RequestFidlTest, DISABLED_PhysMapRollbackOnMultiPayloadFailureTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(100), 100, 0);
  fidl_request.add_data(std::vector<uint8_t>(0), std::numeric_limits<uint64_t>::max(), 0);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  EXPECT_EQ(ZX_ERR_INVALID_ARGS, fidl_request.PhysMap(fake_bti));
}

// Verifies that Unpin() safely tears down pinned VMOs even if request data vector was cleared.
// Disabled: Unpin() requires resource teardown safety even if request payload vector is mutated or
// cleared.
TEST(RequestFidlTest, DISABLED_PhysMapGhostedPayloadTeardownTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(100), 100, 0);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));
  ASSERT_OK(fidl_request.PhysMap(fake_bti));

  // Manually ghost the payload array by mutating it directly in the instance
  fidl_request->data()->clear();

  // Trigger Unpin() manually or via destruction to evaluate memory teardown against the ghosted
  // payload list.
  ASSERT_OK(fidl_request.Unpin());
}

// Verifies that CacheFlushInvalidate continues iterating scatter-gather regions after an error.
// Disabled: CacheFlushInvalidate() requires continuation across scatter-gather regions after
// errors.
TEST(RequestFidlTest, DISABLED_CacheFlushInvalidateContinuationTest) {
  fuchsia_hardware_usb_request::Request request;
  request.data().emplace();
  request.data()
      ->emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(1))
      .offset(100)
      .size(100);
  request.data()
      ->emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(2))
      .offset(0)
      .size(10);
  usb::FidlRequest fidl_request(std::move(request));

  std::vector<uint8_t> valid_memory1(1000);
  std::vector<uint8_t> valid_memory2(1000);
  bool second_processed = false;
  auto status =
      fidl_request.CacheFlushInvalidate([&](const fuchsia_hardware_usb_request::Buffer& b)
                                            -> zx::result<std::optional<usb::internal::MappedVmo>> {
        // In FIDL C++ Natural bindings, b.vmo_id() returns a
        // UnionMemberView<std::optional<uint64_t>>; .value() extracts the underlying scalar
        // uint64_t for comparison.
        if (b.vmo_id().value() == 1)
          return zx::ok(
              usb::internal::MappedVmo{reinterpret_cast<zx_vaddr_t>(valid_memory1.data()), 10});
        if (b.vmo_id().value() == 2) {
          second_processed = true;
          return zx::ok(
              usb::internal::MappedVmo{reinterpret_cast<zx_vaddr_t>(valid_memory2.data()), 100});
        }
        return zx::ok(std::nullopt);
      });

  EXPECT_STATUS(status, ZX_ERR_OUT_OF_RANGE);
  EXPECT_TRUE(second_processed);
}

// Verifies that CopyTo returns 0 bytes copied when region offset overflows.
// Disabled: CopyTo() requires 64-bit integer overflow protection.
TEST(RequestFidlTest, DISABLED_CopyToOverflowTest) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithData(std::vector<uint8_t>(16)))
      .offset(std::numeric_limits<uint64_t>::max())
      .size(16);
  usb::FidlRequest fidl_request(std::move(request));

  std::vector<uint8_t> src(16, 0xAB);
  auto copied = fidl_request.CopyTo(0, src.data(), 16, MakeNulloptMock);
  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 0u);
}

// Verifies that CopyFrom returns 0 bytes copied when region offset overflows.
// Disabled: CopyFrom() requires 64-bit integer overflow protection.
TEST(RequestFidlTest, DISABLED_CopyFromOverflowTest) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithData(std::vector<uint8_t>(16)))
      .offset(std::numeric_limits<uint64_t>::max())
      .size(16);
  usb::FidlRequest fidl_request(std::move(request));

  std::vector<uint8_t> dest(16, 0);
  auto copied = fidl_request.CopyFrom(0, dest.data(), 16, MakeNulloptMock);
  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 0u);
}

// Verifies that PhysMap permits kVmoId pre-mapped VMO requests larger than kMaxTransferSize (e.g.
// 2048-byte MTU buffers).
TEST(RequestFidlTest, PhysMapMtuSizedVmoTest) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(1))
      .offset(0)
      .size(2048);
  usb::FidlRequest fidl_request(std::move(request));

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  EXPECT_OK(fidl_request.PhysMap(fake_bti));
}

// Verifies that phys_iter() correctly calculates initial physical address for unaligned non-zero
// offsets without double-adding sub_offset.
// Disabled: phys_iter() requires address calculation for unaligned non-zero offsets.
TEST(RequestFidlTest, DISABLED_PhysIterNonZeroOffsetAddressTest) {
  usb::FidlRequest fidl_request;
  std::vector<uint8_t> data(256, 0xAA);
  fidl_request.add_data(std::move(data), 128, 100);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));
  ASSERT_OK(fidl_request.PhysMap(fake_bti));

  auto iter = fidl_request.phys_iter(0, zx_system_get_page_size());
  auto it = iter.begin();
  ASSERT_NE(it, iter.end());
  auto [paddr, len] = *it;
  EXPECT_GT(paddr, 0u);
  EXPECT_EQ(len, 128u);
  EXPECT_EQ(paddr % zx_system_get_page_size(), 100u);
}

// Verifies that clear_buffers and reset_buffers gracefully handle unpopulated BufferRegions without
// crashing on Which().
// Disabled: clear_buffers() and reset_buffers() require checks for unpopulated optional buffer
// regions.
TEST(RequestFidlTest, DISABLED_UnpopulatedBufferRegionClearAndResetTest) {
  fuchsia_hardware_usb_request::Request request;
  // Create a BufferRegion with unpopulated buffer optional
  request.data().emplace().emplace_back().offset(0).size(16);
  usb::FidlRequest fidl_request(std::move(request));

  fidl_request.clear_buffers();

  auto get_mapped = [](const fuchsia_hardware_usb_request::Buffer&)
      -> zx::result<std::optional<usb::internal::MappedVmo>> { return zx::ok(std::nullopt); };
  fidl_request.reset_buffers(get_mapped);
}

// Verifies that CopyTo respects non-zero offset on kData inline vectors without overwriting header
// or truncating.
// Disabled: CopyTo() requires dynamic inline vector capacity expansion for non-zero offsets.
TEST(RequestFidlTest, DISABLED_KDataCopyToNonZeroOffsetTest) {
  usb::FidlRequest fidl_request;
  std::vector<uint8_t> initial_data(32, 0xAA);
  fidl_request.add_data(initial_data, 16, 16);

  std::vector<uint8_t> src(16, 0xBB);
  auto copied = fidl_request.CopyTo(0, src.data(), 16, MakeNulloptMock);
  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 16u);

  // Buffer length must be at least offset + copied = 32
  ASSERT_TRUE(fidl_request.request().data().has_value());
  const auto& vec = fidl_request.request().data()->at(0).buffer()->data().value();
  ASSERT_GE(vec.size(), 32u);

  // Index 0..15 must preserve header 0xAA
  for (size_t i = 0; i < 16; i++) {
    EXPECT_EQ(vec[i], 0xAA);
  }
  // Index 16..31 must contain copied data 0xBB
  for (size_t i = 16; i < 32; i++) {
    EXPECT_EQ(vec[i], 0xBB);
  }

  // Test contiguous appending at reg_offset == vec_size (offset 32 on 32-byte vector)
  usb::FidlRequest append_request;
  std::vector<uint8_t> base_data(32, 0x11);
  append_request.add_data(base_data, 16, 32);

  std::vector<uint8_t> append_src(16, 0x22);
  auto append_copied = append_request.CopyTo(0, append_src.data(), 16, MakeNulloptMock);
  ASSERT_EQ(append_copied.size(), 1u);
  EXPECT_EQ(append_copied[0], 16u);
  ASSERT_TRUE(append_request.request().data().has_value());
  const auto& append_vec = append_request.request().data()->at(0).buffer()->data().value();
  ASSERT_GE(append_vec.size(), 48u);
  for (size_t i = 0; i < 32; i++) {
    EXPECT_EQ(append_vec[i], 0x11);
  }
  for (size_t i = 32; i < 48; i++) {
    EXPECT_EQ(append_vec[i], 0x22);
  }
}

// Verifies that CopyFrom respects non-zero offset on kData inline vectors without reading from
// index 0.
// Disabled: CopyFrom() requires offset region slicing for inline kData vectors.
TEST(RequestFidlTest, DISABLED_KDataCopyFromNonZeroOffsetTest) {
  usb::FidlRequest fidl_request;
  std::vector<uint8_t> initial_data(32, 0xAA);
  for (size_t i = 16; i < 32; i++) {
    initial_data[i] = 0xBB;
  }
  fidl_request.add_data(initial_data, 16, 16);

  std::vector<uint8_t> dst(16, 0);
  auto copied = fidl_request.CopyFrom(0, dst.data(), 16, MakeNulloptMock);
  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 16u);

  // Must read from offset 16 (0xBB), not offset 0 (0xAA)
  for (size_t i = 0; i < 16; i++) {
    EXPECT_EQ(dst[i], 0xBB);
  }
}

// Verifies that CopyTo with cur_offset >= region_size advances correctly across scatter-gather
// regions without truncating pre-existing vector capacity.
// Disabled: On origin/main, CopyTo() resizes Region 2's backing vector to std::min(cur_offset +
// todo, buffer_size), truncating the vector from 10 to 2 bytes while leaving the logical size
// at 10. Activated in CL 2.
TEST(RequestFidlTest, DISABLED_ScatterGatherCopyToNonZeroOffsetTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(10, 0x11), 10, 0);
  fidl_request.add_data(std::vector<uint8_t>(10, 0x22), 10, 0);
  fidl_request.add_data(std::vector<uint8_t>(10, 0x33), 10, 0);

  std::vector<uint8_t> src(10, 0xFF);
  // Copy 10 bytes starting at offset 12 (skips region 0, copies 8 into region 1, 2 into region 2).
  auto copied = fidl_request.CopyTo(12, src.data(), 10, MakeNulloptMock);
  ASSERT_EQ(copied.size(), 3u);
  EXPECT_EQ(copied[0], 0u);
  EXPECT_EQ(copied[1], 8u);
  EXPECT_EQ(copied[2], 2u);

  ASSERT_TRUE(fidl_request.request().data().has_value());
  EXPECT_EQ(fidl_request.request().data()->at(0).buffer()->data()->size(), 10u);
  EXPECT_EQ(fidl_request.request().data()->at(1).buffer()->data()->size(), 10u);
  // Region 2's backing vector must not be truncated to 2 bytes by CopyTo
  EXPECT_EQ(fidl_request.request().data()->at(2).buffer()->data()->size(), 10u);
}

// Verifies that CopyTo retains pre-configured region size (20u) when copying shorter payloads (5u).
TEST(RequestFidlTest, CopyToPreservesConfiguredPayloadSizeTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(20, 0xAA), 20, 0);

  std::vector<uint8_t> src(5, 0xBB);
  auto copied = fidl_request.CopyTo(0, src.data(), 5, MakeNulloptMock);
  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 5u);

  // Region payload size retains pre-configured capacity (20u) without truncation
  ASSERT_TRUE(fidl_request.request().data().has_value());
  EXPECT_EQ(fidl_request.request().data()->at(0).size().value(), 20u);
}

// Verifies that reset_buffers() restores full VMO capacity when resetting offset to 0.
TEST(RequestFidlTest, ResetBuffersRestoresFullVmoSizeTest) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(1))
      .offset(20)
      .size(80);
  usb::FidlRequest fidl_request(std::move(request));

  fidl_request.reset_buffers([](const fuchsia_hardware_usb_request::Buffer&)
                                 -> zx::result<std::optional<usb::internal::MappedVmo>> {
    return zx::ok(usb::internal::MappedVmo{0x1000, 100});
  });

  ASSERT_TRUE(fidl_request.request().data().has_value());
  EXPECT_EQ(fidl_request.request().data()->at(0).offset().value_or(0), 0u);
  EXPECT_EQ(fidl_request.request().data()->at(0).size().value_or(0), 100u);
}

// Verifies that sequential CopyTo calls expand d.size() rather than truncating it.
TEST(RequestFidlTest, SequentialCopyToDoesNotTruncatePayloadSizeTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(20, 0), 20, 0);

  std::vector<uint8_t> src1(10, 0xAA);
  auto copied1 = fidl_request.CopyTo(0, src1.data(), 10, MakeNulloptMock);
  ASSERT_EQ(copied1.size(), 1u);
  EXPECT_EQ(copied1[0], 10u);

  std::vector<uint8_t> src2(10, 0xBB);
  auto copied2 = fidl_request.CopyTo(10, src2.data(), 10, MakeNulloptMock);
  ASSERT_EQ(copied2.size(), 1u);
  EXPECT_EQ(copied2[0], 10u);
}

// Verifies that PhysMap correctly calculates pin_length using end_offset - page_offset.
// Disabled: PhysMap() requires page span boundary math calculation using end_offset - page_offset.
TEST(RequestFidlTest, DISABLED_PhysMapPageSpanCalculationTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(200, 0xAB), 50, 100);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));

  ASSERT_OK(fidl_request.PhysMap(fake_bti));

  size_t actual;
  fake_bti_pinned_vmo_info_t info[1];
  ASSERT_OK(fake_bti_get_pinned_vmos(fake_bti.get(), info, 1, &actual));
  ASSERT_EQ(actual, 1u);
  // Span from offset 100 with size 50 ends at 150, page_offset is 0, requiring 4096 bytes (1 page).
  EXPECT_EQ(info[0].size, 4096u);
}

// Verifies that Unpin() clamps copy-back size if an inline vector is shrunk.
// Disabled: Unpin() requires vector bounds clamping to prevent heap buffer overrun on shrunk
// vectors.
TEST(RequestFidlTest, DISABLED_UnpinClampsToVectorSizeTest) {
  usb::FidlRequest fidl_request;
  std::vector<uint8_t> initial_data(20, 0);
  fidl_request.add_data(std::move(initial_data), 10, 0);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));
  ASSERT_OK(fidl_request.PhysMap(fake_bti));

  size_t actual;
  fake_bti_pinned_vmo_info_t info[1];
  ASSERT_OK(fake_bti_get_pinned_vmos(fake_bti.get(), info, 1, &actual));
  ASSERT_EQ(actual, 1u);

  void* mapped = nullptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, zx::vmo(info[0].vmo),
                                       0, info[0].size, reinterpret_cast<uintptr_t*>(&mapped)));
  memset(mapped, 0x77, 10);
  ASSERT_OK(zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(mapped), info[0].size));

  // Shrink the vector under fidl_request before Unpin()
  (*fidl_request->data())[0].buffer()->data()->resize(5);

  // Unpin must clamp copy-back size to current vector length (5)
  EXPECT_OK(fidl_request.Unpin());

  const auto& vec = (*fidl_request->data())[0].buffer()->data().value();
  ASSERT_EQ(vec.size(), 5u);
  EXPECT_EQ(vec[0], 0x77);
  EXPECT_EQ(vec[4], 0x77);
}

// Verifies that PhysMap and Unpin cleanly manage temporary VMO handles during kData mapping.
TEST(RequestFidlTest, PhysMapDoesNotLeakVmoHandlesTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(100, 0xAA), 100, 0);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));
  ASSERT_OK(fidl_request.PhysMap(fake_bti));

  EXPECT_OK(fidl_request.Unpin());
}

// Verifies that if a region fails PhysMap, fit::defer cleanup successfully unmaps mapped VMAR
// regions.
// Disabled: PhysMap() requires fit::defer mapping rollback on pin failures.
TEST(RequestFidlTest, DISABLED_PhysMapRollbackUnmapsVmarOnPinFailureTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(100, 0xBB), 100, 0);
  fidl_request.add_data(std::vector<uint8_t>(0), std::numeric_limits<uint64_t>::max(), 0);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));
  EXPECT_EQ(fidl_request.PhysMap(fake_bti), ZX_ERR_INVALID_ARGS);

  // Verification that Unpin() ran cleanup without crashing or asserting on invalid PMTs
  EXPECT_OK(fidl_request.Unpin());
}

// Verifies that Unpin() safely tears down kernel memory and VMAR mappings even if request_.data()
// was cleared or mutated after PhysMap().
// Disabled: Unpin() requires unconditional resource teardown when payload vector is cleared.
TEST(RequestFidlTest, DISABLED_UnpinWithMutatedRequestDataTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(100, 0xAA), 100, 0);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));
  ASSERT_OK(fidl_request.PhysMap(fake_bti));

  // Mutate request_.data() by clearing all regions after PhysMap().
  fidl_request->data()->clear();

  // Unpin must cleanly tear down BTI and VMAR resources without throwing out_of_range or crashing.
  EXPECT_OK(fidl_request.Unpin());
}

// Verifies that Unpin() safely tears down kernel memory and VMAR mappings even if request_.data()
// optional was reset to nullopt after PhysMap().
// Disabled: Unpin() requires optional dereference check when request data optional is reset.
TEST(RequestFidlTest, DISABLED_UnpinWithResetRequestDataOptionalTest) {
  usb::FidlRequest fidl_request;
  fidl_request.add_data(std::vector<uint8_t>(100, 0xAA), 100, 0);

  zx::bti fake_bti;
  ASSERT_OK(fake_bti_create(fake_bti.reset_and_get_address()));
  ASSERT_OK(fidl_request.PhysMap(fake_bti));

  // Reset optional request_.data() after PhysMap().
  fidl_request->data().reset();

  // Unpin must cleanly tear down BTI and VMAR resources without nullopt dereference crash.
  EXPECT_OK(fidl_request.Unpin());
}

// Verifies that CopyTo dynamically expands an inline kData vector beyond its initial capacity up
// to kMaxTransferSize (1024 bytes) without truncating writes.
// Disabled: CopyTo() requires dynamic expansion beyond existing vector capacity.
TEST(RequestFidlTest, DISABLED_KDataCopyToDynamicGrowthBeyondInitialCapacityTest) {
  usb::FidlRequest fidl_request;
  std::vector<uint8_t> initial_data(20, 0x11);
  fidl_request.add_data(std::move(initial_data), 50, 0);

  std::vector<uint8_t> src(50, 0x22);
  auto copied = fidl_request.CopyTo(0, src.data(), 50, MakeNulloptMock);
  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 50u);
  EXPECT_EQ(fidl_request->data()->at(0).buffer()->data()->size(), 50u);
  EXPECT_EQ(fidl_request->data()->at(0).size().value(), 50u);
  for (size_t i = 0; i < 50; ++i) {
    EXPECT_EQ(fidl_request->data()->at(0).buffer()->data()->at(i), 0x22);
  }
}

// Verifies that CopyFrom strictly bounds reads to initialized vector size() and does not copy
// uninitialized memory from extra capacity.
// Disabled: CopyFrom() requires bounds calculation strictly restricted to size().
TEST(RequestFidlTest, DISABLED_KDataCopyFromDoesNotReadUninitializedCapacityTest) {
  usb::FidlRequest fidl_request;
  std::vector<uint8_t> data;
  data.reserve(64);
  data.resize(10, 0xAA);
  fidl_request.add_data(std::move(data), 10, 0);

  std::vector<uint8_t> dst(64, 0x00);
  auto copied = fidl_request.CopyFrom(0, dst.data(), 64, MakeNulloptMock);
  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 10u);
  for (size_t i = 0; i < 10; ++i) {
    EXPECT_EQ(dst[i], 0xAA);
  }
  for (size_t i = 10; i < 64; ++i) {
    EXPECT_EQ(dst[i], 0x00);
  }
}

// Verifies that CopyFrom on an empty kData vector returns 0 bytes copied and does not read from
// unallocated buffers.
// Disabled: CopyFrom() requires 0-byte available capacity check on empty vectors.
TEST(RequestFidlTest, DISABLED_KDataCopyFromEmptyVectorDoesNotReadGarbageTest) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithData(std::vector<uint8_t>()))
      .offset(0)
      .size(20);
  usb::FidlRequest fidl_request(std::move(request));

  std::vector<uint8_t> dst(20, 0xFF);
  auto copied = fidl_request.CopyFrom(0, dst.data(), 20, MakeNulloptMock);
  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 0u);
}

}  // namespace
