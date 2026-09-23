// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_LINUX_BOOT_CONFIG_INCLUDE_LIB_LINUX_BOOT_CONFIG_LINUX_BOOT_CONFIG_H_
#define ZIRCON_KERNEL_PHYS_LIB_LINUX_BOOT_CONFIG_INCLUDE_LIB_LINUX_BOOT_CONFIG_LINUX_BOOT_CONFIG_H_

#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <lib/stdcompat/bit.h>
#include <stdio.h>
#include <string.h>

#include <array>
#include <bit>
#include <cstddef>
#include <cstdint>
#include <numeric>
#include <optional>
#include <span>
#include <string_view>

#include <fbl/intrusive_container_utils.h>
#include <fbl/intrusive_double_list.h>

namespace linux_boot_config {

struct ParseError {
  // Human readable error description.
  std::string_view description;

  // If present, indicates that parsing was initiated at this offset,
  // when the error was encountered, providing some hint.
  std::optional<uint64_t> offset;
};

struct Trailer {
  static constexpr auto kMagic = []() constexpr {
    constexpr std::string_view kMagic = "#BOOTCONFIG\n";
    std::array<char, kMagic.size()> arr;
    kMagic.copy(arr.data(), kMagic.size());
    return arr;
  }();

  // Read from a byte-view of BOOTCONFIG.
  void Read(std::span<const std::byte> bytes) {
    ZX_ASSERT(bytes.size() >= sizeof(Trailer));
    Trailer trailer = {};
    memcpy(this, bytes.data(), sizeof(trailer));
    if constexpr (std::endian::native == std::endian::big) {
      this->size = cpp23::byteswap(trailer.size);
      this->checksum = cpp23::byteswap(trailer.checksum);
    }
  }

  // Write as bytes, applying proper endianess to size and checksum fields.
  void Write(std::span<std::byte> bytes) {
    ZX_ASSERT(bytes.size() >= sizeof(Trailer));
    Trailer trailer = *this;
    if constexpr (std::endian::native == std::endian::big) {
      trailer.size = cpp23::byteswap(trailer.size);
      trailer.checksum = cpp23::byteswap(trailer.checksum);
    }
    memcpy(bytes.data(), &trailer, sizeof(trailer));
  }

  // Size of the file with added padding('\0') byte that precedes the trailer.
  // This field is stored in LittleEndian format.
  uint32_t size;

  // Checksum of the embedded file, which can be used to verify integrity.
  // This field is stored in LittleEndian format.
  uint32_t checksum;

  // Magic byte sequence, spelling "#BOOTCONFIG\n" (`Trailer::kMagic`).
  // Checked for presence of bootconfig file.
  std::decay_t<decltype(kMagic)> magic;
};

// Within a `NodePath` represents any key or key part defined.
// In the example below:
//
// ```
// foo.bar {
//     baz.fooz {
//        ....
//     }
// }
// ```
// When visiting `foo.bar` the path, only contains one node with `[foo.bar]`.
// When visiting `foo.bar.baz.fooz` the path contains two nodes  `[foo.bar, baz.fooz]`.
struct KeyPart : fbl::DoublyLinkedListable<KeyPart*, fbl::NodeOptions::AllowMove> {
  std::string_view name;
};

// BootConfig provides a hierarchical declaration syntax for keys:
// ```
// foo.bar {
//     baz.fooz {
//        ....
//     }
// }
// ```
//
// Each Node in the path, is a step taken into a nested scope. In order to resolve
// a key, the entire path must be taken into account. Elements in the path are joined by '.'.
//
// When a node is visited, it is provided the current path, through this means visitors/observer
// patterns may skip entire subtrees.
class Key : public fbl::DoublyLinkedList<KeyPart*> {
 public:
  // Result information obtained when comparing abn arbitrary key in string form against a Key.
  // Caller then may take actions based on this.
  //
  // A Key can be thought of a collection of string joined by '.'. We will call each of the elements
  // in this collection as a section of a key. The unit of comparison is per section.For example,
  // guven two keys  a("foo.bar") and b("foo.barz"), even though a is a prefix of b, since the
  // sections 'bar' and 'barz' do not match, the result is considered `kNoMatch`.
  enum class CompareResult : uint8_t {
    // The 'key' is a child of this path
    //
    // ```
    // // Assume Key = ['foo.bar', 'baz']
    // assert(path.Compare("foo.bar.baz.fooz") == CompareResult::kDescendant)
    // ```
    kParent,

    // The 'key' is a parent of this path.
    //
    // ```
    // // Assume Key = ['foo.bar', 'baz']
    // assert(path.Compare("foo.bar") == CompareResult::kAncestor)
    // ```
    kChild,

