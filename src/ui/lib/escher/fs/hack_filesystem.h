// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_LIB_ESCHER_FS_HACK_FILESYSTEM_H_
#define SRC_UI_LIB_ESCHER_FS_HACK_FILESYSTEM_H_

#include <lib/fit/function.h>

#include <functional>
#include <optional>
#include <string>
#include <unordered_map>
#include <unordered_set>

#include "src/lib/fxl/memory/ref_counted.h"

#ifdef __Fuchsia__
#include <fidl/fuchsia.io/cpp/fidl.h>
#endif

namespace escher {

class HackFilesystem;
using HackFilesystemPtr = fxl::RefPtr<HackFilesystem>;
using HackFileContents = std::string;
using HackFilePath = std::string;
using HackFilePathSet = std::unordered_set<HackFilePath>;
class HackFilesystemWatcher;
using HackFilesystemWatcherFunc = fit::function<void(HackFilePath)>;

// An in-memory file system that can be watched for content change.
class HackFilesystem : public fxl::RefCountedThreadSafe<HackFilesystem> {
 public:
  // Instantiate a filesystem.
  //
  // Files will be loaded from the real filesystem, using the specified root directory
  // path.  On Fuchsia the default root is "/pkg/data/"; on Linux, the default is
  // "../test_data/escher", which points to a directory of escher test data relative
  // to the test binary itself.
  //
  // If no files are needed, passing nullptr is OK.  Examples:
  // - a test will be using `WriteFileForTest()`, so doesn't need access to the real filesystem
  // - an application uses Escher with no shaders; e.g. when only `DebugFont` is used.
  static HackFilesystemPtr New(const char* root =
#ifdef __Fuchsia__
                                   "/pkg/data"
#else
                                   "../test_data/escher"
#endif
  );

#ifdef __Fuchsia__
  // Instantiate a filesystem.  Files will be loaded from the provided directory.
  static HackFilesystemPtr New(fidl::ClientEnd<fuchsia_io::Directory> dir);
#endif

  ~HackFilesystem();

  // Return the contents of the file, which can be empty if the path doesn't
  // exist (HackFilesystem doesn't distinguish between empty and non-existent
  // files).
  HackFileContents ReadFile(const HackFilePath& path);

  // The watcher will be notified whenever any of the paths that it cares
  // about change.  To stop watching, simply release the unique_ptr.
  std::unique_ptr<HackFilesystemWatcher> RegisterWatcher(HackFilesystemWatcherFunc func);

  // If the hack file system was initialized with a call to |InitializeWithBasePath|
  // then the member variable base_path_ is set to be the absolute path of the
  // root path that was provided. If the file system was not initialized, then the
  // optional return value will be null.
  const std::optional<std::string>& base_path() const { return base_path_; }

#ifdef __Fuchsia__
  const std::optional<fidl::SyncClient<fuchsia_io::Directory>>& base_dir() const {
    return base_dir_;
  }
#endif

  // Set the file contents and notify watchers of the change.
  void WriteFileForTest(const HackFilePath& path, HackFileContents new_contents) {
    WriteFile(path, std::move(new_contents));
  }

 private:
  HackFilesystem() = default;
  FRIEND_MAKE_REF_COUNTED(HackFilesystem);
  friend class HackFilesystemWatcher;
  friend class fxl::RefCountedThreadSafe<HackFilesystem>;

  // One of `LoadFile()`, `LoadFileAtDir()` is called when a file isn't found in `files_`,
  // to attempt to populate the entry in `files_`.
  static bool LoadFile(HackFilesystem* fs, const HackFilePath& root, const HackFilePath& path);
#ifdef __Fuchsia__
  static bool LoadFileAtDir(HackFilesystem* fs, const HackFilePath& path);
#endif

  // Set the file contents and notify watchers of the change.
  void WriteFile(const HackFilePath& path, HackFileContents new_contents);

  // Notifies all watchers that their watched file has changed.
  void InvalidateFile(const HackFilePath& path);

  // Load the specified files from the real filesystem, given a root directory.
  // On Fuchsia the default root is "/pkg/data/"; on Linux, the default is
  // "../test_data/escher", which points to a directory of escher test data
  // relative to the test binary itself.
  void InitializeWithBasePath(const char* root =
#ifdef __Fuchsia__
                                  "/pkg/data"
#else
                                  "../test_data/"
                                  "escher"
#endif
  );
#ifdef __Fuchsia__
  // Load the specified files from the real filesystem, given a root directory handle.
  void InitializeWithBaseDir(fidl::ClientEnd<fuchsia_io::Directory> dir);
#endif

  std::optional<std::string> base_path_;
#ifdef __Fuchsia__
  std::optional<fidl::SyncClient<fuchsia_io::Directory>> base_dir_;
#endif
  std::unordered_map<HackFilePath, HackFileContents> files_;
  std::unordered_set<HackFilesystemWatcher*> watchers_;
};

// Allows clients to be notified about changes in the specified files.  There is
// no public constructor; instances of HackFilesystemWatcher must be obtained
// via HackFilesystem::RegisterWatcher().
class HackFilesystemWatcher final {
 public:
  ~HackFilesystemWatcher();

  // Start receiving notifications when the file identified by |path| changes.
  void AddPath(HackFilePath path) { paths_to_watch_.insert(std::move(path)); }

  // Read the contents of the specified file, and receive notifications if it
  // subsequently changes.
  HackFileContents ReadFile(const HackFilePath& path) {
    AddPath(path);
    return filesystem_->ReadFile(path);
  }

  // Return true if notifications will be received when |path| changes.
  bool IsWatchingPath(const HackFilePath& path) {
    return paths_to_watch_.find(path) != paths_to_watch_.end();
  }

  // Clear watcher to the default state; no notifications will be received until
  // paths are added by calling AddPath() or ReadFile().
  void ClearPaths() { paths_to_watch_.clear(); }

 private:
  friend class HackFilesystem;

  explicit HackFilesystemWatcher(HackFilesystem* filesystem, HackFilesystemWatcherFunc callback);

  HackFilesystem* const filesystem_;
  HackFilesystemWatcherFunc callback_;
  HackFilePathSet paths_to_watch_;
};

}  // namespace escher

#endif  // SRC_UI_LIB_ESCHER_FS_HACK_FILESYSTEM_H_
