// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/loaded_elf_module.h"

#include <elf.h>
#include <lib/fit/result.h>

#include <cstring>

#include <gtest/gtest.h>

#include "src/lib/unwinder/memory.h"

namespace unwinder {

namespace {

template <size_t kNumPhdrs, size_t kNumShdrs = 0>
struct FakeElf64Module {
  Elf64_Ehdr ehdr;
  Elf64_Phdr phdr[kNumPhdrs];
  Elf64_Shdr shdr[kNumShdrs];

  // This is just a guess based on the number of sections that we are given. If there isn't enough
  // room, |PopulateShStrTab| will fail.
  static constexpr size_t kStrtabSize = kNumShdrs * 16;
  char strtab[kStrtabSize];
};

template <size_t kNumPhdrs, size_t kNumShdrs = 0>
struct FakeElf32Module {
  Elf32_Ehdr ehdr;
  Elf32_Phdr phdr[kNumPhdrs];
  Elf32_Shdr shdr[kNumShdrs];

  // This is just a guess based on the number of sections that we are given. If there isn't enough
  // room, |PopulateShStrTab| will fail.
  static constexpr size_t kStrtabSize = kNumShdrs * 16;
  char strtab[kStrtabSize];
};

// Below are helpers to prevent incorrect usage of PopulateShStrTab (and any future helpers we
// have) since directly writing ELF structs in memory is a bit sketchy, but anything more
// heavyweight than this adds significant complexity to these tests which are otherwise fairly
// simple.
template <typename T, template <std::size_t, std::size_t> class Template>
struct is_specialization : std::false_type {};

template <template <std::size_t, std::size_t> class Template, std::size_t N, std::size_t M>
struct is_specialization<Template<N, M>, Template> : std::true_type {};

template <typename T, template <std::size_t, std::size_t> class Template>
inline constexpr bool is_specialization_v = is_specialization<T, Template>::value;

template <typename T>
concept IsFakeElfModule =
    is_specialization_v<T, FakeElf64Module> || is_specialization_v<T, FakeElf32Module>;

// Fills in the section header string table for the given module. Returns an error if the given
// section names do not fit in the allocated strtab of |elf_module|.
fit::result<fit::failed> PopulateShStrTab(IsFakeElfModule auto& elf_module,
                                          std::span<const std::string> section_names) {
  // Populate section header string table.
  uint32_t table_offset = 0;
  size_t section_index = 0;
  const size_t strtab_len = sizeof(elf_module.strtab);
  for (const auto& name : section_names) {
    auto& shdr = elf_module.shdr[section_index];

    shdr.sh_name = table_offset;
    table_offset += name.size();

    if (table_offset >= strtab_len) {
      return fit::error(fit::failed());
    }

    strncpy(elf_module.strtab + shdr.sh_name, name.c_str(), name.size());

    section_index++;
  }

  return fit::ok();
}

TEST(LoadedElfModule, Load64Bit) {
  constexpr size_t kNumPhdrs = 3;

  using FakeModule = FakeElf64Module<kNumPhdrs>;
  FakeModule fake;
  memset(&fake, 0, sizeof(fake));

  // Set up 64-bit ELF header
  memcpy(fake.ehdr.e_ident, ELFMAG, SELFMAG);
  fake.ehdr.e_ident[EI_CLASS] = ELFCLASS64;
  fake.ehdr.e_ident[EI_DATA] = ELFDATA2LSB;
  fake.ehdr.e_ident[EI_VERSION] = EV_CURRENT;
  fake.ehdr.e_type = ET_DYN;
  fake.ehdr.e_machine = EM_X86_64;
  fake.ehdr.e_version = EV_CURRENT;
  fake.ehdr.e_phoff = offsetof(FakeModule, phdr);
  fake.ehdr.e_phnum = kNumPhdrs;
  fake.ehdr.e_phentsize = sizeof(Elf64_Phdr);

  // Set up a PT_LOAD segment
  fake.phdr[0].p_type = PT_LOAD;
  fake.phdr[0].p_vaddr = 0x1000;
  fake.phdr[0].p_memsz = 0x2000;
  fake.phdr[0].p_flags = PF_R | PF_X;

  fake.phdr[1].p_type = PT_LOAD;
  fake.phdr[1].p_vaddr = 0x3000;
  fake.phdr[1].p_memsz = 0x2000;
  fake.phdr[1].p_flags = PF_R | PF_X;

  fake.phdr[2].p_type = PT_LOAD;
  fake.phdr[2].p_vaddr = 0x5000;
  fake.phdr[2].p_memsz = 0x2000;
  fake.phdr[2].p_flags = PF_R | PF_X;

  LocalMemory mem;
  uint64_t load_addr = reinterpret_cast<uint64_t>(&fake);
  Module module(load_addr, &mem, Module::AddressMode::kProcess);

  LoadedElfModule loaded(module);
  auto result = loaded.Load();
  ASSERT_TRUE(result.is_ok()) << result.error_value().msg();

  EXPECT_EQ(loaded.load_address(), load_addr);
  EXPECT_EQ(loaded.phdrs().size(), 3u);
  EXPECT_EQ(loaded.size(), Module::AddressSize::k64Bit);

  // IsValidPC should check (load_addr + p_vaddr) to (load_addr + p_vaddr + p_memsz)
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x1000));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x2fff));
  EXPECT_FALSE(loaded.IsValidPC(load_addr + 0x0fff));

  // Valid inside of the second phdr.
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x3000));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x3500));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x4000));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x4fff));

  // Valid inside of the third phdr.
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x5000));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x5500));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x6000));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x6fff));

  // Outside of the range of the last.
  EXPECT_FALSE(loaded.IsValidPC(load_addr + 0x7000));
}

