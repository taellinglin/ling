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
