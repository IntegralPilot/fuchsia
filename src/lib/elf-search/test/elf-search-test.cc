// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <elf-search.h>
#include <elf.h>
#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <inttypes.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/elfldltl/constants.h>
#include <lib/elfldltl/container.h>
#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/machine.h>
#include <lib/elfldltl/vmar-loader.h>
#include <lib/elfldltl/vmo.h>
#include <lib/elfldltl/zircon.h>
#include <lib/fit/defer.h>
#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/zx/job.h>
#include <lib/zx/port.h>
#include <lib/zx/process.h>
#include <lib/zx/time.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <stdio.h>
#include <stdlib.h>
#include <zircon/status.h>
#include <zircon/syscalls/port.h>

#include <algorithm>
#include <iterator>
#include <string>

#include <fbl/vector.h>
#include <test-utils/test-utils.h>
#include <zxtest/zxtest.h>

namespace {

template <typename Ehdr, typename Phdr>
void WriteHeaders(const cpp20::span<const Phdr>& phdrs, const zx::vmo& vmo,
                  std::optional<elfldltl::ElfClass> elf_class = elfldltl::ElfClass::kNative,
                  std::optional<elfldltl::ElfMachine> machine = elfldltl::ElfMachine::kNative) {
  const Ehdr ehdr = {
#if defined(__clang__)
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wc99-designator"
#endif
      .e_ident =
          {
              [EI_MAG0] = ELFMAG0,
              [EI_MAG1] = ELFMAG1,
              [EI_MAG2] = ELFMAG2,
              [EI_MAG3] = ELFMAG3,
              [EI_CLASS] = static_cast<uint8_t>(*elf_class),
              [EI_DATA] = ELFDATA2LSB,
              [EI_VERSION] = EV_CURRENT,
              [EI_OSABI] = ELFOSABI_NONE,
          },
#if defined(__clang__)
#pragma GCC diagnostic pop
#endif
      .e_type = ET_DYN,
      .e_machine = static_cast<uint16_t>(*machine),
      .e_version = EV_CURRENT,
      .e_entry = 0,
      .e_phoff = sizeof(Ehdr),
      .e_shoff = 0,
      .e_flags = 0,
      .e_ehsize = sizeof(Ehdr),
      .e_phentsize = sizeof(Phdr),
      .e_phnum = static_cast<uint16_t>(phdrs.size()),
      .e_shentsize = 0,
      .e_shnum = 0,
      .e_shstrndx = 0,
  };
  EXPECT_OK(vmo.write(&ehdr, 0, sizeof(ehdr)));
  EXPECT_OK(vmo.write(phdrs.data(), sizeof(ehdr), sizeof(Phdr) * phdrs.size()));
}

// TODO(jakehehrlich): Switch all uses of uint8_t to std::byte once libc++ lands.

template <typename Nhdr = Elf64_Nhdr>
void WriteBuildID(cpp20::span<const uint8_t> build_id, const zx::vmo& vmo, uint64_t note_offset) {
  uint8_t buf[64];
  const Nhdr nhdr = {
      .n_namesz = sizeof(ELF_NOTE_GNU),
      .n_descsz = static_cast<Elf64_Word>(build_id.size()),
      .n_type = NT_GNU_BUILD_ID,
  };
  ASSERT_GT(sizeof(buf), sizeof(nhdr) + sizeof(ELF_NOTE_GNU) + build_id.size());
  uint64_t note_size = 0;
  memcpy(buf + note_size, &nhdr, sizeof(nhdr));
  note_size += sizeof(nhdr);
  memcpy(buf + note_size, ELF_NOTE_GNU, sizeof(ELF_NOTE_GNU));
  note_size += sizeof(ELF_NOTE_GNU);
  memcpy(buf + note_size, build_id.data(), build_id.size());
  note_size += build_id.size();
  EXPECT_OK(vmo.write(buf, note_offset, note_size));
}

template <typename Phdr>
struct Module {
  std::string_view name;
  cpp20::span<const Phdr> phdrs;
  cpp20::span<const uint8_t> build_id;
  zx::vmo vmo;
};

zx::resource GetVmexResource() {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (system_resource->is_valid()) {
    zx::result<zx::resource> vmex_result =
        maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_VMEX_BASE);
    if (vmex_result.is_ok()) {
      return std::move(vmex_result.value());
    }
  }

