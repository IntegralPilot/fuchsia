// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "mock_boot_service.h"

#include <lib/cksum.h>

#include <span>

#include "efi/protocol/managed-network.h"
#include "efi/protocol/simple-network.h"
#include "efi/system-table.h"
#include "src/lib/utf_conversion/utf_conversion.h"

efi_loaded_image_protocol* gEfiLoadedImage = nullptr;
efi_system_table* gEfiSystemTable = nullptr;
efi_handle gEfiImageHandle;

constexpr uint32_t kGptHeaderRevision = 0x00010000;

namespace gigaboot {

namespace {

void RecalculateGptCrcs(std::span<const gpt_entry_t> entries, gpt_header_t* header) {
  header->entries_crc =
      crc32(0, reinterpret_cast<uint8_t const*>(entries.data()), entries.size_bytes());

  header->crc32 = 0;
  header->crc32 = crc32(0, reinterpret_cast<uint8_t*>(header), sizeof(*header));
}

}  // namespace

// A helper that creates a realistic device path protocol
void Device::InitDevicePathProtocol(std::vector<std::string_view> path_nodes) {
  // UEFI specification chapter 10. `efi_device_path_protocol*` is an array of
  // variable length struct. Specifically, each element is
  // `efi_device_path_protocol struct` + path data.
  device_path_buffer_.clear();
  for (auto name : path_nodes) {
    uint16_t node_size = static_cast<uint16_t>(name.size()) + 4;
    device_path_buffer_.push_back(DEVICE_PATH_HARDWARE);
    device_path_buffer_.push_back(0);
    device_path_buffer_.push_back(node_size & 0xFF);
    device_path_buffer_.push_back(node_size >> 8);
    device_path_buffer_.insert(device_path_buffer_.end(),
                               reinterpret_cast<const uint8_t*>(name.data()),
                               reinterpret_cast<const uint8_t*>(name.data() + name.size()));
  }
  device_path_buffer_.push_back(DEVICE_PATH_END);
  device_path_buffer_.push_back(0);
  device_path_buffer_.push_back(4);
  device_path_buffer_.push_back(0);
}

ManagedNetworkDevice::ManagedNetworkDevice(std::vector<std::string_view> paths,
                                           efi_simple_network_mode state)
    : Device(std::move(paths)), mnp_(state) {}

BlockDevice::BlockDevice(std::vector<std::string_view> paths, size_t blocks)
    : Device(paths), total_blocks_(blocks) {
  memset(&block_io_media_, 0, sizeof(block_io_media_));
  block_io_media_.BlockSize = kBlockSize;
  block_io_media_.LastBlock = blocks - 1;
  block_io_media_.MediaPresent = true;
  block_io_protocol_ = {
      .Revision = 0,  // don't care
      .Media = &this->block_io_media_,
      .Reset = nullptr,        // don't care
      .ReadBlocks = nullptr,   // don't care
      .WriteBlocks = nullptr,  // don't care
      .FlushBlocks = nullptr,  // don't care
  };
  // Only support MediaId = 0. Allocate buffer to serve as block storage.
  fake_disk_io_protocol_.contents(/*MediaId=*/0) = std::vector<uint8_t>(blocks * kBlockSize);
}

void BlockDevice::InitializeGpt() {
  ASSERT_GT(total_blocks_, 2 * kGptHeaderBlocks + 1);
  // Entries start on a block
  uint8_t* start = fake_disk_io_protocol_.contents(0).data();

  gpt_header_t primary = {
      .magic = GPT_MAGIC,
      .revision = kGptHeaderRevision,
      .size = GPT_HEADER_SIZE,
      .crc32 = 0,
      .reserved0 = 0,
      .current = 1,
      .backup = total_blocks_ - 1,
      .first = kGptFirstUsableBlocks,
      .last = total_blocks_ - kGptHeaderBlocks - 1,
      .entries = 2,
      .entries_count = kGptEntries,
      .entries_size = GPT_ENTRY_SIZE,
      .entries_crc = 0,
  };

  gpt_header_t backup(primary);
  backup.backup = 1;
  backup.current = total_blocks_ - 1;
  backup.entries = total_blocks_ - kGptHeaderBlocks;

  primary.entries_crc =
      crc32(0, start + primary.entries * kBlockSize, primary.entries_size * primary.entries_count);
  backup.entries_crc = primary.entries_crc;

  primary.crc32 = crc32(0, reinterpret_cast<uint8_t*>(&primary), sizeof(primary));
  backup.crc32 = crc32(0, reinterpret_cast<uint8_t*>(&backup), sizeof(backup));

  // Copy over the primary header. Skip mbr partition.
  memcpy(start + kBlockSize, &primary, sizeof(primary));

  // Copy the backup header.
  memcpy(start + (backup.current * kBlockSize), &backup, sizeof(backup));

  // Initialize partition entries to 0s
  memset(start + primary.entries * kBlockSize, 0, kGptEntries * sizeof(gpt_entry_t));
}

void BlockDevice::AddGptPartition(const gpt_entry_t& new_entry) {
  ASSERT_GE(new_entry.first, kGptFirstUsableBlocks);
  ASSERT_LE(new_entry.last, total_blocks_ - kGptHeaderBlocks - 1);

  uint8_t* const data = fake_disk_io_protocol_.contents(0).data();

  gpt_header_t* primary_header = reinterpret_cast<gpt_header_t*>(data + kBlockSize);
  gpt_header_t* backup_header =
      reinterpret_cast<gpt_header_t*>(data + (total_blocks_ - 1) * kBlockSize);
  gpt_entry_t* primary_entries = reinterpret_cast<gpt_entry_t*>(data + 2 * kBlockSize);
  gpt_entry_t* backup_entries =
      reinterpret_cast<gpt_entry_t*>(data + (total_blocks_ - kGptHeaderBlocks) * kBlockSize);
  std::span<const gpt_entry_t> entries_span(primary_entries, primary_header->entries_count);

  // Search for an empty entry
  for (size_t i = 0; i < kGptEntries; i++) {
    if (primary_entries[i].first == 0 && primary_entries[i].last == 0) {
      ASSERT_EQ(backup_entries[i].first, 0UL);
      ASSERT_EQ(backup_entries[i].last, 0UL);

      memcpy(&primary_entries[i], &new_entry, sizeof(new_entry));
      memcpy(&backup_entries[i], &new_entry, sizeof(new_entry));

      RecalculateGptCrcs(entries_span, primary_header);
      RecalculateGptCrcs(entries_span, backup_header);

      return;
    }
  }
  ASSERT_TRUE(false);
}

Tcg2Device::Tcg2Device() : Device({}) {
  tcg2_protocol_.protocol_ = {};
  tcg2_protocol_.protocol_.GetCapability = Tcg2Device::GetCapability;
  tcg2_protocol_.protocol_.SubmitCommand = Tcg2Device::SubmitCommand;
}

efi_status Tcg2Device::GetCapability(struct efi_tcg2_protocol*,
                                     efi_tcg2_boot_service_capability* out) {
  *out = {};
  return EFI_SUCCESS;
}

efi_status Tcg2Device::SubmitCommand(struct efi_tcg2_protocol* protocol, uint32_t block_size,
                                     uint8_t* block_data, uint32_t output_size,
                                     uint8_t* output_data) {
  Tcg2Device::Protocol* protocol_data_ = reinterpret_cast<Tcg2Device::Protocol*>(protocol);
  protocol_data_->last_command_ = std::vector<uint8_t>(block_data, block_data + block_size);
  return EFI_SUCCESS;
}

GraphicsOutputDevice::GraphicsOutputDevice() : Device({}) {
  protocol_.Mode = &mode_;
  mode_.Info = &info_;
}

efi_status MockStubService::LocateProtocol(const efi_guid* protocol, void* registration,
                                           void** intf) {
  if (IsProtocol<efi_tcg2_protocol>(*protocol)) {
    for (auto& ele : devices_) {
      if (auto protocol = ele->GetTcg2Protocol(); protocol) {
        *intf = protocol;
        return EFI_SUCCESS;
      }
    }
  } else if (IsProtocol<efi_graphics_output_protocol>(*protocol)) {
    for (auto& ele : devices_) {
      if (auto protocol = ele->GetGraphicsOutputProtocol(); protocol) {
        *intf = protocol;
        return EFI_SUCCESS;
      }
    }
  }

  return EFI_UNSUPPORTED;
}

efi_status MockStubService::LocateHandleBuffer(efi_locate_search_type search_type,
                                               const efi_guid* protocol, void* search_key,
                                               size_t* num_handles, efi_handle** buf) {
  // We'll only ever use ByProtocol search type.
  if (search_type != ByProtocol) {
    return EFI_UNSUPPORTED;
  }

  bool (*search_func)(Device* d) = nullptr;
  if (IsProtocol<efi_block_io_protocol>(*protocol)) {
    search_func = [](Device* d) { return d->GetBlockIoProtocol() != nullptr; };
  } else if (IsProtocol<efi_managed_network_protocol>(*protocol)) {
    search_func = [](Device* d) { return d->GetManagedNetworkProtocol() != nullptr; };
  } else {
    return EFI_UNSUPPORTED;
  }

  std::vector<Device*> lists;
  std::copy_if(devices_.cbegin(), devices_.cend(), std::back_inserter(lists), search_func);
  *num_handles = lists.size();
  size_t size_in_bytes = lists.size() * sizeof(decltype(lists)::value_type);
  void* buffer;
  efi_status status = gEfiSystemTable->BootServices->AllocatePool(EfiLoaderData /*don't care*/,
                                                                  size_in_bytes, &buffer);
  if (status != EFI_SUCCESS) {
    return status;
  }
  std::copy(lists.cbegin(), lists.cend(), reinterpret_cast<Device**>(buffer));
  *buf = reinterpret_cast<efi_handle*>(buffer);
  return EFI_SUCCESS;
}

efi_status MockStubService::OpenProtocol(efi_handle handle, const efi_guid* protocol, void** intf,
                                         efi_handle agent_handle, efi_handle controller_handle,
                                         uint32_t attributes) {
  // The given handle must be a pointer to one of the registered devices added to `devices_`.
  auto iter_find = std::find(devices_.begin(), devices_.end(), handle);
  if (iter_find == devices_.end()) {
    return EFI_NOT_FOUND;
  }

  if (IsProtocol<efi_device_path_protocol>(*protocol)) {
    *intf = (*iter_find)->GetDevicePathProtocol();
  } else if (IsProtocol<efi_block_io_protocol>(*protocol)) {
    *intf = (*iter_find)->GetBlockIoProtocol();
  } else if (IsProtocol<efi_disk_io_protocol>(*protocol)) {
    *intf = (*iter_find)->GetDiskIoProtocol();
  } else if (IsProtocol<efi_managed_network_protocol>(*protocol)) {
    *intf = (*iter_find)->GetManagedNetworkProtocol();
  }

  return *intf ? EFI_SUCCESS : EFI_UNSUPPORTED;
}

efi_status MockStubService::GetMemoryMap(size_t* memory_map_size, efi_memory_descriptor* memory_map,
                                         size_t* map_key, size_t* desc_size,
                                         uint32_t* desc_version) {
  *map_key = mkey_;
  *desc_version = 0;
  *desc_size = sizeof(efi_memory_descriptor);
  size_t total_size = memory_map_.size() * sizeof(efi_memory_descriptor);
  if (*memory_map_size < total_size) {
    return EFI_INVALID_PARAMETER;
  }

  *memory_map_size = total_size;
  if (total_size) {
    memcpy(memory_map, memory_map_.data(), total_size);
  }

  return EFI_SUCCESS;
}

efi_status MockStubService::CreateEvent(uint32_t type, efi_tpl notify_tpl,
                                        efi_event_notify notify_fn, void* notify_ctx,
                                        efi_event* event) {
  // Only deal with timers and signals for the time being.
  if (!(type & (EVT_TIMER | EVT_NOTIFY_SIGNAL)) || event == nullptr) {
    return EFI_INVALID_PARAMETER;
  }

  *event = reinterpret_cast<efi_event>(event_counter_++);
  return EFI_SUCCESS;
}

efi_status MockStubService::SetTimer(efi_event event, efi_timer_delay type, uint64_t trigger_time) {
  // Very quick and dirty check that this is a valid event.
  return (reinterpret_cast<uintptr_t>(event) < event_counter_) ? EFI_SUCCESS
                                                               : EFI_INVALID_PARAMETER;
}

efi_status MockStubService::CloseEvent(efi_event event) {
  // Very quick and dirty check that this is a valid event.
  return (reinterpret_cast<uintptr_t>(event) < event_counter_) ? EFI_SUCCESS
                                                               : EFI_INVALID_PARAMETER;
}

efi_status MockStubService::CheckEvent(efi_event event) {
  // Very quick and dirty check that this is a valid event.
  return (reinterpret_cast<uintptr_t>(event) < event_counter_) ? timer_retval_
                                                               : EFI_INVALID_PARAMETER;
}

void SetGptEntryName(const char* name, gpt_entry_t& entry) {
  size_t dst_len = sizeof(entry.name) / sizeof(uint16_t);
  utf8_to_utf16(reinterpret_cast<const uint8_t*>(name), strlen(name),
                reinterpret_cast<uint16_t*>(entry.name), &dst_len);
}

EfiConfigTable::EfiConfigTable(uint8_t acpi_revision, SmbiosRev smbios_revision) {
  rsdp_ = {
      .signature = kAcpiRsdpSignature,
      .checksum = 0,
      .revision = acpi_revision,
      .length = sizeof(rsdp_),
      .extended_checksum = 0,
  };

  // The checksum sums all the bytes in the rsdp struct.
  // It is valid if the sum is zero.
  std::span<const uint8_t> bytes(reinterpret_cast<const uint8_t*>(&rsdp_), kAcpiRsdpV1Size);
  rsdp_.checksum = static_cast<uint8_t>(0x100 - std::reduce(bytes.begin(), bytes.end(), 0));
  bytes = {bytes.begin(), rsdp_.length};
  rsdp_.extended_checksum =
      static_cast<uint8_t>(0x100 - std::reduce(bytes.begin(), bytes.end(), 0));

  efi_guid guid = ACPI_TABLE_GUID;
  if (acpi_revision >= 2) {
    guid = ACPI_20_TABLE_GUID;
  }

  // Make the table lookups iterate at least once.
  table_.push_back(efi_configuration_table{.VendorGuid = {}});

  table_.push_back(efi_configuration_table{
      .VendorGuid = guid,
      .VendorTable = &rsdp_,
  });

  switch (smbios_revision) {
    case SmbiosRev::kV3:
      table_.push_back(efi_configuration_table{
          .VendorGuid = SMBIOS3_TABLE_GUID,
          .VendorTable = "_SM3_",
      });
      break;
    case SmbiosRev::kV1:
      table_.push_back(efi_configuration_table{
          .VendorGuid = SMBIOS_TABLE_GUID,
          .VendorTable = "_SM_",
      });
      break;
    case SmbiosRev::kNone:
      // Deliberately omit on none
    default:
      break;
  }
}

const fbl::NoDestructor<EfiConfigTable> kDefaultEfiConfigTable(static_cast<uint8_t>(2),
                                                               EfiConfigTable::SmbiosRev::kV3);

std::vector<zbitl::ByteView> FindItems(const void* zbi, uint32_t type) {
  std::vector<zbitl::ByteView> ret;
  zbitl::View<zbitl::ByteView> view{
      zbitl::StorageFromRawHeader(static_cast<const zbi_header_t*>(zbi))};
  for (auto [header, payload] : view) {
    if (header->type == type) {
      ret.push_back(payload);
    }
  }

  ZX_ASSERT(view.take_error().is_ok());
  return ret;
}

}  // namespace gigaboot
