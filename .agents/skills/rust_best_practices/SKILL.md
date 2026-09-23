---
name: rust-best-practices
description: >
  Generic best practices, patterns, and style guidelines for writing Rust code
  in Fuchsia, incorporating the official Fuchsia Rust Rubric.
---

# Rust Best Practices in Fuchsia

This guide outlines generic best practices, style conventions, and API design
rules for writing Rust code in Fuchsia. It incorporates the official **Fuchsia
Rust Rubric** (from `docs/development/api/rust.md`). For driver-specific
practices, refer to the `rust_driver_best_practices` skill.

## 1. Naming and Style Conventions

* **Casing Conforms to Rust Idioms**: Follow standard Rust naming casing. For
  example, `CamelCase` for types, `snake_case` for functions and variables, and
  `SCREAMING_SNAKE_CASE` for constants. See [C-CASE].
* **Getter Names Avoid `get_`**: Do not use the `get_` prefix for getter
  methods. Use direct field names (e.g., `rect.width()` instead of
  `rect.get_width()`). See [C-GETTER].
* **Ad-Hoc Conversions**: Ad-hoc conversions follow standard prefix/suffix
  naming conventions:
  * `as_`: Cheap, reference-to-reference conversion (e.g., `as_bytes()`).
  * `to_`: Expensive, reference-to-value/owned conversion (e.g., `to_str()`).
  * `into_`: Consuming value-to-value conversion (e.g., `into_vec()`).
  * See [C-CONV].
* **Iterator Methods & Type Names**: Methods on collections that produce
  iterators should follow `iter`, `iter_mut`, `into_iter`. Iterator type names
  should match the methods that produce them (e.g., `Iter`, `IterMut`,
  `IntoIter`). See [C-ITER] and [C-ITER-TY].
* **Consistent Word Order**: Keep word ordering consistent across APIs (e.g.
  `new_connection` and `open_connection`, rather than mixing prefixes and
  suffixes). See [C-WORD-ORDER].
* **Use Statements at Top**: All `use` statements must be grouped together at
  the top of the file. Do not place `use` statements inline within functions or
  modules. This makes file dependencies clear and easy to find.
* **Avoid Long Namespaces**: Use `use` statements to import symbols so that you
  can avoid long namespaced structs or methods in the code. This improves
  readability.
* **Descriptive Naming**: Do not use shorthand or abbreviated names for
  variables, structs, or functions. Use complete, descriptive words.
* **Avoid `allow_unused`**: Do not leave `#[allow(unused)]` or
  `#[allow(dead_code)]` in the final implementation. If a field is intentionally
  unused but must be held by a struct (like an async scope to keep it alive),
  prefix its name with an underscore (e.g., `_scope`).
* **Clean up AI-Targeted Comments**: Analyze all comments before finalizing the
  code. Remove any comments that are targeted at the AI agent itself.
* **Associated vs. Free (Top-Level) Functions**:
  * Keep a function as an associated function in the `impl` block if the type is
    important to the function's purpose (e.g., constructors returning `Self` or
    a `Result`/`Option` of `Self`).
  * Prefer associated functions when a free function's name would otherwise feel
    redundant by including the type name (e.g., favor `Thing::does_what()` over
    a free function named `thing_does_what()`).
  * Avoid overly strong preferences for free functions, as too many free
    functions create namespace noise.
  * Note that in Rust, the privacy boundary is the module (`mod`), not the type
    itself. Free functions inside the same module can access private fields of a
    struct just as easily as associated functions.
* **Use Constants, Not Magic Values**: Avoid scattering hardcoded numeric
  literals throughout the code. Use descriptive `const` definitions at the top
  of the file or module for loop counts, timeout limits, array sizes, and
  configuration values.

## 2. Interoperability and Traits

* **Eagerly Implement Common Traits**: Implement common standard library traits
  when appropriate. These include `Copy`, `Clone`, `Eq`, `PartialEq`, `Ord`,
  `PartialOrd`, `Hash`, `Debug`, `Display`, and `Default`. See
  [C-COMMON-TRAITS].
* **Standard Conversion Traits**: Prefer using standard conversion traits like
  `From`, `AsRef`, and `AsMut` rather than custom methods where possible.
  Implementing `From<A> for B` automatically gives you `Into<B> for A`. See
  [C-CONV-TRAITS].
* **Collections and Iterators**: Custom collections should implement
  `FromIterator` and `Extend` to interact naturally with standard Rust iterators
  and `.collect()`. See [C-COLLECT].
