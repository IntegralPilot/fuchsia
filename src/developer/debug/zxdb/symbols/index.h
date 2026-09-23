// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_INDEX_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_INDEX_H_

#include <iosfwd>
#include <map>
#include <string>
#include <string_view>

#include "src/developer/debug/zxdb/symbols/identifier.h"
#include "src/developer/debug/zxdb/symbols/index_node.h"
#include "src/developer/debug/zxdb/symbols/skeleton_unit.h"
#include "src/developer/debug/zxdb/symbols/unit_index.h"
#include "src/lib/fxl/macros.h"

namespace llvm {

class DWARFCompileUnit;
class DWARFDie;

namespace object {
class ObjectFile;
}  // namespace object

}  // namespace llvm

namespace zxdb {

class DwarfBinary;
class DwarfUnit;

class Index {
 public:
  Index() = default;
  ~Index() = default;

  // Returns the information on the .dwo files that are referenced by this binary.
  const std::vector<SkeletonUnit>& dwo_refs() const { return dwo_refs_; }

  // Creates an index for one binary file. If there are skeleton units (references to separate .dwo
  // files with the symbols for individual source files), those are collected in dwo_refs() and NOT
  // indexed (the ModuleSymbols needs to have the ownership of the binaries so needs to create
  // these). They should be separately indexed and merged into this index.
  //
  // Normal callers will want to use the fast path (which internally falls back to the slow path
  // for cross unit references). Tests can set the force_slow_path flag to cause everything to be
  // indexed with the slow path for validation purposes.
  void CreateIndex(DwarfBinary& binary, int32_t dwo_index, bool force_slow_path = false);

  // Dumps the file index to the stream for debugging.
  void DumpFileIndex(std::ostream& out) const;

  // Takes a fully-qualified name with namespaces and classes and template parameters and returns
  // the list of symbols which match exactly.
  //
  // TODO(bug 36754) it would be nice if this could be deleted and all code go through
  // expr/find_name.h to query the index. As-is this duplicates some of FindName's logic in a less
  // flexible way.
  std::vector<IndexNode::SymbolRef> FindExact(const Identifier& input) const;

  // Looks up the name in the file index and returns the set of matches. The name is matched from
  // the right side with a left boundary of either a slash or the beginning of the full path. This
  // may match more than one file name, and the caller is left to decide which one(s) it wants.
  std::vector<std::string> FindFileMatches(std::string_view name) const;

  // Same as FindFileMatches but does a prefix search. This only matches the file name component
  // (not directory paths).
  //
  // In the future it would be nice to match directories if there was a "/".
  std::vector<std::string> FindFilePrefixes(const std::string& prefix) const;

  // Looks up the given exact file path and returns all compile units it appears in. The file must
  // be an exact match (normally it's one of the results from FindFileMatches).
  const std::vector<UnitIndex>* FindFileUnitIndices(const std::string& name) const;

  // See main_functions_ below.
  const std::vector<IndexNode::SymbolRef>& main_functions() const { return main_functions_; }
  std::vector<IndexNode::SymbolRef>& main_functions() { return main_functions_; }

  const IndexNode& root() const { return root_; }
  IndexNode& root() { return root_; }

  size_t files_indexed() const { return file_name_index_.size(); }

  // Returns how many symbols are indexed. This iterates through everything so can be slow.
  size_t CountSymbolsIndexed() const;

 private:
  void IndexCompileUnit(const DwarfUnit& unit, int32_t dwo_index, UnitIndex unit_index,
                        bool force_slow_path);
  void IndexSkeletonCompileUnit(const DwarfUnit& unit, const llvm::DWARFDie& unit_die,
                                UnitIndex unit_index);
  void IndexCompileUnitSourceFiles(const DwarfUnit& unit, UnitIndex unit_index);

  // Populates the file_name_index_ given a now-unchanging files_ map.
  void IndexFileNames();

  // DWO files referenced by this symbol file. SymbolRef.dwo_index_ is an index into this vector.
  std::vector<SkeletonUnit> dwo_refs_;

  // Symbol index.
  IndexNode root_ = IndexNode(IndexNode::Kind::kRoot);

  // Maps full path names to compile units that reference them. This must not be mutated once the
  // file_name_index_ is built.
  //
  // The contents of the vector are indices into the compilation unit array. (see
  // llvm::DWARFContext::get[DWO]UnitAtIndex).
  //
  // This is a map, not a multimap because some files will appear in many compilation units. I
  // suspect it's better to avoid duplicating the names (like a multimap would) and eating the cost
  // of indirect heap allocations for vectors in the single-item case.
  using FileIndex = std::map<std::string, std::vector<UnitIndex>>;
  FileIndex files_;

  // Maps the last file name component (the part following the last slash) to the set of entries in
  // the files_ index that have that name.
  //
  // This is a multimap because the name parts will generally be unique so we should get few
  // duplicates. The cost of using a vector for most items containing one element becomes higher in
  // that case.
  using FileNameIndex = std::multimap<std::string_view, FileIndex::const_iterator>;
  FileNameIndex file_name_index_;

  // All references to functions in this module found annotated with the DW_AT_main_subprogram
  // attribute. Normally there will be 0 (not all compiler annotate this) or 1.
  std::vector<IndexNode::SymbolRef> main_functions_;

  // The file name index stores iterators into the files_ map which will be invalidated if this
  // class is moved.
  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(Index);
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_INDEX_H_
