// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_PERFECT_SYMBOL_TABLE_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_PERFECT_SYMBOL_TABLE_H_

#include <lib/fit/result.h>

#include <array>
#include <bitset>
#include <cassert>
#include <concepts>
#include <ranges>

#include "layout.h"
#include "symbol.h"

// This provides a means to define a compile-time set of ELF symbol names and
// do optimized lookups against this set at runtime.

namespace elfldltl {

// Use this to initialize a constexpr variable, as in:
// ```
// constexpr auto kFooSymbols = elfldltl::PerfectSymbolTable({
//     "foo",
//     "bar",
// });
// ```
// Thereafter use `kFooSymbols` as a template parameter for the classes below.
// It's a good idea to do this in an anonymous namespace, so that all the
// instantiations using it have internal linkage and the tables are only
// generated or used directly in a single translation unit.
template <size_t N>
  requires(N > 0)
consteval std::array<SymbolName, N> PerfectSymbolTable(SymbolName (&&names)[N]) {
  return std::to_array<SymbolName, N>(names);
}

// This requires that a type be a const reference to std::array<SymbolName, N>
// as returned by elfldltl::PerfectSymbolTable.
template <typename T>
concept SymbolNameArray =
    std::tuple_size<std::decay_t<T>>{} > 0 &&
    std::same_as<T, const std::array<SymbolName, std::tuple_size<std::decay_t<T>>{}>&>;

// This does the heavy lifting at compile time.  It computes the necessary size
// for a hash table with bucket depth of one for the given set of symbol names.
// It produces a permutation of the Names array that assigns a unique table
// index to each symbol name.  Its static methods provide that computed
// computed size, so parallel tables can be indexed by name; and lookups to
// translate a name in the set to its index in a table.
template <const auto& Names, double MaxOverheadRatio = 1.0>
  requires SymbolNameArray<decltype(Names)>
class PerfectSymbolSet {
 public:
  // This is a range of (index, name) pairs giving the indices into the
  // table that are actually in use.
  static constexpr auto IndexMap() {
    auto all_buckets = std::views::iota(size_t{0}, kBuckets);
    auto used_buckets = std::views::filter(all_buckets, [](size_t bucket) {  //
      return kBucketIndex[bucket] > 0;
    });
    return std::views::transform(used_buckets, [](size_t bucket) {
      const size_t i = kBucketIndex[bucket] - 1;
      return std::pair<size_t, const SymbolName&>{bucket, Names[i]};
    });
  }

  // This is how many elements a table needs to have a value for each name.  In
  // the ideal case this is exactly Names.size(), so no elements are wasted.
  static constexpr uint32_t TableSize() { return kBuckets; }

  // This is how many of those elements will go to waste.
  static constexpr uint32_t Overhead() { return TableSize() - Names.size(); }

  // This is the ratio of waste to number of names (the ideal table size).
  static consteval double OverheadRatio() {
    uint32_t overhead = Overhead();
    return overhead == 0 ? 0.0 : static_cast<double>(Names.size()) / overhead;
  }

  // Find a name in the set.  If present, return its table index.
  static constexpr std::optional<uint32_t> Lookup(SymbolName& name) {
    const uint32_t bucket = name.gnu_hash() % kBuckets;
    const size_t i = kBucketIndex[bucket];
    if (i > 0 && Names[i - 1] == name) {
      return bucket;
    }
    return std::nullopt;
  }
  static constexpr std::optional<uint32_t> Lookup(const SymbolName& name) {
    SymbolName copy{name};
    return Lookup(copy);
  }

  // Find a name in the set.  If present, return the iterator to its slot in
  // the range; otherwise, return range.end().
  template <std::ranges::random_access_range Range>
  static constexpr std::ranges::iterator_t<Range> Lookup(Range&& range, SymbolName& name) {
    assert(std::size(range) >= TableSize());
    const std::optional<uint32_t> bucket = Lookup(name);
    return bucket ? range.begin() + *bucket : range.end();
  }
  template <std::ranges::random_access_range Range>
  static constexpr std::ranges::iterator_t<Range> Lookup(Range&& range, const SymbolName& name) {
    SymbolName copy{name};
    return Lookup(std::forward<Range>(range), copy);
  }

 private:
  // Returns the first type in the list that can hold Max.
  template <size_t Max, typename Int, typename... Ints>
  static constexpr auto FirstFit() {
    const auto kIntMax = std::numeric_limits<Int>::max();
    if constexpr (sizeof...(Ints) == 0) {
      static_assert(kIntMax >= Max);
      return Int{};
    } else if constexpr (kIntMax >= Max) {
      return Int{};
    } else {
      return FirstFit<Max, Ints...>();
    }
  }

  // Use the smallest sufficient type for elements in the bucket index.
  using IndexType = decltype(FirstFit<Names.size(), uint8_t, uint16_t, uint32_t>());

  template <size_t N>
  static consteval bool HashCollisions() {
    std::bitset<N> used;
    for (const SymbolName& name : Names) {
      const size_t slot = name.gnu_hash() % N;
      if (used[slot]) {
        return true;
      }
      used[slot] = true;
    }
    return false;
  }

  template <size_t N = Names.size()>
  static consteval size_t PerfectHashSize() {
    if constexpr (HashCollisions<N>()) {
      return PerfectHashSize<N + 1>();
    }
    return N;
  }

