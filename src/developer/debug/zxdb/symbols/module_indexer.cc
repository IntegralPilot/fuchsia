// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/module_indexer.h"

#include <condition_variable>
#include <filesystem>
#include <mutex>

#include "src/developer/debug/zxdb/common/worker_pool.h"
#include "src/developer/debug/zxdb/symbols/compile_unit.h"
#include "src/developer/debug/zxdb/symbols/index.h"

namespace zxdb {

namespace {

class ModuleIndexer {
 public:
  ModuleIndexer(fxl::WeakPtr<ModuleSymbols> module_symbols, DwarfBinary& binary,
                const std::string& build_dir)
      : weak_module_symbols_(std::move(module_symbols)),
        binary_(binary),
        build_dir_(build_dir),
        index_(std::make_unique<Index>()) {}

  ModuleIndexerResult GetResult();

  void IndexMainBinary();
  void IndexDwos();

  // Loads the DwoInfo (returning it on success) and adds the file's data to the index.
  std::unique_ptr<DwoInfo> IndexOneDwo(size_t skeleton_index, const SkeletonUnit& skeleton);

 private:
  std::string DwoPathForSkeleton(const SkeletonUnit& skeleton) const;

  // State from constructor.
  fxl::WeakPtr<ModuleSymbols> weak_module_symbols_;
  DwarfBinary& binary_;
  const std::string build_dir_;

  std::mutex index_mutex_;
  std::unique_ptr<Index> index_;  // Guarded by index_mutex_.

  std::mutex state_mutex_;
  ModuleIndexerResult::DwoInfoVector dwo_info_;  // Guarded by state_mutex_.
};

ModuleIndexerResult ModuleIndexer::GetResult() {
  return ModuleIndexerResult(std::move(index_), std::move(dwo_info_));
}

void ModuleIndexer::IndexMainBinary() {
  std::unique_lock<std::mutex> lock(index_mutex_);
  index_->CreateIndex(binary_, IndexNode::SymbolRef::kMainBinary);
}

void ModuleIndexer::IndexDwos() {
  const auto& dwo_refs = index_->dwo_refs();
  if (dwo_refs.empty())
    return;  // No .dwo files to index.

  // Six worker threads was picked as the upper bound for performance by measuring indexing Chromium
  // symbols. Since merging the indices is synchronized, we don't get any benefit once the number of
  // worker threads exceeds 1/(fraction of time spent merging indices).
  WorkerPool pool(6);

  std::mutex work_mutex;

  // Collects the DwoInfos we made. These are collected separately in this local var and then moved
  // to dwo_info_ member to make the locking requirements more clear.
  ModuleIndexerResult::DwoInfoVector collected_dwo_info;  // Guarded by work_mutex.
  collected_dwo_info.resize(dwo_refs.size());

  size_t work_count = dwo_refs.size();  // Remaining work. Guarded by work_mutex.
  std::condition_variable completion;   // Guarded by work_mutex.

  // Schedule loading the .dwo files on the worker pool.
  for (size_t skeleton_index = 0; skeleton_index < dwo_refs.size(); skeleton_index++) {
    // Capture skeleton_index by copy since we want the value from the current loop iteration.
    pool.PostTask([this, &work_mutex, &work_count, &completion, &dwo_refs, skeleton_index,
                   &collected_dwo_info]() {
      std::unique_ptr<DwoInfo> info = IndexOneDwo(skeleton_index, dwo_refs[skeleton_index]);

      // Save the info and update the tracking information inside the lock.
      bool signal_completion = false;
      {
        std::unique_lock<std::mutex> lock(work_mutex);
        collected_dwo_info[skeleton_index] = std::move(info);

        work_count--;
        signal_completion = (work_count == 0);
      }

      // Signal completion outside of the lock.
      if (signal_completion) {
        completion.notify_all();
      }
    });
  }

  // Wait for worker pool to complete.
  std::unique_lock<std::mutex> completion_lock(work_mutex);
  completion.wait(completion_lock, [&]() { return work_count == 0; });

  // Save the collected DWO references.
  dwo_info_ = std::move(collected_dwo_info);
}

std::unique_ptr<DwoInfo> ModuleIndexer::IndexOneDwo(size_t skeleton_index,
                                                    const SkeletonUnit& skeleton) {
  auto dwo_info = std::make_unique<DwoInfo>(skeleton, weak_module_symbols_);
  std::string dwo_path = DwoPathForSkeleton(skeleton);

  if (Err err = dwo_info->Load(dwo_path, dwo_path); err.has_error()) {
    // TODO(brettw) have a better way to report this error/warning to the frontend.
    fprintf(stderr, "Can't load DWO binary %s\n", dwo_path.c_str());
    return nullptr;
  }

  Index dwo_index;
  dwo_index.CreateIndex(*dwo_info->binary(), static_cast<int32_t>(skeleton_index));

  // Merge into the main index.
  {
    std::unique_lock<std::mutex> lock(index_mutex_);
    index_->root().MergeFrom(dwo_index.root());
  }

  return dwo_info;
}

std::string ModuleIndexer::DwoPathForSkeleton(const SkeletonUnit& skeleton) const {
  std::filesystem::path dwo_path;
  std::filesystem::path comp_dir = skeleton.comp_dir;
  if (comp_dir.is_absolute()) {
    return (comp_dir / skeleton.dwo_name).string();
  }

  // When the compilation directories are relative, use our notion of the build dir as the
  // "current directory."
  return (std::filesystem::path(build_dir_) / comp_dir / skeleton.dwo_name).string();
}

}  // namespace

ModuleIndexerResult IndexModule(fxl::WeakPtr<ModuleSymbols> module_symbols, DwarfBinary& binary,
                                const std::string& build_dir) {
  ModuleIndexer indexer(module_symbols, binary, build_dir);

  // This must be done first to discover all of the .dwo references in the main binary.
  indexer.IndexMainBinary();

  // Next index all of the .dwo references that were discovered.
  indexer.IndexDwos();

  // Clear the LLVM cache to significantly reduce the amount of duplicated caches we carry.
  binary.ClearLLVMCache();

  return indexer.GetResult();
}

}  // namespace zxdb
