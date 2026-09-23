// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.component.resolution/cpp/wire.h>
#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.pkg/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/directory.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <cstdint>
#include <format>
#include <fstream>
#include <list>
#include <string_view>
#include <unordered_set>
#include <vector>

#include <fbl/unique_fd.h>
#include <rapidjson/document.h>
#include <src/lib/files/directory.h>
#include <src/lib/files/file.h>

namespace {

zx::result<fidl::ClientEnd<fuchsia_io::Directory>> OpenPkgDir() {
  auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (endpoints.is_error()) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  zx_status_t status = fdio_open3(
      "/pkg",
      static_cast<uint64_t>(fuchsia_io::wire::Flags::kProtocolDirectory |
                            fuchsia_io::wire::kPermReadable | fuchsia_io::wire::kPermExecutable),
      endpoints->server.TakeChannel().release());
  if (status != ZX_OK) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  return zx::ok(std::move(endpoints->client));
}

struct ClientListEntry {
  fidl::ClientEnd<fuchsia_io::Directory> client;
  std::string url_prefix;
  std::string pkg_name;
};

struct BootAndBaseConfigResult {
  std::vector<std::string> boot_drivers;
  std::vector<std::string> base_drivers;
};

class DriverLists : public fidl::WireServer<fuchsia_driver_test::DriverLists> {
 public:
  explicit DriverLists(BootAndBaseConfigResult lists) : lists_(std::move(lists)) {}
  void GetDriverLists(GetDriverListsCompleter::Sync& completer) override {
    fidl::Arena arena;
    completer.ReplySuccess(fidl::ToWire(arena, lists_.boot_drivers),
                           fidl::ToWire(arena, lists_.base_drivers));
  }

 private:
  BootAndBaseConfigResult lists_;
};

class FakeComponentResolver final
    : public fidl::WireServer<fuchsia_component_resolution::Resolver> {
 public:
  explicit FakeComponentResolver(fidl::ClientEnd<fuchsia_io::Directory> boot_dir,
                                 fidl::ClientEnd<fuchsia_io::Directory> test_pkg_dir)
      : boot_dir_(std::move(boot_dir)), test_pkg_dir_(std::move(test_pkg_dir)) {}

 private:
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_component_resolution::Resolver> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOG_KV(WARNING, "Unknown Resolver request", FX_KV("ordinal", metadata.method_ordinal));
  }

  void Resolve(ResolveRequestView request, ResolveCompleter::Sync& completer) override {
    std::string_view kTestPackagePrefix = "dtr-test-pkg://fuchsia.com/";
    std::string_view kBootPrefix = "fuchsia-boot:///";
    std::string_view kPkgPrefix = "fuchsia-pkg://fuchsia.com/";
    std::string_view relative_path = request->component_url.get();
    FX_LOG_KV(DEBUG, "Resolving", FX_KV("url", relative_path));

    if (!cpp20::starts_with(relative_path, kTestPackagePrefix) &&
        !cpp20::starts_with(relative_path, kBootPrefix) &&
        !cpp20::starts_with(relative_path, kPkgPrefix)) {
      FX_LOG_KV(ERROR, "FakeComponentResolver request not supported.",
                FX_KV("url", std::string(relative_path).c_str()));
      completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kInvalidArgs);
      return;
    }

    std::string_view pkg_url;
    fidl::ClientEnd<fuchsia_io::Directory> dir_to_use;

    // kTestPackagePrefix can resolve items from the test package, whether they are in the
    // package directly or part of a subpackage.
    if (cpp20::starts_with(relative_path, kTestPackagePrefix)) {
      relative_path.remove_prefix(kTestPackagePrefix.length());

      // The internal client is used for both direct and subpackage.
      auto internal_client = component::Connect<fuchsia_driver_test::Internal>();
      if (internal_client.is_error()) {
        FX_LOG_KV(ERROR, "Failed to connect to internal protocol.");
        completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kInternal);
        return;
      }