TEST(LoadedElfModule, Load64BitOnePhdr) {
  constexpr size_t kNumPhdrs = 1;
  using FakeModule = FakeElf64Module<kNumPhdrs>;

  FakeModule fake;
  memset(&fake, 0, sizeof(fake));

  // Set up 64-bit ELF header
  memcpy(fake.ehdr.e_ident, ELFMAG, SELFMAG);
  fake.ehdr.e_ident[EI_CLASS] = ELFCLASS64;
  fake.ehdr.e_ident[EI_DATA] = ELFDATA2LSB;
  fake.ehdr.e_ident[EI_VERSION] = EV_CURRENT;
  fake.ehdr.e_type = ET_DYN;
  fake.ehdr.e_machine = EM_X86_64;
  fake.ehdr.e_version = EV_CURRENT;
  fake.ehdr.e_phoff = offsetof(FakeModule, phdr);
  fake.ehdr.e_phnum = kNumPhdrs;
  fake.ehdr.e_phentsize = sizeof(Elf64_Phdr);

  // Set up a PT_LOAD segment
  fake.phdr[0].p_type = PT_LOAD;
  fake.phdr[0].p_vaddr = 0x1000;
  fake.phdr[0].p_memsz = 0x2000;
  fake.phdr[0].p_flags = PF_R | PF_X;

  LocalMemory mem;
  uint64_t load_addr = reinterpret_cast<uint64_t>(&fake);
  Module module(load_addr, &mem, Module::AddressMode::kProcess);

  LoadedElfModule loaded(module);
  auto result = loaded.Load();
  ASSERT_TRUE(result.is_ok()) << result.error_value().msg();

  EXPECT_EQ(loaded.load_address(), load_addr);
  EXPECT_EQ(loaded.size(), Module::AddressSize::k64Bit);

  // IsValidPC should check (load_addr + p_vaddr) to (load_addr + p_vaddr + p_memsz)
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x1000));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x2fff));
  EXPECT_FALSE(loaded.IsValidPC(load_addr + 0x0fff));
  EXPECT_FALSE(loaded.IsValidPC(load_addr + 0x3000));
}

