// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/process/process_builder.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/io.h>
#include <lib/fdio/limits.h>
#include <lib/fdio/namespace.h>
#include <lib/fit/defer.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/dlfcn.h>
#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <array>
#include <iterator>
#include <string_view>
#include <utility>

#include <fbl/unique_fd.h>

namespace process {

namespace {

// This should match the values used by sdk/lib/fdio/spawn.c
constexpr size_t kFdioResolvePrefixLen = 10;
const char kFdioResolvePrefix[kFdioResolvePrefixLen + 1] = "#!resolve ";

// It is possible to setup an infinite loop of resolvers. We want to avoid this
// being a common abuse vector, but also stay out of the way of any complex user
// setups. This value is the same as spawn.c's above.
constexpr int kFdioMaxResolveDepth = 256;

}  // namespace

ProcessBuilder::ProcessBuilder(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir)
    : svc_dir_(std::move(svc_dir)) {
  auto result = component::ConnectAt<fuchsia_process::Launcher>(svc_dir_);
  ZX_ASSERT_MSG(result.is_ok(), "%s", zx_status_get_string(result.error_value()));
  launcher_.Bind(std::move(*result));
}

ProcessBuilder::ProcessBuilder(zx::job job, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir)
    : ProcessBuilder(std::move(svc_dir)) {
  launch_info_.job() = std::move(job);
}

ProcessBuilder::~ProcessBuilder() = default;

void ProcessBuilder::LoadVMO(zx::vmo executable) {
  launch_info_.executable() = std::move(executable);
}

zx_status_t ProcessBuilder::LoadPath(const std::string& path) {
  fbl::unique_fd fd;
  if (zx_status_t status = fdio_open3_fd(
          path.c_str(),
          static_cast<uint64_t>(fuchsia_io::kPermReadable | fuchsia_io::kPermExecutable),
          fd.reset_and_get_address());
      status != ZX_OK) {
    return status;
  }

  zx::vmo executable_vmo;
  if (zx_status_t status = fdio_get_vmo_exec(fd.get(), executable_vmo.reset_and_get_address());
      status != ZX_OK) {
    return status;
  }

  fidl::ClientEnd<fuchsia_ldsvc::Loader> loader_iface;

  // Resolve VMOs containing #!resolve.
  fidl::SyncClient<fuchsia_process::Resolver> resolver;  // Lazily bound.
  for (int i = 0; true; i++) {
    std::array<char, fuchsia_process::kMaxResolveNameSize + kFdioResolvePrefixLen> head;
    head.fill(0);
    if (zx_status_t status = executable_vmo.read(head.data(), 0, head.size()); status != ZX_OK) {
      return status;
    }
    if (memcmp(kFdioResolvePrefix, head.data(), kFdioResolvePrefixLen) != 0) {
      break;  // No prefix match.
    }

    if (i == kFdioMaxResolveDepth) {
      return ZX_ERR_IO_INVALID;  // Too much nesting.
    }

    // The process name is after the prefix until the first newline.
    std::string_view name(&head[kFdioResolvePrefixLen], fuchsia_process::kMaxResolveNameSize);
    if (size_t newline_index = name.rfind('\n'); newline_index != std::string_view::npos) {
      name = name.substr(0, newline_index);
    }

    // The resolver will give us a new VMO and loader to use.
    if (!resolver) {
      if (auto res = component::ConnectAt<fuchsia_process::Resolver>(svc_dir_); res.is_ok()) {
        resolver.Bind(std::move(*res));
      } else {
        return res.error_value();
      }
    }
    if (auto res = resolver->Resolve(std::string(name.data(), name.size())); res.is_ok()) {
      if (res->status() != ZX_OK) {
        return res->status();
      }
      executable_vmo = std::move(res->executable());
      loader_iface = std::move(res->ldsvc());
    } else {
      return res.error_value().status();
    }
  }

  // Save the loader info.
  zx::handle loader = loader_iface.TakeChannel();
  if (!loader) {
    // Resolver didn't give us a specific one, clone ours.
    if (zx_status_t status = dl_clone_loader_service(loader.reset_and_get_address());
        status != ZX_OK) {
      return status;
    }
  }
  AddHandle(PA_LDSVC_LOADER, std::move(loader));

  // Save the VMO info. Name it with the file part of the path.
  launch_info_.executable() = std::move(executable_vmo);
  const char* name = path.c_str();
  if (path.length() >= ZX_MAX_NAME_LEN) {
    size_t offset = path.rfind('/');
    if (offset != std::string::npos) {
      name += offset + 1;
    }
  }
  return launch_info_.executable().set_property(ZX_PROP_NAME, name, strlen(name));
}

zx_status_t ProcessBuilder::AddArgs(const std::vector<std::string>& argv) {
  if (argv.empty()) {
    return ZX_OK;
  }
  if (launch_info_.name().empty()) {
    launch_info_.name() = argv[0];
  }
  std::vector<std::vector<uint8_t>> args;
  args.reserve(argv.size());
  for (const auto& arg : argv) {
    args.emplace_back(arg.begin(), arg.end());
  }
  if (auto res = launcher_->AddArgs(std::move(args)); res.is_error()) {
    return res.error_value().status();
  }
  return ZX_OK;
}

void ProcessBuilder::AddHandle(uint32_t id, zx::handle handle) {
  handles_.emplace_back(std::move(handle), id);
}

void ProcessBuilder::AddHandles(std::vector<fuchsia_process::HandleInfo> handles) {
  std::copy(std::make_move_iterator(handles.begin()), std::make_move_iterator(handles.end()),
            std::back_inserter(handles_));
}

void ProcessBuilder::SetDefaultJob(zx::job job) {
  handles_.emplace_back(std::move(job), PA_JOB_DEFAULT);
}

void ProcessBuilder::SetName(std::string name) { launch_info_.name() = std::move(name); }

zx_status_t ProcessBuilder::CloneJob() {
  zx::job duplicate_job;
  if (launch_info_.job()) {
    if (zx_status_t status = launch_info_.job().duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate_job);
        status != ZX_OK) {
      return status;
    }
  } else {
    if (zx_status_t status =
            zx::job::default_job()->duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate_job);
        status != ZX_OK) {
      return status;
    }
  }
  SetDefaultJob(std::move(duplicate_job));
  return ZX_OK;
}

