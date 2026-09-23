// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_LAZY_SYMBOL_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_LAZY_SYMBOL_H_

#include <stdint.h>

#include <memory>
#include <tuple>

#include "src/developer/debug/zxdb/symbols/dwarf_die_ref.h"
#include "src/lib/fxl/memory/ref_ptr.h"

namespace zxdb {

class Symbol;
class SymbolFactory;

// Symbols can be complex and in many cases are not required. This class holds enough information to
// construct a type from the symbol file as needed. Once constructed, it will cache the type for
// future use.
//
// It can optionally cache the result of the lookup. The rule is that any references that point
// "up" the tree must be uncached to avoid dependency cycles that will cause memory leaks.
class LazySymbolBase {
 public:
  LazySymbolBase();  // Creates a !is_valid() one.
  LazySymbolBase(const LazySymbolBase& other);
  LazySymbolBase(LazySymbolBase&& other);

  LazySymbolBase(fxl::RefPtr<const SymbolFactory> factory, DwarfDieRef die_ref);

  ~LazySymbolBase();

  LazySymbolBase& operator=(const LazySymbolBase& other);
  LazySymbolBase& operator=(LazySymbolBase&& other);

  // LazySymbols have an identity and can be compared for insertion into e.g. sets. These comparison
  // operations assume that the factory pointer is unique for the module.
  bool operator==(const LazySymbolBase& other) const {
    return factory_.get() == other.factory_.get() && die_ref_ == other.die_ref_;
  }
  bool operator!=(const LazySymbolBase& other) const { return !operator==(other); }
  bool operator<(const LazySymbolBase& other) const {
    return std::make_tuple(factory_.get(), die_ref_) <
           std::make_tuple(other.factory_.get(), other.die_ref_);
  }

  // Returns the DIE offset of the symbol that will be created. This will be 0 for invalid or
  // uninitialized LazySymbols, as well as most synthetic symbols (like built-in types and
  // especially types created in unit tests).
  //
  // Do not compare to 0 for testing validity; use is_valid() instead which will handle the
  // synthetic cases.
  //
  // Otherwise, the main use of this can be for comparing symbol identity without decoding them in
  // cases where you know the symbols aren't synthetic.
  DwarfDieRef die_ref() const { return die_ref_; }

 protected:
  // Validity tests both for the factory and the symbol since non-lazy ones don't need a factory.
  // Not exposed publicly because the derived classes need to add additional conditions.
  bool is_valid() const { return factory_.get(); }

  fxl::RefPtr<Symbol> Construct() const;

  // Returns a cached null symbol for error cases.
  static fxl::RefPtr<Symbol> GetNullSymbol();

  const fxl::RefPtr<const SymbolFactory>& factory() const;

 private:
  // May be null if this contains no type reference.
  fxl::RefPtr<const SymbolFactory> factory_;

  // Reference to the DIE for this symbol.
  DwarfDieRef die_ref_;
};

// Use for references from a parent symbol object to its children.
class LazySymbol : public LazySymbolBase {
 public:
  LazySymbol();  // Creates a !is_valid() one.
  LazySymbol(const LazySymbol& other);
  LazySymbol(LazySymbol&& other);
  ~LazySymbol();

  // If the value of the cached object is known at creation time, it can be provided as the
  // pre_cached parameter. Otherwise it is fine to leave this empty (it is an optimization to
  // prevent re-decoding).
  LazySymbol(fxl::RefPtr<const SymbolFactory> factory, DwarfDieRef die_ref,
             fxl::RefPtr<Symbol> pre_cached);
  LazySymbol(fxl::RefPtr<const SymbolFactory> factory, DwarfDieRef die_ref);

  // Implicitly creates a non-lazy one with a pre-cooked object, mostly for tests.
  //
  // Take the symbol factory and DIE offset from the Symbol. This is used for some comparison
  // operations so must be set even when we have a symbol object already.
  template <class SymbolType>
  LazySymbol(fxl::RefPtr<SymbolType> symbol) : LazySymbolBase(), symbol_(std::move(symbol)) {
    AssignFactoryAndDieFromSymbol();
  }
  LazySymbol(const Symbol* symbol);

  bool is_valid() const { return LazySymbolBase::is_valid() || symbol_.get(); }
  explicit operator bool() const { return is_valid(); }

  LazySymbol& operator=(const LazySymbol& other);
  LazySymbol& operator=(LazySymbol&& other);

  // Returns the type associated with this LazySymbol. If this class is invalid or the symbol fails
  // to resolve this will return an empty one. It will never return null.
  const Symbol* Get() const;

 private:
  // This is an annoying function used as a helper for the template constructor above that extracts
  // the factory and DIE offset from the current symbol_ and sets it as the source for this class.
  //
  // This is needed to extract the uses of the Symbol object to the .cc file to avoid circular
  // dependencies.
  void AssignFactoryAndDieFromSymbol();

  mutable fxl::RefPtr<Symbol> symbol_;
};

// Use for references from a child symbol object to its parent.
class UncachedLazySymbol : public LazySymbolBase {
 public:
  UncachedLazySymbol();  // Creates a !is_valid() one.
  UncachedLazySymbol(const UncachedLazySymbol& other);
  UncachedLazySymbol(UncachedLazySymbol&& other);

  UncachedLazySymbol(fxl::RefPtr<const SymbolFactory> factory, DwarfDieRef die_ref);

  ~UncachedLazySymbol();

  bool is_valid() const { return LazySymbolBase::is_valid() || test_symbol_.get(); }
  explicit operator bool() const { return is_valid(); }

  UncachedLazySymbol& operator=(const UncachedLazySymbol& other);
  UncachedLazySymbol& operator=(UncachedLazySymbol&& other);

  // Returns the type associated with this LazySymbol. If this class is invalid or the symbol fails
  // to resolve this will return an empty one. It will never return a null pointer.
  fxl::RefPtr<Symbol> Get() const;

  // Returns a LazySymbol that references the same symbol as this object. This can be used in cases
  // where you need a LazySymbol from an uncached one. This is safe as long as you're not storing it
  // in such a way that it will create a reference cycle.
  //
  // If known in advance, the cached result may be provided as an optimization.
  LazySymbol GetCached(fxl::RefPtr<Symbol> cached_value) const;
  LazySymbol GetCached() const;

  // Makes an object with a static reference to an explicit symbol. Used for tests.
  //
  // Most code should use SymbolParentSetter which is less likely to cause leaks in tests. This
  // should normally only be called with a new object with no references to its children to avoid
  // a cycle. The normal example is the code that sets a mock unit for a test symbol in order to
  // control its language.
  static UncachedLazySymbol MakeUnsafe(fxl::RefPtr<Symbol> symbol);

 private:
  // Creates a non-lazy one with a pre-cooked object.
  UncachedLazySymbol(fxl::RefPtr<Symbol> symbol);

  // Used for injecting mock symbols for tests. See SymbolTestParentSetter.
  fxl::RefPtr<Symbol> test_symbol_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_LAZY_SYMBOL_H_
