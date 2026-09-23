// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_loader/loader.h"

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fdio.h>
#include <lib/fdio/io.h>
#include <lib/fit/defer.h>
#include <lib/ld/abi.h>
#include <lib/ld/remote-abi-heap.h>
#include <lib/ld/remote-abi-stub.h>
#include <lib/ld/remote-abi.h>

#include <filesystem>

#include <fbl/unique_fd.h>

#include "src/devices/bin/driver_loader/diagnostics.h"
#include "src/devices/lib/log/log.h"

namespace fio = fuchsia_io;

namespace {

zx::result<zx::vmo> GetStubLdVmo() {
  std::filesystem::path stub_path{"/pkg/lib"};
  stub_path /= std::string{ld::RemoteAbiStub<>::kFilename};
  const fio::wire::Flags kFlags =
      fio::wire::Flags::kProtocolFile | fio::wire::kPermReadable | fio::wire::kPermExecutable;
  fbl::unique_fd fd;
  zx_status_t status =
      fdio_open3_fd(stub_path.c_str(), static_cast<uint64_t>(kFlags), fd.reset_and_get_address());
  if (status != ZX_OK) {
    return zx::error(status);
  }
  zx::vmo vmo;
  status = fdio_get_vmo_exec(fd.get(), vmo.reset_and_get_address());
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(vmo));
}

}  // namespace

