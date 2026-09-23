# Creating static analyzers for Fuchsia

Shac (Scalable Hermetic Analysis and Checks) is a unified and ergonomic tool and
framework for writing and running static analysis checks. The tool’s source can
be found in the [shac-documentation]. Shac checks are written in
[Starlark].

## Setup

Shac script implementations live in Fuchsia’s `//scripts/shac` directory.

* A shac check is implemented as a starlark functions which takes a ctx
  argument. Use this ctx argument to access the shac standard library.
* If your check is language specific, it should go in one of the language
  specific files (Eg: `rust.star`, `go.star`, `fidl.star`). If it’s language
  specific but does not have a `language.star` file, then create one. If it’s generic,
  use `title.star` (where title is the name of the check function).

### Simple Example

The following example is a static analyzer on all files that creates a
non-blocking, gerrit warning comment on changes where the string “http://”
exists, pointing the user to use “https://” instead.

```python
def http_links(ctx):
    for path, meta in ctx.scm.affected_files().items():
        for num, line in meta.new_lines():
            matches = ctx.re.allmatches(r"(http://)\w+", line)
            if not matches:
                continue
            for match in matches:
                ctx.emit.finding(
                    message = "Avoid http:// links, prefer https://",
                    # Change to "error" if the check should block presubmit.
                    level = "warning",
                    filepath = path,
                    line = num,
                    col = match.offset + 1,
                    end_col = match.offset + 1 + len(match.groups[1]),
                    replacements = ["https://"],
                )
```

Learn more about shac’s implementation of [emit.findings].

Note that Shac does not automatically discover checks. In order for a check to
run, a check function must be passed to `shac.register_check()` in
`//scripts/shac/main.star`:

```python
load("./http_links.star", "http_links")  # NEW

...

def register_all_checks():
    ...
    shac.register_check(http_links)  # NEW
    ...
```

When implementing a new check in a file that already contains other checks, you
may be able to register the new check within that file. For example,
`//scripts/shac/fidl.star` has a `register_fidl_checks()` function that gets
called from `//scripts/shac/main.star`. Add new FIDL checks to `fidl.star` and
register them in the `register_fidl_checks()` function in the same file.

### Advanced example

Using a subprocess is useful if there’s an existing tool that does the check or
if the logic of the check is complex (e.g. more than just a substring search).
Starlark is intentionally feature-limited to encourage writing complicated
business logic in a self-contained tool with its own unit tests.

The following is an example of a JSON formatter implemented in a separate Python
script and run as a subprocess.

Rather than rewriting badly formatted files, the check computes the formatted
contents and passes them to the `replacements` argument of the
`ctx.emit.finding()` function. All formatting checks must be implemented this
way, for the following reasons:

* Subprocesses run by checks are not allowed to write to files in the checkout
    directory. This prevents badly behaved tools from making unexpected changes, and
    ensures that it's safe to run multiple checks in parallel without risking race
    conditions. (Note that filesystem sandboxing is only enforced on Linux).
* Shac is designed to integrate easily with other automation that needs to
    propose the change to the user (e.g. in Gerrit) rather than automatically
    applying the change, so in order for these use cases to work the diff must be
    passed into shac rather than applied by a subprocess.

```python
import json
import sys


def main():
    # Accepts one positional argument referring to the file to format.
    path = sys.args[1]
    with open(path) as f:
        original = f.read()
    # Always use 2-space indents and a trailing blank line.
    formatted = json.dumps(json.loads(original), indent=2) + "\n"
    if formatted == original:
        sys.exit(0)
    else:
        print(json.dumps(doc, indent=2) + "\n")
        sys.exit(1)


if __name__ == "__main__":
    main()
```

```python
load("./common.star", "FORMATTER_MSG", "cipd_platform_name", "get_fuchsia_dir", "os_exec")

def json_format(ctx):
    # Launch processes in parallel.
    procs = {}
    for f in ctx.scm.affected_files():
        if not f.endswith(".json"):
            continue
        # Call fuchsia-specific `os_exec` function instead of
        # `ctx.os.exec()` to ensure proper executable resolution.
        # `os_exec` starts the subprocess but does not block.
        procs[f] = os_exec(ctx, [
            "%s/prebuilt/third_party/python3/%s/bin/python3" % (
                get_fuchsia_dir(ctx),
                cipd_platform_name(ctx),
            ),
            "scripts/shac/json_format.py",
            f,
        ])

    for f, proc in procs.items():
        # wait() blocks until the process completes.
        res = proc.wait()
        if proc.retcode != 0:
            ctx.emit.finding(
                level = "error",
                filepath = f,
                # FORMATTER_MSG is the standard message for formatters
                # in fuchsia.git.
                message = FORMATTER_MSG,
                # json_format.py prints the formatted file contents to stdout.
                # Passing it to `replacements` is necessary for shac to know
                # how to apply the fix.
                replacements = [res.stdout],
            )

# TODO: call this somewhere
shac.register_check(shac.check(
    json_format,
    # Mark the check as a formatter. Only checks with `formatter = True`
    # get run by `fx format-code`.
    formatter = True,
))

```

