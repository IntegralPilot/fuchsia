// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/container.h>
#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/vmar-loader.h>
#include <lib/elfldltl/vmo.h>
#include <lib/elfldltl/zircon.h>
#include <lib/zx/process.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <string.h>
#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>

#include <vector>

#include <fbl/array.h>
#include <zxtest/zxtest.h>

namespace {

const zx::vmo vdso_vmo{zx_take_startup_handle(PA_HND(PA_VMO_VDSO, 0))};

class ScratchPad {
 public:
  ScratchPad() = delete;

  explicit ScratchPad(const char* name) {
    EXPECT_EQ(
        zx_process_create(zx_job_default(), name, static_cast<uint32_t>(strlen(name)), 0,
                          process_.reset_and_get_address(), root_vmar_.reset_and_get_address()),
        ZX_OK, "zx_process_create");
  }

  const zx::vmar& root_vmar() const { return root_vmar_; }
  uintptr_t vdso_base() const { return vdso_base_; }
  uintptr_t vdso_code_offset() const { return vdso_code_offset_; }
  uintptr_t vdso_code_size() const { return vdso_code_size_; }
  uintptr_t vdso_code_address() const { return vdso_base_ + vdso_code_offset_; }
  uintptr_t vdso_total_size() const { return vdso_code_offset_ + vdso_code_size_; }

  zx_status_t load_vdso(zx::vmar* segments_vmar = nullptr, bool really_load = true) {
    using LoadInfo =
        elfldltl::LoadInfo<elfldltl::Elf<>, elfldltl::StdContainer<std::vector>::Container>;

    zx_status_t status = ZX_OK;
    auto report = [&status](auto&&... args) -> bool {
      auto check = [&status](auto arg) {
        if constexpr (std::is_same_v<decltype(arg), elfldltl::ZirconError>) {
          status = arg.status;
        }
      };
      (check(args), ...);
      return false;
    };
    auto diag = elfldltl::Diagnostics(report, elfldltl::DiagnosticsPanicFlags());
    elfldltl::UnownedVmoFile file(vdso_vmo.borrow(), diag);
    LoadInfo load_info;
    elfldltl::RemoteVmarLoader loader{root_vmar()};
    auto headers = elfldltl::LoadHeadersFromFile<elfldltl::Elf<>>(
        diag, file,
        elfldltl::ContainerArrayFromFile<
            elfldltl::StdContainer<std::vector>::Container<elfldltl::Elf<>::Phdr>>(diag,
                                                                                   "impossible"));
    ZX_ASSERT(headers);
    auto& [ehdr, phdrs_result] = *headers;
    std::span<const elfldltl::Elf<>::Phdr> phdrs = phdrs_result;
    ZX_ASSERT(elfldltl::DecodePhdrs(diag, phdrs, load_info.GetPhdrObserver(loader.page_size())));
    load_info.VisitSegments([this](const auto& segment) {
      if (segment.executable()) {
        vdso_code_offset_ = segment.vaddr();
        vdso_code_size_ = segment.memsz();
        return false;
      }
      return true;
    });
    if (really_load) {
      if (loader.Load(diag, load_info, vdso_vmo.borrow())) {
        ZX_ASSERT(status == ZX_OK);
        vdso_base_ = load_info.vaddr_start() + loader.load_bias();
        zx::vmar vmar = std::move(loader).Commit(LoadInfo::Region{}).TakeVmar();
        if (segments_vmar) {
          *segments_vmar = std::move(vmar);
        }
      } else {
        ZX_ASSERT(status != ZX_OK);
      }
    }
    return status;
  }

  zx_status_t compute_vdso_sizes() { return load_vdso(nullptr, false); }