namespace driver_loader {

// static
std::unique_ptr<Loader> Loader::Create(async_dispatcher_t* dispatcher,
                                       LoadDriverHandler load_driver_handler_for_testing) {
  zx::result<zx::vmo> stub_ld_vmo = GetStubLdVmo();
  if (stub_ld_vmo.is_error()) {
    LOGF(ERROR, "Failed to get stub ld vmo for loader: %s", stub_ld_vmo.status_string());
    return nullptr;
  }
  auto diag = MakeDiagnostics();
  return std::unique_ptr<Loader>(new Loader(dispatcher, std::move(load_driver_handler_for_testing),
                                            diag, std::move(*stub_ld_vmo)));
}

void Loader::Connect(fidl::ServerEnd<fuchsia_driver_loader::DriverHostLauncher> server_end) {
  bindings_.AddBinding(dispatcher_, std::move(server_end), this, fidl::kIgnoreBindingClosure);
}

zx::result<> Loader::Start(zx::process process, zx::vmar root_vmar, zx::vmo exec_vmo,
                           zx::vmo vdso_vmo, fidl::ClientEnd<fuchsia_io::Directory> lib_dir,
                           fidl::ServerEnd<fuchsia_driver_loader::DriverHost> server_end) {
  auto diag = MakeDiagnostics();

  Linker linker;
  linker.set_abi_stub(remote_abi_stub_);
  if (!linker.abi_stub()) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  Linker::Module::DecodedPtr decoded_executable =
      Linker::Module::Decoded::Create(diag, std::move(exec_vmo), kPageSize);
  if (!decoded_executable) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  // TODO(https://fxbug.dev/365122208): we should cache the decoded ptr per vdso version.
  Linker::Module::DecodedPtr decoded_vdso =
      Linker::Module::Decoded::Create(diag, std::move(vdso_vmo), kPageSize);
  if (!decoded_vdso) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto init_result = linker.Init(diag,
                                 {Linker::Executable(std::move(decoded_executable)),
                                  Linker::Implicit(std::move(decoded_vdso))},
                                 GetDepFunction(diag, std::move(lib_dir)));
  if (!init_result) {
    // We usually do not expect this to fail. It's possible that there are errors
    // reported to the diagnostics (such as for missing deps), but |init_result| will still be
    // true. For non fatal errors, we will try to proceed as far as possible in order to catch as
    // many issues as possible.
    LOGF(ERROR, "Linker init failed, probably due to a very invalid module");
    return zx::error(ZX_ERR_INTERNAL);
  }

  // We expect this to be equal to the size of the initial_modules list passed to |linker.Init|.
  ZX_ASSERT_MSG(init_result->size() == 2u,
                "Linker.Init returned an unexpected number of elements, want %u got %lu", 2u,
                init_result->size());

  // Allocate new child vmars.
  if (!linker.Allocate(diag, root_vmar.borrow())) {
    // As |Init| has succeeded, |Allocate| should only fail due to system errors such as resource
    // constraints, rather than issues with the module itself. This may be due to kernel OOM, or
    // that there are too many existing mappings.
    LOGF(ERROR, "Linker failed to allocate in vmar, address space may be full");
    return zx::error(ZX_ERR_INTERNAL);
  }

  const Linker::Module& loaded_vdso = *init_result->back();
  struct ProcessState::ProcessStartArgs process_start_args = {
      .entry = linker.main_entry(),
      .vdso_base = loaded_vdso.module().vaddr_start(),
      .stack_size = linker.main_stack_size(),
  };

  // Apply relocations to segment VMOs.
  if (!linker.Relocate(diag)) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  // Since |DiagnosticsFlags::multiple_errors| is set to true, we would not have returned early
  // for non-fatal errors or warnings. Check now before attempting to continue loading.
  auto check_errors = [&diag](std::string_view what, std::string_view error_msg) -> zx::result<> {
    if ((diag.errors() != 0) || (diag.warnings() != 0)) {
      LOGF(ERROR, "Linker diagnostics reported %u errors, %u warnings during %s, %s aborted. %s.",
           diag.errors(), diag.warnings(), what, what, error_msg);
      return zx::error(ZX_ERR_INTERNAL);
    }
    return zx::ok();
  };

  if (auto result = check_errors("relocation", "This is likely due to a bad driver host package");
      result.is_error()) {
    return result.take_error();
  }

  // Load the segments into the allocated vmars.
  if (!linker.Load(diag)) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (auto result = check_errors("loading", "This is an unexpected error"); result.is_error()) {
    return result.take_error();
  }

  // Commit the VMARs and mappings.
  linker.Commit();

  constexpr std::string_view kThreadName = "driver-host-initial-thread";
  zx::thread thread;
  zx_status_t status =
      zx::thread::create(process, kThreadName.data(), kThreadName.size(), 0, &thread);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  zx::channel bootstrap_sender, bootstrap_receiver;
  status = zx::channel::create(0, &bootstrap_sender, &bootstrap_receiver);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  auto preloaded_modules = linker.PreloadedImplicit(*init_result);
  // Start the process.
  auto process_state = ProcessState::Create(
      remote_abi_stub_, std::move(process), std::move(thread), std::move(root_vmar),
      std::move(bootstrap_sender), load_driver_handler_for_testing_.share(),
      std::move(process_start_args), std::move(preloaded_modules));
  if (process_state.is_error()) {
    LOGF(ERROR, "Failed to create process state: %s", process_state.status_string());
    return process_state.take_error();
  }

  status = process_state->BindServer(dispatcher_, std::move(server_end));
  if (status != ZX_OK) {
    LOGF(ERROR, "Failed to bind server endpoint to process state: %s",
         zx_status_get_string(status));
    return zx::error(status);
  }

  status = process_state->Start(std::move(bootstrap_receiver));
  if (status != ZX_OK) {
    LOGF(ERROR, "Failed to start process: %s", zx_status_get_string(status));
    return zx::error(status);
  }
  started_processes_.push_back(std::move(*process_state));
  return zx::ok();
}

void Loader::Launch(LaunchRequestView request, LaunchCompleter::Sync& completer) {
  auto result = Start(std::move(request->process()), std::move(request->root_vmar()),
                      std::move(request->driver_host_binary()), std::move(request->vdso()),
                      std::move(request->driver_host_libs()), std::move(request->driver_host()));
  if (result.is_error()) {
    completer.ReplyError(result.status_value());
    return;
  }
  completer.ReplySuccess();
}

// static
zx::result<zx::vmo> Loader::GetDepVmo(fidl::UnownedClientEnd<fuchsia_io::Directory> lib_dir,
                                      const char* libname) {
  auto [client_end, server_end] = fidl::Endpoints<fio::File>::Create();
  zx_status_t status =
      fdio_open3_at(lib_dir.channel()->get(), libname,
                    static_cast<uint64_t>(fio::wire::kPermReadable | fio::wire::kPermExecutable),
                    server_end.TakeChannel().release());
  if (status != ZX_OK) {
    return zx::error(status);
  }
  fidl::SyncClient file(std::move(client_end));
  auto result = file->GetBackingMemory(fio::VmoFlags::kRead | fio::VmoFlags::kExecute |
                                       fio::VmoFlags::kPrivateClone);
  if (result.is_error()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  return zx::ok(std::move(result->vmo()));
}

// static
zx::result<std::unique_ptr<Loader::ProcessState>> Loader::ProcessState::Create(
    ld::RemoteAbiStub<>::Ptr remote_abi_stub, zx::process process, zx::thread thread,
    zx::vmar root_vmar, zx::channel bootstrap_sender,
    LoadDriverHandler load_driver_handler_for_testing, const ProcessStartArgs& args,
    Linker::InitModuleList preloaded_modules) {
  auto process_state = std::unique_ptr<ProcessState>(new ProcessState());

  process_state->remote_abi_stub_ = std::move(remote_abi_stub);
  process_state->process_ = std::move(process);
  process_state->thread_ = std::move(thread);
  process_state->root_vmar_ = std::move(root_vmar);
  process_state->process_start_args_ = std::move(args);
  process_state->preloaded_modules_ = std::move(preloaded_modules);
  process_state->bootstrap_sender_ = std::move(bootstrap_sender);
  process_state->load_driver_handler_for_testing_ = std::move(load_driver_handler_for_testing);

  zx_status_t status = process_state->AllocateStack();
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(process_state));
}

// TODO(https://fxbug.dev/349913885): this should be replaced by a call to a helper library
// once available.
zx_status_t Loader::ProcessState::AllocateStack() {
  zx::vmo stack_vmo;

  size_t bootstrap_stack_size = process_start_args_.stack_size.value_or(kDefaultStackSize);

  const size_t guard_size = Loader::kPageSize;
  const size_t stack_vmo_size = (bootstrap_stack_size + Loader::kPageSize - 1) & -Loader::kPageSize;
  const size_t stack_vmar_size = stack_vmo_size + guard_size;

  zx_status_t status = zx::vmo::create(stack_vmo_size, 0, &stack_vmo);
  if (status != ZX_OK) {
    LOGF(ERROR, "Failed to create vmo for stack: %s", zx_status_get_string(status));
    return status;
  }

  zx::vmar stack_vmar;
  uintptr_t stack_vmar_base;
  status = root_vmar_.allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                               stack_vmar_size, &stack_vmar, &stack_vmar_base);
  if (status != ZX_OK) {
    LOGF(ERROR, "Failed to allocate child vmar for stack: %s", zx_status_get_string(status));
    return status;
  }

