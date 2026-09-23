// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/lazy_symbol.h"

#include "src/developer/debug/zxdb/symbols/symbol.h"
#include "src/developer/debug/zxdb/symbols/symbol_factory.h"

namespace zxdb {

namespace {

// Singleton null symbol to return when a LazySymbol is invalid.
fxl::RefPtr<Symbol> null_symbol;

}  // namespace

LazySymbolBase::LazySymbolBase() = default;

LazySymbolBase::LazySymbolBase(const LazySymbolBase& other) = default;

LazySymbolBase::LazySymbolBase(LazySymbolBase&& other) = default;

LazySymbolBase::LazySymbolBase(fxl::RefPtr<const SymbolFactory> factory, DwarfDieRef die_ref)
    : factory_(std::move(factory)), die_ref_(die_ref) {}

LazySymbolBase::~LazySymbolBase() = default;

LazySymbolBase& LazySymbolBase::operator=(const LazySymbolBase& other) = default;
LazySymbolBase& LazySymbolBase::operator=(LazySymbolBase&& other) = default;

LazySymbol::LazySymbol(const Symbol* symbol) : symbol_(RefPtrTo(symbol)) {
  AssignFactoryAndDieFromSymbol();
}

fxl::RefPtr<Symbol> LazySymbolBase::Construct() const {
  if (is_valid())
    return factory_->CreateSymbol(die_ref_);
  return GetNullSymbol();
}

// static
fxl::RefPtr<Symbol> LazySymbolBase::GetNullSymbol() {
  if (!null_symbol)
    null_symbol = fxl::MakeRefCounted<Symbol>();
  return null_symbol;
}

const fxl::RefPtr<const SymbolFactory>& LazySymbolBase::factory() const { return factory_; }

LazySymbol::LazySymbol() = default;

LazySymbol::LazySymbol(const LazySymbol& other) = default;

LazySymbol::LazySymbol(LazySymbol&& other) = default;

LazySymbol::LazySymbol(fxl::RefPtr<const SymbolFactory> factory, DwarfDieRef die_ref,
                       fxl::RefPtr<Symbol> pre_cached)
    : LazySymbolBase(std::move(factory), die_ref), symbol_(std::move(pre_cached)) {}

LazySymbol::LazySymbol(fxl::RefPtr<const SymbolFactory> factory, DwarfDieRef die_ref)
    : LazySymbol(std::move(factory), die_ref, {}) {}

LazySymbol& LazySymbol::operator=(const LazySymbol& other) = default;
LazySymbol& LazySymbol::operator=(LazySymbol&& other) = default;

LazySymbol::~LazySymbol() = default;

const Symbol* LazySymbol::Get() const {
  if (!symbol_.get()) {
    if (is_valid()) {
      symbol_ = Construct();
    } else {
      // Return the null symbol. Don't populate symbol_ for this case because it will mean
      // is_valid() will always return true.
      return GetNullSymbol().get();
    }
  }
  return symbol_.get();
}

void LazySymbol::AssignFactoryAndDieFromSymbol() {
  if (symbol_)
    LazySymbolBase::operator=(symbol_->lazy_this_);
}

UncachedLazySymbol::UncachedLazySymbol() = default;

UncachedLazySymbol::UncachedLazySymbol(const UncachedLazySymbol& other) = default;

UncachedLazySymbol::UncachedLazySymbol(UncachedLazySymbol&& other) = default;

UncachedLazySymbol::UncachedLazySymbol(fxl::RefPtr<const SymbolFactory> factory,
                                       DwarfDieRef die_ref)
    : LazySymbolBase(std::move(factory), die_ref) {}

UncachedLazySymbol::UncachedLazySymbol(fxl::RefPtr<Symbol> symbol)
    : test_symbol_(std::move(symbol)) {}

UncachedLazySymbol::~UncachedLazySymbol() = default;

UncachedLazySymbol& UncachedLazySymbol::operator=(const UncachedLazySymbol& other) = default;
UncachedLazySymbol& UncachedLazySymbol::operator=(UncachedLazySymbol&& other) = default;

// static
UncachedLazySymbol UncachedLazySymbol::MakeUnsafe(fxl::RefPtr<Symbol> symbol) {
  return UncachedLazySymbol(std::move(symbol));
}

fxl::RefPtr<Symbol> UncachedLazySymbol::Get() const {
  if (test_symbol_)
    return test_symbol_;
  return Construct();
}

LazySymbol UncachedLazySymbol::GetCached(fxl::RefPtr<Symbol> cached_value) const {
  if (test_symbol_)
    return LazySymbol(test_symbol_);
  return LazySymbol(factory(), die_ref(), std::move(cached_value));
}

LazySymbol UncachedLazySymbol::GetCached() const { return GetCached({}); }

}  // namespace zxdb