TEST(LoadedElfModule, Load32Bit) {
  constexpr size_t kNumPhdrs = 3;
  using FakeModule = FakeElf32Module<kNumPhdrs>;

  FakeModule fake;
  memset(&fake, 0, sizeof(fake));

  // Set up 32-bit ELF header
  memcpy(fake.ehdr.e_ident, ELFMAG, SELFMAG);
  fake.ehdr.e_ident[EI_CLASS] = ELFCLASS32;
  fake.ehdr.e_ident[EI_DATA] = ELFDATA2LSB;
  fake.ehdr.e_ident[EI_VERSION] = EV_CURRENT;
  fake.ehdr.e_type = ET_DYN;
  fake.ehdr.e_machine = EM_ARM;
  fake.ehdr.e_version = EV_CURRENT;
  fake.ehdr.e_phoff = offsetof(FakeModule, phdr);
  fake.ehdr.e_phnum = kNumPhdrs;
  fake.ehdr.e_phentsize = sizeof(Elf32_Phdr);

  // Set up a PT_LOAD segment
  fake.phdr[0].p_type = PT_LOAD;
  fake.phdr[0].p_vaddr = 0x1000;
  fake.phdr[0].p_memsz = 0x2000;

  fake.phdr[1].p_type = PT_LOAD;
  fake.phdr[1].p_vaddr = 0x3000;
  fake.phdr[1].p_memsz = 0x2000;

  fake.phdr[2].p_type = PT_LOAD;
  fake.phdr[2].p_vaddr = 0x5000;
  fake.phdr[2].p_memsz = 0x2000;

  LocalMemory mem;
  uint64_t load_addr = reinterpret_cast<uint64_t>(&fake);
  Module module(load_addr, &mem, Module::AddressMode::kProcess);

  LoadedElfModule loaded(module);
  auto result = loaded.Load();
  ASSERT_TRUE(result.is_ok()) << result.error_value().msg();

  EXPECT_EQ(loaded.load_address(), load_addr);
  EXPECT_EQ(loaded.size(), Module::AddressSize::k32Bit);
  EXPECT_EQ(loaded.phdrs().size(), 3u);

  // Verify upcasting
  EXPECT_EQ(loaded.phdrs()[0].p_type, static_cast<uint32_t>(PT_LOAD));
  EXPECT_EQ(loaded.phdrs()[0].p_vaddr, 0x1000u);
  EXPECT_EQ(loaded.phdrs()[0].p_memsz, 0x2000u);
  EXPECT_EQ(loaded.phdrs()[1].p_type, static_cast<uint32_t>(PT_LOAD));
  EXPECT_EQ(loaded.phdrs()[1].p_vaddr, 0x3000u);
  EXPECT_EQ(loaded.phdrs()[1].p_memsz, 0x2000u);
  EXPECT_EQ(loaded.phdrs()[2].p_type, static_cast<uint32_t>(PT_LOAD));
  EXPECT_EQ(loaded.phdrs()[2].p_vaddr, 0x5000u);
  EXPECT_EQ(loaded.phdrs()[2].p_memsz, 0x2000u);

  // Valid PCs don't start until the beginning of the first loaded segment.
  EXPECT_FALSE(loaded.IsValidPC(load_addr + 0x0fff));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x1500));

  // Valid inside of the second phdr.
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x3000));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x3500));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x4000));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x4fff));

  // Valid inside of the third phdr.
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x5000));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x5500));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x6000));
  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x6fff));

  // Outside of the range of the last.
  EXPECT_FALSE(loaded.IsValidPC(load_addr + 0x7000));
}

