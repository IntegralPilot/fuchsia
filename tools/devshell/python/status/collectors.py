# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import pathlib
import re
import typing
from collections import OrderedDict

import build_dir
import data
import fx_cmd
from async_utils.command import AsyncCommand


def get_fuchsia_dir() -> pathlib.Path:
    """Get the path to the user's Fuchsia checkout.

    Raises:
        RuntimeError: The path is not found.

    Returns:
        pathlib.Path: Path to the Fuchsia checkout.
    """
    path_str = os.environ.get("FUCHSIA_DIR")
    if not path_str:
        raise RuntimeError("Missing FUCHSIA_DIR environment variable.")
    return pathlib.Path(path_str)


async def git_collector() -> list[data.Result]:
    fuchsia_dir = get_fuchsia_dir()
    command = await AsyncCommand.create(
        "git",
        "--no-optional-locks",
        f"--git-dir={str(fuchsia_dir)}/.git",
        "rev-parse",
        "HEAD",
        "JIRI_HEAD",
    )
    output = await command.run_to_completion()

    if output.return_code != 0:
        return [data.Result(error="git command failed")]

    lines = iter(output.stdout.splitlines())

    is_at_head = True
    try:
        l1 = next(lines)
        l2 = next(lines)
        is_at_head = l1 == l2
    except StopIteration:
        pass

    return [
        data.Result(
            item=data.Item(
                category=data.Category.SOURCE,
                key="is_in_jiri_head",
                title="Is fuchsia source project in JIRI_HEAD?",
                value=is_at_head,
            )
        )
    ]


async def jiri_collector() -> list[data.Result]:
    """Collect data from JIRI for display.

    Returns:
        list[data.Result]: Results collected from JIRI.
    """
    command = await fx_cmd.FxCmd().start("jiri", "override", "-list")
    output = await command.run_to_completion()

    has_overrides = False
    if output.return_code == 0:
        has_overrides = len(output.stdout.splitlines()) > 0

    return [
        data.Result(
            item=data.Item(
                category=data.Category.SOURCE,
                key="has_jiri_overrides",
                title="Has Jiri overrides?",
                value=has_overrides,
                notes="output of 'jiri override -list'",
            )
        )
    ]


async def environment_collector() -> list[data.Result]:
    """Collect data from the incoming environment for display.

    Returns:
        list[data.Result]: Results from the environment.
    """
    results: list[data.Result] = []

    current_build_dir: pathlib.Path | None = None
    try:
        current_build_dir = build_dir.get_build_directory()
        results.append(
            data.Result(
                item=data.Item(
                    category=data.Category.ENVIRONMENT,
                    key="build_dir",
                    title="Current build directory",
                    value=str(current_build_dir),
                )
            )
        )
    except build_dir.GetBuildDirectoryError as e:
        results.append(data.Result(error=f"Failed to get build directory: {e}"))

    device_name: str | None = None
    device_notes: str | None = None

    # FUCHSIA_NODENAME is unconditionally set for all fx invocations.
    if (from_env := os.environ.get("FUCHSIA_NODENAME")) is not None:
        device_name = from_env

        # Use a separate environment variable to determine the source of FUCHSIA_NODENAME
        if os.environ.get("FUCHSIA_NODENAME_IS_FROM_FILE") == "true":
            device_notes = "set by `fx set-device`"
        else:
            device_notes = "set by fx -t"

    if device_name:
        results.append(
            data.Result(
                item=data.Item(
                    category=data.Category.ENVIRONMENT,
                    key="device_name",
                    title="Device name",
                    value=device_name,
                    notes=device_notes,
                )
            )
        )

    return results


