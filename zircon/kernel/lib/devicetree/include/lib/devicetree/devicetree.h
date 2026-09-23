// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_DEVICETREE_H_
#define ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_DEVICETREE_H_

#include <lib/fit/result.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <cstdint>
#include <iterator>
#include <numeric>
#include <optional>
#include <span>
#include <string_view>
#include <type_traits>
#include <utility>

#include <fbl/intrusive_container_utils.h>
#include <fbl/intrusive_double_list.h>

#include "internal/devicetree.h"

// This library provides abstractions and utilities for dealing with
// 'devicetree's in their flattened, binary form (.dtb). Although not used in
// Fuchsia-compliant bootloaders (which use the ZBI protocol), dealing with
// devicetrees is necessary for our boot shims.
//
// We follow the v0.3 spec available at
// https://devicetree-specification.readthedocs.io/en/v0.3

namespace devicetree {

using ByteView = std::span<const uint8_t>;

// See
// https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#address-cells-and-size-cells
constexpr uint32_t kRegDefaultAddressCells = 2;
constexpr uint32_t kRegDefaultSizeCells = 1;

// Represents a tuple of N-elements encoded as collection of cells. Each cell is a 32 bit big endian
// unsigned integer.
//     A   B      C   D
//    u32 u32    u32 u32 => 2 tuple of a 16 byte element. 2 cells per tuple element.
//
//    field 1  = A << 32 | B
//    field 2  = C << 32 | D
//
//  A, B, C and D are the endian decoded values of each cell, that is from big endian to the
//  platforms endianess.
//
// This class represents the individual elements of each `prop-encoded-array`.
template <size_t N>
class PropEncodedArrayElement {
 public:
  static constexpr size_t kFields = N;

  constexpr PropEncodedArrayElement() = default;
  constexpr PropEncodedArrayElement(const PropEncodedArrayElement&) = default;
  constexpr PropEncodedArrayElement& operator=(const PropEncodedArrayElement&) = default;
  constexpr PropEncodedArrayElement(ByteView raw_element, const std::array<size_t, N>& num_cells) {
    size_t offset = 0;
    for (size_t i = 0; i < N; ++i) {
      if (num_cells[i] == 0) {
        continue;
      }
      elements_[i] = internal::ParseCells(
          raw_element.subspan(offset, sizeof(uint32_t) * num_cells[i]), uint32_t(num_cells[i]));
      offset += static_cast<size_t>(num_cells[i]) * sizeof(uint32_t);
    }
  }

  constexpr std::optional<uint64_t> operator[](size_t index) const { return elements_[index]; }

 private:
  std::array<std::optional<uint64_t>, N> elements_;
};

// Represents a `prop-encoded-array`, where each element is an N-Tuple, represented by
// PropEncodedArrayElement<N>.
//
// To represent specific properties |DecodedProperty| must provide |DecodedProperty::kFields|
// as the value of N for the array elements. Additionally it is recommended that |DecodedProperty|
// inherits from |PropEncodedArrayElement| and add accessors as needed for each element of the
// tuple.
//
// See |RegProperty| and |RegPropertyElement| for an example.
template <typename ElementType, size_t N = ElementType::kFields>
class PropEncodedArray {
 public:
  static_assert(std::is_base_of_v<PropEncodedArrayElement<N>, ElementType>);

  constexpr PropEncodedArray() = default;

  template <typename... Elements>
  explicit constexpr PropEncodedArray(ByteView data, Elements... num_cells)
      : cells_for_elements_({static_cast<size_t>(num_cells)...}),
        entry_size_(std::accumulate(
            cells_for_elements_.begin(), cells_for_elements_.end(), 0,
            [](size_t acc, size_t num_cells) { return acc + num_cells * sizeof(uint32_t); })),
        prop_encoded_raw_(data) {
    static_assert(sizeof...(Elements) == N);
    ZX_ASSERT(prop_encoded_raw_.size() % entry_size_ == 0);
  }

  constexpr ElementType operator[](size_t index) const {
    ZX_ASSERT(index * entry_size_ < prop_encoded_raw_.size());
    return ElementType(prop_encoded_raw_.subspan(index * entry_size_, entry_size_),
                       cells_for_elements_);
  }

  // Returns the number of entries in the array, accounting for the number of cells per entry.
  constexpr size_t size() const {
    if (entry_size_ == 0) {
      return 0;
    }
    return prop_encoded_raw_.size() / entry_size_;
  }

