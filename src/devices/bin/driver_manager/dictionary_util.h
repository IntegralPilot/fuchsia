// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_DICTIONARY_UTIL_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_DICTIONARY_UTIL_H_

#include <fidl/fuchsia.component.sandbox/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

namespace driver_manager {

class DictionaryUtil {
 public:
  DictionaryUtil(fidl::ClientEnd<fuchsia_component_sandbox::CapabilityStore> store,
                 async_dispatcher_t* dispatcher)
      : store_(std::move(store), dispatcher), dispatcher_(dispatcher) {}

  virtual ~DictionaryUtil() = default;

  virtual void ImportDictionary(
      fuchsia_component_sandbox::DictionaryRef dictionary,
      fit::callback<void(zx::result<fuchsia_component_sandbox::NewCapabilityId>)> callback);

  virtual void ImportDictionaryWire(
      fuchsia_component_sandbox::wire::DictionaryRef dictionary,
      fit::callback<void(zx::result<fuchsia_component_sandbox::wire::NewCapabilityId>)> callback);

  virtual void CopyExportDictionary(
      fuchsia_component_sandbox::CapabilityId dictionary,
      fit::callback<void(zx::result<fuchsia_component_sandbox::DictionaryRef>)> callback);

  virtual void DictionaryDirConnectorOpen(
      fuchsia_component_sandbox::CapabilityId dictionary, std::string_view key,
      fit::callback<void(zx::result<fidl::ClientEnd<fuchsia_io::Directory>>)> callback);

  virtual void CreateDictionaryWith(
      std::unordered_map<std::string, fidl::ClientEnd<fuchsia_component_sandbox::DirReceiver>>
          receivers,
      fit::callback<void(zx::result<fuchsia_component_sandbox::CapabilityId>)> callback);

 private:
  void OpenDirConnector(
      fuchsia_component_sandbox::wire::DirConnector connector,
      fit::callback<void(zx::result<fidl::ClientEnd<fuchsia_io::Directory>>)> callback);

  fidl::WireClient<fuchsia_component_sandbox::CapabilityStore> store_;
  fuchsia_component_sandbox::NewCapabilityId cap_id_ = 0;
  async_dispatcher_t* dispatcher_;
};

struct DirInfo {
  fidl::ClientEnd<fuchsia_io::Directory> dir;
  std::string target_service_name;
  std::unordered_map<std::string, std::string> target_to_source_instance_mapping;
  std::string parent_name;
  bool is_primary;
};

class DirReceiverImpl : public fidl::WireServer<fuchsia_component_sandbox::DirReceiver> {
 public:
  DirReceiverImpl(fidl::ServerEnd<fuchsia_component_sandbox::DirReceiver> dir_receiver,
                  std::vector<DirInfo> dir_infos, async_dispatcher_t* dispatcher)
      : dir_receiver_binding_(dispatcher, std::move(dir_receiver), this,
                              fidl::kIgnoreBindingClosure),
        dir_infos_(std::move(dir_infos)),
        out_(dispatcher) {}

 private:
  void Receive(ReceiveRequestView request, ReceiveCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_component_sandbox::DirReceiver> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  fidl::ServerBinding<fuchsia_component_sandbox::DirReceiver> dir_receiver_binding_;
  std::vector<DirInfo> dir_infos_;
  component::OutgoingDirectory out_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_DICTIONARY_UTIL_H_
