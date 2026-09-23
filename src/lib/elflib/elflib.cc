// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/elflib/elflib.h"

#include <zircon/assert.h>

#include <algorithm>
#include <iterator>
#include <limits>
#include <span>
#include <type_traits>

#include <llvm/BinaryFormat/ELF.h>

namespace elflib {
namespace {

// NT_GNU_BUILD_ID identifier.
constexpr uint64_t kNoteGnuBuildId = 3;

// Pull a null-terminated string out of an array of bytes at an offset. Returns
// empty string if there is no null terminator.
std::string GetNullTerminatedStringAt(const uint8_t* data, size_t data_length, size_t offset) {
  size_t check = offset;

  while (check < data_length && data[check]) {
    check++;
  }

  if (check >= data_length) {
    return std::string();
  }

  const char* start = reinterpret_cast<const char*>(data) + offset;

  return std::string(start);
}

// This method enables the user to deserialize from a raw pointer (void or char*
// or similar) into a data structure, even if the raw pointer may be to an
// unaligned data structure.  Unaligned access is undefined in C++.  This
// happens frequently with the deserialized ELF libs.
template <typename Dest, typename Source>
inline Dest bit_cast(const Source& source) {
  Dest dest;
  memcpy(static_cast<void*>(std::addressof(dest)), static_cast<const void*>(std::addressof(source)),
         sizeof(dest));
  return dest;
}

// This method enables the user to deserialize from a pointer into a
// deserialized buffer, if the pointer might be unaligned.  Unaligned access is
// undefined in C++.  This happens frequently with the deserialized ELF libs.
template <typename T>
inline T safe_copy(const T* source) {
  T dest;
  memcpy(std::addressof(dest), static_cast<const void*>(source), sizeof(T));
  return dest;
}

Elf64_Sym UpcastSym(const Elf32_Sym& sym) {
  Elf64_Sym upcast;
  upcast.st_name = sym.st_name;
  upcast.st_info = sym.st_info;
  upcast.st_shndx = sym.st_shndx;
  upcast.st_value = sym.st_value;
  upcast.st_size = sym.st_size;
  upcast.st_other = sym.st_other;
  return upcast;
}

template <typename ElfSym>
std::optional<Elf64_Sym> FindElfSymbolInTable(
    const std::string& name, const ElfLib::MemoryRegion& symtab,
    std::function<std::optional<std::string>(uint64_t)> get_string) {
  const ElfSym* symbols = reinterpret_cast<const ElfSym*>(symtab.ptr);
  const ElfSym* end = symbols + (symtab.size / sizeof(ElfSym));

  ElfSym s;
  for (auto symbol = symbols; symbol <= end; symbol++) {
    s = safe_copy<ElfSym>(symbol);
    auto got_name = get_string(s.st_name);

    if (got_name && *got_name == name) {
      break;
    }
  }

  if constexpr (std::is_same_v<ElfSym, Elf32_Sym>) {
    return UpcastSym(s);
  } else {
    return s;
  }

  return std::nullopt;
}

// Given a name, a symbol table (sized array of Elf64_Sym), and an accessor for
// a corresponding string table, find the symbol with the given name.
std::optional<Elf64_Sym> GetSymbolFromTable(
    const Elf64_Ehdr& ehdr, const std::string& name, const ElfLib::MemoryRegion& symtab,
    std::function<std::optional<std::string>(uint64_t)> get_string) {
  if (!symtab.ptr) {
    return std::nullopt;
  }

  if (ehdr.e_ident[EI_CLASS] == ELFCLASS32) {
    return FindElfSymbolInTable<Elf32_Sym>(name, symtab, std::move(get_string));
  }

  return FindElfSymbolInTable<Elf64_Sym>(name, symtab, std::move(get_string));
}

std::optional<std::map<std::string, Elf64_Sym>> SymtabToMap(const Elf64_Ehdr& ehdr,
                                                            const ElfLib::MemoryRegion& symtab,
                                                            const ElfLib::MemoryRegion& strtab) {
  if (!symtab.ptr)
    return std::nullopt;

  std::map<std::string, Elf64_Sym> out;

  if (ehdr.e_ident[EI_CLASS] == ELFCLASS32) {
    const Elf32_Sym* symbols = reinterpret_cast<const Elf32_Sym*>(symtab.ptr);
    const Elf32_Sym* end = symbols + (symtab.size / sizeof(Elf32_Sym));
    for (auto symbol = symbols; symbol != end; symbol++) {
      Elf32_Sym s = safe_copy<Elf32_Sym>(symbol);
      auto sym_name = GetNullTerminatedStringAt(strtab.ptr, strtab.size, s.st_name);
      out[sym_name] = UpcastSym(s);
    }

    return out;
  }

  const Elf64_Sym* symbols = reinterpret_cast<const Elf64_Sym*>(symtab.ptr);
  const Elf64_Sym* end = symbols + (symtab.size / sizeof(Elf64_Sym));
  for (auto symbol = symbols; symbol != end; symbol++) {
    Elf64_Sym s = safe_copy<Elf64_Sym>(symbol);
    auto sym_name = GetNullTerminatedStringAt(strtab.ptr, strtab.size, s.st_name);
    out[sym_name] = s;
  }

  return out;
}

}  // namespace

// Proxy object for whatever address space we're exploring.
class ElfLib::MemoryAccessor {
 public:
  virtual ~MemoryAccessor() = default;