* **Data Serialization**: Data structures representing external interfaces or
  configurations should implement Serde's `Serialize` and `Deserialize` traits.
  See [C-SERDE].
* **Concurrency Safety**: Types should be `Send` and `Sync` where possible.
  Ensure your design doesn't inadvertently block these traits unless thread
  unsafety is intended. See [C-SEND-SYNC].
* **Meaningful Error Types**: Error types should be meaningful, well-behaved,
  and implement `std::error::Error`. Avoid returning raw strings or generic
  error types unless prototyping. See [C-GOOD-ERR].
* **Binary Number Formatting**: If representing custom binary numbers or bit
  flags, provide formatting implementations for `Hex`, `Octal`, and `Binary`.
  See [C-NUM-FMT].
* **Generic I/O Signatures**: Generic reader/writer functions should take `R:
  Read` and `W: Write` by value, not by reference, to allow callers to pass
  owned readers/writers or standard wrappers. See [C-RW-VALUE].

## 3. API and Type Design

* **Smart Pointers**: Avoid adding inherent methods to smart pointers. All
  methods on a smart pointer should be accessed via trait implementations or
  associated functions to avoid shadowing methods on the inner type. See
  [C-SMART-PTR].
* **Conversion Location**: Conversions live on the most specific type involved.
  See [C-CONV-SPECIFIC].
* **Clear Receivers**: Functions with a clear primary receiver should be
  designed as methods taking `self`, `&self`, or `&mut self`. See [C-METHOD].
* **No Out-Parameters**: Functions should return their results directly (as
  single values, tuples, or structs) rather than using mutable out-parameters.
  See [C-NO-OUT].
* **Unsurprising Operator Overloads**: Overload operators only when the behavior
  is completely expected and matches standard conventions (e.g., overriding
  `Add` for a custom complex number type). See [C-OVERLOAD].
* **`Deref` & `DerefMut`**: Implement `Deref` and `DerefMut` ONLY for smart
  pointers. Do not use them for inheritance or code reuse. See [C-DEREF].
* **Constructors**: Constructors should be static, inherent methods (typically
  called `new` or `with_...`). See [C-CTOR].
* **Flexibility and Performance**:
  * **Expose Intermediate Results**: Expose intermediate results in your API
    where they can prevent duplicate work for callers. See [C-INTERMEDIATE].
  * **Caller Decides Allocation**: Let the caller decide where to copy and place
    data (e.g., pass owned arguments or `Cow` rather than cloning internally).
    See [C-CALLER-CONTROL].
  * **Generic Parameters**: Minimize assumptions about parameters by using
    generics (e.g., `impl AsRef<str>` instead of `&str` if appropriate). See
    [C-GENERIC].
  * **Object Safety**: Design traits to be object-safe if they may be useful as
    a trait object. See [C-OBJECT].
* **Type Safety and Encapsulation**:
  * **Newtypes for Static Distinction**: Use the newtype pattern to provide
    static type distinctions (e.g., `struct UserId(u64)` rather than a plain
    `u64`). See [C-NEWTYPE].
  * **Arguments Convey Meaning**: Use custom types or enums for arguments to
    convey precise meaning, instead of using boolean flag parameters or nested
    `Option` types. See [C-CUSTOM-TYPE].
  * **Sets of Flags**: Use the `bitflags` crate to represent a set of flags, not
    enums or raw integers. See [C-BITFLAG].
  * **Builders for Complex Values**: Use the builder pattern to construct
    complex values where there are many optional parameters. See [C-BUILDER].
  * **Sealed Traits**: Use sealed traits if you want to define a trait but
    prevent downstream crates from implementing it. See [C-SEALED].
  * **Private Fields**: Struct fields should be private by default to
    encapsulate implementation details. See [C-STRUCT-PRIVATE].
  * **Encapsulation**: Use newtypes to encapsulate internal implementation
    details of complex types. See [C-NEWTYPE-HIDE].
  * **Derived Bounds**: Do not duplicate derived trait bounds on struct
    definitions; place them on the `impl` blocks instead. See [C-STRUCT-BOUNDS].

## 4. Concurrency and Async Patterns

* **`spawn` vs `spawn_local`**:
  * Use `scope.spawn()` as the default when the task implements `Send`.
  * Use `scope.spawn_local()` when the task does not implement `Send` (e.g., it
    holds non-thread-safe types like `Rc` or `RefCell`).