 private:
  std::array<size_t, N> cells_for_elements_ = {};
  size_t entry_size_ = 0;
  ByteView prop_encoded_raw_;
};

// Represents the node name of a devicetree. This has the same API as
// std::string_view and is meant to be used the same way. It requires its own
// type solely to enable use of the fbl intrusive containers with on-stack
// elements to avoid dynamic allocation.
struct Node : public std::string_view,
              fbl::DoublyLinkedListable<Node*, fbl::NodeOptions::AllowCopy> {
  Node(std::string_view name) : std::string_view(name) {}

  // See
  // https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#node-name-requirements
  // for specification and definition of name and unit address.
  constexpr std::string_view name() const {
    size_t ind = find_first_of('@');
    return (ind == std::string_view::npos) ? static_cast<std::string_view>(*this) : substr(0, ind);
  }

  // See
  // https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#node-name-requirements
  // for specification and definition of name and unit address.
  constexpr std::string_view address() const {
    size_t ind = find_first_of('@');
    return (ind == std::string_view::npos) ? std::string_view() : substr(ind + 1);
  }
};

// Forward-declared; defined below.
struct ResolvedPath;

// Represents a rooted path of nodes in a devicetree.
// This can be used interchangeably with `const std::list<std::string_view>`
// to iterate over the elements in a path with implied `/` separators.
struct NodePath
    : public fbl::DoublyLinkedList<Node*, fbl::DefaultObjectTag, fbl::SizeOrder::Constant> {
  // The result of a path comparison between this NodePath and another type
  // encoding a node path.
  enum class Comparison {
    // This node is equal to another.
    kEqual,

    // This node is the parent of another.
    kParent,

    // This node is the indirect ancestor of another.
    kIndirectAncestor,

    // This node is the child of another.
    kChild,

    // This node is the indirect descendent of another.
    kIndirectDescendent,

    // This node does not share a lineage with another node.
    kMismatch,
  };

  //
  // Comparisons with a `std::string_view`-encoded node path.
  //

  bool operator==(std::string_view path) const { return CompareWith(path) == Comparison::kEqual; }

  bool operator!=(std::string_view path) const { return !(*this == path); }

  bool IsParentOf(std::string_view path) const { return CompareWith(path) == Comparison::kParent; }

  bool IsAncestorOf(std::string_view path) const {
    Comparison result = CompareWith(path);
    return result == Comparison::kParent || result == Comparison::kIndirectAncestor;
  }

  bool IsChildOf(std::string_view path) const { return CompareWith(path) == Comparison::kChild; }

  bool IsDescendentOf(std::string_view path) const {
    Comparison result = CompareWith(path);
    return result == Comparison::kChild || result == Comparison::kIndirectDescendent;
  }

  Comparison CompareWith(std::string_view path) const;

  //
  // Comparisons with a `ResolvedPath`-encoded node path.
  //

  bool operator==(const ResolvedPath& path) const {
    return CompareWith(path) == Comparison::kEqual;
  }

  bool IsParentOf(const ResolvedPath& path) const {
    return CompareWith(path) == Comparison::kParent;
  }

  bool IsAncestorOf(const ResolvedPath& path) const {
    Comparison result = CompareWith(path);
    return result == Comparison::kParent || result == Comparison::kIndirectAncestor;
  }

  bool IsChildOf(const ResolvedPath& path) const { return CompareWith(path) == Comparison::kChild; }

  bool IsDescendentOf(const ResolvedPath& path) const {
    Comparison result = CompareWith(path);
    return result == Comparison::kChild || result == Comparison::kIndirectDescendent;
  }

  Comparison CompareWith(const ResolvedPath& path) const;
};

// Some property values encode a list of NUL-terminated strings.
// This is also useful for separating path strings at '/' characters.
//
// By convention, if the last character in a string is a separator, then there
// is no element regarded as following it.
template <char Separator = '\0'>
class StringList {
 public:
  class iterator {
   public:
    using difference_type = ptrdiff_t;
    using value_type = std::string_view;
    using pointer = std::string_view*;
    using reference = std::string_view&;
    using iterator_category = std::forward_iterator_tag;

    constexpr iterator() = default;
    constexpr iterator(const iterator&) = default;
    constexpr iterator& operator=(const iterator&) = default;

    constexpr bool operator==(const iterator& other) const {
      return len_ == other.len_ && rest_.size() == other.rest_.size();
    }
    constexpr bool operator!=(const iterator& other) const { return !(*this == other); }