async def args_gn_collector() -> list[data.Result]:
    """Collect data from args.gn for display.

    Returns:
        list[data.Result]: Results from args.gn.
    """
    results: list[data.Result] = []

    # This dict provides titles and optionally notes for variables
    # retrieved from args.gn. If a variable is not in this list,
    # it is not included in the command output.
    ARGS_TITLE_MAP: OrderedDict[
        str, typing.Tuple[str, str | None]
    ] = OrderedDict(
        boards=("Board", None),
        products=("Product", None),
        main_pb_label=("Product bundle", "set with `fx set-main-pb`"),
        universe_package_labels=(
            "Universe packages",
            "--with argument of `fx set`",
        ),
        host_labels=("Host labels", "--with-host argument of `fx set`"),
        developer_test_labels=(
            "Developer tests",
            "--with-test argument of `fx set`",
        ),
    )

    # Locate the args.gn file and run it through `gn format`. This
    # provides a JSON abstract syntax tree (AST) of the parsed file
    # which we can then process to find the current values of the
    # variables assigned in the user's settings.
    build_dir_path = build_dir.get_build_directory()
    args_gn_path = build_dir_path / "args.gn"
    if not args_gn_path.exists():
        return []
    command = await fx_cmd.FxCmd().start(
        "gn", "format", "--dump-tree=json", str(args_gn_path)
    )
    result = await command.run_to_completion()
    if result.return_code != 0:
        return [data.Result(error="Failed to process args.gn")]

    try:
        json_result: dict[str, typing.Any] = json.loads(result.stdout)
    except json.JSONDecodeError:
        return [data.Result(error="Failed to parse args.gn")]

    def _extract_type(
        value: typing.Any, target_type: str
    ) -> str | list[str] | None:
        """Helper to extract values from the gn AST JSON.

        Args:
            value (typing.Any): Incoming value to process.
            target_type (str): Type we are looking for. "LITERAL" or "IDENTIFIER".

        Returns:
            str | list[str] | None: Extracted value from the tree, or None if not
            found.
        """
        if isinstance(value, list):
            # Find the first matching element of incoming lists.
            for v in value:
                if (ret := _extract_type(v, target_type)) is not None:
                    return ret
        elif isinstance(value, dict) and value.get("type") in [
            target_type,
            "LIST",
        ]:
            # This is the value we are looking for, either as a single value or a list.

            STRIP_CHARS = "\"' \n"  # Get rid of wrapping quotes and whitespace.
            if isinstance((ret := value.get("value")), str):
                # This is a single value, return it.
                return ret.strip(STRIP_CHARS)
            elif (children := value.get("child")) is not None:
                # This is a list of values. Accumulate values
                # filtered by type and return it.
                lst: list[str] = []
                c: dict[str, typing.Any]
                for c in children:
                    if c.get("type") == target_type and isinstance(
                        (val := c.get("value")), str
                    ):
                        lst.append(val.strip(STRIP_CHARS))
                return lst

        return None

    # Will contain the final list of imported files.
    imports: list[str] = []

    # Will contain the final values of assigned variables in the file.
    assigned_variables: dict[str, str | list[str]] = {}

    # Process each item contained in the args.gn AST.
    item: dict[str, typing.Any]
    for item in json_result.get("child") or []:
        if item.get("type") == "FUNCTION" and item.get("value") == "import":
            # This is an import of the form `import("//some/file.gni")`
            # Extract the file name being imported.
            value = _extract_type(item.get("child"), "LITERAL")
            if isinstance(value, str):
                imports.append(value)
            if isinstance(value, list):
                imports.extend(value)
        elif item.get("type") == "BINARY":
            # This is an assignment of one of the following types:
            # "=": Direct assignment
            # "+=": List append
            # "-=": List subtraction

            # Get the variable name (IDENTIFIER) and the value to process (LITERAL).
            identifier = _extract_type(item.get("child"), "IDENTIFIER")
            value = _extract_type(item.get("child"), "LITERAL")
            if isinstance(identifier, str) and value is not None:
                if item.get("value") == "=":
                    # Handle direct assignment by overriding the value.
                    assigned_variables[identifier] = value
                elif item.get("value") == "+=":
                    # Handle list append by extending new values
                    # on the existing list, if one exists. Otherwise,
                    # create a new list.
                    if identifier not in assigned_variables:
                        assigned_variables[identifier] = []
                    target_list = assigned_variables[identifier]
                    assert isinstance(target_list, list)
                    assert isinstance(value, list)
                    target_list.extend(value)
                elif item.get("value") == "-=":
                    # Handle list subtraction by creating a new
                    # list containing only those elements of the
                    # existing value that are not present in the list
                    # passed to the subtraction operation.
                    if identifier not in assigned_variables:
                        assigned_variables[identifier] = []
                    source_list = assigned_variables[identifier]
                    assert isinstance(source_list, list)
                    assert isinstance(value, list)
                    subtraction_set = set(value)
                    assigned_variables[identifier] = [
                        val for val in source_list if val not in subtraction_set
                    ]

    # If a product bundle is set, we want to show its name instead of its label.
    product_bundles_path = build_dir_path / "product_bundles.json"
    if product_bundles_path.exists():
        try:
            with open(product_bundles_path, "r") as f:
                product_bundles = json.load(f)
        except (json.JSONDecodeError, IOError):
            product_bundles = []

        main_pb_label_val = assigned_variables.get("main_pb_label")
        if main_pb_label_val and isinstance(main_pb_label_val, str):
            for pb in product_bundles:
                # The label in product_bundles.json might have a toolchain.
                label = pb.get("label", "")
                if label.startswith(main_pb_label_val):
                    assigned_variables["main_pb_label"] = pb.get("name")
                    break

    # Parse the import names into pairs of (type, value).
    # For instance, `import("//boards/x64.gni")` would be parsed into
    # type="boards", value="x64", and
    # `import("//foo/bar/boards/x64-fb.gni")` would be parsed into
    # type="boards", value="x64-fb"
    # Types are filtered against ARGS_TITLE_MAP for inclusion.
    # Types are "boards" and "products"
    import_regex = re.compile(r".*(boards|products)\/([a-zA-Z0-9-_]+).gni")
    for import_name in imports:
        if (match := import_regex.match(import_name)) is not None:
            key = match.group(1)
            value = match.group(2)
            title, _ = ARGS_TITLE_MAP.get(key) or (key, None)
            results.append(
                data.Result(
                    item=data.Item(
                        category=data.Category.BUILD,
                        key=key,
                        value=value,
                        title=title,
                        notes=import_name,
                    )
                )
            )

    # Process all of the assigned variables, filtered against
    # ARGS_TITLE_MAP for inclusion.
    for key, value in assigned_variables.items():
        title, notes = ARGS_TITLE_MAP.get(key) or (None, None)
        if title is not None:
            results.append(
                data.Result(
                    item=data.Item(
                        category=data.Category.BUILD,
                        key=key,
                        title=title,
                        value=value,
                        notes=notes,
                    )
                )
            )

    # Compute some extra values that are a function of some assigned variables.
    compilation_mode = assigned_variables.get("compilation_mode")
    results.append(
        data.Result(
            item=data.Item(
                category=data.Category.BUILD,
                key="compilation_mode",
                title="Compilation mode",
                value=compilation_mode,
            )
        )
    )

    return results