      if (!cpp20::starts_with(relative_path, "#")) {
        // This is a subpackage of the test so we have to use its resolution context to resolve the
        // subpackage.
        ResolveSubpackage(relative_path, *internal_client, completer);
        return;
      }

      // Remove the "#"
      relative_path.remove_prefix(1);

      zx::result<fidl::ClientEnd<fuchsia_io::Directory>> dir_clone_result =
          component::Clone(test_pkg_dir_);
      if (dir_clone_result.is_error()) {
        FX_LOG_KV(ERROR, "Failed to clone directory.");
        completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kInternal);
        return;
      }
      dir_to_use = std::move(dir_clone_result.value());
    } else {
      // Legacy resolver logic:
      // Looks at the prefix to determine which directory to look in (pkg or
      // boot), then looks at the path following '#' to find the fragment path within that
      // directory. It's assumed that the components can be found by the fragment path.
      bool is_boot = cpp20::starts_with(relative_path, kBootPrefix);
      size_t pos = relative_path.find('#');
      pkg_url = relative_path.substr(0, pos);
      relative_path.remove_prefix(pos + 1);

      zx::result<fidl::ClientEnd<fuchsia_io::Directory>> dir_clone_result;
      if (is_boot) {
        dir_clone_result = component::Clone(boot_dir_);
      } else {
        dir_clone_result = OpenPkgDir();
      }

      if (dir_clone_result.is_error()) {
        FX_LOG_KV(ERROR, "Failed to clone directory.");
        completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kInternal);
        return;
      }
      dir_to_use = std::move(dir_clone_result.value());
    }

    if (!dir_to_use.is_valid()) {
      FX_LOG_KV(ERROR, "Failed to set dir_to_use.");
    }

    zx::result manifest_vmo = ReadFileToVmo(relative_path, dir_to_use.channel().get());
    if (manifest_vmo.is_error()) {
      FX_LOG_KV(ERROR, "Failed to read manifest.",
                FX_KV("manifest", std::string(relative_path).c_str()));
      completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kIo);
      return;
    }

    uint64_t manifest_size;
    zx_status_t status = manifest_vmo->get_prop_content_size(&manifest_size);
    if (status != ZX_OK) {
      FX_LOG_KV(ERROR, "Failed to get vmo size.",
                FX_KV("manifest", std::string(relative_path).c_str()));
      completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kIo);
      return;
    }

    std::vector<uint8_t> manifest;
    manifest.resize(manifest_size);
    status = manifest_vmo->read(manifest.data(), 0, manifest_size);
    if (status != ZX_OK) {
      FX_LOG_KV(ERROR, "Failed to read manifest vmo.",
                FX_KV("manifest", std::string(relative_path).c_str()));
      completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kIo);
      return;
    }

    auto declaration = fidl::Unpersist<fuchsia_component_decl::Component>(manifest);
    if (declaration.is_error()) {
      FX_LOG_KV(ERROR, "Failed to parse component manifest.",
                FX_KV("manifest", std::string(relative_path).c_str()));
      completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kInvalidManifest);
      return;
    }

    // Check if there is a config file associated with the driver, and read if it exists.
    uint64_t config_size = 0;
    zx::vmo config_vmo;

    if (declaration->config() &&
        declaration->config()->value_source()->package_path().has_value()) {
      std::string config_path = declaration->config()->value_source()->package_path().value();
      zx::result config_file = ReadFileToVmo(config_path, dir_to_use.channel().get());
      if (config_file.is_error()) {
        FX_LOG_KV(ERROR, "Failed to read config vmo.", FX_KV("config", config_path.c_str()));
        completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kIo);
        return;
      }

      status = config_file->get_prop_content_size(&config_size);
      if (status != ZX_OK) {
        FX_LOG_KV(ERROR, "Failed to get vmo size.", FX_KV("config", config_path.c_str()));
        completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kIo);
        return;
      }

      config_vmo = std::move(*config_file);
    }

    // Not all boot components resolved by this resolver are packaged (e.g. root.cm) so they do
    // not have an ABI revision. As a workaround, apply the ABI revision of this package so
    // components can pass runtime ABI compatibility checks during testing.
    std::ifstream abi_revision_file("/pkg/meta/fuchsia.abi/abi-revision");
    if (!abi_revision_file) {
      FX_LOG_KV(ERROR, "Failed to open abi-revision.");
      completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kIo);
      return;
    }

    uint64_t abi_revision;
    abi_revision_file.read(reinterpret_cast<char*>(&abi_revision), sizeof(abi_revision));
    if (!abi_revision_file) {
      FX_LOG_KV(ERROR, "Failed to read abi-revision.");
      completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kIo);
      return;
    }
    abi_revision_file.close();

    fidl::Arena arena;
    auto package = fuchsia_component_resolution::wire::Package::Builder(arena)
                       .url(pkg_url)
                       .directory(std::move(dir_to_use))
                       .Build();

    auto builder =
        fuchsia_component_resolution::wire::Component::Builder(arena)
            .url(request->component_url)
            .abi_revision(abi_revision)
            .decl(fuchsia_mem::wire::Data::WithBuffer(arena,
                                                      fuchsia_mem::wire::Buffer{
                                                          .vmo = std::move(*manifest_vmo),
                                                          .size = manifest_size,
                                                      }))
            .package(package);

    if (config_vmo.is_valid()) {
      builder.config_values(
          fuchsia_mem::wire::Data::WithBuffer(arena, fuchsia_mem::wire::Buffer{
                                                         .vmo = std::move(config_vmo),
                                                         .size = config_size,
                                                     }));
    }

    auto component = builder.Build();

    FX_LOG_KV(DEBUG, "Successfully Resolved", FX_KV("url", relative_path));
    completer.ReplySuccess(component);
  }

  void ResolveWithContext(ResolveWithContextRequestView request,
                          ResolveWithContextCompleter::Sync& completer) override {
    FX_LOG_KV(ERROR, "FakeComponentResolver does not currently support ResolveWithContext");
    completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kInvalidArgs);
  }

  static void ResolveSubpackage(
      std::string_view relative_path,
      fidl::UnownedClientEnd<fuchsia_driver_test::Internal> internal_client,
      ResolveCompleter::Sync& completer) {
    auto resolution_context = fidl::WireCall(internal_client)->GetTestResolutionContext();
    if (!resolution_context.ok()) {
      FX_LOG_KV(ERROR, "Failed to get resolution context.",
                FX_KV("description", resolution_context.FormatDescription().c_str()));
      completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kInternal);
      return;
    }

    if (resolution_context.value().is_error()) {
      FX_LOG_KV(ERROR, "Failed to get resolution context.",
                FX_KV("error", zx_status_get_string(resolution_context.value().error_value())));
      completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kInternal);
      return;
    }

    auto resolver = component::Connect<fuchsia_component_resolution::Resolver>(
        "/svc/fuchsia.component.resolution.Resolver-hermetic");
    if (resolver.is_error()) {
      FX_LOG_KV(ERROR, "Failed to connect to resolver protocol.");
      completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kInternal);
      return;
    }

    fidl::Arena arena;
    auto result = fidl::WireCall(*resolver)->ResolveWithContext(
        fidl::StringView(arena, relative_path), *resolution_context.value().value()->context.get());
    if (!result.ok() || result.value().is_error()) {
      FX_LOG_KV(ERROR, "Failed to resolve.");
      completer.ReplyError(fuchsia_component_resolution::wire::ResolverError::kInternal);
      return;
    }

    completer.ReplySuccess(result->value()->component);
  }

  static zx::result<zx::vmo> ReadFileToVmo(std::string_view path, zx_handle_t dir) {
    auto file_ep = fidl::CreateEndpoints<fuchsia_io::File>();
    if (file_ep.is_error()) {
      FX_LOG_KV(ERROR, "Failed to create file endpoints");
      return zx::error(ZX_ERR_INTERNAL);
    }

    zx_status_t status = fdio_open3_at(dir, std::string(path).data(),
                                       static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                                       file_ep->server.channel().release());
    if (status != ZX_OK) {
      FX_LOG_KV(ERROR, "Failed to open file.", FX_KV("file", std::string(path).c_str()));
      return zx::error(ZX_ERR_IO);
    }

    fidl::WireResult result =
        fidl::WireCall(file_ep->client)->GetBackingMemory(fuchsia_io::wire::VmoFlags::kRead);
    if (!result.ok()) {
      FX_LOG_KV(DEBUG, "Failed to read file.", FX_KV("file", std::string(path).c_str()));
      return zx::error(ZX_ERR_NOT_FOUND);
    }

    auto& response = result.value();
    if (response.is_error()) {
      FX_LOG_KV(ERROR, "Failed to read file.", FX_KV("file", std::string(path).c_str()));
      return zx::error(ZX_ERR_IO);
    }

    return zx::ok(std::move(response.value()->vmo));
  }

  fidl::ClientEnd<fuchsia_io::Directory> boot_dir_;
  fidl::ClientEnd<fuchsia_io::Directory> test_pkg_dir_;
};

