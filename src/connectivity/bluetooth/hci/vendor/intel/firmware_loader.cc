// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "firmware_loader.h"

#include <endian.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/status.h>

#include <cinttypes>

#include <fbl/string_printf.h>
#include <fbl/unique_fd.h>

#include "logging.h"

namespace bt_hci_intel {

namespace {

struct {
  size_t css_header_offset;
  size_t css_header_size;
  size_t pki_offset;
  size_t pki_size;
  size_t sig_offset;
  size_t sig_size;
  size_t write_offset;
} sec_boot_params[] = {
    // RSA
    {
        .css_header_offset = 0,
        .css_header_size = 128,
        .pki_offset = 128,
        .pki_size = 256,
        .sig_offset = 388,
        .sig_size = 256,
        .write_offset = 964,
    },
    // ECDSA
    {
        .css_header_offset = 644,
        .css_header_size = 128,
        .pki_offset = 772,
        .pki_size = 96,
        .sig_offset = 868,
        .sig_size = 96,
        .write_offset = 964,
    },
};

}  // anonymous namespace

FirmwareLoader::LoadStatus FirmwareLoader::LoadBseq(const void* firmware, const size_t& len) {
  cpp20::span<const uint8_t> file(static_cast<const uint8_t*>(firmware), len);

  size_t offset = 0;
  bool patched = false;

  if (file.size() < pw::bluetooth::emboss::CommandHeader::IntrinsicSizeInBytes()) {
    errorf("FirmwareLoader: Error: BSEQ too small: {} < {}", len,
           pw::bluetooth::emboss::CommandHeader::IntrinsicSizeInBytes());
    return LoadStatus::kError;
  }

  // A bseq file consists of a sequence of:
  // - [0x01] [command w/params]
  // - [0x02] [expected event w/params]
  while (file.size() - offset > pw::bluetooth::emboss::CommandHeader::IntrinsicSizeInBytes()) {
    // Parse the next items
    if (file[offset] != 0x01) {
      errorf("FirmwareLoader: Error: expected command packet");
      return LoadStatus::kError;
    }
    offset++;
    cpp20::span<const uint8_t> command_view = file.subspan(offset);
    auto command_header = pw::bluetooth::emboss::MakeCommandHeaderView(&command_view);
    const size_t cmd_size = pw::bluetooth::emboss::CommandHeader::IntrinsicSizeInBytes() +
                            command_header.parameter_total_size().Read();
    command_view = command_view.subspan(0, cmd_size);
    offset += cmd_size;
    if (!patched && command_header.opcode_bits().ogf().Read() == kVendorOgf &&
        command_header.opcode_bits().ocf().Read() == kLoadPatchOcf) {
      patched = true;
    }
    if ((file.size() - offset <= pw::bluetooth::emboss::EventHeader::IntrinsicSizeInBytes()) ||
        (file[offset] != 0x02)) {
      errorf("FirmwareLoader: Error: expected event packet");
      return LoadStatus::kError;
    }
    std::deque<cpp20::span<const uint8_t>> events;
    while ((file.size() - offset > pw::bluetooth::emboss::EventHeader::IntrinsicSizeInBytes()) &&
           (file[offset] == 0x02)) {
      offset++;
      cpp20::span<const uint8_t> event_span = file.subspan(offset);
      auto event_view = pw::bluetooth::emboss::MakeEventHeaderView(&event_span);
      size_t event_size = pw::bluetooth::emboss::EventHeader::IntrinsicSizeInBytes() +
                          event_view.parameter_total_size().Read();
      events.emplace_back(file.subspan(offset, event_size));
      offset += event_size;
    }

    if (!hci_.SendAndExpect(command_view, std::move(events))) {
      return LoadStatus::kError;
    }
  }

  return patched ? LoadStatus::kPatched : LoadStatus::kComplete;
}

constexpr uint16_t kWriteBootParamsOcf = 0x000e;

FirmwareLoader::LoadStatus FirmwareLoader::LoadSfi(const void* firmware, const size_t& len,
                                                   enum SecureBootEngineType engine_type,
                                                   uint32_t* boot_addr) {
  cpp20::span<const uint8_t> file(static_cast<const uint8_t*>(firmware), len);

  // index to access the 'sec_boot_params[]'.
  size_t idx = (engine_type == SecureBootEngineType::kECDSA) ? 1 : 0;

  size_t min_fw_size = sec_boot_params[idx].write_offset;
  if (file.size() < min_fw_size) {
    errorf("FirmwareLoader: SFI is too small: {} < {}", file.size(), min_fw_size);
    return LoadStatus::kError;
  }

  // SFI File format:
  // [128 bytes CSS Header]
  if (!hci_.SendSecureSend(0x00, file.subspan(sec_boot_params[idx].css_header_offset,
                                              sec_boot_params[idx].css_header_size))) {
    errorf("FirmwareLoader: Failed sending CSS Header!");
    return LoadStatus::kError;
  }

  // [256 bytes PKI]
  if (!hci_.SendSecureSend(
          0x03, file.subspan(sec_boot_params[idx].pki_offset, sec_boot_params[idx].pki_size))) {
    errorf("FirmwareLoader: Failed sending PKI Header!");
    return LoadStatus::kError;
  }

  // [256 bytes signature info]
  if (!hci_.SendSecureSend(
          0x02, file.subspan(sec_boot_params[idx].sig_offset, sec_boot_params[idx].sig_size))) {
    errorf("FirmwareLoader: Failed sending signature Header!");
    return LoadStatus::kError;
  }

  size_t offset = sec_boot_params[idx].write_offset;
  size_t frag_len = 0;
  // [N bytes of command packets, arranged so that the "Secure send" command
  // param size can be a multiple of 4 bytes]
  while (offset < file.size()) {
    cpp20::span<const uint8_t> next_cmd = file.subspan(offset + frag_len);
    auto hdr_view = pw::bluetooth::emboss::MakeCommandHeaderView(&next_cmd);
    size_t cmd_size = pw::bluetooth::emboss::CommandHeader::IntrinsicSizeInBytes() +
                      hdr_view.parameter_total_size().Read();
    if (hdr_view.opcode_bits().ogf().Read() == kVendorOgf &&
        hdr_view.opcode_bits().ocf().Read() == kWriteBootParamsOcf) {
      next_cmd = next_cmd.subspan(0, cmd_size);
      auto params = MakeWriteBootParamsCommandView(&next_cmd);
      if (!params.IsComplete()) {
        errorf("FirmwareLoader: Write boot params command is too small ({} , expected: {})",
               next_cmd.size(), WriteBootParamsCommand::IntrinsicSizeInBytes());
        return LoadStatus::kError;
      }
      if (boot_addr != nullptr) {
        *boot_addr = params.boot_address().Read();
      }
      infof("FirmwareLoader: Loading fw {} ww {} yy {} - boot addr {:#x}",
            params.firmware_build_number().Read(), params.firmware_build_ww().Read(),
            params.firmware_build_yy().Read(), params.boot_address().Read());
    }
    frag_len += cmd_size;
    if ((frag_len % 4) == 0) {
      if (!hci_.SendSecureSend(0x01, file.subspan(offset, frag_len))) {
        errorf("Failed sending a command chunk!");
        return LoadStatus::kError;
      }
      offset += frag_len;
      frag_len = 0;
    }
  }

  return LoadStatus::kComplete;
}

}  // namespace bt_hci_intel
