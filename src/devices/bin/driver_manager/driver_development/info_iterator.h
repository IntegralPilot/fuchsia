// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_DEVELOPMENT_INFO_ITERATOR_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_DEVELOPMENT_INFO_ITERATOR_H_

#include <fidl/fuchsia.driver.development/cpp/fidl.h>

#include <deque>

namespace driver_development {

class DeviceInfoIterator : public fidl::WireServer<fuchsia_driver_development::NodeInfoIterator> {
 public:
  explicit DeviceInfoIterator(std::unique_ptr<fidl::Arena<512>> arena,
                              std::vector<fuchsia_driver_development::wire::NodeInfo> list)
      : arena_(std::move(arena)), list_(std::move(list)) {}

  void GetNext(GetNextCompleter::Sync& completer) override;

 private:
  size_t offset_ = 0;
  std::unique_ptr<fidl::Arena<512>> arena_;
  std::vector<fuchsia_driver_development::wire::NodeInfo> list_;
};

class DriverHostInfoIterator
    : public fidl::Server<fuchsia_driver_development::DriverHostInfoIterator> {
 public:
  explicit DriverHostInfoIterator(std::deque<fuchsia_driver_development::DriverHostInfo> list)
      : list_(std::move(list)) {}

  void GetNext(GetNextCompleter::Sync& completer) override;

 private:
  std::deque<fuchsia_driver_development::DriverHostInfo> list_;
};

class CompositeInfoIterator
    : public fidl::WireServer<fuchsia_driver_development::CompositeInfoIterator> {
 public:
  explicit CompositeInfoIterator(
      std::unique_ptr<fidl::Arena<512>> arena,
      std::vector<fuchsia_driver_development::wire::CompositeNodeInfo> list)
      : arena_(std::move(arena)), list_(std::move(list)) {}

  void GetNext(GetNextCompleter::Sync& completer) override;

 private:
  size_t offset_ = 0;
  std::unique_ptr<fidl::Arena<512>> arena_;
  std::vector<fuchsia_driver_development::wire::CompositeNodeInfo> list_;
};

}  // namespace driver_development

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_DEVELOPMENT_INFO_ITERATOR_H_