  zx::result client = component::Connect<fuchsia_kernel::VmexResource>();
  if (client.is_ok()) {
    fidl::WireResult result = fidl::WireCall(client.value())->Get();
    if (result.ok()) {
      return std::move(result.value().resource);
    }
  }

  return zx::resource();
}

void MakeELF(Module<Elf64_Phdr>* mod) {
  size_t size = 0;
  for (const auto& phdr : mod->phdrs) {
    size = std::max(size, phdr.p_offset + phdr.p_filesz);
  }
  ASSERT_OK(zx::vmo::create(size, 0, &mod->vmo));
  EXPECT_OK(mod->vmo.set_property(ZX_PROP_NAME, mod->name.data(), mod->name.size()));
  zx::resource vmex = GetVmexResource();
  ASSERT_TRUE(vmex.is_valid());
  EXPECT_OK(mod->vmo.replace_as_executable(vmex, &mod->vmo));

  ASSERT_NO_FATAL_FAILURE((WriteHeaders<Elf64_Ehdr, Elf64_Phdr>(mod->phdrs, mod->vmo)));

  for (const auto& phdr : mod->phdrs) {
    if (phdr.p_type == PT_NOTE) {
      ASSERT_NO_FATAL_FAILURE(WriteBuildID(mod->build_id, mod->vmo, phdr.p_offset));
    }
  }
}

void MakeELF(Module<Elf32_Phdr>* mod) {
  uint32_t size = 0;
  for (const auto& phdr : mod->phdrs) {
    size = std::max(size, phdr.p_offset + phdr.p_filesz);
  }
  ASSERT_OK(zx::vmo::create(size, 0, &mod->vmo));
  EXPECT_OK(mod->vmo.set_property(ZX_PROP_NAME, mod->name.data(), mod->name.size()));
  zx::resource vmex = GetVmexResource();
  ASSERT_TRUE(vmex.is_valid());
  EXPECT_OK(mod->vmo.replace_as_executable(vmex, &mod->vmo));

  ASSERT_NO_FATAL_FAILURE((WriteHeaders<Elf32_Ehdr, Elf32_Phdr>(
      mod->phdrs, mod->vmo, elfldltl::ElfClass::k32, elfldltl::ElfMachine::kArm)));

  for (const auto& phdr : mod->phdrs) {
    if (phdr.p_type == PT_NOTE) {
      ASSERT_NO_FATAL_FAILURE(WriteBuildID<Elf32_Nhdr>(mod->build_id, mod->vmo, phdr.p_offset));
    }
  }
}

constexpr Elf64_Phdr MakePhdr(uint32_t type, uint64_t size, uint64_t offset, uint64_t addr,
                              uint32_t flags, uint32_t align) {
  return Elf64_Phdr{
      .p_type = type,
      .p_flags = flags,
      .p_offset = offset,
      .p_vaddr = addr,
      .p_paddr = addr,
      .p_filesz = size,
      .p_memsz = size,
      .p_align = align,
  };
}

constexpr Elf32_Phdr MakePhdr32(uint32_t type, uint32_t size, uint32_t offset, uint32_t addr,
                                uint32_t flags, uint32_t align) {
  return Elf32_Phdr{
      .p_type = type,
      .p_offset = offset,
      .p_vaddr = addr,
      .p_paddr = addr,
      .p_filesz = size,
      .p_memsz = size,
      .p_flags = flags,
      .p_align = align,
  };
}

