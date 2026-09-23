// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/ffx/lib/profiler/sys/unwinder_wrapper.h"

#include <elf.h>
#include <string.h>
#include <zircon/syscalls/debug.h>

#include <memory>
#include <string>
#include <vector>

#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/module.h"
#include "src/lib/unwinder/registers.h"
#include "src/lib/unwinder/unwind.h"

namespace {
struct MemoryChunk {
  uint64_t base;
  std::vector<uint8_t> data;
};

class ChunkMemory : public unwinder::Memory {
 public:
  ChunkMemory() = default;

  void AddChunk(uint64_t base, const uint8_t* data, size_t size) {
    chunks_.push_back({base, std::vector<uint8_t>(data, data + size)});
  }

  void Clear() { chunks_.clear(); }

  unwinder::Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override {
    uint8_t* dst_ptr = static_cast<uint8_t*>(dst);

    for (const auto& chunk : chunks_) {
      if (addr >= chunk.base && addr < chunk.base + chunk.data.size()) {
        size_t offset = addr - chunk.base;
        size_t available = chunk.data.size() - offset;
        if (size > available) {
          return unwinder::Error("ChunkMemory read out of chunk bounds");
        }

        memcpy(dst_ptr, chunk.data.data() + offset, size);
        return unwinder::Success();
      }
    }

    return unwinder::Error("ChunkMemory read bounds out of range");
  }

 private:
  std::vector<MemoryChunk> chunks_;
};
}  // namespace

namespace profiler {
namespace {

unwinder::Registers FromFuchsiaRegistersHost(const uint8_t* regs_data, size_t regs_size) {
  static_assert(sizeof(zx_x86_64_thread_state_general_regs_t) !=
                sizeof(zx_arm64_thread_state_general_regs_t));
  static_assert(sizeof(zx_x86_64_thread_state_general_regs_t) !=
                sizeof(zx_riscv64_thread_state_general_regs_t));
  static_assert(sizeof(zx_arm64_thread_state_general_regs_t) !=
                sizeof(zx_riscv64_thread_state_general_regs_t));

  if (regs_size == sizeof(zx_x86_64_thread_state_general_regs_t)) {
    const auto* regs = reinterpret_cast<const zx_x86_64_thread_state_general_regs_t*>(regs_data);
    unwinder::Registers res(unwinder::Registers::Arch::kX64);
    res.Set(unwinder::RegisterID::kX64_rax, regs->rax);
    res.Set(unwinder::RegisterID::kX64_rbx, regs->rbx);
    res.Set(unwinder::RegisterID::kX64_rcx, regs->rcx);
    res.Set(unwinder::RegisterID::kX64_rdx, regs->rdx);
    for (int i = 4; i < static_cast<int>(unwinder::RegisterID::kX64_last); i++) {
      res.Set(static_cast<unwinder::RegisterID>(i), reinterpret_cast<const uint64_t*>(regs)[i]);
    }
    return res;
  }

  if (regs_size == sizeof(zx_arm64_thread_state_general_regs_t)) {
    const auto* regs = reinterpret_cast<const zx_arm64_thread_state_general_regs_t*>(regs_data);
    unwinder::Registers res(unwinder::Registers::Arch::kArm64);
    for (int i = 0; i < static_cast<int>(unwinder::RegisterID::kArm64_last); i++) {
      res.Set(static_cast<unwinder::RegisterID>(i), reinterpret_cast<const uint64_t*>(regs)[i]);
    }
    return res;
  }

  if (regs_size == sizeof(zx_riscv64_thread_state_general_regs_t)) {
    const auto* regs = reinterpret_cast<const zx_riscv64_thread_state_general_regs_t*>(regs_data);
    unwinder::Registers res(unwinder::Registers::Arch::kRiscv64);
    res.SetPC(regs->pc);
    for (int i = 1; i < static_cast<int>(unwinder::RegisterID::kRiscv64_last); i++) {
      res.Set(static_cast<unwinder::RegisterID>(i), reinterpret_cast<const uint64_t*>(regs)[i]);
    }
    return res;
  }
  // Fallback if unsupported or unknown size
  return unwinder::Registers(unwinder::Registers::Arch::kArm64);
}

struct ElfMapping {
  uint64_t vaddr;
  uint64_t memsz;
  uint64_t filesz;
  uint64_t offset;
};

class MappedElfFileMemory : public unwinder::Memory {
 public:
  MappedElfFileMemory(uint64_t load_address, std::unique_ptr<unwinder::FileMemory> file_memory)
      : load_address_(load_address), file_memory_(std::move(file_memory)) {
    uint8_t e_ident[EI_NIDENT];
    if (file_memory_->ReadBytes(0, EI_NIDENT, e_ident).has_err()) {
      return;
    }

    if (e_ident[EI_CLASS] == ELFCLASS64) {
      Elf64_Ehdr ehdr;
      if (file_memory_->ReadBytes(0, sizeof(ehdr), &ehdr).has_err()) {
        return;
      }
      for (int i = 0; i < ehdr.e_phnum; i++) {
        Elf64_Phdr phdr;
        if (file_memory_
                ->ReadBytes(ehdr.e_phoff + (static_cast<uint64_t>(i) * ehdr.e_phentsize),
                            sizeof(phdr), &phdr)
                .has_err()) {
          continue;
        }
        if (phdr.p_type == PT_LOAD) {
          mappings_.push_back({phdr.p_vaddr, phdr.p_memsz, phdr.p_filesz, phdr.p_offset});
        }
      }
    } else if (e_ident[EI_CLASS] == ELFCLASS32) {
      Elf32_Ehdr ehdr;
      if (file_memory_->ReadBytes(0, sizeof(ehdr), &ehdr).has_err()) {
        return;
      }
      for (int i = 0; i < ehdr.e_phnum; i++) {
        Elf32_Phdr phdr;
        if (file_memory_
                ->ReadBytes(ehdr.e_phoff + (static_cast<uint64_t>(i) * ehdr.e_phentsize),
                            sizeof(phdr), &phdr)
                .has_err()) {
          continue;
        }
        if (phdr.p_type == PT_LOAD) {
          mappings_.push_back({phdr.p_vaddr, phdr.p_memsz, phdr.p_filesz, phdr.p_offset});
        }
      }
    }
  }

