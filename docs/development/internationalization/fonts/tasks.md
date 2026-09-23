# Common font maintenance tasks

[Fonts][fnt] are stored on device, on behalf of all apps. Font updates are
occasionally needed, for example to allow apps to render the latest emojis.

This is a registry of step-by-step instructions for font maintenance tasks. Some
of the tasks require manual intervention, while some are automated. Some of the
manual tasks will be automated eventually, while others will likely always
require a human to approve.

## How do I update existing font files? {#eff}

You need to update the [`//fontdata`][fnd] repository's [JIRI manifest][fjm]
with the latest commit IDs of the font files. You will then need to shepherd
the update all the way to Fuchsia products. At the moment, this update is
manual.

For full detail, consult [build time configuration][btc].

## How do I get new glyphs added to font files?

Note: This is a Google-internal task only.

Make sure that the new glyphs are necessary. For products, this may involve
asking the person responsible for the product.

If the glyphs are not necessary, you can stop here.

If the glyphs are necessary, you have work to do. For "universal" fonts such as
Noto or Roboto, you may need to order new glyphs to be made if they are
missing.  See `b/213345773` for an example.

Once the glyphs are available, see [above](#eff) to roll the update through.

[btc]: /docs/development/internationalization/fonts/build.md
[fnt]: /docs/development/internationalization/fonts/README.md#glossary
[fnd]: https://fuchsia.googlesource.com/fontdata
[fjm]: https://fuchsia.googlesource.com/fontdata/+/refs/heads/main/manifest.xml
