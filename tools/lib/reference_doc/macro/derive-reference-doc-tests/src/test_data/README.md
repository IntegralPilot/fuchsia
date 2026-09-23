# CML Refdoc Golden

To update `cml_refdoc.golden.md` manually:

```sh
fx build obj/tools/docsgen/cmldoc_out/index.md && fx run-in-build-dir cp obj/tools/docsgen/cmldoc_out/index.md ../../tools/lib/reference_doc/macro/derive-reference-doc-tests/src/test_data/cml_refdoc.golden.md
```
