// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::str::FromStr;

use either::Either;
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{ToTokens, quote};
use syn::spanned::Spanned as _;

/// A specific implementation of a test variant.
#[derive(Clone, Debug)]
struct Implementation {
    type_name: syn::Path,
    suffix: &'static str,
    attrs: Vec<syn::Attribute>,
}

/// A variant tests will be generated for.
struct Variant {
    ident: syn::Ident,
    variant_type: VariantType,
    seen: bool,
}

impl syn::parse::Parse for Variant {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let ident = syn::Ident::parse(input)?;
        let _ = <syn::Token![,]>::parse(input)?;
        let variant_ident = syn::Ident::parse(input)?;
        if !input.is_empty() {
            return Err(syn::Error::new(input.span(), "unexpected trailing arguments"));
        }
        let variant_type = VariantType::from_str(&variant_ident.to_string())
            .map_err(|()| syn::Error::new(variant_ident.span(), "invalid variant"))?;
        Ok(Variant { ident, variant_type, seen: false })
    }
}

#[derive(Debug, Copy, Clone)]
enum VariantType {
    Ip,
    Netstack,
    DhcpClient,
    Manager,
    NetstackAndDhcpClient,
}

impl FromStr for VariantType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Ip" => Ok(Self::Ip),
            "Netstack" => Ok(Self::Netstack),
            "DhcpClient" => Ok(Self::DhcpClient),
            "Manager" => Ok(Self::Manager),
            "NetstackAndDhcpClient" => Ok(Self::NetstackAndDhcpClient),
            _ => Err(()),
        }
    }
}

impl VariantType {
    // NB: Extracted to constants so they can be reused in tests.
    const NETSTACK2: &'static str = "netstack_testing_common::realms::Netstack2";
    const NETSTACK3: &'static str = "netstack_testing_common::realms::Netstack3";
    const IPV4: &'static str = "net_types::ip::Ipv4";
    const IPV6: &'static str = "net_types::ip::Ipv6";