* **Do Not Detach Tasks**: Never use
  `fuchsia_async::Task::spawn(...).detach();`. Detaching tasks is considered bad
  style and can lead to resource leaks or silent task failures. Use
  `scope.spawn(...);` instead.
* **Mutex Preference**: When a mutex is strictly necessary, prefer using
  `fuchsia_sync::Mutex` over `std::sync::Mutex`.

## 5. Macros

* **Evocative Input**: The input syntax of a macro should be evocative of the
  output it produces. See [C-EVOCATIVE].
* **Attribute Composition**: Macros should compose well with attributes,
  ensuring that doc-comments and other attributes can be attached directly to
  generated items. See [C-MACRO-ATTR].
* **Item Scope**: Item macros should work anywhere that items are allowed (e.g.,
  in modules, functions). See [C-ANYWHERE].
* **Visibility**: Item macros should support standard visibility specifiers
  (like `pub`, `pub(crate)`). See [C-MACRO-VIS].
* **Type Flexibility**: Macro type fragments should be flexible and allow for
  standard Rust types and lifetime bounds. See [C-MACRO-TY].

## 6. Documentation & Rustdoc Guidelines

* **Crate-Level Docs**: Crate-level documentation must be thorough and include
  examples of how to use the crate. See [C-CRATE-DOC].
* **Item Examples**: All public items should have a rustdoc example illustrating
  their usage. See [C-EXAMPLE].
  > [!NOTE]
  > This guideline is not strictly enforced for targets building on Fuchsia until doctests are supported on Fuchsia targets.
* **Ergonomic Examples**: Examples should use the `?` operator, not `try!`, and
  not call `unwrap` or `expect`. See [C-QUESTION-MARK].
* **Function Failure and Safety**: Function docs must clearly document:
  * Error conditions under which the function returns `Err`.
  * Panic conditions (under what inputs or states the function will panic).
  * Safety considerations (preconditions required for `unsafe` functions). See
    [C-FAILURE].
* **Prose Hyperlinks**: Include hyperlinks to relevant types, methods, and
  concepts in rustdoc prose. See [C-LINK].
* **Hide Implementation Details**: Rustdoc should not show unhelpful internal
  implementation details. See [C-HIDDEN].

## 7. Safety and Unsafe Code Rules

* **Error Handling**: Avoid `unwrap()` and `expect()` which can panic the
  process. Use `ok_or_else()` or handle errors explicitly. If an error is truly
  unrecoverable, provide a context string explaining why it is safe to panic.
* **Arithmetic**: Always use checked math operations (e.g., `checked_add()`,
  `checked_mul()`) on external or untrusted input to prevent overflows.
* **Fuchsia Unsafe Guidelines**:
  
  > [!IMPORTANT]
  > **Every `unsafe` block must have an accompanying justification.**
  > Safety justifications should begin with `// SAFETY: ` and explain why the unsafe block is sound.

  * **Format Requirements**:
    * Must use `// SAFETY: <why this unsafe operation is sound>`
    * Do **NOT** use other formats like `// Safety:`, `// [SAFETY]`, or lazy
      placeholders like `// SAFETY: Trust me.`
  * **Soundness Explanation**: Explain why the unsafe block is necessary and why
    the code inside is sound. If there are safe alternatives that appear
    suitable but cannot be used, document why they are not viable (e.g.,
    performance).
  
  **Do:**
  ```rust
  // SAFETY: The `bytes` returned from our string builder are guaranteed to be
  // valid UTF-8. We used to call `from_utf8`, but this caused performance issues
  // with large inputs.
  let s = unsafe { String::from_utf8_unchecked(bytes) };
  ```

  **Don't:**
  ```rust
  // SAFETY: We shouldn't have to validate `bytes`, and the safe version is slow.
  let s = unsafe { String::from_utf8_unchecked(bytes) };
  ```

  * **Unsafe Traits & Impls**:
    * Unsafe traits must document safety considerations.
    * Unsafe trait implementations must be justified using the `// SAFETY: `
      comment pattern.
  
  * **Always use `unsafe` Blocks in `unsafe` Functions**:
    * `unsafe` functions are not considered unsafe contexts in Fuchsia. Unsafe
      operations must always be located inside an explicit `unsafe` block, even
      if they are in an `unsafe` function body.
  
  **Do:**
  ```rust
  unsafe fn clear_slice(ptr: *mut i32, len: usize) {
      assert!(len.checked_mul(mem::size_of::<i32>()).unwrap() < isize::MAX);

      // SAFETY:
      // - The caller has guaranteed that `ptr` points to `len` consecutive, valid
      //   i32s and that the data behind `ptr` is not simultaneously accessed.
      // - We asserted that the total size of the slice is less than isize::MAX.
      let slice = unsafe { slice::from_raw_parts_mut(ptr, len) };
      for x in slice.iter_mut() {
          *x = 0;
      }
  }
  ```

