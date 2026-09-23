// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/view_tree/view_tree_snapshotter.h"

#include <lib/async/cpp/task.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

namespace view_tree {

namespace {

template <typename K, typename V>
bool ContainsKeyValuePair(const std::multimap<K, V>& multimap, K key, V value) {
  auto key_range = multimap.equal_range(key);
  for (auto it = key_range.first; it != key_range.second; ++it) {
    if (it->second == value) {
      return true;
      break;
    }
  }
  return false;
}

// Recursively walks the tree and calls |visitor| on each node.
// Ignores child pointers without corresponding child nodes.
void TreeWalk(const std::unordered_map<zx_koid_t, ViewNode>& view_tree, zx_koid_t root,
              const fit::function<void(zx_koid_t, const ViewNode&)>& visitor) {
  if (!view_tree.contains(root)) {
    return;
  }

  visitor(root, view_tree.at(root));

  for (auto child : view_tree.at(root).children) {
    TreeWalk(view_tree, child, visitor);
  }
}

bool ValidateSnapshot(const Snapshot& snapshot) {
  TRACE_DURATION("gfx", "ViewTreeSnapshotter::ValidateSnapshot");

  const auto& [root, view_tree, unconnected_views, sequence_number, hit_testers] = snapshot;
  FX_DCHECK(sequence_number > 0);
  if (view_tree.empty() && root == ZX_KOID_INVALID) {
    return true;
  }

  FX_DCHECK(root != ZX_KOID_INVALID);
  FX_DCHECK(view_tree.contains(root));
  FX_DCHECK(view_tree.at(root).parent == ZX_KOID_INVALID);

  size_t tree_walk_size = 0;
  TreeWalk(view_tree, root,
           [&tree_walk_size](zx_koid_t koid, const ViewNode& node) { ++tree_walk_size; });
  FX_DCHECK(tree_walk_size == view_tree.size()) << "ViewTree is not fully connected";

  for (const auto& [koid, node] : view_tree) {
    FX_DCHECK(!unconnected_views.contains(koid))
        << "Node " << koid << " was in both the ViewTree and the unconnected nodes set";
    for (auto child : node.children) {
      FX_DCHECK(view_tree.contains(child))
          << "Child " << child << " of node " << koid << " is not part of the ViewTree";
      FX_DCHECK(view_tree.at(child).parent == koid)
          << "Node " << koid << " has child " << child << ", but its parent pointer is "
          << view_tree.at(child).parent;
    }
  }

  return true;
}

bool ValidateSubtree(const SubtreeSnapshot& subtree) {
  TRACE_DURATION("gfx", "ViewTreeSnapshotter::ValidateSubtree");

  const auto& [root, view_tree, unconnected_views, hit_tester, tree_boundaries] = subtree;
  if (view_tree.empty() && root == ZX_KOID_INVALID) {
    return true;
  }
  FX_DCHECK(root != ZX_KOID_INVALID);
  FX_DCHECK(view_tree.contains(root));
  FX_DCHECK(view_tree.at(root).parent == ZX_KOID_INVALID);

  size_t tree_walk_size = 0;
  TreeWalk(view_tree, root, [&tree_walk_size](zx_koid_t koid, const ViewNode& node) {
    FX_DCHECK(node.view_ref) << "ViewRef not set on node " << koid;
    ++tree_walk_size;
  });
  FX_DCHECK(tree_walk_size == view_tree.size()) << "ViewTree is not fully connected";

  for (const auto& [koid, node] : view_tree) {
    FX_DCHECK(!unconnected_views.contains(koid))
        << "Node " << koid << " was in both the ViewTree and the unconnected nodes set";
    for (auto child : node.children) {
      FX_DCHECK(view_tree.contains(child) || ContainsKeyValuePair(tree_boundaries, koid, child))
          << "Child " << child << " of node " << koid
          << " is not part of the ViewTree or tree_boundaries";
      FX_DCHECK(!view_tree.contains(child) || view_tree.at(child).parent == koid)
          << "Node " << koid << " has child " << child << ", but its parent pointer is "
          << view_tree.at(child).parent;
    }
  }

  for (const auto& [parent, child] : tree_boundaries) {
    FX_DCHECK(view_tree.contains(parent))
        << "Parent " << parent << " in tree_boundaries does not exist in the same subtree";
    FX_DCHECK(!view_tree.contains(child))
        << "Child " << child << " in tree_boundaries should not exist in the same subtree";
  }

  return true;
}

}  // namespace

ViewTreeSnapshotter::ViewTreeSnapshotter(std::vector<SubtreeSnapshotGenerator> subtree_generators,
                                         std::vector<Subscriber> subscribers)
    : subtree_generators_(std::move(subtree_generators)) {
  for (auto& subscriber : subscribers) {
    subscriber_callbacks_.emplace_back(std::move(subscriber.on_new_view_tree));
  }

  cached_subtree_snapshots_.resize(subtree_generators_.size());
}

bool ViewTreeSnapshotter::UpdateSnapshot() {
  TRACE_DURATION("gfx", "ViewTreeSnapshotter::UpdateSnapshot");

  bool any_subtree_changed = false;
  for (size_t i = 0; i < subtree_generators_.size(); ++i) {
    TRACE_DURATION("gfx", "ViewTreeSnapshotter::UpdateSnapshot [generate]");

    GeneratedSubtreeSnapshot generated_subtree = subtree_generators_[i]();
    if (auto* full_subtree = std::get_if<std::unique_ptr<SubtreeSnapshot>>(&generated_subtree)) {
      FX_DCHECK(*full_subtree);
      FX_DCHECK(ValidateSubtree(*full_subtree->get()));
      cached_subtree_snapshots_[i] = std::move(*full_subtree);
      any_subtree_changed = true;
      continue;
    }
    if (auto* _ = std::get_if<SubtreeSnapshotNoDiff>(&generated_subtree)) {
      FX_CHECK(cached_subtree_snapshots_[i])
          << "NoDiff requires there to already be a SubtreeSnapshot";
      continue;
    }
    // Guarantee that all variant types are handled above.
    __UNREACHABLE;
  }
  if (!any_subtree_changed) {
    // Nothing has changed, so we don't need to rebuild the snapshot nor notify subscribers.
    return false;
  }

  auto new_snapshot = std::make_shared<Snapshot>();
  new_snapshot->sequence_number = next_sequence_number_++;
  std::multimap<zx_koid_t, zx_koid_t> tree_boundaries;
  {
    size_t reserved_view_tree_size = 0;
    size_t reserved_unconnected_views_size = 0;
    for (auto& cached_subtree : cached_subtree_snapshots_) {
      reserved_view_tree_size += cached_subtree->view_tree.size();
      reserved_unconnected_views_size += cached_subtree->unconnected_views.size();
    }
    new_snapshot->view_tree.reserve(reserved_view_tree_size);
    new_snapshot->unconnected_views.reserve(reserved_unconnected_views_size);
    // TODO(https://fxbug.dev/465552708): consider using `std::unordered_multimap` for tree
    // boundaries because that supports `reserve()`, but `std::multimap` does not.
  }

  // Merge subtrees.
  for (auto& subtree : cached_subtree_snapshots_) {
    auto& [root, view_tree, unconnected_views, hit_tester, subtree_boundaries] = *subtree;

    TRACE_DURATION("gfx", "ViewTreeSnapshotter::UpdateSnapshot [merge]");

    if (new_snapshot->root == ZX_KOID_INVALID) {
      new_snapshot->root = root;
    }

    {
      const size_t tree_size_before = new_snapshot->view_tree.size();
      const size_t subtree_size = view_tree.size();
      new_snapshot->view_tree.insert(view_tree.begin(), view_tree.end());
      FX_DCHECK(new_snapshot->view_tree.size() == tree_size_before + subtree_size)
          << "Two subtrees had duplicate nodes";
    }
    {
      const size_t unconnected_size_before = new_snapshot->unconnected_views.size();
      const size_t subtree_unconnected_size = unconnected_views.size();
      new_snapshot->unconnected_views.insert(unconnected_views.begin(), unconnected_views.end());
      FX_DCHECK(new_snapshot->unconnected_views.size() ==
                unconnected_size_before + subtree_unconnected_size)
          << "Two subtrees had duplicate unconnected nodes";
    }

    {
      const size_t boundaries_size_before = tree_boundaries.size();
      const size_t subtree_boundaries_size = subtree_boundaries.size();
      tree_boundaries.insert(subtree_boundaries.begin(), subtree_boundaries.end());
      FX_DCHECK(tree_boundaries.size() == boundaries_size_before + subtree_boundaries_size)
          << "Two subtrees had duplicate tree boundaries";
    }

    if (hit_tester) {
      new_snapshot->hit_testers.push_back(hit_tester);
    }
  }

  // Fix parent pointers at subtree boundaries.
  for (const auto& [parent, child] : tree_boundaries) {
    FX_DCHECK(new_snapshot->view_tree.contains(parent)) << "missing parent: " << parent;
    FX_DCHECK(new_snapshot->view_tree.contains(child)) << "missing child: " << child;
    new_snapshot->view_tree.at(child).parent = parent;
  }

  FX_DCHECK(ValidateSnapshot(*new_snapshot));

  // Update all subscribers with the new snapshot.
  for (const auto& subscriber_callback : subscriber_callbacks_) {
    TRACE_DURATION("gfx", "ViewTreeSnapshotter::UpdateSnapshot [subscriber]");
    subscriber_callback(new_snapshot);
  }
  return true;
}

}  // namespace view_tree