  virtual const uint8_t* GetMemory(uint64_t mapped_address, size_t mapped_size) = 0;
};

ElfLib::ElfLib(std::unique_ptr<MemoryAccessor>&& memory, uint64_t load_address,
               ElfLib::AddressMode address_mode)
    : address_mode_(address_mode), load_address_(load_address), memory_(std::move(memory)) {}

ElfLib::~ElfLib() = default;

std::unique_ptr<ElfLib> ElfLib::Create(std::unique_ptr<MemoryAccessor>&& memory,
                                       uint64_t load_address, ElfLib::AddressMode address_mode) {
  std::unique_ptr<ElfLib> out{new ElfLib(std::move(memory), load_address, address_mode)};

  auto ident = out->memory_->GetMemory(0, EI_NIDENT);
  if (!ident) {
    return std::unique_ptr<ElfLib>();
  }

  // Header magic should be correct.
  if (!std::equal(ElfMagic, ElfMagic + 4, ident)) {
    return std::unique_ptr<ElfLib>();
  }

  if (ident[EI_CLASS] != ELFCLASS64 && ident[EI_CLASS] != ELFCLASS32) {
    return std::unique_ptr<ElfLib>();
  }

  const uint32_t kOne = 1;

  // Endianness of the file has to match the endianness of the host. To do the
  // endianness check, we snip the first byte off of a 4-byte word. If it
  // contains the LSB (a value of 1) we are on a little-endian machine.
  if (ident[EI_DATA] == ELFDATA2MSB && *reinterpret_cast<const char*>(&kOne)) {
    return std::unique_ptr<ElfLib>();
  }

  // Version field has only had one correct value for most of the life of the
  // spec.
  if (ident[EI_VERSION] != EV_CURRENT) {
    return std::unique_ptr<ElfLib>();
  }

  if (ident[EI_CLASS] == ELFCLASS64) {
    auto header =
        reinterpret_cast<const Elf64_Ehdr*>(out->memory_->GetMemory(0, sizeof(Elf64_Ehdr)));
    if (!header) {
      return std::unique_ptr<ElfLib>();
    }
    out->header_ = *header;
  } else if (ident[EI_CLASS] == ELFCLASS32) {
    auto header =
        reinterpret_cast<const Elf32_Ehdr*>(out->memory_->GetMemory(0, sizeof(Elf32_Ehdr)));
    if (!header) {
      return std::unique_ptr<ElfLib>();
    }

    // Convert the 32-bit header to a 64-bit one.
    out->header_ = {
        .e_type = header->e_type,
        .e_machine = header->e_machine,
        .e_version = header->e_version,
        .e_entry = header->e_entry,
        .e_phoff = header->e_phoff,
        .e_shoff = header->e_shoff,
        .e_flags = header->e_flags,
        .e_ehsize = header->e_ehsize,
        .e_phentsize = header->e_phentsize,
        .e_phnum = header->e_phnum,
        .e_shentsize = header->e_shentsize,
        .e_shnum = header->e_shnum,
        .e_shstrndx = header->e_shstrndx,
    };
    memcpy(out->header_.e_ident, ident, EI_NIDENT);
  } else {
    return std::unique_ptr<ElfLib>();
  }

  if (out->header_.e_version != EV_CURRENT) {
    return std::unique_ptr<ElfLib>();
  }

  // We'll skip EI_OSABI and EI_ABIVERSION as well as e_machine and e_type. In
  // either case any valid value should be fine. We just don't screen for
  // invalid values.

  // We don't support non-standard section header sizes. Stripped binaries that
  // don't have sections sometimes zero out the shentsize, so we can ignore it
  // if we have no sections.
  auto section_header_size =
      ident[EI_CLASS] == ELFCLASS64 ? sizeof(Elf64_Shdr) : sizeof(Elf32_Shdr);
  if (out->header_.e_shnum > 0 && out->header_.e_shentsize != section_header_size) {
    return std::unique_ptr<ElfLib>();
  }

  // We don't support non-standard program header sizes.
  auto program_header_size =
      ident[EI_CLASS] == ELFCLASS64 ? sizeof(Elf64_Phdr) : sizeof(Elf32_Phdr);
  if (out->header_.e_phentsize != program_header_size) {
    return std::unique_ptr<ElfLib>();
  }

  return out;
}

std::unique_ptr<ElfLib> ElfLib::Create(FILE* fp, ElfLib::Ownership owned) {
  class FileAccessor : public MemoryAccessor {
   public:
    FileAccessor(FILE* fp, bool take_ownership) : file_(fp), take_ownership_(take_ownership) {}

    ~FileAccessor() {
      if (file_ && take_ownership_) {
        fclose(file_);
      }
    }

    const uint8_t* GetMemory(uint64_t offset, size_t size) override {
      if (!file_) {
        return nullptr;
      }
      if (offset > std::numeric_limits<long>::max()) {
        return nullptr;
      }

      auto& ret = data_[std::make_pair(offset, size)];
      if (ret.size() == size) {
        return ret.data();
      }

      ret.resize(size);

      fseek(file_, static_cast<long>(offset), SEEK_SET);
      if (fread(ret.data(), 1, size, file_) != size) {
        return nullptr;
      }

      return ret.data();
    }

   private:
    FILE* file_ = nullptr;
    bool take_ownership_;

    // Elflib treats our read functions like they're random-access, so we cache
    // the results of reads. We could end up with overlaps in this map, though
    // in practice we don't in today's code.
    std::map<std::pair<uint64_t, size_t>, std::vector<uint8_t>> data_;
  };

  return Create(std::make_unique<FileAccessor>(fp, owned == ElfLib::Ownership::kTakeOwnership), 0,
                AddressMode::kFile);
}

std::unique_ptr<ElfLib> ElfLib::Create(const uint8_t* mem, size_t size) {
  class DataAccessor : public MemoryAccessor {
   public:
    DataAccessor(const uint8_t* mem, size_t size) : mem_(mem), size_(size) {}

    const uint8_t* GetMemory(uint64_t offset, size_t size) override {
      // This check and the one below are NOT redundant. This one prevents the other one from
      // returning a false negative due to overflow.
      if (offset > size_) {
        return nullptr;
      }

      if (size_ - offset < size) {
        return nullptr;
      }

      return mem_ + offset;
    }

   private:
    const uint8_t* mem_;
    size_t size_;
  };

  return Create(std::make_unique<DataAccessor>(mem, size), 0, AddressMode::kFile);
}

std::unique_ptr<ElfLib> ElfLib::Create(std::function<bool(uint64_t, std::vector<uint8_t>*)> fetch,
                                       uint64_t load_address, ElfLib::AddressMode address_mode) {
  class CallbackAccessor : public MemoryAccessor {
   public:
    CallbackAccessor(std::function<bool(uint64_t, std::vector<uint8_t>*)> fetch) : fetch_(fetch) {}

    const uint8_t* GetMemory(uint64_t offset, size_t size) override {
      for (const auto& range : data_[offset]) {
        if (range.size() >= size) {
          return range.data();
        }
      }

      auto& vec = data_[offset].emplace_back(size, 0);

      if (!fetch_(offset, &vec)) {
        return nullptr;
      }

      return vec.data();
    }

   private:
    std::function<bool(uint64_t, std::vector<uint8_t>*)> fetch_;
    std::map<uint64_t, std::vector<std::vector<uint8_t>>> data_;
  };

  return Create(std::make_unique<CallbackAccessor>(std::move(fetch)), load_address, address_mode);
}

std::unique_ptr<ElfLib> ElfLib::Create(const std::string& path) {
  return Create(fopen(path.c_str(), "r"), ElfLib::Ownership::kTakeOwnership);
}

bool ElfLib::SetDebugData(std::unique_ptr<ElfLib> debug) {
  if (debug_) {
    return false;
  }

  if (debug->debug_) {
    return false;
  }

  debug_ = std::move(debug);

  debug_->LoadSectionNames();
  section_names_ = debug_->section_names_;
  sections_ = debug_->sections_;

  LoadProgramHeaders();
  std::map<size_t, size_t> bounds;
  for (size_t i = 0; i < segments_.size(); i++) {
    if (segments_[i].p_type != PT_LOAD) {
      continue;
    }

    bounds[segments_[i].p_vaddr] = i;
  }

  for (auto& section : sections_) {
    if (section.sh_type != SHT_NOBITS) {
      // When we encounter an SHT_NULL section and we have debug data, we'll
      // consult the debug data for that section.
      section.sh_type = SHT_NULL;
      continue;
    }

    // Find the first segment starting at or before this section by finding the
    // first segment starting at or after this section, and backing up one
    // entry if necessary.
    auto found = bounds.lower_bound(section.sh_addr);
    if (found == bounds.end()) {
      continue;
    }
    if (found->first != section.sh_addr) {
      if (found == bounds.begin()) {
        continue;
      }

      --found;
    }

    auto& segment = segments_[found->second];

    if (segment.p_vaddr + segment.p_memsz <= section.sh_addr) {
      continue;
    }

    section.sh_offset = segment.p_offset + (section.sh_addr - segment.p_vaddr);
    section.sh_type = SHT_PROGBITS;
  }

  return true;
}

const Elf64_Shdr* ElfLib::GetSectionHeader(size_t section) {
  if (!LoadSectionHeaders()) {
    return nullptr;
  }

  if (section >= sections_.size()) {
    return nullptr;
  }

  return &sections_[section];
}

bool ElfLib::LoadProgramHeaders() {
  if (!segments_.empty()) {
    return true;
  }

  if (Is64Bit()) {
    auto segments = reinterpret_cast<const Elf64_Phdr*>(
        memory_->GetMemory(header_.e_phoff, sizeof(Elf64_Phdr) * header_.e_phnum));
    if (!segments) {
      return false;
    }

    std::copy(segments, segments + header_.e_phnum, std::back_inserter(segments_));
  } else {
    auto segments_start = reinterpret_cast<const Elf32_Phdr*>(
        memory_->GetMemory(header_.e_phoff, sizeof(Elf32_Phdr) * header_.e_phnum));
    if (!segments_start) {
      return false;
    }

    auto segments = std::span{segments_start, header_.e_phnum};
    for (const auto& segment : segments) {
      Elf64_Phdr segment64;
      segment64.p_type = segment.p_type;
      segment64.p_flags = segment.p_flags;
      segment64.p_offset = segment.p_offset;
      segment64.p_vaddr = segment.p_vaddr;
      segment64.p_paddr = segment.p_paddr;
      segment64.p_filesz = segment.p_filesz;
      segment64.p_memsz = segment.p_memsz;
      segment64.p_align = segment.p_align;
      segments_.push_back(segment64);
    }
  }

  return true;
}

bool ElfLib::LoadSectionHeaders() {
  // Already loaded.
  if (!sections_.empty()) {
    return true;
  }

  // Processes may not map the section headers at all, so we don't look for
  // section headers unless we're in file mode.
  if (address_mode_ == AddressMode::kFile && sections_.empty()) {
    if (Is64Bit()) {
      auto sections = reinterpret_cast<const Elf64_Shdr*>(
          memory_->GetMemory(header_.e_shoff, sizeof(Elf64_Shdr) * header_.e_shnum));
      if (!sections) {
        return false;
      }

      std::copy(sections, sections + header_.e_shnum, std::back_inserter(sections_));
    } else {
      auto sections_start = reinterpret_cast<const Elf32_Shdr*>(
          memory_->GetMemory(header_.e_shoff, sizeof(Elf32_Shdr) * header_.e_shnum));
      if (!sections_start) {
        return false;
      }

      auto sections = std::span{sections_start, header_.e_shnum};
      for (const auto& section : sections) {
        Elf64_Shdr section64;
        section64.sh_name = section.sh_name;
        section64.sh_type = section.sh_type;
        section64.sh_flags = section.sh_flags;
        section64.sh_addr = section.sh_addr;
        section64.sh_offset = section.sh_offset;
        section64.sh_size = section.sh_size;
        section64.sh_link = section.sh_link;
        section64.sh_info = section.sh_info;
        section64.sh_addralign = section.sh_addralign;
        section64.sh_entsize = section.sh_entsize;
        sections_.push_back(section64);
      }
    }

    return true;
  }

  return false;
}

const std::vector<Elf64_Shdr>& ElfLib::GetSectionHeaders() {
  LoadSectionHeaders();
  return sections_;
}

const std::vector<Elf64_Phdr>& ElfLib::GetSegmentHeaders() {
  LoadProgramHeaders();
  return segments_;
}

ElfLib::MemoryRegion ElfLib::GetSegmentData(size_t segment) {
  LoadProgramHeaders();

  if (segment > segments_.size()) {
    return {};
  }

  const Elf64_Phdr* header = &segments_[segment];
  ElfLib::MemoryRegion result;

  if (address_mode_ == AddressMode::kFile) {
    result.ptr = memory_->GetMemory(header->p_offset, header->p_filesz);
    result.size = header->p_filesz;
  } else if (header_.e_type == ET_EXEC) {
    // If a file type is ET_EXEC, then it was likely compiled with -fno-pie.
    // These will be loaded directly at the address specified in p_vaddr, which
    // will be equal to |load_address_| plus an offset. Most implementers of
    // MemoryAccessor expect to receive an _offset_ in the first parameter of
    // GetMemory calls, so we need to calculate the offset from |load_address_|
    // explicitly.
    result.ptr = memory_->GetMemory(header->p_vaddr - load_address_, header->p_memsz);
    result.size = header->p_memsz;
  } else {
    result.ptr = memory_->GetMemory(header->p_vaddr, header->p_memsz);
    result.size = header->p_memsz;
  }

  if (result.ptr == nullptr) {
    result.size = 0;
  }

  return result;
}

static_assert(sizeof(Elf64_Nhdr) == sizeof(Elf32_Nhdr), "note parsing doesn't vary by abi width");

std::optional<std::vector<uint8_t>> ElfLib::GetNote(const std::string& name, uint64_t type) {
  LoadProgramHeaders();

  for (size_t idx = 0; idx < segments_.size(); idx++) {
    if (segments_[idx].p_type != PT_NOTE) {
      continue;
    }

    auto data = GetSegmentData(idx);
    if (data.ptr == nullptr || data.size == 0) {
      continue;
    }

    Elf64_Nhdr header;
    size_t namesz_padded;
    size_t descsz_padded;

    for (const uint8_t* pos = data.ptr; (pos + sizeof(Elf64_Nhdr)) < data.ptr + data.size;
         pos += sizeof(Elf64_Nhdr) + namesz_padded + descsz_padded) {
      header = safe_copy(reinterpret_cast<const Elf64_Nhdr*>(pos));

      // Don't overflow the padded lengths.
      if (header.n_namesz > (std::numeric_limits<Elf64_Word>::max() - 3)) {
        namesz_padded = header.n_namesz;
      } else {
        namesz_padded = (header.n_namesz + 3) & ~3UL;
      }
      if (header.n_descsz > (std::numeric_limits<Elf64_Word>::max() - 3)) {
        descsz_padded = header.n_descsz;
      } else {
        descsz_padded = (header.n_descsz + 3) & ~3UL;
      }

      if (header.n_type != type) {
        continue;
      }

      if (pos + sizeof(Elf64_Nhdr) + namesz_padded + descsz_padded > data.ptr + data.size) {
        continue;
      }
      if (header.n_namesz == 0) {
        continue;
      }

      auto name_data = pos + sizeof(Elf64_Nhdr);
      if (strncmp(reinterpret_cast<const char*>(name_data), name.c_str(), name.size()) == 0) {
        auto desc_data = name_data + namesz_padded;

        if (header.n_descsz == std::numeric_limits<Elf64_Word>::max()) {
          return std::nullopt;
        }

        return std::vector(desc_data, desc_data + header.n_descsz);
      }
    }
  }

  return std::nullopt;
}

std::string ElfLib::GetGNUBuildID() {
  auto note = GetNote("GNU", kNoteGnuBuildId);
  if (!note) {
    return std::string();
  }

  std::string ret;

  for (const auto& byte : *note) {
    char buf[3];
    snprintf(buf, 3, "%02x", byte);
    ret += buf;
  }

  return ret;
}

std::optional<std::string> ElfLib::GetSoname() {
  LoadDynamicSymbols();

  if (soname_offset_) {
    return GetDynamicString(soname_offset_);
  }
  return std::nullopt;
}

ElfLib::MemoryRegion ElfLib::GetSectionData(size_t section) {
  auto header = GetSectionHeader(section);
  if (!header) {
    return {};
  }

  // This check has to be performed here so that |debug_| can have a chance to load its own headers.
  if (header->sh_type == SHT_NULL) {
    if (debug_) {
      // |section| is not a typo. Since we're calling into another instance of ElfLib, we have to
      // use the higher level interface so that it can load its headers which are not necessarily
      // the same this instance's.
      return debug_->GetSectionData(section);
    }

    return {};
  }

  return GetSectionData(header);
}

ElfLib::MemoryRegion ElfLib::GetSectionData(const Elf64_Shdr* header) {
  if (!header) {
    return {};
  }

  if (address_mode_ == AddressMode::kFile && header->sh_type == SHT_NOBITS) {
    return {};
  }

  ElfLib::MemoryRegion result;
  result.size = header->sh_size;

  if (address_mode_ == AddressMode::kFile) {
    result.ptr = memory_->GetMemory(header->sh_offset, header->sh_size);
  } else {
    result.ptr = memory_->GetMemory(header->sh_addr, header->sh_size);
  }

  return result;
}

bool ElfLib::LoadSectionNames() {
  if (section_names_.size() != 0) {
    return true;
  }

  auto section_name_data = GetSectionData(header_.e_shstrndx);

  if (!section_name_data.ptr) {
    return false;
  }

  size_t idx = 0;
  // We know sections_ is populated from the GetSectionData above
  for (const auto& section : sections_) {
    auto name =
        GetNullTerminatedStringAt(section_name_data.ptr, section_name_data.size, section.sh_name);
    section_names_[name] = idx;

    idx++;
  }

  return true;
}

ElfLib::MemoryRegion ElfLib::GetSectionData(const std::string& name) {
  if (!LoadSectionNames()) {
    return {};
  }

  const auto& iter = section_names_.find(name);

  if (iter == section_names_.end()) {
    return {};
  }

  return GetSectionData(iter->second);
}

template <typename ElfDyn>
bool ElfLib::LoadDynamicSymbolsForElf(const ElfLib::MemoryRegion& memory) {
  if (memory.ptr == nullptr) {
    return false;
  }

  const ElfDyn* start = reinterpret_cast<const ElfDyn*>(memory.ptr);
  const ElfDyn* end = start + (memory.size / sizeof(ElfDyn));
  for (auto dyn = start; dyn != end; dyn++) {
    if (dyn->d_tag == DT_STRTAB) {
      if (dynstr_.offset) {
        Warn("Multiple DT_STRTAB entries found.");
        continue;
      }

      dynstr_.offset = MappedAddressToOffset(dyn->d_un.d_ptr);
    } else if (dyn->d_tag == DT_SYMTAB) {
      if (dynsym_.offset) {
        Warn("Multiple DT_SYMTAB entries found.");
        continue;
      }

      dynsym_.offset = MappedAddressToOffset(dyn->d_un.d_ptr);
    } else if (dyn->d_tag == DT_STRSZ) {
      if (dynstr_.size) {
        Warn("Multiple DT_STRSZ entries found.");
        continue;
      }

      dynstr_.size = dyn->d_un.d_val;
    } else if (dyn->d_tag == DT_HASH) {
      // A note: The old DT_HASH style of hash table is considered legacy on
      // Fuchsia. Technically a binary could provide both styles of hash
      // table and we can produce a reasonable result in that case, so this code
      // ignores DT_HASH.
      Warn("Old style DT_HASH table found.");
    } else if (dyn->d_tag == DT_GNU_HASH) {
      if (dynsym_.size) {
        Warn("Multiple DT_GNU_HASH entries found.");
        continue;
      }
      auto addr = dyn->d_un.d_ptr;

      // Our elf header doesn't provide the DT_GNU_HASH header structure.
      struct Header {
        uint32_t nbuckets;
        uint32_t symoffset;
        uint32_t bloom_size;
        uint32_t bloom_shift;
      } header;

      static_assert(sizeof(Header) == 16);

      auto data = memory_->GetMemory(MappedAddressToOffset(addr), sizeof(header));

      if (!data) {
        continue;
      }

      header = *reinterpret_cast<const Header*>(data);

      addr += sizeof(header);

      if constexpr (std::is_same_v<ElfDyn, Elf32_Dyn>) {
        addr += 4 * header.bloom_size;
      } else {
        addr += 8 * header.bloom_size;
      }

      size_t bucket_bytes = 4 * header.nbuckets;
      auto bucket_data = memory_->GetMemory(MappedAddressToOffset(addr), bucket_bytes);

      if (!bucket_data) {
        continue;
      }

      const uint32_t* buckets = reinterpret_cast<const uint32_t*>(bucket_data);
      uint32_t max_bucket = *std::max_element(buckets, buckets + header.nbuckets);

      if (max_bucket < header.symoffset) {
        dynsym_.size = max_bucket;
        continue;
      }

      addr += bucket_bytes;
      addr += (max_bucket - header.symoffset) * 4;

      for (uint32_t nsyms = max_bucket + 1;; nsyms++, addr += 4) {
        auto chain_entry_data = memory_->GetMemory(MappedAddressToOffset(addr), 4);

        if (!chain_entry_data) {
          break;
        }

        uint32_t chain_entry = *reinterpret_cast<const uint32_t*>(chain_entry_data);

        if (chain_entry & 1) {
          dynsym_.size = nsyms;
          break;
        }
      }
    } else if (dyn->d_tag == DT_PLTREL) {
      dynamic_plt_use_rela_ = dyn->d_un.d_val == DT_RELA;
    } else if (dyn->d_tag == DT_SONAME) {
      soname_offset_ = dyn->d_un.d_val;
    }
  }
  return true;
}

bool ElfLib::LoadDynamicSymbols() {
  if (did_load_dynamic_symbols_ && dynsym_.IsValid()) {
    return true;
  }

  did_load_dynamic_symbols_ = true;

  LoadProgramHeaders();

  for (size_t idx = 0; idx < segments_.size(); idx++) {
    if (segments_[idx].p_type != PT_DYNAMIC) {
      continue;
    }

    auto data = GetSegmentData(idx);

    if (!data.ptr) {
      return false;
    }

    if (Is64Bit()) {
      return LoadDynamicSymbolsForElf<Elf64_Dyn>(data);
    } else {
      return LoadDynamicSymbolsForElf<Elf32_Dyn>(data);
    }
  }

  return false;
}

// A PltEntryBuffer is a PLT region - a sequence of PLT entries - with a mark
// somewhere in it.  The mark indicates the position of the next element to be
// read or written.
class ElfLib::PltEntryBuffer {
 public:
  PltEntryBuffer(ElfLib* lib)
      : ptr_(nullptr), end_(nullptr), start_(nullptr), size_(0), lib_(lib) {}
  void SetRegion(const uint8_t* ptr, size_t size) {
    size_ = size;
    end_ = ptr + size;
    start_ = ptr + (IgnoredEntryCount() * EntrySize());
    ptr_ = start_;
  }