##### Performance optimization

Some formatters have built-in support for validating the formatting of many
files at a time, which is often parallelized internally and therefore much
faster than launching a separate subprocess to check every file. In this case,
you can run the formatter once on all files in "check" mode to get a list of
badly formatted files, and then iterate over only the badly formatted files to
get the formatted result (as opposed to iterating over all files).

Example: for [rustfmt] first run `rustfmt --check --files-with-diff
<all rust files>` to get a list of badly formatted files, then run `rustfmt`
separately on each file to get the formatted result.

If the formatter does not have a dry-run mode to print the formatted result to
`stdout`: The formatter subprocesses will not be able to write to the checkout.
However, some formatters unconditionally write files. In this case, you'll need
to copy each file into a tempdir, to which the subprocess can write, format the
temp file, and report its contents, as an example see [buildifier].

By default, `os_exec` raises an un-recoverable error if the subprocess produces
a nonzero return code. If non-zero return codes are expected, you can use the
ok_retcodes parameter, e.g. `ok_retcodes = [0, 1]` may be appropriate if the
formatter produces a return code of 1 when the file is unformatted.

### Locally running checks

To run static analysis checks locally across modified files in your change, use `fx lint`:

* `fx lint` runs all in-tree linters and analyzers on modified files.
* `fx lint --fix` automatically applies suggested replacements emitted by checks.
* `fx lint --only <check_name>` runs only a specific check.
* `fx lint --all` runs static analysis across all tracked files in the repository.

During local check development it’s recommended to test your check by running
`fx lint` or invoking shac directly via `fx host-tool shac check <file>`. Let’s create a scenario in
which we can test the `http_links` check described above:

1. Find a file that currently violates the check, or create a new one if one
    doesn't exist, eg: `echo "http://example.com" > temp.txt`
1. `fx host-tool shac check --only http_links temp.txt`
    * This should fail and print the file contents with "http://" highlighted
    * `--only` causes shac to only run the http_links check, excluding other
      checks because in this instance we only care about testing http_links and
    don't care about results from other checks
1. `fx host-tool shac fix --only http_links temp.txt` should change the http://
    to https://
1. `fx host-tool shac check --only http_links temp.txt` Should now pass
1. `fx host-tool shac check --only http_links --all`
    * Runs on all files in the tree (except git-ignored or ignored in
      `//shac.textproto`), not just changed files
    * If this fails with errors, then you'll need to fix those errors in the
      offending files either in the same commit or in a separate commit
    (preferable if there are more than ~10 files to fix) before landing your
    check.
        * Alternatively, land the check as non-blocking, fix the errors, then
          switch it to blocking
    * If your check emits warnings, note how many warnings there are. If there
      is a very large number (more than 100s) this will lead to many noisy
    Gerrit comments and may be disruptive to other contributors. Consider doing a
    bulk fix-up beforehand, reducing the scope of the check or reconsidering the
    check’s usefulness.
1. Finally, upload your check to Gerrit, run pre-submit, examine the failures
    with the goal of 0 failures. (Presubmit’s behavior is the same as running `fx
    host-tool shac check --all`)

It is recommended that you document your check if it is opt-in (not run in pre-submit) or there's a non-obvious
opt-out mechanism. All documentation should be added to `//docs/development/source_code/presubmit_checks.md`

<!-- Reference links -->

[starlark]: https://bazel.build/rules/language
[emit.findings]: https://fuchsia.googlesource.com/shac-project/shac/+/HEAD/doc/stdlib.md#ctx_emit_finding
[shac-documentation]: https://fuchsia.googlesource.com/shac-project/shac/+/refs/heads/main/doc/stdlib.md
[rustfmt]: https://cs.opensource.google/fuchsia/fuchsia/+/main:scripts/shac/rust.star
[buildifier]: https://cs.opensource.google/fuchsia/fuchsia/+/main:scripts/shac/starlark.star;l=7
