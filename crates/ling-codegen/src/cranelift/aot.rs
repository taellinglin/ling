use super::numtype;
use super::translate::{build_function_body, max_local_index, TransCtx};
use crate::CodegenBackend;
use crate::MirProgram;
use anyhow::Result;
use cranelift::codegen::ir::{FuncRef, GlobalValue};
use cranelift::prelude::*;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::DataId;
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use ling_mir::ir::*;
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;

// ─── AOT Backend ───────────────────────────────────────────────────────────

pub struct CraneliftBackend {
    module: Option<ObjectModule>,
    builder_ctx: FunctionBuilderContext,
    progress: bool,
}

/// Render a single-line tqdm-style progress bar to stderr (overwriting in place
/// with `\r`). Colors follow the Ling palette: teal fill on a grey track.
fn render_progress(done: usize, total: usize, label: &str) {
    use std::io::Write as _;
    const WIDTH: usize = 28;
    let frac = if total == 0 {
        1.0
    } else {
        done as f64 / total as f64
    };
    let filled = ((frac * WIDTH as f64).round() as usize).min(WIDTH);
    let bar_full = "█".repeat(filled);
    let bar_empty = "░".repeat(WIDTH - filled);
    let pct = (frac * 100.0) as u32;
    // truncate the label so the line never wraps the terminal
    let label: String = label.chars().take(24).collect();
    let mut err = std::io::stderr();
    let _ = write!(
        err,
        "\r    compiling \x1b[38;5;37m{bar_full}\x1b[38;5;240m{bar_empty}\x1b[0m {pct:>3}% [{done}/{total}] {label:<24}",
    );
    let _ = err.flush();
}

// ─── Runtime Function Declarations ─────────────────────────────────────────

struct RuntimeDecl {
    id: FuncId,
}

