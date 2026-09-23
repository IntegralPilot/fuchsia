// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_LOCK_CLASS_STATE_H_
#define SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_LOCK_CLASS_STATE_H_

#include <lib/fxt/interned_string.h>
#include <stdint.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <algorithm>
#include <atomic>

#include <fbl/algorithm.h>
#include <lockdep/common.h>
#include <lockdep/lock_dependency_set.h>
#include <lockdep/lock_traits.h>

namespace lockdep {

// The LockClassState type and its derivatives hold essential metadata and state
// for a lock class. This is used by ThreadLockState and the loop detector logic
// to uniformly operate on the variety of lock classes created by each template
// instantiation of LockClass. Each template instantiation of LockClass creates
// a unique static instance of LockClassState, or a derivative, depending on
// whether validation or metadata only compile time options are enabled.

// Empty base class that provides the basic interface with no internal state.
class LockClassState {
 public:
  // Constructs an instance of LockClassState.
  constexpr LockClassState() = default;
  constexpr LockClassState(fxt::InternedString& name, LockFlags flags) {}

  // Disable copy construction / assignment.
  LockClassState(const LockClassState&) = delete;
  LockClassState& operator=(const LockClassState&) = delete;

  // Returns the LockClassState instance for the given lock class id. The id
  // must be a valid lock class id.
  static LockClassState* Get(LockClassId id) { return const_cast<LockClassState*>(id); }

  // Returns the lock class id for this instance. The id is the address of the
  // instance.
  constexpr LockClassId id() const { return this; }
};

// A derivative of LockClassState that stores basic metadata about the lock
// class, such as the name and lock flags. This type is used when metadata
// only mode is selected at compile time.
class MetadataLockClassState : public LockClassState {
 public:
  // Constructs an instance of MetadataLockClassState.
  constexpr MetadataLockClassState(const fxt::InternedString& name, LockFlags flags)
      : name_{name} {}

  // Disable copy construction / assignment.
  MetadataLockClassState(const MetadataLockClassState&) = delete;
  MetadataLockClassState& operator=(const MetadataLockClassState&) = delete;

  // Returns the MetadataLockClassState instance for the given lock class id.
  // The id must be a valid lock class id. This method intentionally shadows
  // LockClassState::Get.
  static MetadataLockClassState* Get(LockClassId id) {
    ZX_DEBUG_ASSERT(kLockMetadataAvailable);
    return static_cast<MetadataLockClassState*>(LockClassState::Get(id));
  }

  // Returns the type name of the lock class for the given lock class id.
  static const char* GetName(LockClassId id) { return Get(id)->name(); }

  // Returns the name of this lock class.
  const char* name() const { return name_.string(); }
  const fxt::InternedString& interned_name() { return name_; }

 private:
  // The name of the lock class type.
  const fxt::InternedString& name_;
};

// A derivative of MetadataLockClassState that provides full state necessary for
// runtime lock validation. This type is used when lock validation is enabled at
// compile time.
class ValidatorLockClassState : public MetadataLockClassState {
 public:
  // Constructs an instance of ValidatorLockClassState.
  ValidatorLockClassState(const fxt::InternedString& name, LockFlags flags)
      : MetadataLockClassState{name, flags}, flags_{flags} {}

  // Disable copy construction / assignment.
  ValidatorLockClassState(const ValidatorLockClassState&) = delete;
  ValidatorLockClassState& operator=(const ValidatorLockClassState&) = delete;

  // Returns the ValidatorLockClassState instance for the given lock class id.
  // The id must be a valid lock class id. This method intentionally shadows
  // MetadataLockClassState::Get.
  static ValidatorLockClassState* Get(LockClassId id) {
    ZX_DEBUG_ASSERT(kLockValidationEnabled);
    return static_cast<ValidatorLockClassState*>(MetadataLockClassState::Get(id));
  }

  // Returns true if the given lock class is irq-safe, false otherwise.
  static bool IsIrqSafe(LockClassId id) { return !!(Get(id)->flags() & LockFlagsIrqSafe); }

  // Returns true if the given lock class is nestable, false otherwise.
  static bool IsNestable(LockClassId id) { return !!(Get(id)->flags() & LockFlagsNestable); }

  // Returns true if the given lock class is multi-acquire, false otherwise.
  static bool IsMultiAcquire(LockClassId id) {
    return !!(Get(id)->flags() & LockFlagsMultiAcquire);
  }

  static bool IsLeaf(LockClassId id) { return !!(Get(id)->flags() & LockFlagsLeaf); }

  // Returns true if reporting is disabled for the given lock class, false
  // otherwise.
  static bool IsReportingDisabled(LockClassId id) {
    return !!(Get(id)->flags() & LockFlagsReportingDisabled);
  }

  // Returns true if the validator should abort the program if it detects an
  // invalid re-acquire with this lock class.
  static bool IsReAcquireFatal(LockClassId id) {
    return !!(Get(id)->flags() & LockFlagsReAcquireFatal);
  }

  // Returns true if the lock should not be added to the active lock list
  // during an acquire.
  static bool IsActiveListDisabled(LockClassId id) {
    return !!(Get(id)->flags() & LockFlagsActiveListDisabled);
  }