    fn implementations(&self) -> Vec<Implementation> {
        let disable_on_riscv = || syn::parse_quote!(#[cfg(not(target_arch = "riscv64"))]);

        match self {
            Self::Netstack => vec![
                Implementation {
                    type_name: str_to_syn_path(Self::NETSTACK2),
                    suffix: "ns2",
                    attrs: vec![disable_on_riscv()],
                },
                Implementation {
                    type_name: str_to_syn_path(Self::NETSTACK3),
                    suffix: "ns3",
                    attrs: vec![],
                },
            ],
            Self::Ip => vec![
                Implementation {
                    type_name: str_to_syn_path(Self::IPV4),
                    suffix: "v4",
                    attrs: vec![],
                },
                Implementation {
                    type_name: str_to_syn_path(Self::IPV6),
                    suffix: "v6",
                    attrs: vec![],
                },
            ],
            Self::Manager => vec![
                Implementation {
                    type_name: str_to_syn_path("netstack_testing_common::realms::NetCfgBasic"),
                    suffix: "netcfg_basic",
                    attrs: vec![],
                },
                Implementation {
                    type_name: str_to_syn_path("netstack_testing_common::realms::NetCfgAdvanced"),
                    suffix: "netcfg_advanced",
                    attrs: vec![],
                },
            ],
            Self::DhcpClient => vec![
                Implementation {
                    type_name: str_to_syn_path("netstack_testing_common::realms::InStack"),
                    suffix: "dhcp_in_stack",
                    attrs: vec![],
                },
                Implementation {
                    type_name: str_to_syn_path("netstack_testing_common::realms::OutOfStack"),
                    suffix: "dhcp_out_of_stack",
                    attrs: vec![],
                },
            ],
            Self::NetstackAndDhcpClient => vec![
                Implementation {
                    type_name: str_to_syn_path(
                        "netstack_testing_common::realms::Netstack2AndInStackDhcpClient",
                    ),
                    suffix: "ns2_with_dhcp_in_stack",
                    attrs: vec![disable_on_riscv()],
                },
                Implementation {
                    type_name: str_to_syn_path(
                        "netstack_testing_common::realms::Netstack2AndOutOfStackDhcpClient",
                    ),
                    suffix: "ns2_with_dhcp_out_of_stack",
                    attrs: vec![disable_on_riscv()],
                },
                Implementation {
                    type_name: str_to_syn_path(
                        "netstack_testing_common::realms::Netstack3AndOutOfStackDhcpClient",
                    ),
                    suffix: "ns3_with_dhcp_out_of_stack",
                    attrs: vec![],
                },
            ],
        }
    }
}

/// A specific variation of a test.
#[derive(Default, Debug)]
struct TestVariation {
    /// Params that we use to instantiate the test.
    params: Vec<syn::Path>,
    /// Unrelated bounds that we pass along.
    generics: Vec<syn::TypeParam>,
    /// Suffix of the test name.
    suffix: String,
    /// Extra attributes that get stuck on the test function.
    attributes: Vec<syn::Attribute>,
}

fn str_to_syn_path(path: &str) -> syn::Path {
    let mut segments = syn::punctuated::Punctuated::<_, syn::token::PathSep>::new();
    for seg in path.split("::") {
        segments.push(syn::PathSegment {
            ident: syn::Ident::new(seg, Span::call_site()),
            arguments: syn::PathArguments::None,
        });
    }
    syn::Path { leading_colon: None, segments }
}

fn permutations_over_type_generics<'a>(
    variants: &'a mut [Variant],
    type_generics: &'a [&'a syn::TypeParam],
) -> impl Iterator<Item = TestVariation> + 'a {
    if type_generics.is_empty() {
        return Either::Right(std::iter::once(TestVariation::default()));
    }

    let mut find_variants = |type_param: &syn::TypeParam| -> Option<VariantType> {
        variants.iter_mut().find_map(|Variant { ident, variant_type, seen }| {
            (ident == &type_param.ident).then(|| {
                *seen = true;
                *variant_type
            })
        })
    };

    #[derive(Clone, Debug)]
    enum Piece<'a> {
        PassThroughGeneric(&'a syn::TypeParam),
        Instantiated(Implementation),
    }
    let piece_iterators = type_generics.iter().map(|type_param| {
        // If there are multiple implementations, produce an iterator that
        // will yield them all. Otherwise produce an iterator that will
        // yield the generic parameter once.
        match find_variants(type_param) {
            None => Either::Left(std::iter::once(Piece::PassThroughGeneric(*type_param))),
            Some(variant_type) => Either::Right(
                variant_type
                    .implementations()
                    .into_iter()
                    .map(|implementation| Piece::Instantiated(implementation)),
            ),
        }
    });

    Either::Left(itertools::Itertools::multi_cartesian_product(piece_iterators).map(|pieces| {
        let (mut params, mut generics, mut attributes, mut name_pieces) =
            (Vec::new(), Vec::new(), Vec::new(), vec![""]);
        for piece in pieces {
            match piece {
                Piece::Instantiated(Implementation { type_name, suffix, attrs }) => {
                    params.push(type_name);
                    name_pieces.push(suffix);
                    attributes.extend(attrs.into_iter());
                }
                Piece::PassThroughGeneric(type_param) => {
                    params.push(type_param.ident.clone().into());
                    generics.push(type_param.clone());
                }
            }
        }
        let suffix = name_pieces.join("_");
        TestVariation { params, generics, suffix, attributes }
    }))
}

struct NetstackTestArgs {
    test_name: bool,
}

impl syn::parse::Parse for NetstackTestArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        // Default using test name to true. User can override.
        let mut test_name = true;

        let args =
            syn::punctuated::Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated(
                input,
            )?;
        for syn::MetaNameValue { path, value, .. } in args {
            let ident = path
                .get_ident()
                .ok_or_else(|| syn::Error::new(path.span(), "expecting identifier"))?;
            if ident == "test_name" {
                let lit = match value {
                    syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Bool(b), .. }) => b,
                    _ => return Err(syn::Error::new(value.span(), "expected boolean")),
                };
                test_name = lit.value;
            } else {
                return Err(syn::Error::new(path.span(), format!("unrecognized option {ident}")));
            }
        }
        Ok(Self { test_name })
    }
}

fn extract_variants(mut input: syn::ItemFn) -> Result<(syn::ItemFn, Vec<Variant>), syn::Error> {
    let syn::ItemFn { attrs, .. } = &mut input;
    let all_attrs = std::mem::take(attrs);
    // Filter out the attributes that are determining variants, putting back the
    // ones that aren't part of netstack_test.
    let mut variants = Vec::new();
    for attr in all_attrs {
        if !attr.path().get_ident().is_some_and(|ident| ident == "variant") {
            // Not a netstack_test variant attribute.
            attrs.push(attr);
            continue;
        }
        variants.push(attr.parse_args::<Variant>()?);
    }

    Ok((input, variants))
}

