// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/linux-boot-config/linux-boot-config.h"

#include <ctype.h>
#include <lib/fit/defer.h>
#include <stdio.h>

#include <algorithm>

namespace linux_boot_config {
namespace {

// Represents a section of the BOOTDATA at a given offset. The offset allows providing some
// context about where an error might have happened within the file being parsed.
//
// Also the offset does not align
struct Chunk {
  // Behaves like a string_view with a reference to were it was found in some other string.
  constexpr const std::string_view& operator*() const { return data; }
  constexpr const std::string_view* operator->() const { return &data; }

  constexpr void remove_prefix(size_t n) {
    data.remove_prefix(n);
    chunk_offset += n;
  }

  fit::error<ParseError> error(std::string_view description) {
    return fit::error(ParseError{.description = description, .offset = chunk_offset});
  }

  void remove_prefix_matching(std::string_view matching) {
    size_t index = data.find_first_not_of(matching);
    if (index == std::string_view::npos) {
      data = std::string_view();
      chunk_offset = data.size() + chunk_offset;
      return;
    }

    data = data.substr(index);
    chunk_offset = index + chunk_offset;
  }

  void remove_prefix_until(std::string_view matching) {
    size_t index = data.find_first_of(matching);
    if (index == std::string_view::npos) {
      index = data.size() - 1;
    }
    remove_prefix(index + 1);
  }

  // Collection of characters representing  a stream.
  std::string_view data;