  zx_vaddr_t stack_base;
  status = stack_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC | ZX_VM_ALLOW_FAULTS,
                          guard_size, stack_vmo, 0, stack_vmo_size, &stack_base);
  if (status != ZX_OK) {
    LOGF(ERROR, "Failed to map stack vmar: %s", zx_status_get_string(status));
    return status;
  }

  stack_ = std::move(stack_vmo);
  initial_stack_pointer_ = elfldltl::AbiTraits<>::InitialStackPointer(stack_base, stack_vmo_size);
  return ZX_OK;
}

zx_status_t Loader::ProcessState::BindServer(
    async_dispatcher_t* dispatcher, fidl::ServerEnd<fuchsia_driver_loader::DriverHost> server_end) {
  if (binding_.has_value()) {
    return ZX_ERR_ALREADY_BOUND;
  }
  binding_.emplace(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
  return ZX_OK;
}

zx_status_t Loader::ProcessState::Start(zx::channel bootstrap_receiver) {
  return process_.start(thread_, process_start_args_.entry, initial_stack_pointer_,
                        std::move(bootstrap_receiver), process_start_args_.vdso_base);
}

void Loader::ProcessState::LoadDriver(LoadDriverRequestView request,
                                      LoadDriverCompleter::Sync& completer) {
  fidl::VectorView<fuchsia_driver_loader::wire::RootModule> additional_root_modules;
  if (request->has_additional_root_modules()) {
    additional_root_modules = request->additional_root_modules();
  }
  auto result = LoadDriverModule(std::string(request->driver_soname().get()),
                                 std::move(request->driver_binary()),
                                 std::move(request->driver_libs()), additional_root_modules);
  if (result.is_error()) {
    completer.ReplyError(result.status_value());
    return;
  }
  fidl::Arena arena;
  completer.ReplySuccess(fuchsia_driver_loader::wire::DriverHostLoadDriverResponse::Builder(arena)
                             .dynamic_linking_abi(*result)
                             .Build());
}

zx::result<Loader::DynamicLinkingPassiveAbi> Loader::ProcessState::LoadDriverModule(
    std::string driver_name, zx::vmo driver_module, fidl::ClientEnd<fuchsia_io::Directory> lib_dir,
    fidl::VectorView<fuchsia_driver_loader::wire::RootModule> additional_root_modules) {
  auto diag = MakeDiagnostics();

  if (driver_name == "compat.so") {
    driver_name = "libdriver.so";
  }

  Linker::Soname root_module_name{driver_name};

  Linker linker;
  linker.set_abi_stub(remote_abi_stub_);

  Linker::Module::DecodedPtr decoded_module =
      Linker::Module::Decoded::Create(diag, std::move(driver_module), kPageSize);
  if (!decoded_module) {
    LOGF(ERROR, "Failed to decode driver module for %s", root_module_name.c_str());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Make a copy of the list of driver host initial modules that will be in the
  // driver's dynamic linking namespace - this is the driver host (libdriver.so) and the vDSO.
  Linker::InitModuleList initial_modules = preloaded_modules_;
  initial_modules.emplace_back(Linker::RootModule(decoded_module, root_module_name));

  // We need to keep these strings alive for |Linker.Init|.
  std::vector<std::string> additional_root_modules_names;
  for (auto& module : additional_root_modules) {
    additional_root_modules_names.emplace_back(module.name().get());
    Linker::Soname name{additional_root_modules_names.back()};
    Linker::Module::DecodedPtr decoded_module =
        Linker::Module::Decoded::Create(diag, std::move(module.binary()), kPageSize);
    if (!decoded_module) {
      LOGF(ERROR, "Failed to decode additional root module module %s", name.c_str());
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    initial_modules.emplace_back(Linker::RootModule(decoded_module, name));
  }

  auto init_result =
      linker.Init(diag, std::move(initial_modules), GetDepFunction(diag, std::move(lib_dir)));
  if (!init_result) {
    // We usually do not expect this to fail. It's possible that there are errors
    // reported to the diagnostics (such as for missing deps), but |init_result| will still be
    // true. For non fatal errors, we will try to proceed as far as possible in order to catch as
    // many issues as possible.
    LOGF(ERROR, "Linker init failed, probably due to a very invalid module");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // We expect the |init_result| vector to be the same size as the number of |initial_modules|.
  // This includes the driver module, the two preloaded modules (driver host and vdso),
  // and any additional root modules.
  size_t expected_init_result_size = 3u + additional_root_modules.size();
  ZX_ASSERT_MSG(init_result->size() == expected_init_result_size,
                "Linker.Init returned an unexpected number of elements, want %lu got %lu",
                expected_init_result_size, init_result->size());

  if (!linker.Allocate(diag, root_vmar_.borrow())) {
    // As |Init| has succeeded, |Allocate| should only fail due to system errors such as resource
    // constraints, rather than issues with the module itself. This may be due to kernel OOM, or
    // that there are too many existing mappings.
    LOGF(ERROR, "Linker failed to allocate in vmar, address space may be full");
    return zx::error(ZX_ERR_INTERNAL);
  }

  if (!linker.Relocate(diag)) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  // Since |DiagnosticsFlags::multiple_errors| is set to true, we would not have returned early
  // for non-fatal errors or warnings. Check now before attempting to continue loading.
  auto check_errors = [&diag](std::string_view what, std::string_view error_msg) -> zx::result<> {
    if ((diag.errors() != 0) || (diag.warnings() != 0)) {
      LOGF(ERROR, "Linker diagnostics reported %u errors, %u warnings during %s, %s aborted. %s.",
           diag.errors(), diag.warnings(), what, what, error_msg);
      return zx::error(ZX_ERR_INTERNAL);
    }
    return zx::ok();
  };

  if (auto result = check_errors("relocation", "This is likely due to a bad driver host package");
      result.is_error()) {
    return result.take_error();
  }

  if (!linker.Load(diag)) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (auto result = check_errors("loading", "This is an unexpected error"); result.is_error()) {
    return result.take_error();
  }

  linker.Commit();

  DynamicLinkingPassiveAbi passive_abi = linker.abi_vaddr();

  ZX_ASSERT_MSG(passive_abi != 0u, "Got null for dynamic linking passive abi address");

  if (load_driver_handler_for_testing_) {
    load_driver_handler_for_testing_(bootstrap_sender_.borrow(), passive_abi);
  }

  return zx::ok(passive_abi);
}

}  // namespace driver_loader
