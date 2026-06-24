//! The AOT driver uses [`cranelift_object`] to write object files suitable for linking into a
//! standalone executable.

use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

use cranelift_object::{ObjectBuilder, ObjectModule};
use rustc_codegen_ssa::assert_module_sources::CguReuse;
use rustc_codegen_ssa::back::write::produce_final_output_artifacts;
use rustc_codegen_ssa::base::determine_cgu_reuse;
use rustc_codegen_ssa::{CompiledModule, CompiledModules, ModuleKind};
use rustc_data_structures::profiling::SelfProfilerRef;
use rustc_data_structures::stable_hash::{StableHash, StableHashCtxt, StableHasher};
use rustc_data_structures::sync::{IntoDynSyncSend, par_map};
use rustc_hir::attrs::Linkage as RLinkage;
use rustc_middle::dep_graph::{WorkProduct, WorkProductId};
use rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags;
use rustc_middle::mono::{CodegenUnit, InstantiationMode, MonoItem, MonoItemData, Visibility};
use rustc_session::Session;
use rustc_session::config::{OutputFilenames, OutputType};
use rustc_span::Symbol;

use crate::base::CodegenedFunction;
use crate::concurrency_limiter::{ConcurrencyLimiter, ConcurrencyLimiterToken};
use crate::debuginfo::TypeDebugContext;
use crate::global_asm::{GlobalAsmConfig, GlobalAsmContext};
use crate::prelude::*;
use crate::unwind_module::UnwindModule;

fn disable_incr_cache() -> bool {
    env::var("CG_CLIF_DISABLE_INCR_CACHE").as_deref() == Ok("1")
}

struct ModuleCodegenResult {
    module_regular: CompiledModule,
    module_global_asm: Option<CompiledModule>,
}

enum OngoingModuleCodegen {
    Sync(Result<ModuleCodegenResult, String>),
    Async(JoinHandle<Result<ModuleCodegenResult, String>>),
}

impl StableHash for OngoingModuleCodegen {
    fn stable_hash<Hcx: StableHashCtxt>(&self, _: &mut Hcx, _: &mut StableHasher) {
        // do nothing
    }
}

pub(crate) struct OngoingCodegen {
    modules: Vec<OngoingModuleCodegen>,
    allocator_module: Option<CompiledModule>,
    concurrency_limiter: ConcurrencyLimiter,
}

impl OngoingCodegen {
    pub(crate) fn join(
        self,
        sess: &Session,
        outputs: &OutputFilenames,
    ) -> (CompiledModules, FxIndexMap<WorkProductId, WorkProduct>) {
        let mut work_products = FxIndexMap::default();
        let mut modules = vec![];
        let disable_incr_cache = disable_incr_cache();

        for module_codegen in self.modules {
            let module_codegen_result = match module_codegen {
                OngoingModuleCodegen::Sync(module_codegen_result) => module_codegen_result,
                OngoingModuleCodegen::Async(join_handle) => match join_handle.join() {
                    Ok(module_codegen_result) => module_codegen_result,
                    Err(panic) => std::panic::resume_unwind(panic),
                },
            };

            let module_codegen_result = match module_codegen_result {
                Ok(module_codegen_result) => module_codegen_result,
                Err(err) => sess.dcx().fatal(err),
            };
            let ModuleCodegenResult { mut module_regular, module_global_asm } =
                module_codegen_result;

            let work_product = if disable_incr_cache {
                None
            } else if let Some(module_global_asm) = &module_global_asm {
                rustc_incremental::copy_cgu_workproduct_to_incr_comp_cache_dir(
                    sess,
                    &module_regular.name,
                    &[
                        ("o", module_regular.object.as_ref().unwrap()),
                        ("asm.o", module_global_asm.object.as_ref().unwrap()),
                    ],
                    &[
                        module_regular.links_from_incr_cache.as_slice(),
                        module_global_asm.links_from_incr_cache.as_slice(),
                    ]
                    .concat(),
                    module_regular.object_digest.as_deref(),
                )
            } else {
                rustc_incremental::copy_cgu_workproduct_to_incr_comp_cache_dir(
                    sess,
                    &module_regular.name,
                    &[("o", module_regular.object.as_ref().unwrap())],
                    &module_regular.links_from_incr_cache,
                    module_regular.object_digest.as_deref(),
                )
            };
            if let Some((work_product_id, work_product, object_digest)) = work_product {
                module_regular.object_digest = object_digest;
                work_products.insert(work_product_id, work_product);
            }

            modules.push(module_regular);
            if let Some(module_global_asm) = module_global_asm {
                modules.push(module_global_asm);
            }
        }

        self.concurrency_limiter.finished();

        sess.dcx().abort_if_errors();

        let compiled_modules = CompiledModules { modules, allocator_module: self.allocator_module };

        produce_final_output_artifacts(sess, &compiled_modules, outputs);

        (compiled_modules, work_products)
    }
}