  // Offset of `chunk` in the source string.
  size_t chunk_offset = 0;
};

// Removes the trailing characters from `tail`, that
constexpr std::string_view TrimTail(std::string_view tail) {
  size_t end_at = tail.find_last_not_of(' ');
  if (end_at == std::string_view::npos) {
    return std::string_view();
  }
  return tail.substr(0, end_at + 1);
}

bool IsKeyCharacter(const char c) { return isalnum(c) || c == '-' || c == '_' || c == '.'; }

bool IsUnquotedValueCharacter(const char c) {
  // This cannot be embedded inside the value.
  constexpr std::string_view kForbidden = " \n;,}#";

  if (!isprint(c)) {
    return false;
  }

  return kForbidden.find(c) == std::string_view::npos;
}

// Parses individual value entries, stopping at ',' , '\n' or ';'.
fit::result<ParseError, std::string_view> GetValue(Chunk& chunk) {
  // Find first non-empty character.
  chunk.remove_prefix_matching(" ");
  if (chunk->empty()) {
    return chunk.error("Operation with no value.");
  }

  if (chunk->front() == '"' || chunk->front() == '\'') {
    // Great the value is delimited by quotes.
    const char quote = chunk->front();
    chunk.remove_prefix(1);
    size_t end_quote = chunk->find(quote);
    if (end_quote == std::string_view::npos) {
      return chunk.error("Unclosed quotes");
    }

    std::string_view value_string = chunk->substr(0, end_quote);
    if (std::ranges::find_if_not(value_string, isprint) != value_string.end()) {
      return chunk.error("Invalid character in value");
    }
    chunk.remove_prefix(end_quote + 1);
    return fit::ok(value_string);
  }

  auto value_end = std::ranges::find_if_not(*chunk, IsUnquotedValueCharacter);
  std::string_view value_string = std::string_view(chunk->begin(), value_end);
  chunk.remove_prefix(value_string.size());
  return fit::ok(value_string);
}

fit::result<ParseError, std::string_view> GetKey(Chunk& chunk) {
  // Eat any spaces and comment lines.
  while (!chunk->empty()) {
    chunk.remove_prefix_matching(" ");
    if (chunk->empty()) {
      return chunk.error("Empty key entry");
    }

    // Line breaks are only supported at the end of comments, key or values.
    if (chunk->front() == '#') {
      chunk.remove_prefix_until("\n");
      continue;
    }

    if (chunk->front() == '}' || chunk->front() == ';') {
      return fit::ok("");
    }

    // First non-space or comment, that isn't the end of a scope.
    if (!IsKeyCharacter(chunk->front())) {
      return chunk.error("Unsupported character for key");
    }

    size_t key_end =
        std::distance(chunk->begin(), std::ranges::find_if_not(*chunk, &IsKeyCharacter));
    std::string_view key = chunk->substr(0, key_end);
    chunk.remove_prefix(key_end);
    key = TrimTail(key);
    return fit::ok(key);
  }

  return fit::ok("");
}

// Parses a value, and handles the possibility of array values. The assumption is,
// all values are "array-like", some of them are just one element arrays.
//
// Validates that comments do not precede a ','.
fit::result<ParseError> VisitValues(Key& key, auto& visitor, Chunk& chunk, Value::Action action) {
  bool comment_before = false;
  while (!chunk->empty()) {
    chunk.remove_prefix_matching(" ");
    if (chunk->empty()) {
      if (action != Value::Action::kUnknown) {
        return chunk.error("Operation without value");
      }
      return fit::ok();
    }
    switch (chunk->front()) {
      case '#':
        comment_before = true;
        chunk.remove_prefix_until("\n");
        // Comment following an array element.
        // ```
        // foo = 1, # Comment
        // ```
        if (action == Value::Action::kUnknown) {
          // Continue so we check that we are not preceding a ','.
          continue;
        }

        // Comment following  empty value.
        // ```
        // foo# Comment
        // ```
        if (action == Value::Action::kDefine) {
          visitor(key, Value{.action = action});
          return fit::ok();
        }
        continue;
      case ',':
        if (comment_before) {
          return fit::error(ParseError("Comment before ','", chunk.chunk_offset));
        }
        action = Value::Action::kAppend;
        chunk.remove_prefix(1);
        continue;

      // Value has been fully visited.
      case ';':
        // Consume it so `VisitBody` does not treat this as the key having
        // empty value.
        chunk.remove_prefix(1);
        return fit::ok();
      case '\n':
        if (action == Value::Action::kAppend) {
          chunk.remove_prefix(1);
          continue;
        }
        return fit::ok();
      case '}':
        return fit::ok();

      // Consume a value.
      default:
        break;
    };

    // A comment that was at the end of the statement.
    if (action == Value::Action::kUnknown) {
      return fit::ok();
    }

    auto value_res = GetValue(chunk);
    if (value_res.is_error()) {
      return value_res.take_error();
    }
    visitor(key, Value{.action = action, .value = *value_res});
    // Reset the value, so if no ',' is found and we run into a linebreak
    // we exit.
    action = Value::Action::kUnknown;
    chunk.remove_prefix_matching(" ");
    comment_before = false;
  }
  return fit::ok();
}

// Visits all leaf nodes with their respective values. If any key in this scope
// has children nodes, then those are recursively visited in pre-order fashion.
fit::result<ParseError> VisitBody(Key& key, auto& visitor, Chunk& chunk) {
  auto is_assign_or_override = [&chunk]() -> fit::result<ParseError> {
    if (chunk->size() < 2) {
      return chunk.error("Not enough characters for operator.");
    }
    if (chunk->at(1) != '=') {
      return chunk.error("Invalid operator.");
    }
    chunk.remove_prefix(2);
    return fit::ok();
  };

  auto visit_empty = [&key, &visitor](KeyPart& node) {
    if (node.name.empty()) {
      return;
    }
    visitor(key, Value{.action = Value::Action::kDefine});
  };

  while (!chunk->empty()) {
    auto key_res = GetKey(chunk);
    if (key_res.is_error()) {
      return key_res.take_error();
    }

    // Key may be empty.
    KeyPart key_part{.name = *key_res};

// This is safe, every recursive call pushes one element to key at most, and
// if it does, it pops it as soon as it returns.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wdangling-pointer"
    key.push_back(&key_part);
#pragma GCC diagnostic pop
    auto deferred_pop = fit::defer([&key]() { key.pop_back(); });

    chunk.remove_prefix_matching(" ");
    if (chunk->empty()) {
      // Do not visit empty keys.
      if (key_part.name.empty()) {
        break;
      }
      visitor(key, Value{.action = Value::Action::kDefine});
      return fit::ok();
    }

    Value::Action action = Value::Action::kDefine;
    switch (chunk->front()) {
      case '\n':
      case ';':
        visit_empty(key_part);
        chunk.remove_prefix(1);
        continue;
      case '}':
        visit_empty(key_part);
        return fit::ok();

      case '{':
        // Visit intermediate node.
        chunk.remove_prefix(1);
        chunk.remove_prefix_matching(" \n");
        // Intermediate node, we may want to skip.
        if (auto res = VisitBody(key, visitor, chunk); res.is_error()) {
          return res.take_error();
        }
        chunk.remove_prefix_matching(" \n");
        if (chunk->empty() || chunk->front() != '}') {
          return chunk.error("Unterminated scope");
        }
        chunk.remove_prefix(1);
        chunk.remove_prefix_matching(" \n");
        continue;

      case ':':
        action = Value::Action::kOverride;
        if (auto res = is_assign_or_override(); res.is_error()) {
          return res.take_error();
        }
        break;
      case '+':
        action = Value::Action::kAppend;
        if (auto res = is_assign_or_override(); res.is_error()) {
          return res.take_error();
        }
        break;
      case '=':
        action = Value::Action::kDefine;
        chunk.remove_prefix(1);
        break;
      default:
        break;
    };

    if (auto res = VisitValues(key, visitor, chunk, action); res.is_error()) {
      return res.take_error();
    }
    chunk.remove_prefix_matching(" \n");
  }
  return fit::ok();
}

}  // namespace

Key::CompareResult Key::Compare(std::string_view key) const {
  if (is_empty()) {
    return key.empty() ? CompareResult::kMatch : CompareResult::kNoMatch;
  }

  // 'key' is used as the remainder of the compared key, we will be sliding
  // forward and comparing the available sections, one at a time.
  auto it = begin();
  while (it != end() && key.size() > it->name.size()) {
    std::string_view sections = it->name;
    if (!key.starts_with(sections)) {
      return CompareResult::kNoMatch;
    }

    // The section is a prefix of the key, but this chunk is not matching
    // an entire section. E,g, 'foo.bar' with 'foo.b'
    if (key[sections.size()] != '.') {
      return CompareResult::kNoMatch;
    }

    it = std::next(it);
    key.remove_prefix(sections.size() + 1);
  }

  // We consume all the sections, but not the key.
  // E.g. foo.bar with `foo.bar.baz`
  if (it == end()) {
    return CompareResult::kParent;
  }

  // The opposite situation from above, `foo.bar.baz` with `foo.bar`.
  if (key.empty()) {
    return CompareResult::kChild;
  }

  std::string_view section = it->name;
  if (!section.starts_with(key)) {
    return CompareResult::kNoMatch;
  }

  if (section.size() == key.size()) {
    if (std::next(it) == end()) {
      return CompareResult::kMatch;
    }
    return CompareResult::kChild;
  }

  if (section[key.size()] == '.') {
    return CompareResult::kChild;
  }
  return CompareResult::kNoMatch;
}

fit::result<ParseError, LinuxBootConfig> LinuxBootConfig::Create(std::span<const std::byte> initrd,
                                                                 FILE* f) {
  if (initrd.size_bytes() < sizeof(Trailer)) {
    return fit::ok(LinuxBootConfig{});
  }

  Trailer trailer;
  trailer.Read(initrd.subspan(initrd.size() - sizeof(Trailer)));

  if (trailer.magic != Trailer::kMagic) {
    return fit::ok(LinuxBootConfig{});
  }

  // The spec requires a filesize alignment of 4, but we do not actually
  // require this and we have encountered production violations of this
  // alignment, so we pragmatically swallow any deviations.
  if (trailer.size % 4 != 0) {
    fprintf(f, "Warning: `bootconfig` file size is not properly aligned.\n");
  }

  if (trailer.size + sizeof(Trailer) > initrd.size_bytes()) {
    return fit::error(ParseError("`bootconfig` file is bigger than `initrd`."));
  }

  auto byte_content =
      initrd.subspan(initrd.size_bytes() - trailer.size - sizeof(trailer), trailer.size);
  uint32_t checksum = Checksum(byte_content);
  if (checksum != trailer.checksum) {
    return fit::error(ParseError("`bootconfig` file checksum failed."));
  }
  LinuxBootConfig boot_config;
  boot_config.contents_ = {reinterpret_cast<const char*>(byte_content.data()),
                           byte_content.size_bytes()};
  return fit::ok(boot_config);
}

fit::result<ParseError> LinuxBootConfig::VisitInternal(LinuxBootConfig::NodeVisitor visitor) const {
  Key key;

  // No boot config.
  if (contents_.empty()) {
    return fit::ok();
  }

  Chunk boot_config{.data = contents_, .chunk_offset = 0};

  // Trim off any padding ahead of time to avoid more complex checking in the parsing logic.
  //
  // We intentionally look for the first `\0` rather than e.g. right-trimming because some
  // bootloaders leave nonzero data later on in the padding. This technically violates the spec
  // which says padding should all be `\0`, but standard parsing behavior is to just stop at the
  // first terminator without checking the remaining bytes so we do the same.
  if (size_t first_null = boot_config.data.find('\0'); first_null != std::string_view::npos) {
    boot_config.data = boot_config.data.substr(0, first_null);
  }

  boot_config.remove_prefix_matching(" \n");
  // Empty boot config.
  if (boot_config->empty()) {
    return fit::ok();
  }

  // Non-empty boot-config, so we can enforce certain invariants. Also, because this is the root,
  // we do not need to check for a closing brace.
  if (auto visit_result = VisitBody(key, visitor, boot_config); visit_result.is_error()) {
    return visit_result.take_error();
  }

  return fit::ok();
}

}  // namespace linux_boot_config