    constexpr iterator& operator++() {  // prefix
      if (len_ == std::string_view::npos || len_ == rest_.size() - 1) {
        // We're at the end.
        *this = {};
      } else {
        // Move to the next word.  If it's empty, record len_ = 0.
        // Otherwise len_ = npos if it's the last word and not empty.
        rest_ = rest_.substr(len_ + 1);
        len_ = rest_.empty() ? 0 : rest_.find_first_of(Separator);
      }
      return *this;
    }

    constexpr iterator operator++(int) {  // postfix
      iterator old = *this;
      ++*this;
      return old;
    }

    constexpr std::string_view operator*() const { return rest_.substr(0, len_); }

   private:
    friend StringList;

    std::string_view rest_;
    size_t len_ = std::string_view::npos;
  };
  using const_iterator = iterator;

  constexpr explicit StringList(std::string_view str) : data_(str) {}

  constexpr iterator begin() const {
    iterator it;
    it.rest_ = data_;
    it.len_ = data_.find_first_of(Separator);
    return it;
  }

  constexpr iterator end() const { return iterator{}; }

 private:
  std::string_view data_;
};

// Forward declaration of PropertyDecoder, see below.
class PropertyDecoder;

// Represents a `reg` property, which usually encodes mmio mapped registers for a device.
//
// This property usually encodes series or memory-mapped registers as pairs of [address, size] where
// each entry in the pair is defined by a sequence of cells. For the address the number of cells is
// determined by |num_address_cells| and for the size the number of cells is determined by
// `num_size_cells`.
//
// See
// https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#reg
// for property description.
class RegPropertyElement : public PropEncodedArrayElement<2> {
 public:
  using PropEncodedArrayElement<2>::PropEncodedArrayElement;

  constexpr std::optional<uint64_t> address() const { return (*this)[0]; }
  constexpr std::optional<uint64_t> size() const { return (*this)[1]; }
};

class RegProperty : public PropEncodedArray<RegPropertyElement> {
 public:
  static std::optional<RegProperty> Create(uint32_t num_address_cells, uint32_t num_size_cells,
                                           ByteView bytes);

  static std::optional<RegProperty> Create(const PropertyDecoder& decoder, ByteView bytes);

 private:
  using PropEncodedArray<RegPropertyElement>::PropEncodedArray;
};

// See
// https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#ranges
// for property description.
class RangesPropertyElement : public PropEncodedArrayElement<3> {
 public:
  using PropEncodedArrayElement<3>::PropEncodedArrayElement;

  constexpr std::optional<uint64_t> child_bus_address() const { return (*this)[0]; }
  constexpr std::optional<uint64_t> parent_bus_address() const { return (*this)[1]; }
  constexpr std::optional<uint64_t> length() const { return (*this)[2]; }
};

class RangesProperty : public PropEncodedArray<RangesPropertyElement> {
 public:
  static std::optional<RangesProperty> Create(uint32_t num_address_cells, uint32_t num_size_cells,
                                              uint32_t num_parent_address_cells, ByteView bytes);

  static std::optional<RangesProperty> Create(const PropertyDecoder& decoder, ByteView bytes);

  // Returns the translated address from a child address to parent address.
  //
  // Essentially a ranges property dictates how addresses in child nodes are translated
  // into parent node address range. For example:
  //
  // '/' {
  //   foo {
  //     ranges = <0 123 4> <4 256 4>
  //     bar {
  //       reg = <0 2>
  //     }
  // }
  //
  // If we wanted to translate the register bank of device node |bar|, we would need to use the
  // ranges property, such that the child bus address is translated to the parent bus address (in
  // this case, this is the root), To achieve that would look for the first range where the address
  // would fit, in this case that is the first range. This range tells us that child addresses from
  // [0, 4] are at offset 123 from the parent bus address, so the parent address for 'bar' node
  // would be '123 + 0'. This address would then be used for mmio access in the case of register
  // banks.
  constexpr std::optional<uint64_t> TranslateChildAddress(uint64_t address) const {
    // No rangees (ranges = <empty>) means identity mapping, which is not the same
    // as missing |ranges|.

    if (size() == 0) {
      return address;
    }

    for (size_t i = 0; i < size(); ++i) {
      auto range = (*this)[i];
      if (!range.child_bus_address() || !range.parent_bus_address() || !range.length()) {
        continue;
      }
      if (address >= *range.child_bus_address() &&
          address < *range.child_bus_address() + *range.length()) {
        return address - *range.child_bus_address() + *range.parent_bus_address();
      }
    }
    // Something went wrong, we couldnt find a valid range to translate the addres.
    return std::nullopt;
  }

