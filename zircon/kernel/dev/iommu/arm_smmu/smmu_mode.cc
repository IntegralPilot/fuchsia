// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <ctype.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/assert.h>

#include <dev/arm_smmu/smmu_mode.h>

namespace arm_smmu {

namespace {

const char* SkipWhitespace(const char* ptr) {
  while (*ptr && isspace(*ptr)) {
    ++ptr;
  }
  return ptr;
}

size_t TrimmedLength(const char* start, const char* end) {
  while (end > start && isspace(*(end - 1))) {
    --end;
  }
  return end - start;
}

bool StringEquals(const char* str, size_t len, const char* match) {
  return len == strlen(match) && strncasecmp(str, match, len) == 0;
}

ktl::optional<ArmSmmuMode> ParseMode(const char* str, size_t len) {
  if (StringEquals(str, len, "disabled")) {
    return ArmSmmuMode::kDisabled;
  }
  if (StringEquals(str, len, "passthru")) {
    return ArmSmmuMode::kPassthru;
  }
  if (StringEquals(str, len, "enforced")) {
    return ArmSmmuMode::kEnforced;
  }
  return ktl::nullopt;
}

enum class ParseBehavior {
  kValidate,        // Return nullopt if any part of the string is invalid.
  kTolerateErrors,  // Attempt to parse segments and return a result even if some are malformed.
};

ktl::optional<ArmSmmuMode> ParseSmmuModeCommon(const char* mode_string,
                                               ktl::optional<uint64_t> base_addr,
                                               ParseBehavior behavior) {
  // If no string is passed, or the string is empty, fail.
  if (!mode_string) {
    return ktl::nullopt;
  }

  const char* ptr = SkipWhitespace(mode_string);
  if (*ptr == '\0') {
    return ktl::nullopt;
  }

  // Find the default mode token and parse it.  The default mode token is the
  // first token in the string, and is terminated by either a comma or the end
  // of the string.
  const char* const comma = strchr(ptr, ',');
  const size_t default_len =
      comma ? TrimmedLength(ptr, comma) : TrimmedLength(ptr, ptr + strlen(ptr));
  const ktl::optional<ArmSmmuMode> default_mode_opt = ParseMode(ptr, default_len);
  if (!default_mode_opt.has_value()) {
    return ktl::nullopt;
  }

  const ArmSmmuMode default_mode = default_mode_opt.value();
  // If we are tolerating errors and don't have a base address to match,
  // we can just return the default mode immediately without parsing segments.
  if (behavior == ParseBehavior::kTolerateErrors && !base_addr.has_value()) {
    return default_mode;
  }

  // Parse all of the base address / mode segments.
  ptr = comma;
  while (ptr && *ptr == ',') {
    ++ptr;
    ptr = SkipWhitespace(ptr);

    // Find the next comma (if any) and the equals sign in this segment.  If the
    // equals sign is not found, or if the next comma occurs before the equals
    // sign, then this is not a valid segment.
    const char* const next_comma = strchr(ptr, ',');
    const char* const equals = strchr(ptr, '=');
    if (!equals || (next_comma && (equals > next_comma))) {
      if (behavior == ParseBehavior::kValidate) {
        return ktl::nullopt;
      }
      ptr = next_comma;
      continue;
    }

    // Parse the base address token.
    char* endptr;
    const uint64_t parsed_addr = strtoul(ptr, &endptr, 0);
    if (endptr == ptr) {
      if (behavior == ParseBehavior::kValidate) {
        return ktl::nullopt;
      }
      ptr = next_comma;
      continue;
    }

    // Endptr must point exactly to the '=' (after skipping whitespace) to ensure
    // clean separation of the address token and the mode token.
    const char* const after_addr = SkipWhitespace(endptr);
    if (after_addr != equals) {
      if (behavior == ParseBehavior::kValidate) {
        return ktl::nullopt;
      }
      ptr = next_comma;
      continue;
    }

    // Now attempt to parse the mode token.
    const char* const mode_ptr = SkipWhitespace(equals + 1);
    const size_t mode_len = next_comma ? TrimmedLength(mode_ptr, next_comma)
                                       : TrimmedLength(mode_ptr, mode_ptr + strlen(mode_ptr));
    const ktl::optional<ArmSmmuMode> mode_opt = ParseMode(mode_ptr, mode_len);

    if (behavior == ParseBehavior::kValidate) {
      if (!mode_opt.has_value()) {
        return ktl::nullopt;
      }
    } else {
      ZX_DEBUG_ASSERT(behavior == ParseBehavior::kTolerateErrors);
      ZX_DEBUG_ASSERT(base_addr.has_value());
      if (parsed_addr == base_addr.value()) {
        return mode_opt;
      }
    }

    // We didn't match this segment (or we are just validating), so move on to the next one.
    ptr = next_comma;
  }

  return default_mode;
}

}  // namespace

ktl::optional<ArmSmmuMode> GetSmmuMode(const char* mode_string, ktl::optional<uint64_t> base_addr) {
  return ParseSmmuModeCommon(mode_string, base_addr, ParseBehavior::kTolerateErrors);
}

bool ValidateSmmuModeString(const char* mode_string) {
  return ParseSmmuModeCommon(mode_string, ktl::nullopt, ParseBehavior::kValidate).has_value();
}

}  // namespace arm_smmu
