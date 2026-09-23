// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;
use quote::{ToTokens, quote, quote_spanned};
use syn::spanned::Spanned;

/// Instantiates an impl block as separate Ipv4 and Ipv6 blocks.
///
/// This macro covers the lack of stable specialization of trait implementations
/// in Rust with the addition that the [`net_types::ip::Ip`] macro has a known
/// and closed set of implementers.
///
/// It is not uncommon that we need to specialize some implementations over the
/// IP version because of significant differences in implementations. Dependents
/// of the specific split implementations have to often split the implementation
/// too, even if the implementation block itself would look identical.
///
/// Until specialization is stabilized, this macro provides a way out of this
/// bind. It should be noted, however, that if an implementation _can_ be
/// written generically then it _should_. Facilities such as
/// [`net_types::ip::GenericOverIp`] make that possible in many cases but,
/// notably, associated types remain an unsolved problem.
///
/// `instantiate_ip_impl_block` takes a single identifier argument for the type
/// in the Generics `impl` block that will take the `Ipv4` and `Ipv6` forms in
/// the duplicated emitted blocks.
///
/// Example:
///
/// ```rust
/// struct Dependency<I>(I::Addr);
///
/// impl Dependency<Ipv4> { fn execute(&self) { /* ... */} }
/// impl Dependency<Ipv6> { fn execute(&self) { /* ... */} }
///
/// trait TraitOverIp<I: Ip> {
///     fn do_something(&self);
/// }
///
/// struct Impl;
///
/// #[instantiate_ip_impl_block(I)]
/// impl<I> TraitOverIp<I> for Impl {
///     fn do_something(&self) {
///         // instantiate_ip_impl_block rewrites this impl block instantiating
///         // I with Ipv4 and Ipv6 each time, allowing us to use dependencies
///         // that might not have complete implementations over generic I: Ip.
///         let x : Dependency<I> = Default::default();
///         x.execute();
///     }
/// }
/// ```
///
/// # Limitations and caveats
///
/// 1. If and when specialization meets our goals for generic IP implementations
///    that can be specialized, we expect to be able to delete this macro.
/// 1. This being a proc macro, it is *not* capable of replacing tokens inside a
///    declarative macro.
///
#[proc_macro_attribute]
pub fn instantiate_ip_impl_block(attr: TokenStream, input: TokenStream) -> TokenStream {
    let ip_ident = syn::parse_macro_input!(attr as syn::Ident);
    let mut item = syn::parse_macro_input!(input as syn::ItemImpl);

    let params = item.generics.params.clone();
    // Clear all params and put back in the ones not referencing the IP
    // identifier.
    item.generics.params.clear();
    for param in params {
        let pass = match &param {
            syn::GenericParam::Type(param) => param.ident != ip_ident,
            _ => true,
        };
        if pass {
            item.generics.params.push(param);
        }
    }

    struct IdentifierReplacementVisitor {
        search: syn::Ident,
        replace: syn::Path,
    }

    impl syn::visit_mut::VisitMut for IdentifierReplacementVisitor {
        fn visit_path_mut(&mut self, i: &mut syn::Path) {
            let mut iter = i.segments.iter();

            // We're always replacing a single identifier, so should always be
            // the first segment of the path.
            if iter
                .next()
                .map(|syn::PathSegment { ident, arguments }| {
                    // Look for our identifier and only accept empty path
                    // arguments, we don't expect parens or angle brackets
                    // arguments on our identifier.
                    ident == &self.search && arguments.is_empty()
                })
                .unwrap_or(false)
            {
                // Build a new path, replacing the beginning with our
                // replacement path and extending with the tail of the path.
                let mut new_segments = self.replace.segments.clone();
                new_segments.extend(iter.cloned());
                i.segments = new_segments;
            } else {
                drop(iter);
            }
            syn::visit_mut::visit_path_mut(self, i)
        }
    }

    // Now simply duplicate the definition and walk the AST twice, renaming
    // everything matching the identifier.
    let mut v4 = item.clone();
    let mut v6 = item;

    syn::visit_mut::visit_item_impl_mut(
        &mut IdentifierReplacementVisitor {
            search: ip_ident.clone(),
            replace: syn::parse_quote!(net_types::ip::Ipv4),
        },
        &mut v4,
    );
    syn::visit_mut::visit_item_impl_mut(
        &mut IdentifierReplacementVisitor {
            search: ip_ident,
            replace: syn::parse_quote!(net_types::ip::Ipv6),
        },
        &mut v6,
    );

    quote! { #v4 #v6 }.into()
}

struct IpBoundsArgs {
    ip_ident: syn::Path,
    bindings_ctx: syn::Path,
    ns3_core: syn::Path,
}

impl syn::parse::Parse for IpBoundsArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let args =
            syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated(input)?;
        let args_span = args.span();
        let mut args = args.into_iter();
        let ip_ident = args
            .next()
            .ok_or_else(|| syn::Error::new(args_span, "missing IP identifier argument"))?;
        let bindings_ctx = args
            .next()
            .ok_or_else(|| syn::Error::new(args_span, "missing bindings context argument"))?;

        // If a third argument is not provided, default to `netstack3_core`.
        let ns3_core = args.next().unwrap_or_else(|| syn::parse_quote! { netstack3_core });
        Ok(Self { ip_ident, bindings_ctx, ns3_core })
    }
}