 private:
  using PropEncodedArray<RangesPropertyElement>::PropEncodedArray;
};

// Represents a `status` property, which indicates if the containing device node
// is in  a usable state.
//
// See
// https://devicetree-specification.readthedocs.io/en/latest/chapter2-devicetree-basics.html#status
// for more details.
class StatusProperty {
 public:
  static std::optional<StatusProperty> Create(std::optional<std::string_view> prop_value);

  constexpr StatusProperty() = default;

  constexpr bool is_okay() const { return status_ == Status::kOkay; }
  constexpr bool is_disabled() const { return status_ == Status::kDisabled; }

  constexpr bool is_fail() const {
    return status_ == Status::kFail || status_ == Status::kFailWithCode;
  }

  constexpr std::optional<std::string_view> fail_code() const {
    if (status_ != Status::kFailWithCode) {
      return std::nullopt;
    }
    return code_;
  }

 private:
  enum class Status : uint8_t {
    kOkay,
    kDisabled,
    kFail,
    kFailWithCode,
    kUnknown,
  };

  explicit constexpr StatusProperty(Status status, std::string_view code = std::string_view())
      : status_(status), code_(code) {}

  Status status_ = Status::kUnknown;

  // Only available when `status_` is `kFailWithCode`.
  // Stores the `sss` portion of `fail-sss` status.
  std::string_view code_;
};

// See
// https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#property-values
// for the types and representations of possible property values.
class PropertyValue {
 public:
  constexpr PropertyValue(ByteView bytes) : bytes_(bytes) {}

  ByteView AsBytes() const { return bytes_; }

  std::optional<std::string_view> AsString() const {
    if (bytes_.empty()) {
      return std::nullopt;
    }

    // While a string property is spec'd as including a NUL terminator, per
    // Postel, we opt not to enforce that, trimming the NUL only if present.
    std::string_view str{reinterpret_cast<const char*>(bytes_.data()), bytes_.size()};
    if (str.back() == '\0') {
      str.remove_suffix(1);
    }
    return str;
  }

  std::optional<StringList<>> AsStringList() const {
    auto string = AsString();
    if (string == std::nullopt) {
      return std::nullopt;
    }
    return StringList<>{*string};
  }

  std::optional<uint32_t> AsUint32() const;

  std::optional<uint64_t> AsUint64() const;

  // A value without size represents a Boolean property whose truthiness is a
  // function of the nature of the property's name and its presence in the tree.
  std::optional<bool> AsBool() const {
    if (!bytes_.empty()) {
      return std::nullopt;
    }
    return true;
  }

  // Returns a valid `StatusProperty` or `std::nullopt` in case the provided string does not match
  // any known value.
  std::optional<StatusProperty> AsStatus() const { return StatusProperty::Create(AsString()); }

  // Given a device node's property decoder |decoder|, the property value is assumed
  // to be a property encoded array, to be interpreted as 'reg'.
  //
  // The number of address cells and size cells are obtained from the parent, or if
  // not present they are assumed to be 2 for address cells or 1 for size cells.
  //
  // See:
  //  * 'reg'
  //  https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#reg
  //  * defaults:
  //  https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#address-cells-and-size-cells
  std::optional<RegProperty> AsReg(const PropertyDecoder& decoder) const {
    return RegProperty::Create(decoder, bytes_);
  }

  // Attempts to parse the bytes a 'ranges' property. A ranges property consist of an array of
  // 3-Tuples, the first element being the child bus address, then the parent bus address and the
  // length of the range last. The number of cells of each entry is obtained as follows:
  //   * 'address-cells' for child bus address from decoder itself.
  //   * 'address-cells' for parent bus address from decoder's parent.
  //   * 'size-cells' for length from decoder itself.
  //
  // See 'ranges'
  // https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#ranges
  std::optional<RangesProperty> AsRanges(const PropertyDecoder& decoder) const {
    return RangesProperty::Create(decoder, bytes_);
  }