  // Returns true if the lock should not be tracked.
  static bool IsTrackingDisabled(LockClassId id) {
    return !!(Get(id)->flags() & LockFlagsTrackingDisabled);
  }

  // Returns true if lock class given by |search_id| is in the dependency set of
  // the lock class given by |id|, false otherwise.
  static bool HasLockClass(LockClassId id, LockClassId search_id) {
    return Get(id)->dependency_set_.HasLockClass(search_id);
  }

  // Adds the lock class given by |add_id| to the dependency set of the lock
  // class given by |id|.
  static LockResult AddLockClass(LockClassId id, LockClassId add_id) {
    return Get(id)->dependency_set_.AddLockClass(add_id);
  }

  // Return the flags of this lock class.
  LockFlags flags() const { return flags_; }

  // Iterator type to traverse the set of ValidatorLockClassState instances.
  class Iterator {
   public:
    Iterator() = default;
    Iterator(const Iterator&) = default;
    Iterator& operator=(const Iterator&) = default;

    ValidatorLockClassState& operator*() { return *state_; }
    Iterator operator++() {
      state_ = state_->next_;
      return *this;
    }
    bool operator!=(const Iterator& other) const { return state_ != other.state_; }

    Iterator begin() { return {head_}; }
    Iterator end() { return {nullptr}; }

   private:
    Iterator(ValidatorLockClassState* state) : state_{state} {}
    ValidatorLockClassState* state_{nullptr};
  };

  // Returns an iterator for the init-time linked list of state instances.
  static Iterator Iter() { return {}; }

  // Returns the dependency set for this lock class.
  const LockDependencySet& dependency_set() const { return dependency_set_; }

  ValidatorLockClassState* connected_set() { return LoopDetector::FindSet(&loop_node_)->ToState(); }

  uint64_t index() const { return loop_node_.index; }
  uint64_t least() const { return loop_node_.least; }

  // Resets the dependency set and disjoint set of this object. This is
  // primarily used to initialize the state between successive tests.
  void Reset() {
    dependency_set_.clear();
    loop_node_.Reset();
  }

 private:
  friend class SingletonLoopDetector;

  // Flags specifying which rules to apply during lock validation.
  const LockFlags flags_;

  // The set of out edges from this node in the lock class dependency graph.
  // Out edges represent lock classes that have been held before this class.
  LockDependencySet dependency_set_;

  // Head pointer to linked list of state instances.
  inline static ValidatorLockClassState* head_{nullptr};

  // Linked list pointer to the next state instance. This list is constructed
  // by a global initializer and never modified again. The list is used by the
  // loop detector and runtime lock inspection commands to access the complete
  // list of lock classes.
  ValidatorLockClassState* next_{InitNext(this)};

  // Updates the linked list to include the given state node and returns the
  // previous head. This is used by the global initializer to setup the
  // next_ member.
  static ValidatorLockClassState* InitNext(ValidatorLockClassState* state) {
    ValidatorLockClassState* head = head_;
    head_ = state;
    return head;
  }

  // Per-lock class state used by the loop detection algorithm.
  struct LoopNode {
    LoopNode(ValidatorLockClassState* state) : state{state} {}

    ValidatorLockClassState* const state;

    // The parent of the disjoint sets this node belongs to. Nodes start out
    // alone in their own set. Sets are joined by the loop detector when
    // found within a cycle.
    std::atomic<LoopNode*> parent{this};

    // Linked list node for the loop detector's active node stack. The use
    // of a linked list of statically allocated nodes avoids dynamic memory
    // allocation during graph traversal.
    LoopNode* next{nullptr};

    // Index values used by the loop detector algorithm.
    uint64_t index{0};
    uint64_t least{0};

    // Returns a pointer to the ValidatorLockClassState instance that contains this
    // LoopNode. This allows the loop detector to mostly operate in terms
    // of LoopNode instances, simplifying expressions in the main algorithm.
    ValidatorLockClassState* ToState() const { return state; }

    // Performs a relaxed, weak compare exchange on the parent pointer of
    // this loop node. Due to the loops in FindSet() and UnionSet() this
    // may fail due to races, the result is not required and will be
    // retried based on other conditions.  Relaxed order is used because
    // neither caller publishes any other stores, nor depends on any other
    // loads.
    void CompareExchangeParent(LoopNode** expected, LoopNode* desired) {
      parent.compare_exchange_weak(*expected, desired, std::memory_order_relaxed,
                                   std::memory_order_relaxed);
    }

    // Removes this node from whatever disjoint set it belongs to and
    // returns it to its own separate set.
    void Reset() { parent.store(this, std::memory_order_relaxed); }
  };

  // Loop detector node.
  LoopNode loop_node_{this};

  // Loop detection using Tarjan's strongly connected components algorithm to
  // efficiently identify loops and disjoint set structures to store and
  // update the sets of nodes involved in loops.
  // NOTE: The loop detector methods, except FindSet and UnionSets, must only
  // be called from the loop detector thread.
  struct LoopDetector {
    // The maximum index of the last loop detection run. Node index values
    // are compared with this value to determine whether to revisit the
    // node. Using a generation count permits running subsequent passes
    // without first clearing the state of every node.
    uint64_t generation{0};