fn context_ip_bounds_inner(attr: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let IpBoundsArgs { ip_ident, bindings_ctx, ns3_core } = syn::parse::<IpBoundsArgs>(attr)?;
    let mut item = syn::parse::<syn::Item>(input)?;
    let generics = match &mut item {
        syn::Item::Impl(item_impl) => &mut item_impl.generics,
        syn::Item::Fn(item_fn) => &mut item_fn.sig.generics,
        o => return Err(syn::Error::new_spanned(o, "not supported for this input"))?,
    };
    let where_clause = generics.make_where_clause();
    where_clause.predicates.push(syn::parse_quote! {
        for<'macro_lifetime> #ns3_core::UnlockedCoreCtx<'macro_lifetime, #bindings_ctx>:
            #ns3_core::CoreContext< #ip_ident, #bindings_ctx >
    });
    where_clause.predicates.push(syn::parse_quote! {
        #bindings_ctx : #ns3_core::IpBindingsContext< #ip_ident >
    });

    Ok(item.into_token_stream().into())
}

/// Generates common bounds for `netstack3_core::UnlockedCoreCtx`.
///
/// This macro is used to emit a common `where` clause when writing code that is
/// generic over IP version that wants to call into netstack3 core.
///
/// It takes up to three arguments:
///
/// ```
/// #[context_ip_bounds(IpIdentifier, BindingsCtx [,netstack3_core]]
/// ```
///
/// * `IpIdentifier` is the generic IP type for the targeted item. Required.
/// * `BindingsCtx` is the bindings context implementation in use. Required.
/// * `netstack3_core` is the path to the root of the netstack3 core crate.
///   Optional, assumed to be `netstack3_core` if omitted.
///
/// Example:
///
/// ```
/// #[context_ip_bounds(I, FooBindingsCtx)]
/// impl<I: netstack3_core::IpExt> Foo {}
/// ```
///
/// Expands to:
///
/// ```
/// impl<I: netstack3_core::IpExt> Foo
/// where
///     for<'a> netstack3_core::UnlockedCoreCtx<'a, FooBindingsCtx>:
///             netstack3_core::CoreContext<I, FooBindingsCtx>,
///     FooBindingsCtx: netstack3_core::IpBindingsContext<I> {}
/// ```
#[proc_macro_attribute]
pub fn context_ip_bounds(attr: TokenStream, input: TokenStream) -> TokenStream {
    match context_ip_bounds_inner(attr, input) {
        Ok(t) => t,
        Err(e) => e.into_compile_error().into(),
    }
}

// Substitutes the given identifier with an arbitrary replacement.
fn substitute_ident(
    input: impl quote::ToTokens,
    ident: &str,
    replacement: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    input
        .to_token_stream()
        .into_iter()
        .map(|t| {
            if t.to_string().as_str() == ident {
                replacement.clone()
            } else {
                proc_macro2::TokenStream::from(t)
            }
        })
        .collect()
}

