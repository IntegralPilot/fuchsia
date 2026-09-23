// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree.h>
#include <lib/fit/defer.h>

#include <cstdio>
#include <cstring>
#include <functional>
#include <span>

#include <fbl/alloc_checker.h>
#include <fbl/intrusive_hash_table.h>
#include <fbl/intrusive_single_list.h>

namespace boot_shim {
namespace {

// It's okay to use raw pointers here and not ever free them individually
// because the expected model is that the shim's .set_allocator was called with
// an allocation function that provides doing any necessary cleanup en masse
// after the shim object is destroyed.
struct HashableIsaString
    : public fbl::SinglyLinkedListable<HashableIsaString*, fbl::NodeOptions::AllowClearUnsafe> {
  // Required to instantiate fbl::DefaultKeyedObjectTraits.
  std::string_view GetKey() const { return value; }

  // Required to instantiate fbl::DefaultHashTraits.
  static size_t GetHash(std::string_view isa_string) {
    return std::hash<std::string_view>{}(isa_string);
  }

  std::string_view value;
  size_t strtab_index = 0;
};

}  // namespace

void RiscvDevicetreeCpuTopologyItemBase::OnDone() {
  // Finalizes cpu_entries() and sorts it by hart ID.
  DevicetreeCpuTopologyItem::OnDone();

  fbl::HashTable<std::string_view, HashableIsaString*> isa_strings;
  auto cleanup = fit::defer([&isa_strings]() { isa_strings.clear_unsafe(); });

  fbl::AllocChecker ac;

  // First we build up the size of table and record all mappings into it.
  size_t isa_strtab_size = 1;  // Account for the initial NUL entry.
  for (const CpuEntry& cpu : cpu_entries()) {
    devicetree::PropertyDecoder decoder(cpu.properties);
    std::optional<uint32_t> reg =
        decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsUint32>("reg");
    ZX_DEBUG_ASSERT(reg);  // Validated in the arch info checker.
    auto isa_string =
        decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsString>("riscv,isa");
    ZX_DEBUG_ASSERT(isa_string);  // Validated in the arch info checker.

    std::optional<uint32_t> cbom =
        decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsUint32>(
            "riscv,cbom-block-size");
    std::optional<uint32_t> cboz =
        decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsUint32>(
            "riscv,cboz-block-size");

    std::string_view formatted_isa = *isa_string;
    if (cbom || cboz) {
      size_t max_len = isa_string->size() + 25;
      char* buf = Allocate<char>(max_len, ac).data();
      if (!ac.check()) {
        return;
      }
      char* ptr = buf;
      size_t len = isa_string->size();
      memcpy(ptr, isa_string->data(), len);
      ptr += len;

      size_t remaining = max_len - len;
      int written = 0;
      if (cbom) {
        written = snprintf(ptr, remaining, "_zicbom%" PRIu32, *cbom);
        ptr += written;
        remaining -= static_cast<size_t>(written);
      }
      if (cboz) {
        written = snprintf(ptr, remaining, "_zicboz%" PRIu32, *cboz);
        ptr += written;
        remaining -= static_cast<size_t>(written);
      }
      formatted_isa = std::string_view(buf, static_cast<size_t>(ptr - buf));
    }

    auto map_hart_to_index = [this, &ac](uint32_t id, size_t index) {
      auto* hashable = Allocate<IsaStrtabIndex>(ac);
      if (!ac.check()) {
        return false;
      }
      hashable->hart_id = id;
      hashable->strtab_index = index;
      id_to_index_.insert(hashable);
      return true;
    };

    if (auto it = isa_strings.find(formatted_isa); it != isa_strings.end()) {
      if (!map_hart_to_index(*reg, it->strtab_index)) {
        OnError("Failed to allocate hart ID to ISA string hash map entry");
        return;
      }
      continue;
    }
    auto* hashable = Allocate<HashableIsaString>(ac);
    if (!ac.check()) {
      return;
    }
    hashable->value = formatted_isa;
    hashable->strtab_index = isa_strtab_size;
    isa_strings.insert(hashable);
    map_hart_to_index(*reg, isa_strtab_size);
    isa_strtab_size += formatted_isa.size() + 1;  // +1 for NUL.
  }

  // Now that we know the size and would-be contents of the table, we can build
  // it up.
  isa_strtab_ = Allocate<char>(isa_strtab_size, ac);
  if (!ac.check()) {
    return;
  }
  isa_strtab_[0] = '\0';  // Initial NUL entry.
  for (const HashableIsaString& str : isa_strings) {
    char* dest = &isa_strtab_[str.strtab_index];
    [[maybe_unused]] size_t copied = str.value.copy(dest, isa_strtab_size - str.strtab_index);
    ZX_DEBUG_ASSERT(copied == str.value.size());
    ZX_DEBUG_ASSERT(&dest[str.value.size()] < isa_strtab_.data() + isa_strtab_size);
    dest[str.value.size()] = '\0';
  }
  IsaStrtabItem::set_payload(std::as_bytes(isa_strtab_));
}

}  // namespace boot_shim
