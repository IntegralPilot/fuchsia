# Simplest app flatland app

This is a simplest app to handle touch input.

It can be run in session:

```sh
ffx session start
ffx session add fuchsia-pkg://fuchsia.com/flatland-examples#meta/simplest-app-flatland-session.cm
```

`simplest-app-flatland-session.cml` will pass `view_provider` argument to the
app. Then the app will setup `fuchsia.ui.app.ViewProvider` service, and ignore
`fuchsia.element.GraphicalPresenter` setup.

It can be also run with component:

```sh
ffx component run <NAME> fuchsia-pkg://fuchsia.com/flatland-examples#meta/simplest-app-flatland.cm
```

`simplest-app-flatland.cml` will pass `graphical_presenter` argument to the
app.