 private:
  zx::process process_;
  zx::vmar root_vmar_;
  uintptr_t vdso_base_ = 0;
  uintptr_t vdso_code_offset_ = 0;
  uintptr_t vdso_code_size_ = 0;
};

TEST(VdsoTests, vdso_map_twice_test) {
  ScratchPad scratch(__func__);

  // Load the vDSO once.  That's on me.
  EXPECT_EQ(scratch.load_vdso(), ZX_OK, "load vDSO into empty process");

  // Load the vDSO twice.  Can't get loaded again.
  EXPECT_EQ(scratch.load_vdso(), ZX_ERR_ACCESS_DENIED, "load vDSO second time");
}

TEST(VdsoTests, vdso_map_change_test) {
  ScratchPad scratch(__func__);

  // Load the vDSO and hold onto the sub-VMAR.
  zx::vmar vdso_vmar;
  EXPECT_EQ(scratch.load_vdso(&vdso_vmar), ZX_OK, "load vDSO");

  // Changing protections on the code pages is forbidden.
  EXPECT_EQ(
      vdso_vmar.protect(ZX_VM_PERM_READ, scratch.vdso_code_address(), scratch.vdso_code_size()),
      ZX_ERR_ACCESS_DENIED, "zx_vmar_protect on vDSO code");

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(scratch.vdso_total_size(), 0, &vmo), ZX_OK, "zx_vmo_create");

  // Implicit unmapping by overwriting the mapping is forbidden.
  uintptr_t addr = 0;
  EXPECT_EQ(vdso_vmar.map(ZX_VM_PERM_READ | ZX_VM_SPECIFIC_OVERWRITE, 0, vmo, 0,
                          scratch.vdso_total_size(), &addr),
            ZX_ERR_ACCESS_DENIED, "zx_vmar_map to overmap vDSO");
  EXPECT_EQ(addr, 0, "zx_vmar_map to overmap vDSO");

  // Also forbidden if done from a parent VMAR.
  zx_info_vmar_t root_vmar_info;
  ASSERT_EQ(scratch.root_vmar().get_info(ZX_INFO_VMAR, &root_vmar_info, sizeof(root_vmar_info),
                                         nullptr, nullptr),
            ZX_OK, "zx_object_get_info on root VMAR");
  EXPECT_EQ(scratch.root_vmar().map(ZX_VM_PERM_READ | ZX_VM_SPECIFIC_OVERWRITE,
                                    scratch.vdso_base() - root_vmar_info.base, vmo, 0,
                                    scratch.vdso_total_size(), &addr),
            ZX_ERR_ACCESS_DENIED, "zx_vmar_map to overmap vDSO from root");
  EXPECT_EQ(addr, 0, "zx_vmar_map to overmap vDSO from root");

  // Explicit unmapping covering the vDSO code region is forbidden.
  EXPECT_EQ(scratch.root_vmar().unmap(scratch.vdso_base(), scratch.vdso_total_size()),
            ZX_ERR_ACCESS_DENIED, "zx_vmar_unmap to unmap vDSO");

  // Implicit unmapping by destroying a containing VMAR is forbidden.
  EXPECT_EQ(vdso_vmar.destroy(), ZX_ERR_ACCESS_DENIED, "zx_vmar_destroy to unmap vDSO");
}

TEST(VdsoTests, vdso_map_code_wrong_test) {
  ScratchPad scratch(__func__);

  ASSERT_EQ(scratch.compute_vdso_sizes(), ZX_OK, "cannot read vDSO program headers");

  // Try to map the first page, which is not the code, as executable.
  const size_t page_size = zx_system_get_page_size();
  uintptr_t addr;
  EXPECT_EQ(scratch.root_vmar().map(ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE, 0, vdso_vmo, 0, page_size,
                                    &addr),
            ZX_ERR_ACCESS_DENIED, "executable mapping of wrong part of vDSO");

  // Try to map only part of the code, not the whole code segment.
  ASSERT_GE(scratch.vdso_code_size(), page_size, "vDSO code < page??");
  if (scratch.vdso_code_size() > page_size) {
    ASSERT_EQ(scratch.vdso_code_size() % page_size, 0);
    EXPECT_EQ(scratch.root_vmar().map(ZX_VM_PERM_READ | ZX_VM_PERM_EXECUTE, 0, vdso_vmo,
                                      scratch.vdso_code_offset(), page_size, &addr),
              ZX_ERR_ACCESS_DENIED, "executable mapping of subset of vDSO code");
  }
}

}  // namespace