fn declare_runtime_functions(module: &mut ObjectModule) -> HashMap<String, RuntimeDecl> {
    let mut decls = HashMap::new();

    let runtime_fns: &[(&str, &[types::Type], types::Type)] = &[
        ("ling_f64_add", &[types::F64, types::F64], types::F64),
        ("ling_f64_sub", &[types::F64, types::F64], types::F64),
        ("ling_f64_mul", &[types::F64, types::F64], types::F64),
        ("ling_f64_div", &[types::F64, types::F64], types::F64),
        ("ling_f64_rem", &[types::F64, types::F64], types::F64),
        ("ling_f64_neg", &[types::F64], types::F64),
        ("ling_f64_eq", &[types::F64, types::F64], types::I64),
        ("ling_f64_lt", &[types::F64, types::F64], types::I64),
        ("ling_f64_gt", &[types::F64, types::F64], types::I64),
        ("ling_f64_le", &[types::F64, types::F64], types::I64),
        ("ling_f64_ge", &[types::F64, types::F64], types::I64),
        ("ling_sin", &[types::F64], types::F64),
        ("ling_cos", &[types::F64], types::F64),
        ("ling_sqrt", &[types::F64], types::F64),
        ("ling_abs", &[types::F64], types::F64),
        ("ling_floor", &[types::F64], types::F64),
        ("ling_ceil", &[types::F64], types::F64),
        ("ling_round", &[types::F64], types::F64),
        ("ling_add", &[types::I64, types::I64], types::I64),
        ("ling_sub", &[types::I64, types::I64], types::I64),
        ("ling_mul", &[types::I64, types::I64], types::I64),
        ("ling_div", &[types::I64, types::I64], types::I64),
        ("ling_rem", &[types::I64, types::I64], types::I64),
        ("ling_neg", &[types::I64, types::I64], types::I64),
        ("ling_eq", &[types::I64, types::I64], types::I64),
        ("ling_ne", &[types::I64, types::I64], types::I64),
        ("ling_lt", &[types::I64, types::I64], types::I64),
        ("ling_le", &[types::I64, types::I64], types::I64),
        ("ling_gt", &[types::I64, types::I64], types::I64),
        ("ling_ge", &[types::I64, types::I64], types::I64),
        ("ling_and", &[types::I64, types::I64], types::I64),
        ("ling_or", &[types::I64, types::I64], types::I64),
        ("ling_not", &[types::I64], types::I64),
        ("ling_bool_to_u64", &[types::I64], types::I64),
        ("ling_alloc", &[types::I64], types::I64),
        ("ling_free", &[types::I64], types::I64),
        ("ling_panic", &[types::I64], types::I64),
        ("ling_str_new", &[types::I64, types::I64], types::I64),
        ("ling_str_len", &[types::I64], types::I64),
        ("ling_str_concat", &[types::I64, types::I64], types::I64),
        ("ling_str_eq", &[types::I64, types::I64], types::I64),
        ("ling_list_new", &[], types::I64),
        ("ling_list_push", &[types::I64, types::I64], types::I64),
        ("ling_list_get", &[types::I64, types::I64], types::I64),
        ("ling_list_len", &[types::I64], types::I64),
        (
            "ling_struct_new",
            &[types::I64, types::I64, types::I64, types::I64],
            types::I64,
        ),
        (
            "ling_struct_get",
            &[types::I64, types::I64, types::I64],
            types::I64,
        ),
        ("ling_print", &[types::I64], types::I64),
        ("ling_print_val", &[types::I64], types::I64),
        ("ling_print_newline", &[], types::I64),
        ("ling_time_now", &[], types::I64),
        (
            "ling_builtin",
            &[types::I64, types::I64, types::I64, types::I64],
            types::I64,
        ),
        // Kernel runtime intrinsics (no_std)
        ("ling_kernel_vga_write_str", &[types::I64], types::I64),
        ("ling_kernel_vga_write_char", &[types::I64], types::I64),
        ("ling_kernel_vga_clear", &[], types::I64),
        (
            "ling_kernel_serial_write",
            &[types::I64, types::I64],
            types::I64,
        ),
        ("ling_kernel_halt", &[], types::I64),
        ("ling_kernel_cli", &[], types::I64),
        ("ling_kernel_sti", &[], types::I64),
        ("ling_kernel_inb", &[types::I64], types::I64),
        ("ling_kernel_outb", &[types::I64, types::I64], types::I64),
        ("ling_kernel_panic", &[types::I64], types::I64),
        ("ling_kernel_init", &[], types::I64),
        (
            "ling_kernel_cpuid",
            &[types::I64, types::I64, types::I64, types::I64],
            types::I64,
        ),
        (
            "ling_kernel_asm_exec",
            &[types::I64, types::I64],
            types::I64,
        ),
        ("ling_kernel_kbd_read_char", &[], types::I64),
        ("ling_kernel_kbd_poll_char", &[], types::I64),
        (
            "ling_kernel_vga_set_color",
            &[types::I64, types::I64],
            types::I64,
        ),
        ("ling_kernel_fs_selftest", &[], types::I64),
        ("ling_kernel_read_line", &[], types::I64),
        (
            "ling_kernel_gui_read_line",
            &[types::I64, types::I64, types::I64, types::I64],
            types::I64,
        ),
        (
            "ling_kernel_gui_read_line_masked",
            &[types::I64, types::I64, types::I64, types::I64],
            types::I64,
        ),
        (
            "ling_kernel_gui_read_password_confirmed",
            &[
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
            ],
            types::I64,
        ),
        ("ling_kernel_print_lstr", &[types::I64], types::I64),
        ("ling_kernel_fs_mount", &[], types::I64),
        ("ling_kernel_fs_ls", &[types::I64], types::I64),
        (
            "ling_kernel_fs_write",
            &[types::I64, types::I64],
            types::I64,
        ),
        ("ling_kernel_fs_read", &[types::I64], types::I64),
        ("ling_kernel_str_cmd", &[types::I64], types::I64),
        ("ling_kernel_str_arg", &[types::I64], types::I64),
        ("ling_kernel_theme_dark", &[], types::I64),
        ("ling_kernel_theme_light", &[], types::I64),
        ("ling_kernel_fb_available", &[], types::I64),
        ("ling_kernel_fb_width", &[], types::I64),
        ("ling_kernel_ui_x", &[types::I64], types::I64),
        ("ling_kernel_ui_y", &[types::I64], types::I64),
        ("ling_kernel_ui_len", &[types::I64], types::I64),
        ("ling_kernel_fb_height", &[], types::I64),
        (
            "ling_kernel_fb_set_pixel",
            &[types::I64, types::I64, types::I64],
            types::I64,
        ),
        (
            "ling_kernel_fb_fill_rect",
            &[types::I64, types::I64, types::I64, types::I64, types::I64],
            types::I64,
        ),
        ("ling_kernel_fb_clear", &[types::I64], types::I64),
        ("ling_kernel_fb_back_clear", &[types::I64], types::I64),
        (
            "ling_kernel_fb_back_fill_rect",
            &[types::I64, types::I64, types::I64, types::I64, types::I64],
            types::I64,
        ),
        ("ling_kernel_fb_present", &[], types::I64),
        (
            "ling_kernel_fb_draw_str",
            &[types::I64, types::I64, types::I64, types::I64, types::I64],
            types::I64,
        ),
        (
            "ling_kernel_fb_draw_utf8_str",
            &[
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
            ],
            types::I64,
        ),
        (
            "ling_kernel_wm_liquid_step",
            &[types::I64, types::I64],
            types::I64,
        ),
        ("ling_kernel_wm_liquid_x", &[], types::I64),
        ("ling_kernel_wm_liquid_y", &[], types::I64),
        ("ling_kernel_wm_liquid_scale_w_pct", &[], types::I64),
        ("ling_kernel_wm_liquid_scale_h_pct", &[], types::I64),
        ("ling_kernel_pkg_install", &[types::I64], types::I64),
        ("ling_kernel_pkg_install_module", &[], types::I64),
        ("ling_kernel_life_run", &[], types::I64),
        ("ling_kernel_mouse_init", &[], types::I64),
        (
            "ling_kernel_mouse_set_bounds",
            &[types::I64, types::I64],
            types::I64,
        ),
        ("ling_kernel_mouse_poll", &[], types::I64),
        ("ling_kernel_mouse_x", &[], types::I64),
        ("ling_kernel_mouse_y", &[], types::I64),
        ("ling_kernel_mouse_buttons", &[], types::I64),
        ("ling_kernel_bootmodule_addr", &[], types::I64),
        ("ling_kernel_bootmodule_size", &[], types::I64),
        (
            "ling_kernel_disk_write_raw",
            &[types::I64, types::I64, types::I64],
            types::I64,
        ),
        (
            "ling_kernel_user_create",
            &[
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
            ],
            types::I64,
        ),
        (
            "ling_kernel_user_login",
            &[types::I64, types::I64],
            types::I64,
        ),
        (
            "ling_kernel_user_set_password",
            &[types::I64, types::I64],
            types::I64,
        ),
        (
            "ling_kernel_user_set_group",
            &[types::I64, types::I64],
            types::I64,
        ),
        ("ling_kernel_whoami", &[], types::I64),
        ("ling_kernel_user_group", &[], types::I64),
        ("ling_kernel_user_is_wheel", &[], types::I64),
        ("ling_kernel_cd", &[types::I64], types::I64),
        ("ling_kernel_cwd", &[], types::I64),
        ("ling_kernel_read_line_masked", &[], types::I64),
        ("ling_kernel_read_line_masked_confirmed", &[], types::I64),
        (
            "ling_kernel_ed25519_verify",
            &[types::I64, types::I64, types::I64],
            types::I64,
        ),
        // Cooperative scheduler (proc::sched) -- these existed as real,
        // working Rust intrinsics in ling-kernel but were never added here,
        // so any `.ling` call to them was silently dropped at compile time
        // (undeclared-symbol calls don't error, they just don't get a
        // func_refs entry -- see `declare_runtime_functions`'s caller).
        ("ling_kernel_spawn", &[types::I64], types::I64),
        ("ling_kernel_yield", &[], types::I64),
        ("ling_kernel_exit", &[], types::I64),
        ("ling_kernel_getpid", &[], types::I64),
        ("ling_kernel_proc_selftest", &[], types::I64),
        ("ling_kernel_proc_pairtest", &[], types::I64),
        ("ling_kernel_proc_ps", &[], types::I64),
        ("ling_kernel_uptime_ms", &[], types::I64),
        ("ling_kernel_rtc_unix_ts", &[], types::I64),
        ("ling_kernel_rtc_year", &[], types::I64),
        ("ling_kernel_rtc_month", &[], types::I64),
        ("ling_kernel_rtc_day", &[], types::I64),
        ("ling_kernel_rtc_hour", &[], types::I64),
        ("ling_kernel_rtc_minute", &[], types::I64),
        ("ling_kernel_rtc_second", &[], types::I64),
        ("ling_kernel_locale_count", &[], types::I64),
        ("ling_kernel_locale_id", &[types::I64], types::I64),
        ("ling_kernel_locale_native_name", &[types::I64], types::I64),
        ("ling_kernel_locale_latin_name", &[types::I64], types::I64),
        ("ling_kernel_locale_utc_offset_min", &[types::I64], types::I64),
        ("ling_kernel_locale_is_celestial", &[types::I64], types::I64),
        ("ling_kernel_locale_uses_daemon", &[types::I64], types::I64),
        ("ling_kernel_locale_flag_id", &[types::I64], types::I64),
        ("ling_kernel_moon_day_progress", &[], types::I64),
        ("ling_kernel_net_init", &[], types::I64),
        ("ling_kernel_net_link_up", &[], types::I64),
        ("ling_kernel_net_mac_byte", &[types::I64], types::I64),
        ("ling_kernel_net_self_ip_byte", &[types::I64], types::I64),
        ("ling_kernel_net_gateway_ip_byte", &[types::I64], types::I64),
        ("ling_kernel_net_arp_selftest", &[], types::I64),
        ("ling_kernel_net_arp_reply_mac_byte", &[types::I64], types::I64),
        ("ling_kernel_locale_selected", &[], types::I64),
        ("ling_kernel_locale_index_of_id", &[types::I64], types::I64),
        ("ling_kernel_locale_select", &[types::I64], types::I64),
        ("ling_kernel_locale_reset", &[], types::I64),
        ("ling_kernel_kbd_layout_count", &[], types::I64),
        ("ling_kernel_kbd_layout_name", &[types::I64], types::I64),
        ("ling_kernel_kbd_layout_current", &[], types::I64),
        ("ling_kernel_kbd_layout_set", &[types::I64], types::I64),
        ("ling_kernel_kbd_layout_cursor", &[], types::I64),
        ("ling_kernel_kbd_layout_cursor_up", &[], types::I64),
        ("ling_kernel_kbd_layout_cursor_down", &[], types::I64),
        ("ling_kernel_kbd_layout_confirm_cursor", &[], types::I64),
        ("ling_kernel_kbd_layout_reset", &[], types::I64),
        ("ling_kernel_kbd_layout_selected", &[], types::I64),
        ("ling_kernel_locale_cursor", &[], types::I64),
        ("ling_kernel_locale_cursor_up", &[], types::I64),
        ("ling_kernel_locale_cursor_down", &[], types::I64),
        ("ling_kernel_locale_confirm_cursor", &[], types::I64),
        (
            "ling_kernel_fb_draw_flag",
            &[types::I64, types::I64, types::I64, types::I64, types::I64],
            types::I64,
        ),
        // LingOS userspace syscall intrinsics (no_std ring-3)
        ("ling_sys_exit", &[types::I64], types::I64),
        ("ling_sys_write", &[types::I64, types::I64, types::I64], types::I64),
        ("ling_sys_read", &[types::I64, types::I64, types::I64], types::I64),
        ("ling_sys_open", &[types::I64, types::I64], types::I64),
        ("ling_sys_close", &[types::I64], types::I64),
        ("ling_sys_lseek", &[types::I64, types::I64, types::I64], types::I64),
        ("ling_sys_mmap", &[types::I64, types::I64, types::I64, types::I64], types::I64),
        ("ling_sys_munmap", &[types::I64, types::I64], types::I64),
        ("ling_sys_spawn", &[types::I64, types::I64], types::I64),
        ("ling_sys_waitpid", &[types::I64], types::I64),
        ("ling_sys_yield", &[], types::I64),
        ("ling_sys_getpid", &[], types::I64),
        ("ling_sys_sleep_ms", &[types::I64], types::I64),
        ("ling_sys_poll_input", &[], types::I64),
        ("ling_sys_fb_map", &[], types::I64),
        ("ling_sys_uname", &[types::I64, types::I64], types::I64),
    ];

    for &(name, params, ret) in runtime_fns {
        let mut sig = module.make_signature();
        for &pt in params {
            sig.params.push(AbiParam::new(pt));
        }
        sig.returns.push(AbiParam::new(ret));
        let id = module
            .declare_function(name, Linkage::Import, &sig)
            .unwrap();
        decls.insert(name.to_string(), RuntimeDecl { id });
    }

    decls
}

