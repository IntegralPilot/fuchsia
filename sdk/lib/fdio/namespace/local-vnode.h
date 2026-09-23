// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDIO_NAMESPACE_LOCAL_VNODE_H_
#define LIB_FDIO_NAMESPACE_LOCAL_VNODE_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fdio/cleanpath.h>
#include <lib/fdio/namespace.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <lib/zxio/types.h>
#include <limits.h>
#include <zircon/types.h>

#include <utility>

#include <fbl/intrusive_wavl_tree.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <fbl/string.h>
#include <fbl/string_buffer.h>

#include "sdk/lib/fdio/internal.h"

namespace fdio_internal {

using EnumerateCallback = fit::function<zx_status_t(std::string_view path, zxio_t* entry)>;

// Represents a mapping from a string name to a remote connection.
//
// Each LocalVnode may have named children, which themselves may also
// optionally represent remote connections.
//
// This class is thread-compatible.
class LocalVnode : public fbl::RefCounted<LocalVnode> {
 public:
  LocalVnode(const LocalVnode&) = delete;
  LocalVnode(LocalVnode&&) = delete;
  LocalVnode& operator=(const LocalVnode&) = delete;
  LocalVnode& operator=(LocalVnode&&) = delete;

  // Detaches this vnode from its parent. The Vnode's own children are not unlinked.
  void UnlinkFromParent();

  // Returns the next child vnode from the list of children, assuming that
  // |last_seen| is the ID of the last returned vnode. At the same time,
  // |last_seen| is updated to reflect the current ID.
  //
  // If the end of iteration is reached, |ZX_ERR_NOT_FOUND| is returned.
  zx::result<std::string_view> Readdir(uint64_t* last_seen) const;

  // Invoke |func| on the (path, channel) pairs for all remote nodes found in the
  // node hierarchy rooted at `this`.
  zx_status_t EnumerateRemotes(const EnumerateCallback& func);

  struct IdTreeTag {};
  struct NameTreeTag {};

  class Entry : public fbl::ContainableBaseClasses<
                    fbl::TaggedWAVLTreeContainable<std::unique_ptr<Entry>, IdTreeTag,
                                                   fbl::NodeOptions::AllowMultiContainerUptr>,
                    fbl::TaggedWAVLTreeContainable<Entry*, NameTreeTag>> {
   public:
    Entry(uint64_t id, fbl::String name, fbl::RefPtr<LocalVnode> node)
        : id_(id), name_(std::move(name)), node_(std::move(node)) {}
    ~Entry() = default;

    uint64_t id() const { return id_; }
    const fbl::String& name() const { return name_; }
    const fbl::RefPtr<LocalVnode>& node() const { return node_; }

   private:
    const uint64_t id_;
    const fbl::String name_;
    const fbl::RefPtr<LocalVnode> node_;
  };

  struct KeyByIdTraits {
    static uint64_t GetKey(const Entry& entry) { return entry.id(); }
    static bool LessThan(uint64_t key1, uint64_t key2) { return key1 < key2; }
    static bool EqualTo(uint64_t key1, uint64_t key2) { return key1 == key2; }
  };

  struct KeyByNameTraits {
    static const fbl::String& GetKey(const Entry& entry) { return entry.name(); }
    static bool LessThan(const fbl::String& key1, const fbl::String& key2) { return key1 < key2; }
    static bool EqualTo(const fbl::String& key1, const fbl::String& key2) { return key1 == key2; }
  };

  using EntryByIdMap =
      fbl::TaggedWAVLTree<uint64_t, std::unique_ptr<Entry>, IdTreeTag, KeyByIdTraits>;
  using EntryByNameMap = fbl::TaggedWAVLTree<fbl::String, Entry*, NameTreeTag, KeyByNameTraits>;

  class Intermediate;

  using ParentAndId = std::tuple<std::reference_wrapper<Intermediate>, uint64_t>;

  class Intermediate {
   public:
    Intermediate(const Intermediate&) = delete;
    Intermediate(Intermediate&&) = delete;
    Intermediate& operator=(const Intermediate&) = delete;
    Intermediate& operator=(Intermediate&&) = delete;

    Intermediate() = default;
    ~Intermediate();

    size_t num_children() const;
    // Returns (child, false) if a child with |name| exists, otherwise creates a new child with
    // |name| using |builder| and returns (child, true). Returns the error if |builder| fails.
    zx::result<std::tuple<fbl::RefPtr<LocalVnode>, bool>> LookupOrInsert(
        fbl::String name, fit::function<zx::result<fbl::RefPtr<LocalVnode>>(ParentAndId)> builder);
    void RemoveEntry(LocalVnode* vn, uint64_t id);

    // See |LocalVnode::Readdir|.
    zx::result<std::string_view> Readdir(uint64_t* last_seen) const;

    // Returns a child if it has the name |name|.
    // Otherwise, returns nullptr.
    fbl::RefPtr<LocalVnode> Lookup(const fbl::String& name) const;

    // Invoke |Fn()| on all entries in this Intermediate node_type.
    // May be used as a const visitor-pattern for all entries.
    //
    // Any status other than ZX_OK returned from |Fn()| will halt iteration
    // immediately and return.
    template <typename Fn>
    zx_status_t ForAllEntries(Fn fn) const;

   private:
    uint64_t next_node_id_ = 1;
    EntryByNameMap entries_by_name_;
    EntryByIdMap entries_by_id_;
  };

  class Local {
   public:
    Local(const Local&) = delete;
    Local(Local&&) = delete;
    Local& operator=(const Local&) = delete;
    Local& operator=(Local&&) = delete;

    Local(fdio_open_local_func_t on_open, void* context);
    zx::result<fdio_ptr> Open();

   private:
    const fdio_open_local_func_t on_open_;
    void* const context_;
  };

  class Remote {
   public:
    Remote(const Remote&) = delete;
    Remote(Remote&&) = delete;
    Remote& operator=(const Remote&) = delete;
    Remote& operator=(Remote&&) = delete;

    explicit Remote(zxio_storage_t remote_storage) : remote_storage_(remote_storage) {}
    ~Remote();

    zxio_t* Connection() { return &remote_storage_.io; }

   private:
    zxio_storage_t remote_storage_;
  };

  std::variant<Local, Intermediate, Remote>& NodeType() { return node_type_; }

 private:
  friend class fbl::internal::MakeRefCountedHelper<LocalVnode>;
  friend class fbl::RefPtr<LocalVnode>;

  zx_status_t EnumerateInternal(PathBuffer* path, std::string_view name,
                                const EnumerateCallback& func);

  // The parent must outlive the child.
  template <class T, class... Args>
  LocalVnode(std::optional<ParentAndId> parent_and_id, std::in_place_type_t<T> in_place,
             Args&&... args)
      : node_type_(in_place, std::forward<Args>(args)...), parent_and_id_(parent_and_id) {}

  std::variant<Local, Intermediate, Remote> node_type_;
  std::optional<ParentAndId> parent_and_id_;
};

}  // namespace fdio_internal

#endif  // LIB_FDIO_NAMESPACE_LOCAL_VNODE_H_