## 8. Dependability and Debuggability

* **Argument Validation**: Functions validate their arguments and return early
  with an error or panic on invalid inputs before corrupting state. See
  [C-VALIDATE].
* **Destructors Never Fail**: Destructors (`Drop` implementations) should never
  fail or panic. See [C-DTOR-FAIL].
* **Non-blocking Destructors**: Destructors that may block the thread must have
  non-blocking alternatives. See [C-DTOR-BLOCK].
* **Public Types Debuggability**: All public types must implement `Debug`. See
  [C-DEBUG].
* **Non-empty Debug Representation**: The `Debug` representation should never be
  empty. See [C-DEBUG-NONEMPTY].

## 9. Testing Idioms

* **Test Macro**: Use `#[fuchsia::test]` instead of `#[test]`. It natively
  supports async tests and logging out of the box.
* **Ergonomic Testing**: Utilize the `assert_matches` crate for pattern
  assertions and `pretty_assertions` for colored diffs on failure.
* **GN Configuration**: Add `with_unit_tests = true` to your `rustc_binary` or
  `rustc_library` templates to generate test targets.

## 10. Framework and Build Rules

* **Target Edition**: Set `edition = "2024"` for new `BUILD.gn` Rust targets.
* **Deprecated Crates**: Do not use the `fuchsia_zircon` crate. Use the standard
  `zx` bindings or relevant crates.
* **Handle Methods**: Do not include `zx::AsHandleRef` to call methods on
  handles. It is no longer needed.
* **Lazy Initialization**: Do not use the `lazy_static!` macro. Use
  `std::sync::LazyLock` from the standard library instead.

## 11. Reviewing Changes

Upon finishing authoring a change to Rust code, you MUST use this skill to
review the change. Verify that:
- [ ] All `use` statements are grouped at the top of the file.
- [ ] No shorthand or abbreviated names are used for variables or structs.
- [ ] No `#[allow(unused)]` or `#[allow(dead_code)]` attributes remain in the
  final code.
- [ ] Comments are targeted at human maintainers, not the AI agent.
- [ ] Constants are used instead of magic values.
- [ ] `spawn` vs `spawn_local` is correctly chosen based on `Send` requirements.
- [ ] `fuchsia_sync::Mutex` is preferred if a mutex is strictly necessary.
- [ ] `#[fuchsia::test]` is used for tests.
- [ ] `unwrap()` and `expect()` are avoided where possible.
- [ ] **Every `unsafe` block has a `// SAFETY: ` comment** that details exactly
  why it is sound and necessary.
- [ ] Unsafe operations within `unsafe` functions are wrapped in an explicit
  `unsafe` block.
- [ ] Public types implement `Debug` and getter methods avoid `get_`.
- [ ] Checked math is used on external or untrusted input.

---

### References to Upstream Rust API Guidelines