zx::result<std::list<ClientListEntry>> MakeClientList(
    fidl::UnownedClientEnd<fuchsia_io::Directory> boot_drivers_dir,
    fidl::UnownedClientEnd<fuchsia_io::Directory> test_pkg_dir,
    const std::optional<fuchsia_component_resolution::Context>& test_resolution_context) {
  std::list<ClientListEntry> client_list;

  zx::result cloned_boot_drivers_dir = component::Clone(boot_drivers_dir);
  if (cloned_boot_drivers_dir.is_error()) {
    FX_LOG_KV(ERROR, "Unable to clone dir");
    return zx::error(ZX_ERR_IO);
  }

  client_list.emplace_back(ClientListEntry{
      *std::move(cloned_boot_drivers_dir),
      "fuchsia-boot:///",
      "dtr",
  });

  if (test_pkg_dir.is_valid()) {
    zx::result cloned_test_pkg_dir = component::Clone(test_pkg_dir);
    if (cloned_test_pkg_dir.is_error()) {
      FX_LOG_KV(ERROR, "Unable to clone dir");
      return zx::error(ZX_ERR_IO);
    }

    // Add the test package itself.
    client_list.emplace_back(ClientListEntry{
        *std::move(cloned_test_pkg_dir),
        "dtr-test-pkg://fuchsia.com/",
        "",
    });

    // Need to clone again to use in fdio_fd_create.
    cloned_test_pkg_dir = component::Clone(test_pkg_dir);
    if (cloned_test_pkg_dir.is_error()) {
      FX_LOG_KV(ERROR, "Unable to clone dir");
      return zx::error(ZX_ERR_IO);
    }
    fbl::unique_fd dir_fd;
    zx_status_t status =
        fdio_fd_create(cloned_test_pkg_dir->TakeHandle().release(), dir_fd.reset_and_get_address());
    if (status != ZX_OK) {
      FX_LOG_KV(ERROR, "Failed to turn dir into fd");
      return zx::error(ZX_ERR_IO);
    }

    if (files::IsFileAt(dir_fd.get(), "meta/fuchsia.pkg/subpackages")) {
      // Read off the subpackages that exist in the test package.
      std::string result;
      files::ReadFileToStringAt(dir_fd.get(), "meta/fuchsia.pkg/subpackages", &result);
      rapidjson::Document subpackages_doc;
      subpackages_doc.Parse(result.c_str());
      auto& subpackages = subpackages_doc["subpackages"];
      std::vector<std::string> subpackage_names;
      subpackage_names.reserve(subpackages.MemberCount());
      for (rapidjson::Value::ConstMemberIterator itr = subpackages.MemberBegin();
           itr != subpackages.MemberEnd(); ++itr) {
        subpackage_names.push_back(itr->name.GetString());
      }

      if (!subpackage_names.empty()) {
        // Resolve the subpackage using the context.
        auto resolver = component::Connect<fuchsia_pkg::PackageResolver>(
            "/svc/fuchsia.pkg.PackageResolver-hermetic");
        if (resolver.is_error()) {
          FX_LOG_KV(ERROR, "Failed to connect to resolver protocol.",
                    FX_KV("error", zx_status_get_string(resolver.error_value())));
          return resolver.take_error();
        }
        if (!test_resolution_context.has_value()) {
          FX_LOG_KV(
              WARNING,
              "Subpackages exist in this package but a resolution context was not provided with the test_component. Skipping.");
        } else {
          fidl::Arena arena;
          fuchsia_pkg::wire::ResolutionContext converted_context(
              {.bytes = fidl::ToWire(arena, test_resolution_context->bytes())});

          // Add the subpackages of the test package into the list.
          for (auto& subpackage_name : subpackage_names) {
            auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
            auto resolve_result = fidl::WireCall(*resolver)->ResolveWithContext(
                fidl::StringView(arena, subpackage_name), converted_context, std::move(server));
            if (!resolve_result.ok() || resolve_result->is_error()) {
              FX_LOG_KV(ERROR, "Failed to resolve_with_context in the test package.",
                        FX_KV("error", resolve_result.FormatDescription().c_str()));
              return zx::error(ZX_ERR_INTERNAL);
            }

            client_list.emplace_back(ClientListEntry{
                std::move(client),
                "dtr-test-pkg://fuchsia.com/",
                subpackage_name,
            });
          }
        }
      }
    }
  }

  return zx::ok(std::move(client_list));
}