fn make_module(sess: &Session, name: String) -> UnwindModule<ObjectModule> {
    let isa = crate::build_isa(sess, false);

    let mut builder =
        ObjectBuilder::new(isa, name + ".o", cranelift_module::default_libcall_names()).unwrap();

    // Disable function sections by default on MSVC as it causes significant slowdowns with link.exe.
    // Maybe link.exe has exponential behavior when there are many sections with the same name? Also
    // explicitly disable it on MinGW as rustc already disables it by default on MinGW and as such
    // isn't tested. If rustc enables it in the future on MinGW, we can re-enable it too once it has
    // been on MinGW.
    let default_function_sections = sess.target.function_sections && !sess.target.is_like_windows;
    builder.per_function_section(
        sess.opts.unstable_opts.function_sections.unwrap_or(default_function_sections),
    );

    UnwindModule::new(ObjectModule::new(builder), true)
}

fn emit_cgu(
    output_filenames: &OutputFilenames,
    prof: &SelfProfilerRef,
    name: String,
    module: UnwindModule<ObjectModule>,
    debug: Option<DebugContext>,
    global_asm_object_file: Option<PathBuf>,
    producer: &str,
) -> Result<ModuleCodegenResult, String> {
    let mut product = module.finish();

    if let Some(mut debug) = debug {
        debug.emit(&mut product);
    }

    let module_regular = emit_module(
        output_filenames,
        prof,
        product.object,
        ModuleKind::Regular,
        name.clone(),
        producer,
    )?;

    Ok(ModuleCodegenResult {
        module_regular,
        module_global_asm: global_asm_object_file.map(|global_asm_object_file| CompiledModule {
            name: format!("{name}.asm"),
            kind: ModuleKind::Regular,
            object: Some(global_asm_object_file),
            dwarf_object: None,
            bytecode: None,
            assembly: None,
            llvm_ir: None,
            links_from_incr_cache: Vec::new(),
            object_digest: None,
        }),
    })
}

fn emit_module(
    output_filenames: &OutputFilenames,
    prof: &SelfProfilerRef,
    mut object: cranelift_object::object::write::Object<'_>,
    kind: ModuleKind,
    name: String,
    producer_str: &str,
) -> Result<CompiledModule, String> {
    if object.format() == cranelift_object::object::BinaryFormat::Elf {
        let comment_section = object.add_section(
            Vec::new(),
            b".comment".to_vec(),
            cranelift_object::object::SectionKind::OtherString,
        );
        let mut producer = vec![0];
        producer.extend(producer_str.as_bytes());
        producer.push(0);
        object.set_section_data(comment_section, producer, 1);
    }

    let tmp_file = output_filenames.temp_path_for_cgu(OutputType::Object, &name);
    let file = match File::create(&tmp_file) {
        Ok(file) => file,
        Err(err) => return Err(format!("error creating object file: {}", err)),
    };

    let mut file = BufWriter::new(file);
    if let Err(err) = object.write_stream(&mut file) {
        return Err(format!("error writing object file: {}", err));
    }
    let file = match file.into_inner() {
        Ok(file) => file,
        Err(err) => return Err(format!("error writing object file: {}", err)),
    };

    if prof.enabled() {
        prof.artifact_size(
            "object_file",
            tmp_file.file_name().unwrap().to_string_lossy(),
            file.metadata().unwrap().len(),
        );
    }

    Ok(CompiledModule {
        name,
        kind,
        object: Some(tmp_file),
        dwarf_object: None,
        bytecode: None,
        assembly: None,
        llvm_ir: None,
        links_from_incr_cache: Vec::new(),
        object_digest: None,
    })
}

