// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42165807): Fix null safety and remove this language version.
// @dart=2.9

import 'dart:async';
import 'dart:convert';

import 'package:quiver/core.dart' show Optional;

/// Make sure there's only one instance of the item in the input.
///
/// Example expected input:
/// Rule {
///     host_match: "fuchsia.com",
///     host_replacement: "devhost",
///     path_prefix_match: "/",
///     path_prefix_replacement: "/",
/// }
///
/// We want to make sure there is only one instance of a given key:value.
bool hasExclusivelyOneItem(
    String input, String expectedKey, String expectedValue) {
  return 1 ==
      RegExp('($expectedKey):\\s+\\"($expectedValue)\\"')
          .allMatches(input)
          .length;
}

/// Find the redirect rule for `fuchsia.com`, if there is one.
///
/// Example expected input:
/// Rule {
///     host_match: "fuchsia.com",
///     host_replacement: "devhost",
///     path_prefix_match: "/",
///     path_prefix_replacement: "/",
/// }
///
/// Returns `Optional.of("devhost")` for the above example input.
/// If there are no rules for `"fuchsia.com"`, will return `Optional.absent()`.
Optional<String> getCurrentRewriteRule(String ruleList) {
  String fuchsiaRule = '';
  var matches = RegExp('Rule (\{.+?\})', dotAll: true).allMatches(ruleList);
  for (final match in matches) {
    if (hasExclusivelyOneItem(match.group(0), 'host_match', 'fuchsia.com')) {
      fuchsiaRule = match.group(0);
      break;
    }
  }
  matches = RegExp('host_replacement:\\s+\\"(.+)\\"').allMatches(fuchsiaRule);
  for (final match in matches) {
    // There should only be one match. If not, just take the first one.
    return Optional.of(match.group(1));
  }
  return Optional.absent();
}
