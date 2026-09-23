// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use include_str_from_working_dir::include_str_from_working_dir_env;
use proc_macro::TokenStream;
use quote::quote;
use serde_json::Value;
use syn::Result;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;

const FFX_CONFIG_DEFAULT: &'static str = "ffx_config_default";

#[proc_macro]
pub fn include_default(_input: TokenStream) -> TokenStream {
    // Test deserializing the default configuration file at compile time.
    let default = include_str_from_working_dir_env!("FFX_DEFAULT_CONFIG_JSON");
    // This is being used as a way of validating the default json.
    // This should not happen as the JSON file is built using json_merge which does validation, but
    // leaving this here as the last line of defense.
    let _res: Option<Value> =
        serde_json::from_str(default).expect("default configuration is malformed");

    std::format!("serde_json::json!({})", default).parse().unwrap()
}

fn type_ident<'a>(ty: &'a syn::Type) -> Option<&'a syn::Ident> {
    if let syn::Type::Path(path) = ty {
        if path.qself.is_some() {
            return None;
        }
        // Checks last segment as the path might not necessarily be the type on
        // its own. This doesn't necessarily work with type aliases.
        let last_segment = path.path.segments.last()?;
        return Some(&last_segment.ident);
    }
    None
}

fn option_wrapped_type<'a>(ty: &'a syn::Type) -> Option<&'a syn::Type> {
    if let syn::Type::Path(path) = ty {
        if path.qself.is_some() {
            return None;
        }
        // Check last in the event that someone is using std::option::Option for some
        // reason.
        let last_segment = path.path.segments.last()?;
        if last_segment.ident == "Option" {
            if let syn::PathArguments::AngleBracketed(args) = &last_segment.arguments {
                let generic_args = args.args.first()?;
                if let syn::GenericArgument::Type(ty) = &generic_args {
                    return Some(ty);
                }
            }
        }
    }
    None
}

enum ConfigArgs {
    Str(syn::LitStr),
    MetaNameValue(Box<syn::MetaNameValue>),
}

impl syn::parse::Parse for ConfigArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        Ok(if input.peek(syn::LitStr) {
            ConfigArgs::Str(input.parse()?)
        } else {
            ConfigArgs::MetaNameValue(Box::new(input.parse()?))
        })
    }
}

struct FfxConfigField<'a> {
    value_type: ConfigValueType,
    key: syn::LitStr,
    default: Option<syn::LitStr>,
    func_name: &'a syn::Ident,
}

impl<'a> FfxConfigField<'a> {
    fn parse(field: &'a syn::Field, attr: &syn::Attribute) -> Result<Self> {
        let wrapped_type = option_wrapped_type(&field.ty)
            .ok_or_else(|| syn::Error::new(field.ty.span(), "type must be wrapped in Option<_>"))?;
        let value_type =
            ConfigValueType::try_from(type_ident(wrapped_type).ok_or_else(|| {
                syn::Error::new(wrapped_type.span(), "couldn't get wrapped type")
            })?)?;
        let (key, default) = {
            let mut key = None;
            let mut default = None;
            let nested =
                attr.parse_args_with(Punctuated::<ConfigArgs, syn::Token![,]>::parse_terminated)?;
            for meta in nested {
                match meta {
                    ConfigArgs::MetaNameValue(name) => {
                        let value_to_update = if name.path.is_ident("key") {
                            &mut key
                        } else if name.path.is_ident("default") {
                            &mut default
                        } else {
                            return Err(syn::Error::new(name.span(), "unsupported ident"));
                        };
                        if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(lit), .. }) =
                            &name.value
                        {
                            *value_to_update = Some(lit.clone())
                        } else {
                            return Err(syn::Error::new(
                                name.value.span(),
                                format!(
                                    "value for \"{}\" must be set to a string",
                                    value_to_update.as_ref().expect("value set to empty").value()
                                ),
                            ));
                        }
                    }
                    ConfigArgs::Str(lit) => {
                        key = Some(lit);
                        default = Option::<syn::LitStr>::None;
                        break;
                    }
                }
            }
            (key.ok_or_else(|| syn::Error::new(attr.span(), "key expected"))?, default)
        };
        let func_name = field.ident.as_ref().ok_or_else(|| {
            syn::Error::new(field.span(), "ffx_config_default fields must have names")
        })?;
        Ok(Self { value_type, key, default, func_name })
    }
}