fn reuse_workproduct_for_cgu(
    tcx: TyCtxt<'_>,
    cgu: &CodegenUnit<'_>,
) -> Result<ModuleCodegenResult, String> {
    let work_product = cgu.previous_work_product(tcx);
    let obj_out_regular =
        tcx.output_filenames(()).temp_path_for_cgu(OutputType::Object, cgu.name().as_str());
    let source_file_regular = rustc_incremental::in_incr_comp_dir_sess(
        tcx.sess,
        work_product.saved_files.get("o").expect("no saved object file in work product"),
    );

    if let Err(err) = rustc_fs_util::link_or_copy(&source_file_regular, &obj_out_regular) {
        return Err(format!(
            "unable to copy {} to {}: {}",
            source_file_regular.display(),
            obj_out_regular.display(),
            err
        ));
    }

    let obj_out_global_asm =
        crate::global_asm::add_file_stem_postfix(obj_out_regular.clone(), ".asm");
    let source_file_global_asm = if let Some(asm_o) = work_product.saved_files.get("asm.o") {
        let source_file_global_asm = rustc_incremental::in_incr_comp_dir_sess(tcx.sess, asm_o);
        if let Err(err) = rustc_fs_util::link_or_copy(&source_file_global_asm, &obj_out_global_asm)
        {
            return Err(format!(
                "unable to copy {} to {}: {}",
                source_file_global_asm.display(),
                obj_out_global_asm.display(),
                err
            ));
        }
        Some(source_file_global_asm)
    } else {
        None
    };

    Ok(ModuleCodegenResult {
        module_regular: CompiledModule {
            name: cgu.name().to_string(),
            kind: ModuleKind::Regular,
            object: Some(obj_out_regular),
            dwarf_object: None,
            bytecode: None,
            assembly: None,
            llvm_ir: None,
            links_from_incr_cache: vec![source_file_regular.clone()],
            object_digest: rustc_incremental::read_sld_cgu_object_digest(
                &source_file_regular,
                &work_product,
            ),
        },
        module_global_asm: source_file_global_asm.map(|source_file| CompiledModule {
            name: cgu.name().to_string(),
            kind: ModuleKind::Regular,
            object: Some(obj_out_global_asm),
            dwarf_object: None,
            bytecode: None,
            assembly: None,
            llvm_ir: None,
            links_from_incr_cache: vec![source_file],
            object_digest: None,
        }),
    })
}

fn codegen_cgu_content<'tcx>(
    tcx: TyCtxt<'tcx>,
    module: &mut dyn Module,
    cgu_name: rustc_span::Symbol,
    object_private_definitions: &FxHashMap<Instance<'tcx>, ()>,
) -> (
    Option<DebugContext>,
    TypeDebugContext<'tcx>,
    Vec<CodegenedFunction>,
    FxHashMap<FuncId, Instance<'tcx>>,
    String,
) {
    let _timer = tcx.prof.generic_activity_with_arg("codegen cgu", cgu_name.as_str());

    let cgu = tcx.codegen_unit(cgu_name);
    let mono_items = cgu.items_in_deterministic_order(tcx);

    let mut debug_context = DebugContext::new(tcx, module.isa(), false, cgu_name.as_str());
    let mut global_asm = String::new();
    let mut type_dbg = TypeDebugContext::default();
    super::predefine_mono_items(tcx, module, &mono_items);
    let mut codegened_functions = vec![];
    let mut referenced_functions = FxHashMap::default();
    for (mono_item, item_data) in mono_items {
        match mono_item {
            MonoItem::Fn(instance) => {
                let flags = tcx.codegen_instance_attrs(instance.def).flags;
                if flags.contains(CodegenFnAttrFlags::NAKED) {
                    rustc_codegen_ssa::mir::naked_asm::codegen_naked_asm(
                        &mut GlobalAsmContext { tcx, global_asm: &mut global_asm },
                        instance,
                        MonoItemData {
                            linkage: RLinkage::External,
                            visibility: if item_data.linkage == RLinkage::Internal {
                                Visibility::Hidden
                            } else {
                                item_data.visibility
                            },
                            ..item_data
                        },
                    );
                    continue;
                }
                let codegened_function = crate::base::codegen_fn(
                    tcx,
                    cgu_name,
                    debug_context.as_mut(),
                    &mut type_dbg,
                    Function::new(),
                    module,
                    instance,
                    &mut referenced_functions,
                );
                codegened_functions.push(codegened_function);
            }
            MonoItem::Static(def_id) => {
                let data_id =
                    crate::constant::codegen_static(tcx, module, def_id, &mut referenced_functions);
                if let Some(debug_context) = debug_context.as_mut() {
                    debug_context.define_static(tcx, &mut type_dbg, def_id, data_id);
                }
            }
            MonoItem::GlobalAsm(item_id) => {
                rustc_codegen_ssa::base::codegen_global_asm(
                    &mut GlobalAsmContext { tcx, global_asm: &mut global_asm },
                    item_id,
                );
            }
        }
    }

    materialize_referenced_functions(
        tcx,
        module,
        cgu_name,
        &mut debug_context,
        &mut type_dbg,
        &mut codegened_functions,
        &mut referenced_functions,
        false,
        object_private_definitions,
    );
    crate::main_shim::maybe_create_entry_wrapper(tcx, module, false, cgu.is_primary());

    (debug_context, type_dbg, codegened_functions, referenced_functions, global_asm)
}