// ─── String/Builtin name collection ─────────────────────────────────────────

fn collect_string_constants(
    functions: &[MirFunction],
    module: &mut ObjectModule,
) -> (HashMap<String, DataId>, HashMap<String, DataId>) {
    let mut string_ids: HashMap<String, DataId> = HashMap::new();
    let mut builtin_ids: HashMap<String, DataId> = HashMap::new();
    for func in functions {
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(_, rval) = &stmt.kind {
                    visit_rvalue_strings(rval, module, &mut string_ids);
                    visit_rvalue_builtin_names(rval, module, &mut builtin_ids);
                }
            }
            if let Some(term) = &bb.terminator {
                visit_term_strings(term, module, &mut string_ids);
            }
        }
    }
    (string_ids, builtin_ids)
}

fn visit_operand_strings(
    op: &Operand,
    module: &mut ObjectModule,
    string_ids: &mut HashMap<String, DataId>,
) {
    if let Operand::Constant(Constant::Str(s)) = op {
        if !string_ids.contains_key(s) {
            let name = format!("__str_{}", string_ids.len());
            let data_id = module
                .declare_data(&name, Linkage::Local, true, false)
                .unwrap();
            let mut desc = DataDescription::new();
            // NUL-terminated even though the boxed `ling_str_new(ptr, len)`
            // path only ever reads `len` bytes: kernel-mode calls pass this
            // same pointer raw (see `translate_op_int`'s `Constant::Str` arm)
            // to intrinsics like `ling_kernel_vga_write_str` that take a
            // pointer only, no length, and scan for a NUL terminator.
            let mut bytes = s.as_bytes().to_vec();
            bytes.push(0);
            desc.define(bytes.into_boxed_slice());
            desc.set_align(1);
            module.define_data(data_id, &desc).unwrap();
            string_ids.insert(s.clone(), data_id);
        }
    }
}

