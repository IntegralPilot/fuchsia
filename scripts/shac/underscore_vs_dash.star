# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Reports new files that use a - when it should use a _, or vice versa."""

# If there are fewer than this many files with - or _ in a given directory, move
# up a directory.
MIN_LOCAL_FILES = 10

def _underscore_or_dash(path):
    """Categorizes a path based on whether it contains a `-` or `_`.

    Args:
        path (str): Path of the file to examine.

    Returns:
        If the basename of the path should be considered to have an underscore,
        returns the string "_". If it has a dash, returns "-". Otherwise,
        returns None.
    """
    basename = path.split("/")[-1]
    if "_" in basename and not basename.startswith("_"):
        return "_"
    if "-" in basename:
        return "-"
    return None

def _get_path_prefixes(path):
    """Returns a list of parent directory paths for `path`.

    For instance "a/b/c" yields ["a/b", "a", ""].
    """
    res = []
    segments = path.split("/")
    for i in range(len(segments)):
        res.append("/".join(segments[:-i - 1]))
    return res

def _get_extension(path):
    """Returns the given path's extension (e.g. `.json`).

    Returns the empty string if the file has no extension."""
    basename = path.split("/")[-1]
    if "." in basename:
        return "." + basename.split(".")[-1]
    else:
        return ""

def _count_paths(ctx):
    """Count presence of "_" or "-", given constraints.

    The key is ("_" or "-", extension, parent directory) -> count.
    """
    res = {}
    for f in ctx.scm.all_files(glob = ["*_*", "*-*"]):
        category = _underscore_or_dash(f)
        if not category:
            continue
        ext = _get_extension(f)
        for prefix in _get_path_prefixes(f):
            key = (prefix, ext, category)
            if key not in res:
                res[key] = 0
            res[key] += 1
    return res

def _compute_scores(index, path):
    """Computes scores for _ and -, relative to `path`.

    Gets the lowest-level prefix of `path` that has at least MIN_LOCAL_FILES
    files with a matching extension that use - or _. Returns that path, and how
    many files match.

    Args:
        index: output of _count_paths().
        path: path of a newly added file.

    Returns:
        (prefix, underscore_score, dash_score)
    """
    ext = _get_extension(path)

    prefix = ""
    u_score = 0
    d_score = 0
    for prefix in _get_path_prefixes(path):
        u_score = index.get((prefix, ext, "_"), 0)
        d_score = index.get((prefix, ext, "-"), 0)

        if MIN_LOCAL_FILES <= u_score + d_score:
            break

    return prefix, u_score, d_score

def _emit_finding(ctx, path, category, prefix, underscore_score, dash_score):
    """Emit a finding indicating that the given path makes an unusual choice."""
    ext = _get_extension(path)

    if category == "_":
        choice = r"\_"
        opposite = "-"
    else:
        choice = "-"
        opposite = r"\_"

    if ext == "":
        ext_note = "extensionless"
    else:
        ext_note = "'" + ext + "'"
    if prefix == "":
        prefix_note = "in this repo"
    else:
        prefix_note = "under '%s'" % prefix
    ctx.emit.finding(
        level = "warning",
        message = """filename contains a '{0}' character. Similar files tend to use '{1}'. Consider using '{1}' instead.

Of other {2} files {3}:
* {4} use '\\_'
* {5} use '-'
""".format(choice, opposite, ext_note, prefix_note, underscore_score, dash_score),
        filepath = path,
    )

def underscore_vs_dash(ctx):
    """Reports new files that use a - when it should use a _, or vice versa.

    This check heuristically determines whether a '-' or '_' is more typical,
    based on nearby files with matching extensions.

    Args:
        ctx: shac context.
    """
    added = []
    for path, file in ctx.scm.affected_files(glob = ["*_*", "*-*"]).items():
        if file.action != "A":
            continue
        category = _underscore_or_dash(path)

        if category:
            added.append((path, category))

    # Don't bother indexing if we didn't add any relevant files.
    if not added:
        return

    indexed = _count_paths(ctx)

    for path, category in added:
        # If the file is out of step with the scores, report a finding.
        prefix, underscore_score, dash_score = _compute_scores(indexed, path)

        # Don't count the file itself.
        if category == "_":
            underscore_score -= 1
        else:
            dash_score -= 1

        # We put our thumb on the scale here in favor of "_", as there's this
        # doc, which says "_" is preferred:
        # https://fuchsia.dev/fuchsia-src/development/source_code/layout#naming_conventions
        #
        # Thus, we only recommend "-" if it substantially outnumbers "_".
        if (2 * underscore_score < dash_score and category == "_") or (dash_score < underscore_score and category == "-"):
            _emit_finding(ctx, path, category, prefix, underscore_score, dash_score)

def register_underscore_vs_dash_checks():
    """Register all checks that should run."""
    shac.register_check(underscore_vs_dash)