  static constexpr size_t kBuckets = PerfectHashSize();

  // The bucket index maps each bucket to an entry in Names.
  // Empty buckets hold 0; others hold 1 + the index into Names.
  using BucketIndex = std::array<IndexType, kBuckets>;

  static constexpr BucketIndex kBucketIndex = []() {
    BucketIndex buckets{};
    for (IndexType i = 0; i < Names.size(); ++i) {
      buckets[Names[i].gnu_hash() % kBuckets] = 1 + i;
    }
    return buckets;
  }();

  static_assert(OverheadRatio() <= MaxOverheadRatio,
                "too much wasted space required for depth-1 hash buckets");
};

// elfldltl::PerfectSymbolMap is a bit like a std::unordered_map<SymbolName, T>
// that has all the Names pre-inserted with default-constructed values.  Its
// operator[] does a quick hash lookup, but there is no .find() method.  It
// doesn't have other container / range methods itself, but its Enumerate()
// method returns a range that is similar to enumerating std::unordered_map.
// Its iterators are different than st::unordered_map iterators, because they
// yield a pair of references rather than a reference to a pair.  So even for
// mutable use, it must be bound with `auto [name, value]` and not `auto&`.
template <typename T, const auto& Names, auto... SetArgs>
  requires std::default_initializable<T> && std::copy_constructible<T> &&
           SymbolNameArray<decltype(Names)>
class PerfectSymbolMap {
 public:
  using Set = PerfectSymbolSet<Names, SetArgs...>;

  constexpr T operator[](SymbolName& name) const {
    if (std::optional bucket = Set::Lookup(name)) {
      return map_[*bucket];
    }
    return T{};
  }
  constexpr T operator[](const SymbolName& name) const {
    SymbolName copy{name};
    return operator[](copy);
  }

  constexpr auto Enumerate() { return Iterable(map_); }
  constexpr auto Enumerate() const { return Iterable(map_); }

 private:
  // This returns a range of the valid map elements rendered as pairs of (name,
  // element_reference).
  static constexpr auto Iterable(auto& map) {
    return std::views::transform(Set::IndexMap(), [&map](const auto& elt) {
      const auto& [i, name] = elt;
      return std::pair<const SymbolName&, T&>{name, map[i]};
    });
  }

  std::array<T, Set::TableSize()> map_{};
};

// elfldltl::PerfectSymbolFilter is a PerfectSymbolMap<const Sym*> with some
// conveniences for using it with the <lib/elfldltl/resolve.h> Module API.
// Initialize it with pointers into a specific module's symbol table.  Then use
// it as a symbol lookup function for the chosen subset of that module.
template <const auto& Names, ElfApi ElfLayout = Elf<>, auto... SetArgs>
  requires SymbolNameArray<decltype(Names)>
class PerfectSymbolFilter : public PerfectSymbolMap<const typename ElfLayout::Sym*, Names> {
 public:
  using Elf = ElfLayout;
  using Sym = Elf::Sym;

  // This takes a Module API object as described in <lib/elfldltl/resolve.h>
  // and uses it to look up each symbol in the set.  If the .Lookup() calls
  // return error, this bails out early with the error value.  If the .Lookup()
  // calls return nullptr "success" (symbol not found), then the corresponding
  // map entry is just set to nullptr.  If the optional undef_ok flag is not
  // set, then this calls diag.UndefinedSymbol for each undefined symbol
  // (bailing out early if it returns false).  The return value is true if all
  // symbols were found or the diagnostics object said to keep going.
  constexpr bool Init(auto& diag, const auto& module, bool undef_ok = false) {
    for (auto [name, sym] : symbols_.Enumerate()) {
      // The Lookup API requires a mutable SymbolName so it can always cache
      // the on-demand hash calculation for a runtime SymbolName.  The name
      // here is constexpr with precomputed hashes so that won't actually be
      // required, but a mutable copy must be passed to the Lookup call.
      elfldltl::SymbolName mutable_name = name;
      auto result = module.Lookup(diag, mutable_name);
      if (result.is_error()) [[unlikely]] {
        return result.error_value();
      }
      sym = *result;
      if (!sym && !undef_ok && !diag.UndefinedSymbol(name)) [[unlikely]] {
        return false;
      }
    }
    return true;
  }

  // A one-argument call just returns the const Sym* or nullptr.
  constexpr const Sym* operator()(SymbolName& name) const { return symbols_[name]; }
  constexpr const Sym* operator()(const SymbolName& name) const {
    SymbolName copy{name};
    return symbols_[copy];
  }

  // This can match the signature of the Lookup method for the Module API in
  // <lib/elfldltl/resolve.h>, where the first argument is some diagnostics
  // object that won't be used here.  Or it can match the signature of
  // ld::RemoteDynamicLinker::Module::SymbolFilter, where the first argument is
  // this same module.
  constexpr fit::result<bool, const Sym*> operator()(auto& ignored_context,
                                                     SymbolName& name) const {
    return fit::ok(symbols_[name]);
  }

 private:
  PerfectSymbolMap<const Sym*, Names, SetArgs...> symbols_;
};

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_PERFECT_SYMBOL_TABLE_H_
