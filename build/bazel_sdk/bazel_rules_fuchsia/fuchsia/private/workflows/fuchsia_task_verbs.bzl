# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# buildifier: disable=module-docstring
def _make_verb(verb = None):
    def _make(name):
        return name + "." + verb if verb else name

    return _make

# buildifier: disable=function-docstring
def make_help_executable(ctx, verbs, name = None):
    name = name or ctx.label.name
    exe = ctx.actions.declare_file(name + "_help_text.sh")
    tasks = ['echo "  - {}: {}"'.format(verb(name), help) for (verb, help) in verbs.items()]
    ctx.actions.write(
        exe,
        """
    echo "------------------------------------------------------"{default_target_invalid_str}
    echo "USAGE: To interact with this object use the following tasks:"
    {tasks}
    echo "------------------------------------------------------"
    """.format(
            default_target_invalid_str = "" if _verbs.noverb in verbs else """
echo "ERROR: The target '%s' cannot be run directly." """ % name,
            tasks = "\n".join(tasks),
        ),
        is_executable = True,
    )
    return exe

def _make_verbs(*verbs):
    return struct(
        noverb = _make_verb(),
        custom = _make_verb,
        **{
            verb: _make_verb(verb)
            for verb in verbs
        }
    )

_verbs = _make_verbs(*"""
check_now
create
debug_symbols
delete
delete_repo
download
emu
fetch
flash
help
make_default
ota
publish
reboot
remove
repo_add
start
set_channel
stop
test_enumerated
wait
zip
""".strip().split("\n"))

verbs = _verbs