fn netstack_test_inner(
    args: NetstackTestArgs,
    input: syn::ItemFn,
    variants: &mut [Variant],
) -> TokenStream {
    let mut item = input.clone();
    let impl_attrs = std::mem::replace(&mut item.attrs, Vec::new());
    let syn::ItemFn { attrs: _, vis: _, sig, block: _ } = &item;
    let syn::Signature {
        constness: _,
        asyncness: _,
        unsafety: _,
        abi: _,
        fn_token: _,
        ident: name,
        generics: syn::Generics { lt_token: _, params, gt_token: _, where_clause },
        paren_token: _,
        inputs,
        variadic: _,
        output,
    } = sig;

    let NetstackTestArgs { test_name } = args;

    if test_name {
        let arg = if let Some(arg) = inputs.first() {
            arg
        } else {
            return syn::Error::new_spanned(inputs, "test functions must have a name argument")
                .to_compile_error()
                .into();
        };

        let arg_type = match arg {
            syn::FnArg::Typed(syn::PatType { attrs: _, pat: _, colon_token: _, ty }) => ty,
            other => return syn::Error::new_spanned(
                inputs,
                format!(
                    "test function's first argument must be a `&str` for test name; got = {:#?}",
                    other
                ),
            )
            .to_compile_error()
            .into(),
        };

        let arg_type = match arg_type.as_ref() {
            syn::Type::Reference(syn::TypeReference {
                and_token: _,
                lifetime: _,
                mutability: _,
                elem,
            }) => elem,
            other => return syn::Error::new_spanned(
                inputs,
                format!(
                    "test function's first argument must be a `&str` for test name; got = {:#?}",
                    other
                ),
            )
            .to_compile_error()
            .into(),
        };

        let arg_type = match arg_type.as_ref() {
            syn::Type::Path(syn::TypePath { qself: _, path }) => path,
            other => return syn::Error::new_spanned(
                inputs,
                format!(
                    "test function's first argument must be a `&str` for test name; got = {:#?}",
                    other
                ),
            )
            .to_compile_error()
            .into(),
        };

        if !arg_type.is_ident("str") {
            return syn::Error::new_spanned(
                inputs,
                "test function's first argument must be a `&str`  for test name",
            )
            .to_compile_error()
            .into();
        }
    }

    // We only care about generic type parameters and their last trait bound.
    let mut type_generics = Vec::with_capacity(params.len());
    for generic in params.iter() {
        let generic_type = match generic {
            syn::GenericParam::Type(t) => t,
            other => {
                return syn::Error::new_spanned(
                    input.to_token_stream(),
                    format!("test functions only support generic parameters; got = {:#?}", other),
                )
                .to_compile_error()
                .into();
            }
        };

        type_generics.push(generic_type)
    }

    // Pass the test name as the first argument, and keep other arguments
    // in the generated function which will be passed to the original function.
    let skip = test_name.then_some(1).unwrap_or(0);
    let impl_inputs = inputs.iter().skip(skip).cloned().collect::<Vec<_>>();

    let mut args = Vec::new();
    for arg in impl_inputs.iter() {
        let arg = match arg {
            syn::FnArg::Typed(syn::PatType { attrs: _, pat, colon_token: _, ty: _ }) => pat,
            other => {
                return syn::Error::new_spanned(
                    input.to_token_stream(),
                    format!("expected typed fn arg; got = {:#?}", other),
                )
                .to_compile_error()
                .into();
            }
        };

        let arg = match arg.as_ref() {
            syn::Pat::Ident(syn::PatIdent {
                attrs: _,
                by_ref: _,
                mutability: _,
                ident,
                subpat: _,
            }) => ident,
            other => {
                return syn::Error::new_spanned(
                    input.to_token_stream(),
                    format!("expected ident fn arg; got = {:#?}", other),
                )
                .to_compile_error()
                .into();
            }
        };

        args.push(syn::Expr::Path(syn::ExprPath {
            attrs: Vec::new(),
            qself: None,
            path: arg.clone().into(),
        }));
    }

    let make_args = |name: String| {
        test_name
            .then(|| {
                syn::Expr::Lit(syn::ExprLit {
                    attrs: vec![],
                    lit: syn::Lit::Str(syn::LitStr::new(&name, Span::call_site())),
                })
            })
            .into_iter()
            .chain(args.iter().cloned())
    };

    let mut permutations = permutations_over_type_generics(variants, &type_generics).peekable();
    let first_permutation = permutations.next().expect("at least one permutation");

    // If we're not emitting any variants we're aware of, just re-emit the
    // function with its name passed in as the first argument.
    if permutations.peek().is_none() {
        let TestVariation { params: _, generics, suffix, attributes } = first_permutation;
        // Suffix should be empty for single permutation.
        assert_eq!(suffix, "");
        let args = make_args(name.to_string());
        let result = quote! {
            #(#impl_attrs)*
            #(#attributes)*
            #[fuchsia::test]
            async fn #name < #(#generics),* > ( #(#impl_inputs),* ) #output #where_clause {
                #item
                #name ( #(#args),* ).await
            }
        }
        .into();
        return result;
    }

    // Generate the list of test variations we will generate.
    //
    // Glue the first permutation back on so it gets included in the output.
    let impls = std::iter::once(first_permutation).chain(permutations).map(
        |TestVariation { params, generics, suffix, attributes }| {
            // We don't need to add an "_" between the name and the suffix here as the suffix
            // will start with one.
            let test_name_str = format!("{}{}", name, suffix);
            let test_name = syn::Ident::new(&test_name_str, Span::call_site());
            let args = make_args(test_name_str);

            quote! {
                #(#impl_attrs)*
                #(#attributes)*
                #[fuchsia::test]
                async fn #test_name < #(#generics),* > ( #(#impl_inputs),* ) #output #where_clause {
                    #name :: < #(#params),* > ( #(#args),* ).await
                }
            }
        },
    );

    let result = quote! {
        #item
        #(#impls)*
    };

    result.into()
}