fn visit_rvalue_builtin_names(
    rval: &Rvalue,
    module: &mut ObjectModule,
    builtin_ids: &mut HashMap<String, DataId>,
) {
    if let Rvalue::Call { func: Operand::Constant(Constant::Function(n)), .. } = rval {
        // `ling_kernel_*` calls must resolve through `runtime_decls` (a direct
        // extern import, see `declare_runtime_functions`) — never through the
        // generic `__ling_builtin` dispatch. Registering them here too would
        // make the per-function `func_refs` loop's `builtin_ids` check match
        // first (it runs before the `runtime_decls` check), silently routing
        // every kernel-intrinsic call into `emit_builtin_call` instead, which
        // needs a `ling_builtin` the `#![no_std]` kernel runtime never provides.
        if n.starts_with("ling_kernel_") {
            return;
        }
        if !builtin_ids.contains_key(n) {
            let name = format!("__builtin_{}", builtin_ids.len());
            let data_id = module
                .declare_data(&name, Linkage::Local, true, false)
                .unwrap();
            let mut desc = DataDescription::new();
            let mut bytes = n.as_bytes().to_vec();
            bytes.push(0);
            desc.define(bytes.into_boxed_slice());
            desc.set_align(1);
            module.define_data(data_id, &desc).unwrap();
            builtin_ids.insert(n.clone(), data_id);
        }
    }
}

