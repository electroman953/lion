//! Type checker tests: source in, typed IR or error messages out.

use lion_diagnostics::SourceMap;
use lion_ir::print_program;
use lion_syntax::{lex, parse};

use crate::check;

fn check_text(text: &str) -> Result<String, Vec<String>> {
    let mut map = SourceMap::new();
    let id = map.add("t.lion", text);
    let lexed = lex(id, text);
    let parsed = parse(&lexed.tokens);
    assert!(lexed.diagnostics.is_empty() && parsed.diagnostics.is_empty(), "syntax error in {text:?}");
    let checked = check(&parsed.module);
    match checked.program {
        Some(program) => Ok(print_program(&program)),
        None => Err(checked.diagnostics.into_iter().map(|d| d.message).collect()),
    }
}

/// The body of the printed IR, one statement per line.
fn body(text: &str) -> String {
    let ir = check_text(text).unwrap_or_else(|errors| panic!("errors in {text:?}: {errors:?}"));
    let body = ir.split("body\n").nth(1).expect("a body section");
    body.lines().map(str::trim).collect::<Vec<_>>().join("\n")
}

fn errors(text: &str) -> Vec<String> {
    check_text(text).expect_err("expected errors")
}

#[test]
fn first_vertical_slice() {
    assert_eq!(
        body("let x = 10\nlet y = 20\nlet z = x + y\nshow(z)"),
        "x#0 = 10\ny#1 = 20\nz#2 = (add_int x#0 y#1)\n(show z#2)"
    );
}

#[test]
fn int_is_converted_to_float_where_needed() {
    assert_eq!(body("let b = 5 in Float"), "b#0 = (int_to_float 5)");
    assert_eq!(body("let c = 5 as Float"), "c#0 = (int_to_float 5)");
    assert_eq!(body("let d = 1 + 2.5"), "d#0 = (add_float (int_to_float 1) 2.5)");
    assert_eq!(body("var f = 1.5\nf = 2"), "f#0 = 1.5\nf#0 = (int_to_float 2)");
    // `/` always gives a Float (§8).
    assert_eq!(body("let q = 7 / 2"), "q#0 = (div_float (int_to_float 7) (int_to_float 2))");
    assert_eq!(body("let q = 7 div 2"), "q#0 = (div_int 7 2)");
}

#[test]
fn compound_assignments() {
    assert_eq!(body("var n = 1\nn += 2"), "n#0 = 1\nn#0 = (add_int n#0 2)");
    assert_eq!(body("var f = 1.0\nf *= 2"), "f#0 = 1.0\nf#0 = (mul_float f#0 (int_to_float 2))");
}

#[test]
fn deferred_values() {
    assert_eq!(body("let e in Text\ne = \"a\"\nshow(e)"), "e#0 = \"a\"\n(show e#0)");
    assert_eq!(body("var d in Float\nd = 3"), "d#0 = (int_to_float 3)");
    // A `var` that never receives a value is accepted: there is no warning for unused names.
    assert_eq!(body("var unused in Int"), "");
}

#[test]
fn redeclaration_in_the_same_block_hides_the_old_binding() {
    assert_eq!(
        body("var count = 0\nlet count = \"zero\"\nshow(count)"),
        "count#0 = 0\ncount#1 = \"zero\"\n(show count#1)"
    );
    assert_eq!(body("let x = 1\nlet x = x + 1"), "x#0 = 1\nx#1 = (add_int x#0 1)");
}

#[test]
fn chained_comparisons_evaluate_each_operand_once() {
    // Literals are read twice; other operands go through a temporary.
    assert_eq!(
        body("let g = 12\nlet ok = 0 <= g <= 20").lines().nth(1).unwrap(),
        "ok#2 = (let %t#1 g#0 (and (le_int 0 %t#1) (le_int %t#1 20)))"
    );
    assert_eq!(
        body("let a = 1\nlet ok = a < 2 < 3.5").lines().nth(1).unwrap(),
        "ok#2 = (let %t#1 a#0 (and (lt_int %t#1 2) (lt_float (int_to_float 2) 3.5)))"
    );
    assert_eq!(
        body("let a = 1\nlet ok = 0 < a + 1 < a * 2 < 10").lines().nth(1).unwrap(),
        "ok#3 = (let %t#1 (add_int a#0 1) (and (lt_int 0 %t#1) \
         (let %t#2 (mul_int a#0 2) (and (lt_int %t#1 %t#2) (lt_int %t#2 10)))))"
    );
}

#[test]
fn interpolation() {
    assert_eq!(
        body("let n = 3\nshow(\"n = {n}, {n / 2}!\")").lines().nth(1).unwrap(),
        "(show (concat \"n = \" (to_text n#0) \", \" (to_text (div_float (int_to_float n#0) (int_to_float 2))) \"!\"))"
    );
}

#[test]
fn type_errors() {
    assert_eq!(errors("let x = 3 + \"hello\""), ["`+` cannot be applied to Int and Text"]);
    assert_eq!(errors("var count = 0\ncount = \"zero\""), ["mismatched types"]);
    assert_eq!(errors("let x = 5.5 in Int"), ["mismatched types"]);
    assert_eq!(errors("var x = 1\nx += 1.5"), ["mismatched types"]);
    assert_eq!(errors("let b = not 1"), ["`not` expects a Bool"]);
    assert_eq!(errors("let b = 1 and true"), ["`and` expects a Bool on each side"]);
    assert_eq!(errors("let n = -\"a\""), ["`-` cannot be applied to a Text"]);
    assert_eq!(errors("let x = true as Int"), ["cannot convert a Bool to Int with `as`"]);
    assert_eq!(errors("let x = 1 in Strng"), ["cannot find the type `Strng`"]);
}