    // Running counter marking the step at which a node has been visited or
    // revisited.
    uint64_t index{0};

    // The head of the stack of active nodes in a path traversal. The bottom
    // of the stack is marked with the sentinel value 1 instead of nullptr
    // to simplify determining whether a node is on the stack. Every node on
    // the stack has LoopNode::next != nulltpr.
    LoopNode* stack{reinterpret_cast<LoopNode*>(1)};

    // Performs a single traversal of the lock dependency graph and updates
    // the disjoint set structures with any detected loops.
    void DetectionPass() {
      // The next generation starts at the end of the previous. All nodes
      // with indices less than or equal to the generation are revisited.
      generation = index;

      for (auto& state : ValidatorLockClassState::Iter()) {
        if (state.loop_node_.index <= generation)
          Connect(&state.loop_node_);
      }
    }

    // Recursively traverses a node path and updates the disjoint set
    // structures when loops are detected.
    void Connect(LoopNode* node) {
      index += 1;
      node->index = index;
      node->least = index;
      Push(node);

      // Evaluate each node along the out edges of the dependency graph.
      const auto& out_edges = node->ToState()->dependency_set();
      for (LockClassId id : out_edges) {
        LoopNode* related_node = &Get(id)->loop_node_;
        if (related_node->index <= generation) {
          Connect(related_node);
          node->least = std::min(node->least, related_node->least);
        } else if (related_node->next != nullptr) {
          node->least = std::min(node->least, related_node->index);
        }
      }

      // Update the disjoint set structures. Other nodes above this one on
      // the stack are merged into this set.
      if (node->index == node->least) {
        LoopNode* top = nullptr;
        size_t set_size = 0;
        while (top != node) {
          top = Pop();
          UnionSets(node, top);
          set_size++;
        }

        // Report loops with more than two components. Basic inversions
        // with only two locks are reported by ThreadLockState::Acquire.
        if (set_size > 2) {
          LoopNode* root = FindSet(node);
          SystemCircularLockDependencyDetected(root->ToState());
        }
      }
    }

    // Pushes a node on the active nodes stack.
    void Push(LoopNode* node) {
      ZX_DEBUG_ASSERT(node->next == nullptr);
      node->next = stack;
      stack = node;
    }

    // Pops a node from the active nodes stack.
    LoopNode* Pop() {
      ZX_DEBUG_ASSERT(stack != reinterpret_cast<LoopNode*>(1));
      LoopNode* node = stack;
      stack = node->next;
      node->next = nullptr;
      return node;
    }

    // Finds the parent node of the disjoint set this node belongs to. This
    // approach applies thread-safe path splitting to flatten the set as it
    // traverses the path, using the two-try optimization suggested by
    // Jayanti and Tarjan.
    static LoopNode* FindSet(LoopNode* node) {
      while (true) {
        // First pass: either terminate or attempt path split.
        LoopNode* parent = node->parent.load(std::memory_order_relaxed);
        LoopNode* grandparent = parent->parent.load(std::memory_order_relaxed);
        if (parent == grandparent)
          return parent;
        node->CompareExchangeParent(&parent, grandparent);

        // Second pass: either terminate, retry if last pass failed, or
        // advance and attempt path split.
        parent = node->parent.load(std::memory_order_relaxed);
        grandparent = parent->parent.load(std::memory_order_relaxed);
        if (parent == grandparent)
          return parent;
        node->CompareExchangeParent(&parent, grandparent);

        // Advance regardless of whether split succeeded or failed.
        node = parent;
      }
    }

    // Joins the disjoint sets for the given nodes. Performs linking based
    // on address order, which approximates the randomized total order
    // suggested by Jayanti and Tarjan.
    static void UnionSets(LoopNode* a, LoopNode* b) {
      while (true) {
        LoopNode* root_a = FindSet(a);
        LoopNode* root_b = FindSet(b);

        a = root_a;
        b = root_b;

        if (root_a == root_b) {
          return;  // Nothing to do for nodes in the same set.
        } else if (reinterpret_cast<uintptr_t>(root_a) < reinterpret_cast<uintptr_t>(root_b)) {
          root_b->CompareExchangeParent(&root_b, root_a);
        } else {
          root_a->CompareExchangeParent(&root_a, root_b);
        }
      }
    }
  };
};

// Provides storage for the global instance of the loop detector.
class SingletonLoopDetector {
 public:
  // Runs a loop detection pass on the set of lock classes to find possible
  // circular lock dependencies.
  static void LoopDetectionPass() { detector_.DetectionPass(); }

 private:
  inline static ValidatorLockClassState::LoopDetector detector_;
};

// Runs a loop detection pass to find circular lock dependencies. This must be
// invoked at some point in the future after the lock validator calls the
// system-defined SystemTriggerLoopDetection().
inline void LoopDetectionPass() { SingletonLoopDetector::LoopDetectionPass(); }

}  // namespace lockdep

#endif  // SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_LOCK_CLASS_STATE_H_
