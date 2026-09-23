// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim-utils/devicetree-bootloader-files-item.h>

void DevicetreeBootloaderFilesItem::SetScratchBuffer(std::span<std::byte> buffer) {
  if (reinterpret_cast<size_t>(buffer.data()) % ZBI_ALIGNMENT) {
    fprintf(stdout,
            "Bootloader file buffer is not aligned to ZBI_ALIGNMENT. Files will not append.\n");
    return;
  }

  files_ = zbitl::Image<std::span<std::byte>>(buffer);
  if (auto result = files_.clear(); result.is_error()) {
    fprintf(stdout, "Failed to initialize ZBI files container: %s\n",
            result.error_value().zbi_error.data());
  }
}

devicetree::ScanState DevicetreeBootloaderFilesItem::OnNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  const char kRootPath[] = "/chosen/google/bootloader-files/";
  switch (path.CompareWith(kRootPath)) {
    // If this is in a parent node, continues searching the sub tree.
    case devicetree::NodePath::Comparison::kParent:
    case devicetree::NodePath::Comparison::kIndirectAncestor:
    case devicetree::NodePath::Comparison::kEqual:
      return devicetree::ScanState::kActive;
    // If this is in a different sub tree, don't visit it any further.
    case devicetree::NodePath::Comparison::kMismatch:
    case devicetree::NodePath::Comparison::kIndirectDescendent:
      return devicetree::ScanState::kDoneWithSubtree;
    // If this is a direct child, collects the ZBI bootloader file.
    case devicetree::NodePath::Comparison::kChild:
      break;
  };

  // Extracts file name.
  auto name = decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsString>("id");
  if (!name) {
    fprintf(stdout, "File has no `id` property. Skips.\n");
    return devicetree::ScanState::kActive;
  } else if (name->size() > std::numeric_limits<uint8_t>::max()) {
    fprintf(stdout, "Name length oveflows uint8_t max\n");
    return devicetree::ScanState::kActive;
  }

  // Extracts file payload.
  auto data = decoder.FindProperty("data");
  if (!data) {
    fprintf(stdout, "File has no `data` property. Skips.\n");
    return devicetree::ScanState::kActive;
  }
  auto payload = data->AsBytes();
  size_t total = 1 + name->size() + payload.size();
  if (total < payload.size()) {
    fprintf(stdout, "Payload size overflows. Skips.\n");
    return devicetree::ScanState::kActive;
  }
  // Collects the ZBI bootloader file.
  auto result = files_.Append({
      .type = ZBI_TYPE_BOOTLOADER_FILE,
      .length = static_cast<uint32_t>(total),
      .extra = 0,
      .flags = 0,
      .magic = ZBI_ITEM_MAGIC,
  });
  if (result.is_error()) {
    fprintf(stdout, "Failed to append ZBI bootloader file %s: %s\n", name->data(),
            result.error_value().zbi_error.data());
    return devicetree::ScanState::kActive;
  }

  // name->size() already checked to fit within uint8_t above.
  result->payload[0] = static_cast<std::byte>(name->size());
  memcpy(result->payload.data() + 1, name->data(), name->size());
  memcpy(result->payload.data() + 1 + name->size(), payload.data(), payload.size());

  return devicetree::ScanState::kActive;
}

fit::result<DevicetreeBootloaderFilesItem::DataZbi::Error>
DevicetreeBootloaderFilesItem::AppendItems(DataZbi& zbi) const {
  if (files_.storage().empty()) {
    return fit::ok();
  }

  auto zbi_view = zbitl::View(files_.storage());
  for (auto [header, payload] : zbi_view) {
    if (auto result = zbi.Append(*header, payload); result.is_error()) {
      fprintf(stdout, "Failed to append: %s\n", result.error_value().zbi_error.data());
      return result.take_error();
    }
  }
  return zbi_view.take_error();
}