[C-CASE]: https://rust-lang.github.io/api-guidelines/naming.html#c-case
[C-CONV]: https://rust-lang.github.io/api-guidelines/naming.html#c-conv
[C-GETTER]: https://rust-lang.github.io/api-guidelines/naming.html#c-getter
[C-ITER]: https://rust-lang.github.io/api-guidelines/naming.html#c-iter
[C-ITER-TY]: https://rust-lang.github.io/api-guidelines/naming.html#c-iter-ty
[C-WORD-ORDER]:
https://rust-lang.github.io/api-guidelines/naming.html#c-word-order
[C-COMMON-TRAITS]:
https://rust-lang.github.io/api-guidelines/interoperability.html#c-common-traits
[C-CONV-TRAITS]:
https://rust-lang.github.io/api-guidelines/interoperability.html#c-conv-traits
[C-COLLECT]:
https://rust-lang.github.io/api-guidelines/interoperability.html#c-collect
[C-SERDE]:
https://rust-lang.github.io/api-guidelines/interoperability.html#c-serde
[C-SEND-SYNC]:
https://rust-lang.github.io/api-guidelines/interoperability.html#c-send-sync
[C-GOOD-ERR]:
https://rust-lang.github.io/api-guidelines/interoperability.html#c-good-err
[C-NUM-FMT]:
https://rust-lang.github.io/api-guidelines/interoperability.html#c-num-fmt
[C-RW-VALUE]:
https://rust-lang.github.io/api-guidelines/interoperability.html#c-rw-value
[C-SMART-PTR]:
https://rust-lang.github.io/api-guidelines/predictability.html#c-smart-ptr
[C-CONV-SPECIFIC]:
https://rust-lang.github.io/api-guidelines/predictability.html#c-conv-specific
[C-METHOD]:
https://rust-lang.github.io/api-guidelines/predictability.html#c-method
[C-NO-OUT]:
https://rust-lang.github.io/api-guidelines/predictability.html#c-no-out
[C-OVERLOAD]:
https://rust-lang.github.io/api-guidelines/predictability.html#c-overload
[C-DEREF]:
https://rust-lang.github.io/api-guidelines/predictability.html#c-deref [C-CTOR]:
https://rust-lang.github.io/api-guidelines/predictability.html#c-ctor
[C-INTERMEDIATE]:
https://rust-lang.github.io/api-guidelines/flexibility.html#c-intermediate
[C-CALLER-CONTROL]:
https://rust-lang.github.io/api-guidelines/flexibility.html#c-caller-control
[C-GENERIC]:
https://rust-lang.github.io/api-guidelines/flexibility.html#c-generic
[C-OBJECT]: https://rust-lang.github.io/api-guidelines/flexibility.html#c-object
[C-NEWTYPE]:
https://rust-lang.github.io/api-guidelines/type-safety.html#c-newtype
[C-CUSTOM-TYPE]:
https://rust-lang.github.io/api-guidelines/type-safety.html#c-custom-type
[C-BITFLAG]:
https://rust-lang.github.io/api-guidelines/type-safety.html#c-bitflag
[C-BUILDER]:
https://rust-lang.github.io/api-guidelines/type-safety.html#c-builder
[C-VALIDATE]:
https://rust-lang.github.io/api-guidelines/dependability.html#c-validate
[C-DTOR-FAIL]:
https://rust-lang.github.io/api-guidelines/dependability.html#c-dtor-fail
[C-DTOR-BLOCK]:
https://rust-lang.github.io/api-guidelines/dependability.html#c-dtor-block
[C-DEBUG]: https://rust-lang.github.io/api-guidelines/debuggability.html#c-debug
[C-DEBUG-NONEMPTY]:
https://rust-lang.github.io/api-guidelines/debuggability.html#c-debug-nonempty
[C-SEALED]:
https://rust-lang.github.io/api-guidelines/future-proofing.html#c-sealed
[C-STRUCT-PRIVATE]:
https://rust-lang.github.io/api-guidelines/future-proofing.html#c-struct-private
[C-NEWTYPE-HIDE]:
https://rust-lang.github.io/api-guidelines/future-proofing.html#c-newtype-hide
[C-STRUCT-BOUNDS]:
https://rust-lang.github.io/api-guidelines/future-proofing.html#c-struct-bounds
[C-EVOCATIVE]:
https://rust-lang.github.io/api-guidelines/macros.html#c-evocative
[C-MACRO-ATTR]:
https://rust-lang.github.io/api-guidelines/macros.html#c-macro-attr
[C-ANYWHERE]: https://rust-lang.github.io/api-guidelines/macros.html#c-anywhere
[C-MACRO-VIS]:
https://rust-lang.github.io/api-guidelines/macros.html#c-macro-vis [C-MACRO-TY]:
https://rust-lang.github.io/api-guidelines/macros.html#c-macro-ty [C-CRATE-DOC]:
https://rust-lang.github.io/api-guidelines/documentation.html#c-crate-doc
[C-EXAMPLE]:
https://rust-lang.github.io/api-guidelines/documentation.html#c-example
[C-QUESTION-MARK]:
https://rust-lang.github.io/api-guidelines/documentation.html#c-question-mark
[C-FAILURE]:
https://rust-lang.github.io/api-guidelines/documentation.html#c-failure
[C-LINK]: https://rust-lang.github.io/api-guidelines/documentation.html#c-link
[C-HIDDEN]:
https://rust-lang.github.io/api-guidelines/documentation.html#c-hidden
