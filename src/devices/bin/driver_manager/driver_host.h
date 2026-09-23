// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_HOST_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_HOST_H_

#include <fidl/fuchsia.driver.host/cpp/fidl.h>
#include <fidl/fuchsia.driver.host/cpp/wire.h>
#include <fidl/fuchsia.driver.loader/cpp/fidl.h>
#include <lib/async/cpp/wait.h>
#include <lib/vfs/cpp/pseudo_dir.h>

#include <fbl/intrusive_double_list.h>

#include "src/devices/bin/driver_loader/loader.h"

namespace driver_manager {
struct PowerElementStartArgs {
  fidl::ClientEnd<fuchsia_power_broker::ElementControl> element_control;
  fidl::ServerEnd<fuchsia_power_broker::ElementRunner> element_runner;
  fidl::ClientEnd<fuchsia_power_broker::Lessor> lessor;
  fuchsia_power_broker::DependencyToken power_element_token;
};
class DriverHost {
 public:
  using StartCallback = fit::callback<void(zx::result<>)>;

  // Components needed to load a driver using dynamic linking.
  struct DriverLoadArgs {
    static zx::result<DriverLoadArgs> Create(
        fuchsia_component_runner::wire::ComponentStartInfo start_info);

    DriverLoadArgs(std::string_view driver_soname, zx::vmo driver_file,
                   fidl::ClientEnd<fuchsia_io::Directory> lib_dir,
                   std::vector<fuchsia_driver_loader::RootModule> additional_root_modules)
        : driver_soname(driver_soname),
          driver_file(std::move(driver_file)),
          lib_dir(std::move(lib_dir)),
          additional_root_modules(std::move(additional_root_modules)) {}

    std::string driver_soname;
    zx::vmo driver_file;
    fidl::ClientEnd<fuchsia_io::Directory> lib_dir;

    // Additional modules that should be loaded together with the driver.
    // e.g. the DFv1 driver to be loaded with the compatibility shim.
    std::vector<fuchsia_driver_loader::RootModule> additional_root_modules;
  };

  // Components that will be sent to the driver host when requesting to start a driver.
  struct DriverStartArgs {
    DriverStartArgs(fuchsia_driver_framework::wire::NodePropertyDictionary2 node_properties,
                    fidl::VectorView<fuchsia_driver_framework::wire::NodeSymbol> symbols,
                    fidl::VectorView<fuchsia_driver_framework::wire::Offer> offers,
                    fuchsia_component_runner::wire::ComponentStartInfo start_info)
        :  // We need to make a copy of these FIDL fields. We receive these fields
           // as part of the |fuchsia_component_runner::ComponentRunner::Start| FIDL call
           // and create this |DriverStartArgs| object, but we may not call
           // |Node::StartDriverWithDynamicLinker| until later (after the FIDL call has returned).
          node_properties_(fidl::ToNatural(node_properties)),
          symbols_(fidl::ToNatural(symbols)),
          offers_(fidl::ToNatural(offers)),
          start_info_(fidl::ToNatural(start_info)) {}

    std::optional<fuchsia_driver_framework::NodePropertyDictionary2> node_properties_;
    std::optional<std::vector<fuchsia_driver_framework::NodeSymbol>> symbols_;
    std::optional<std::vector<fuchsia_driver_framework::Offer>> offers_;
    fuchsia_component_runner::ComponentStartInfo start_info_;
  };

  virtual void Start(fidl::ClientEnd<fuchsia_driver_framework::Node> client_end,
                     std::string node_name,
                     fuchsia_driver_framework::wire::NodePropertyDictionary2 node_properties,
                     fidl::VectorView<fuchsia_driver_framework::wire::NodeSymbol> symbols,
                     fidl::VectorView<fuchsia_driver_framework::wire::Offer> offers,
                     fuchsia_component_runner::wire::ComponentStartInfo start_info,
                     zx::event node_token, fidl::ServerEnd<fuchsia_driver_host::Driver> driver,
                     PowerElementStartArgs power_element_args, StartCallback cb) = 0;

  // Loads and starts a driver using dynamic linking.
  virtual void StartWithDynamicLinker(fidl::ClientEnd<fuchsia_driver_framework::Node> node,
                                      std::string node_name, DriverLoadArgs load_args,
                                      DriverStartArgs start_args, zx::event node_token,
                                      fidl::ServerEnd<fuchsia_driver_host::Driver> driver,
                                      PowerElementStartArgs power_element_args, StartCallback cb) {
    cb(zx::error(ZX_ERR_NOT_SUPPORTED));
  }

