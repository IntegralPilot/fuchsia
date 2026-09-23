// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim-utils/commandline-bootloader-file-item.h>
#include <stdio.h>
#include <string.h>

#include <limits>
#include <ranges>

namespace {

constexpr size_t kBase64DecodeError = static_cast<size_t>(-1);

size_t Base64Decode(const char* src, size_t len, char* dst) {
  if (len % 4 != 0) {
    return kBase64DecodeError;
  }

  auto decode_char = [](char c) -> int {
    if (c >= 'A' && c <= 'Z')
      return c - 'A';
    if (c >= 'a' && c <= 'z')
      return c - 'a' + 26;
    if (c >= '0' && c <= '9')
      return c - '0' + 52;
    if (c == '+')
      return 62;
    if (c == '/')
      return 63;
    if (c == '=')
      return 0;
    return -1;
  };

  char* p = dst;
  for (size_t i = 0; i < len; i += 4) {
    if (src[i] == '=' || src[i + 1] == '=') {
      return kBase64DecodeError;
    }
    if (src[i + 2] == '=' && src[i + 3] != '=') {
      return kBase64DecodeError;
    }

    int c0 = decode_char(src[i]);
    int c1 = decode_char(src[i + 1]);
    int c2 = decode_char(src[i + 2]);
    int c3 = decode_char(src[i + 3]);

    if (c0 < 0 || c1 < 0 || c2 < 0 || c3 < 0) {
      return kBase64DecodeError;
    }

    size_t pad = 0;
    if (src[i + 3] == '=') {
      pad++;
      if (src[i + 2] == '=') {
        pad++;
      }
    }

    if (pad > 0 && (i + 4 < len)) {
      return kBase64DecodeError;
    }

    uint32_t val = (static_cast<uint32_t>(c0) << 18) | (static_cast<uint32_t>(c1) << 12) |
                   (static_cast<uint32_t>(c2) << 6) | static_cast<uint32_t>(c3);

    *p++ = static_cast<char>((val >> 16) & 0xFF);
    if (pad < 2) {
      *p++ = static_cast<char>((val >> 8) & 0xFF);
    }
    if (pad < 1) {
      *p++ = static_cast<char>(val & 0xFF);
    }
  }

  return static_cast<size_t>(p - dst);
}

// Executes `callback` on each `cmdline` arg starting with `prefix`.
template <typename Callback>
void ForEachChunk(std::string_view cmdline, std::string_view prefix, Callback&& callback) {
  // Don't bother with quoting or complex cases, we don't expect the bootloader to be passing
  // anything that won't work with simple space delimiters.
  for (const auto range : std::views::split(cmdline, std::string_view(" "))) {
    std::string_view arg(range.begin(), range.end());
    if (arg.starts_with(prefix)) {
      callback(arg.substr(prefix.length()));
    }
  }
}

// Returns the `ZBI_TYPE_BOOTLOADER_FILE` payload size, or error if it won't fit.
fit::result<CommandlineBootloaderFileItem::DataZbi::Error, uint32_t> PayloadSize(
    std::string_view filename, size_t content_size) {
  // Filename length must fit in a single byte.
  if (filename.size() > std::numeric_limits<uint8_t>::max()) {
    return fit::error(CommandlineBootloaderFileItem::DataZbi::Error{
        .zbi_error = "Bootloader file name overflow",
        .item_offset = 0,
    });
  }

  // ZBI item payload length must fit in a U32.
  size_t payload_size = 1 + filename.size() + content_size;
  if (payload_size <= content_size || payload_size > std::numeric_limits<uint32_t>::max()) {
    return fit::error(CommandlineBootloaderFileItem::DataZbi::Error{
        .zbi_error = "Bootloader file size overflow",
        .item_offset = 0,
    });
  }

  return fit::ok(static_cast<uint32_t>(payload_size));
}

}  // namespace

void CommandlineBootloaderFileItem::Init(std::string_view cmdline, std::string_view prefix,
                                         std::string_view filename) {
  cmdline_ = cmdline;
  prefix_ = prefix;
  filename_ = filename;

  // Pre-calculate Base64 length since we'll need this a few times.
  base64_size_ = 0;
  ForEachChunk(cmdline_, prefix_, [&](std::string_view chunk) { base64_size_ += chunk.size(); });
}

size_t CommandlineBootloaderFileItem::size_bytes() const {
  if (base64_size_ == 0) {
    return 0;
  }
  // This function can't fail so just return 0 (no ZBI space allocation) on error; we'll report the
  // actual error later in `AppendItems()`.
  auto res = PayloadSize(filename_, base64_size_);
  return res.is_ok() ? ItemSize(res.value()) : 0;
}

fit::result<CommandlineBootloaderFileItem::DataZbi::Error>
CommandlineBootloaderFileItem::AppendItems(DataZbi& zbi) const {
  if (base64_size_ == 0) {
    return fit::ok();
  }

  auto base64_payload_size = PayloadSize(filename_, base64_size_);
  if (base64_payload_size.is_error()) {
    return base64_payload_size.take_error();
  }

  auto zbi_item = zbi.Append({
      .type = ZBI_TYPE_BOOTLOADER_FILE,
      // Request enough buffer for the filename header plus full Base64 capacity; we will shrink
      // the final item length after decoding the data.
      .length = *base64_payload_size,
      .extra = 0,
      .flags = 0,
      .magic = ZBI_ITEM_MAGIC,
  });
  if (zbi_item.is_error()) {
    return zbi_item.take_error();
  }

  // Write the header: filename length (1 byte) and filename.
  zbi_item->payload[0] = static_cast<std::byte>(filename_.size());
  memcpy(zbi_item->payload.data() + 1, filename_.data(), filename_.size());

  // Copy the Base64 data after the header.
  char* base64_buffer = reinterpret_cast<char*>(zbi_item->payload.data()) + 1 + filename_.size();
  size_t offset = 0;
  ForEachChunk(cmdline_, prefix_, [&](std::string_view chunk) {
    memcpy(base64_buffer + offset, chunk.data(), chunk.size());
    offset += chunk.size();
  });

  // Decode Base64 in-place. Since decoded data is always smaller, this only overwrites data which
  // has already been read so is safe to do.
  size_t decoded_size = Base64Decode(base64_buffer, base64_size_, base64_buffer);
  if (decoded_size == kBase64DecodeError) {
    return fit::error(DataZbi::Error{
        .zbi_error = "Invalid Base64 in commandline bootloader file chunks",
        .item_offset = 0,
    });
  }

  printf("commandline-bootloader-file-item: registering bootloader file '%.*s' (%zu bytes)\n",
         static_cast<int>(filename_.size()), filename_.data(), decoded_size);

  fit::result final_payload_size = PayloadSize(filename_, decoded_size);
  if (final_payload_size.is_error()) {
    return final_payload_size.take_error();
  }

  // Resize the item to only contain the decoded data size.
  if (auto trim_res = zbi.TrimLastItem(*zbi_item, *final_payload_size); trim_res.is_error()) {
    return trim_res.take_error();
  }

  return fit::ok();
}