/// Define functions that Cranelift leaves referenced without another available definition.
fn materialize_referenced_functions<'tcx>(
    tcx: TyCtxt<'tcx>,
    module: &mut dyn Module,
    cgu_name: Symbol,
    debug_context: &mut Option<DebugContext>,
    type_dbg: &mut TypeDebugContext<'tcx>,
    codegened_functions: &mut Vec<CodegenedFunction>,
    referenced_functions: &mut FxHashMap<FuncId, Instance<'tcx>>,
    materialize_post_monomorphization_references: bool,
    object_private_definitions: &FxHashMap<Instance<'tcx>, ()>,
) {
    // Optimized Rust compilation permits LocalCopy instances and conflict-mangled global copies
    // to be omitted from the CGU on the assumption that the backend will inline every reference.
    // Cranelift may deliberately retain a call or function address, so materialize their
    // transitive closure. References introduced by the post-monomorphization inliner are too late
    // for CGU partitioning, so also materialize available non-external item definitions there.
    let mut defined_functions = codegened_functions
        .iter()
        .map(|function| (function.func_id, ()))
        .collect::<FxHashMap<_, _>>();
    // An ordinary function address can reference a definition that CGU partitioning made private
    // to another object. Materialize those references before post-monomorphization processing.
    // A fallback can also reveal drop glue too late for CGU partitioning; make that glue and its
    // dependencies a self-contained local closure. Late ordinary references retain the narrower
    // existing HiddenWeak path so they do not clone a large transitive closure into every CGU.
    let mut private_drop_glue_closure = FxHashMap::default();
    loop {
        let mut pending = referenced_functions
            .iter()
            .filter_map(|(&func_id, &instance)| {
                let has_object_private_definition =
                    object_private_definitions.contains_key(&instance);
                let materialize_private_definition =
                    !materialize_post_monomorphization_references && has_object_private_definition;
                let materialize_private_drop_glue = private_drop_glue_closure
                    .contains_key(&func_id)
                    || matches!(instance.def, InstanceKind::DropGlue(_, Some(_)))
                        && has_object_private_definition;
                let can_materialize_late_reference = (materialize_post_monomorphization_references
                    || materialize_private_definition
                    || materialize_private_drop_glue)
                    && match instance.def {
                        InstanceKind::Item(_) => tcx.is_mir_available(instance.def_id()),
                        InstanceKind::DropGlue(_, Some(_)) => true,
                        _ => false,
                    }
                    && {
                        let attrs = tcx.codegen_instance_attrs(instance.def);
                        !attrs.contains_extern_indicator()
                            && !attrs.flags.contains(CodegenFnAttrFlags::NAKED)
                    };
                (!defined_functions.contains_key(&func_id)
                    && (can_materialize_late_reference
                        || matches!(
                            MonoItem::Fn(instance).instantiation_mode(tcx),
                            InstantiationMode::LocalCopy
                                | InstantiationMode::GloballyShared { may_conflict: true }
                        )))
                .then_some((
                    func_id,
                    instance,
                    materialize_private_definition,
                    materialize_private_drop_glue,
                ))
            })
            .collect::<Vec<_>>();
        pending.sort_unstable_by_key(|(func_id, _, _, _)| func_id.as_u32());
        if pending.is_empty() {
            break;
        }

        for (
            func_id,
            instance,
            materializing_private_definition,
            materializing_private_drop_glue,
        ) in pending
        {
            defined_functions.insert(func_id, ());
            let name = instance_symbol_name_for_object(tcx, instance);
            let sig = get_function_sig(tcx, module.target_config().default_call_conv, instance);
            // See `predefine_mono_items`: hidden-weak Mach-O functions can retain stale unwind
            // metadata after the linker coalesces their text atoms.
            let macho_unwinding = cfg!(feature = "unwinding")
                && module.isa().triple().binary_format == target_lexicon::BinaryFormat::Macho;
            let linkage = if materializing_private_definition
                || materializing_private_drop_glue
                || macho_unwinding
            {
                Linkage::Local
            } else {
                Linkage::HiddenWeak
            };
            let declared_func_id = module.declare_function(&name, linkage, &sig).unwrap();
            debug_assert_eq!(declared_func_id, func_id);
            let mut newly_referenced_functions = FxHashMap::default();
            let function = crate::base::codegen_fn(
                tcx,
                cgu_name,
                debug_context.as_mut(),
                type_dbg,
                Function::new(),
                module,
                instance,
                &mut newly_referenced_functions,
            );
            for (referenced_func_id, referenced_instance) in newly_referenced_functions {
                referenced_functions.entry(referenced_func_id).or_insert(referenced_instance);
                if materializing_private_drop_glue {
                    private_drop_glue_closure.insert(referenced_func_id, ());
                }
            }
            codegened_functions.push(function);
        }
    }
}