impl<'a> quote::ToTokens for FfxConfigField<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let return_type = &self.value_type;
        let func_name = &self.func_name;
        let config_key = &self.key;
        let (return_value, top_level_return, conversion_res, backup_res) = match &self.default {
            Some(default) => (
                quote! { #return_type },
                quote! { t },
                quote! { v },
                quote! { <#return_type as std::str::FromStr>::from_str(#default)? },
            ),
            None => (
                quote! { Option<#return_type> },
                quote! { Some(t) },
                quote! { Some(v) },
                quote! { None },
            ),
        };
        tokens.extend(quote! {
            pub fn #func_name(&self, context: &ffx_config::EnvironmentContext) -> ffx_config::macro_deps::anyhow::Result<#return_value> {
                let field = self.#func_name.clone();
                Ok(if let Some(t) = field {
                    #top_level_return
                } else {
                    let cfg_value: Option<#return_type> = context.get_optional(#config_key)?;
                    if let Some(v) = cfg_value {
                        #conversion_res
                    } else {
                        #backup_res
                    }
                })
            }
        });
    }
}

enum ConfigValueType {
    StringType,
    FloatType,
    IntegerType,
}

impl quote::ToTokens for ConfigValueType {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            ConfigValueType::StringType => tokens.extend(quote! { String }),
            ConfigValueType::FloatType => tokens.extend(quote! { f64 }),
            ConfigValueType::IntegerType => tokens.extend(quote! { u64 }),
        }
    }
}

impl ConfigValueType {
    fn try_from(value: &syn::Ident) -> Result<Self> {
        Ok(match value {
            n if n == "String" => ConfigValueType::StringType,
            n if n == "f64" => ConfigValueType::FloatType,
            n if n == "u64" => ConfigValueType::IntegerType,
            _ => return Err(syn::Error::new(value.span(), "unsupported type")),
        })
    }
}

fn generate_impls(item: &syn::ItemStruct) -> TokenStream {
    let mut configs = Vec::new();
    let mut runtime_config_builder = Vec::new();
    runtime_config_builder.push(quote! {
        let mut config_builder = Option::<String>::None;
    });
    for field in item.fields.iter() {
        if let Some(attr) = field.attrs.iter().find(|a| a.path().is_ident(FFX_CONFIG_DEFAULT)) {
            let config_field = match FfxConfigField::parse(field, attr) {
                Ok(f) => f,
                Err(e) => return e.to_compile_error().into(),
            };
            let field = &config_field.func_name;
            let key = &config_field.key;
            runtime_config_builder.push(quote! {
                // TODO(awdavies): Perhaps this should do something with the
                // default value (since this isn't a necessary if/else)?
                let append_string = if let Some(t) = &self.#field {
                    Some(format!("{}={}", #key, t))
                } else {
                    None
                };
                if let Some(s) = append_string {
                    if let Some(c) = &mut config_builder {
                        c.push_str(format!(",{}", s).as_str());
                    } else {
                        config_builder = Some(s);
                    }
                }
            });
            configs.push(config_field);
        }
    }
    runtime_config_builder.push(quote! {
        config_builder
    });
    let struct_name = &item.ident;
    TokenStream::from(quote! {
        impl #struct_name {
            #(#configs)*

            pub fn runtime_config_overrides(&self) -> Option<String>  {
                #(#runtime_config_builder)*
            }
        }
    })
}

/// Allows specifying parameters that are backed by configurations. Also allows
/// specifying a default value if the field is missing from the configuration.
///
/// # Example
/// ```
/// #[derive(FfxConfigBacked)]
/// struct Abc {
///   #[ffx_config_default(key = "abc.first", default = "0")]
///   field1: Option<u64>,
///
///   #[ffx_config_default(key = "abc.second")]
///   field2: Option<u64>,
///
///   #[ffx_config_default("abc.third")]
///   field3: Option<u64>,
///
///   field4: u32,
/// }
/// ```
#[proc_macro_derive(FfxConfigBacked, attributes(ffx_config_default))]
pub fn derive_ffx_config_backed(input: TokenStream) -> TokenStream {
    let item: syn::ItemStruct = syn::parse(input.into()).expect("expected struct");
    let impls = generate_impls(&item);
    impls
}
