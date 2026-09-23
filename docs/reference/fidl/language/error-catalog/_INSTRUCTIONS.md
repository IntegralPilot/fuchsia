# README

This directory is a catalog of all fidlc errors. Each error has a Markdown file
that describes it with "good" and "bad" example code.

## Adding a new error

Run this script, then edit the TODOs in the new files it creates:

    $FUCHSIA_DIR/tools/fidl/scripts/add_errcat_entry.py {ERROR_NUMBER} \
        --bad={NUMBER_OF_BAD_EXAMPLES} \
        --good={NUMBER_OF_GOOD_EXAMPLES}

### Manually adding a new error

Follow these steps if the script doesn't work:

- Create `//docs/reference/fidl/language/error-catalog/_fi-NNNN.md`, replacing
  the `NNNN` with the new error code.

- Add the new error to the end of `//docs/reference/fidl/language/_files.txt`:

  ```none
  _fi-NNNN.md
  ```

- Add the new error to the end of `//docs/reference/fidl/language/errcat.md`:

  ```none
  <<error-catalog/_fi-NNNN.md>>
  ```

- Add a new entry to the end of `//docs/error/_redirects.yaml`:

      - from: /fuchsia-src/error/fi-NNNN
        to: /fuchsia-src/reference/fidl/language/errcat.md#fi-NNNN

- Create at least one good example in `//tools/fidl/fidlc/tests/fidl/good`.
  If you're creating only one, name it `fi-NNNN.test.fidl`. Otherwise, name
  them `fi-NNNN-a.test.fidl`, `fi-NNNN-b.test.fidl`, etc.

- Create at least one bad example in `//tools/fidl/fidlc/tests/fidl/bad`.
  If you're creating only one, name it `fi-NNNN.test.fidl`. Otherwise, name
  them `fi-NNNN-a.test.fidl`, `fi-NNNN-b.test.fidl`, etc.

- Add a test case to `//tools/fidl/fidlc/tests/errcat_good_tests.cc` using the
  "good" example.

- Add or update an existing test case using the "bad" example. A good way to
  find existing test cases is to grep for the error name. If there are no
  existing test cases, add a new one somewhere in `//tools/fidl/fidlc/tests`.

## Retiring an error

- Delete good and bad examples:

      rm tools/fidl/fidlc/tests/fidl/{good,bad}/fi-NNNN*.test.fidl

- Update `//docs/reference/fidl/language/error-catalog/_fi-NNNN.md`:

      ## fi-NNNN {:#fi-NNNN .hide-from-toc}

      Deprecated: This error code has been retired.

- Change `//tools/fidl/fidlc/src/diagnostics.h` to define the error
  using `RetiredDef` instead of `ErrorDef`. For example:

      -constexpr ErrorDef<10, std::string_view> ErrInvalidIdentifier("invalid identifier '{}'");
      +constexpr RetiredDef<10> ErrInvalidIdentifier;

<!-- doc-checker: ignore-unused -->
