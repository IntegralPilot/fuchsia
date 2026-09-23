# Best Practices for `config_schema`

## Defaults

The most common use-case should require the least amount of typing. This requires our configuration to have reasonable default values.

Suppose that you have a subsystem that groups a set of functionality together, and you want to nest it under the top-level `PlatformConfig`.

```
struct PlatformConfig {
  ...
  #[serde(default)]
  my_subsystem: MySubsystem,
}
```

Provide a `Default` implementation for `MySubsystem` if possible and use `#[serde(default)]` in the `PlatformConfig` to populate the struct if the entire subsystem is omitted.

Prefer `#[derive(Default)]`  because it is simpler, and use the container-level attribute `#[serde(default)]` to allow the user to supply only the fields they care about, and let serde fill in the rest.
```
#[derive(Default)]
#[serde(default)]
struct MySubsystem {
    enable_feature1: bool,
    enable_feature2: bool,
}
```

If a specific field needs a specialized default -- such as `enable_feature1` needing a default value of `true` -- avoid using `#[derive(Default)]` with `#[serde(default = "default_true")]`. This is a common source of bugs. See below for an example:

```
#[derive(Default)]
struct MySubsystem {
    #[serde(default = "default_true")]
    enable_feature1: bool,
    enable_feature2: bool,
}

fn default_true -> bool {
    true
}
```

If the user provides a fully empty platform config `{}`,  `enable_feature1` will be populated using the `#[derive(Default)]` which defaults to `false`.

If the user instead provides `{ "my_subsystem": {} }`, `enable_feature1` will be populated using `default_true()` which returns `true`.

Use `impl Default` to keep the deserialization consistent no matter what the user provides.
```
impl Default for MySubsystem {
    fn default() -> Self {
	    Self {
		    enable_feature1: true,
		    enable_feature2: false,
		}
	}
}
```

## Use `Option<bool>` sparingly

Tri-state fields are often mis-interpreted and should only be used when their `None` value has an obvious meaning.

```
#[derive(Default)]
struct MySubsystem {
    enable_feature: Option<bool>,
}
```

In the above example, it is not clear what the difference is between:
* `enable_feature = None`
* `enable_feature = Some(false)`

`enable_feature` could be changed to a `bool` without losing functionality.

One use-case that requires `Option<bool>` is if the default value of the field should change depending on other configuration values, such as the build type:
```
let enable_feature = match (build_type, my_subsystem.enable_feature) {
    // If nothing is provided, the default value is determined
    // based on the build type.
    (Eng, None) => true,
    (UserDebug | User, None) => false,

	// Otherwise, we use the value provided by the user.
	(_, Some(enabled)) => enabled,
};
```

