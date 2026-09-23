// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LINK_MAP_LIST_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LINK_MAP_LIST_H_

#include <cassert>
#include <iterator>

#include "layout.h"
#include "svr4-abi.h"

namespace elfldltl {

// Forward declaration; see below.
struct LinkMapListDefaultTraits;

// This provides a container API with bidirectional iterators for a
// doubly-linked list that uses the traditional `struct link_map` (see
// elfldltl::Elf<...>::LinkMap<...> in <lib/elfldltl/svr4-abi.h>) as part
// of its element type layout.  The first template parameter gives the type
// of elements, what will be `value_type` in the container and its
// iterators.  The second template parameter defines how the LinkMap member
// is found therein via a "traits" type (see below).
template <typename ElementType = Elf<>::LinkMap<>, class Traits = LinkMapListDefaultTraits>
class LinkMapList {
  template <bool Reverse, bool Const>
  class IteratorImpl;

 public:
  using value_type = ElementType;
  using pointer = value_type*;
  using reference = value_type&;
  using const_reference = const value_type&;
  using difference_type = ptrdiff_t;
  using size_type = size_t;

  using iterator = IteratorImpl<false, false>;
  using reverse_iterator = IteratorImpl<true, false>;
  using const_iterator = IteratorImpl<false, true>;
  using const_reverse_iterator = IteratorImpl<true, true>;

  constexpr LinkMapList() = default;

  constexpr LinkMapList(const LinkMapList&) = default;

  constexpr explicit LinkMapList(const value_type* head) : head_(head) {}

  constexpr LinkMapList& operator=(const LinkMapList&) = default;

  constexpr iterator begin() { return iterator(this, head_); }

  constexpr iterator end() { return iterator(this, nullptr); }

  constexpr const_iterator begin() const { return const_iterator(this, head_); }

  constexpr const_iterator end() const { return const_iterator(this, nullptr); }

  constexpr reverse_iterator rbegin() {
    reverse_iterator it(this, nullptr);
    if (head_) {
      ++it;
    }
    return it;
  }

  constexpr reverse_iterator rend() {
    reverse_iterator it(this, head_);
    if (head_) {
      ++it;
    }
    return it;
  }

  constexpr const_reverse_iterator rbegin() const { return const_reverse_iterator(this, nullptr); }

  constexpr const_reverse_iterator rend() const { return const_reverse_iterator(this, head_); }

  constexpr bool empty() const { return !head_; }

 private:
  constexpr const value_type* Next(const value_type* pos) const {
    if (!pos) {
      // Moving "forwards" from rend() gets the head.
      assert(head_);
      return head_;
    }
    if (auto* next = Traits::ElementToLinkMap(*pos).next.get()) {
      return &(Traits::LinkMapToElement(*next));
    }
    // We've reached the end, so cache the tail if not already known.
    if (!tail_) {
      tail_ = pos;
    }
    return nullptr;
  }

  constexpr const value_type* Prev(const value_type* pos) const {
    if (!pos) {
      // Moving backwards from the end gets the tail.
      if (!tail_) {
        // It's not already known, so find it forward from the head and
        // cache it.  Note this takes O(n) time.
        assert(head_);
        const value_type* pos = head_;
        do {
          pos = Next(pos);
        } while (pos);
        assert(tail_);
      }
      return tail_;
    }
    if (auto* prev = Traits::ElementToLinkMap(*pos).prev.get()) {
      return &(Traits::LinkMapToElement(*prev));
    }
    return nullptr;
  }

  const value_type* head_ = nullptr;
  mutable const value_type* tail_ = nullptr;
};

// This is the default "traits" type for LinkMapList, which only works when
// the element type is LinkMap itself, or really is any type with members
// named `next` and `prev` that are pointers to that same type.
struct LinkMapListDefaultTraits {
  // This must translate a `const LinkMap&` to a `const value_type&`.
  template <class Element>
  static constexpr Element& LinkMapToElement(Element& map) {
    return map;
  }

  // This must translate a `const value_type&` to a `const LinkMap&`.
  template <class Element>
  static constexpr Element& ElementToLinkMap(Element& map) {
    return map;
  }
};

// This defines the "traits" type for a struct with a first member named
// `link_map` that is of the LinkMap type.
template <class ElementType>
struct LinkMapListInFirstMemberTraits {
  using LinkMapType = decltype(std::declval<ElementType>().link_map);

  static const ElementType& LinkMapToElement(const LinkMapType& map) {
    if constexpr (std::is_standard_layout_v<ElementType>) {
      static_assert(offsetof(ElementType, link_map) == 0);
    } else {
      // It's not kosher to use offsetof here, but this should get compiled
      // away so there's no runtime test at all (but maybe a runtime failure).
      assert(&(reinterpret_cast<const ElementType&>(map).link_map) == &map);
    }
    return reinterpret_cast<const ElementType&>(map);
  }

  static const LinkMapType& ElementToLinkMap(const ElementType& element) {
    return element.link_map;
  }
};

template <typename ElementType, class Traits>
template <bool Reverse, bool Const>
class LinkMapList<ElementType, Traits>::IteratorImpl {
 public:
  using value_type = LinkMapList::value_type;
  using pointer = LinkMapList::pointer;
  using reference = LinkMapList::reference;
  using difference_type = LinkMapList::difference_type;
  using iterator_category = std::bidirectional_iterator_tag;

  constexpr IteratorImpl() = default;

  constexpr IteratorImpl(const IteratorImpl&) = default;

  // const_iterator is constructible from iterator.
  template <bool C = Const, typename = std::enable_if_t<C>>
  constexpr IteratorImpl(const IteratorImpl<Reverse, false>& other)
      : list_(other.list_), pos_(other.pos_) {}

  constexpr IteratorImpl& operator=(const IteratorImpl&) = default;

  constexpr bool operator==(const IteratorImpl& other) const { return pos_ == other.pos_; }

  constexpr bool operator!=(const IteratorImpl& other) const { return !(*this == other); }

  constexpr auto& operator*() const { return *operator->(); }

  constexpr auto* operator->() const {
    if constexpr (Const) {
      return pos_;
    } else {
      return const_cast<value_type*>(pos_);
    }
  }

  constexpr IteratorImpl& operator++() {  // prefix
    pos_ = (list_->*kNext)(pos_);
    return *this;
  }

  constexpr IteratorImpl operator++(int) {  // postfix
    IteratorImpl old = *this;
    ++*this;
    return old;
  }

  constexpr IteratorImpl& operator--() {  // prefix
    pos_ = (list_->*kPrev)(pos_);
    return *this;
  }

  constexpr IteratorImpl operator--(int) {  // postfix
    IteratorImpl old = *this;
    --*this;
    return old;
  }

 private:
  friend LinkMapList;

  static constexpr auto kNext = Reverse ? &LinkMapList::Prev : &LinkMapList::Next;
  static constexpr auto kPrev = !Reverse ? &LinkMapList::Prev : &LinkMapList::Next;

  constexpr IteratorImpl(const LinkMapList* list, const value_type* pos) : list_{list}, pos_{pos} {}

  const LinkMapList* list_ = nullptr;
  const value_type* pos_ = nullptr;
};

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LINK_MAP_LIST_H_