fn visit_rvalue_strings(
    rval: &Rvalue,
    module: &mut ObjectModule,
    string_ids: &mut HashMap<String, DataId>,
) {
    match rval {
        Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => visit_operand_strings(op, module, string_ids),
        Rvalue::BinaryOp(_, lhs, rhs) => {
            visit_operand_strings(lhs, module, string_ids);
            visit_operand_strings(rhs, module, string_ids);
        },
        Rvalue::Call { args, .. } => {
            for arg in args {
                visit_operand_strings(arg, module, string_ids);
            }
        },
        Rvalue::Aggregate(_, ops) => {
            for op in ops {
                visit_operand_strings(op, module, string_ids);
            }
        },
        Rvalue::InlineAsm(template) => {
            if !string_ids.contains_key(template) {
                let name = format!("__asm_{}", string_ids.len());
                let data_id = module
                    .declare_data(&name, Linkage::Local, true, false)
                    .unwrap();
                let mut desc = DataDescription::new();
                desc.define(template.as_bytes().to_vec().into_boxed_slice());
                desc.set_align(1);
                module.define_data(data_id, &desc).unwrap();
                string_ids.insert(template.clone(), data_id);
            }
        },
        _ => {},
    }
}

fn visit_term_strings(
    term: &Terminator,
    module: &mut ObjectModule,
    string_ids: &mut HashMap<String, DataId>,
) {
    if let TerminatorKind::SwitchInt { discr, .. } = &term.kind {
        visit_operand_strings(discr, module, string_ids);
    }
}

