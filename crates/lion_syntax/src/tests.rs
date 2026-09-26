//! Parser tests: source in, S-expression out.

use lion_diagnostics::SourceMap;

use crate::{lex, parse, print_module};

fn parse_text(text: &str) -> (String, Vec<String>) {
    let mut map = SourceMap::new();
    let id = map.add("t.lion", text);
    let lexed = lex(id, text);
    let parsed = parse(&lexed.tokens);
    let errors = lexed.diagnostics.iter().chain(&parsed.diagnostics).map(|d| d.message.clone()).collect();
    (print_module(&parsed.module), errors)
}

fn ast(text: &str) -> String {
    let (tree, errors) = parse_text(text);
    assert!(errors.is_empty(), "unexpected errors for {text:?}: {errors:?}");
    tree.trim_end().to_string()
}

fn first_error(text: &str) -> String {
    let (_, errors) = parse_text(text);
    errors.into_iter().next().unwrap_or_else(|| panic!("expected an error for {text:?}"))
}

#[test]
fn declarations() {
    assert_eq!(ast("let x = 42"), "(let x 42)");
    assert_eq!(ast("var d in Float"), "(var d : Float)");
    assert_eq!(ast("let b = 5 in Float"), "(let b 5 : Float)");
    assert_eq!(ast("let c = 5 as Float"), "(let c (as 5 Float))");
    assert_eq!(ast("x = 1\nx += 2\nx -= 3\nx *= 4"), "(= x 1)\n(+= x 2)\n(-= x 3)\n(*= x 4)");
}

#[test]
fn in_after_a_declaration_value() {
    // §6.2: `in` followed by a type annotates; followed by a value, it tests membership.
    assert_eq!(ast("let x = 3 in Int"), "(let x 3 : Int)");
    assert_eq!(ast("let ok = 3 in primes"), "(let ok (in 3 primes))");
    assert_eq!(ast("let ok = (3 in Int)"), "(let ok (paren (in 3 Int)))");
    assert_eq!(ast("let s = x in m.Student"), "(let s x : m.Student)");
    assert_eq!(ast("let n = a and b in Bool"), "(let n (and a b) : Bool)");
    assert_eq!(ast("let n = a < b in Bool"), "(let n (< a b) : Bool)");
    assert_eq!(ast("if_ok = 3 in Int"), "(= if_ok (in 3 Int))");
}

#[test]
fn precedence_examples_from_the_spec() {
    // §9.1, "Exemples de lecture".
    assert_eq!(ast("1..n - 1"), "(.. 1 (- n 1))");
    assert_eq!(ast("p + 2 in primes"), "(in (+ p 2) primes)");
    assert_eq!(ast("x + 1 as Text"), "(as (+ x 1) Text)");
    assert_eq!(ast("y = not x in S"), "(= y (not (in x S)))");
}

#[test]
fn arithmetic_precedence() {
    assert_eq!(ast("1 + 2 * 3"), "(+ 1 (* 2 3))");
    assert_eq!(ast("(1 + 2) * 3"), "(* (paren (+ 1 2)) 3)");
    assert_eq!(ast("7 div 2 * 3 mod 4"), "(mod (* (div 7 2) 3) 4)");
    assert_eq!(ast("1 - 2 - 3"), "(- (- 1 2) 3)");
    assert_eq!(ast("1 over 3 + 1"), "(+ (over 1 3) 1)");
    // `^` is right-associative and binds more tightly than unary minus (§8.4).
    assert_eq!(ast("y = -2 ^ 2"), "(= y (neg (^ 2 2)))");
    assert_eq!(ast("2 ^ 3 ^ 2"), "(^ 2 (^ 3 2))");
    assert_eq!(ast("2 ^ -1"), "(^ 2 (neg 1))");
    assert_eq!(ast("y = - - x"), "(= y (neg (neg x)))");
}

#[test]
fn logic_and_sets() {
    assert_eq!(ast("a or b and c"), "(or a (and b c))");
    assert_eq!(ast("y = not a and b"), "(= y (and (not a) b))");
    assert_eq!(ast("a union b inter c"), "(union a (inter b c))");
    assert_eq!(ast("a minus b union c"), "(union (minus a b) c)");
    assert_eq!(ast("a subset b"), "(subset a b)");
    assert_eq!(ast("x as Int or ok"), "(or (as x Int) ok)");
}

#[test]
fn chained_comparisons() {
    assert_eq!(ast("0 <= grade <= 20"), "(chain 0 <= grade <= 20)");
    assert_eq!(ast("a == b"), "(== a b)");
    assert_eq!(ast("a != b"), "(!= a b)");
    assert_eq!(ast("a < b and b > c"), "(and (< a b) (> b c))");
}

