use ling::{CompilerConfig, LingCompiler};
use std::path::PathBuf;

fn temp_output_dir() -> PathBuf {
    let mut p = std::env::current_dir().unwrap();
    p.push("target");
    p.push("ling-test-output-user");
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn test_hello_world() {
    let source = r#"
        bind start = do {
            print("Hello")
        }
    "#;

    let dir = temp_output_dir();
    let input = dir.join("test.ling");
    std::fs::write(&input, source).unwrap();

    let compiler = LingCompiler::new(CompilerConfig::default());
    let result = compiler.compile(&input, &dir.join("output"));
    assert!(result.is_ok());
}

#[test]
fn test_run_hello_ling() {
    let source = std::fs::read_to_string("examples/basics/thai_hello_world.ling")
        .expect("examples/basics/thai_hello_world.ling must exist");
    let result = ling::run(&source);
    assert!(result.is_ok(), "ling::run failed: {:?}", result.err());
}

#[test]
fn test_polyglot_chinese() {
    let source = r#"
        令 启动 = 执 {
            印("你好")
        }
    "#;

    assert!(
        ling::run(source).is_ok(),
        "chinese-alias program failed to run"
    );
}

// ── B1: user-defined data types (form / choose) ─────────────────────────────

#[test]
fn test_struct_form_and_field_access() {
    let source = r#"
        form Point { x, y }
        bind start = do {
            bind p = Point(3, 4)
            print(p.x)
            print(p.y)
        }
    "#;
    assert!(ling::run(source).is_ok(), "struct form/field access failed");
}

#[test]
fn test_enum_choose_and_match() {
    let source = r#"
        choose Shape { Circle(r), Rect(w, h), Dot }
        fn area(s) {
            match s {
                Circle(r) => 3 * r * r,
                Rect(w, h) => w * h,
                Dot() => 0,
                _ => -1,
            }
        }
        bind start = do {
            print(area(Circle(2)))
            print(area(Rect(3, 4)))
            print(area(Dot))
        }
    "#;
    assert!(ling::run(source).is_ok(), "enum choose/match failed");
}

#[test]
fn test_struct_wrong_arity_errors() {
    let source = r#"
        form Pair { a, b }
        bind start = do {
            bind p = Pair(1)
        }
    "#;
    assert!(
        ling::run(source).is_err(),
        "wrong-arity construction should error"
    );
}

#[test]
fn test_data_types_multilingual_chinese() {
    let source = r#"
        形 点 { x, y }
        选 形状 { 圆(r), 原点 }
        函 面积(s) {
            配 s {
                圆(r) => 3 * r * r,
                _ => 0,
            }
        }
        令 启 = 执 {
            令 p = 点(6, 8)
            打印(p.x)
            打印(面积(圆(5)))
            打印(原点)
        }
    "#;
    assert!(ling::run(source).is_ok(), "multilingual data types failed");
}

#[test]
fn test_borrow_checker() {
    // The borrow checker (own/lend/move) is not implemented yet, so this only
    // pins down that the parser accepts the syntax without panicking. Once the
    // checker lands this should assert result.is_err() instead.
    let source = r#"
        bind start = do {
            bind x = own 5
            bind y = lend x
            bind z = move x  // Error: x already lent
        }
    "#;

    let _ = ling::run(source);
}

#[test]
fn test_deduplicated_math_aliases() {
    let source = r#"
        bind start = do {
            print(sqrt(16))
            print(ceil(3.2))
            print(clamp(15, 0, 10))
            print(tau)
        }
    "#;
    assert!(
        ling::run(source).is_ok(),
        "deduplicated math aliases failed"
    );
}

// ── records, results and variants through the JIT ───────────────────────────
// A record built in compiled code, returned through a builtin (`st_set` / `st_get`), wrapped in `ok`, matched out with `ok(v)`
// / `bad(e)`, and read field by field must come out the same as under the interpreter (they used to come back as unit). The
// program writes its verdict to a file (a runtime error inside a builtin does not stop a JIT run, so `panic` cannot tell).
const RECORDS_THROUGH_THE_JIT: &str = r#"
    form Summary { unlocked, sync_kind }
    choose Shape { Circle(r), Rect(w, h) }
    fn make() { return Summary(5, "synced") }
    fn res(good) {
        if good { return ok(make()) }
        return bad("locked")
    }
    fn area(s) {
        match s {
            Circle(r) => 3 * r * r,
            Rect(w, h) => w * h,
            _ => 0 - 1,
        }
    }
    fn check(cond, what) {
        if !cond { st_set("failed", st_get("failed", "") + what + "; ") }
    }
    bind start = do {
        bind m = match res(true) {
            ok(v) => v,
            bad(e) => (),
        }
        check(m != (), "ok(v) bound unit")
        check(m.sync_kind == "synced" && m.unlocked == 5, "field read")
        st_set("k", m)
        bind g = st_get("k", ())
        check(g.sync_kind == "synced", "record through st_set / st_get")
        bind e = match res(false) {
            ok(v) => "wrong arm",
            bad(why) => why,
        }
        check(e == "locked", "bad(e) binding")
        check(area(Circle(2)) == 12 && area(Rect(3, 4)) == 12, "variant patterns")
        write_file("OUT", "verdict: " + st_get("failed", "ok"))
    }
"#;

fn records_verdict(jit: bool, source: &str) -> String {
    // its own folder: temp_output_dir() wipes a folder the other tests write into at the same time
    let dir = std::env::current_dir().unwrap().join("target").join("ling-test-records");
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join(if jit { "records_jit.txt" } else { "records_interp.txt" });
    let _ = std::fs::remove_file(&out);
    let src = source.replace("OUT", &out.to_string_lossy().replace('\\', "/"));
    let r = if jit { ling::run_jit(&src, None, None) } else { ling::run(&src).map_err(|e| format!("{e:?}")) };
    assert!(r.is_ok(), "run failed: {r:?}");
    std::fs::read_to_string(&out).unwrap_or_else(|e| format!("no verdict: {e}"))
}

#[test]
fn test_records_survive_the_jit() {
    assert_eq!(records_verdict(false, RECORDS_THROUGH_THE_JIT), "verdict: ok", "interpreter");
    assert_eq!(records_verdict(true, RECORDS_THROUGH_THE_JIT), "verdict: ok", "JIT");
    // the checks really catch a wrong value
    let broken = RECORDS_THROUGH_THE_JIT.replace("check(e == \"locked\"", "check(e == \"unlocked\"");
    assert_eq!(records_verdict(true, &broken), "verdict: bad(e) binding; ");
}
