//! Translation tests: Lion in, Rust out. The behavior of the code is tested by the
//! golden programs, compiled by `cargo test --test native`; these tests show the shape
//! of the translation.

use lion_diagnostics::SourceMap;
use lion_syntax::{lex, parse};

use super::*;

/// The Rust code of a program of one file.
fn rust(text: &str) -> String {
    let mut map = SourceMap::new();
    let id = map.add("t.lion", text);
    let lexed = lex(id, text);
    let parsed = parse(text, &lexed.tokens);
    assert!(lexed.diagnostics.is_empty() && parsed.diagnostics.is_empty(), "syntax error in {text:?}");
    let checked = lion_sema::check(&parsed.module);
    let program = checked.program.unwrap_or_else(|| panic!("errors in {text:?}: {:?}", checked.diagnostics));
    generate(&program, &[("t.lion", text)])
}

/// The body of the function `fN` of the code, one statement per line; the places in
/// the source are all written `S`.
fn function(code: &str, number: usize) -> Vec<String> {
    let start = code.find(&format!("fn f{number}(")).expect("the function");
    code[start..]
        .lines()
        .skip(2)
        .take_while(|line| line.starts_with("        "))
        .map(|line| without_span_numbers(line.trim()))
        .collect()
}

fn without_span_numbers(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == 'S' && chars.peek().is_some_and(char::is_ascii_digit) {
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
        }
    }
    out
}

#[test]
fn literals_are_valid_rust() {
    assert_eq!(int_literal(-5), "(-5i64)");
    assert_eq!(int_literal(i64::MIN), "i64::MIN");
    assert_eq!(float_literal(0.1), "(0.1f64)");
    assert_eq!(float_literal(-0.0), "(-0.0f64)");
    assert_eq!(float_literal(f64::INFINITY), "f64::from_bits(0x7ff0000000000000)");
}

#[test]
fn numbers_are_rust_numbers_and_operations_are_checked() {
    let code = rust("let x = 10\nshow(x * 2 + 1)");
    assert_eq!(
        function(&code, 0),
        [
            "let mut l0: i64 = 0;",
            "l0 = (10i64);",
            "let t1: i64 = rt.check(ops::int_mul(l0, (2i64)), S)?;",
            "let t2: i64 = rt.check(ops::int_add(t1, (1i64)), S)?;",
            "let t3: Value = Value::Int(t2);",
            "rt.show(&t3)?;",
            "Ok(Value::None)",
        ]
    );
}

#[test]
fn a_call_checks_the_depth_and_records_its_place() {
    let code = rust("fun twice(n in Int) in Int = n * 2\nshow(twice(4))");
    let script = function(&code, 1);
    assert_eq!(script[..3], ["rt.enter(S)?;", "let t1 = f0(rt, (4i64));", "let t2: i64 = rt.leave(t1, S)?;"]);
}

#[test]
fn an_operand_read_before_a_call_that_may_change_it_is_taken_first() {
    // §9.2: `total` is read before `bump` changes it.
    let code = rust(
        "var total = 1\nfun bump() in Int modifies total:\n    total += 1\n    return total\n;\nshow(total + bump())",
    );
    let script = function(&code, 1);
    let read = script.iter().position(|line| line.contains("= (*&raw mut G0);")).expect("a copy of total");
    let call = script.iter().position(|line| line.contains("f0(rt)")).expect("the call");
    assert!(read < call, "{script:#?}");
}

#[test]
fn the_globals_that_functions_reach_are_static() {
    let code =
        rust("var reached = 0\nvar only_here = 0\nfun f() modifies reached:\n    reached += 1\n;\nf()");
    assert!(code.contains("static mut G0: i64 = 0;"));
    assert!(!code.contains("G1"));
    assert!(function(&code, 1).contains(&"let mut l1: i64 = 0;".to_string()));
}

#[test]
fn a_var_parameter_is_a_pointer_and_a_default_is_computed_by_the_callee() {
    let code =
        rust("fun add(var l in List of Int, n in Int = 1):\n    l.add(n)\n;\nvar mine = [2]\nadd(mine)");
    assert!(code.contains("fn f0(rt: &mut Rt, mut l0: *mut Value, p1: Option<i64>) -> R<Value>"));
    assert!(code.contains("let t3 = f0(rt, &raw mut l0, None);"));
}

#[test]
fn a_function_value_gets_a_wrapper_that_takes_values() {
    let code = rust("fun inc(n in Int) in Int = n + 1\nlet f = inc\nshow(f(1))");
    assert!(code.contains("Some(d0),"));
    assert!(code.contains("fn d0(rt: &mut Rt, args: Vec<Value>, captures: &[Value]) -> R<Value> {"));
}

#[test]
fn the_turns_of_a_parallel_loop_run_in_a_closure() {
    let code = rust("let squares = parallel [x * x, x in 1..10]\nshow(squares)");
    assert!(code.contains("rt.parallel("), "{code}");
    assert!(code.contains("TurnExit::End"));
}

#[test]
fn turns_that_read_the_keyboard_run_in_their_order() {
    let code = rust("parallel for i in 1..3:\n    show(ask(\"name?\"))\n;");
    assert!(!code.contains("rt.parallel("), "{code}");
}

#[test]
fn turns_that_use_a_synced_object_run_in_their_order() {
    let code = rust(
        "var total = shared synced 0\nfun add(n in Int) in Int modifies total:\n    total += n\n    return n\n;\nshow(parallel [add(n), n in 1..4])",
    );
    assert!(!code.contains("rt.parallel("), "{code}");
}