zx::result<BootAndBaseConfigResult> ConstructBootAndBaseConfig(
    std::list<ClientListEntry> client_list,
    const std::unordered_set<std::string>& boot_driver_components) {
  std::unordered_set<std::string> inserted;
  std::vector<std::string> boot_drivers;
  std::vector<std::string> base_drivers;

  for (auto& [dir, url_prefix, pkg_name] : client_list) {
    // Check each manifest to see if it uses the driver runner.
    fbl::unique_fd dir_fd;
    zx_status_t status = fdio_fd_create(dir.TakeHandle().release(), dir_fd.reset_and_get_address());
    if (status != ZX_OK) {
      FX_LOG_KV(ERROR, "Failed to turn dir into fd");
      return zx::error(ZX_ERR_IO);
    }

    std::vector<std::string> manifests;
    if (!files::ReadDirContentsAt(dir_fd.get(), "meta", &manifests)) {
      FX_LOG_KV(WARNING, "Unable to read dir contents.", FX_KV("url_prefix", url_prefix),
                FX_KV("pkg_name", pkg_name));
      continue;
    }

    for (const auto& manifest : manifests) {
      std::string manifest_path = "meta/" + manifest;
      if (!files::IsFileAt(dir_fd.get(), manifest_path) ||
          !cpp20::ends_with(std::string_view(manifest), ".cm")) {
        continue;
      }
      std::vector<uint8_t> manifest_bytes;
      if (!files::ReadFileToVectorAt(dir_fd.get(), manifest_path, &manifest_bytes)) {
        FX_LOG_KV(ERROR, "Unable to read file contents for", FX_KV("manifest", manifest_path));
        return zx::error(ZX_ERR_IO);
      }
      fit::result component = fidl::Unpersist<fuchsia_component_decl::Component>(manifest_bytes);
      if (component.is_error()) {
        FX_LOG_KV(ERROR, "Unable to unpersist component manifest",
                  FX_KV("manifest", manifest_path));
        return zx::error(ZX_ERR_IO);
      }
      if (!component->program() || !component->program()->runner() ||
          *component->program()->runner() != "driver") {
        continue;
      }

      // Construct the url entry from the pieces provided in the list entry.
      std::string entry = std::format("{}{}#meta/{}", url_prefix, pkg_name, manifest);

      // Protect against duplicate drivers. Only add the driver if it is unique.
      if (inserted.insert(std::format("{}", manifest)).second) {
        // Add to corresponding list.
        if (entry.starts_with("fuchsia-boot:///") ||
            boot_driver_components.find(manifest) != boot_driver_components.end()) {
          boot_drivers.push_back(entry);
        } else {
          base_drivers.push_back(entry);
        }
      }
    }
  }

  return zx::ok(BootAndBaseConfigResult{
      .boot_drivers = std::move(boot_drivers),
      .base_drivers = std::move(base_drivers),
  });
}

