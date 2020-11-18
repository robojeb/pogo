use once_cell::sync::OnceCell;
use pogo::{Edition, Global, PogoFuncCtx, PogoFuncDefinition, PogoGroup};
use std::sync::atomic::{AtomicUsize, Ordering};

fn __native_is_even(n: u32) -> bool {
    n % 2 == 0
}

#[inline]
fn is_even_with_group<Grp: PogoGroup>(n: u32) -> bool {
    match __pogo_ctx_is_even.get() {
        Some(ctx) if Grp::USE_PGO => {
            match ctx.groups.get(Grp::NAME) {
                Some(group) => {
                    match &group.pgo_state {
                        pogo::PgoState::Uninitialized | pogo::PgoState::CompilationFailed => {
                            __native_is_even(n)
                        }
                        pogo::PgoState::GatheringData(lib) => {
                            if group.pgo_count.fetch_add(1, Ordering::SeqCst) >= Grp::PGO_EXEC_COUNT
                            {
                                pogo::submit_optimization_request(ctx, Grp::NAME);
                            }

                            unsafe {
                                let func: libloading::Symbol<unsafe extern fn(u32) -> bool> = lib.get(ctx.info.name.as_bytes()).expect("Run-time compiled shared object didn't contain expected function");
                                func(n)
                            }
                        }
                        pogo::PgoState::Compiling(lib) | pogo::PgoState::Optimized(lib) => unsafe {
                            let func: libloading::Symbol<unsafe extern fn(u32) -> bool> = lib.get(ctx.info.name.as_bytes()).expect("Run-time compiled shared object didn't contain expected function");
                            func(n)
                        },
                    }
                }
                None => {
                    ctx.groups.upsert(
                        Grp::NAME,
                        || {
                            pogo::GroupState {
                                pgo_state: pogo::PgoState::Uninitialized,
                                pgo_count: AtomicUsize::new(0),
                            }
                            //TODO: Submit the first compilation request to the backend
                        },
                        |_| {
                            // The value already existed by the time we got to this branch
                            // so don't touch it, someone should have already initialized it
                        },
                    );
                    // Execute the unoptimized non-tracking version for now
                    __native_is_even(n)
                }
            }
        }

        // If we haven't initialized POGO for this function, or in this context
        // POGO is turned off, then call the "native" version of the function
        _ => __native_is_even(n),
    }
}

fn is_even(n: u32) -> bool {
    is_even_with_group::<Global>(n)
}

#[allow(non_upper_case_globals)]
static __pogo_ctx_is_even: OnceCell<PogoFuncCtx> = OnceCell::new();

#[allow(non_upper_case_globals)]
const __pogo_info_is_even: PogoFuncDefinition = PogoFuncDefinition {
    edition: Edition::Rust2018,
    name: "is_even",
    src: r#"fn is_even(n: u32) -> bool {
    n % 2 == 0
}
"#,
};

fn main() {
    pogo::init("./ex_wrk", &[(&__pogo_info_is_even, &__pogo_ctx_is_even)]).unwrap();

    let mut n: u32 = 0;
    for i in 0..10_000u32 {
        n = n.wrapping_add(i % 679);
        std::thread::sleep(std::time::Duration::from_millis(1));
        eprintln!("{}: {}", i, is_even(n));
    }

    println!("{:#?}", __pogo_ctx_is_even.get());
}
