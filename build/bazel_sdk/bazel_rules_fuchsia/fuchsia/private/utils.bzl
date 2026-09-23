# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common utilities needed by Bazel SDK rules."""

load(
    "@fuchsia_rules_common//:utils.bzl",
    "flatten",
    "stub_executable",
)
load(":providers.bzl", "FuchsiaProvidersInfo")

_INVALID_LABEL_CHARACTERS = "\"!%@^_#$&'()*+,;<=>?[]{|}~/".elems()

def normalized_target_name(label):
    label = label.lower()
    for c in _INVALID_LABEL_CHARACTERS:
        label = label.replace(c, ".")
    return label

def append_suffix_to_label(label_str, suffix, separator = "."):
    """ Canonicalizes a label given at the macro-level and appends a suffix.

    foo -> foo.bar
    :foo -> :foo.bar
    //src/foo:foo -> //src/foo:foo.bar
    //src/foo:baz -> //src/foo:baz.bar
    //src/foo -> //src/foo:foo.bar

    Args:
        label_str: The label string to append to.
        suffix: The suffix to append.
        separator: An optional string to use as a separator.
    Returns:
        The new name with the suffix appended
    """
    unqualified_name = separator.join([Label(label_str).name, suffix])
    if label_str.startswith(":"):
        return ":{}".format(unqualified_name)
    elif label_str.startswith("//"):
        return "{}:{}".format(label_str.split(":")[0], unqualified_name)
    else:
        return unqualified_name

def get_project_execroot(ctx):
    # Gets the project/workspace execroot relative to the output base.
    # See https://bazel.build/docs/output_directories.
    return "execroot/%s" % ctx.workspace_name

def get_target_execroot(ctx, target):
    # Gets the execroot for a given target, relative to the project execroot.
    # See https://bazel.build/docs/output_directories.
    return target[DefaultInfo].files_to_run.runfiles_manifest.dirname + "/" + ctx.workspace_name

def collect_runfiles(ctx, *elements, ignore_types = []):
    # buildifier: disable=function-docstring-args
    # buildifier: disable=function-docstring-return
    """Collects multiple types of elements (...files, ...targets, ...runfiles) into runfiles."""

    # Map to runfiles objects.
    runfiles = []
    for elem in flatten(elements):
        if type(elem) == "Target":
            runfiles.append(elem[DefaultInfo].default_runfiles)
            files_to_run = elem[DefaultInfo].files_to_run
            if files_to_run.executable and files_to_run.runfiles_manifest:
                runfiles.append(ctx.runfiles([
                    files_to_run.executable,
                    files_to_run.runfiles_manifest,
                ]))
        elif type(elem) == "File":
            runfiles.append(ctx.runfiles([elem]))
        elif type(elem) == "runfiles":
            runfiles.append(elem)
        elif type(elem) not in ignore_types:
            fail("Unable to get runfiles from %s: %s" % (type(elem), str(elem)))

    # Merges runfiles for a given target.
    return ctx.runfiles().merge_all(runfiles)

def wrap_executable(ctx, executable, *arguments, script_name = None):
    # buildifier: disable=function-docstring-args
    # buildifier: disable=function-docstring-return
    """Wraps an executable with predefined command line arguments.

    Creates a wrapper script that invokes an underlying executable with
    predefined command line arguments.

    script_name defaults to `run_${target_name}.sh`.
    """
    wrapper = ctx.actions.declare_file(script_name or "run_%s.sh" % ctx.attr.name)

    # Convert file arguments into strings and serialize arguments.
    def serialize(arg):
        readlink = False
        if type(arg) == "Target":
            arg = arg[DefaultInfo].files_to_run.executable
            readlink = True
        if type(arg) == "File":
            arg = arg.short_path
        arg = "'%s'" % arg.replace("'", "\\'")

        # Follow symlink for complex tool executables, otherwise we will run
        # into issues with nested runfiles symlink farms.
        if readlink:
            arg = "$(readlink -f %s)" % arg
        return arg

    command = [serialize(arg) for arg in [executable] + list(arguments)]

    ctx.actions.write(wrapper, """#!/bin/bash
%s $@
""" % " ".join(command), is_executable = True)
    return wrapper, collect_runfiles(ctx, executable, arguments, ignore_types = ["string"])

def _add_providers_info(implementation):
    """Wrap a rule implementation function to add a FuchsiaProvidersInfo provider.

    Args:
       implementation: A rule implementation function, i.e. a callable object that
          takes a single rule 'ctx' value as argument and returns a list of
          providers.

    Returns:
       A new rule implementation function / callable object, which returns the
       result of calling 'implementation(ctx)', after potentially appending a
       FuchsiaProvidersInfo provider to it.
    """

    def _impl(ctx):
        return track_providers(implementation(ctx))

    return _impl