    // The 'key' is a child of this path.
    //
    // ```
    // // Assume Key = ['foo.bar', 'baz']
    // assert(path.Compare("foo.bar.baz") == CompareResult::kMatch)
    // ```
    kMatch,

    // The 'key' is not related to this path.
    //
    // ```
    // // Assume Key = ['foo.bar', 'baz']
    // assert(path.Compare("foo.baz") == CompareResult::kNoMatch)
    // ```
    kNoMatch,
  };

  // Returns the relationship between the `ley` and the path to the current.
  CompareResult Compare(std::string_view key) const;

  // The key parts in `key` form a key prefix of this.
  // E.g.:
  // ```
  // Key key = ["foo.bar"]
  //
  // a.prefix_of("foo.bar.baz") is true.
  // ```
  bool start_of(std::string_view key) const { return Compare(key) == CompareResult::kParent; }

  // The key parts in this form a key prefix of `key`.
  // E.g.:
  // ```
  // Key key = ["foo.bar"]
  //
  // a.starts_with("foo") is true.
  // ```
  bool starts_with(std::string_view key) const { return Compare(key) == CompareResult::kChild; }

  bool operator==(std::string_view key) const { return Compare(key) == CompareResult::kMatch; }
};

// The set of bytes applied to a key with the type of operation performed.
struct Value {
  enum class Action : uint8_t {
    kUnknown,
    // A new key-valye pair is defined.
    kDefine,
    // Overwrite the current value of a key-value pair with the provided one.
    kOverride,
    // From the current key-value pair, assume its an array and append this value to it.
    kAppend,
  };

  // User can look at the value type to determine what to do with the provided `value`.
  Action action;

  // Actual bytes to be used, any surrounded quotes are removed, and so are comments.
  std::string_view value;
};

// A callback for visiting each statement in declaration order. A key may be visited multiple times,
// when it is operated on multiple times. For example:
//
// ```
// foo = 123
// foo += 234
// foo := 5
// ```
// key `foo` is visited 3 times.
template <typename T>
concept Visitor = requires(T visitor, const Key& key, const Value& value) {
  // Called when visiting a leaf node, each key operation consists of a node.
  //
  // ```
  // foo = 123
  // foo := 1234
  // foo += 1234
  // ```
  // In this case, leaf node 'foo' will be visited three times.
  { visitor(key, value) };
};

// BootConfig's checksum is calculated a simple sum over all bytes, padding bytes included.
// This method in conjunction with the trailer allow for easy appending, since we can continue
// adding from the previous checksum, and the `\0` padding bytes can be safely overwritten,
// since they dont contribute to the checksum.
constexpr uint32_t Checksum(std::span<const std::byte> bytes) {
  auto as_nums = std::span(std::bit_cast<const uint8_t*>(bytes.data()), bytes.size_bytes());
  uint32_t accumulated = std::accumulate(as_nums.begin(), as_nums.end(), 0);
  return accumulated;
}

// Represents a `BOOTCONFIG` object with functionality for extracting and parsisng the contents.
//
// See https://docs.kernel.org/admin-guide/bootconfig.html for more details.
class LinuxBootConfig {
 public:
  // A bootconfig file is embedded in `initrd`, and can be modified by the bootloader.
  // The ramdisk consis on the first N bytes of he initrd, and the
  static fit::result<ParseError, LinuxBootConfig> Create(std::span<const std::byte> initrd,
                                                         FILE* f = stdout);

  constexpr LinuxBootConfig() = default;
  explicit constexpr LinuxBootConfig(std::string_view contents) : contents_(contents) {}
  constexpr LinuxBootConfig(const LinuxBootConfig&) = default;
  constexpr LinuxBootConfig(LinuxBootConfig&&) noexcept = default;

  constexpr LinuxBootConfig& operator=(const LinuxBootConfig&) = default;
  constexpr LinuxBootConfig& operator=(LinuxBootConfig&&) = default;

  // Number of bytes of the embedded file with the padding bytes.
  constexpr size_t size_bytes() const { return contents_.size(); }
  constexpr std::string_view contents() const { return contents_; }

  template <Visitor V>
  fit::result<ParseError> Parse(V&& v) const {
    return VisitInternal(std::forward<V>(v));
  }

 private:
  using NodeVisitor = fit::inline_function<void(const Key&, const Value&), 32>;

  fit::result<ParseError> VisitInternal(NodeVisitor visitor) const;

  std::string_view contents_;
};

}  // namespace linux_boot_config

#endif  // ZIRCON_KERNEL_PHYS_LIB_LINUX_BOOT_CONFIG_INCLUDE_LIB_LINUX_BOOT_CONFIG_LINUX_BOOT_CONFIG_H_