  // The number of bogus PLT entries at the beginning of the buffer.
  virtual int IgnoredEntryCount() const = 0;

  // The number of the element at this position.
  virtual uint32_t MarkIndex() const = 0;

  // Ensures that the PLT entry at the current mark is valid.
  virtual bool VerifyAtMark() const = 0;

  // Returns a value indicating whether the mark is currently in the region.
  bool MarkInBound() const { return ptr_ >= start_ && ptr_ + EntrySize() <= end_; }

  // Move the mark to the next position.  Also increments the index by one.
  virtual void IncrementMark() { ptr_ += EntrySize(); }

  // The size of a PLT entry in this buffer.
  virtual size_t EntrySize() const = 0;

 protected:
  const uint8_t* ptr_;
  const uint8_t* end_;
  const uint8_t* start_;
  size_t size_;
  ElfLib* lib_;
};

class PltEntryBufferX86 : public ElfLib::PltEntryBuffer {
 public:
  explicit PltEntryBufferX86(ElfLib* lib) : PltEntryBuffer(lib) {}
  virtual uint32_t MarkIndex() const override { return GetPltPtr()->index; }
  virtual bool VerifyAtMark() const override {
    if (GetPltPtr()->push_opcode != 0x68) {
      lib_->Warn("Push OpCode not found where expected in PLT.");
      return false;
    }
    return true;
  }