fn object_private_function_definitions<'tcx>(
    cgus: &[CodegenUnit<'tcx>],
) -> FxHashMap<Instance<'tcx>, ()> {
    let mut definitions = FxHashMap::default();
    for cgu in cgus {
        for (&mono_item, data) in cgu.items() {
            let MonoItem::Fn(instance) = mono_item else {
                continue;
            };
            let is_object_private = data.linkage == RLinkage::Internal && !data.inlined;
            definitions
                .entry(instance)
                .and_modify(|private| *private &= is_object_private)
                .or_insert(is_object_private);
        }
    }
    definitions.retain(|_, private| *private);
    definitions.into_iter().map(|(instance, _)| (instance, ())).collect()
}

fn module_codegen<'tcx>(
    tcx: TyCtxt<'tcx>,
    global_asm_config: Arc<GlobalAsmConfig>,
    cgu_name: rustc_span::Symbol,
    token: ConcurrencyLimiterToken,
    object_private_definitions: &FxHashMap<Instance<'tcx>, ()>,
) -> OngoingModuleCodegen {
    let mut module = make_module(tcx.sess, cgu_name.as_str().to_string());

    let (
        mut debug_context,
        mut type_dbg,
        mut codegened_functions,
        referenced_functions,
        mut global_asm,
    ) = codegen_cgu_content(tcx, &mut module, cgu_name, object_private_definitions);

    let mut inline_catalog_module =
        make_module(tcx.sess, format!("{}.inline-catalog", cgu_name.as_str()));
    let (post_monomorphization_candidates, mut post_monomorphization_references) =
        match crate::base::codegen_post_monomorphization_inline_candidates(
            tcx,
            cgu_name,
            &mut module,
            &mut inline_catalog_module,
            referenced_functions,
        ) {
            Ok(candidates) => candidates,
            Err(err) => tcx
                .dcx()
                .fatal(format!("failed to prepare post-monomorphization inline candidates: {err}")),
        };

    if let Err(err) = tcx.prof.generic_activity("inline clif functions").run(|| {
        crate::base::inline_small_functions(
            &module,
            &post_monomorphization_candidates,
            &mut codegened_functions,
        )
    }) {
        tcx.dcx().fatal(format!("failed to inline Cranelift functions: {err}"));
    }
    materialize_referenced_functions(
        tcx,
        &mut module,
        cgu_name,
        &mut debug_context,
        &mut type_dbg,
        &mut codegened_functions,
        &mut post_monomorphization_references,
        true,
        object_private_definitions,
    );

    let cgu_name = cgu_name.as_str().to_owned();

    let producer = crate::debuginfo::producer(tcx.sess);

    let profiler = tcx.prof.clone();
    let output_filenames = tcx.output_filenames(()).clone();
    let should_write_ir = crate::pretty_clif::should_write_ir(tcx.sess);

    OngoingModuleCodegen::Async(std::thread::spawn(move || {
        profiler.clone().generic_activity_with_arg("compile functions", &*cgu_name).run(|| {
            cranelift_codegen::timing::set_thread_profiler(Box::new(super::MeasuremeProfiler(
                profiler.clone(),
            )));

            let mut cached_context = Context::new();
            for codegened_func in codegened_functions {
                crate::base::compile_fn(
                    &profiler,
                    &output_filenames,
                    should_write_ir,
                    &mut cached_context,
                    &mut module,
                    debug_context.as_mut(),
                    &mut global_asm,
                    codegened_func,
                );
            }
        });

        let global_asm_object_file =
            profiler.generic_activity_with_arg("compile assembly", &*cgu_name).run(|| {
                crate::global_asm::compile_global_asm(&global_asm_config, &cgu_name, global_asm)
            })?;

        let codegen_result =
            profiler.generic_activity_with_arg("write object file", &*cgu_name).run(|| {
                emit_cgu(
                    &global_asm_config.output_filenames,
                    &profiler,
                    cgu_name,
                    module,
                    debug_context,
                    global_asm_object_file,
                    &producer,
                )
            });
        std::mem::drop(token);
        codegen_result
    }))
}

