// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_LOADER_LOADER_H_
#define SRC_DEVICES_BIN_DRIVER_LOADER_LOADER_H_

#include <fidl/fuchsia.driver.loader/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fdio/directory.h>
#include <lib/ld/remote-dynamic-linker.h>
#include <lib/ld/remote-load-module.h>
#include <lib/zircon-internal/default_stack_size.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <memory>
#include <optional>

#include <fbl/intrusive_double_list.h>

#include "src/devices/bin/driver_loader/diagnostics.h"

namespace driver_loader {

class Loader : public fidl::WireServer<fuchsia_driver_loader::DriverHostLauncher> {
 public:
  using Linker = ld::RemoteDynamicLinker<>;
  // This is the ABI returned from dynamic linking that provides immutable access to
  // the data structures and symbols of loaded modules.
  using DynamicLinkingPassiveAbi = Linker::size_type;
  using LoadDriverHandler = fit::function<void(zx::unowned_channel bootstrap_sender,
                                               DynamicLinkingPassiveAbi passive_abi)>;

  // If |load_driver_handler_for_testing| is provided, it will be called each time a driver has been
  // loaded. This allows tests to inject custom bootstrap behavior.
  static std::unique_ptr<Loader> Create(async_dispatcher_t* dispatcher,
                                        LoadDriverHandler load_driver_handler_for_testing = {});

  void Connect(fidl::ServerEnd<fuchsia_driver_loader::DriverHostLauncher> server_end);

  // fidl::WireServer<fuchsia_driver_loader::DriverHostLauncher>
  void Launch(LaunchRequestView request, LaunchCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_driver_loader::DriverHostLauncher> md,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_UNAVAILABLE);
  }

 private:
  using RemoteModule = ld::RemoteLoadModule<>;

  inline static const size_t kPageSize = zx_system_get_page_size();
  inline static constexpr size_t kDefaultStackSize = ZIRCON_DEFAULT_STACK_SIZE;

  // Stores the state for a started process.
  class ProcessState : public fidl::WireServer<fuchsia_driver_loader::DriverHost>,
                       public fbl::DoublyLinkedListable<std::unique_ptr<ProcessState>> {
   public:
    struct ProcessStartArgs {
      // Entry point for the thread.
      uintptr_t entry = 0;
      // Base pointer of the loaded vdso.
      uintptr_t vdso_base = 0;
      // Stack size request from the executable's PT_GNU_STACK, if any.
      std::optional<size_t> stack_size = std::nullopt;
    };

    static zx::result<std::unique_ptr<ProcessState>> Create(
        ld::RemoteAbiStub<>::Ptr remote_abi_stub, zx::process process, zx::thread thread,
        zx::vmar root_vmar, zx::channel bootstrap_sender,
        LoadDriverHandler load_driver_handler_for_testing,
        const ProcessStartArgs& process_start_args, Linker::InitModuleList preloaded_modules);

    // fidl::WireServer<fuchsia_driver_loader::DriverHost>
    void LoadDriver(LoadDriverRequestView request, LoadDriverCompleter::Sync& completer) override;

    void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_driver_loader::DriverHost> md,
                               fidl::UnknownMethodCompleter::Sync& completer) override {
      completer.Close(ZX_ERR_UNAVAILABLE);
    }

    zx_status_t BindServer(async_dispatcher_t* dispatcher,
                           fidl::ServerEnd<fuchsia_driver_loader::DriverHost> server_end);

    // Begins execution of the process.
    zx_status_t Start(zx::channel bootstrap_receiver);

   private:
    // TODO(https://fxbug.dev/357948288): this is a placeholder until we can sub in
    // the actual start function signature.
    static constexpr std::string_view kDriverStartSymbol = "DriverStart";

    // Use |Create| instead.
    ProcessState() = default;