 private:
  ByteView bytes_;
};

struct Property {
  std::string_view name;
  PropertyValue value;
};

// A view-like object representing a set of properties given by a range within the
// structure block of a flattened devicetree.
// https://devicetree-specification.readthedocs.io/en/v0.3/flattened-format.html#structure-block
class Properties {
 public:
  constexpr Properties() = default;
  constexpr Properties(const Properties& other) = default;
  constexpr Properties& operator=(const Properties&) = default;

  // Constructed from a byte span beginning just after the first internal::FDT_PROP token
  // in a flattened block of properties (or an otherwise empty one), along with
  // a string block for property name look-up.
  constexpr Properties(ByteView property_block, std::string_view string_block)
      : property_block_(property_block), string_block_(string_block) {}

  // A property iterator is identified with the position in a block of
  // properties at which a property is encoded. Incrementing the iterator
  // amounts to seeking in the range for the offset of the next property.
  //
  // It implements std's LegacyInputIterator.
  // https://en.cppreference.com/w/cpp/named_req/InputIterator
  class iterator {
   public:
    using value_type = Property;
    using difference_type = ptrdiff_t;  // std::next complains without it.
    using pointer = Property*;
    using reference = Property&;
    using iterator_category = std::input_iterator_tag;

    iterator() = default;

    iterator(const iterator&) = default;

    iterator& operator=(const iterator&) = default;

    bool operator==(const iterator& it) const {
      return position_.data() == it.position_.data() && position_.size() == it.position_.size();
    }

    bool operator!=(const iterator& it) const { return !(*this == it); }

    Property operator*() const;

    iterator& operator++();  // prefix incrementing.

    iterator operator++(int) {  // postfix incrementing.
      auto prev = *this;
      ++(*this);
      return prev;
    }

   private:
    // Only to be called by Properties::begin and Properties::end().
    constexpr iterator(ByteView position, std::string_view string_block)
        : position_(position), string_block_(string_block) {}
    friend class Properties;

    // Pointer into the block of properties along with remaining size from that
    // point onward.
    ByteView position_;
    std::string_view string_block_;
  };

  constexpr iterator begin() const { return iterator{property_block_, string_block_}; }

  constexpr iterator end() const { return iterator{{property_block_.end(), 0}, string_block_}; }

 private:
  ByteView property_block_;
  std::string_view string_block_;
};

struct MemoryReservation {
  constexpr uint64_t end() const { return start + size; }

  uint64_t start;
  uint64_t size;
};

class MemoryReservations {
 public:
  using value_type = MemoryReservation;

  class iterator {
   public:
    iterator() = default;
    iterator(const iterator&) = default;
    iterator& operator=(const iterator&) = default;

    bool operator==(const iterator& other) const {
      return mem_rsvmap_.size() == other.mem_rsvmap_.size();
    }
    bool operator!=(const iterator& other) const { return !(*this == other); }

    iterator& operator++();  // prefix

    iterator operator++(int) {  // postfix
      iterator old = *this;
      ++*this;
      return old;
    }

    value_type operator*() const;

   private:
    friend MemoryReservations;

    void Normalize();

    ByteView mem_rsvmap_;
  };
  using const_iterator = iterator;

  iterator begin() const;

  iterator end() const { return iterator{}; }

 private:
  friend class Devicetree;

  ByteView mem_rsvmap_;
};

// A path is translated into two absolute components, |prefix| and |suffix|. The full absolute path
// is the concatenation of |prefix|/|suffix|.
// This allows an aliased path "alias/suffix_path" to become:
//     alias: "/real_path/"
//     prefix: "/real_path"
//     suffix: "suffix_path"
//
// When the path is not aliased, "/real_path/suffix_path" then:
//     prefix: "/real_path/suffix_path"
//     suffix: ""
struct ResolvedPath {
  using Components = StringList<'/'>;

  // Helpers to iterate over respective components.
  Components Prefix() const { return Components(prefix); }

  Components Suffix() const { return Components(suffix); }

  std::string_view prefix;
  std::string_view suffix;
};

// Property decoder provides an interface for accessing |devicetree::Properties|
// during a |Devicetree::Walk|.
//
// This class preserves node hierarchy of the properties, such that properties of arbitrary nodes
// along the path can be accessed. This allows for things like register blocks, interrupts or others
// to be parsed correctly.
//
// This class' object lifetime is tied to the walk, and no references or pointers should be kept
// around, without this consideration.
class PropertyDecoder {
 public:
  // Possible errors when resolving a path.
  enum class PathResolveError {
    // Alias data was available, but the alias had no match.
    kBadAlias,
    // Alias data is not yet available.
    kNoAliases,
  };

