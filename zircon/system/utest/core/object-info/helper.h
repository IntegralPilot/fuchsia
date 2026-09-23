// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_OBJECT_INFO_HELPER_H_
#define ZIRCON_SYSTEM_UTEST_CORE_OBJECT_INFO_HELPER_H_

#include <lib/fit/defer.h>
#include <lib/zx/process.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls/object.h>

#include <type_traits>

#include <zxtest/zxtest.h>

namespace object_info_test {

template <typename EntryType, uint32_t EntryCount = 1, class HandleType>
void CheckSelfInfoSucceeds(zx_object_info_topic_t topic, const HandleType& self) {
  EntryType entries[EntryCount];
  size_t actual;
  size_t avail;

  ASSERT_OK(self.get_info(topic, entries, sizeof(entries), &actual, &avail));
}

// Invalid handles should fail.
template <typename EntryType, uint32_t EntryCount = 1, typename HandleProvider>
void CheckInvalidHandleFails(zx_object_info_topic_t topic, const HandleProvider& provider) {
  EntryType entries[EntryCount];
  size_t actual;
  size_t avail;
  std::decay_t<decltype(provider())> handle;

  ASSERT_EQ(handle.get_info(topic, entries, sizeof(entries), &actual, &avail), ZX_ERR_BAD_HANDLE);
}

// Call should fail if the handle type does not support the requested topic.
template <typename EntryType, uint32_t EntryCount = 1, typename HandleProvider>
void CheckWrongHandleTypeFails(zx_object_info_topic_t topic, const HandleProvider& provider) {
  EntryType entries[EntryCount];
  size_t actual;
  size_t avail;
  const auto& handle = provider();

  ASSERT_NOT_OK(handle.get_info(topic, entries, sizeof(entries), &actual, &avail));
}

// Calls without enough rights, should fail with ZX_ERR_ACCESS_DENIED
template <typename EntryType, uint32_t EntryCount, typename HandleProvider>
void CheckMissingRightsFail(zx_object_info_topic_t topic, zx_rights_t missing_rights,
                            const HandleProvider& provider) {
  const auto& handle = provider();
  EntryType entries[EntryCount];
  size_t actual;
  size_t avail;

  // Verify handle rights are present.
  zx_info_handle_basic_t handle_info;
  ASSERT_OK(
      handle.get_info(ZX_INFO_HANDLE_BASIC, &handle_info, sizeof(handle_info), nullptr, nullptr));
  ASSERT_EQ(handle_info.rights & missing_rights, missing_rights, "rights 0x%" PRIx32,
            handle_info.rights);

  // Create a handle without the important rights.
  std::decay_t<decltype(handle)> unpriviledged_handle;
  ASSERT_OK(handle.duplicate(handle_info.rights & ~missing_rights, &unpriviledged_handle));

  // Call should fail without these rights.
  EXPECT_EQ(unpriviledged_handle.get_info(topic, entries, sizeof(entries), &actual, &avail),
            ZX_ERR_ACCESS_DENIED);
}

// Passing a zero-sized buffer to a topic that expects a single
// in/out entry should fail.
template <typename EntryType, typename HandleProvider>
void CheckZeroSizeBufferFails(zx_object_info_topic_t topic, const HandleProvider& provider) {
  EntryType entry;
  const auto& handle = provider();
  size_t actual;
  size_t avail;

  EXPECT_EQ(handle.get_info(topic,
                            &entry,  // buffer
                            0,       // len
                            &actual, &avail),
            ZX_ERR_BUFFER_TOO_SMALL);
  EXPECT_EQ(0u, actual);
  EXPECT_GT(avail, 0u);
}

// Passing a zero-sized buffer to a topic that expects a multiple
// in/out entry should succeed.
template <typename EntryType, typename HandleProvider>
void CheckZeroSizeBufferSucceeds(zx_object_info_topic_t topic, const HandleProvider& provider) {
  EntryType entry;
  const auto& handle = provider();
  size_t actual;
  size_t avail;

  EXPECT_OK(handle.get_info(topic,
                            &entry,  // buffer
                            0,       // len
                            &actual, &avail));
  EXPECT_EQ(0u, actual);
  EXPECT_GT(avail, 0u);
}

// Passing a nullptr buffer to a topic that expects a single
// in/out entry should fail.
template <typename HandleProvider>
void CheckNullBufferSucceeds(zx_object_info_topic_t topic, const HandleProvider& provider) {
  const auto& handle = provider();
  size_t actual;
  size_t avail;

  EXPECT_OK(handle.get_info(topic,
                            nullptr,  // buffer
                            0,        // len
                            &actual, &avail));
  EXPECT_EQ(0u, actual);
  EXPECT_GT(avail, 0u);
}

// Passing a buffer shorter than avail should succeed.
template <typename EntryType, uint32_t EntryCount = 1, typename HandleProvider>
void CheckSmallBufferSucceeds(zx_object_info_topic_t topic, const HandleProvider& provider) {
  EntryType entries[EntryCount];
  size_t actual;
  size_t avail;
  const auto& handle = provider();

  ASSERT_OK(handle.get_info(topic, entries, sizeof(entries), &actual, &avail));

  EXPECT_EQ(1u, actual);
  EXPECT_GT(avail, actual);
}

template <typename EntryType, uint32_t EntryCount = 1, typename HandleProvider>
void CheckNullActualSucceeds(zx_object_info_topic_t topic, const HandleProvider& provider) {
  EntryType entries[EntryCount];
  const auto& handle = provider();
  size_t tmp;

  ASSERT_OK(handle.get_info(topic, entries, sizeof(entries), nullptr, &tmp));
}

template <typename EntryType, uint32_t EntryCount = 1, typename HandleProvider>
void CheckNullAvailSucceeds(zx_object_info_topic_t topic, const HandleProvider& provider) {
  EntryType entries[EntryCount];
  const auto& handle = provider();
  size_t tmp;

  ASSERT_OK(handle.get_info(topic, entries, sizeof(entries), &tmp, nullptr));
}

template <typename EntryType, uint32_t EntryCount = 1, typename HandleProvider>
void CheckNullActualAndAvailSucceeds(zx_object_info_topic_t topic, const HandleProvider& provider) {
  EntryType entries[EntryCount];
  const auto& handle = provider();
  ASSERT_OK(handle.get_info(topic, entries, sizeof(entries), nullptr, nullptr));
}

template <typename EntryType, typename HandleProvider>
void CheckInvalidBufferPointerFails(zx_object_info_topic_t topic, const HandleProvider& provider) {
  const auto& handle = provider();
  size_t actual;
  size_t avail;

  EXPECT_EQ(handle.get_info(topic, (EntryType*)1, sizeof(EntryType), &actual, &avail),
            ZX_ERR_INVALID_ARGS);
}

template <typename EntryType, typename HandleProvider>
void CheckPartiallyUnmappedBufferIsError(zx_object_info_topic_t topic,
                                         const HandleProvider& provider, zx_status_t error_status) {
  // Create a two-page VMAR.
  zx::vmar vmar;
  uintptr_t vmar_addr;
  const auto& handle = provider();

  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0,
      2 * zx_system_get_page_size(), &vmar, &vmar_addr));

  // Create a one-page VMO.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0u, &vmo));

  // Map the first page of the VMAR.
  uintptr_t vmo_addr;
  ASSERT_OK(vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                     zx_system_get_page_size(), &vmo_addr));

  // Once mapped, we need to destroy it before closing the handle.
  auto cleanup = fit::defer([&vmar]() { vmar.destroy(); });
  ASSERT_EQ(vmar_addr, vmo_addr);

  // Point to a spot in the mapped page just before the unmapped region:
  // the first entry will hit mapped memory, the second entry will hit
  // unmapped memory.
  EntryType* entries = reinterpret_cast<EntryType*>(vmo_addr + zx_system_get_page_size()) - 1;

  size_t actual;
  size_t avail;
  EXPECT_STATUS(handle.get_info(topic, entries, sizeof(EntryType) * 4, &actual, &avail),
                error_status);
  vmar.destroy();
}

template <typename EntryType, uint32_t EntryCount = 1, typename HandleProvider>
void BadActualIsInvalidArgs(zx_object_info_topic_t topic, const HandleProvider& provider) {
  const auto& handle = provider();
  EntryType entries[EntryCount];
  size_t avail;
  EXPECT_EQ(handle.get_info(topic, entries, sizeof(entries),
                            // Bad actual pointer value.
                            reinterpret_cast<size_t*>(1), &avail),
            ZX_ERR_INVALID_ARGS);
}

template <typename EntryType, uint32_t EntryCount = 1, typename HandleProvider>
void BadAvailIsInvalidArgs(zx_object_info_topic_t topic, const HandleProvider& provider) {
  const auto& handle = provider();
  EntryType entries[EntryCount];
  size_t actual;
  EXPECT_EQ(handle.get_info(topic, entries, sizeof(entries), &actual,
                            // Bad actual pointer value.
                            reinterpret_cast<size_t*>(1)),
            ZX_ERR_INVALID_ARGS);
}
}  // namespace object_info_test

#endif  // ZIRCON_SYSTEM_UTEST_CORE_OBJECT_INFO_HELPER_H_
