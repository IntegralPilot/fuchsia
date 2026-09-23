# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load(
    "@fuchsia_rules_common//assembly:product_configuration.bzl",
    "BUILD_TYPES",
    "FEATURE_SET_LEVELS",
)
load("@fuchsia_rules_common//assembly:providers.bzl", "AssemblyInputBundleInfo")
load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")

def _assembly_input_bundle_impl(ctx):
    out_dir = ctx.actions.declare_directory(ctx.label.name)

    args = ctx.actions.args()
    args.add("create")
    args.add("--outdir", out_dir.path)

    if ctx.attr.experimental:
        args.add("--experimental")

    # Validate and add the allowed_in, scrutiny_required, and auto_include_in values.
    for value in ctx.attr.allowed_in:
        _validate_allowed_in_value("allowed_in", value)
        args.add("--allowed-in", value)

    for value in ctx.attr.scrutiny_required:
        _validate_allowed_in_value("scrutiny_required", value)
        args.add("--scrutiny-required", value)

    for value in ctx.attr.auto_include_in:
        _validate_allowed_in_value("auto_include_in", value)
        args.add("--auto-include-in", value)

    inputs = []

    # Handle package lists
    package_sets = {
        "base_packages": "--base-pkg-list",
        "cache_packages": "--cache-pkg-list",
        "flexible_packages": "--flexible-pkg-list",
        "system_packages": "--system-pkg-list",
        "bootfs_packages": "--bootfs-pkg-list",
        "bootfs_or_base_packages": "--bootfs-or-base-pkg-list",
        "on_demand_packages": "--on-demand-pkg-list",
        "anchored_automatic_packages": "--anchored-automatic-pkg-list",
        "anchored_on_demand_packages": "--anchored-on-demand-pkg-list",
    }

    for attr_name, arg_name in package_sets.items():
        deps = getattr(ctx.attr, attr_name)
        if deps:
            manifests = [dep[FuchsiaPackageInfo].package_manifest.path for dep in deps]
            list_file = ctx.actions.declare_file(ctx.label.name + "_" + attr_name + ".json")
            ctx.actions.write(
                output = list_file,
                content = json.encode(manifests),
            )
            args.add(arg_name, list_file)
            inputs.append(list_file)

            # Add all inputs from the packages
            for dep in deps:
                inputs.extend(dep[FuchsiaPackageInfo].files)

    if ctx.attr.kernel_cmdline:
        cmdline_file = ctx.actions.declare_file(ctx.label.name + "_kernel_cmdline.json")
        ctx.actions.write(
            output = cmdline_file,
            content = json.encode(ctx.attr.kernel_cmdline),
        )
        args.add("--kernel-cmdline", cmdline_file)
        inputs.append(cmdline_file)

    if ctx.attr.shell_commands:
        parsed_shell_commands = json.decode(ctx.attr.shell_commands)
        for shell_cmd in parsed_shell_commands:
            if "package" not in shell_cmd:
                fail("shell_command entries must specify a package name: %s" % shell_cmd)
            if "components" not in shell_cmd:
                fail("shell_command components must be a list of strings pointing to binaries that are components in the package that make up the package: %s" % shell_cmd)
            for key in shell_cmd.keys():
                if key not in ["package", "components", "bootfs_package"]:
                    fail("unknown field in shell_command entry:  %s" % key)

        shell_commands_file = ctx.actions.declare_file(ctx.label.name + "shell_commands.json")
        ctx.actions.write(
            output = shell_commands_file,
            content = json.encode_indent(parsed_shell_commands),
        )
        args.add("--shell-cmds-list", shell_commands_file)
        inputs.append(shell_commands_file)

    if ctx.files.memory_buckets:
        for memory_bucket_file in ctx.files.memory_buckets:
            args.add("--memory-buckets", memory_bucket_file)
            inputs.append(memory_bucket_file)

    if ctx.attr.bootfs_files_package:
        dep = ctx.attr.bootfs_files_package
        args.add("--bootfs-files-package", dep[FuchsiaPackageInfo].package_manifest.path)
        inputs.append(dep[FuchsiaPackageInfo].package_manifest)
        inputs.extend(dep[FuchsiaPackageInfo].files)

    if ctx.file.qemu_kernel:
        args.add("--qemu-kernel", ctx.file.qemu_kernel.path)
        inputs.append(ctx.file.qemu_kernel)

    if ctx.file.kernel:
        args.add("--kernel", ctx.file.kernel.path)
        inputs.append(ctx.file.kernel)

    # Handle shards and generate compiled packages JSON
    compiled_packages = {}

    if ctx.attr.compiled_packages:
        parsed = json.decode(ctx.attr.compiled_packages)

        # Build map from canonical label to path.
        # Since str(dep.label) returns the canonical label string, we rely on the macro
        # having canonicalized the labels in the JSON to match this format.
        path_map = {}
        for dep in ctx.attr.compiled_packages_inputs:
            lbl = str(dep.label)
            if FuchsiaPackageInfo in dep:
                # For package inputs, map the label to the path of its package manifest,
                # which is what the AIB creation tool expects.
                path_map[lbl] = dep[FuchsiaPackageInfo].package_manifest.path

                # Add all the package files (including the manifest) to the action's inputs.
                inputs.extend(dep[FuchsiaPackageInfo].files)
            else:
                # For non-package inputs, they need to be a single file.
                files = dep.files.to_list()
                if len(files) > 1:
                    fail(
                        ("Label '%s' resolved to multiple files, but only single " +
                         "files are supported in 'shards' and 'component_includes' " +
                         "fields of 'compiled_packages'.") % lbl,
                    )
                if not files:
                    fail("Label '%s' did not resolve to any files." % lbl)
                path_map[lbl] = files[0].path

                # Add the file to the action's inputs.
                inputs.extend(files)

        # Update parsed structure with paths
        for pkg in parsed:
            if "packages" in pkg:
                pkg["packages"] = [path_map[lbl] for lbl in pkg["packages"]]

            components = {}
            if "components" in pkg:
                for comp in pkg["components"]:
                    if "shards" in comp:
                        comp["shards"] = [path_map[lbl] for lbl in comp["shards"]]
                    components[comp["component_name"]] = comp
            pkg["components"] = components

            if "component_includes" in pkg:
                for inc in pkg["component_includes"]:
                    if "source" in inc:
                        inc["source"] = path_map[inc["source"]]
            compiled_packages[pkg["name"]] = pkg

    shard_configs = [
        ("bootstrap_shards", "bootstrap", True),
        ("core_shards", "core", False),
        ("root_shards", "root", True),
        ("toolbox_shards", "toolbox", True),
    ]

    for attr_name, name, is_bootfs in shard_configs:
        shards = getattr(ctx.attr, attr_name)
        if shards:
            shard_paths = [f.path for f in getattr(ctx.files, attr_name)]

            # Add the files from the hardcoded shards to the action's inputs.
            inputs.extend(getattr(ctx.files, attr_name))

            # Get or create package
            pkg = compiled_packages.setdefault(name, {
                "name": name,
                "components": {},
                "bootfs_package": is_bootfs,
            })

            # Get or create component
            comp = pkg["components"].setdefault(name, {
                "component_name": name,
                "shards": [],
            })

            comp["shards"].extend(shard_paths)

    if compiled_packages:
        # Convert components dict to a list for JSON.
        compiled_packages_list = []
        for pkg in compiled_packages.values():
            new_pkg = dict(pkg)
            new_pkg["components"] = list(pkg["components"].values())
            compiled_packages_list.append(new_pkg)

        compiled_packages_file = ctx.actions.declare_file(ctx.label.name + "_compiled_packages.json")
        ctx.actions.write(
            output = compiled_packages_file,
            content = json.encode_indent(compiled_packages_list),
        )
        args.add("--compiled-packages", compiled_packages_file)
        inputs.append(compiled_packages_file)

    # Handle drivers
    drivers_list = []
    if ctx.attr.drivers:
        parsed_drivers = json.decode(ctx.attr.drivers)
        path_map = {}
        for dep in ctx.attr.drivers_inputs:
            lbl = str(dep.label)
            path_map[lbl] = dep[FuchsiaPackageInfo].package_manifest.path
            inputs.extend(dep[FuchsiaPackageInfo].files)

        for drv in parsed_drivers:
            if "package_target" not in drv:
                fail("driver entries must specify a package_target: %s" % drv)
            if "driver_components" not in drv:
                fail("driver entries must specify driver_components: %s" % drv)
            if "set" not in drv:
                fail("driver entries must specify set: %s" % drv)

            lbl = drv["package_target"]
            if lbl in path_map:
                drv["package_target"] = path_map[lbl]
            else:
                fail("Driver package %s not found in inputs" % lbl)
            drivers_list.append(drv)

    if drivers_list:
        drivers_file = ctx.actions.declare_file(ctx.label.name + "_drivers.json")
        ctx.actions.write(
            output = drivers_file,
            content = json.encode(drivers_list),
        )
        args.add("--drivers-list", drivers_file)
        inputs.append(drivers_file)

    # Handle config_data
    config_data_list = []
    if ctx.attr.config_data:
        parsed_config_data = json.decode(ctx.attr.config_data)
        path_map = {}
        for dep in ctx.attr.config_data_inputs:
            lbl = str(dep.label)
            files = dep.files.to_list()
            if len(files) > 1:
                fail(
                    ("Label '%s' resolved to multiple files, but only single " +
                     "files are supported in the 'source' field of 'config_data'.") % lbl,
                )
            if not files:
                fail("Label '%s' did not resolve to any files in config_data" % lbl)
            path_map[lbl] = files[0].path
            inputs.extend(files)

        for pkg in parsed_config_data:
            package_name = pkg["package_name"]
            for file in pkg["files"]:
                lbl = file["source"]
                if lbl in path_map:
                    source_path = path_map[lbl]
                else:
                    fail("Config data source %s not found in inputs" % lbl)
                config_data_list.append({
                    "package_name": package_name,
                    "source": source_path,
                    "destination": file["destination"],
                })

    if config_data_list:
        config_data_file = ctx.actions.declare_file(ctx.label.name + "_config_data.json")
        ctx.actions.write(
            output = config_data_file,
            content = json.encode(config_data_list),
        )
        args.add("--config-data-list", config_data_file)
        inputs.append(config_data_file)

    ctx.actions.run(
        inputs = inputs,
        outputs = [out_dir],
        executable = ctx.executable._tool,
        arguments = [args],
        mnemonic = "AssemblyInputBundle",
        progress_message = "Creating Assembly Input Bundle %s" % ctx.label.name,
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [
        DefaultInfo(files = depset([out_dir])),
        AssemblyInputBundleInfo(
            name = ctx.label.name,
            directory = out_dir.path,
        ),
    ]

_assembly_input_bundle = rule(
    implementation = _assembly_input_bundle_impl,
    attrs = {
        "allowed_in": attr.string_list(),
        "scrutiny_required": attr.string_list(),
        "auto_include_in": attr.string_list(),
        "experimental": attr.bool(),
        "base_packages": attr.label_list(providers = [FuchsiaPackageInfo]),
        "cache_packages": attr.label_list(providers = [FuchsiaPackageInfo]),
        "flexible_packages": attr.label_list(providers = [FuchsiaPackageInfo]),
        "system_packages": attr.label_list(providers = [FuchsiaPackageInfo]),
        "bootfs_packages": attr.label_list(providers = [FuchsiaPackageInfo]),
        "bootfs_or_base_packages": attr.label_list(providers = [FuchsiaPackageInfo]),
        "on_demand_packages": attr.label_list(providers = [FuchsiaPackageInfo]),
        "anchored_automatic_packages": attr.label_list(providers = [FuchsiaPackageInfo]),
        "anchored_on_demand_packages": attr.label_list(providers = [FuchsiaPackageInfo]),
        "kernel_cmdline": attr.string_list(),
        "shell_commands": attr.string(),
        "memory_buckets": attr.label_list(allow_files = True),
        "bootstrap_shards": attr.label_list(allow_files = True),
        "core_shards": attr.label_list(allow_files = True),
        "root_shards": attr.label_list(allow_files = True),
        "toolbox_shards": attr.label_list(allow_files = True),
        "compiled_packages": attr.string(),
        # Internal attribute to pass labels extracted from compiled_packages by the macro.
        "compiled_packages_inputs": attr.label_list(allow_files = True),
        "drivers": attr.string(),
        "drivers_inputs": attr.label_list(providers = [FuchsiaPackageInfo]),
        "config_data": attr.string(),
        "config_data_inputs": attr.label_list(allow_files = True),
        "bootfs_files_package": attr.label(providers = [FuchsiaPackageInfo]),
        "qemu_kernel": attr.label(allow_single_file = True),
        "kernel": attr.label(allow_single_file = True),
        "_tool": attr.label(
            default = "//build/assembly/scripts:assembly_input_bundle_tool",
            executable = True,
            cfg = "exec",
        ),
    },
)

def assembly_input_bundle(
        *,
        name,
        allowed_in = [],
        scrutiny_required = [],
        auto_include_in = [],
        experimental = False,
        base_packages = [],
        cache_packages = [],
        flexible_packages = [],
        system_packages = [],
        bootfs_packages = [],
        bootfs_or_base_packages = [],
        on_demand_packages = [],
        anchored_automatic_packages = [],
        anchored_on_demand_packages = [],
        kernel_cmdline = [],
        shell_commands = [],
        memory_buckets = [],
        bootstrap_shards = [],
        core_shards = [],
        root_shards = [],
        toolbox_shards = [],
        compiled_packages = [],
        drivers = [],
        config_data = [],
        qemu_kernel = None,
        kernel = None,
        bootfs_files_package = None,
        **kwargs):
    """Creates an Assembly Input Bundle.

    Args:
        name: The name of the target.

        experimental: [boolean, default False]
            Whether this AIB is experimental and should be excluded from the scrutiny goldens.
            Experimental AIBs must be available in userdebug and never in user. Experimental AIBs
            are allowed in any feature set level.

            The typical way to mark an AIB as available in userdebug but not user is to
            set `experimental = true` and `allowed_in = ["userdebug", "eng"]`.

        allowed_in: [list of strings] Which feature set + build type combinations this AIB is
           allowed to be included in. Assembly asserts during product assembly that
           the AIB is not included in any feature set + build type combinations
           that are not in this list.

           Options:
           - "everything"
           - "standard", "utility", "bootstrap", "embeddable"
           - "user", "userdebug", "eng"
           - "standard::user", "utility::eng", etc.

        scrutiny_required [list of strings]
            Which feature set + build type combinations to expect the contents of this AIB to be
            added to. Adding to this list will force the contents of this AIB to be listed as
            required in scrutiny goldens.  Valid values are the same as for 'allowed_in'.

        auto_include_in: [list of strings]
            Which feature set + build type combinations to automatically include this AIB in.  Valid
            values are the same as for 'allowed_in'

        base_packages: [list of labels] Package targets to include in the base package set.

        cache_packages: [list of labels] Package targets to include in the cache package set.

        flexible_packages: [list of labels] Package targets that assembly may choose to put in base,
            cache, or elsewhere.

        system_packages: [list of labels] Package targets to include in the system package set.

        bootfs_packages: [list of labels] Package targets to include in the bootfs package set.

        bootfs_files_package: [label] A label pointing to a package target
            containing bootfs files to include in the assembly input bundle.
            This package is typically generated in GN to handle resource/binary
            aggregation.


        bootfs_or_base_packages: [list of labels] Package targets to include in the bootfs_or_base
            package set.

        on_demand_packages: [list of labels] Package targets to include in the on-demand package
            set.

        anchored_automatic_packages: [list of labels] Package targets to include in the anchored
            automatic package set.

        anchored_on_demand_packages: [list of labels] Package targets to include in the anchored
            on-demand package set.

        kernel_cmdline: [list of strings] Kernel cmdline arguments.

        shell_commands: [list of dicts] A list of dicts that describe the shell commands for each
            listed package

            Example:
            shell_commands = [
              {
                package = "//third_party/sbase"
                components = [ "ls" ]
              },
            ]

        memory_buckets: [list of labels] Paths to memory bucket configs that should get merged
            and passed to memory monitor.

        bootstrap_shards: [list of labels] A list of CML shard files to add to the "bootstrap"
            component of the "bootstrap" compiled package. This package is a bootfs package.

        core_shards: [list of labels] A list of CML shard files to add to the "core"
            component of the "core" compiled package.

        root_shards: [list of labels] A list of CML shard files to add to the "root"
            component of the "root" compiled package. This package is a bootfs package.

        toolbox_shards: [list of labels] A list of CML shard files to add to the "toolbox"
            component of the "toolbox" compiled package. This package is a bootfs package.

        compiled_packages: [list of dicts] List of dicts that describe packages that are to be built
            dynamically by Assembly, for example, the `core` package.

            Example:
            compiled_packages = [
              {
                name = "core"
                packages = [ "//path/to:package" ]
                components = [
                  {
                    component_name = "core"
                    shards = [
                            "//src/sys/process-resolver:process_resolver.core_shard.cml",
                    ]
                  },
                ]
                component_includes = [ ... ]
              },
            ]

            shards [optional]
              [list of labels] List of CML files to merge together when
              compiling the component.

            cmc_features [optional]
              [list of strings] List of CMC features to enable for component
              compilation.

            component_includes [optional]
              [list of dicts] List of source/destination pairs related to a
              compiled package in the compiled_packages list that specifies cml files
              that can be included by any cml shards in any platform AIB for the given
              package. These files will be included in the Assembly Input Bundle.

        drivers: [list of dicts] List of dicts that describe driver packages to include in the assembly input bundle.
            Example:
            drivers = [
              {
                package_target = "//path/to:driver_package"
                driver_components = [ "meta/driver.cm" ]
                set = "base"
              }
            ]

        config_data: [list of dicts] List of dicts that describe config data associated with a given package.
            Example:
            config_data = [
              {
                "package_name": "example_package",
                "files": [
                  {
                    "source": "//src/path_to_config:config1.json",
                    "destination": "config1.json"
                  },
                ]
              }
            ]

        qemu_kernel: [label] Optional label to a single file to use as the qemu kernel.

        **kwargs: Other arguments to pass to the rule.
    """

    # Extract labels from compiled_packages to pass to the rule.
    #
    # We also canonicalize the labels here (converting them to strings using str(Label(...))).
    # This is done because the rule implementation needs to look up these labels in the
    # compiled_packages_inputs list to get their file paths. Since str(dep.label) in the
    # implementation returns a canonical label string, we must ensure the strings in our JSON
    # match that canonical format, regardless of how the user wrote the label (e.g., relative
    # vs absolute).
    #
    #  e.g.  //some/path/to:foo/bar.cml => "@@//some/path/to:foo/bar.cml"

    compiled_packages_inputs = []
    for pkg in compiled_packages:
        if "packages" in pkg:
            packages_labels = [str(Label(lbl)) for lbl in pkg["packages"]]
            pkg["packages"] = packages_labels

            # Add the package labels to the inputs list so Bazel tracks them as dependencies
            # and makes their files available to the action.
            compiled_packages_inputs.extend(packages_labels)
        if "components" in pkg:
            for comp in pkg["components"]:
                if "shards" in comp:
                    shards_labels = [str(Label(lbl)) for lbl in comp["shards"]]
                    comp["shards"] = shards_labels

                    # Add the shard labels to the inputs list so Bazel tracks them as dependencies.
                    compiled_packages_inputs.extend(shards_labels)
        if "component_includes" in pkg:
            for inc in pkg["component_includes"]:
                if "source" in inc:
                    source_label = str(Label(inc["source"]))
                    inc["source"] = source_label

                    # Add the component include source labels to the inputs list so Bazel tracks them as dependencies.
                    compiled_packages_inputs.append(source_label)

    compiled_packages_inputs = depset(compiled_packages_inputs).to_list()

    # Extract labels from drivers to pass to the rule and canonicalize them.
    #
    # We canonicalize the labels here (converting them to strings using str(Label(...))).
    # This is done because the rule implementation needs to look up these labels in the
    # drivers_inputs list to get their file paths. Since str(dep.label) in the
    # implementation returns a canonical label string, we must ensure the strings in our JSON
    # match that canonical format.
    #
    #  e.g.  //some/path/to:foo/bar.cml => "@@//some/path/to:foo/bar.cml"

    drivers_inputs = []
    for drv in drivers:
        if "package_target" in drv:
            driver_label = str(Label(drv["package_target"]))
            drv["package_target"] = driver_label
            drivers_inputs.append(driver_label)
    drivers_inputs = depset(drivers_inputs).to_list()

    # Extract labels from config_data to pass to the rule and canonicalize them.
    #
    # We canonicalize the labels here (converting them to strings using str(Label(...))).
    # This is done because the rule implementation needs to look up these labels in the
    # config_data_inputs list to get their file paths. Since str(dep.label) in the
    # implementation returns a canonical label string, we must ensure the strings in our JSON
    # match that canonical format.
    #
    #  e.g.  //some/path/to:foo/bar.cml => "@@//some/path/to:foo/bar.cml"

    config_data_inputs = []
    for pkg in config_data:
        if "files" in pkg:
            for file in pkg["files"]:
                if "source" in file:
                    source_label = str(Label(file["source"]))
                    file["source"] = source_label
                    config_data_inputs.append(source_label)
    config_data_inputs = depset(config_data_inputs).to_list()

    _assembly_input_bundle(
        name = name,
        experimental = experimental,
        allowed_in = allowed_in,
        scrutiny_required = scrutiny_required,
        auto_include_in = auto_include_in,
        base_packages = base_packages,
        cache_packages = cache_packages,
        flexible_packages = flexible_packages,
        system_packages = system_packages,
        bootfs_packages = bootfs_packages,
        bootfs_or_base_packages = bootfs_or_base_packages,
        on_demand_packages = on_demand_packages,
        anchored_automatic_packages = anchored_automatic_packages,
        anchored_on_demand_packages = anchored_on_demand_packages,
        kernel_cmdline = kernel_cmdline,
        shell_commands = json.encode_indent(shell_commands),
        memory_buckets = memory_buckets,
        bootstrap_shards = bootstrap_shards,
        core_shards = core_shards,
        root_shards = root_shards,
        toolbox_shards = toolbox_shards,
        compiled_packages = json.encode(compiled_packages),
        compiled_packages_inputs = compiled_packages_inputs,
        drivers = json.encode(drivers),
        drivers_inputs = drivers_inputs,
        config_data = json.encode(config_data),
        config_data_inputs = config_data_inputs,
        qemu_kernel = qemu_kernel,
        kernel = kernel,
        bootfs_files_package = bootfs_files_package,
        **kwargs
    )

# Construct the lists of valid build types and feature set levels.  Only do this once, not each
# time we validate a value.
_build_types = json.decode(json.encode(BUILD_TYPES)).values()
_feature_set_levels = json.decode(json.encode(FEATURE_SET_LEVELS)).values()

def _validate_allowed_in_value(list_name, value):
    if value == "everything":
        return

    if value in _build_types:
        return

    if value in _feature_set_levels:
        return

    tokens = value.split("::")
    if len(tokens) == 2:
        if tokens[0] in _feature_set_levels and tokens[1] in _build_types:
            return

    fail("'%s' is an invalid value for '%s'.  The valid values are '" % (value, list_name) + "', '".join(_build_types + _feature_set_levels) + "' or a '<feature_set_level>::<build_type>'.")

def _assembly_resources_directory_impl(ctx):
    out_dir = ctx.actions.declare_directory(ctx.label.name)

    # We're using a shell script to create the resources directory because we cannot have
    # Bazel directly copy files into a directory that we declare with declare_directory().
    cmds = [
        "mkdir -p %s" % out_dir.path,
        "printf '{}' > %s/assembly_config.json" % out_dir.path,
    ]

    for f in ctx.files.sources:
        dest_name = f.basename
        if dest_name == "latest_commit_date.txt" or dest_name == "build_info_version.txt":
            dest_name = "version.txt"
        cmds.append("cp %s %s/%s" % (f.path, out_dir.path, dest_name))

    ctx.actions.run_shell(
        inputs = ctx.files.sources,
        outputs = [out_dir],
        command = "\n".join(cmds),
        mnemonic = "AssemblyResourcesDirectory",
        progress_message = "Creating Assembly Resources Directory %s" % ctx.label.name,
    )

    return [
        DefaultInfo(files = depset([out_dir])),
        AssemblyInputBundleInfo(
            name = ctx.label.name,
            directory = out_dir.path,
        ),
    ]

assembly_resources_directory = rule(
    implementation = _assembly_resources_directory_impl,
    attrs = {
        "sources": attr.label_list(
            allow_files = True,
            mandatory = True,
            doc = "List of resource files to be added to the directory.",
        ),
    },
)
