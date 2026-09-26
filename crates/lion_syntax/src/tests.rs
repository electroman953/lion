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
    assert_eq!(ast("let ok = (3 in Int)"), "(let ok (paren (in-type 3 Int)))");
    assert_eq!(ast("let s = x in m.Student"), "(let s x : m.Student)");
    assert_eq!(ast("let n = a and b in Bool"), "(let n (and a b) : Bool)");
    assert_eq!(ast("let n = a < b in Bool"), "(let n (< a b) : Bool)");
    assert_eq!(ast("if_ok = 3 in Int"), "(= if_ok (in-type 3 Int))");
}

#[test]
fn precedence_examples_from_the_spec() {
    // §9.1, "Exemples de lecture".
    assert_eq!(ast("1..n - 1"), "(.. 1 (- n 1))");
    assert_eq!(ast("p + 2 in primes"), "(in (+ p 2) primes)");
    assert_eq!(ast("x + 1 as Text"), "(as (+ x 1) Text)");
    assert_eq!(ast("y = not x in S"), "(= y (not (in-type x S)))");
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
    let text = "struct S(a: 1):\n    let inner = 1\n    if a: x = 1 ;\n;\nlet after = 2\n";
    let (tree, errors) = parse_text(text);
    assert_eq!(errors, ["not implemented yet: structures"]);
    assert_eq!(tree, "(let after 2)\n");
    let text = "struct S:\n    if a:\n        b = 1\n    elif c:\n        d = 2\n    else:\n        e = 3\n    ;\n;\nlet after = 2\n";
    let (tree, errors) = parse_text(text);
    assert_eq!(errors, ["not implemented yet: structures"]);
    assert_eq!(tree, "(let after 2)\n");
}

#[test]
fn unsupported_constructions_are_reported() {
    let cases = [
        ("parallel for f in files: show(f) ;", "not implemented yet: parallelism"),
        ("struct S:\n;", "not implemented yet: structures"),
        ("Color = {red, green}", "not implemented yet: type definitions (enumerations and named unions)"),
        ("let s = {1, 2}", "not implemented yet: sets and comprehensions"),
        ("let t = (1, 2)", "not implemented yet: tuples"),
    ];
    for (text, message) in cases {
        assert_eq!(first_error(text), message, "for {text:?}");
    }
}

#[test]
fn if_blocks() {
    assert_eq!(
        ast(
            "if x > 0:\n    show(\"positif\")\nelif x == 0:\n    show(\"nul\")\nelse:\n    show(\"négatif\")\n;"
        ),
        "(if (> x 0) [(call show \"positif\")] elif (== x 0) [(call show \"nul\")] else [(call show \"négatif\")])"
    );
    assert_eq!(ast("if x > 0: show(x) ;"), "(if (> x 0) [(call show x)])");
    assert_eq!(
        ast("if a: show(1) elif b: show(2) else: show(3) ;"),
        "(if a [(call show 1)] elif b [(call show 2)] else [(call show 3)])"
    );
    assert_eq!(ast("if a:\n    x = 1\nelse: x = 2 ;"), "(if a [(= x 1)] else [(= x 2)])");
    assert_eq!(ast("if a:\n;"), "(if a [])");
    assert_eq!(ast("if a: if b: show(1) ; ;"), "(if a [(if b [(call show 1)])])");
    assert_eq!(ast("if a:\n\n    x = 1\n\n    y = 2\n\n;"), "(if a [(= x 1) (= y 2)])");
}

#[test]
fn loops_and_jumps() {
    assert_eq!(ast("while n > 1:\n    n = n div 2\n;"), "(while (> n 1) [(= n (div n 2))])");
    assert_eq!(ast("while true: break ;"), "(while true [break])");
    assert_eq!(
        ast("while a:\n    if b: continue ;\n    return\n;"),
        "(while a [(if b [continue]) (return)])"
    );
    assert_eq!(ast("return"), "(return)");
    assert_eq!(ast("return x + 1"), "(return (+ x 1))");
    assert_eq!(ast("if a: return ;"), "(if a [(return)])");
}

