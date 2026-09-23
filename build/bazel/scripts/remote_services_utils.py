# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Functions to generate a remote_services.bazelrc file from a template and RBE config files.
"""

import typing as T
from pathlib import Path

# The list of configuration files, relative to the FUCHSIA_DIR.
RBE_CONFIG_FILES = [
    "build/rbe/fuchsia-rewrapper.cfg",
    "build/rbe/fuchsia-reproxy.cfg",
]

# Location of template file.
RBE_TEMPLATE_FILE = "build/bazel/templates/template.remote_services.bazelrc"


def generate_rbe_config(
    fuchsia_dir: Path, config_files: T.Sequence[str] = RBE_CONFIG_FILES
) -> tuple[dict[str, str], set[Path]]:
    """Read RBE config files into a dictionary.

    Args:
        fuchsia_dir: Fuchsia source directory.
        config_files: An optional list of config file path strings, relative to
           fuchsia_dir.
    Returns:
        A (config_dict, input_files) pair, where config_dict is a dictionary
        mapping configuration keys to values, and input_files is a set of
        input file Paths read by the function.
    """
    input_files: set[Path] = set()
    config: dict[str, str] = {}
    for f in config_files:
        cfg_file = fuchsia_dir / f
        input_files.add(cfg_file)
        for line in cfg_file.read_text().splitlines():
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                # skip empty and comment lines
                continue
            key, sep, value = stripped.partition("=")
            if sep == "=":
                config[key] = value

    return (config, input_files)


def generate_rbe_template_substitutions(
    config_dict: dict[str, str], remote_download_outputs: str
) -> dict[str, str]:
    """Generate formatting arguments to expand the remote services template.

    Args:
        config_dict: The value returned by generate_rbe_config
        remote_download_outputs: Value for remote_download_outputs key.
    Returns:
        A dictionary used to expand template.remote_services.bazelrc.
    """
    # Expected format: projects/<project_name>/instances/<name>
    instance = config_dict["instance"]
    project = instance.split("/")[1]

    # Expected format: comma-separated "key=value" strings.
    platform_values = config_dict["platform"].split(",")
    platform_vars = {}
    for pv in platform_values:
        k, _, v = pv.partition("=")
        platform_vars[k] = v

    return {
        "remote_download_outputs": remote_download_outputs,
        "remote_instance_name": instance,
        "rbe_project": project,
        "container_image": platform_vars.get("container-image", ""),
    }


def generate_remote_services_bazelrc(
    fuchsia_dir: Path,
    output_path: Path,
    download_outputs: str,
) -> set[Path]:
    """Generate the remote_services.bazelrc file.

    Args:
        fuchsia_dir: Fuchsia source directory.
        output_path: Output file location.
        download_outputs: Value for "remote_download_outputs".

    Returns:
        A set of Path values for the input files read by this function.
    """
    rbe_config, rbe_inputs = generate_rbe_config(fuchsia_dir)
    rbe_substitutions = generate_rbe_template_substitutions(
        rbe_config, download_outputs
    )

    rbe_template_path = fuchsia_dir / RBE_TEMPLATE_FILE
    rbe_inputs.add(rbe_template_path)
    rbe_template = rbe_template_path.read_text()

    remote_services_bazelrc = rbe_template.format(**rbe_substitutions)
    output_path.write_text(remote_services_bazelrc)

    return rbe_inputs
