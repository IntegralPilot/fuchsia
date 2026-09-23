# Symbolizer

This is a C++ implementation of a Fuchsia log symbolizer. A symbolizer takes text logs as input,
containing unsymbolicated stack traces indicated via a special [Symbolizer markup
format](/docs/reference/kernel/symbolizer_markup.md), and symbolizes them provided debug symbols.

## E2E tests

E2E tests are disabled by default because they depend on the presence of
`//prebuilt/test_data/symbolizer/symbols`, which is not downloaded by default. This could be done by
`jiri init -fetch-optional=symbolizer-test-data && jiri fetch-packages`.

Once there are symbols, the E2E tests could be built by adding `//tools/symbolizer:e2e_tests` to
`args.gn` and executed by `fx test symbolizer_e2e_tests`.

## Log Parser Architecture

`LogParser::ProcessNextLine` processes incoming log streams using a lazy,
zero-allocation pull parser architecture:

* **Lexing (`LogTokenIterator`)**: Scans input lines into zero-allocation
  `std::string_view` tokens (`kText` and `kMarkup`) on demand via `.Next()`.
  When an opening `{{{` delimiter is followed by another `{{{` prior to a
  closing `}}}`, the earlier `{{{` fragment is classified as `kText`. This
  recovers cleanly from stray delimiters (for example,
  `random {{{ text {{{bt:0:0x1000}}}`).
* **Evaluation & Buffering**: Consumes tokens from `LogTokenIterator` in a
  single streaming pass without allocating intermediate token vectors. It
  maintains unprinted preceding text and dropped non-printing tag text
  (`pending_prefix`) before invoking active markup callbacks. Surrounding syslog
  headers (for example, `[klog] INFO: `) are discarded for lines that contain
  only valid non-printing metadata tags (`module` or `reset`) and no active
  output tags.

## Output and Tag Handling Strategy

The parser balances cleaning up noisy metadata logs with preserving raw output
and surrounding log context:

* **Valid Printing Tags (e.g., `{{{bt:...}}}`)**: The symbolizer transforms
  these tags into human-readable backtrace frames. Preceding text is prepended
  to the first emitted frame, and trailing suffix text is appended to the last
  frame. Output is buffered in a FIFO queue to maintain original stream
  ordering.

* **Valid Silent Tags (e.g., `{{{module:...}}}`, `{{{mmap:...}}}`,
  `{{{reset}}}`)**: These tags configure symbolizer state without producing
  output lines.
  * When a line contains *only* valid silent tags, both the tag text and any
    surrounding syslog prefixes (such as `[klog] INFO: `) are suppressed,
    producing zero output.
  * When a line mixes silent tags with printing or invalid tags, the silent tag
    text is omitted, but the surrounding prefix text is carried forward and
    emitted alongside the other tags.

* **Invalid or Unrecognized Markup**: If a markup tag is syntactically
  malformed or unrecognized, the raw tag text (`{{{...}}}`) is preserved
  verbatim. The parser transitions the line disposition to emit surrounding
  text, ensuring unparsed log content is printed rather than silently lost.