  unwinder::Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override {
    if (addr < load_address_) {
      return unwinder::Error("out of bounds");
    }
    uint64_t vaddr = addr - load_address_;
    uint8_t* dst_ptr = static_cast<uint8_t*>(dst);

    for (const auto& mapping : mappings_) {
      if (vaddr >= mapping.vaddr && vaddr < mapping.vaddr + mapping.memsz) {
        size_t offset = vaddr - mapping.vaddr;
        size_t available = mapping.memsz - offset;
        if (size > available) {
          return unwinder::Error("vaddr read out of mapping bounds");
        }

        if (offset < mapping.filesz) {
          size_t file_available = mapping.filesz - offset;
          size_t file_copy_size = std::min(static_cast<size_t>(size), file_available);
          uint64_t file_offset = mapping.offset + offset;
          if (auto err = file_memory_->ReadBytes(file_offset, file_copy_size, dst_ptr);
              err.has_err()) {
            return err;
          }
          if (file_copy_size < size) {
            memset(dst_ptr + file_copy_size, 0, size - file_copy_size);
          }
        } else {
          memset(dst_ptr, 0, size);
        }

        return unwinder::Success();
      }
    }

    return unwinder::Error("vaddr out of mapped ranges");
  }

 private:
  uint64_t load_address_;
  std::unique_ptr<unwinder::FileMemory> file_memory_;
  std::vector<ElfMapping> mappings_;
};

}  // namespace
}  // namespace profiler

struct ffi_unwinder_t {
  std::vector<std::unique_ptr<profiler::MappedElfFileMemory>> file_memories;
  std::vector<unwinder::Module> modules;
  ChunkMemory memory;
};

extern "C" {

ffi_unwinder_t* ffi_unwinder_new() { return new ffi_unwinder_t(); }

void ffi_unwinder_free(ffi_unwinder_t* unwinder) {
  assert(unwinder);
  delete unwinder;
}

void ffi_unwinder_add_memory(ffi_unwinder_t* unwinder, uint64_t base, const uint8_t* data,
                             size_t size) {
  assert(unwinder);
  unwinder->memory.AddChunk(base, data, size);
}

void ffi_unwinder_clear_memory(ffi_unwinder_t* unwinder) {
  assert(unwinder);
  unwinder->memory.Clear();
}

void ffi_unwinder_add_module(ffi_unwinder_t* unwinder, uint64_t load_address, const char* file_path,
                             size_t file_path_len) {
  assert(unwinder);
  if (file_path && file_path_len > 0) {
    std::string path(file_path, file_path_len);
    auto raw_mem = std::make_unique<unwinder::FileMemory>(path);
    auto file_mem =
        std::make_unique<profiler::MappedElfFileMemory>(load_address, std::move(raw_mem));
    unwinder::Memory* mem_ptr = file_mem.get();
    unwinder->file_memories.push_back(std::move(file_mem));

    unwinder->modules.emplace_back(load_address, mem_ptr, unwinder::Module::AddressMode::kProcess);
  }
}

size_t ffi_unwinder_unwind(ffi_unwinder_t* unwinder, const uint8_t* regs_data, size_t regs_size,
                           ffi_frame_t* output_frames, size_t max_depth) {
  assert(unwinder);
  assert(regs_data);
  assert(output_frames);
  if (regs_size == 0) {
    return 0;
  }

  unwinder::Registers unwinder_regs = profiler::FromFuchsiaRegistersHost(regs_data, regs_size);
  unwinder::Unwinder cxx_unwinder(unwinder->modules);

  std::vector<unwinder::Frame> frames =
      cxx_unwinder.Unwind(&unwinder->memory, unwinder_regs, max_depth);

  size_t count = 0;
  for (const auto& frame : frames) {
    if (count >= max_depth)
      break;

    uint64_t pc = 0;
    uint64_t sp = 0;
    if (frame.regs.GetPC(pc).has_err() || pc == 0) {
      continue;
    }
    frame.regs.GetSP(sp);  // best-effort SP

    output_frames[count].pc = pc;
    output_frames[count].sp = sp;
    count++;
  }

  return count;
}
}
