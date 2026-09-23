# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# The blocklist is all mirrors that have licensing concerns
# that Fuchsia does not want in its' checkout.
blocklist = [
    "fuchsia.googlesource.com/third_party/github.com/vim/vim",
    "fuchsia.googlesource.com/third_party/github.com/GNOME/glib",
    "fuchsia.googlesource.com/third_party/github.com/PCRE2Project/pcre2",
    "fuchsia.googlesource.com/third_party/github.com/GNOME/gvdb",
    "fuchsia.googlesource.com/third_party/github.com/tensorflow/tensorflow",
]

def blocklist_mirrors(ctx):
    """Checks if mirror is in blocklist and errors if so.

    Args:
      ctx: A ctx instance.
    """
    affected_files = ctx.scm.affected_files()
    if not affected_files:
        return

    for f in affected_files:
        if f == "scripts/shac/mirror_blocklists.star":
            continue
        contents = ctx.io.read_file(ctx.scm.root + "/" + f)
        if contents == None:
            continue

        normalized_contents = str(contents).replace("https://", "").replace("http://", "")
        normalized_contents = normalized_contents.rstrip("/")

        for url in blocklist:
            if url in normalized_contents:
                ctx.emit.finding(
                    level = "error",
                    message = "File contains a blocklisted mirror URL: %s" % url,
                    filepath = f,
                )

def register_mirror_blocklists_checks():
    shac.register_check(shac.check(blocklist_mirrors))
