// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file->

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <memory>
#include <utility>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vmo_file.h"

namespace {

const size_t VMO_SIZE = zx_system_get_page_size() * 3u;
const size_t PAGE_0 = 0u;
const size_t PAGE_1 = zx_system_get_page_size();
const size_t PAGE_2 = zx_system_get_page_size() * 2u;

zx_koid_t GetKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

zx_rights_t GetRights(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.rights : 0u;
}

void FillVmo(const zx::vmo& vmo, size_t offset, size_t length, uint8_t byte) {
  auto data = std::make_unique<uint8_t[]>(length);
  memset(data.get(), byte, length);

  zx_status_t status = vmo.write(data.get(), offset, length);
  ASSERT_EQ(ZX_OK, status);
}

void CheckVmo(const zx::vmo& vmo, size_t offset, size_t length, uint8_t expected_byte) {
  auto data = std::make_unique<uint8_t[]>(length);

  zx_status_t status = vmo.read(data.get(), offset, length);
  ASSERT_EQ(ZX_OK, status);

  for (size_t i = 0; i < length; i++) {
    ASSERT_EQ(expected_byte, data[i]);
  }
}

void CheckData(uint8_t* data, size_t offset, size_t length, uint8_t expected_byte) {
  for (size_t i = 0; i < length; i++) {
    ASSERT_EQ(expected_byte, data[i + offset]);
  }
}

void CreateVmoABC(zx::vmo* out_vmo) {
  zx_status_t status = zx::vmo::create(VMO_SIZE, 0u, out_vmo);
  ASSERT_EQ(ZX_OK, status);

  FillVmo(*out_vmo, PAGE_0, zx_system_get_page_size(), 'A');
  FillVmo(*out_vmo, PAGE_1, zx_system_get_page_size(), 'B');
  FillVmo(*out_vmo, PAGE_2, zx_system_get_page_size(), 'C');
}

TEST(VmoFile, Constructor) {
  zx::vmo abc;
  CreateVmoABC(&abc);

  // default parameters
  {
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), zx_system_get_page_size());
    EXPECT_EQ(zx_system_get_page_size(), file->length());
    EXPECT_FALSE(file->is_writable());
  }

  // everything explicit
  {
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), PAGE_2 + 1u, true,
                                                 fs::VmoFile::DefaultSharingMode::kCloneCow);
    EXPECT_EQ(PAGE_2 + 1u, file->length());
    EXPECT_TRUE(file->is_writable());
  }
}

TEST(VmoFile, Open) {
  zx::vmo abc;
  CreateVmoABC(&abc);
  auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(abc), 0u, /*writable=*/false);
  fbl::RefPtr<fs::Vnode> redirect;
  ASSERT_EQ(ZX_OK, file->Open(&redirect));
  ASSERT_EQ(redirect, nullptr);
}

TEST(VmoFile, Protocols) {
  zx::vmo abc;
  CreateVmoABC(&abc);
  auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(abc), 0u, /*writable=*/false);
  ASSERT_EQ(file->GetProtocols(), fuchsia_io::NodeProtocolKinds::kFile);
}

TEST(VmoFile, ValidateRights) {
  zx::vmo abc;
  CreateVmoABC(&abc);

  // read-only
  {
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

    auto read_only = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 0u, /*writable=*/false);
    ASSERT_TRUE(read_only->ValidateRights(fuchsia_io::Rights::kReadBytes));
    ASSERT_FALSE(read_only->ValidateRights(fuchsia_io::Rights::kWriteBytes));
    ASSERT_FALSE(read_only->ValidateRights(fuchsia_io::Rights::kExecute));
  }

  // writable
  {
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

    auto writable = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 0u, /*writable=*/true);
    ASSERT_TRUE(writable->ValidateRights(fuchsia_io::Rights::kReadBytes));
    ASSERT_TRUE(writable->ValidateRights(fuchsia_io::Rights::kWriteBytes));
    ASSERT_FALSE(writable->ValidateRights(fuchsia_io::Rights::kExecute));
  }
}

