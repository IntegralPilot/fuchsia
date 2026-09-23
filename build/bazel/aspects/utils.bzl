# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Helper functions for Fuchsia specific aspects."""

def get_target_deps_from_attributes(rule_attr):
    """Return all dependencies from a given target context during analysis.

    Args:
        rule_attr: The ctx.attr value for the current target.
    Returns:
        A list of Target values corresponding to the dependencies of the current
        target.
    """
    result = {}
    for attr_name in dir(rule_attr):
        attr_value = getattr(rule_attr, attr_name, None)
        if not attr_value:
            continue
        if type(attr_value) == "Target":
            result[attr_value] = True
            continue
        if type(attr_value) == "list" and type(attr_value[0]) == "Target":
            for target in attr_value:
                result[target] = True
            continue
    return result.keys()
