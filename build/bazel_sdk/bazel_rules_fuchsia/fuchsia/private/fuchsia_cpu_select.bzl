# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""fuchsia_cpu_select() function definition."""

def _add_dictionary_items(target_dict, clauses_dict):
    for condition, value in clauses_dict.items():
        if condition in target_dict:
            fail("Cannot add condition %s twice when calling fuchsia_cpu_select()" % condition)
        target_dict[condition] = value

def fuchsia_cpu_filter_dict(cpu_map, valid_cpus, common = None):
    """Return a dictionary that contains CPU-specific keys and values.

    This function returns a dictionary that only contains keys that match
    a specific set of cpu names.

    Examples:

          _cpu_map = {
            "arm64": { "is_arm64": True },
            "x64": { "is_x64": True },
          }

          fuchsia_cpu_filter_dict(_cpu_map, ["x64"])
            => { "is_x64": True }

          fuchsia_cpu_filter_dict(_cpu_map, ["arm64"])
            => { "is_arm64": True }

          fuchsia_cpu_filter_dict(_cpu_map, ["x64", "arm64"])
            => { "is_x64": True, "is_arm64": True }

          fuchsia_cpu_filter_dict(_cpu_map, ["riscv64"])
            => { }

    Args:
      cpu_map: A dictionary mapping Fuchsia cpu names to { key -> value }
          dictionaries. If the cpu name is listed in `valid_cpus`, then all
          entries this dictionary will be added to the result, otherwise
          they will be ignored.

      valid_cpus: A list of Fuchsia cpu names used to filter cpu_map.

      common: If not None, a dictionary whose items will be added
          unconditionally to the final result.

    Returns:
      A select() value whose dictionary has been built by filtering the content
      of cpu_map with only content for cpu names in valid_cpus.
    """
    final_dict = {}
    for cpu in valid_cpus:
        entries = cpu_map.get(cpu, {})
        _add_dictionary_items(final_dict, entries)

    if common != None:
        _add_dictionary_items(final_dict, common)

    return final_dict

def fuchsia_cpu_select(cpu_map, valid_cpus, common = None, default = None):
    """Return a select() statement that contains CPU-specific clauses.

    This function returns a select() statements that only contains clauses that
    depend on specific Fuchsia CPU names.

    Args:
      cpu_map: A dictionary mapping Fuchsia cpu names to select()-compatible
          { condition -> value } dictionaries. If the cpu name is listed in
          `valid_cpus`, then all clauses from this dictionary will be added
          to the result select() statement, otherwise they will be ignored.

      valid_cpus: A list of Fuchsia cpu names used to filter cpu_map.

      common: If not None, a { condition -> value } whose clauses will be added
          unconditionally to the final result select() statement.

      default: If not None, a value for the //conditions:default condition to
          be added to the result select() statement.

    Returns:
      A select() value whose dictionary has been built by filtering the content
      of cpu_map with only content for cpu names in valid_cpus.
    """
    final_dict = fuchsia_cpu_filter_dict(cpu_map, valid_cpus, common)

    if default != None:
        _add_dictionary_items(final_dict, {"//conditions:default": default})

    return select(final_dict)
