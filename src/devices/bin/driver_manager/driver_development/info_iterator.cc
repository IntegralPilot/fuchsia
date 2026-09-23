// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/driver_development/info_iterator.h"

namespace fdd = fuchsia_driver_development;

namespace driver_development {

namespace {

constexpr size_t kMaxEntries = 7;

}  // namespace

void DeviceInfoIterator::GetNext(GetNextCompleter::Sync& completer) {
  if (offset_ >= list_.size()) {
    completer.Reply(fidl::VectorView<fdd::wire::NodeInfo>{});
    return;
  }

  auto result = cpp20::span(&list_[offset_], std::min(kMaxEntries, list_.size() - offset_));
  offset_ += result.size();

  completer.Reply(
      fidl::VectorView<fdd::wire::NodeInfo>::FromExternal(result.data(), result.size()));
}

void DriverHostInfoIterator::GetNext(GetNextCompleter::Sync& completer) {
  std::vector<fdd::DriverHostInfo> result;
  constexpr size_t kMaxEntries = 50;
  std::ranges::move(list_ | std::views::take(kMaxEntries), std::back_inserter(result));
  list_.erase(list_.begin(), list_.begin() + std::min<size_t>(kMaxEntries, list_.size()));

  completer.Reply(std::move(result));
}

void CompositeInfoIterator::GetNext(GetNextCompleter::Sync& completer) {
  if (offset_ >= list_.size()) {
    completer.Reply(fidl::VectorView<fdd::wire::CompositeNodeInfo>{});
    return;
  }

  auto result = cpp20::span(&list_[offset_], std::min(kMaxEntries, list_.size() - offset_));
  offset_ += result.size();
  completer.Reply(
      fidl::VectorView<fdd::wire::CompositeNodeInfo>::FromExternal(result.data(), result.size()));
}

}  // namespace driver_development