void GetKoid(const zx::vmo& obj, zx_koid_t* out) {
  zx_info_handle_basic_t info;
  ASSERT_OK(obj.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  *out = info.koid;
}

zx_status_t LoadElf(const zx::vmar& vmar, const zx::vmo& vmo, uintptr_t& base, uintptr_t& entry) {
  // Ignore any error details, but capture the zx_status_t of a SystemError.
  // Tell the toolkit code to keep going after a SystemError if possible.  No
  // other kinds of errors should be possible since those would indicate an
  // invalid ELF image.
  zx_status_t status = ZX_OK;
  auto report = [&status](auto&&... args) -> bool {
    auto check = [&status](auto arg) -> bool {
      if constexpr (std::is_same_v<decltype(arg), elfldltl::ZirconError>) {
        status = arg.status;
        return true;
      }
      return false;
    };
    return (check(args) || ...);
  };

  auto diag = elfldltl::Diagnostics(report, elfldltl::DiagnosticsPanicFlags());

  elfldltl::UnownedVmoFile file(vmo.borrow(), diag);
  elfldltl::RemoteVmarLoader loader{vmar};

  auto phdr_allocator = [&diag]<typename T>(size_t count) {
    return elfldltl::ContainerArrayFromFile<elfldltl::StdContainer<std::vector>::Container<T>>(
        diag, "impossible")(count);
  };

  auto load = [&]<class Ehdr, class Phdr>(const Ehdr& ehdr, std::span<const Phdr> phdrs) -> bool {
    using Elf = typename Ehdr::ElfLayout;
    using LoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container>;

    // Now that we know the type of module, we can load the segments.
    LoadInfo load_info;
    if (elfldltl::DecodePhdrs(
            diag, phdrs,
            load_info.GetPhdrObserver(static_cast<LoadInfo::size_type>(loader.page_size()))) &&
        loader.Load(diag, load_info, vmo.borrow())) {
      ZX_ASSERT(load_info.vaddr_start() == 0);
      base = loader.load_bias();
      entry = ehdr.entry + loader.load_bias();
      std::ignore = std::move(loader).Commit(typename LoadInfo::Region{});
      return true;
    }

    return false;
  };

  // Use |WithLoadHeadersFromFile| so we can deal with both 32 and 64 bit ELF modules. Ensure to
  // pass std::nullopt for the machine type since the 32 bit module we will construct will not be
  // the same as the native machine type.
  EXPECT_TRUE(elfldltl::WithLoadHeadersFromFile(diag, file, phdr_allocator, std::move(load),
                                                std::nullopt, std::nullopt));

  return status;
}

// TODO(jakehehrlich): Not all error cases are tested. Appropriate tests can be
// sussed out by looking at coverage results.
TEST(ElfSearchTest, ForEachModule) {
  // Define some dummy modules.
  constexpr Elf64_Phdr mod0_phdrs[] = {
      MakePhdr(PT_LOAD, 0x2000, 0, 0, PF_R, 0x1000), MakePhdr(PT_NOTE, 20, 0x1000, 0x1000, PF_R, 4),
      MakePhdr(PT_LOAD, 0x1000, 0x2000, 0x2000, PF_R | PF_W, 0x1000),
      MakePhdr(PT_LOAD, 0x1000, 0x3000, 0x3000, PF_R | PF_X, 0x1000)};
  constexpr uint8_t mod0_build_id[] = {0xde, 0xad, 0xbe, 0xef};
  constexpr Elf64_Phdr mod1_phdrs[] = {
      MakePhdr(PT_LOAD, 0x2000, 0x0000, 0x0000, PF_R, 0x1000),
      MakePhdr(PT_NOTE, 20, 0x1000, 0x1000, PF_R, 4),
      MakePhdr(PT_LOAD, 0x1000, 0x2000, 0x2000, PF_R | PF_X, 0x1000)};
  constexpr uint8_t mod1_build_id[] = {0xff, 0xff, 0xff, 0xff};
  constexpr Elf64_Phdr mod2_phdrs[] = {MakePhdr(PT_LOAD, 0x2000, 0x0000, 0x0000, PF_R, 0x1000),
                                       MakePhdr(PT_NOTE, 20, 0x1000, 0x1000, PF_R, 4)};
  constexpr uint8_t mod2_build_id[] = {0x00, 0x00, 0x00, 0x00};
  constexpr Elf64_Phdr mod3_phdrs[] = {MakePhdr(PT_LOAD, 0x2000, 0, 0, PF_R, 0x1000),
                                       MakePhdr(PT_NOTE, 20, 0x1000, 0x1000, PF_R, 4),
                                       MakePhdr(PT_DYNAMIC, 0x800, 0x1800, 0x1800, PF_R, 4)};
  constexpr uint8_t mod3_build_id[] = {0x12, 0x34, 0x56, 0x78};
  constexpr Elf64_Dyn mod3_dyns[] = {{DT_STRTAB, {0x1900}}, {DT_SONAME, {1}}, {DT_NULL, {}}};
  constexpr const char* mod3_soname = "soname";
  // mod4 has `-z noseparate-code`, i.e., multiple PT_LOAD segments live on the same page,
  // and has r/w dynamic table so the values in it are absolute addresses rather than offsets.
  constexpr Elf64_Phdr mod4_phdrs[] = {MakePhdr(PT_LOAD, 0x950, 0, 0, PF_R, 0x1000),
                                       MakePhdr(PT_LOAD, 0x2b0, 0x950, 0x1950, PF_R | PF_X, 0x1000),
                                       MakePhdr(PT_LOAD, 0x258, 0xc00, 0x2c00, PF_R | PF_W, 0x1000),
                                       MakePhdr(PT_DYNAMIC, 0x100, 0xc00, 0x2c00, PF_R | PF_W, 8),
                                       MakePhdr(PT_NOTE, 20, 0x270, 0x270, PF_R, 4)};
  constexpr uint8_t mod4_build_id[] = {0x44, 0x33, 0x22, 0x11};
  Elf64_Dyn mod4_dyns[] = {{DT_STRTAB, {0x900}}, {DT_SONAME, {1}}, {DT_NULL, {}}};
  constexpr const char* mod4_soname = "another_soname";
  // Define a 32 bit module.
  constexpr Elf32_Phdr mod5_phdrs[] = {
      MakePhdr32(PT_LOAD, 0x2000, 0, 0, PF_R, 0x1000),
      MakePhdr32(PT_NOTE, 20, 0x1000, 0x1000, PF_R, 4),
      MakePhdr32(PT_LOAD, 0x1000, 0x2000, 0x2000, PF_R | PF_W, 0x1000),
      MakePhdr32(PT_LOAD, 0x1000, 0x3000, 0x3000, PF_R | PF_X, 0x1000)};
  constexpr uint8_t mod5_build_id[] = {0xba, 0xdb, 0x10, 0x0d};

  using ModuleVariant = std::variant<Module<Elf32_Phdr>, Module<Elf64_Phdr>>;
  ModuleVariant mods[] = {
      Module<Elf64_Phdr>{.name = "mod0", .phdrs = mod0_phdrs, .build_id = mod0_build_id, .vmo = {}},
      Module<Elf64_Phdr>{.name = "mod1", .phdrs = mod1_phdrs, .build_id = mod1_build_id, .vmo = {}},
      Module<Elf64_Phdr>{.name = "mod2", .phdrs = mod2_phdrs, .build_id = mod2_build_id, .vmo = {}},
      Module<Elf64_Phdr>{.name = "mod3", .phdrs = mod3_phdrs, .build_id = mod3_build_id, .vmo = {}},
      Module<Elf64_Phdr>{.name = "mod4", .phdrs = mod4_phdrs, .build_id = mod4_build_id, .vmo = {}},
      Module<Elf32_Phdr>{.name = "mod5", .phdrs = mod5_phdrs, .build_id = mod5_build_id, .vmo = {}},
  };

  // Create the test process using the Launcher service, which has the proper clearance to spawn
  // new processes. This has the side effect of loading in the VDSO and dynamic linker, which are
  // explicitly ignored below.
  const char* file = "bin/elf-search-test-helper";
  const char* root_dir = getenv("TEST_ROOT_DIR");
  // When running as a component, TEST_ROOT_DIR is not set and should be "/pkg".
  if (!root_dir) {
    root_dir = "/pkg";
  }
  std::string helper = std::string(root_dir) + "/" + file;
  const char* argv[] = {helper.c_str()};
  springboard_t* sb =
      tu_launch_init(ZX_HANDLE_INVALID, "mod-test", 1, argv, 0, nullptr, 0, nullptr, nullptr);
  auto delete_sb = fit::defer([=] { tu_launch_abort(sb); });
  zx::vmar vmar{springboard_get_root_vmar_handle(sb)};

  uintptr_t base, entry;
  for (auto& module_variant : mods) {
    std::visit(
        [&](auto&& mod) {
          ASSERT_NO_FATAL_FAILURE(MakeELF(&mod));
          if (mod.name == "mod3") {
            // Handle mod3's string table.
            EXPECT_OK(mod.vmo.write(&mod3_dyns, 0x1800, sizeof(mod3_dyns)));
            EXPECT_OK(mod.vmo.write(mod3_soname, 0x1901, strlen(mod3_soname) + 1));
          }
          if (mod.name == "mod4") {
            // Setup mod4's dynamic table, otherwise LoadElf will fail.
            EXPECT_OK(mod.vmo.write(&mod4_dyns, 0xc00, sizeof(mod4_dyns)));
          }

          ASSERT_OK(LoadElf(vmar, mod.vmo, base, entry), "Unable to load extra ELF");

          if (mod.name == "mod4") {
            // Relocate the dynamic table and populate the soname.
            mod4_dyns[0].d_un.d_val += base;
            EXPECT_OK(mod.vmo.write(&mod4_dyns, 0xc00, sizeof(mod4_dyns)));
            EXPECT_OK(mod.vmo.write(mod4_soname, 0x901, strlen(mod4_soname) + 1));
          }
        },
        module_variant);
  }

  zx::process process;
  EXPECT_NE(ZX_HANDLE_INVALID,
            *process.reset_and_get_address() = springboard_get_process_handle(sb));
  auto cleanup = fit::defer([&]() { process.kill(); });

  // These modules appear in the list as they are the mimimum possible set of mappings that a
  // process can be spawned with using fuchsia.process.Launcher, which tu_launch_init relies on.
  const char* ignored_mods[] = {
      // The dynamic linker, a.k.a. ld.so.1 in packages.
      "libc.so",
      // The VDSO.
      "libzircon.so",
  };

  // Now loop though everything, checking module info along the way.
  uint32_t matchCount = 0, moduleCount = 0;
  zx_status_t status = elf_search::ForEachModule(process, [&](const elf_search::ModuleInfo& info) {
    for (const auto& mod : ignored_mods) {
      if (info.name == mod) {
        return;
      }
    }
    ++moduleCount;
    for (const auto& module_variant : mods) {
      std::visit(
          [&](auto&& mod) {
            if (mod.build_id.size() == info.build_id.size() &&
                std::equal(mod.build_id.begin(), mod.build_id.end(), info.build_id.begin())) {
              ++matchCount;
              zx_koid_t vmo_koid = 0;
              ASSERT_NO_FATAL_FAILURE(GetKoid(mod.vmo, &vmo_koid));
              EXPECT_EQ(mod.phdrs.size(), info.phdrs.size(), "expected same number of phdrs");

              char name[ZX_MAX_NAME_LEN];
              if (mod.name == "mod3") {
                snprintf(name, sizeof(name), "%s", mod3_soname);
              } else if (mod.name == "mod4") {
                snprintf(name, sizeof(name), "%s", mod4_soname);
              } else {
                snprintf(name, sizeof(name), "<VMO#%" PRIu64 "=%.*s>", vmo_koid,
                         static_cast<int>(mod.name.size()), mod.name.data());
              }
              EXPECT_STREQ(info.name, name);
            }
          },
          module_variant);
    }

    EXPECT_EQ(moduleCount, matchCount, "Build for module was not found.");
  });
  EXPECT_OK(status);
  EXPECT_EQ(moduleCount, std::size(mods), "Unexpected number of modules found.");
}

}  // namespace