  constexpr PropertyDecoder() = default;
  constexpr PropertyDecoder(const PropertyDecoder&) = delete;
  constexpr PropertyDecoder(PropertyDecoder&&) = delete;
  explicit constexpr PropertyDecoder(PropertyDecoder* parent, Properties properties,
                                     const std::optional<Properties>& aliases)
      : parent_(parent),
        properties_(std::move(properties)),
        aliases_(aliases ? &*aliases : nullptr),
        cached_properties_(properties_.begin()) {}
  explicit constexpr PropertyDecoder(PropertyDecoder* parent, Properties properties)
      : parent_(parent),
        properties_(std::move(properties)),
        cached_properties_(properties_.begin()) {}
  explicit constexpr PropertyDecoder(Properties properties)
      : PropertyDecoder(nullptr, std::move(properties)) {}

  // Returns the decoder of the current node's parent if any.
  // Root node will return |nullptr|.
  constexpr PropertyDecoder* parent() const { return parent_; }

  // Attempts to find a list of unique property names in a single iteration of the available
  // properties. If the property is not found, and |std::nullopt| is returned in the respective
  // location.
  //
  // E.g.:
  // PropertyDecoder decoder(...);
  // auto [name, foo, bar, other_foo] = decoder.FindProperties("name", "foo", "bar", "other_foo");
  //
  // if (name.has_value()) ....
  template <typename... PropertyNames>
  constexpr std::array<std::optional<PropertyValue>, sizeof...(PropertyNames)> FindProperties(
      PropertyNames&&... property_names) const {
    return FindProperties(std::make_index_sequence<sizeof...(PropertyNames)>{},
                          std::forward<PropertyNames>(property_names)...);
  }

  constexpr std::optional<PropertyValue> FindProperty(std::string_view name) const {
    return FindProperties(name)[0];
  }

  // Performs property lookup and decoding, using the provided |PropertyValue::*| method pointer.
  template <auto PropertyValue::* ValueDecoder, typename... Args>
  constexpr auto FindAndDecodeProperty(std::string_view name, Args&&... args) const {
    using ReturnValue = std::invoke_result_t<decltype(ValueDecoder), PropertyValue, Args...>;
    auto prop = FindProperty(name);
    if (!prop) {
      return ReturnValue{std::nullopt};
    }

    auto* prop_value = &(*prop);
    return (prop_value->*ValueDecoder)(std::forward<Args>(args)...);
  }

  // On success, returns a |ResolvedPath| that contains the resolved aliases prefix and the suffix.
  // On failure, when an aliased path is met, the returned value is true if the caller should try
  // later (e.g. aliases node has not been found yet) or false if the aliases node was found, but
  // the alias was not.
  fit::result<PathResolveError, ResolvedPath> ResolvePath(
      std::string_view maybe_aliased_path) const;

  // Underlying |Properties| object.
  constexpr const Properties& properties() const { return properties_; }

  // Cached special '#' properties.
  constexpr std::optional<uint32_t> num_address_cells() const {
    return cached_properties_.LookUp(cached_properties_.address, properties_.end());
  }
  constexpr std::optional<uint32_t> num_size_cells() const {
    return cached_properties_.LookUp(cached_properties_.size, properties_.end());
  }
  constexpr std::optional<uint32_t> num_interrupt_cells() const {
    return cached_properties_.LookUp(cached_properties_.interrupt, properties_.end());
  }
  constexpr std::optional<StatusProperty> status() const {
    return cached_properties_.LookUp(cached_properties_.status, properties_.end());
  }
  constexpr bool is_status_ok() const {
    auto s = status();
    return !s || s->is_okay();
  }

  // Translate |address| from this node's view to the root's view of the address.
  // Each bus translation is commonly defined by |ranges| property. Lack of this property
  // means no translation is possible or some other means of translation needs to happen.
  //
  // This method will recursively translate address into parent's bus until it meets a node without
  // a |ranges| property. Root node is not allowed to have a |ranges| property.
  std::optional<uint64_t> TranslateAddress(uint64_t address) const;

 private:
  struct CachedProperties {
    explicit constexpr CachedProperties() = default;
    explicit constexpr CachedProperties(Properties::iterator start) : next(start) {}

