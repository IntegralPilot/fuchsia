# Future work

## Localization

For information about the future work related to localization, see
[Future work](./localization/future_work.md).

## Internationalization preferences

-   Migrate `fuchsia.intl.PropertyProvider` change watcher API to a
    [hanging get](/docs/development/api/fidl.md#hanging-get) design and migrate all
    existing clients.

-   Instead of having a single `fuchsia.intl.PropertyProvider` implemented by
    the `intl` component, demonstrate best practices for embedding custom
    `PropertyProvider`s in [session component](/docs/glossary#session-component)
    implementations, ideally with multi-user use cases.

-   Wire up Dart's `Platform.localeName` (blocked by Dart SDK
    [#37586](https://github.com/dart-lang/sdk/issues/37586)).

-   Wire up and verify Chromium's
    [`navigator.language` and `navigator.languages`][navigator-languages].

## Fonts

-   Use ICU4X to implement real language and script ID matching in font service.

<!--xrefs-->

[navigator-languages]: https://developer.mozilla.org/en-US/docs/Web/API/NavigatorLanguage/languages