// ─── Per-function symbol references ──────────────────────────────────────────

/// The data/function symbols a single function actually references: the string
/// constants it materializes and the callee/builtin names it invokes.
///
/// Declaring every global string, builtin, and user function into *every*
/// function is O(functions × symbols) in both time and per-function IR size,
/// which exhausts memory on large programs. The translator only ever looks up
/// the symbols a function references, so we declare only those.
#[derive(Default)]
struct FuncRefs {
    strings: HashSet<String>,
    /// Direct callee names — either user functions (resolved to `FuncId`) or
    /// builtin names (resolved to a name-string `DataId`).
    names: HashSet<String>,
}

fn collect_func_refs(func: &MirFunction) -> FuncRefs {
    let mut refs = FuncRefs::default();
    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            if let StatementKind::Assign(_, rval) = &stmt.kind {
                collect_rvalue_refs(rval, &mut refs);
            }
        }
        if let Some(term) = &bb.terminator {
            if let TerminatorKind::SwitchInt { discr, .. } = &term.kind {
                collect_operand_str(discr, &mut refs);
            }
        }
    }
    refs
}

fn collect_operand_str(op: &Operand, refs: &mut FuncRefs) {
    if let Operand::Constant(Constant::Str(s)) = op {
        refs.strings.insert(s.clone());
    }
}

fn collect_rvalue_refs(rval: &Rvalue, refs: &mut FuncRefs) {
    match rval {
        Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => collect_operand_str(op, refs),
        Rvalue::BinaryOp(_, lhs, rhs) => {
            collect_operand_str(lhs, refs);
            collect_operand_str(rhs, refs);
        },
        Rvalue::Call { func, args } => {
            if let Operand::Constant(Constant::Function(n)) = func {
                refs.names.insert(n.clone());
            }
            for arg in args {
                collect_operand_str(arg, refs);
            }
        },
        Rvalue::Aggregate(_, ops) => {
            for op in ops {
                collect_operand_str(op, refs);
            }
        },
        Rvalue::InlineAsm(template) => {
            refs.strings.insert(template.clone());
        },
        _ => {},
    }
}

// ─── Main AOT compilation ───────────────────────────────────────────────────

impl CraneliftBackend {
    pub fn new() -> Self {
        let isa_builder = cranelift::native::builder()
            .unwrap_or_else(|_| isa::lookup_by_name("aarch64").unwrap());
        Self::from_isa_builder(isa_builder)
    }

    /// Cross-target constructor: emit for `arch` (e.g. `"aarch64"`) regardless
    /// of the host the compiler itself is running on. Used for kernel targets
    /// whose triple doesn't match the build host, like `aarch64-unknown-none`
    /// (Raspberry Pi) compiled from an x86_64 dev machine.
    pub fn new_for_arch(arch: &str) -> Self {
        let isa_builder = isa::lookup_by_name(arch)
            .unwrap_or_else(|e| panic!("cranelift: no codegen backend for arch '{arch}': {e}"));
        Self::from_isa_builder(isa_builder)
    }