    void TryToPopulate() {
      const auto& [name, value] = *(next++);

      // Not a special property.
      if (name == "status") {
        // Reserve `nullopt` only for no `status` property at all, since that is equivalent to
        // "okay". The default ctor of `StatusProperty` is `kUnknown`, which fits this state, and
        // disambiguates no property at all from property with bad value.
        status = value.AsStatus().value_or(StatusProperty{});
        return;
      }

      if (name[0] != '#') {
        return;
      }

      if (!address && name == "#address-cells") {
        address = value.AsUint32();
        return;
      }

      if (!size && name == "#size-cells") {
        size = value.AsUint32();
        return;
      }

      if (!interrupt && name == "#interrupt-cells") {
        interrupt = value.AsUint32();
        return;
      }
    }

    template <typename T>
    constexpr std::optional<T>& LookUp(std::optional<T>& value, Properties::iterator end) {
      while (!value && next != end) {
        TryToPopulate();
      }
      return value;
    }

    // Points to the next property to keep searching for unseen cacheable
    // properties when trying to query them on demand.
    Properties::iterator next;

    // Actual properties.
    std::optional<uint32_t> address;
    std::optional<uint32_t> size;
    std::optional<uint32_t> interrupt;
    std::optional<StatusProperty> status;
  };

  template <size_t... Is, typename... PropertyNames>
  constexpr std::array<std::optional<PropertyValue>, sizeof...(PropertyNames)> FindProperties(
      std::index_sequence<Is...> is, PropertyNames&&... property_names) const {
    std::array<std::optional<PropertyValue>, sizeof...(PropertyNames)> result = {};
    int count = 0;
    auto try_populate = [&](auto name, size_t index, const Property& property, bool& matched) {
      if (!matched && !result[index].has_value() && property.name == name) {
        matched = true;
        result[index] = property.value;
        count++;
      }
      return !matched;
    };

    // Like an if/elseif branch, where |matched| determines whether the branch
    // should be evaluated or not.
    Visit([&](auto property) {
      bool matched = false;
      std::ignore = ((try_populate(property_names, Is, property, matched)) && ...);
      return count != sizeof...(PropertyNames);
    });

    return result;
  }

  template <typename Visitor>
  constexpr void Visit(Visitor&& visitor) const {
    for (auto it = properties_.begin(); it != properties_.end(); ++it) {
      // If we reach the last processed cell count iterator, bump it to
      // avoid doing duplicate iteration later.
      if (cached_properties_.next == it) {
        cached_properties_.TryToPopulate();
      }
      if (!visitor(*it)) {
        break;
      }
    }
  }

  PropertyDecoder* parent_ = nullptr;
  Properties properties_;
  const Properties* aliases_ = nullptr;
  mutable CachedProperties cached_properties_;
};

// Represents a devicetree. This class does not dynamically allocate
// memory and is appropriate for use in all low-level environments.
class Devicetree {
 private:
  // A Walker for visiting each node. It is presented as a type erased
  // callback that will do proper cast.
  template <typename ReturnType>
  class NodeVisitor {
   public:
    using TypedCallback = ReturnType (*)(const NodePath&, const PropertyDecoder&);

    template <typename TypedCaller>
    explicit constexpr NodeVisitor(TypedCaller& callback)
        : typed_callback_(
              [](void* ctx, const NodePath& path, const PropertyDecoder& decoder) -> ReturnType {
                if constexpr (std::is_void_v<ReturnType>) {
                  (*static_cast<TypedCaller*>(ctx))(path, decoder);
                } else {
                  return (*static_cast<TypedCaller*>(ctx))(path, decoder);
                }
              }),
          callback_(&callback) {
      static_assert(
          std::is_invocable_r_v<ReturnType, TypedCaller, const NodePath&, PropertyDecoder&>,
          "Invalid signature for NodeVisitor");
    }

    ReturnType operator()(const NodePath& path, const PropertyDecoder& decoder) const {
      return typed_callback_(callback_, path, decoder);
    }

   private:
    using CallbackType = ReturnType (*)(void*, const NodePath&, const PropertyDecoder&);
    CallbackType typed_callback_;
    void* callback_;
  };

  // Pre order is able to prune walks.
  using PreOrderNodeVisitor = NodeVisitor<bool>;
  // Post order cant, since the subtree has already been visited.
  using PostOrderNodeVisitor = NodeVisitor<void>;

 public:
  explicit Devicetree() = default;