zx::result<BootAndBaseConfigResult> SetupLists(
    fidl::UnownedClientEnd<fuchsia_io::Directory> boot_dir,
    fidl::UnownedClientEnd<fuchsia_io::Directory> test_pkg_dir,
    const std::optional<fuchsia_component_resolution::Context>& test_resolution_context,
    const std::unordered_set<std::string>& boot_driver_components_set) {
  auto client_list = MakeClientList(boot_dir, test_pkg_dir, test_resolution_context);

  if (client_list.is_error()) {
    return client_list.take_error();
  }

  return ConstructBootAndBaseConfig(*std::move(client_list), boot_driver_components_set);
}

}  // namespace

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto internal_client = component::Connect<fuchsia_driver_test::Internal>();
  if (internal_client.is_error()) {
    FX_LOG_KV(ERROR, "Failed to connect to internal protocol.");
    return internal_client.status_value();
  }

  fidl::ClientEnd<fuchsia_io::Directory> boot_dir;
  auto boot_result = fidl::WireCall(*internal_client)->GetBootDirectory();
  if (!boot_result.ok() || boot_result->is_error() || !boot_result->value()->boot_dir.is_valid()) {
    zx::result self_pkg = OpenPkgDir();
    if (self_pkg.is_error()) {
      return self_pkg.status_value();
    }
    boot_dir = std::move(self_pkg.value());
  } else {
    boot_dir = std::move(boot_result->value()->boot_dir);
  }

  fidl::ClientEnd<fuchsia_io::Directory> test_pkg_dir;
  std::optional<fuchsia_component_resolution::Context> test_resolution_context;
  auto test_pkg_result = fidl::WireCall(*internal_client)->GetTestPackage();
  if (test_pkg_result.ok() && test_pkg_result->is_ok() &&
      test_pkg_result->value()->test_pkg_dir.is_valid()) {
    test_pkg_dir = std::move(test_pkg_result->value()->test_pkg_dir);

    auto test_res_context_result = fidl::WireCall(*internal_client)->GetTestResolutionContext();
    if (test_res_context_result.ok() && test_res_context_result->is_ok() &&
        test_res_context_result->value()->context) {
      test_resolution_context.emplace(
          fidl::ToNatural(*test_res_context_result->value()->context.get()));
    }
  }

  std::unordered_set<std::string> boot_driver_components_set;
  auto boot_overrides_result = fidl::WireCall(*internal_client)->GetBootDriverOverrides();
  if (boot_overrides_result.ok() && boot_overrides_result->is_ok() &&
      !boot_overrides_result->value()->boot_overrides.empty()) {
    for (auto override : boot_overrides_result->value()->boot_overrides) {
      boot_driver_components_set.emplace(override.get());
    }
  }

  // Only index pkg if only one of boot or pkg was overridden.
  zx::result<BootAndBaseConfigResult> list_setup =
      SetupLists(boot_dir, test_pkg_dir, test_resolution_context, boot_driver_components_set);
  if (list_setup.is_error()) {
    return list_setup.status_value();
  }

  component::OutgoingDirectory outgoing(loop.dispatcher());
  zx::result<> serve_result = outgoing.ServeFromStartupInfo();
  if (serve_result.is_error()) {
    return serve_result.status_value();
  }

  // Server up driver lists.
  zx::result<> add_protocol_result = outgoing.AddProtocol<fuchsia_driver_test::DriverLists>(
      std::make_unique<DriverLists>(std::move(list_setup.value())));

  add_protocol_result = outgoing.AddProtocol<fuchsia_component_resolution::Resolver>(
      std::make_unique<FakeComponentResolver>(std::move(boot_dir), std::move(test_pkg_dir)));
  if (add_protocol_result.is_error()) {
    return add_protocol_result.status_value();
  }

  loop.Run();
  return 0;
}