/// Runs a test `fn` over different variations of known type parameters based
/// on the test `fn`'s type parameters.
///
/// Each supported type parameter substitution must be informed with a call to
/// the `#[variant(X, Variant)]` attribute where `X` is the type parameter
/// identifier and `Variant` is one of the supported variant substitutions.
///
/// See [`VariantType`] for the supported variants passed to the
/// `#[variant(..)]` attribute.
///
/// Tests are annotated with `#[fuchsia::test]` from `//src/lib/fuchsia` in order
/// to provide async test support and hook up the standard logging backend.
/// This means that log-severity limits will apply to tests, i.e. an error log
/// due to a panic will fail a test even if the test is configured to expect to
/// fail via //src/lib/testing/expectation.
///
/// Example:
///
/// ```
/// #[netstack_test]
/// #[variant(N, Netstack)]
/// async fn test_foo<N: Netstack>(name: &str) {}
/// ```
///
/// Expands to:
/// ```
/// async fn test_foo<N: Netstack>(name: &str){/*...*/}
/// #[fuchsia::test]
/// async fn test_foo_ns2() {
///     test_foo::<netstack_testing_common::realms::Netstack2>("test_foo_ns2").await
/// }
/// #[fuchsia::test]
/// async fn test_foo_ns3() {
///     test_foo::<netstack_testing_common::realms::Netstack3>("test_foo_ns3").await
/// }
/// ```
///
/// Similarly,
/// ```
/// #[netstack_test]
/// #[variant(M, Manager)]
/// async fn test_foo<M: Manager>(name: &str) {/*...*/}
/// ```
///
/// Expands equivalently to the netstack variant.
///
/// This macro also supports expanding with multiple variations, including
/// multiple occurrences of the same trait bound.
/// ```
/// #[netstack_test]
/// #[variant(N1, Netstack)]
/// #[variant(N2, Netstack)]
/// async fn test_foo<N1: Netstack, N2: Netstack>(name: &str) {/*...*/}
/// ```
///
/// Expands to:
/// ```
/// async fn test_foo<N1: Netstack, N2: Netstack>(name: &str) {/*...*/}
/// #[fuchsia::test]
/// async fn test_foo_ns2_ns2() {
///     test_foo::<
///         netstack_testing_common::realms::Netstack2,
///         netstack_testing_common::realms::Netstack2,
///     >("test_foo_ns2_ns2")
///     .await
/// }
/// #[fuchsia::test]
/// async fn test_foo_ns2_ns3() {
///     test_foo::<
///         netstack_testing_common::realms::Netstack2,
///         netstack_testing_common::realms::Netstack3,
///     >("test_foo_ns2_ns3")
///     .await
/// }
/// #[fuchsia::test]
/// async fn test_foo_ns3_ns2() {
///     test_foo::<
///         netstack_testing_common::realms::Netstack3,
///         netstack_testing_common::realms::Netstack2,
///     >("test_foo_ns3_ns2")
///     .await
/// }
/// #[fuchsia::test]
/// async fn test_foo_ns3_ns3() {
///     test_foo::<
///         netstack_testing_common::realms::Netstack3,
///         netstack_testing_common::realms::Netstack3,
///     >("test_foo_ns3_ns3")
///     .await
/// }
/// ```
///
/// A test with no type parameters is expanded to receive the function name as
/// the first argument.
/// ```
/// #[netstack_test]
/// async fn test_foo(name: &str) {/*...*/}
/// ```
///
/// Expands to
/// ```
/// #[fuchsia::test]
/// async fn test_foo() {
///    async fn test_foo(name: &str){/*...*/}
///    test_foo("test_foo").await
/// }
/// ```
///
/// Substitution of the first parameter with the test name can be disabled with
///
/// ```
/// #[netstack_test(test_name = false)]
/// ```
#[proc_macro_attribute]
pub fn netstack_test(attrs: TokenStream, input: TokenStream) -> TokenStream {
    let args = syn::parse_macro_input!(attrs as NetstackTestArgs);

    let item = syn::parse_macro_input!(input as syn::ItemFn);
    let (item, mut variants) = match extract_variants(item) {
        Ok(i) => i,
        Err(e) => return e.to_compile_error().into(),
    };

    let result = netstack_test_inner(args, item, &mut variants[..]);

    // Check for unused variants before returning.
    if let Some(v) = variants.iter().find(|v| !v.seen) {
        return syn::Error::new(v.ident.span(), "unused variant").to_compile_error().into();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[derive(Debug)]
    struct VariantExpectation {
        params: Vec<&'static str>,
        suffix: &'static str,
    }

    impl PartialEq<TestVariation> for VariantExpectation {
        fn eq(&self, other: &TestVariation) -> bool {
            self.suffix == other.suffix
                && self
                    .params
                    .iter()
                    .map(|p| syn::parse_str::<syn::Path>(p).unwrap())
                    .collect::<Vec<_>>()
                    == other.params
        }
    }

    #[test_case(vec![] => vec![VariantExpectation {
        params: vec![],
        suffix: "",
    }]; "default")]
    #[test_case(vec!["N"] => vec![VariantExpectation {
        params: vec![VariantType::NETSTACK2],
        suffix: "_ns2",
    }, VariantExpectation {
        params: vec![VariantType::NETSTACK3],
        suffix: "_ns3",
    }]; "simple case")]
    #[test_case(vec!["N", "I"] => vec![VariantExpectation {
        params: vec![VariantType::NETSTACK2, VariantType::IPV4],
        suffix: "_ns2_v4",
    }, VariantExpectation {
        params: vec![VariantType::NETSTACK2, VariantType::IPV6],
        suffix: "_ns2_v6",
    }, VariantExpectation {
        params: vec![VariantType::NETSTACK3, VariantType::IPV4],
        suffix: "_ns3_v4",
    }, VariantExpectation {
        params: vec![VariantType::NETSTACK3, VariantType::IPV6],
        suffix: "_ns3_v6",
    }]; "two traits")]
    #[test_case(vec!["N", "NN"] => vec![VariantExpectation {
        params: vec![VariantType::NETSTACK2, VariantType::NETSTACK2],
        suffix: "_ns2_ns2",
    }, VariantExpectation {
        params: vec![VariantType::NETSTACK2, VariantType::NETSTACK3],
        suffix: "_ns2_ns3",
    }, VariantExpectation {
        params: vec![VariantType::NETSTACK3, VariantType::NETSTACK2],
        suffix: "_ns3_ns2",
    }, VariantExpectation {
        params: vec![VariantType::NETSTACK3, VariantType::NETSTACK3],
        suffix: "_ns3_ns3",
    }]; "two occurrences of a single variant type")]
    fn permutation(generics: impl IntoIterator<Item = &'static str>) -> Vec<TestVariation> {
        let generics = generics
            .into_iter()
            .map(|g| syn::parse_str(g).unwrap())
            .collect::<Vec<syn::TypeParam>>();
        let generics = generics.iter().collect::<Vec<&_>>();

        permutations_over_type_generics(
            &mut [
                Variant {
                    ident: syn::parse_str("N").unwrap(),
                    variant_type: VariantType::Netstack,
                    seen: false,
                },
                Variant {
                    ident: syn::parse_str("NN").unwrap(),
                    variant_type: VariantType::Netstack,
                    seen: false,
                },
                Variant {
                    ident: syn::parse_str("I").unwrap(),
                    variant_type: VariantType::Ip,
                    seen: false,
                },
            ],
            &generics,
        )
        .collect()
    }
}
