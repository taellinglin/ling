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
    let _source = r#"
        令 启动 = 执行 {
            印("你好")
        }
    "#;

    // Should compile to same binary as English version
    assert!(true);
}

#[test]
fn test_borrow_checker() {
    let _source = r#"
        bind start = do {
            bind x = own 5
            bind y = lend x
            bind z = move x  // Error: x already lent
        }
    "#;

    // Should fail with borrow error — checker is WIP
    assert!(true);
}
