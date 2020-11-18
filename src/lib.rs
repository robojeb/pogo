extern crate pogo_attr;

use chashmap::CHashMap;
use crossbeam::channel::{unbounded, Receiver, Sender};
use libloading::Library;
use once_cell::sync::OnceCell;
use std::error::Error;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;

pub use pogo_attr::pogo;

pub type ContextCell = once_cell::sync::OnceCell<PogoFuncCtx>;

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum Edition {
    Rust2015,
    Rust2018,
}

#[derive(Debug)]
pub struct PogoFuncDefinition {
    pub edition: Edition,
    pub name: &'static str,
    pub src: &'static str,
}

#[derive(Debug)]
pub struct PogoFuncCtx {
    pub info: &'static PogoFuncDefinition,
    pub groups: CHashMap<&'static str, GroupState>,
}

#[derive(Debug)]
pub struct GroupState {
    pub pgo_state: PgoState,
    pub pgo_count: AtomicUsize,
}

#[derive(Debug)]
pub enum PgoState {
    /// The optimization group exists but the initial shared object is not created
    Uninitialized,
    /// The function is being profiled so we have to count how many executions
    /// have occured so far
    GatheringData(Library),
    /// The shared object is being recompiled with PGO right now, counting executions
    /// is no longer needed.
    Compiling(Library),
    /// The current shared object is has PGO applied
    Optimized(Library),
    /// Compiling the shared object failed for some reason
    CompilationFailed,
}

impl PgoState {
    fn to_compiling(&mut self) {
        let mut other = PgoState::Uninitialized;
        std::mem::swap(self, &mut other);
        match other {
            PgoState::Uninitialized | PgoState::CompilationFailed => {
                *self = PgoState::CompilationFailed;
            }
            PgoState::GatheringData(lib) | PgoState::Compiling(lib) | PgoState::Optimized(lib) => {
                *self = PgoState::Compiling(lib)
            }
        }
    }
}

pub fn init<P: Into<PathBuf>>(
    working_dir: P,
    funcs: &[(&'static PogoFuncDefinition, &'static OnceCell<PogoFuncCtx>)],
) -> Result<(), Box<dyn Error>> {
    // Initialize the working directory
    let working_dir: PathBuf = working_dir.into();
    std::fs::create_dir_all(&working_dir)?;

    // Initialize the background thread
    let (send, recv) = unbounded();

    match PGO_REQ_SENDER.set(send) {
        Ok(()) => {
            // We filled this so initialize the background thread
            let thread_working_dir = working_dir.clone();
            std::thread::spawn(|| pgo_worker(thread_working_dir, recv));
        }
        Err(_) => {} // Just jump straight to submitting
    }

    let req_sender = PGO_REQ_SENDER.get().unwrap().clone();

    // Submit all the functions for initialization
    for (func_def, func_ctx_cell) in funcs {
        // Try to initialize the function context
        let func_ctx_struct = PogoFuncCtx {
            info: func_def,
            groups: CHashMap::with_capacity(1),
        };

        // Submit the global context unconditionally
        func_ctx_struct.groups.insert_new(
            Global::NAME,
            GroupState {
                pgo_state: PgoState::Uninitialized,
                pgo_count: AtomicUsize::new(0),
            },
        );

        match func_ctx_cell.set(func_ctx_struct) {
            Ok(()) => {
                // Create the source-code for this function
                std::fs::create_dir_all(working_dir.join(func_def.name))?;

                let mut src_file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .open(working_dir.join(func_def.name).join("func_src.rs"))?;

                src_file.write(
                    r#"#![crate_type="cdylib"]

#[no_mangle]
pub "#
                        .as_bytes(),
                )?;
                src_file.write_all(func_def.src.as_bytes())?;
                src_file.flush()?;

                // Submit this for initial compilation
                req_sender.send(PGORequest::Initial(PGOCompilationInfo {
                    ctx: &func_ctx_cell.get().unwrap(),
                    group_name: Global::NAME,
                }))?;
            }

            // This is already initialized, just skip it
            Err(_) => continue,
        }
    }

    Ok(())
}

static PGO_REQ_SENDER: OnceCell<Sender<PGORequest>> = OnceCell::new();

pub fn submit_optimization_request(ctx: &'static PogoFuncCtx, group_name: &'static str) {
    let req_sender = PGO_REQ_SENDER.get().unwrap().clone();

    req_sender
        .send(PGORequest::Optimized(PGOCompilationInfo {
            ctx,
            group_name,
        }))
        .unwrap();
}