TEST(LoadedElfModule, Load32BitOnePhdr) {
  constexpr size_t kNumPhdrs = 1;
  using FakeModule = FakeElf32Module<kNumPhdrs>;

  FakeModule fake;
  memset(&fake, 0, sizeof(fake));

  // Set up 32-bit ELF header
  memcpy(fake.ehdr.e_ident, ELFMAG, SELFMAG);
  fake.ehdr.e_ident[EI_CLASS] = ELFCLASS32;
  fake.ehdr.e_ident[EI_DATA] = ELFDATA2LSB;
  fake.ehdr.e_ident[EI_VERSION] = EV_CURRENT;
  fake.ehdr.e_type = ET_DYN;
  fake.ehdr.e_machine = EM_ARM;
  fake.ehdr.e_version = EV_CURRENT;
  fake.ehdr.e_phoff = offsetof(FakeModule, phdr);
  fake.ehdr.e_phnum = kNumPhdrs;
  fake.ehdr.e_phentsize = sizeof(Elf32_Phdr);

  // Set up a PT_LOAD segment
  fake.phdr[0].p_type = PT_LOAD;
  fake.phdr[0].p_vaddr = 0x1000;
  fake.phdr[0].p_memsz = 0x2000;

  LocalMemory mem;
  uint64_t load_addr = reinterpret_cast<uint64_t>(&fake);
  Module module(load_addr, &mem, Module::AddressMode::kProcess);

  LoadedElfModule loaded(module);
  auto result = loaded.Load();
  ASSERT_TRUE(result.is_ok()) << result.error_value().msg();

  EXPECT_EQ(loaded.size(), Module::AddressSize::k32Bit);
  EXPECT_EQ(loaded.phdrs().size(), 1u);
  // Verify upcasting
  EXPECT_EQ(loaded.phdrs()[0].p_type, static_cast<uint32_t>(PT_LOAD));
  EXPECT_EQ(loaded.phdrs()[0].p_vaddr, 0x1000u);
  EXPECT_EQ(loaded.phdrs()[0].p_memsz, 0x2000u);

  EXPECT_TRUE(loaded.IsValidPC(load_addr + 0x1500));
  EXPECT_FALSE(loaded.IsValidPC(load_addr + 0x0fff));
  EXPECT_FALSE(loaded.IsValidPC(load_addr + 0x3000));
}

TEST(LoadedElfModule, GetSegmentByType) {
  constexpr size_t kNumPhdrs = 1;
  using FakeModule = FakeElf64Module<kNumPhdrs>;

  FakeModule fake;
  memset(&fake, 0, sizeof(fake));

  // Set up 64-bit ELF header
  memcpy(fake.ehdr.e_ident, ELFMAG, SELFMAG);
  fake.ehdr.e_ident[EI_CLASS] = ELFCLASS64;
  fake.ehdr.e_ident[EI_DATA] = ELFDATA2LSB;
  fake.ehdr.e_ident[EI_VERSION] = EV_CURRENT;
  fake.ehdr.e_phoff = offsetof(FakeModule, phdr);
  fake.ehdr.e_phnum = kNumPhdrs;
  fake.ehdr.e_phentsize = sizeof(Elf64_Phdr);

  fake.phdr[0].p_type = PT_NOTE;
  fake.phdr[0].p_vaddr = 0x1000;

  LocalMemory mem;
  uint64_t load_addr = reinterpret_cast<uint64_t>(&fake);
  Module module(load_addr, &mem, Module::AddressMode::kProcess);

  LoadedElfModule loaded(module);
  ASSERT_TRUE(loaded.Load().is_ok());

  // Success case
  auto res = loaded.GetSegmentByType(PT_NOTE);
  ASSERT_TRUE(res.is_ok());
  EXPECT_EQ(res->p_type, static_cast<uint32_t>(PT_NOTE));
  EXPECT_EQ(res->p_vaddr, 0x1000u);

  // Failure case
  auto res_fail = loaded.GetSegmentByType(PT_DYNAMIC);
  EXPECT_TRUE(res_fail.is_error());
}

TEST(LoadedElfModule, GetSegmentByType32) {
  constexpr size_t kNumPhdrs = 1;
  using FakeModule = FakeElf32Module<kNumPhdrs>;

  FakeModule fake;
  memset(&fake, 0, sizeof(fake));

  // Set up 32-bit ELF header
  memcpy(fake.ehdr.e_ident, ELFMAG, SELFMAG);
  fake.ehdr.e_ident[EI_CLASS] = ELFCLASS32;
  fake.ehdr.e_ident[EI_DATA] = ELFDATA2LSB;
  fake.ehdr.e_ident[EI_VERSION] = EV_CURRENT;
  fake.ehdr.e_phoff = offsetof(FakeModule, phdr);
  fake.ehdr.e_phnum = kNumPhdrs;
  fake.ehdr.e_phentsize = sizeof(Elf32_Phdr);

  fake.phdr[0].p_type = PT_NOTE;
  fake.phdr[0].p_vaddr = 0x1000;

  LocalMemory mem;
  uint64_t load_addr = reinterpret_cast<uint64_t>(&fake);
  Module module(load_addr, &mem, Module::AddressMode::kProcess);

  LoadedElfModule loaded(module);
  ASSERT_TRUE(loaded.Load().is_ok());

  auto res = loaded.GetSegmentByType(PT_NOTE);
  ASSERT_TRUE(res.is_ok());
  EXPECT_EQ(res->p_type, static_cast<uint32_t>(PT_NOTE));
  EXPECT_EQ(res->p_vaddr, 0x1000u);
}

