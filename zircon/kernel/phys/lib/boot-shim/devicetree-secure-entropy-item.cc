// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree.h>
#include <lib/boot-shim/item-base.h>
#include <lib/devicetree/matcher.h>
#include <lib/fit/defer.h>
#include <lib/fit/internal/result.h>
#include <lib/zbi-format/secure-entropy.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/image.h>

#include <explicit-memory/bytes.h>

namespace boot_shim {
namespace {

constexpr std::string_view kKaslrProperty = "kaslr-seed";
constexpr std::string_view kRngProperty = "rng-seed";

fit::result<ItemBase::DataZbi::Error> MaybeAppendEntropy(std::span<std::byte> random_bytes,
                                                         zbi_secure_entropy_t entropy_type,
                                                         ItemBase::DataZbi& zbi) {
  if (!random_bytes.empty()) {
    if (auto res = zbi.Append(
            {
                .type = ZBI_TYPE_SECURE_ENTROPY,
                .length = static_cast<uint32_t>(random_bytes.size_bytes()),
                .extra = entropy_type,
            },
            random_bytes);
        res.is_error()) {
      return res.take_error();
    }
    // clear the original source of the bytes, devicetree.
    mandatory_memset(random_bytes.data(), DevicetreeSecureEntropyItem::kFillPattern,
                     random_bytes.size_bytes());
  }
  return fit::ok();
}

}  // namespace

fit::result<ItemBase::DataZbi::Error> DevicetreeSecureEntropyItem::AppendItems(DataZbi& zbi) {
  if (auto res = MaybeAppendEntropy(rng_bytes_, ZBI_SECURE_ENTROPY_GENERAL, zbi); res.is_error()) {
    return res.take_error();
  }
  rng_bytes_ = {};

  if (auto res = MaybeAppendEntropy(kaslr_bytes_, ZBI_SECURE_ENTROPY_EARLY_BOOT, zbi);
      res.is_error()) {
    return res.take_error();
  }
  kaslr_bytes_ = {};
  return fit::ok();
}

devicetree::ScanState DevicetreeSecureEntropyItem::OnNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  if (path == "/") {
    return devicetree::ScanState::kActive;
  }

  if (path != "/chosen") {
    return devicetree::ScanState::kDoneWithSubtree;
  }

  auto [kaslr_seed_prop, rng_seed_prop] = decoder.FindProperties(kKaslrProperty, kRngProperty);
  if (rng_seed_prop) {
    auto tmp = rng_seed_prop->AsBytes();
    rng_bytes_ = {reinterpret_cast<std::byte*>(const_cast<uint8_t*>(tmp.data())), tmp.size_bytes()};
  }

  if (kaslr_seed_prop) {
    auto tmp = kaslr_seed_prop->AsBytes();
    kaslr_bytes_ = {reinterpret_cast<std::byte*>(const_cast<uint8_t*>(tmp.data())),
                    tmp.size_bytes()};
  }
  return devicetree::ScanState::kDone;
}

}  // namespace boot_shim