pub fn pgo_worker(working_directory: PathBuf, rec_recv: Receiver<PGORequest>) {
    while let Ok(req) = rec_recv.recv() {
        match req {
            PGORequest::Initial(comp_info) => {
                println!(
                    "Got initial compilation request: {}::{}",
                    comp_info.group_name, comp_info.ctx.info.name
                );

                let func_base_path = working_directory.join(comp_info.ctx.info.name);
                let group_working_dir = func_base_path.join(comp_info.group_name);

                // Create the directory for this group
                match std::fs::create_dir_all(&group_working_dir) {
                    Ok(_) => {}
                    Err(_) => {
                        match comp_info.ctx.groups.get_mut(comp_info.group_name) {
                            Some(mut group) => {
                                group.pgo_state = PgoState::CompilationFailed;
                            }
                            None => {}
                        };
                        continue;
                    }
                }

                let mut cmd = std::process::Command::new("rustc");
                cmd.arg(format!(
                    "-Cprofile-generate={}",
                    group_working_dir.join("profile_data").to_string_lossy()
                ));

                cmd.arg("--edition");
                match comp_info.ctx.info.edition {
                    Edition::Rust2015 => cmd.arg("2015"),
                    Edition::Rust2018 => cmd.arg("2018"),
                };
                cmd.arg("-o");
                cmd.arg(group_working_dir.join("instrumented.so").as_os_str());
                cmd.arg(func_base_path.join("func_src.rs").as_os_str());

                println!("{:?}", cmd);

                match cmd.status() {
                    Ok(exit_status) => {
                        if exit_status.success() {
                            match comp_info.ctx.groups.get_mut(comp_info.group_name) {
                                Some(mut group) => {
                                    group.pgo_state = match Library::new(
                                        group_working_dir.join("instrumented.so"),
                                    ) {
                                        Ok(lib) => PgoState::GatheringData(lib),
                                        Err(_) => PgoState::CompilationFailed,
                                    };
                                }
                                None => {}
                            }
                        } else {
                            match comp_info.ctx.groups.get_mut(comp_info.group_name) {
                                Some(mut group) => {
                                    group.pgo_state = PgoState::CompilationFailed;
                                }
                                None => {}
                            }
                        }
                    }
                    Err(_) => match comp_info.ctx.groups.get_mut(comp_info.group_name) {
                        Some(mut group) => {
                            group.pgo_state = PgoState::CompilationFailed;
                        }
                        None => {}
                    },
                }
            }

            PGORequest::Optimized(comp_info) => {
                println!(
                    "Got optimized compilation request: {}::{}",
                    comp_info.group_name, comp_info.ctx.info.name
                );

                // Update to indicate that we are currently compiling
                match comp_info.ctx.groups.get_mut(comp_info.group_name) {
                    Some(mut group) => {
                        group.pgo_state.to_compiling();
                        match group.pgo_state {
                            PgoState::CompilationFailed => {
                                continue;
                            }
                            _ => {}
                        };
                    }
                    None => {
                        continue;
                    }
                }

                let func_base_path = working_directory.join(comp_info.ctx.info.name);
                let group_working_dir = func_base_path.join(comp_info.group_name);
                let profile_data_dir = group_working_dir.join("profile_data");

                // Gather all the data together
                let mut cmd = std::process::Command::new("cargo");
                cmd.args(&["profdata", "--"]);

                cmd.arg("merge");
                cmd.arg("-o");
                cmd.arg(group_working_dir.join("pgo.profdata"));
                cmd.arg(profile_data_dir.as_os_str());

                println!("{:?}", cmd);

                match cmd.status() {
                    Ok(exit_status) => {
                        if !exit_status.success() {
                            match comp_info.ctx.groups.get_mut(comp_info.group_name) {
                                Some(mut group) => {
                                    group.pgo_state = PgoState::CompilationFailed;
                                }
                                None => {}
                            }
                            continue;
                        }
                    }
                    Err(_) => {
                        match comp_info.ctx.groups.get_mut(comp_info.group_name) {
                            Some(mut group) => {
                                group.pgo_state = PgoState::CompilationFailed;
                            }
                            None => {}
                        };
                        continue;
                    }
                }

                // Compile using the gathered data
                let mut cmd = std::process::Command::new("rustc");
                cmd.arg(format!(
                    "-Cprofile-use={}",
                    group_working_dir.join("pgo.profdata").to_string_lossy()
                ));

                cmd.arg("--edition");
                match comp_info.ctx.info.edition {
                    Edition::Rust2015 => cmd.arg("2015"),
                    Edition::Rust2018 => cmd.arg("2018"),
                };
                cmd.arg("-o");
                cmd.arg(group_working_dir.join("optimized.so").as_os_str());
                cmd.arg(func_base_path.join("func_src.rs").as_os_str());

                println!("{:?}", cmd);

                match cmd.status() {
                    Ok(exit_status) => {
                        if exit_status.success() {
                            match comp_info.ctx.groups.get_mut(comp_info.group_name) {
                                Some(mut group) => {
                                    group.pgo_state = match Library::new(
                                        group_working_dir.join("optimized.so"),
                                    ) {
                                        Ok(lib) => PgoState::Optimized(lib),
                                        Err(_) => PgoState::CompilationFailed,
                                    };
                                }
                                None => {}
                            }
                        } else {
                            match comp_info.ctx.groups.get_mut(comp_info.group_name) {
                                Some(mut group) => {
                                    group.pgo_state = PgoState::CompilationFailed;
                                }
                                None => {}
                            }
                        }
                    }
                    Err(_) => match comp_info.ctx.groups.get_mut(comp_info.group_name) {
                        Some(mut group) => {
                            group.pgo_state = PgoState::CompilationFailed;
                        }
                        None => {}
                    },
                }
            }
        }
    }

    // We need to identify a graceful shutdown but for now... we will jsut die
    panic!("PGO Worker failed on an error");
}

pub enum PGORequest {
    Initial(PGOCompilationInfo),
    Optimized(PGOCompilationInfo),
}

pub struct PGOCompilationInfo {
    ctx: &'static PogoFuncCtx,
    group_name: &'static str,
}

pub trait PogoGroup {
    const USE_PGO: bool = true;
    const NAME: &'static str;
    const PGO_EXEC_COUNT: usize;
}

pub struct Global;
impl PogoGroup for Global {
    const NAME: &'static str = "__POGO_GLOBAL";
    const PGO_EXEC_COUNT: usize = 5_000;
}

pub struct NoPGO;
impl PogoGroup for NoPGO {
    const USE_PGO: bool = false;
    const NAME: &'static str = "__NO_PGO";
    const PGO_EXEC_COUNT: usize = 0;
}
