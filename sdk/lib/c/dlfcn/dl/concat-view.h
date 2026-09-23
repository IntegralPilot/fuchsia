// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DL_CONCAT_VIEW_H_
#define LIB_DL_CONCAT_VIEW_H_

// This is a partial implementation of C++26 std::ranges::concat_view.  It
// doesn't do everything the spec requires, but enough for the dl code.
// Notably, it only implements a forward iterator (even if all the underlying
// iterators are more general).  It may miss some other spec nits that haven't
// come up in using it yet.

#include <ranges>
#include <tuple>
#include <type_traits>
#include <utility>

namespace dl {

template <class Iterator>
concept IteratorHasArrow = requires(Iterator it) {
  { it.operator->() } -> std::same_as<typename Iterator::value_type*>;
};

template <std::ranges::input_range... Views>
  requires(sizeof...(Views) > 0 &&
           (std::is_same_v<
                std::ranges::range_reference_t<Views>,
                std::ranges::range_reference_t<std::tuple_element_t<0, std::tuple<Views...>>>> &&
            ...))
class ConcatView : public std::ranges::view_interface<ConcatView<Views...>> {
 public:
  constexpr ConcatView() = default;

  constexpr explicit(false) ConcatView(Views... views) : views_{std::move(views)...} {}

  constexpr auto begin() { return BeginIterator(this); }
  constexpr auto begin() const
    requires(std::ranges::range<const Views> && ...)
  {
    return BeginIterator(this);
  }
  constexpr auto end() { return EndIterator(this); }
  constexpr auto end() const
    requires(std::ranges::range<const Views> && ...)
  {
    return EndIterator(this);
  }

 private:
  // The iterator implementation holds a pointer to the containing concat_view
  // object, and a std::variant<std::ranges::iterator_t<Views>...>.  It starts
  // at variant index 0 with the begin() of the first view.  When that hits the
  // end() of the first view, then it moves up to variant index 1 with the
  // begin() of the second view; and so on.
  //
  // This makes operator* very easy since it's just a std::visit.  Likewise,
  // operator== comes entirely for free with an explicit default (and thus an
  // implicitly defaulted operator!= from it too!) because std::variant is
  // conveniently comparable to treat different variant index as not equal and
  // same variant index as the two underlying iterators' equality.
  template <bool Const>
  class IteratorImpl {
   public:
    using value_type = std::ranges::range_value_t<std::tuple_element_t<0, std::tuple<Views...>>>;
    using difference_type = ptrdiff_t;  // Required by std::weakly_incrementable.
    using pointer = value_type*;
    using reference = value_type&;

    constexpr IteratorImpl() = default;
    constexpr IteratorImpl(const IteratorImpl&) = default;
    constexpr IteratorImpl& operator=(const IteratorImpl&) = default;

    constexpr bool operator==(const IteratorImpl&) const = default;

    constexpr IteratorImpl& operator++() {  // prefix
      VisitEnumerate([this](auto I, auto& it) {
        ++it;
        this->template satisfy<I>();
      });
      return *this;
    }

    constexpr IteratorImpl operator++(int) {  // postfix
      IteratorImpl old = *this;
      ++*this;
      return old;
    }

    constexpr decltype(auto) operator*() const {
      return std::visit([](const auto& it) -> decltype(auto) { return *it; }, it_);
    }

    constexpr decltype(auto) operator->() const
      requires(IteratorHasArrow<std::ranges::iterator_t<Views>> && ...)
    {
      return std::visit([](const auto& it) -> decltype(auto) { return it.operator->(); }, it_);
    }

   private:
    friend ConcatView;

    template <typename T>
    using MaybeConst = std::conditional_t<Const, const T, T>;

    using Parent = MaybeConst<ConcatView>;

    template <typename... Args>
    explicit constexpr IteratorImpl(Parent* parent, Args&&... args)
        : parent_{parent}, it_{std::forward<Args>(args)...} {}

    template <typename F>
    constexpr void VisitEnumerate(F&& f) {
      VisitEnumerate(std::forward<F>(f), std::make_index_sequence<sizeof...(Views)>{});
    }

    template <typename F, size_t... I>
    constexpr void VisitEnumerate(F&& f, std::index_sequence<I...> seq) {
      auto invoke_if = [this, &f](auto Idx) -> bool {
        if (auto* it = std::get_if<Idx>(&it_)) {
          std::invoke(std::forward<F>(f), Idx, *it);
          return true;
        }
        return false;
      };
      (invoke_if(std::integral_constant<size_t, I>{}) || ...);
    }

    // This is called with the current variant index as the template parameter.
    // If the current iterator is at its end, then move to the next one.
    template <size_t I>
    constexpr void satisfy() {
      if constexpr (I < sizeof...(Views) - 1) {
        if (std::get<I>(it_) == InnerEnd<I>(parent_)) {
          it_.template emplace<I + 1>(InnerBegin<I + 1>(parent_));
          satisfy<I + 1>();
        }
      }
    }

    MaybeConst<ConcatView>* parent_ = nullptr;
    std::variant<std::ranges::iterator_t<MaybeConst<Views>>...> it_;
  };

  template <size_t I, class Self>
  static constexpr auto InnerBegin(Self* self) {
    return std::ranges::begin(std::get<I>(self->views_));
  }

  template <size_t I, class Self>
  static constexpr auto InnerEnd(Self* self) {
    return std::ranges::end(std::get<I>(self->views_));
  }

  template <class Self>
  using IteratorFor = IteratorImpl<std::is_const_v<Self>>;

  template <class Self>
  static constexpr IteratorFor<Self> BeginIterator(Self* self) {
    IteratorFor<Self> it{self, std::in_place_index<0>, InnerBegin<0>(self)};
    it.template satisfy<0>();
    return it;
  }

  template <class Self>
  static constexpr IteratorFor<Self> EndIterator(Self* self) {
    constexpr size_t kLast = sizeof...(Views) - 1;
    return IteratorFor<Self>{self, std::in_place_index<kLast>, InnerEnd<kLast>(self)};
  }

  std::tuple<Views...> views_;
};

}  // namespace dl

#endif  // LIB_DL_CONCAT_VIEW_H_