fn emit_allocator_module(tcx: TyCtxt<'_>) -> Option<CompiledModule> {
    let mut allocator_module = make_module(tcx.sess, "allocator_shim".to_string());
    let created_alloc_shim = crate::allocator::codegen(tcx, &mut allocator_module);

    if created_alloc_shim {
        let product = allocator_module.finish();

        match emit_module(
            tcx.output_filenames(()),
            &tcx.sess.prof,
            product.object,
            ModuleKind::Allocator,
            "allocator_shim".to_owned(),
            &crate::debuginfo::producer(tcx.sess),
        ) {
            Ok(allocator_module) => Some(allocator_module),
            Err(err) => tcx.dcx().fatal(err),
        }
    } else {
        None
    }
}

pub(crate) fn run_aot(tcx: TyCtxt<'_>) -> Box<OngoingCodegen> {
    let cgus = tcx.collect_and_partition_mono_items(()).codegen_units;
    let object_private_definitions = object_private_function_definitions(cgus);

    if tcx.dep_graph.is_fully_enabled() {
        for cgu in cgus {
            tcx.ensure_ok().codegen_unit(cgu.name());
        }
    }

    // Calculate the CGU reuse
    let cgu_reuse = tcx.sess.time("find_cgu_reuse", || {
        cgus.iter().map(|cgu| determine_cgu_reuse(tcx, cgu)).collect::<Vec<_>>()
    });

    rustc_codegen_ssa::assert_module_sources::assert_module_sources(tcx, &|cgu_reuse_tracker| {
        for (i, cgu) in cgus.iter().enumerate() {
            let cgu_reuse = cgu_reuse[i];
            cgu_reuse_tracker.set_actual_reuse(cgu.name().as_str(), cgu_reuse);
        }
    });

    let global_asm_config = Arc::new(crate::global_asm::GlobalAsmConfig::new(tcx));

    let disable_incr_cache = disable_incr_cache();
    let (todo_cgus, done_cgus) =
        cgus.iter().enumerate().partition::<Vec<_>, _>(|&(i, _)| match cgu_reuse[i] {
            _ if disable_incr_cache => true,
            CguReuse::No => true,
            CguReuse::PreLto | CguReuse::PostLto => false,
        });

    let concurrency_limiter = IntoDynSyncSend(ConcurrencyLimiter::new(todo_cgus.len()));

    let modules: Vec<_> =
        tcx.sess.time("codegen mono items", || {
            let modules: Vec<_> = par_map(todo_cgus, |(_, cgu)| {
                let dep_node = cgu.codegen_dep_node(tcx);
                let (module, _) = tcx.dep_graph.with_task(
                    dep_node,
                    tcx,
                    || {
                        module_codegen(
                            tcx,
                            global_asm_config.clone(),
                            cgu.name(),
                            concurrency_limiter.acquire(tcx.dcx()),
                            &object_private_definitions,
                        )
                    },
                    Some(rustc_middle::dep_graph::hash_result),
                );
                IntoDynSyncSend(module)
            });
            modules
                .into_iter()
                .map(|module| module.0)
                .chain(done_cgus.into_iter().map(|(_, cgu)| {
                    OngoingModuleCodegen::Sync(reuse_workproduct_for_cgu(tcx, cgu))
                }))
                .collect()
        });

    let allocator_module = emit_allocator_module(tcx);

    Box::new(OngoingCodegen {
        modules,
        allocator_module,
        concurrency_limiter: concurrency_limiter.0,
    })
}
