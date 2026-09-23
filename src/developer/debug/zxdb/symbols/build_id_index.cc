// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/build_id_index.h"

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <memory>
#include <set>
#include <string>
#include <system_error>
#include <tuple>
#include <utility>
#include <vector>

#include <rapidjson/document.h>
#include <rapidjson/istreamwrapper.h>

#include "src/developer/debug/shared/logging/logging.h"
#include "src/lib/elflib/elflib.h"
#include "src/lib/files/glob.h"
#include "src/lib/fxl/strings/trim.h"

namespace zxdb {

BuildIDIndex::Entry BuildIDIndex::EntryForBuildID(const std::string& build_id) {
  EnsureCacheClean();

  if (!build_id_to_files_.contains(build_id))
    SearchBuildIdDirs(build_id);

  // No matter whether SearchBuildIdDirs found the symbol or not, build_id_to_files_[build_id] will
  // always create the entry so next time no search will be performed.
  Entry found = build_id_to_files_[build_id];
  if (found.build_dir.empty()) {
    DEBUG_LOG(BuildIDIndex) << "Entry for " << build_id << " has no build dir.";
  }
  if (found.debug_info.empty()) {
    DEBUG_LOG(BuildIDIndex) << "Entry for " << build_id << " has no debug info.";
  }
  if (found.binary.empty()) {
    DEBUG_LOG(BuildIDIndex) << "Entry for " << build_id << " has no binary.";
  }
  return found;
}

void BuildIDIndex::SearchBuildIdDirs(const std::string& build_id) {
  if (build_id.size() <= 2) {
    return;
  }

  auto path = build_id.substr(0, 2) + "/" + build_id.substr(2);

  for (const auto& build_id_dir : build_id_dirs_) {
    // There are potentially two files, one with just the build ID, one with a ".debug" suffix. The
    // ".debug" suffix one is supposed to contain either just the DWARF symbols, or the full
    // unstripped binary. The plain one is supposed to be either a stripped or unstripped binary.
    //
    // Since we're looking for DWARF information, look in the ".debug" one first.
    IndexSourceFile(build_id_dir.path + "/" + path + ".debug", build_id_dir.build_dir);
    IndexSourceFile(build_id_dir.path + "/" + path, build_id_dir.build_dir);
  }
}

void BuildIDIndex::AddBuildIDMappingForTest(const std::string& build_id,
                                            const std::string& file_name) {
  // This map saves the manual mapping across cache updates.
  manual_mappings_[build_id].debug_info = file_name;
  manual_mappings_[build_id].binary = file_name;
  // Don't bother marking the cache dirty since we can just add it.
  build_id_to_files_[build_id].debug_info = file_name;
  build_id_to_files_[build_id].binary = file_name;
}

void BuildIDIndex::ClearAll() {
  DEBUG_LOG(BuildIDIndex) << "Clearing build id index.";
  ids_txts_.clear();
  build_id_dirs_.clear();
  sources_.clear();
  ClearCache();
}

bool BuildIDIndex::AddOneFile(const std::string& file_name) {
  return IndexSourceFile(file_name, "", true);
}

void BuildIDIndex::AddIdsTxt(const std::string& ids_txt, const std::string& build_dir) {
  // If the file is already loaded, ignore it.
  if (std::ranges::any_of(ids_txts_, [&ids_txt](const IdsTxt& it) { return it.path == ids_txt; }))
    return;

  DEBUG_LOG(BuildIDIndex) << "Adding ids.txt " << ids_txt << " for build id dir " << build_dir
                          << ".";
  ids_txts_.push_back({.path = ids_txt, .build_dir = build_dir});
  ClearCache();
}

void BuildIDIndex::AddBuildIdDir(const std::string& dir, const std::string& build_dir) {
  if (std::ranges::any_of(build_id_dirs_, [&dir](const BuildIdDir& it) { return it.path == dir; }))
    return;

  DEBUG_LOG(BuildIDIndex) << "Adding build id dir " << build_dir << " for path " << dir << ".";
  build_id_dirs_.push_back({.path = dir, .build_dir = build_dir});
  ClearCache();
}

void BuildIDIndex::AddSymbolServer(const std::string& url, bool require_authentication) {
  if (std::ranges::any_of(symbol_servers_,
                          [&url](const SymbolServer& it) { return it.url == url; }))
    return;

  if (require_authentication) {
    DEBUG_LOG(BuildIDIndex) << "Adding private symbol server " << url << ".";
  } else {
    DEBUG_LOG(BuildIDIndex) << "Adding public symbol server " << url << ".";
  }
  symbol_servers_.push_back({.url = url, .require_authentication = require_authentication});
}

void BuildIDIndex::SetCacheDir(const std::string& cache_dir) {
  DEBUG_LOG(BuildIDIndex) << "Setting symbol cache dir to " << cache_dir << ".";
  AddBuildIdDir(cache_dir);
  cache_dir_ = std::make_unique<CacheDir>(cache_dir);
}

void BuildIDIndex::AddSymbolIndex(const std::string& contents,
                                  const std::filesystem::path& relative_to) {
  rapidjson::Document document;
  document.Parse(contents);
  if (document.HasParseError() || !document.IsObject()) {
    LOGS(Warn) << "Provided string is not a valid symbol-index.json";
    return;
  }

  DEBUG_LOG(BuildIDIndex) << "Loading symbol index from string contents.";
  std::vector<std::filesystem::path> files_to_load;
  LoadSymbolIndexDocument(document, relative_to, files_to_load);

  std::set<std::filesystem::path> visited;
  LoadIndexFilesFromQueue(files_to_load, visited);
}

void BuildIDIndex::AddSymbolIndexFile(const std::string& path) {
  std::vector<std::filesystem::path> files_to_load = {path};
  std::set<std::filesystem::path> visited;
  LoadIndexFilesFromQueue(files_to_load, visited);
}

void BuildIDIndex::AddPlainFileOrDir(const std::string& path) {
  if (std::ranges::contains(sources_, path))
    return;

  sources_.push_back(path);
  ClearCache();
}

BuildIDIndex::StatusList BuildIDIndex::GetStatus() {
  EnsureCacheClean();
  return status_;
}

void BuildIDIndex::ClearCache() { cache_dirty_ = true; }

// static
int BuildIDIndex::ParseIDs(const std::string& input, const std::filesystem::path& containing_dir,
                           const std::string& build_dir, BuildIDMap* output) {
  int added = 0;
  for (size_t line_begin = 0; line_begin < input.size(); line_begin++) {
    size_t newline = input.find('\n', line_begin);
    if (newline == std::string::npos)
      newline = input.size();

    std::string_view line(&input[line_begin], newline - line_begin);
    if (!line.empty()) {
      // Format is <buildid> <space> <filename>.
      size_t first_space = line.find(' ');
      if (first_space != std::string::npos && first_space > 0 && first_space + 1 < line.size()) {
        // There is a space and it separates two nonempty things.
        std::string_view to_trim(" \t\r\n");
        std::string_view build_id = fxl::TrimString(line.substr(0, first_space), to_trim);
        std::string_view path_data =
            fxl::TrimString(line.substr(first_space + 1, line.size() - first_space - 1), to_trim);

        std::filesystem::path path(path_data);

        if (path.is_relative()) {
          path = containing_dir / path;
        }

        BuildIDIndex::Entry entry;
        // Assume the file contains both debug info and program bits.
        entry.debug_info = path;
        entry.binary = path;
        entry.build_dir = build_dir;

        added++;
        output->emplace(std::piecewise_construct,
                        std::forward_as_tuple(build_id.data(), build_id.size()),
                        std::forward_as_tuple(entry));
      }
    }

    line_begin = newline;  // The for loop will advance past this.
  }
  return added;
}

void BuildIDIndex::LoadIdsTxt(const IdsTxt& ids_txt) {
  std::error_code err;

  auto path = std::filesystem::canonical(ids_txt.path, err);

  if (err) {
    status_.emplace_back(ids_txt.path, 0);
    LOGS(Warn) << "Can't open build ID file: " << ids_txt.path;
    return;
  }

  auto containing_dir = path.parent_path();

  FILE* id_file = fopen(ids_txt.path.c_str(), "r");
  if (!id_file) {
    status_.emplace_back(ids_txt.path, 0);
    LOGS(Warn) << "Can't open build ID file: " << ids_txt.path;
    return;
  }

  fseek(id_file, 0, SEEK_END);
  long length = ftell(id_file);
  if (length <= 0) {
    status_.emplace_back(ids_txt.path, 0);
    LOGS(Warn) << "Can't load build ID file: " << ids_txt.path;
    return;
  }

  std::string contents;
  contents.resize(length);

  fseek(id_file, 0, SEEK_SET);
  if (fread(contents.data(), 1, contents.size(), id_file) != static_cast<size_t>(length)) {
    status_.emplace_back(ids_txt.path, 0);
    LOGS(Warn) << "Can't read build ID file: " << ids_txt.path;
    return;
  }

  fclose(id_file);

  int added = ParseIDs(contents, containing_dir, ids_txt.build_dir, &build_id_to_files_);
  status_.emplace_back(ids_txt.path, added);
  if (!added)
    LOGS(Warn) << "No mappings found in build ID file: " << ids_txt.path;
}

void BuildIDIndex::LoadIndexFilesFromQueue(std::vector<std::filesystem::path>& files_to_load,
                                           std::set<std::filesystem::path>& visited) {
  while (!files_to_load.empty()) {
    std::filesystem::path file_name = files_to_load.back();
    files_to_load.pop_back();
    DEBUG_LOG(BuildIDIndex) << "Loading symbol index file " << file_name << "...";

    // Avoid recursive includes.
    if (visited.contains(file_name)) {
      continue;
    }
    visited.insert(file_name);

    std::ifstream file(file_name);
    if (!file) {
      LOGS(Warn) << "Can't open " << file_name;
      continue;
    }

    rapidjson::IStreamWrapper input_stream(file);
    rapidjson::Document document;
    document.ParseStream(input_stream);
    if (document.HasParseError() || !document.IsObject()) {
      LOGS(Warn) << file_name << " is not a valid symbol-index.json";
      continue;
    }

    LoadSymbolIndexDocument(document, file_name.parent_path(), files_to_load);
  }
}

void BuildIDIndex::LoadSymbolIndexDocument(const rapidjson::Document& document,
                                           const std::filesystem::path& relative_to,
                                           std::vector<std::filesystem::path>& queue) {
  auto resolve_path = [relative_to](const char* path) {
    // "/abc/def/..".lexically_normal() => "/abc/", while we want "/abc"
    auto res = (relative_to / path).lexically_normal();
    if (!res.has_filename()) {
      res = res.parent_path();
    }
    return res;
  };

  if (document.HasMember("includes") && document["includes"].IsArray()) {
    for (auto& value : document["includes"].GetArray()) {
      if (value.IsString() && strlen(value.GetString())) {
        for (auto path : files::Glob(resolve_path(value.GetString()))) {
          queue.push_back(path);
        }
      }
    }
  }

  if (document.HasMember("build_id_dirs") && document["build_id_dirs"].IsArray()) {
    for (auto& value : document["build_id_dirs"].GetArray()) {
      if (value.IsObject() && value.HasMember("path") && value["path"].IsString() &&
          strlen(value["path"].GetString())) {
        std::string build_dir;
        if (value.HasMember("build_dir") && value["build_dir"].IsString()) {
          build_dir = resolve_path(value["build_dir"].GetString());
        }
        for (auto path : files::Glob(resolve_path(value["path"].GetString()))) {
          AddBuildIdDir(path, build_dir);
        }
      }
    }
  }

  if (document.HasMember("ids_txts") && document["ids_txts"].IsArray()) {
    for (auto& value : document["ids_txts"].GetArray()) {
      if (value.IsObject() && value.HasMember("path") && value["path"].IsString() &&
          strlen(value["path"].GetString())) {
        std::string build_dir;
        if (value.HasMember("build_dir") && value["build_dir"].IsString()) {
          build_dir = resolve_path(value["build_dir"].GetString());
        }
        for (auto path : files::Glob(resolve_path(value["path"].GetString()))) {
          AddIdsTxt(path, build_dir);
        }
      }
    }
  }

  if (document.HasMember("gcs_flat") && document["gcs_flat"].IsArray()) {
    for (auto& value : document["gcs_flat"].GetArray()) {
      if (value.IsObject() && value.HasMember("url") && value["url"].IsString() &&
          strlen(value["url"].GetString())) {
        bool require_authentication = false;
        if (value.HasMember("require_authentication") && value["require_authentication"].IsBool()) {
          require_authentication = value["require_authentication"].GetBool();
        }
        AddSymbolServer(value["url"].GetString(), require_authentication);
      }
    }
  }

  if (document.HasMember("debuginfod") && document["debuginfod"].IsArray()) {
    for (auto& value : document["debuginfod"].GetArray()) {
      if (value.IsObject() && value.HasMember("url") && value["url"].IsString() &&
          strlen(value["url"].GetString())) {
        bool require_authentication = false;
        if (value.HasMember("require_authentication") && value["require_authentication"].IsBool()) {
          require_authentication = value["require_authentication"].GetBool();
        }
        AddSymbolServer(value["url"].GetString(), require_authentication);
      }
    }
  }
}

void BuildIDIndex::IndexSourcePath(const std::string& path) {
  std::error_code ec;
  if (std::filesystem::is_directory(path, ec)) {
    // Iterate through all files in this directory, but don't recurse.
    int indexed = 0;
    for (const auto& child : std::filesystem::directory_iterator(path, ec)) {
      if (IndexSourceFile(child.path()))
        indexed++;
    }

    status_.emplace_back(path, indexed);
  } else if (!ec && IndexSourceFile(path)) {
    status_.emplace_back(path, 1);
  } else {
    status_.emplace_back(path, 0);
    LOGS(Warn) << "Symbol file could not be loaded: " << path;
  }
}

bool BuildIDIndex::IndexSourceFile(const std::string& file_path, const std::string& build_dir,
                                   bool preserve) {
  DEBUG_LOG(BuildIDIndex) << "Indexing source file " << file_path << ".";
  auto file = fopen(file_path.c_str(), "r");
  if (!file) {
    DEBUG_LOG(BuildIDIndex) << "Couldn't open " << file_path << ": " << strerror(errno);
    return false;
  }
  auto elf = elflib::ElfLib::Create(file, elflib::ElfLib::Ownership::kTakeOwnership);
  if (!elf) {
    DEBUG_LOG(BuildIDIndex) << "Couldn't parse ELF file from " << file_path;
    return false;
  }

  std::string build_id = elf->GetGNUBuildID();
  if (build_id.empty()) {
    DEBUG_LOG(BuildIDIndex) << "ELF file " << file_path << " did not have a build ID.";
    return false;
  }

  DEBUG_LOG(BuildIDIndex) << "Source file " << file_path << " has build id " << build_id << ".";

  if (cache_dir_)
    cache_dir_->NotifyFileAccess(file_path);

  auto ret = false;
  if (elf->ProbeHasDebugInfo() && build_id_to_files_[build_id].debug_info.empty()) {
    build_id_to_files_[build_id].debug_info = file_path;
    ret = true;
  }
  if (elf->ProbeHasProgramBits() && build_id_to_files_[build_id].binary.empty()) {
    build_id_to_files_[build_id].binary = file_path;
    ret = true;
  }

  if (ret && !build_dir.empty()) {
    build_id_to_files_[build_id].build_dir = build_dir;
  }

  if (ret && preserve) {
    manual_mappings_[build_id] = build_id_to_files_[build_id];
  }

  return ret;
}

void BuildIDIndex::EnsureCacheClean() {
  if (!cache_dirty_)
    return;

  status_.clear();
  build_id_to_files_ = manual_mappings_;

  for (const auto& source : sources_)
    IndexSourcePath(source);

  for (const auto& ids_txt : ids_txts_)
    LoadIdsTxt(ids_txt);

  for (const auto& build_id_dir : build_id_dirs_)
    status_.emplace_back(build_id_dir.path, BuildIDIndex::kStatusIsFolder);

  cache_dirty_ = false;
}

}  // namespace zxdb