  virtual size_t EntrySize() const override { return sizeof(PltEntryX86); }

  virtual int IgnoredEntryCount() const override { return 1; }

 private:
  // A PLT entry consists of 3 x86 instructions: a jump using a 6-byte
  // encoding, a push of one 32 bit value on to the stack, and another jump,
  // this one using a 5-byte encoding.
  //
  // We don't care about either of the jumps, but we want the value that is
  // pushed as it is the index into the relocation table which will tell us
  // what symbol this entry is for.
  struct PltEntryX86 {
    char first_jump[6];
    char push_opcode;
    uint32_t index;
    char second_jump[5];
  } __attribute__((packed, aligned(1)));

  static_assert(sizeof(PltEntryX86) == 16);

  const PltEntryX86* GetPltPtr() const { return reinterpret_cast<const PltEntryX86*>(ptr_); }
};

class PltEntryBufferArm : public ElfLib::PltEntryBuffer {
 public:
  explicit PltEntryBufferArm(ElfLib* lib) : PltEntryBuffer(lib) {}
  virtual uint32_t MarkIndex() const override { return index_; }
  virtual bool VerifyAtMark() const override {
    // Prefix for adrp is 1??10000.
    if ((GetPltPtr()->adrp & 0x9F000000) != 0x90000000) {
      lib_->Warn("adrp OpCode not found where expected in PLT.");
      return false;
    }
    // Prefix for ldr immediate is 1?11100101
    if ((GetPltPtr()->ldr & 0xBFC00000) != 0xB9400000) {
      lib_->Warn("ldr OpCode not found where expected in PLT.");
      return false;
    }
    // Prefix for 64-bit add immediate is 10010001
    if ((GetPltPtr()->add & 0xFF800000) != 0x91000000) {
      lib_->Warn("add OpCode not found where expected in PLT.");
      return false;
    }
    // Prefix for br is 1101_0110_0001_1111_0000_00??_???0_0000
    if ((GetPltPtr()->br & 0xFFFFFC1F) != 0xD61F0000) {
      lib_->Warn("br OpCode not found where expected in PLT.");
      return false;
    }
    return true;
  }