def _add_default_executable(implementation):
    """Wrap a rule implementation function to add a default stub executable if needed.

    This returns a new callable object that acts as a rule implementation function,
    i.e. it accepts a single 'ctx' rule context argument, and will first call
    'implementation(ctx)' with it to retrieve a list of provider values.

    If that list does not include a DefaultInfo value, a new one will be appended,
    which points to a stub shell script executable. The script will print an error
    message at build time to indicate that the target is not really executable.

    Used internally by rule_variants(), see related documentation for more details.

    Args:
       implementation: A rule implementation function, i.e. a callable object that
          takes a single rule 'ctx' value as argument and returns a list of
          providers.

    Returns:
       A new rule implementation function / callable object, which returns the
       result of calling 'implementation(ctx)', after potentially appending a
       DefaultInfo value to it.
    """

    def _impl(ctx):
        providers = implementation(ctx)
        if not [provider for provider in providers if type(provider) == "DefaultInfo"]:
            providers.append(DefaultInfo(executable = stub_executable(ctx)))
        return providers

    return _impl

def rule_variants(implementation, variants = [], attrs = {}, **rule_kwargs):
    """Creates variants of a rule.

    Creates one or more rule() objects whose attributes vary slightly based on
    the value of items in the 'variants' input list, and which share a common
    implementation function.

    Example usage:

       ```
       def _foo_impl(ctx):
           ....

       foo_binary, foo_test = rule_variants(
            _foo_impl, ["executable", "test"], attrs = {...})
       ```

    Args:
        implementation: base rule implementation function used by all
           result rule instances. Must take a single 'ctx' argument.

        variants: A list of variant, each item can be None or a string
           describing a non-default variant. Valid values are:

           - None: Create a rule() that uses 'implementation' and 'attrs' directly.
                This sets both 'executable' and 'test' to False.

           - "executable": Create a rule() that uses an implementation function
                that calls 'implementation' then looks at its result, and will
                add a stub executable *if* it does not include an executable in
                its DefaultInfo value (this is used to print an error message
                at build or run time to tell the user the target is not really
                executable). Sets 'executable' to True, and 'test' to False.

            - "test": Similar to "executable" but sets 'test' to True, making
                the target usable with `bazel test`.

        attrs: A rule() 'attrs' dictionary value, providing common attributes
           for all result rule instances. For each result rule() value, this will
           be augmented with a private '_variant' attribute corresponding to
           the 'variants' item value used to create it.

        **rule_kwargs: Extra arguments are passed directly to each result rule()
           constructor.

    Returns:
        A list of rule() instances, one per item in 'variants'.
    """
    return [rule(
        executable = variant == "executable",
        test = variant == "test",
        attrs = dict(attrs, _variant = attr.string(default = variant or "")),
        implementation = _add_providers_info(
            implementation if variant == None else _add_default_executable(implementation),
        ),
        **rule_kwargs
    ) for variant in variants]

def rule_variant(implementation, variant = None, attrs = {}, **rule_kwargs):
    """Creates a variant of a rule. See rule_variants for argument descriptions."""
    return rule_variants(variants = [variant], attrs = attrs, implementation = implementation, **rule_kwargs)[0]

def track_providers(providers):
    return providers + [FuchsiaProvidersInfo(
        providers = [
            provider
            for provider in providers
            if type(provider) != "DefaultInfo"
        ],
    )]

def can_forward_provider(provider):
    """Return True if a given provider value should never be forwarded to dependents.

    This is important for providers that are collected through aspects, as forwarding
    them (e.g. with a function like forward_providers()), would create
    duplicate entries in the build graph, resulting in chaos.

    Unlike native providers like DefaultInfo or CCInfo, `type(provider)` will always
    return "struct" for custom providers, so instead rely on the fact that their values
    include a "never_forward" field which should be True if they should not be
    forwarded.

    Args:
       provider: A provider value.
    Returns:
       True if the value can be forwarded to dependents by fowrard_providers().
    """
    return not getattr(provider, "never_forward", False)

# buildifier: disable=function-docstring
def forward_providers(ctx, target, *providers, rename_executable = None):
    default_info = target[DefaultInfo]
    if default_info.files_to_run and default_info.files_to_run.executable:
        executable = default_info.files_to_run.executable
        executable_symlink = ctx.actions.declare_file(
            rename_executable or "_" + executable.basename,
        )
        ctx.actions.symlink(
            output = executable_symlink,
            target_file = executable,
            is_executable = True,
        )
        default_info = DefaultInfo(
            files = depset([executable_symlink] + [
                file
                for file in default_info.files.to_list()
                if file != executable
            ]) if rename_executable else default_info.files,
            runfiles = default_info.default_runfiles,
            executable = executable_symlink,
        )
    target_provider_info = target[FuchsiaProvidersInfo] if (
        FuchsiaProvidersInfo in target
    ) else struct(providers = [])
    return [
        target[Provider]
        for Provider in providers
        if Provider in target and can_forward_provider(Provider)
    ] + target_provider_info.providers + [default_info]

