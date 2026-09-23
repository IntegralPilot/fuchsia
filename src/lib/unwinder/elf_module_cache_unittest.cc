// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/elf_module_cache.h"

#include <elf.h>
#include <inttypes.h>

#include <cstring>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/module.h"

namespace unwinder {

namespace {

struct FakeElfModule {
  Elf64_Ehdr ehdr;
  Elf64_Phdr phdr;
};

void SetupFakeElf(FakeElfModule& fake, uint64_t vaddr, uint64_t memsz) {
  memset(&fake, 0, sizeof(fake));
  memcpy(fake.ehdr.e_ident, ELFMAG, SELFMAG);
  fake.ehdr.e_ident[EI_CLASS] = ELFCLASS64;
  fake.ehdr.e_ident[EI_DATA] = ELFDATA2LSB;
  fake.ehdr.e_ident[EI_VERSION] = EV_CURRENT;
  fake.ehdr.e_type = ET_DYN;
  fake.ehdr.e_machine = EM_X86_64;
  fake.ehdr.e_version = EV_CURRENT;
  fake.ehdr.e_phoff = offsetof(FakeElfModule, phdr);
  fake.ehdr.e_phnum = 1;
  fake.ehdr.e_phentsize = sizeof(Elf64_Phdr);

  fake.phdr.p_type = PT_LOAD;
  fake.phdr.p_vaddr = vaddr;
  fake.phdr.p_memsz = memsz;
  fake.phdr.p_flags = PF_R | PF_X;
}

class FakeMemory : public Memory {
 public:
  void AddModule(uint64_t load_addr, void* real_addr, uint64_t size) {
    modules_[load_addr] = {.real_ptr = reinterpret_cast<uint64_t>(real_addr), .size = size};
  }

  Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override {
    for (auto const& [load_addr, info] : modules_) {
      if (addr >= load_addr && addr + size <= load_addr + info.size) {
        uint64_t offset = addr - load_addr;
        memcpy(dst, reinterpret_cast<void*>(info.real_ptr + offset), size);
        return Success();
      }
    }
    return Error("Address not mapped in FakeMemory: %#" PRIx64 "", addr);
  }

 private:
  struct ModuleInfo {
    uint64_t real_ptr;
    uint64_t size;
  };
  std::map<uint64_t, ModuleInfo> modules_;
};

TEST(ElfModuleCache, Lookup) {
  FakeElfModule fake1;
  SetupFakeElf(fake1, 0x1000, 0x1000);
  FakeElfModule fake2;
  SetupFakeElf(fake2, 0x1000, 0x1000);

  constexpr uint64_t kLoadAddr1 = 0x10000000;
  constexpr uint64_t kLoadAddr2 = 0x20000000;

  FakeMemory mem;
  mem.AddModule(kLoadAddr1, &fake1, sizeof(fake1));
  mem.AddModule(kLoadAddr2, &fake2, sizeof(fake2));

  std::vector<Module> modules;
  modules.emplace_back(kLoadAddr1, &mem, Module::AddressMode::kProcess);
  modules.emplace_back(kLoadAddr2, &mem, Module::AddressMode::kProcess);

  ElfModuleCache cache(modules);

  // PC in module 1: load_addr1 + 0x1500
  auto result1 = cache.GetLoadedElfModuleForPc(kLoadAddr1 + 0x1500);
  ASSERT_TRUE(result1.is_ok()) << result1.error_value().msg();
  EXPECT_EQ(result1.value().get().load_address(), kLoadAddr1);

  // PC in module 2: load_addr2 + 0x1500
  auto result2 = cache.GetLoadedElfModuleForPc(kLoadAddr2 + 0x1500);
  ASSERT_TRUE(result2.is_ok()) << result2.error_value().msg();
  EXPECT_EQ(result2.value().get().load_address(), kLoadAddr2);

  // PC not in any module (below first)
  EXPECT_TRUE(cache.GetLoadedElfModuleForPc(0).is_error());

  // PC just outside ranges
  EXPECT_TRUE(cache.GetLoadedElfModuleForPc(kLoadAddr1 + 0x0500).is_error());
  EXPECT_TRUE(cache.GetLoadedElfModuleForPc(kLoadAddr1 + 0x0fff).is_error());
  EXPECT_TRUE(cache.GetLoadedElfModuleForPc(kLoadAddr1 + 0x2000).is_error());
  EXPECT_TRUE(cache.GetLoadedElfModuleForPc(kLoadAddr1 + 0x2500).is_error());

  // IsValidPC
  EXPECT_TRUE(cache.IsValidPC(kLoadAddr1 + 0x1500));
  EXPECT_TRUE(cache.IsValidPC(kLoadAddr2 + 0x1500));
  EXPECT_FALSE(cache.IsValidPC(kLoadAddr1 + 0x0500));
  EXPECT_FALSE(cache.IsValidPC(kLoadAddr2 + 0x2500));
}

TEST(ElfModuleCache, LookupEmpty) {
  std::vector<Module> modules;

  ElfModuleCache cache(modules);

  ASSERT_FALSE(cache.IsValidPC(0x12345));
  ASSERT_TRUE(cache.GetLoadedElfModuleForPc(0x12345).is_error());
}

}  // namespace

}  // namespace unwinder