  void IncrementMark() override {
    PltEntryBuffer::IncrementMark();
    ++index_;
  }

  virtual size_t EntrySize() const override { return sizeof(PltEntryArm); }

  virtual int IgnoredEntryCount() const override { return 2; }

 private:
  // A PLT entry consists of 4 ARM instructions: an adrp that gives the page
  // containing the real location of the link target, a ldr giving the offset on
  // the page, an add to add the two, and a br that takes you to the loaded
  // target.
  //
  // We need the target at the offset of the LDR as it is the index into the
  // relocation table which will tell us what symbol this entry is for.
  struct PltEntryArm {
    uint32_t adrp;
    uint32_t ldr;
    uint32_t add;
    uint32_t br;
  } __attribute__((packed, aligned(1)));

  static_assert(sizeof(PltEntryArm) == 16);

  const PltEntryArm* GetPltPtr() const { return reinterpret_cast<const PltEntryArm*>(ptr_); }

  uint32_t index_ = 0;
};

std::map<std::string, uint64_t> ElfLib::GetPLTOffsets() {
  // We assume Fuchsia's defaults for each architecture. We could perhaps check
  // ELF_OSABI to firm up those assumptions. Fuchsia sets it to NONE.
  switch (header_.e_machine) {
    case EM_X86_64: {
      PltEntryBufferX86 pos(this);
      return GetPLTOffsetsCommon(pos);
    }
    case EM_AARCH64: {
      PltEntryBufferArm pos(this);
      return GetPLTOffsetsCommon(pos);
    }
    default:
      Warn("Architecture doesn't support GetPLTOffsets.");
      return {};
  }
}

std::map<std::string, uint64_t> ElfLib::GetPLTOffsetsCommon(PltEntryBuffer& buffer) {
  // We'd prefer if this works but we can get by without it, so we're not
  // checking the return value.
  LoadDynamicSymbols();

  if (!LoadSectionNames()) {
    return {};
  }

  if (!dynamic_plt_use_rela_) {
    Warn("Assuming Elf64_Rela PLT relocation format.");
    dynamic_plt_use_rela_ = true;
  }

  auto plt_section = section_names_.find(".plt");

  if (plt_section == section_names_.end()) {
    return {};
  }

  auto plt_idx = plt_section->second;

  auto plt_shdr = GetSectionHeader(plt_idx);
  auto plt_memory = GetSectionData(plt_idx);

  if (!plt_shdr || !plt_memory.ptr) {
    return {};
  }

  auto plt_load_addr = plt_shdr->sh_addr;

  buffer.SetRegion(plt_memory.ptr, plt_memory.size);

  auto reloc_memory = GetSectionData(*dynamic_plt_use_rela_ ? ".rela.plt" : ".rel.plt");

  if (!reloc_memory.ptr) {
    return {};
  }

  const Elf64_Rela* reloc_rela = nullptr;
  const Elf64_Rel* reloc_rel = nullptr;
  size_t reloc_count = reloc_memory.size;
  if (*dynamic_plt_use_rela_) {
    reloc_rela = reinterpret_cast<const Elf64_Rela*>(reloc_memory.ptr);
    reloc_count /= sizeof(Elf64_Rela);
  } else {
    reloc_rel = reinterpret_cast<const Elf64_Rel*>(reloc_memory.ptr);
    reloc_count /= sizeof(Elf64_Rel);
  }

  ElfLib::MemoryRegion dynsym_mem = GetSectionData(".dynsym");

  if (!dynsym_mem.ptr) {
    return {};
  }

  auto symtab = reinterpret_cast<const Elf64_Sym*>(dynsym_mem.ptr);
  auto sym_count = dynsym_mem.size / sizeof(Elf64_Sym);

  ElfLib::MemoryRegion dynstr_mem = GetSectionData(".dynstr");

  if (!dynstr_mem.ptr) {
    return {};
  }

  uint64_t idx = buffer.IgnoredEntryCount();

  std::map<std::string, uint64_t> ret;

  for (; buffer.MarkInBound(); buffer.IncrementMark(), idx++) {
    if (!buffer.VerifyAtMark()) {
      continue;
    }

    uint32_t index = buffer.MarkIndex();
    if (index >= reloc_count) {
      Warn("PLT referenced reloc outside reloc table.");
      continue;
    }

    auto sym_idx = reloc_rela ? reloc_rela[index].getSymbol() : reloc_rel[index].getSymbol();

    if (sym_idx >= sym_count) {
      Warn("PLT reloc referenced symbol outside symbol table.");
      continue;
    }

    auto name = GetNullTerminatedStringAt(dynstr_mem.ptr, dynstr_mem.size, symtab[sym_idx].st_name);

    if (!name.size()) {
      Warn("PLT symbol name could not be retrieved.");
      continue;
    }

    ret[name] = idx * buffer.EntrySize() + plt_load_addr;
  }

  return ret;
}

std::optional<std::string> ElfLib::GetDynamicString(size_t offset) {
  if (!LoadDynamicSymbols() || !dynstr_.IsValid()) {
    return std::nullopt;
  }

  auto data = memory_->GetMemory(*dynstr_.offset, *dynstr_.size);

  if (!data) {
    return std::nullopt;
  }

  return GetNullTerminatedStringAt(data, *dynstr_.size, offset);
}

std::optional<std::string> ElfLib::GetString(size_t offset) {
  auto string_data = GetSectionData(".strtab");

  if (!string_data.ptr) {
    return std::nullopt;
  }

  return GetNullTerminatedStringAt(string_data.ptr, string_data.size, offset);
}

ElfLib::MemoryRegion ElfLib::GetSymtab() { return GetSectionData(".symtab"); }

ElfLib::MemoryRegion ElfLib::GetDynamicSymtab() {
  if (!LoadDynamicSymbols()) {
    return {};
  }

  if (!dynsym_.IsValid()) {
    return {};
  }

  size_t symtab_entry_size = 0;
  if (Is64Bit()) {
    symtab_entry_size = sizeof(Elf64_Sym);
  } else {
    symtab_entry_size = sizeof(Elf32_Sym);
  }
  auto memory = memory_->GetMemory(*dynsym_.offset, *dynsym_.size * symtab_entry_size);

  return {.ptr = memory, .size = *dynsym_.size * symtab_entry_size};
}

std::optional<Elf64_Sym> ElfLib::GetSymbol(const std::string& name) {
  return GetSymbolFromTable(header_, name, GetSymtab(),
                            [this](uint64_t idx) { return GetString(idx); });
}

std::optional<Elf64_Sym> ElfLib::GetDynamicSymbol(const std::string& name) {
  return GetSymbolFromTable(header_, name, GetDynamicSymtab(),
                            [this](uint64_t idx) { return GetDynamicString(idx); });
}

std::optional<std::map<std::string, Elf64_Sym>> ElfLib::GetAllSymbols() {
  return SymtabToMap(header_, GetSymtab(), GetSectionData(".strtab"));
}

std::optional<std::map<std::string, Elf64_Sym>> ElfLib::GetAllDynamicSymbols() {
  if (!LoadDynamicSymbols() || !dynstr_.IsValid()) {
    return std::nullopt;
  }

  return SymtabToMap(header_, GetDynamicSymtab(),
                     ElfLib::MemoryRegion{.ptr = memory_->GetMemory(*dynstr_.offset, *dynstr_.size),
                                          .size = *dynstr_.size});
}

std::optional<std::vector<std::string>> ElfLib::GetSharedObjectDependencies() {
  LoadProgramHeaders();

  std::vector<std::string> output;

  for (size_t idx = 0; idx < segments_.size(); idx++) {
    if (segments_[idx].p_type != PT_DYNAMIC) {
      continue;
    }

    auto data = GetSegmentData(idx);

    if (!data.ptr) {
      return std::nullopt;
    }

    const Elf64_Dyn* start = reinterpret_cast<const Elf64_Dyn*>(data.ptr);
    const Elf64_Dyn* end = start + (data.size / sizeof(Elf64_Dyn));

    for (auto dyn = start; dyn != end; dyn++) {
      if (dyn->d_tag == DT_NEEDED) {
        auto string = GetDynamicString(dyn->d_un.d_val);
        if (!string) {
          Warn("Invalid symbol table string");
          continue;
        }
        output.push_back(std::move(*string));
      }
    }
  }

  return output;
}

bool ElfLib::ProbeHasDebugInfo() {
  if (!header_.e_shnum) {
    // No sections, no debug info.
    return false;
  }

  LoadSectionNames();

  for (const auto& [name, _] : section_names_) {
    if (name == ".debug_info") {
      return true;
    }
  }

  return false;
}

bool ElfLib::ProbeHasProgramBits() {
  if (!header_.e_shnum) {
    // No sections, so either this is *just* the text, or it's a very broken
    // file.
    return true;
  }

  for (size_t i = 0; i < header_.e_shnum; i++) {
    if (auto section = GetSectionHeader(i)) {
      if (section->sh_type == SHT_DYNAMIC) {
        // If the program data is present, this section will be here. It
        // becomes a NOBITS section in the split debug info.
        return true;
      }
    }
  }

  return false;
}

uint64_t ElfLib::MappedAddressToOffset(uint64_t mapped_address) {
  if (address_mode_ == AddressMode::kProcess) {
    // Does it looks like an offset?
    for (const auto& segment : GetSegmentHeaders()) {
      if (mapped_address >= segment.p_vaddr && mapped_address < segment.p_vaddr + segment.p_memsz) {
        return mapped_address;
      }
    }
    if (mapped_address >= load_address_) {
      return mapped_address - load_address_;
    }
  } else {
    for (const auto& segment : GetSegmentHeaders()) {
      if (mapped_address >= segment.p_vaddr && mapped_address < segment.p_vaddr + segment.p_memsz) {
        return mapped_address - segment.p_vaddr + segment.p_offset;
      }
    }
  }

  // No segment covering the mapped_address. Return an invalid offset that fails GetMemory().
  return -1;
}

}  // namespace elflib
