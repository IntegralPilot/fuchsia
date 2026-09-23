// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/bin/vulkan_loader/icd_component.h"

#include <fidl/fuchsia.component.decl/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>

#include <fbl/unique_fd.h>

#include "rapidjson/prettywriter.h"
#include "rapidjson/schema.h"
#include "src/graphics/bin/vulkan_loader/app.h"
#include "src/lib/files/file.h"
#include "src/lib/json_parser/json_parser.h"
#include "src/lib/json_parser/pretty_print.h"

namespace {

const char* kSchema = R"(
{
 "$schema": "http://json-schema.org/schema#",
 "type": "object",
 "properties": {
   "version": {"type":"number", "maximum": 1, "minimum": 1},
   "file_path": {"type":"string"},
   "manifest_path": {"type":"string"}
 },
 "required": ["version", "file_path", "manifest_path"]
}
)";

const char* kManifestSchema = R"(
{
  "$schema":"http://json-schema.org/schema#",
  "type":"object",
  "properties":{
    "file_format_version":{
      "type":"string"
    },
    "ICD":{
      "type":"object",
      "properties":{
        "library_path":{
          "type":"string"
        },
        "api_version":{
          "type":"string"
        }
      },
      "required":[
        "library_path",
        "api_version"
      ]
    }
  },
  "required":[
    "file_format_version",
    "ICD"
  ]
}
)";

const char* kCollectionName = "icd-loaders";

}  // namespace

IcdComponent::IcdComponent(LoaderApp* app, fidl::ClientEnd<fuchsia_component::Realm> realm,
                           std::string component_url)
    : app_(app),
      realm_(std::move(realm), app->dispatcher()),
      component_url_(std::move(component_url)) {}

IcdComponent::~IcdComponent() {
  RemoveManifestFromFs();
  if (realm_ && !child_instance_name_.empty()) {
    fuchsia_component_decl::wire::ChildRef child_ref{
        .name = fidl::StringView::FromExternal(child_instance_name_),
        .collection = fidl::StringView::FromExternal(kCollectionName),
    };
    auto result = realm_.sync()->DestroyChild(child_ref);
    if (!result.ok()) {
      FX_LOGS(ERROR) << child_instance_name_ << " DestroyChild transport error: " << result;
    } else if (result->is_error()) {
      FX_LOGS(ERROR) << child_instance_name_
                     << " DestroyChild error: " << static_cast<uint32_t>(result->error_value());
    }
  }
}

void IcdComponent::AddManifestToFs() {
  assert(manifest_file_);
  std::optional<std::string> manifest_file_name = GetManifestFileName();
  assert(manifest_file_name);
  app_->manifest_fs_root_node()->AddEntry(*manifest_file_name, manifest_file_);
}

void IcdComponent::RemoveManifestFromFs() {
  if (!manifest_file_)
    return;
  std::optional<std::string> manifest_file_name = GetManifestFileName();
  if (manifest_file_name) {
    app_->manifest_fs_root_node()->RemoveEntry(*manifest_file_name, manifest_file_.get());
  }
}