    fn from_isa_builder(isa_builder: isa::Builder) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("is_pic", "true").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        flag_builder.set("enable_verifier", "false").unwrap();
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();
        let obj_builder = ObjectBuilder::new(
            isa,
            "ling_program",
            cranelift_module::default_libcall_names(),
        )
        .expect("ObjectBuilder");
        let module = ObjectModule::new(obj_builder);
        Self {
            module: Some(module),
            builder_ctx: FunctionBuilderContext::new(),
            progress: false,
        }
    }

    /// Enable a tqdm-style progress bar over the per-function codegen loop.
    /// Off by default so library/test use stays silent.
    pub fn with_progress(mut self, on: bool) -> Self {
        self.progress = on;
        self
    }
}

impl CodegenBackend for CraneliftBackend {
    fn emit(&mut self, program: &MirProgram, out: &std::path::Path) -> Result<()> {
        let module: &mut ObjectModule = self.module.as_mut().unwrap();

        let num_types = numtype::analyze(&program.mir.functions);

        // Phase 0: declare all runtime functions as imports
        let runtime_decls = declare_runtime_functions(module);

        // Phase 1: collect and declare string/builtin data objects
        let (string_ids, builtin_ids) = collect_string_constants(&program.mir.functions, module);

        // Phase 2: declare all user functions as exports
        let mut func_ids: HashMap<String, FuncId> = HashMap::new();
        let mut func_arities: HashMap<String, usize> = HashMap::new();
        for func in &program.mir.functions {
            let mut sig = module.make_signature();
            for _ in 0..func.arg_count {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            let id = module
                .declare_function(&func.name, Linkage::Export, &sig)
                .unwrap();
            func_ids.insert(func.name.clone(), id);
            func_arities.insert(func.name.clone(), func.arg_count);
        }

        // Phase 3: translate each function body
        let total = program.mir.functions.len();
        // Throttle redraws so huge programs don't spend real time on the bar.
        let step = (total / 100).max(1);
        // Only draw the live bar on a real terminal; piped/CI logs stay clean.
        let show_progress = self.progress && total > 1 && std::io::stderr().is_terminal();
        for (idx, func) in program.mir.functions.iter().enumerate() {
            if show_progress && (idx % step == 0 || idx + 1 == total) {
                let label = if func.name == "__main__" {
                    "main"
                } else {
                    func.name.as_str()
                };
                render_progress(idx + 1, total, label);
            }
            let &fid = func_ids.get(&func.name).unwrap();

            let mut ctx = module.make_context();
            let mut sig = module.make_signature();
            for _ in 0..func.arg_count {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            ctx.func.signature = sig;

            let mut builder = FunctionBuilder::new(&mut ctx.func, &mut self.builder_ctx);
            let blocks: Vec<Block> = func
                .basic_blocks
                .iter()
                .map(|_| builder.create_block())
                .collect();

            let max_local = max_local_index(func);
            let mut vars: HashMap<Local, Variable> = HashMap::new();
            for i in 0..=max_local {
                vars.insert(Local(i), builder.declare_var(types::I64));
            }

            // Declare only the symbols this function actually references. The
            // global `string_ids`/`builtin_ids`/`func_ids` maps cover the whole
            // program; declaring all of them into every function is O(functions ×
            // symbols) and runs the compiler out of memory on large projects.
            let refs = collect_func_refs(func);

            // Build string GlobalValue map for this function
            let mut string_gvs: HashMap<String, GlobalValue> = HashMap::new();
            for s in &refs.strings {
                if let Some(&data_id) = string_ids.get(s) {
                    let gv = module.declare_data_in_func(data_id, builder.func);
                    string_gvs.insert(s.clone(), gv);
                }
            }

            // Build runtime FuncRef map for this function. The shared translator
            // looks up runtime helpers by their JIT symbol name (`__ling_*`); the
            // object backend declares them without that prefix, so re-key here.
            let mut runtime_refs: HashMap<String, FuncRef> = HashMap::new();
            for (name, decl) in &runtime_decls {
                let fr = module.declare_func_in_func(decl.id, builder.func);
                runtime_refs.insert(format!("__{name}"), fr);
            }

            // FuncRefs are function-local, so callee references must be declared
            // into this builder (not shared across functions). A referenced name is
            // either a user function (declared as a FuncRef) or a builtin (declared
            // as a name-string GlobalValue for the `__ling_builtin` dispatch path).
            let mut func_refs: HashMap<String, FuncRef> = HashMap::new();
            let mut builtin_gvs: HashMap<String, GlobalValue> = HashMap::new();
            for name in &refs.names {
                if let Some(&id) = func_ids.get(name) {
                    let fr = module.declare_func_in_func(id, builder.func);
                    func_refs.insert(name.clone(), fr);
                } else if let Some(&data_id) = builtin_ids.get(name) {
                    let gv = module.declare_data_in_func(data_id, builder.func);
                    builtin_gvs.insert(name.clone(), gv);
                } else if let Some(decl) = runtime_decls.get(name) {
                    // Kernel runtime functions (ling_kernel_*) declared as imports
                    let fr = module.declare_func_in_func(decl.id, builder.func);
                    func_refs.insert(name.clone(), fr);
                }
            }

            let tctx = TransCtx {
                vars: &vars,
                string_gvs: &string_gvs,
                builtin_gvs: &builtin_gvs,
                runtime_refs: &runtime_refs,
                func_refs: &func_refs,
                func_arities: &func_arities,
                nt: &num_types,
                fname: &func.name,
            };
            build_function_body(&mut builder, func, &blocks, &tctx);
            builder.finalize();
            module.define_function(fid, &mut ctx).unwrap();
        }
        if show_progress {
            eprintln!();
        }

        // Phase 4: finalize and emit .o file
        let obj = self.module.take().unwrap().finish();
        let bytes = obj.emit().map_err(|e| anyhow::anyhow!("{:?}", e))?;
        std::fs::write(out, bytes)?;
        Ok(())
    }
}

impl Default for CraneliftBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CodegenBackend;
    use ling_ast::Span;

    fn decl() -> LocalDecl {
        LocalDecl {
            ty: MirType::Any,
            name: None,
            span: Span::DUMMY,
            is_mut: false,
            is_owning: false,
        }
    }
    fn stmt(kind: StatementKind) -> Statement {
        Statement { kind, span: Span::DUMMY }
    }
    fn ret() -> Terminator {
        Terminator { kind: TerminatorKind::Return, span: Span::DUMMY }
    }

    /// A program that uses a string constant, a builtin call, and a call to
    /// another user function must still compile when each function only declares
    /// the symbols it actually references (the O(functions × symbols) fix). The
    /// `helper` function references none of `__main__`'s symbols, so a regression
    /// that under-declares would surface here as a missing-symbol panic or an
    /// empty object.
    #[test]
    fn emit_declares_only_referenced_symbols() {
        // helper(x) = x
        let mut helper = MirFunction::new("helper", 1);
        helper.basic_blocks = vec![BasicBlock {
            statements: vec![stmt(StatementKind::Assign(
                Local(0),
                Rvalue::Use(Operand::Copy(Local(1))),
            ))],
            terminator: Some(ret()),
        }];

        // __main__: s = "hi"; print(s); r = helper(5); return r
        let mut main = MirFunction::new("__main__", 0);
        main.locals = vec![decl(), decl(), decl()]; // Local(1), Local(2), Local(3)
        main.basic_blocks = vec![BasicBlock {
            statements: vec![
                stmt(StatementKind::Assign(
                    Local(1),
                    Rvalue::Use(Operand::Constant(Constant::Str("hi".into()))),
                )),
                stmt(StatementKind::Assign(
                    Local(2),
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("print".into())),
                        args: vec![Operand::Copy(Local(1))],
                    },
                )),
                stmt(StatementKind::Assign(
                    Local(3),
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("helper".into())),
                        args: vec![Operand::Constant(Constant::I64(5))],
                    },
                )),
                stmt(StatementKind::Assign(
                    Local(0),
                    Rvalue::Use(Operand::Copy(Local(3))),
                )),
            ],
            terminator: Some(ret()),
        }];

        let program = ling_mir::MirProgram { functions: vec![helper, main] };
        let wrapped = crate::MirProgram::new(program, "test.ling");

        let mut backend = CraneliftBackend::new();
        let out = std::env::temp_dir().join(format!("ling_aot_test_{}.o", std::process::id()));
        backend.emit(&wrapped, &out).expect("emit should succeed");
        let bytes = std::fs::read(&out).expect("object file should exist");
        assert!(!bytes.is_empty(), "emitted object must be non-empty");
        let _ = std::fs::remove_file(&out);
    }
}