#[test]
fn if_expressions() {
    assert_eq!(
        ast("let sign = if x > 0 then 1 elif x == 0 then 0 else -1"),
        "(let sign (if-expr (> x 0) 1 elif (== x 0) 0 else (neg 1)))"
    );
    assert_eq!(ast("let bonus = if late then 0"), "(let bonus (if-expr late 0))");
    // The last branch covers everything to its right (§9.1), up to an annotation.
    assert_eq!(ast("let v = if c then 1 else 2 + 3"), "(let v (if-expr c 1 else (+ 2 3)))");
    assert_eq!(ast("let v = if c then 1 else 2 in Float"), "(let v (if-expr c 1 else 2) : Float)");
    assert_eq!(
        ast("let v = if a then if b then 1 else 2 else 3"),
        "(let v (if-expr a (if-expr b 1 else 2) else 3))"
    );
    assert_eq!(ast("let v = 1 + (if c then 2 else 3)"), "(let v (+ 1 (paren (if-expr c 2 else 3))))");
    assert_eq!(ast("if c then show(1) else show(2)"), "(if-expr c (call show 1) else (call show 2))");
}

#[test]
fn block_errors() {
    assert_eq!(first_error("if a: show(1)\nshow(2)"), "the one-line `if` block is not closed");
    assert_eq!(first_error("if a:\n    show(1) ;"), "`;` must start a new line here");
    assert_eq!(first_error("if a:\n    show(1)"), "the `if` block is never closed");
    assert_eq!(
        first_error("if a:\n    show(1)\nelse:\n    show(2)\nelse:\n    show(3)\n;"),
        "unexpected `else`"
    );
    assert_eq!(first_error("while a:\n    show(1)\nelse:\n    show(2)\n;"), "unexpected `else`");
    assert_eq!(first_error("else:\n    show(2)\n;"), "`else` without `if`");
    assert_eq!(first_error("if a show(1) ;"), "expected `:` to open the block, found name `show`");
    assert_eq!(first_error("let v = if a 1"), "expected `then`, found a number");
    assert_eq!(first_error("if a: ;"), "a one-line block needs a statement");
}

#[test]
fn a_forgotten_semicolon_is_located_with_indentation() {
    let parse_diagnostic = |text: &str| {
        let mut map = SourceMap::new();
        let id = map.add("t.lion", text);
        let lexed = lex(id, text);
        let mut diagnostics = parse(&lexed.tokens).diagnostics;
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let diagnostic = diagnostics.remove(0);
        let lines: Vec<usize> =
            diagnostic.labels.iter().map(|label| map.get(id).line_col(label.span.start).0).collect();
        (diagnostic.message, lines)
    };
    // The next statement is less indented than the block: the `;` was forgotten before it.
    assert_eq!(
        parse_diagnostic("if a:\n    show(1)\nshow(2)\n"),
        ("the `if` block is never closed".to_string(), vec![1, 3])
    );
    // The `;` of line 5 closes the inner `if`, although it is aligned with the `while`.
    assert_eq!(
        parse_diagnostic("while c:\n    if a:\n        show(1)\n    show(2)\n;\n"),
        ("the `while` block is never closed".to_string(), vec![1, 4])
    );
    // `else` aligned with its `if` is not a sign of a forgotten `;`.
    assert_eq!(
        parse_diagnostic(
            "while c:\n    if a:\n        show(1)\n    else:\n        show(2)\n    show(3)\n;\n"
        ),
        ("the `while` block is never closed".to_string(), vec![1, 6])
    );
    // Lines are counted in the source, blank lines and comments included.
    assert_eq!(
        parse_diagnostic("// note\n\nwhile c:\n\n    if a:\n        show(1)\n    show(2)\n;\n"),
        ("the `while` block is never closed".to_string(), vec![3, 7])
    );
    // Nothing is less indented: no guess.
    assert_eq!(
        parse_diagnostic("if a:\n    show(1)\n"),
        ("the `if` block is never closed".to_string(), vec![1])
    );
}

