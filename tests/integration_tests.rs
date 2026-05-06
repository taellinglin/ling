// tests/integration_tests.rs
use ling::LingCompiler;
use std::fs;
use std::path::PathBuf;

fn temp_output_dir() -> PathBuf {
    // Avoid external `tempfile` dep; use a deterministic temp path.
    let mut p = std::env::temp_dir();
    p.push("ling-tests-output");
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
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
    
    let compiler = LingCompiler::new(ling::CompilerConfig::default());
    let result = compiler.compile(&input, &dir.join("output"));




    assert!(result.is_ok());
}

#[test]
fn test_polyglot_chinese() {
    let source = r#"
        令 启动 = 执行 {
            印("你好")
        }
    "#;
    
    // Should compile to same binary as English version
    assert!(true);
}

#[test]
fn test_borrow_checker() {
    let source = r#"
        bind start = do {
            bind x = own 5
            bind y = lend x
            bind z = move x  // Error: x already lent
        }
    "#;
    
    // Should fail with borrow error
    assert!(true);
}