void IcdComponent::Initialize(inspect::Node* parent_node) {
  static uint64_t name_id;
  auto pending_action_token = app_->GetPendingActionToken();

  child_instance_name_ = std::to_string(name_id++);
  node_ = parent_node->CreateChild(child_instance_name_);
  initialization_status_ = node_.CreateString("status", "uninitialized");
  node_.CreateString("component_url", component_url_, &value_list_);
  fidl::Arena arena;
  fuchsia_component_decl::wire::CollectionRef collection{
      .name = fidl::StringView::FromExternal(kCollectionName),
  };
  fuchsia_component_decl::wire::Child decl =
      fuchsia_component_decl::wire::Child::Builder(arena)
          .name(child_instance_name_)
          .url(component_url_)
          .startup(fuchsia_component_decl::wire::StartupMode::kLazy)
          .Build();

  auto failure_callback =
      fit::defer_callback([this, pending_action_token = std::move(pending_action_token)]() {
        std::lock_guard lock(vmo_lock_);
        stage_ = LookupStages::kFailed;
        app_->NotifyIcdsChanged();
      });

  realm_->CreateChild(collection, decl, fuchsia_component::wire::CreateChildArgs())
      .Then([this, failure_callback = std::move(failure_callback)](
                fidl::WireUnownedResult<fuchsia_component::Realm::CreateChild>& result) mutable {
        if (!result.ok()) {
          FX_LOGS(ERROR) << component_url_ << " CreateChild transport error: " << result;
          node_.CreateUint("create_response", static_cast<uint32_t>(result.status()), &value_list_);
          child_instance_name_ = "";
          return;
        }
        if (result->is_error()) {
          FX_LOGS(ERROR) << component_url_
                         << " CreateChild error: " << static_cast<uint32_t>(result->error_value());
          node_.CreateUint("create_response", static_cast<uint32_t>(result->error_value()),
                           &value_list_);
          child_instance_name_ = "";
          return;
        }

        initialization_status_.Set("created");

        fuchsia_component_decl::wire::ChildRef child_ref{
            .name = fidl::StringView::FromExternal(child_instance_name_),
            .collection = fidl::StringView::FromExternal(kCollectionName),
        };

        auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
        if (endpoints.is_error()) {
          FX_LOGS(ERROR) << "Failed to create endpoints: " << endpoints.status_string();
          return;
        }

        realm_->OpenExposedDir(child_ref, std::move(endpoints->server))
            .Then([this, client_end = std::move(endpoints->client),
                   failure_callback = std::move(failure_callback)](
                      fidl::WireUnownedResult<fuchsia_component::Realm::OpenExposedDir>&
                          result) mutable {
              if (!result.ok()) {
                FX_LOGS(ERROR) << component_url_ << " OpenExposedDir transport error: " << result;
                node_.CreateUint("bind_response", static_cast<uint32_t>(result.status()),
                                 &value_list_);
                return;
              }
              if (result->is_error()) {
                FX_LOGS(ERROR) << component_url_ << " OpenExposedDir error: "
                               << static_cast<uint32_t>(result->error_value());
                node_.CreateUint("bind_response", static_cast<uint32_t>(result->error_value()),
                                 &value_list_);
                return;
              }

              initialization_status_.Set("bound");
              async::PostTask(
                  app_->fdio_loop_dispatcher(), [this, shared_this = this->shared_from_this(),
                                                 failure_callback = std::move(failure_callback),
                                                 directory = std::move(client_end)]() mutable {
                    ReadFromComponent(std::move(failure_callback), std::move(directory));
                  });
            });
      });
}

std::optional<std::string> IcdComponent::ReadManifest(int contents_dir_fd,
                                                      const std::string& manifest_path) {
  std::string manifest_result;
  if (!files::ReadFileToStringAt(contents_dir_fd, manifest_path, &manifest_result)) {
    FX_LOGS(ERROR) << component_url_ << " Failed to read manifest path " << manifest_path;
    return {};
  }
  json_parser::JSONParser manifest_parser;
  auto manifest_doc = manifest_parser.ParseFromString(manifest_result, manifest_path);
  if (manifest_parser.HasError()) {
    FX_LOGS(ERROR) << component_url_ << " JSON parser had error " << manifest_parser.error_str();
    return {};
  }
  if (!ValidateManifestJson(component_url_, manifest_doc)) {
    return {};
  }

  // Update library_path in manifest with a unique name.
  auto& library_path_node = manifest_doc["ICD"].GetObject()["library_path"];
  std::string library_path = library_path_node.GetString();
  library_path = child_instance_name_ + "-" + library_path;
  node_.CreateString("library_path", library_path, &value_list_);
  library_path_node.SetString(library_path.c_str(), manifest_doc.GetAllocator());

  manifest_result = json_parser::JsonValueToPrettyString(manifest_doc);

  node_.CreateString("manifest_contents", manifest_result, &value_list_);
  manifest_file_ =
      fbl::MakeRefCounted<fs::BufferedPseudoFile>([manifest_result](fbl::String* out_string) {
        *out_string = manifest_result.c_str();
        return ZX_OK;
      });
  return library_path;
}

// static
bool IcdComponent::ValidateMetadataJson(const std::string& component_url,
                                        const rapidjson::GenericDocument<rapidjson::UTF8<>>& doc) {
  rapidjson::Document schema_doc;
  schema_doc.Parse(kSchema);
  FX_CHECK(!schema_doc.HasParseError()) << schema_doc.GetParseError();

  rapidjson::SchemaDocument schema(schema_doc);
  rapidjson::SchemaValidator validator(schema);
  if (!doc.Accept(validator)) {
    rapidjson::StringBuffer sb;
    rapidjson::PrettyWriter<rapidjson::StringBuffer> w(sb);
    validator.GetError().Accept(w);
    FX_LOGS(ERROR) << component_url << " metadata.json failed validation " << sb.GetString();
    return false;
  }
  return true;
}
bool IcdComponent::ValidateManifestJson(const std::string& component_url,
                                        const rapidjson::GenericDocument<rapidjson::UTF8<>>& doc) {
  rapidjson::Document schema_doc;
  schema_doc.Parse(kManifestSchema);
  FX_CHECK(!schema_doc.HasParseError()) << schema_doc.GetParseError();

  rapidjson::SchemaDocument schema(schema_doc);
  rapidjson::SchemaValidator validator(schema);
  if (!doc.Accept(validator)) {
    rapidjson::StringBuffer sb;
    rapidjson::PrettyWriter<rapidjson::StringBuffer> w(sb);
    validator.GetError().Accept(w);
    FX_LOGS(ERROR) << component_url << " manifest.json failed validation " << sb.GetString();
    return false;
  }
  return true;
}