  // Consumes a view representing the range of memory the flattened devicetree
  // is expected to take up, its beginning pointing to that of the binary data
  // and its size giving an upper bound on the size that the data is permitted
  // to occupy.
  //
  // It is okay to pass a view size of SIZE_MAX if an upper bound on the size
  // is not known; only up to the size encoded in the devicetree header will
  // be dereferenced.
  explicit Devicetree(ByteView fdt);
  Devicetree(const Devicetree& other) = default;
  explicit Devicetree(std::span<const std::byte> fdt)
      : Devicetree(ByteView{reinterpret_cast<const uint8_t*>(fdt.data()), fdt.size()}) {}

  Devicetree& operator=(const Devicetree& rhs) = default;

  ByteView fdt() const { return fdt_; }

  // The size in bytes of the flattened devicetree blob.
  size_t size_bytes() const { return fdt_.size(); }

  // Walk provides a means of walking a devicetree. It purposefully avoids
  // reliance on specifying a 'walker' by means of inheritance so as to avoid
  // vtables, which are not permitted in the phys environment. A WalkCallback
  // here is an object callable with the signature of
  // `(const NodePath&, Properties) -> bool`
  // and is called depth-first at every node for which no ancestor node
  // returned false. That is, if a walker returns false at a given node, then
  // the subtree rooted there and its member nodes are said to be "pruned" and
  // the walker will not be called on them.
  //
  // There are two flavors of this method:
  //
  // Pre-Order Traversal Only: A single visitor is supplied, and will be called
  // on every node following a pre order traversal.
  //
  // Pre-Order and Post-Order Traversal: A pair of visitors are supplied,
  // each callback is called once per visited node. The Pre-Order callback
  // is called before visiting the offspring(Pre-Order Traversal) and the
  // Post-Order callback is called after visiting a node's offspring.
  //
  // This method will only have one instantiation in practice (in any
  // conceivable context), so the templating should not result in undue bloat.
  //
  // TODO: If boot shims start using multiple walks, refactor this to move the
  // logic into a non-template function and use a function pointer callback,
  // with this templated wrapper calling that with a captureless lambda to call
  // the templated walker.
  template <typename Visitor>
  void Walk(Visitor&& visitor) const {
    return Walk(std::forward<Visitor>(visitor),
                [](const auto& NodePath, const PropertyDecoder& props) {});
  }

  template <typename PreOrderVisitor, typename PostOrderVisitor>
  void Walk(PreOrderVisitor&& pre_order_visitor, PostOrderVisitor&& post_order_visitor) const {
    WalkInternal(pre_order_visitor, post_order_visitor);
  }

  MemoryReservations memory_reservations() const {
    MemoryReservations result;
    result.mem_rsvmap_ = mem_rsvmap_;
    return result;
  }

 private:
  Devicetree(ByteView fdt, ByteView struct_block, std::string_view string_block)
      : fdt_(fdt), struct_block_(struct_block), string_block_(string_block) {}

  // Given a byte span that starts at a flattened property block, returns the
  // iterator in that span pointing to the 4-byte aligned end of that block.
  ByteView EndOfPropertyBlock(ByteView bytes) const;

  // Extra step for dealing with rvalue references.
  template <typename T, typename U>
  void WalkInternal(T& pre_order_visitor, U& post_order_visitor) const {
    WalkTree(PreOrderNodeVisitor(pre_order_visitor), PostOrderNodeVisitor(post_order_visitor));
  }

  // Walks the tree with the provided walker arguments.
  void WalkTree(PreOrderNodeVisitor pre_order_visitor,
                PostOrderNodeVisitor post_order_visitor) const;

  ByteView WalkSubtree(ByteView subtree, NodePath* path, PropertyDecoder* parent,
                       PreOrderNodeVisitor& pre_order_visitor,
                       PostOrderNodeVisitor& post_order_visitor, bool visit) const;

  ByteView fdt_;
  // https://devicetree-specification.readthedocs.io/en/v0.3/flattened-format.html#structure-block
  ByteView struct_block_;
  // https://devicetree-specification.readthedocs.io/en/v0.3/flattened-format.html#strings-block
  std::string_view string_block_;
  // https://devicetree-specification.readthedocs.io/en/v0.3/flattened-format.html#memory-reservation-block
  ByteView mem_rsvmap_;

  // '/aliases' properties, populated during the walk, once found it wont change.
  mutable std::optional<Properties> aliases_;
};

}  // namespace devicetree

#endif  // ZIRCON_KERNEL_LIB_DEVICETREE_INCLUDE_LIB_DEVICETREE_DEVICETREE_H_