  virtual zx::result<uint64_t> GetMainThreadKoid() const = 0;

  virtual zx::result<uint64_t> GetProcessKoid() const = 0;

  virtual void GetProcessKoidAsync(fit::callback<void(zx::result<uint64_t>)> cb) const = 0;

  virtual bool IsDynamicLinkingEnabled() const { return false; }

  // The string tag for cross-DAG driver host colocation (e.g. driver_host = "tag" in NodeAddArgs).
  // When a driver host process is dying during host restart, set_name_for_colocation("") is called
  // to unregister the tag upfront so that GetDriverHost("tag") will not return the dying host.
  virtual std::string_view name_for_colocation() const { return ""; }
  virtual void set_name_for_colocation(std::string name) {}

  virtual void TriggerStackTrace() const {}
};

class DriverHostComponent final
    : public DriverHost,
      public fbl::DoublyLinkedListable<std::unique_ptr<DriverHostComponent>> {
 public:
  DriverHostComponent(fidl::ClientEnd<fuchsia_driver_host::DriverHost> driver_host,
                      async_dispatcher_t* dispatcher,
                      fbl::DoublyLinkedList<std::unique_ptr<DriverHostComponent>>* driver_hosts,
                      std::shared_ptr<bool> server_connected, std::string_view name_for_colocation,
                      fidl::ClientEnd<fuchsia_driver_loader::DriverHost> loader_client = {});

  void Start(fidl::ClientEnd<fuchsia_driver_framework::Node> client_end, std::string node_name,
             fuchsia_driver_framework::wire::NodePropertyDictionary2 node_properties,
             fidl::VectorView<fuchsia_driver_framework::wire::NodeSymbol> symbols,
             fidl::VectorView<fuchsia_driver_framework::wire::Offer> offers,
             fuchsia_component_runner::wire::ComponentStartInfo start_info, zx::event node_token,
             fidl::ServerEnd<fuchsia_driver_host::Driver> driver,
             PowerElementStartArgs power_element_args, StartCallback cb) override;

  void StartWithDynamicLinker(fidl::ClientEnd<fuchsia_driver_framework::Node> node,
                              std::string node_name, DriverLoadArgs load_args,
                              DriverStartArgs start_args, zx::event node_token,
                              fidl::ServerEnd<fuchsia_driver_host::Driver> driver_host_server_end,
                              PowerElementStartArgs power_element_args,

                              StartCallback cb) override;

  zx::result<fuchsia_driver_host::ProcessInfo> GetProcessInfo() const;
  void GetCrashInfo(
      uint64_t thread_koid,
      fit::callback<void(zx::result<fuchsia_driver_host::DriverCrashInfo>)> info_callback) const;

  zx::result<uint64_t> GetMainThreadKoid() const override;
  zx::result<uint64_t> GetProcessKoid() const override;
  void GetProcessKoidAsync(fit::callback<void(zx::result<uint64_t>)> cb) const override;
  zx::result<uint64_t> GetJobKoid() const;

  void TriggerStackTrace() const override;

  zx::result<> InstallLoader(fidl::ClientEnd<fuchsia_ldsvc::Loader> loader_client) const;

  std::string_view name_for_colocation() const override { return name_for_colocation_; }
  void set_name_for_colocation(std::string name) override {
    name_for_colocation_ = std::move(name);
  }

 private:
  void InitializeElfDir();

  bool IsDynamicLinkingEnabled() const override { return dynamic_linker_driver_loader_.is_valid(); }

  fidl::WireSharedClient<fuchsia_driver_host::DriverHost> driver_host_;
  std::string name_for_colocation_;
  mutable std::optional<fuchsia_driver_host::ProcessInfo> process_info_;
  vfs::PseudoDir runtime_dir_;
  async_dispatcher_t* dispatcher_;
  std::shared_ptr<bool> server_connected_;
  // Only valid for driver hosts loaded using dynamic linking.
  fidl::WireClient<fuchsia_driver_loader::DriverHost> dynamic_linker_driver_loader_;
};

zx::result<> SetEncodedConfig(
    fidl::WireTableBuilder<fuchsia_driver_framework::wire::DriverStartArgs>& args,
    fuchsia_component_runner::wire::ComponentStartInfo& start_info);

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_DRIVER_HOST_H_