/// A derive macro for the `netstack3_base::CounterCollection` and
/// `netstack3_base::CounterCollectionSpec` traits.
///
/// For a given input definition:
///
/// ```
///   #[derive(CounterCollection)]
///   #[counter(C)]
///   struct Foo<B: Bar, C: netstack3_base::CounterRepr>
/// ```
///
/// it produces the following implementations:
///
/// ```
///   impl<B: Bar, C: netstack3_base::CounterRepr> netstack3_base::CounterCollection for Foo<B, C> {
///      type Spec = Foo<B, netstack3_base::Counter>;
///      type Repr = C;
///   }
///
///   impl<B: Bar> netstack3_base::CounterCollectionSpec for Foo<B, netstack3_base::Counter> {
///       type CounterCollection<C: netstack3_base::CounterRepr> = Foo<B, C>;
///       fn transform<C1: netstack3_base::CounterRepr, C2: netstack3_base::CounterRepr>(
///           counters = &Foo<B, C1>
///       ) -> Foo<B, C2> {
///           let Foo {
///               field1,
///               field2,
///               <remaining-fields>
///           } = counters;
///           Foo {
///               field1: field1.cast::<C2>(),
///               field2: field1.cast::<C2>(),
///               <remaining-fields>
///           }
///       }
///   }
/// ```
///
/// The `#[counter(<TYPE_PARAM>)]` attribute specifies which type parameter is
/// the counter type parameter. If unset it defaults to `C`.
///
/// Every field in `Foo` is expected to implement either
/// `netstack3_base::CounterRepr` or `netstack3_base::CounterCollection`.
#[proc_macro_derive(CounterCollection, attributes(counter))]
pub fn derive_counter_collection_spec(item: TokenStream) -> TokenStream {
    let ast = syn::parse_macro_input!(item as syn::DeriveInput);
    let name = ast.ident;

    // Determine which type parameter is the special "counter" type param.
    const COUNTER_TYPE_PARAM_ATTRIBUTE: &str = "counter";
    let counter_attribute = ast.attrs.iter().find(|attr| {
        attr.path().get_ident().map(|ident| ident == COUNTER_TYPE_PARAM_ATTRIBUTE).unwrap_or(false)
    });
    let counter_type_param: syn::Ident = match counter_attribute {
        // Note: If the "counter" attribute is missing, use the default.
        None => syn::parse_quote!(C),
        Some(attr) => {
            let type_param: Result<syn::Ident, _> = attr.parse_args();
            match type_param {
                Ok(t) => t,
                Err(_) => {
                    return quote_spanned! {
                        name.span() => std::compile_error!(
                            "Invalid `counter` attribute. Expected `#[counter(TYPE_PARAM)]`"
                        );
                    }
                    .into();
                }
            }
        }
    };
    let counter_type_param_str = counter_type_param.to_string();

    // Verify that the type has the special "counter" type parameter defined.
    if !ast.generics.type_params().any(|p| p.ident == counter_type_param_str) {
        return quote_spanned! {
            name.span() => std::compile_error!(
                "types deriving `CounterCollection` must have a generic \
                counter type parameter. By default this is `C` but can be \
                modified with `counter` attribute."
            );
        }
        .into();
    }

    // Manipulate the type's generic parameters in various ways. We need to make
    // adjustments to counter type parameter while leaving all other parameters
    // (types, lifetimes, etc) untouched.
    let original_generics = ast.generics.clone();
    let (_impl_generics, ty_generics, where_clause) = original_generics.split_for_impl();
    // Note: Substitute the counter type parameter for the concrete `Counter`
    // type from netstack3 base.
    let concrete_ty_generics = substitute_ident(
        ty_generics.clone(),
        &counter_type_param_str,
        syn::parse_quote!(netstack3_base::Counter),
    );
    // Note: Substitute the counter type parameter for `C1`.
    let c1_ty_generics =
        substitute_ident(ty_generics.clone(), &counter_type_param_str, syn::parse_quote!(C1));
    // Note: Substitute the counter type parameter for `C2`.
    let c2_ty_generics =
        substitute_ident(ty_generics.clone(), &counter_type_param_str, syn::parse_quote!(C2));
    // Note: Remove the counter type parameter from the `impl_generics`.
    let mut generics_no_counter = ast.generics.clone();
    generics_no_counter.params = syn::punctuated::Punctuated::from_iter(
        ast.generics
            .params
            .iter()
            .filter(|param| match param {
                syn::GenericParam::Type(p) => p.ident != counter_type_param_str,
                _ => true,
            })
            .cloned(),
    );
    let (impl_generics_no_counter, _ty_generics, _where_clause) =
        generics_no_counter.split_for_impl();
    // Note: Add an extra bound to the counter type parameter:
    // `netstack3_base::CounterRepr`.
    let mut generics_extra_bounds = ast.generics.clone();
    generics_extra_bounds.params.iter_mut().for_each(|param| match param {
        syn::GenericParam::Type(p) => {
            if p.ident == counter_type_param_str {
                // Note: We could check if the type already has the bound before
                // adding it, however it's not necessary because Rust allows bounds
                // to be duplicated (e.g. C: CounterRepr + CounterRepr) is valid.
                p.bounds.push(syn::parse_quote!(netstack3_base::CounterRepr))
            }
        }
        _ => {}
    });
    let (impl_generics_extra_bounds, _ty_generics, _where_clause) =
        generics_extra_bounds.split_for_impl();

    // Extract all the names of the fields defined by the type.
    let field_names = match ast.data {
        syn::Data::Struct(data) => data
            .fields
            .into_iter()
            .map(|f| f.ident.expect("field should have identifier"))
            .collect::<Vec<_>>(),
        // NB: It would be feasible to provide such an implementation, but we
        // have no need at the moment.
        _ => {
            return quote_spanned! {
                name.span() => std::compile_error!(
                    "derive CounterCollectionSpec only supported on structs"
                );
            }
            .into();
        }
    };

    quote! {
        impl #impl_generics_extra_bounds netstack3_base::CounterCollection
            for #name #ty_generics #where_clause {
            type Spec = #name #concrete_ty_generics;
            type Repr = #counter_type_param;
        }

        impl #impl_generics_no_counter netstack3_base::CounterCollectionSpec
            for #name #concrete_ty_generics #where_clause {
            type CounterCollection<#counter_type_param: netstack3_base::CounterRepr> =
                #name #ty_generics;
            fn transform<C1: netstack3_base::CounterRepr, C2: netstack3_base::CounterRepr>(
                counters: &#name #c1_ty_generics
            ) -> #name #c2_ty_generics {
                // Destructure `counters` into it's fields.
                let #name {
                    #(#field_names,)*
                } = counters;
                // Reconstruct `counters`, converting each field with `cast()`.
                // Import the relevant trait methods so we're certain they're in
                // scope.
                use netstack3_base::{CounterCollection as _, CounterRepr as _};
                #name {
                    #(#field_names: #field_names.cast::<C2>(),)*
                }
            }
        }
    }
    .into()
}