TEST(LoadedElfModule, GetSegment) {
  constexpr size_t kNumPhdrs = 3;
  using FakeModule = FakeElf64Module<kNumPhdrs>;

  FakeModule fake;
  memset(&fake, 0, sizeof(fake));

  // Set up 64-bit ELF header
  memcpy(fake.ehdr.e_ident, ELFMAG, SELFMAG);
  fake.ehdr.e_ident[EI_CLASS] = ELFCLASS64;
  fake.ehdr.e_ident[EI_DATA] = ELFDATA2LSB;
  fake.ehdr.e_ident[EI_VERSION] = EV_CURRENT;
  fake.ehdr.e_phoff = offsetof(FakeModule, phdr);
  fake.ehdr.e_phnum = 3;
  fake.ehdr.e_phentsize = sizeof(Elf64_Phdr);

  fake.phdr[0].p_type = PT_LOAD;
  fake.phdr[0].p_vaddr = 0x1000;

  fake.phdr[1].p_type = PT_NOTE;
  fake.phdr[1].p_vaddr = 0x2000;

  fake.phdr[2].p_type = PT_DYNAMIC;
  fake.phdr[2].p_vaddr = 0x3000;

  LocalMemory mem;
  uint64_t load_addr = reinterpret_cast<uint64_t>(&fake);
  Module module(load_addr, &mem, Module::AddressMode::kProcess);

  LoadedElfModule loaded(module);
  ASSERT_TRUE(loaded.Load().is_ok());

  auto res = loaded.GetSegmentByType(PT_NOTE);
  ASSERT_TRUE(res.is_ok());
  EXPECT_EQ(res->p_type, static_cast<uint32_t>(PT_NOTE));
  EXPECT_EQ(res->p_vaddr, 0x2000u);

  auto res2 = loaded.GetSegmentByType(PT_DYNAMIC);
  ASSERT_TRUE(res2.is_ok());
  EXPECT_EQ(res2->p_type, static_cast<uint32_t>(PT_DYNAMIC));
  EXPECT_EQ(res2->p_vaddr, 0x3000u);
}