zx::result<zx::vmo> IcdComponent::CloneVmo() const {
  std::lock_guard lock(vmo_lock_);
  if (!vmo_info_)
    return zx::error(ZX_ERR_BAD_STATE);

  uint64_t size;
  zx_status_t status = vmo_info_->vmo.get_size(&size);
  if (status != ZX_OK)
    return zx::error(status);
  zx::vmo vmo;
  // Snapshot is ok because we never modify our VMO, and blobfs should never modify it either. We
  // use ZX_VMO_CHILD_NO_WRITE because otherwise ZX_RIGHT_EXECUTE is removed.
  status = vmo_info_->vmo.create_child(
      ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE | ZX_VMO_CHILD_NO_WRITE, 0, size, &vmo);
  if (status != ZX_OK)
    return zx::error(status);
  return zx::ok(std::move(vmo));
}

// See the accompanying README.md for a description of what a Vulkan component needs to have.
void IcdComponent::ReadFromComponent(fit::deferred_callback failure_callback,
                                     fidl::ClientEnd<fuchsia_io::Directory> out_dir) {
  initialization_status_.Set("reading from package");
  auto metadata_endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (metadata_endpoints.is_error()) {
    FX_LOGS(ERROR) << "Failed to create endpoints: " << metadata_endpoints.status_string();
    return;
  }

  zx_status_t status = fdio_open3_at(out_dir.channel().get(), "metadata",
                                     static_cast<uint64_t>(fuchsia_io::kPermReadable),
                                     metadata_endpoints->server.TakeChannel().release());
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << component_url_ << " Failed opening metadata dir";
    return;
  }
  auto contents_endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (contents_endpoints.is_error()) {
    FX_LOGS(ERROR) << "Failed to create endpoints: " << contents_endpoints.status_string();
    return;
  }
  status =
      fdio_open3_at(out_dir.channel().get(), "contents",
                    static_cast<uint64_t>(fuchsia_io::kPermReadable | fuchsia_io::kPermExecutable),
                    contents_endpoints->server.TakeChannel().release());
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << component_url_ << " Failed opening pkg dir";
    return;
  }
  fbl::unique_fd metadata_dir_fd;
  status = fdio_fd_create(metadata_endpoints->client.TakeChannel().release(),
                          metadata_dir_fd.reset_and_get_address());
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << component_url_ << " Failed creating FD for metadata";
    return;
  }

  json_parser::JSONParser parser;
  auto doc = parser.ParseFromFileAt(metadata_dir_fd.get(), "metadata.json");
  if (parser.HasError()) {
    FX_LOGS(ERROR) << component_url_ << " JSON parser had error " << parser.error_str();
    return;
  }

  fbl::unique_fd contents_dir_fd;
  status = fdio_fd_create(contents_endpoints->client.TakeChannel().release(),
                          contents_dir_fd.reset_and_get_address());
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << component_url_ << " Failed creating FD";
    return;
  }
  if (!ValidateMetadataJson(component_url_, doc)) {
    return;
  }
  node_.CreateUint("version", doc["version"].GetInt(), &value_list_);
  std::string file_path = doc["file_path"].GetString();
  node_.CreateString("file_path", file_path, &value_list_);
  initialization_status_.Set("opening manifest");
  std::string manifest_path = doc["manifest_path"].GetString();
  std::optional<std::string> library_path_option =
      ReadManifest(contents_dir_fd.get(), manifest_path);
  if (!library_path_option) {
    return;
  }

  // Manifest file will be added to the filesystem in IcdList::UpdateCurrentComponent.

  fbl::unique_fd fd;

  initialization_status_.Set("opening VMO");
  status = fdio_open3_fd_at(
      contents_dir_fd.get(), file_path.c_str(),
      static_cast<uint64_t>(fuchsia_io::kPermReadable | fuchsia_io::kPermExecutable),
      fd.reset_and_get_address());
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << component_url_ << " Could not open path " << file_path;
    return;
  }
  zx::vmo vmo;
  status = fdio_get_vmo_exec(fd.get(), vmo.reset_and_get_address());
  fd.reset();
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << component_url_ << " Could not clone vmo exec";
    return;
  }
  // Create another pending action token to keep everything alive until we're done initializing
  // the data.
  auto pending_action_token = app_->GetPendingActionToken();
  VmoInfo info;
  info.library_path = *library_path_option;
  info.vmo = std::move(vmo);
  {
    std::lock_guard lock(vmo_lock_);
    vmo_info_ = std::move(info);
    failure_callback.cancel();
    stage_ = LookupStages::kFinished;
  }
  app_->NotifyIcdsChanged();
  initialization_status_.Set("initialized");
}
