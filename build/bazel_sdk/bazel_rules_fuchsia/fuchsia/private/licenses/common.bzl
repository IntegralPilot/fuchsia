# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common functions."""

def check_type(value, expected_type):
    """Fails if the value's type doesn't match expected_type.

    Args:
        value: The value to check.
        expected_type: The expected type.
    """
    t = type(value)
    if t != expected_type:
        fail("Expected value of type %s but got %s" % (expected_type, t))

def is_target(value):
    """Whether value is of type Target.

    Args:
        value: The value to verify.
    Returns:
        Whether the value is of type Target
    """
    return type(value) == "Target"

def is_label(value):
    """Whether value is of type Label.

    Args:
        value: The value to verify.
    Returns:
        Whether the value is of type Label
    """
    return type(value) == "Label"

def check_is_target(value):
    """Causes a failure if the type of value is not Target.

    Args:
        value: value to check.
    """
    check_type(value, "Target")

def is_same_package(target_a, target_b):
    """Whether target_a and target_b are of the same bazel package.

    Args:
        target_a: 1st target to compare
        target_b: 2nd target to compare
    Returns:
        whether the 2 targets are within the same Bazel package.
    """
    check_is_target(target_a)
    check_is_target(target_b)
    label_a = target_a.label
    label_b = target_b.label
    return label_a.package == label_b.package and label_a.workspace_name == label_b.workspace_name

def to_label_str(obj):
    """Serializes a Target or Label obj into a string based on its label.

    Args:
        obj: The instance to serialize.
    Returns:
        The serialized label of the object.
    """
    if is_target(obj):
        return "%s" % obj.label
    elif is_label(obj):
        return "%s" % obj
    else:
        fail("Unexpected type %s" % type(obj))

def to_package_str(obj):
    """Serializes a Target or Label obj into a namespace + package path

    Args:
        obj: The instance to serialize.
    Returns:
        The serialized label of the object without the in-package suffix.
    """
    if is_target(obj):
        label = obj
    elif is_label(obj):
        label = obj
    else:
        fail("Unexpected type %s" % type(obj))
    return "@%s//%s" % (label.workspace_name, label.package)

def bool_dict(values):
    """Converts a list of values [v1, v2, ...] into a Dictionary of { v1: True, v2: True, ...} for optimized set operations.

    Args:
        values: List of values v1, v2, ....
    Returns:
        A Dictionary of { v1: True, v2: True, ...}
    """
    check_type(values, type([]))
    out = {}
    for v in values:
        out[v] = True
    return out

def to_file_path(label_or_file):
    """Converts a Label or File into a file path adequate to serve as a tools' input.

    Args:
        label_or_file: The input to convert.
    Returns:
        The file path to the label or file input.
    """
    if is_label(label_or_file):
        return label_or_file.package + "/" + label_or_file.name
    elif type(label_or_file) == "File":
        return label_or_file.path
    else:
        fail("Unexpected type %s", type(label_or_file))
