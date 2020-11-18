extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

#[proc_macro_attribute]
pub fn pogo(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);

    let input_src_string = quote!(#input).to_string();

    let function_name = input.sig.ident;
    let function_inputs = input.sig.inputs;
    let return_type = input.sig.output;
    let function_body = input.block;
    let native_func_name = quote::format_ident!("__pogo_native_{}", function_name);

    let native_function = quote! {
        pub(crate) fn #native_func_name(#function_inputs) #return_type {
            #function_body
        }
    };

    let mut type_args: syn::punctuated::Punctuated<Box<syn::Type>, syn::token::Comma> =
        syn::punctuated::Punctuated::new();

    for arg in function_inputs.iter() {
        match arg {
            syn::FnArg::Receiver(_) => panic!("Not supported"),
            syn::FnArg::Typed(pat_type) => {
                type_args.push_value(pat_type.ty.clone());
            }
        }
    }

    let mut arg_names: syn::punctuated::Punctuated<syn::Ident, syn::token::Comma> =
        syn::punctuated::Punctuated::new();

    for arg in function_inputs.iter() {
        match arg {
            syn::FnArg::Receiver(_) => panic!("Not supported"),
            syn::FnArg::Typed(pat_type) => match &pat_type.pat.as_ref() {
                syn::Pat::Ident(ident) => arg_names.push_value(ident.ident.clone()),
                _ => panic!("Not supported"),
            },
        }
    }

    let vis = input.vis;
    let group_func_name = quote::format_ident!("{}_with_group", function_name);
    let ctx_name = quote::format_ident!("__pogo_ctx_{}", function_name);
    let info_name = quote::format_ident!("__pogo_info_{}", function_name);

    let str_func_name = function_name.to_string();

    TokenStream::from(quote! {
        #native_function

        #[allow(non_upper_case_globals)]
        static #ctx_name: pogo::ContextCell = pogo::ContextCell::new();

        #[allow(non_upper_case_globals)]
        static #info_name: pogo::PogoFuncDefinition = pogo::PogoFuncDefinition {
            edition: pogo::Edition::Rust2018,
            name: #str_func_name,
            src: #input_src_string,
        };

        #vis fn #function_name(#function_inputs) #return_type {
            #group_func_name::<pogo::Global>(#arg_names)
        }

        #vis fn #group_func_name<Grp: pogo::PogoGroup>(#function_inputs) #return_type {
            match #ctx_name.get() {
                Some(ctx) if Grp::USE_PGO => {
                    match ctx.groups.get(Grp::NAME) {
                        Some(group) => {
                            match &group.pgo_state {
                                pogo::PgoState::Uninitialized | pogo::PgoState::CompilationFailed => {
                                    #native_func_name(#arg_names)
                                }
                                pogo::PgoState::GatheringData(lib) => {
                                    if group.pgo_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= Grp::PGO_EXEC_COUNT
                                    {
                                        pogo::submit_optimization_request(ctx, Grp::NAME);
                                    }

                                    unsafe {
                                        let func: libloading::Symbol<unsafe extern fn(#type_args) #return_type> = lib.get(ctx.info.name.as_bytes()).expect("Run-time compiled shared object didn't contain expected function");
                                        func(#arg_names)
                                    }
                                }
                                pogo::PgoState::Compiling(lib) | pogo::PgoState::Optimized(lib) => unsafe {
                                    let func: libloading::Symbol<unsafe extern fn(#type_args) #return_type> = lib.get(ctx.info.name.as_bytes()).expect("Run-time compiled shared object didn't contain expected function");
                                    func(#arg_names)
                                },
                            }
                        }
                        None => {
                            ctx.groups.upsert(
                                Grp::NAME,
                                || {
                                    pogo::GroupState {
                                        pgo_state: pogo::PgoState::Uninitialized,
                                        pgo_count: std::sync::atomic::AtomicUsize::new(0),
                                    }
                                },
                                |_| {
                                    // The value already existed by the time we got to this branch
                                    // so don't touch it, someone should have already initialized it
                                },
                            );
                            // Execute the unoptimized non-tracking version for now
                            #native_func_name(#arg_names)
                        }
                    }
                },
                _ => #native_func_name(#arg_names),
            }
        }
    })
}