    // Loads a driver module into the driver host process.
    // Returns the dynamic linking passive ABI for the loaded modules.
    zx::result<DynamicLinkingPassiveAbi> LoadDriverModule(
        std::string driver_name, zx::vmo driver_module,
        fidl::ClientEnd<fuchsia_io::Directory> lib_dir,
        fidl::VectorView<fuchsia_driver_loader::wire::RootModule> additional_root_modules);

    // Allocates the stack for the initial thread of the process.
    zx_status_t AllocateStack();

    ld::RemoteAbiStub<>::Ptr remote_abi_stub_;

    zx::process process_;
    zx::thread thread_;
    zx::vmar root_vmar_;

    // Connection to the started driver host process for sending bootstrap messages.
    zx::channel bootstrap_sender_;
    // Handler to call after a driver is loaded.
    LoadDriverHandler load_driver_handler_for_testing_;

    ProcessStartArgs process_start_args_;

    // Stack for the initial thread allocated by |AllocateStack|.
    zx::vmo stack_;
    uintptr_t initial_stack_pointer_ = 0;

    std::optional<fidl::ServerBinding<fuchsia_driver_loader::DriverHost>> binding_;
    Linker::InitModuleList preloaded_modules_;
  };

  // Use |Create| instead.
  explicit Loader(async_dispatcher_t* dispatcher, LoadDriverHandler load_driver_handler_for_testing,
                  Diagnostics& diag, zx::vmo stub_ld_vmo)
      : dispatcher_(dispatcher),
        load_driver_handler_for_testing_(std::move(load_driver_handler_for_testing)),
        remote_abi_stub_(ld::RemoteAbiStub<>::Create(diag, std::move(stub_ld_vmo), kPageSize)) {}

  // Launches the |exec| driver host binary into |process|.
  zx::result<> Start(zx::process process, zx::vmar root_vmar, zx::vmo exec, zx::vmo vdso,
                     fidl::ClientEnd<fuchsia_io::Directory> lib_dir,
                     fidl::ServerEnd<fuchsia_driver_loader::DriverHost> driver_host);

  // Retrieves the vmo for |libname| from |lib_dir|.
  static zx::result<zx::vmo> GetDepVmo(fidl::UnownedClientEnd<fuchsia_io::Directory> lib_dir,
                                       const char* libname);

  // Returns a closure that can be passed to |ld::RemoteDynamicLinker::Init|.
  static auto GetDepFunction(Diagnostics& diag, fidl::ClientEnd<fuchsia_io::Directory> lib_dir) {
    return [&diag, lib_dir = std::move(lib_dir)](
               const RemoteModule::Soname& soname) -> Linker::GetDepResult {
      auto result = GetDepVmo(lib_dir, soname.c_str());
      if (result.is_ok()) {
        return RemoteModule::Decoded::Create(diag, std::move(*result), kPageSize);
      }
      // |MissingDependency| or |SystemError| will return true if we should continue
      // processing even on error, or false if we should abort early.
      if (result.error_value() == ZX_ERR_NOT_FOUND
              ? !diag.MissingDependency(soname.str())
              : !diag.SystemError("cannot open dependency ", soname.str(), ": ",
                                  elfldltl::ZirconError{result.error_value()})) {
        // Tell the linker to abort.
        return std::nullopt;
      }
      // Return an empty pointer. This tells the linker to try to continue even with the error.
      return RemoteModule::Decoded::Ptr();
    };
  }

  async_dispatcher_t* dispatcher_ = nullptr;
  LoadDriverHandler load_driver_handler_for_testing_;
  ld::RemoteAbiStub<>::Ptr remote_abi_stub_;

  fbl::DoublyLinkedList<std::unique_ptr<ProcessState>> started_processes_;

  fidl::ServerBindingGroup<fuchsia_driver_loader::DriverHostLauncher> bindings_;
};

}  // namespace driver_loader

#endif  // SRC_DEVICES_BIN_DRIVER_LOADER_LOADER_H_
