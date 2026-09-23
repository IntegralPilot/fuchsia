# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utilities for walking json structs and extracting data from it"""

load("@bazel_skylib//lib:paths.bzl", "paths")

def walk_json(json_dict, visit_node_funcs):
    """Walks a json dictionary, applying the functions in `visit_node_funcs` on every node.

    Args:
        json_dict: The dictionary to walk.
        visit_node_funcs: A function that takes 3 arguments: dictionary, key, value.
    """
    nodes_to_visit = []

    def _enqueue(dictionary, k, v):
        nodes_to_visit.append(struct(
            dictionary = dictionary,
            key = k,
            value = v,
        ))

    def _enqueue_dictionary_children(dictionary):
        for key, value in dictionary.items():
            _enqueue(dictionary, key, value)

    def _enqueue_array(array):
        for i, value in enumerate(array):
            _enqueue(array, i, value)

    _enqueue_dictionary_children(json_dict)

    # Bazel doesn't support recursion, but a json object will always have less
    # nodes than number of serialized characters, so this iteration suffices.
    max_nodes = len(str(json_dict))
    for _unused in range(0, max_nodes):
        if not nodes_to_visit:
            break
        node = nodes_to_visit.pop()
        for visit_node_func in visit_node_funcs:
            visit_node_func(dictionary = node.dictionary, key = node.key, value = node.value)
        if type(node.value) == "dict":
            _enqueue_dictionary_children(node.value)
        if type(node.value) == "list":
            _enqueue_array(node.value)

def replace_labels_with_files(json_dict, target_to_string_map, relative = None):
    """Replace all labels in json_dict with file paths.

    Uses target_to_string_map to find the labels to replace.
    Note that this function cannot be merged with extract_labels(), because we
    need to pass the labels into a Bazel rule with label_keyed_string_dict so
    that Bazel can convert the labels to targets. That is the only way we can
    get the file paths from a label.

    Args:
      json_dict: starlark dictionary with label strings
      target_to_string_map: starlark dictionary mapping the Target to the
        LABEL(label) strings
      relative: if provided, remove the given directory prefix in order to refer
        to files under the given directory by relative paths
    """

    # Invert the map so that it is LABEL(label) to label.
    string_to_target_map = {}
    for label, string in target_to_string_map.items():
        string_to_target_map[string] = label

    # Replace each LABEL(label) with the file path.
    def _replace_labels_visitor(dictionary, key, value):
        # buildifier: disable=uninitialized
        if type(value) == "string" and value in string_to_target_map:
            label = string_to_target_map.get(value)
            label_files = label.files.to_list()
            if relative:
                dictionary[key] = paths.relativize(label_files[0].path, relative)
            else:
                dictionary[key] = label_files[0].path

    walk_json(json_dict, [_replace_labels_visitor])

def extract_labels(json_dict):
    """Walk json_dict and return a map of all the labels found.

    Args:
      json_dict: starlark dictionary to extract labels from

    Returns:
        A map of the label to LABEL(label) strings.
    """
    extracted_raw_config_labels = {}

    # buildifier: disable=unused-variable
    def _extract_labels_visitor(dictionary, key, value):
        if type(value) == "string" and value.startswith("LABEL("):
            if not value.endswith(")"):
                fail("Syntax error: LABEL does not have closing bracket")
            label = value[6:-1]
            extracted_raw_config_labels[label] = value

    def _remove_none_values_visitor(dictionary, key, value):
        """Remove keys with a value of None.

        Some optional keys will necessarily be supplied as 'None' value instead
        of being omitted entirely, because Bazel doesn't allow the use of top-
        level 'if' statements, instead, they can only be used within the value
        of an expression:

          "foo": value if foo else None

        However, we want to strip those 'None' values before generating the json
        from the nested dicts.
        """
        if value == None:
            dictionary.pop(key)

    walk_json(json_dict, [_remove_none_values_visitor, _extract_labels_visitor])
    return extracted_raw_config_labels