zx_status_t ProcessBuilder::CloneNamespace() {
  fdio_flat_namespace_t* flat = nullptr;
  if (zx_status_t status = fdio_ns_export_root(&flat); status != ZX_OK) {
    return status;
  }
  auto cleanup = fit::defer([flat]() { fdio_ns_free_flat_ns(flat); });
  std::vector<fuchsia_process::NameInfo> names;
  for (size_t i = 0; i < flat->count; ++i) {
    names.emplace_back(flat->path[i],
                       fidl::ClientEnd<fuchsia_io::Directory>(zx::channel(flat->handle[i])));
  }
  if (auto res = launcher_->AddNames(std::move(names)); res.is_error()) {
    return res.error_value().status();
  }
  return ZX_OK;
}

void ProcessBuilder::CloneStdio() {
  // These file descriptors might be closed. Skip over errors cloning them.
  CloneFileDescriptor(STDIN_FILENO, STDIN_FILENO);
  CloneFileDescriptor(STDOUT_FILENO, STDOUT_FILENO);
  CloneFileDescriptor(STDERR_FILENO, STDERR_FILENO);
}

zx_status_t ProcessBuilder::CloneEnvironment() {
  std::vector<std::vector<uint8_t>> env;
  for (size_t i = 0; environ[i]; ++i) {
    std::string_view var{environ[i]};
    env.emplace_back(var.begin(), var.end());
  }
  if (auto res = launcher_->AddEnvirons(std::move(env)); res.is_error()) {
    return res.error_value().status();
  }
  return ZX_OK;
}

zx_status_t ProcessBuilder::CloneAll() {
  if (zx_status_t status = CloneJob(); status != ZX_OK) {
    return status;
  }
  if (zx_status_t status = CloneNamespace(); status != ZX_OK) {
    return status;
  }
  if (zx_status_t status = CloneEnvironment(); status != ZX_OK) {
    return status;
  }
  CloneStdio();
  return ZX_OK;
}

zx_status_t ProcessBuilder::CloneFileDescriptor(int local_fd, int target_fd) {
  zx::handle fd_handle;
  if (zx_status_t status = fdio_fd_clone(local_fd, fd_handle.reset_and_get_address());
      status != ZX_OK) {
    return status;
  }
  handles_.emplace_back(std::move(fd_handle), PA_HND(PA_FD, target_fd));
  return ZX_OK;
}

zx_status_t ProcessBuilder::Prepare(std::string* error_message) {
  if (auto res = launcher_->AddHandles(std::move(handles_)); res.is_error()) {
    return res.error_value().status();
  }
  if (!launch_info_.job()) {
    if (zx_status_t status =
            zx::job::default_job()->duplicate(ZX_RIGHT_SAME_RIGHTS, &launch_info_.job());
        status != ZX_OK) {
      return status;
    }
  }
  auto res = launcher_->CreateWithoutStarting(std::move(launch_info_));
  if (res.is_error()) {
    return res.error_value().status();
  }
  if (res->status() != ZX_OK) {
    return res->status();
  }
  if (!res->data()) {
    return ZX_ERR_INVALID_ARGS;
  }
  data_ = std::move(*res->data());
  return ZX_OK;
}

zx_status_t ProcessBuilder::Start(zx::process* process_out) {
  zx_status_t status =
      zx_process_start(data_.process().get(), data_.thread().get(), data_.entry(), data_.stack(),
                       data_.bootstrap().release(), data_.vdso_base());
  if (status == ZX_OK && process_out)
    *process_out = std::move(data_.process());
  return status;
}

}  // namespace process