#[test]
fn calls_fields_and_texts() {
    assert_eq!(ast("show(x)"), "(call show x)");
    assert_eq!(ast("f(var a, name: 1, 2)"), "(call f var a name: 1 2)");
    assert_eq!(ast("s.grade"), "(. s grade)");
    assert_eq!(ast("l[1].name"), "(. (index l 1) name)");
    assert_eq!(ast("f(1)(2)"), "(call (call f 1) 2)");
    assert_eq!(ast("show(\"a\")"), "(call show \"a\")");
    assert_eq!(ast("show(\"x = {x + 1}!\")"), "(call show (text \"x = \" {(+ x 1)} \"!\"))");
    assert_eq!(ast("show(\"\")"), "(call show \"\")");
}

#[test]
fn continuation_inside_parentheses() {
    assert_eq!(ast("total = (a + b\n         + c)"), "(= total (paren (+ (+ a b) c)))");
    assert_eq!(ast("f(1,\n  2)\ng()"), "(call f 1 2)\n(call g)");
}

#[test]
fn types() {
    assert_eq!(ast("var l in List of Int"), "(var l : (List of Int))");
    assert_eq!(ast("var l in List of Student or Error"), "(var l : (or (List of Student) Error))");
    assert_eq!(ast("var m in Map of (Text, Int)"), "(var m : (Map of Text Int))");
    assert_eq!(ast("var l in List of ((Text, Int))"), "(var l : (List of (tuple Text Int)))");
    assert_eq!(ast("var l in List of List of Int"), "(var l : (List of (List of Int)))");
    assert_eq!(ast("var m in maybe List of Int"), "(var m : (maybe (List of Int)))");
    assert_eq!(ast("var m in List of (maybe Int)"), "(var m : (List of (maybe Int)))");
    assert_eq!(ast("var f in fun(Int, Text) in Bool"), "(var f : (fun (Int Text) in Bool))");
    assert_eq!(ast("var s in notes_data.Student"), "(var s : notes_data.Student)");
}

#[test]
fn semicolons_do_not_end_statements() {
    assert_eq!(first_error("let x = 10;"), "unexpected `;`");
    assert_eq!(first_error(";"), "unexpected `;`");
}

#[test]
fn lines_cannot_start_with_an_operator() {
    assert_eq!(first_error("total = a + b\n      + c"), "a line cannot start with an operator");
    assert_eq!(first_error("x = 1\n- 2"), "a line cannot start with an operator");
}

#[test]
fn comparison_combinations_need_parentheses() {
    let message = "these comparisons cannot be combined without parentheses";
    assert_eq!(first_error("a < b in c"), message);
    assert_eq!(first_error("a in b in c"), message);
    assert_eq!(first_error("a in b == c"), message);
    assert_eq!(first_error("1..2..3"), "`..` cannot be chained");
}

#[test]
fn syntax_errors() {
    assert_eq!(first_error("let X = 1"), "`X` cannot name a value");
    assert_eq!(first_error("let test = 1"), "`test` is a reserved keyword");
    assert_eq!(first_error("let x"), "`let x` needs a value or a type");
    assert_eq!(first_error("let x = "), "expected an expression, found end of file");
    assert_eq!(first_error("let x = 1 2"), "expected the end of the line, found a number");
    assert_eq!(first_error("let y = x = 5"), "unexpected `=`");
    assert_eq!(first_error("f(1"), "expected `)`, found end of file");
    assert_eq!(first_error("show(\"{}\")"), "empty interpolation");
    assert_eq!(first_error("var x in int"), "`int` is not a type");
}

#[test]
fn errors_recover_at_the_next_line() {
    let (tree, errors) = parse_text("let x = )\nlet y = 2\nlet z = (\n");
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert_eq!(tree, "(let y 2)\n");
}

#[test]
fn recovery_skips_the_blocks_of_a_failed_statement() {
    let text = "fun f(a: 1):\n    let inner = 1\n    if a: x = 1 ;\n;\nlet after = 2\n";
    let (tree, errors) = parse_text(text);
    assert_eq!(errors, ["not implemented yet: functions"]);
    assert_eq!(tree, "(let after 2)\n");
    let text = "if a:\n    b = 1\nelif c:\n    d = 2\nelse:\n    e = 3\n;\nlet after = 2\n";
    let (tree, errors) = parse_text(text);
    assert_eq!(errors, ["not implemented yet: `if` blocks"]);
    assert_eq!(tree, "(let after 2)\n");
}

#[test]
fn unsupported_constructions_are_reported() {
    let cases = [
        ("fun f() = 1", "not implemented yet: functions"),
        ("if x: show(x) ;", "not implemented yet: `if` blocks"),
        ("while x: x = 1 ;", "not implemented yet: loops"),
        ("struct S:\n;", "not implemented yet: structures"),
        ("Color = {red, green}", "not implemented yet: type definitions (enumerations and named unions)"),
        ("let l = [1, 2]", "not implemented yet: lists"),
        ("let s = {1, 2}", "not implemented yet: sets and comprehensions"),
        ("let t = (1, 2)", "not implemented yet: tuples"),
        ("let v = if c then 1 else 2", "not implemented yet: `if ... then ... else` expressions"),
        ("let v = try f()", "not implemented yet: `try`"),
    ];
    for (text, message) in cases {
        assert_eq!(first_error(text), message, "for {text:?}");
    }
}