#[test]
fn constructions_not_defined_by_the_spec_are_refused() {
    assert_eq!(errors("let t = \"a\" + \"b\""), ["`+` cannot be applied to Text and Text"]);
    assert_eq!(errors("let b = 1 == \"1\""), ["cannot compare an Int with a Text"]);
    assert_eq!(errors("let b = \"a\" < \"b\""), ["`<` is not defined for Text values"]);
    assert_eq!(errors("let q = 7.5 div 2"), ["`div` is defined only for Int values"]);
    assert_eq!(errors("let show = 1"), ["`show` is the name of a standard function"]);
}

#[test]
fn names_and_values() {
    assert_eq!(errors("show(totl)"), ["cannot find `totl` in this scope"]);
    assert_eq!(errors("x = 1"), ["cannot find `x` in this scope"]);
    assert_eq!(errors("let x = 1\nx = 2"), ["cannot assign to the constant `x`"]);
    assert_eq!(errors("let e in Text\ne = \"a\"\ne = \"b\""), ["the constant `e` already has a value"]);
    assert_eq!(
        errors("let e in Text\nshow(e)"),
        ["`e` is used before it has a value", "the constant `e` never receives a value",]
    );
    assert_eq!(errors("var n in Int\nn += 1"), ["`n` is used before it has a value"]);
    assert_eq!(errors("let x = 1\nx(2)"), ["`x` is not a function"]);
    assert_eq!(errors("show(1, 2)"), ["`show` takes exactly one value, not 2"]);
    assert_eq!(errors("show(var x)").last().unwrap(), "cannot find `x` in this scope");
}

#[test]
fn one_mistake_gives_one_message() {
    // `y` has no type because of the error in its value; its uses stay silent.
    assert_eq!(
        errors("let y = 1 + \"a\"\nlet z = y * 2\nshow(z)"),
        ["`+` cannot be applied to Int and Text"]
    );
}

#[test]
fn unsupported_types_are_reported() {
    assert_eq!(errors("var l in List of Int"), ["not implemented yet: collections"]);
    assert_eq!(errors("var m in maybe Int"), ["not implemented yet: union types (`or`, `maybe`)"]);
    assert_eq!(
        errors("let n = \"12\" as Int"),
        [
            "not implemented yet: converting a Text to a number (its result, `Int or Error`, needs union types)"
        ]
    );
}

#[test]
fn definite_assignment_follows_every_path() {
    // Both branches assign: the value is certain after the `if`.
    assert!(check_text("let e in Text\nif true:\n    e = \"a\"\nelse:\n    e = \"b\"\n;\nshow(e)").is_ok());
    // `while true` is left only through `break`, which comes after the assignment.
    assert!(check_text("var x in Int\nwhile true:\n    x = 1\n    break\n;\nshow(x)").is_ok());
    // Code that cannot be reached reports nothing.
    assert!(check_text("return\nlet e in Text\nshow(e)").is_ok());
    // A branch that ends the script does not reach the read.
    assert!(check_text("var x in Int\nif false:\n    return\n;\nx = 1\nshow(x)").is_ok());
    assert!(check_text("var x in Int\nif false:\n    return\nelse:\n    x = 2\n;\nshow(x)").is_ok());
}

#[test]
fn definite_assignment_errors() {
    assert_eq!(errors("var x in Int\nif true: x = 1 ;\nshow(x)"), ["`x` may not have a value here"]);
    assert_eq!(
        errors("var x in Int\nwhile false:\n    x = 1\n;\nshow(x)"),
        ["`x` may not have a value here"]
    );
    // In a loop, a read before the assignment sees the first turn without a value.
    assert_eq!(
        errors("var x in Int\nwhile true:\n    show(x)\n    x = 1\n;"),
        ["`x` may not have a value here"]
    );
    assert_eq!(
        errors("let e in Text\nwhile true:\n    e = \"a\"\n    break\n;"),
        ["the constant `e` may already have a value"]
    );
    assert_eq!(
        errors("let e in Text\nif true: e = \"a\" ;"),
        ["the constant `e` does not receive a value on every path"]
    );
}

#[test]
fn scopes_of_blocks() {
    assert_eq!(
        errors("let n = 1\nif true:\n    let n = 2\n;"),
        ["`n` is already declared in an enclosing block"]
    );
    assert_eq!(errors("if true:\n    let inner = 1\n;\nshow(inner)"), ["cannot find `inner` in this scope"]);
    // Sibling blocks may reuse a name, and so may the outer block once the inner one is closed.
    assert!(check_text("if true:\n    let t = 1\n;\nif false:\n    let t = 2\n;\nlet t = 3").is_ok());
    // Redeclaring in the same inner block is allowed (§6.3).
    assert!(check_text("while true:\n    let a = 1\n    let a = \"one\"\n    break\n;").is_ok());
}

#[test]
fn if_expressions() {
    assert_eq!(body("let s = if true then 1 else 2"), "s#0 = (if true 1 2)");
    assert_eq!(body("let s = if true then 1 elif false then 2 else 3"), "s#0 = (if true 1 (if false 2 3))");
    assert_eq!(body("let s = if true then 1 else 2.5"), "s#0 = (if true (int_to_float 1) 2.5)");
    assert_eq!(errors("let s = if 1 then 1 else 2"), ["the condition of `if` must be a Bool"]);
}