TEST(LoadedElfModule, GetSectionByName64) {
  constexpr size_t kNumPhdrs = 1;
  constexpr size_t kNumShdrs = 2;
  using FakeModule = FakeElf64Module<kNumPhdrs, kNumShdrs>;

  FakeModule fake;
  memset(&fake, 0, sizeof(fake));

  memcpy(fake.ehdr.e_ident, ELFMAG, SELFMAG);
  fake.ehdr.e_ident[EI_CLASS] = ELFCLASS64;
  fake.ehdr.e_ident[EI_DATA] = ELFDATA2LSB;
  fake.ehdr.e_ident[EI_VERSION] = EV_CURRENT;
  fake.ehdr.e_shoff = offsetof(FakeModule, shdr);
  fake.ehdr.e_shnum = kNumShdrs;
  fake.ehdr.e_shentsize = sizeof(Elf64_Shdr);
  fake.ehdr.e_shstrndx = 1;
  fake.ehdr.e_phnum = kNumPhdrs;
  fake.ehdr.e_phoff = offsetof(FakeModule, phdr);
  fake.ehdr.e_phentsize = sizeof(Elf64_Phdr);

  // Section 0: .debug_frame
  // Section 1: .shstrtab
  constexpr std::array<std::string, kNumShdrs> kSectionNames = {".debug_frame", ".shstrtab"};

  fake.shdr[0].sh_type = SHT_PROGBITS;
  fake.shdr[0].sh_offset = offsetof(FakeModule, shdr);
  fake.shdr[0].sh_size = 0;

  fake.shdr[1].sh_type = SHT_STRTAB;
  fake.shdr[1].sh_offset = offsetof(FakeModule, strtab);
  fake.shdr[1].sh_size = sizeof(fake.strtab);

  fake.phdr[0].p_type = PT_LOAD;
  fake.phdr[0].p_vaddr = 0x1000;
  fake.phdr[0].p_memsz = 0x1000;

  ASSERT_TRUE(PopulateShStrTab(fake, kSectionNames).is_ok());

  LocalMemory mem;
  uint64_t load_addr = reinterpret_cast<uint64_t>(&fake);
  Module module(load_addr, &mem, Module::AddressMode::kProcess);

  LoadedElfModule loaded(module);
  // Loading is typically handled when fetching a LoadedElfModule object from the ElfModuleCache,
  // but since we don't have that here, we have to load it ourselves.
  ASSERT_TRUE(loaded.Load().is_ok());

  auto res = loaded.GetSectionByName(".shstrtab");
  ASSERT_TRUE(res.is_ok()) << res.error_value().msg();
  EXPECT_EQ(res->sh_type, static_cast<uint32_t>(SHT_STRTAB));

  res = loaded.GetSectionByName(".debug_frame");
  ASSERT_TRUE(res.is_ok()) << res.error_value().msg();
  EXPECT_EQ(res->sh_type, static_cast<uint32_t>(SHT_PROGBITS));

  res = loaded.GetSectionByName(".nonexistent");
  EXPECT_TRUE(res.is_error());
}

TEST(LoadedElfModule, GetSectionByName64NoPhdrs) {
  constexpr size_t kNumPhdrs = 0;
  constexpr size_t kNumShdrs = 2;
  using FakeModule = FakeElf64Module<kNumPhdrs, kNumShdrs>;

  FakeModule fake;
  memset(&fake, 0, sizeof(fake));

  memcpy(fake.ehdr.e_ident, ELFMAG, SELFMAG);
  fake.ehdr.e_ident[EI_CLASS] = ELFCLASS64;
  fake.ehdr.e_ident[EI_DATA] = ELFDATA2LSB;
  fake.ehdr.e_ident[EI_VERSION] = EV_CURRENT;
  fake.ehdr.e_shoff = offsetof(FakeModule, shdr);
  fake.ehdr.e_shnum = kNumShdrs;
  fake.ehdr.e_shentsize = sizeof(Elf64_Shdr);
  fake.ehdr.e_shstrndx = 1;
  fake.ehdr.e_phnum = kNumPhdrs;

  // Section 0: .debug_frame
  // Section 1: .shstrtab
  constexpr std::array<std::string, kNumShdrs> kSectionNames = {".debug_frame", ".shstrtab"};

  fake.shdr[0].sh_type = SHT_PROGBITS;
  fake.shdr[0].sh_offset = offsetof(FakeModule, shdr);
  fake.shdr[0].sh_size = 0;

  fake.shdr[1].sh_type = SHT_STRTAB;
  fake.shdr[1].sh_offset = offsetof(FakeModule, strtab);
  fake.shdr[1].sh_size = sizeof(fake.strtab);

  ASSERT_TRUE(PopulateShStrTab(fake, kSectionNames).is_ok());

  LocalMemory mem;
  uint64_t load_addr = reinterpret_cast<uint64_t>(&fake);
  Module module(load_addr, &mem, Module::AddressMode::kProcess);

  LoadedElfModule loaded(module);
  // Loading is typically handled when fetching a LoadedElfModule object from the ElfModuleCache,
  // but since we don't have that here, we have to load it ourselves.
  ASSERT_TRUE(loaded.Load().is_ok());

  auto res = loaded.GetSectionByName(".shstrtab");
  ASSERT_TRUE(res.is_ok()) << res.error_value().msg();
  EXPECT_EQ(res->sh_type, static_cast<uint32_t>(SHT_STRTAB));

  res = loaded.GetSectionByName(".debug_frame");
  ASSERT_TRUE(res.is_ok()) << res.error_value().msg();
  EXPECT_EQ(res->sh_type, static_cast<uint32_t>(SHT_PROGBITS));

  res = loaded.GetSectionByName(".nonexistent");
  EXPECT_TRUE(res.is_error());
}

