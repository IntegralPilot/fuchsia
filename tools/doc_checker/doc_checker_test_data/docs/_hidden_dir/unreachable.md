# Unreachable file in hidden directory

This file sits inside a directory whose name starts with an underscore (`_hidden_dir`). It does not appear in `_toc.yaml`, but `doc_checker` exempts it from the reachability check because it resides under a hidden directory.

It contains a valid link to [README][readme] to ensure link checking passes.

[readme]: ../README.md