#[test]
fn function_declarations() {
    assert_eq!(
        ast("fun area(width in Float, height in Float) in Float:\n    return width * height\n;"),
        "(fun area ((width : Float) (height : Float)) in Float [(return (* width height))])"
    );
    assert_eq!(ast("fun square(x) = x * x"), "(fun square ((x)) = (* x x))");
    assert_eq!(
        ast("fun add_grade(var notes, n):\n    notes = n\n;"),
        "(fun add_grade ((var notes) (n)) [(= notes n)])"
    );
    assert_eq!(
        ast("fun f(a, c = 1, d in Int = 2): show(a) ;"),
        "(fun f ((a) (c = 1) (d : Int = 2)) [(call show a)])"
    );
    assert_eq!(ast("fun reset() modifies total, count:\n;"), "(fun reset () (modifies total count) [])");
    assert_eq!(
        ast("fun biggest(a in T, b in T) in T, T in Comparable = if b > a then b else a"),
        "(fun biggest ((a : T) (b : T)) in T (where T Comparable) = (if-expr (> b a) b else a))"
    );
    assert_eq!(
        ast("fun Student.passes() in Bool = self.grade >= 10"),
        "(fun Student.passes () in Bool = (>= (. self grade) 10))"
    );
    assert_eq!(
        ast("fun Student.add_bonus(var self, n): show(n) ;"),
        "(fun Student.add_bonus ((var self) (n)) [(call show n)])"
    );
    assert_eq!(
        ast("infix fun Vector.dot(other in Vector) in Float = 0.0"),
        "(infix-fun Vector.dot ((other : Vector)) in Float = 0.0)"
    );
}

#[test]
fn function_declaration_errors() {
    assert_eq!(first_error("fun f"), "expected `(` and the parameters, found end of file");
    assert_eq!(
        first_error("fun f() show(1)"),
        "expected `:` and the body of the function, or `=` and its value, found name `show`"
    );
    assert_eq!(first_error("fun f():\n    show(1)\n"), "the `fun` block is never closed");
    assert_eq!(first_error("fun Area() = 1"), "`Area` cannot name a value");
    assert_eq!(first_error("fun(x) = x"), "not implemented yet: anonymous functions");
}

#[test]
fn for_loops() {
    assert_eq!(ast("for i in 1..3: show(i) ;"), "(for i (.. 1 3) [(call show i)])");
    assert_eq!(
        ast("for s in students:\n    if s > 10: continue ;\n    show(s)\n;"),
        "(for s students [(if (> s 10) [continue]) (call show s)])"
    );
    assert_eq!(ast("for d in 2..isqrt(n): show(d) ;"), "(for d (.. 2 (call isqrt n)) [(call show d)])");
    assert_eq!(
        first_error("for i 1..3: show(i) ;"),
        "expected `in` and the values to go through, found a number"
    );
    assert_eq!(first_error("for i in 1..3:\n    show(i)\n"), "the `for` block is never closed");
}

#[test]
fn lists() {
    assert_eq!(ast("let l = [1, 2, 3]"), "(let l (list 1 2 3))");
    assert_eq!(ast("var l = [] in List of Int"), "(var l (list) : (List of Int))");
    assert_eq!(ast("let l = [\n    1,\n    2\n]"), "(let l (list 1 2))");
    assert_eq!(
        ast("let n = [s.name, s in students, s.grade >= 10]"),
        "(let n (list (. s name) (in s students) (>= (. s grade) 10)))"
    );
    assert_eq!(ast("let e = l[2..4]"), "(let e (index l (.. 2 4)))");
    assert_eq!(first_error("let l = [1, 2,]"), "expected an expression, found `]`");
    assert_eq!(first_error("let l = [a: 1]"), "the elements of a list have no name, like `a:`");
}

#[test]
fn type_tests_and_try() {
    assert_eq!(ast("if v in Float: show(v) ;"), "(if (in-type v Float) [(call show v)])");
    assert_eq!(
        ast("let ok = (r in List of Int or Error)"),
        "(let ok (paren (in-type r (or (List of Int) Error))))"
    );
    assert_eq!(ast("let ok = (x in primes)"), "(let ok (paren (in x primes)))");
    assert_eq!(ast("y = not x in None"), "(= y (not (in-type x None)))");
    // In a declaration, a final `in T` is still the annotation (§26 rule 1).
    assert_eq!(ast("let x = 3 in Int"), "(let x 3 : Int)");
    assert_eq!(ast("let g = try r.get(1) as Float"), "(let g (try (as (call (. r get) 1) Float)))");
    assert_eq!(ast("return try text as Int"), "(return (try (as text Int)))");
}