TEST(VmoFile, Read) {
  zx::vmo abc;
  CreateVmoABC(&abc);

  auto data = std::make_unique<uint8_t[]>(VMO_SIZE);
  memset(data.get(), 0, VMO_SIZE);

  {
    SCOPED_TRACE("empty-read-nonempty-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 0u);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Read(data.get(), 0u, 0u, &actual));
    EXPECT_EQ(0u, actual);
  }

  {
    SCOPED_TRACE("nonempty-read-empty-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 0u);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Read(data.get(), 1u, 0u, &actual));
    EXPECT_EQ(0u, actual);
  }

  {
    SCOPED_TRACE("empty-read-end-of-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 10u);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Read(data.get(), 0u, 10u, &actual));
    EXPECT_EQ(0u, actual);
  }

  {
    SCOPED_TRACE("nonempty-read-end-of-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 10u);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Read(data.get(), 1u, 10u, &actual));
    EXPECT_EQ(0u, actual);
  }

  {
    SCOPED_TRACE("empty-read-beyond-end-of-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 10u);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Read(data.get(), 0u, 11u, &actual));
    EXPECT_EQ(0u, actual);
  }

  {
    SCOPED_TRACE("nonempty-read-beyond-end-of-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 10u);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Read(data.get(), 1u, 11u, &actual));
    EXPECT_EQ(0u, actual);
  }

  {
    SCOPED_TRACE("short-read-nonempty-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 10u);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Read(data.get(), 11u, 1u, &actual));
    EXPECT_EQ(9u, actual);
    CheckData(data.get(), 0u, 9u, 'A');
    CheckData(data.get(), 9u, 1, 0);
  }

  {
    SCOPED_TRACE("full-read");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), VMO_SIZE);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Read(data.get(), VMO_SIZE, 0u, &actual));
    EXPECT_EQ(VMO_SIZE, actual);
    CheckData(data.get(), PAGE_0, zx_system_get_page_size(), 'A');
    CheckData(data.get(), PAGE_1, zx_system_get_page_size(), 'B');
    CheckData(data.get(), PAGE_2, zx_system_get_page_size(), 'C');
  }
}

TEST(VmoFile, Write) {
  zx::vmo abc;
  CreateVmoABC(&abc);

  auto data = std::make_unique<uint8_t[]>(VMO_SIZE);
  memset(data.get(), '!', VMO_SIZE);

  {
    SCOPED_TRACE("empty-write-nonempty-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), zx_system_get_page_size(), true);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Write(data.get(), 0u, 0u, &actual));
    EXPECT_EQ(0u, actual);
    CheckVmo(abc, PAGE_0, zx_system_get_page_size(), 'A');
    CheckVmo(abc, PAGE_1, zx_system_get_page_size(), 'B');
    CheckVmo(abc, PAGE_2, zx_system_get_page_size(), 'C');
  }

  {
    SCOPED_TRACE("nonempty-write-empty-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 0u, true);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_ERR_NO_SPACE, file->Write(data.get(), 1u, 0u, &actual));
  }

  {
    SCOPED_TRACE("empty-write-end-of-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 10u, true);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Write(data.get(), 0u, 10u, &actual));
    EXPECT_EQ(0u, actual);
    CheckVmo(abc, PAGE_0, zx_system_get_page_size(), 'A');
    CheckVmo(abc, PAGE_1, zx_system_get_page_size(), 'B');
    CheckVmo(abc, PAGE_2, zx_system_get_page_size(), 'C');
  }

  {
    SCOPED_TRACE("nonempty-write-end-of-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 10u, true);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_ERR_NO_SPACE, file->Write(data.get(), 1u, 10u, &actual));
  }

  {
    SCOPED_TRACE("empty-write-beyond-end-of-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 10u, true);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Write(data.get(), 0u, 11u, &actual));
    EXPECT_EQ(0u, actual);
    CheckVmo(abc, PAGE_0, zx_system_get_page_size(), 'A');
    CheckVmo(abc, PAGE_1, zx_system_get_page_size(), 'B');
    CheckVmo(abc, PAGE_2, zx_system_get_page_size(), 'C');
  }

  {
    SCOPED_TRACE("nonempty-write-beyond-end-of-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 10u, true);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_ERR_NO_SPACE, file->Write(data.get(), 1u, 11u, &actual));
  }

  {
    SCOPED_TRACE("short-write-nonempty-file");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 10u, true);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Write(data.get(), 11u, 1u, &actual));
    EXPECT_EQ(9u, actual);
    CheckVmo(abc, PAGE_0, 1, 'A');
    CheckVmo(abc, PAGE_0 + 1, 9u, '!');
    CheckVmo(abc, PAGE_0 + 10, zx_system_get_page_size() - 10, 'A');
    CheckVmo(abc, PAGE_1, zx_system_get_page_size(), 'B');
    CheckVmo(abc, PAGE_2, zx_system_get_page_size(), 'C');
  }

  {
    SCOPED_TRACE("full-write");
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), VMO_SIZE, true);
    size_t actual = UINT64_MAX;
    EXPECT_EQ(ZX_OK, file->Write(data.get(), VMO_SIZE, 0u, &actual));
    EXPECT_EQ(VMO_SIZE, actual);
    CheckVmo(abc, 0u, VMO_SIZE, '!');
  }
}

TEST(VmoFile, Getattr) {
  zx::vmo abc;
  CreateVmoABC(&abc);

  // read-only
  {
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file =
        fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), zx_system_get_page_size() * 3u + 117u);
    zx::result attr = file->GetAttributes();
    ASSERT_TRUE(attr.is_ok());
    EXPECT_EQ(zx_system_get_page_size() * 3u + 117u, attr->content_size);
    EXPECT_EQ(4u * zx_system_get_page_size(), attr->storage_size);
  }

  // writable
  {
    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup),
                                                 zx_system_get_page_size() * 3u + 117u, true);
    zx::result attr = file->GetAttributes();
    ASSERT_TRUE(attr.is_ok());
    EXPECT_EQ(zx_system_get_page_size() * 3u + 117u, attr->content_size);
    EXPECT_EQ(4u * zx_system_get_page_size(), attr->storage_size);
  }
}

TEST(VmoFile, DefaultSharingMode) {
  {
    SCOPED_TRACE("DefaultSharingMode::kDuplicate,read-only");
    zx::vmo abc;
    CreateVmoABC(&abc);

    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 23u, false,
                                                 fs::VmoFile::DefaultSharingMode::kDuplicate);
    zx::vmo vmo;
    EXPECT_EQ(ZX_OK, file->GetVmo(fuchsia_io::wire::VmoFlags::kRead, &vmo));

    EXPECT_NE(abc.get(), vmo.get());
    EXPECT_EQ(GetKoid(abc.get()), GetKoid(vmo.get()));
    EXPECT_EQ(ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_READ,
              GetRights(vmo.get()));
    uint64_t size;
    EXPECT_EQ(ZX_OK, vmo.get_prop_content_size(&size));
    EXPECT_EQ(VMO_SIZE, size);

    CheckVmo(vmo, PAGE_0, 23u, 'A');
  }

  {
    SCOPED_TRACE("DefaultSharingMode::kDuplicate,read-write");
    zx::vmo abc;
    CreateVmoABC(&abc);

    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 23u, true,
                                                 fs::VmoFile::DefaultSharingMode::kDuplicate);
    zx::vmo vmo;
    EXPECT_EQ(ZX_OK, file->GetVmo(fuchsia_io::wire::VmoFlags::kRead, &vmo));

    EXPECT_NE(abc.get(), vmo.get());
    EXPECT_EQ(GetKoid(abc.get()), GetKoid(vmo.get()));

    // As the VmoFile implementation does not currently track size changes, we ensure that the
    // handle provided in kDuplicate sharing mode is not writable.
    EXPECT_EQ(ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_READ,
              GetRights(vmo.get()));
    uint64_t size;
    EXPECT_EQ(ZX_OK, vmo.get_prop_content_size(&size));
    EXPECT_EQ(VMO_SIZE, size);

    CheckVmo(vmo, PAGE_0, 23u, 'A');
  }

  {
    SCOPED_TRACE("DefaultSharingMode::kDuplicate,write-only");
    zx::vmo abc;
    CreateVmoABC(&abc);

    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 23u, true,
                                                 fs::VmoFile::DefaultSharingMode::kDuplicate);
    zx::vmo vmo;
    EXPECT_EQ(ZX_OK, file->GetVmo({}, &vmo));

    EXPECT_NE(abc.get(), vmo.get());
    EXPECT_EQ(GetKoid(abc.get()), GetKoid(vmo.get()));
    // As the VmoFile implementation does not currently track size changes, we ensure that the
    // handle provided in kDuplicate sharing mode is not writable.
    EXPECT_EQ(ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY, GetRights(vmo.get()));
    uint64_t size;
    EXPECT_EQ(ZX_OK, vmo.get_prop_content_size(&size));
    EXPECT_EQ(VMO_SIZE, size);
  }

  {
    SCOPED_TRACE("DefaultSharingMode::kCloneCow,read-only");
    zx::vmo abc;
    CreateVmoABC(&abc);

    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 23u, false,
                                                 fs::VmoFile::DefaultSharingMode::kCloneCow);
    zx::vmo vmo;
    // There is non-trivial lazy initialization happening here - repeat it
    // to make sure it's nice and deterministic.
    for (int i = 0; i < 2; i++) {
      EXPECT_EQ(ZX_OK, file->GetVmo(fuchsia_io::wire::VmoFlags::kRead, &vmo));
    }

    EXPECT_NE(abc.get(), vmo.get());
    EXPECT_NE(GetKoid(abc.get()), GetKoid(vmo.get()));
    EXPECT_EQ(ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_READ,
              GetRights(vmo.get()));
    uint64_t size;
    EXPECT_EQ(ZX_OK, vmo.get_prop_content_size(&size));
    EXPECT_EQ(23u, size);

    CheckVmo(vmo, PAGE_0, 23u, 'A');
  }

  {
    SCOPED_TRACE("DefaultSharingMode::kCloneCow,read-write");
    zx::vmo abc;
    CreateVmoABC(&abc);

    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 23u, true,
                                                 fs::VmoFile::DefaultSharingMode::kCloneCow);
    zx::vmo vmo;
    EXPECT_EQ(
        ZX_OK,
        file->GetVmo(fuchsia_io::wire::VmoFlags::kRead | fuchsia_io::wire::VmoFlags::kWrite, &vmo));

    EXPECT_NE(abc.get(), vmo.get());
    EXPECT_NE(GetKoid(abc.get()), GetKoid(vmo.get()));
    EXPECT_EQ(ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_READ |
                  ZX_RIGHT_WRITE | ZX_RIGHT_SET_PROPERTY,
              GetRights(vmo.get()));
    uint64_t size;
    EXPECT_EQ(ZX_OK, vmo.get_prop_content_size(&size));
    EXPECT_EQ(23u, size);

    FillVmo(vmo, 0, 23u, '!');

    CheckVmo(abc, PAGE_0, zx_system_get_page_size(), 'A');
    CheckVmo(abc, PAGE_1, zx_system_get_page_size(), 'B');
    CheckVmo(abc, PAGE_2, zx_system_get_page_size(), 'C');
  }

  {
    SCOPED_TRACE("DefaultSharingMode::kCloneCow,write-only");
    zx::vmo abc;
    CreateVmoABC(&abc);

    zx::vmo dup;
    ASSERT_EQ(ZX_OK, abc.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));

    auto file = fbl::MakeRefCounted<fs::VmoFile>(std::move(dup), 23u, true,
                                                 fs::VmoFile::DefaultSharingMode::kCloneCow);
    zx::vmo vmo;
    EXPECT_EQ(ZX_OK, file->GetVmo(fuchsia_io::wire::VmoFlags::kWrite, &vmo));

    EXPECT_NE(abc.get(), vmo.get());
    EXPECT_NE(GetKoid(abc.get()), GetKoid(vmo.get()));
    EXPECT_EQ(ZX_RIGHTS_BASIC | ZX_RIGHT_MAP | ZX_RIGHT_GET_PROPERTY | ZX_RIGHT_WRITE |
                  ZX_RIGHT_SET_PROPERTY,
              GetRights(vmo.get()));
    uint64_t size;
    EXPECT_EQ(ZX_OK, vmo.get_prop_content_size(&size));
    EXPECT_EQ(23u, size);

    FillVmo(vmo, 0, 23u, '!');

    CheckVmo(abc, PAGE_0, zx_system_get_page_size(), 'A');
    CheckVmo(abc, PAGE_1, zx_system_get_page_size(), 'B');
    CheckVmo(abc, PAGE_2, zx_system_get_page_size(), 'C');
  }
}

}  // namespace