TEST(LoadedElfModule, GetSectionByName32) {
  constexpr size_t kNumPhdrs = 1;
  constexpr size_t kNumShdrs = 2;
  using FakeModule = FakeElf32Module<kNumPhdrs, kNumShdrs>;

  FakeModule fake;
  memset(&fake, 0, sizeof(fake));

  memcpy(fake.ehdr.e_ident, ELFMAG, SELFMAG);
  fake.ehdr.e_ident[EI_CLASS] = ELFCLASS32;
  fake.ehdr.e_ident[EI_DATA] = ELFDATA2LSB;
  fake.ehdr.e_ident[EI_VERSION] = EV_CURRENT;
  fake.ehdr.e_shoff = offsetof(FakeModule, shdr);
  fake.ehdr.e_shnum = kNumShdrs;
  fake.ehdr.e_shentsize = sizeof(Elf32_Shdr);
  fake.ehdr.e_shstrndx = 1;
  fake.ehdr.e_phnum = kNumPhdrs;
  fake.ehdr.e_phoff = offsetof(FakeModule, phdr);
  fake.ehdr.e_phentsize = sizeof(Elf32_Phdr);

  fake.phdr[0].p_type = PT_LOAD;
  fake.phdr[0].p_vaddr = 0x1000;
  fake.phdr[0].p_memsz = 0x1000;

  // Section 0: .debug_frame
  // Section 1: .shstrtab
  constexpr std::array<std::string, kNumShdrs> kSectionNames = {".debug_frame", ".shstrtab"};
  fake.shdr[0].sh_type = SHT_PROGBITS;
  fake.shdr[0].sh_offset = offsetof(FakeModule, shdr);
  fake.shdr[0].sh_size = 0;

  fake.shdr[1].sh_type = SHT_STRTAB;
  fake.shdr[1].sh_offset = offsetof(FakeModule, strtab);
  fake.shdr[1].sh_size = sizeof(fake.strtab);

  ASSERT_TRUE(PopulateShStrTab(fake, kSectionNames).is_ok());

  LocalMemory mem;
  uint64_t load_addr = reinterpret_cast<uint64_t>(&fake);
  Module module(load_addr, &mem, Module::AddressMode::kProcess);

  LoadedElfModule loaded(module);
  ASSERT_TRUE(loaded.Load().is_ok());

  auto res = loaded.GetSectionByName(".shstrtab");
  ASSERT_TRUE(res.is_ok()) << res.error_value().msg();
  EXPECT_EQ(res->sh_type, static_cast<uint32_t>(SHT_STRTAB));

  res = loaded.GetSectionByName(".debug_frame");
  ASSERT_TRUE(res.is_ok()) << res.error_value().msg();
  EXPECT_EQ(res->sh_type, static_cast<uint32_t>(SHT_PROGBITS));

  res = loaded.GetSectionByName(".nonexistent");
  ASSERT_TRUE(res.is_error());
}

TEST(LoadedElfModule, InvalidHeader) {
  constexpr size_t kNumPhdrs = 1;
  using FakeModule = FakeElf64Module<kNumPhdrs>;

  FakeModule fake;
  memset(&fake, 0, sizeof(fake));
  // Wrong magic
  memcpy(fake.ehdr.e_ident, "NOTELF", 6);

  LocalMemory mem;
  uint64_t load_addr = reinterpret_cast<uint64_t>(&fake);
  // Module constructor might fail or ProbeElfModuleClass returns kUnknown
  Module module(load_addr, &mem, Module::AddressMode::kProcess);

  LoadedElfModule loaded(module);
  auto result = loaded.Load();
  EXPECT_TRUE(result.is_error());
}

}  // namespace

}  // namespace unwinder