def _forward_providers(ctx):
    return forward_providers(ctx, ctx.attr.actual)

_alias, _alias_for_executable, _alias_for_test = rule_variants(
    variants = (None, "executable", "test"),
    implementation = _forward_providers,
    attrs = {
        "actual": attr.label(
            doc = "The test workflow entity target to alias.",
            providers = [FuchsiaProvidersInfo],
            mandatory = True,
        ),
    },
)

def alias(*, name, executable, testonly = False, **kwargs):
    # buildifier: disable=function-docstring-header
    """
    We have to create our own alias macro because Bazel is unreasonable:
    https://github.com/bazelbuild/bazel/issues/10893

    The underlying target must be created with `rule_variant(s)` or manually
    include `FuchsiaProvidersInfo` in order to forward providers.
    """
    return ((
        _alias_for_test if testonly else _alias_for_executable
    ) if executable else _alias)(
        name = name,
        testonly = testonly,
        **kwargs
    )

def filter(obj, value = None, exclude = True):
    # buildifier: disable=function-docstring-args
    # buildifier: disable=function-docstring-return
    """Recursively removes matching fields/elements from an object by mutating."""
    if type(obj) not in ("dict", "list"):
        fail("Unsupported data type.")

    nested_fields = [obj]

    # Since dictionaries and lists can be represented as DAGs, this represents
    # one filter operation within an iterative BFS.
    def filter_next():
        obj = nested_fields.pop(0)

        # Lists and dictionaries can both be represented as key-value pairs.
        for k, nested in (obj.items() if type(obj) == "dict" else enumerate(obj)):
            if type(nested) in ("dict", "list"):
                # Add a nested object to the BFS queue.
                nested_fields.append(nested)
            elif (nested == value) == exclude:
                # Remove the matching value's field by mutating the object.
                obj.pop(k)

    # Using and iterative BFS to filter all matching values within `obj` should
    # take less than `len(str(obj))` iterations.
    for _ in range(len(str(obj))):
        # Empty nested_fields means that we're done with our BFS.
        if not nested_fields:
            return obj
        filter_next()

    # In case the previous assumption is violated.
    fail("Unable to filter all none values!")

def get_runfiles(target):
    # Helper function to get the runfiles as a list of files from a target.
    return [symlink.target_file for symlink in target[DefaultInfo].default_runfiles.root_symlinks.to_list()]

# Libs all end with .so or .so followed by a semantic version.
# Examples: libname.so, libname.so.1, libname.so.1.1
# buildifier: disable=function-docstring
def is_lib(file):
    rparts = file.basename.rpartition(".so")
    if (rparts[1] != ".so"):
        return False
    for char in rparts[2].elems():
        if not (char.isdigit() or char == "."):
            return False

PREPROCESS_FILE_ATTRS = {
    "_preprocessor_binary": attr.label(
        default = Label("@fuchsia_clang//:bin/clang-cpp"),
        executable = True,
        allow_single_file = True,
        cfg = "exec",
    ),
}

def preprocess_file(ctx, source, includes, headers = [], files = depset()):
    """Helper function for .S file preprocessing.

    This function must be called from a rule implementation function
    whose rule definitions' attributes includes PREPROCESS_FILE_ATTRS

    Args:
      ctx: The rule context.
      source: The source file to be preprocessed. It should be a .S file
      includes: A depset of includes paths (as strings) needed for preprocessing.
      headers: A list of headers in the format of File.
      files: A depset of additional inputs to preprocessing action.

    Returns:
      A File of preprocessed file.
    """

    dest_filename = source.basename.rstrip(".S")
    output_file = ctx.actions.declare_file(dest_filename)

    pp_args = ctx.actions.args()
    pp_args.add("-undef")
    pp_args.add("-P")
    pp_args.add_all(includes, before_each = "-I")
    pp_args.add("-x", "assembler-with-cpp")
    pp_args.add(source)
    pp_args.add("-o", output_file)

    ctx.actions.run(
        executable = ctx.executable._preprocessor_binary,
        arguments = [pp_args],
        inputs = [source] + headers + files.to_list(),
        outputs = [output_file],
        mnemonic = "ClangPreprocessor",
    )
    return output_file
