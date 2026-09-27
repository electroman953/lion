//! Type checker tests: source in, typed IR or error messages out.

use lion_diagnostics::SourceMap;
use lion_ir::print_program;
use lion_syntax::{lex, parse};

use crate::check;

fn check_text(text: &str) -> Result<String, Vec<String>> {
    let mut map = SourceMap::new();
    let id = map.add("t.lion", text);
    let lexed = lex(id, text);
    let parsed = parse(text, &lexed.tokens);
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
    // The body of the script ends where the first function starts.
    body.lines().take_while(|line| line.starts_with(' ')).map(str::trim).collect::<Vec<_>>().join("\n")
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
        "ok#1 = (let %t#2 g#0 (and (le_int 0 %t#2) (le_int %t#2 20)))"
    );
    assert_eq!(
        body("let a = 1\nlet ok = a < 2 < 3.5").lines().nth(1).unwrap(),
        "ok#1 = (let %t#2 a#0 (and (lt_int %t#2 2) (lt_float (int_to_float 2) 3.5)))"
    );
    assert_eq!(
        body("let a = 1\nlet ok = 0 < a + 1 < a * 2 < 10").lines().nth(1).unwrap(),
        "ok#1 = (let %t#2 (add_int a#0 1) (and (lt_int 0 %t#2) \
         (let %t#3 (mul_int a#0 2) (and (lt_int %t#2 %t#3) (lt_int %t#3 10)))))"
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
}

#[test]
fn names_and_values() {
    assert_eq!(errors("show(totl)"), ["cannot find `totl` in this scope"]);
    assert_eq!(errors("x = 1"), ["cannot find `x` in this scope"]);
    assert_eq!(errors("let x = 1\nx = 2"), ["cannot assign to the constant `x`"]);
    assert_eq!(errors("let e in Text\ne = \"a\"\ne = \"b\""), ["the constant `e` already has a value"]);
    assert_eq!(
        errors("let e in Text\nshow(e)"),
        ["the constant `e` never receives a value", "`e` is used before it has a value"]
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
    assert_eq!(errors("var s in Map of (Text, Int)"), ["not implemented yet: collections"]);
}

#[test]
fn unions_and_narrowing() {
    assert!(check_text("var m in maybe Int\nm = 3\nshow(m + 1)").is_ok());
    assert_eq!(
        errors("let n = \"12\" as Int\nshow(n + 1)"),
        ["`+` cannot be applied to Int or Error and Int"]
    );
    // A test narrows in its branch; an early exit narrows after it (§7.4).
    assert!(check_text("let n = \"12\" as Int\nif n in Int: show(n + 1) ;").is_ok());
    assert!(check_text("let n = \"12\" as Int\nif n in Error: return ;\nshow(n + 1)").is_ok());
    assert!(check_text("let n = \"12\" as Int\nif n in Int and n > 3: show(n) ;").is_ok());
    assert!(check_text("let n = \"12\" as Int\nif n in Error or n < 3: return ;\nshow(n + 1)").is_ok());
    // Paths that did not narrow keep the whole union.
    assert_eq!(
        errors("let n = \"12\" as Int\nif n in Int: show(n) ;\nshow(n + 1)"),
        ["`+` cannot be applied to Int or Error and Int"]
    );
    // Values of different types make a union (§7.3).
    let ir = check_text("let v = if true then 1 else \"one\"").unwrap();
    assert!(ir.contains("v#0 let Int or Text"), "{ir}");
}

#[test]
fn errors_and_try() {
    assert!(check_text("fun f(t in Text) in Int or Error = try t as Int\nshow(f(\"1\"))").is_ok());
    assert_eq!(
        errors("fun f(t in Text) in Int = try t as Int"),
        ["`try` cannot return an Error from a function that returns an Int"]
    );
    assert_eq!(errors("let x = try 5"), ["`try` needs a value that may be an Error; this is an Int"]);
    let ir = check_text("fun f(t in Text) = try t as Int").unwrap();
    assert!(ir.contains("fun f in Int or Error"), "{ir}");
    assert!(check_text("let e = error(\"oops\")\nshow(e.message())").is_ok());
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

#[test]
fn declarations_may_hide_standard_functions() {
    // C2: the standard library is not a block of the program.
    assert!(check_text("var sum = 0\nsum += 1\nshow(sum)").is_ok());
    assert_eq!(errors("let show = 1\nshow(show)"), ["`show` is not a function"]);
    assert_eq!(body("fun show(x in Int) = x + 1\nlet y = show(1)"), "y#0 = (call show 1)");
}

#[test]
fn generic_functions_have_one_instance_per_argument_types() {
    let ir = check_text("fun square(x) = x * x\nlet a = square(3)\nlet b = square(2)\nlet c = square(1.5)")
        .unwrap();
    let instances: Vec<&str> = ir.lines().filter(|line| line.starts_with("fun ")).collect();
    assert_eq!(instances, ["fun square[Int] in Int", "fun square[Float] in Float"]);
    // An omitted argument takes the type of its default value.
    let ir = check_text("fun scale(value, factor = 2) = value * factor\nlet a = scale(1.5)").unwrap();
    assert!(ir.contains("fun scale[Float, Int] in Float"), "{ir}");
    // A `var` parameter without a type takes the type of the variable.
    assert!(check_text("fun reset(var v, to) = 0\nvar t = \"a\"\nreset(t, 1)").is_ok());
}

#[test]
fn errors_in_generic_functions() {
    assert_eq!(
        errors("fun square(x) = x * x\nshow(square(\"a\"))"),
        ["`*` cannot be applied to Text and Text"]
    );
    // The same error in two instances is reported once.
    assert_eq!(
        errors("fun f(x) = x + missing\nshow(f(1))\nshow(f(2.5))"),
        ["cannot find `missing` in this scope"]
    );
    assert_eq!(
        errors("fun fact(n) = if n <= 1 then 1 else n * fact(n - 1)\nshow(fact(3))"),
        ["the return type of `fact` must be written"]
    );
    assert!(check_text("fun fact(n) in Int = if n <= 1 then 1 else n * fact(n - 1)\nshow(fact(3))").is_ok());
}

const STUDENT: &str = "struct Student:\n    name in Text\n    grade in Float, 0 <= grade <= 20\n;\n";

#[test]
fn constructions_from_constants_are_checked_now() {
    // D39: valid constants give a Student, with no check at run time.
    assert_eq!(
        body(&format!("{STUDENT}let s = Student(\"Léa\", 12)")),
        "s#0 = (struct Student \"Léa\" (int_to_float 12))"
    );
    assert_eq!(errors(&format!("{STUDENT}let s = Student(\"Léa\", 25)")), ["this `Student` is invalid"]);
    // Other values go through the constructor, which gives a `Student or Error`.
    let ir = check_text(&format!("{STUDENT}let g = 12.0\nlet s = Student(\"Léa\", g)")).unwrap();
    assert!(ir.contains("s#1 let Error or Student"), "{ir}");
    assert!(ir.contains("s#1 = (call Student \"Léa\" g#0)"), "{ir}");
    // Without conditions, a construction is always a plain value; defaults fill the end.
    assert_eq!(
        body("struct P:\n    x in Int = 0\n    y in Int = 0\n;\nlet p = P(3)"),
        "p#0 = (struct P 3 0)"
    );
}

#[test]
fn construction_forms() {
    let expected = "s#0 = (struct Student \"Léa\" (int_to_float 12))";
    assert_eq!(body(&format!("{STUDENT}let s = Student(name: \"Léa\", grade: 12)")), expected);
    assert_eq!(body(&format!("{STUDENT}let s = (\"Léa\", 12) as Student")), expected);
    assert_eq!(body(&format!("{STUDENT}let s = (\"Léa\", 12) in Student")), expected);
    assert_eq!(body(&format!("{STUDENT}let ok = ((\"Léa\", 25) in Student)")), "ok#0 = false");
    assert_eq!(
        errors(&format!("{STUDENT}let s = Student(grade: 12, name: \"Léa\")")),
        [
            "this value is named `grade`, but the field at this position is `name`",
            "this value is named `name`, but the field at this position is `grade`"
        ]
    );
    assert_eq!(
        errors(&format!("{STUDENT}let s = Student(\"Léa\")")),
        ["`Student` is built from 2 values, not 1"]
    );
}

#[test]
fn changes_of_fields_are_checked() {
    let ir = body(&format!("{STUDENT}var s = Student(\"Léa\", 12)\ns.grade = 14"));
    assert!(ir.contains("s#0.grade = (int_to_float 14)"), "{ir}");
    assert!(ir.contains("(call Student.check s#0)"), "{ir}");
    assert!(ir.contains("(broken \"Student\""), "{ir}");
    assert_eq!(
        errors(&format!("{STUDENT}let s = Student(\"Léa\", 12)\ns.grade = 14")),
        ["cannot change the content of the constant `s`"]
    );
    // A structure without conditions is not checked.
    assert_eq!(body("struct P:\n    x in Int\n;\nvar p = P(1)\np.x = 2"), "p#0 = (struct P 1)\np#0.x = 2");
}

#[test]
fn methods() {
    let text = format!(
        "{STUDENT}fun Student.passes() in Bool = self.grade >= 10\nfun Student.bump(var self):\n    self.grade += 1\n;\nvar s = Student(\"Léa\", 12)\nshow(s.passes())\ns.bump()"
    );
    let ir = check_text(&text).unwrap();
    assert!(ir.contains("fun Student.passes in Bool"), "{ir}");
    assert!(ir.contains("(call Student.bump &s#0)"), "{ir}");
    // A method without `var self` cannot change `self`; a constant cannot call one with it.
    assert_eq!(
        errors(&format!("{STUDENT}fun Student.reset():\n    self.grade = 0\n;")),
        ["this method cannot change `self`"]
    );
    assert_eq!(
        errors(&format!(
            "{STUDENT}fun Student.bump(var self):\n    self.grade += 1\n;\nlet s = Student(\"a\", 1)\ns.bump()"
        )),
        ["cannot change the content of the constant `s`"]
    );
    assert_eq!(errors("fun Int.twice() = self * 2\nshow(\"a\".twice())"), ["A Text has no method `twice`"]);
    assert_eq!(body("fun Int.twice() = self * 2\nshow(3.twice())"), "(show (call Int.twice 3))");
}

#[test]
fn conditions_of_structures() {
    assert_eq!(
        errors("var limit = 3\nstruct A:\n    x in Int, x < limit\n;"),
        ["a structure cannot read the variable `limit`"]
    );
    assert_eq!(errors("struct A:\n    x in Int, x + 1\n;"), ["a condition of a structure must be a Bool"]);
    assert_eq!(
        errors("fun f() in Int = 1\nstruct A:\n    x in Int = f()\n;"),
        ["the default value of a field is a constant"]
    );
}

const COLOR: &str = "Color = {red, green, blue}\nDays = [mon, tue, wed]\n";

#[test]
fn values_of_enumerations() {
    assert_eq!(body(&format!("{COLOR}let c = Color.red")), "c#0 = Color.red");
    // Alone, a value needs a type that expects it (D32).
    assert_eq!(body(&format!("{COLOR}let c = red in Color")), "c#0 = Color.red");
    assert_eq!(
        body(&format!("{COLOR}let c = Color.red\nlet b = c == blue")),
        "c#0 = Color.red\nb#1 = (eq_value c#0 Color.blue)"
    );
    assert_eq!(errors(&format!("{COLOR}let c = red")), ["cannot find `red` in this scope"]);
    assert_eq!(errors(&format!("{COLOR}let c = Color.purple")), ["`purple` is not a value of `Color`"]);
    assert_eq!(body(&format!("{COLOR}fun f(c in Color) = 1\nshow(f(green))")), "(show (call f Color.green))");
}

#[test]
fn order_of_enumerations() {
    assert_eq!(
        body(&format!("{COLOR}let d = Days.mon\nlet b = d < tue")),
        "d#0 = Days.mon\nb#1 = (lt_int (enum_position d#0) (enum_position Days.tue))"
    );
    assert_eq!(
        errors(&format!("{COLOR}let c = Color.red\nlet b = c < blue")),
        ["`<` is not defined for Color values"]
    );
}

#[test]
fn matches_on_enumerations() {
    let text = format!(
        "{COLOR}fun f(c in Color) in Int:\n    return match c:\n        red then 1\n        green then 2\n    ;\n;"
    );
    assert_eq!(errors(&text), ["this `match` has no case for `blue`"]);
    let text = format!(
        "{COLOR}fun f(c in Color) in Int:\n    return match c:\n        red then 1\n        green then 2\n        blue then 3\n    ;\n;"
    );
    assert!(check_text(&text).is_ok());
}

#[test]
fn named_unions() {
    let text = "struct Circle:\n    r in Float\n;\nstruct Rect:\n    w in Float\n;\nShape = Circle or Rect\nfun f(s in Shape) = 1\nshow(f(Circle(1.0)))";
    assert!(check_text(text).is_ok());
    assert_eq!(errors("A = B\nB = A"), ["the type `A` is defined by itself"]);
}

#[test]
fn sets_and_tuples() {
    assert_eq!(body("let s = {1, 2, 1}"), "s#0 = (set 1 2 1)");
    assert_eq!(body("let s = {1, 2}\nlet b = 1 in s"), "s#0 = (set 1 2)\nb#1 = (in_set 1 s#0)");
    assert_eq!(body("let s = {1} union {2}"), "s#0 = (union (set 1) (set 2))");
    assert_eq!(body("let t = (1, \"a\")"), "t#0 = (tuple 1 \"a\")");
    let ir = check_text("let s = {x, x in [1, 2]}").unwrap();
    assert!(ir.contains("s#0 let Set of Int"), "{ir}");
    assert_eq!(errors("let s = {}"), ["the type of this empty set is not known"]);
    assert_eq!(
        errors("let s = {1} union [2]"),
        ["`union` needs two Sets of the same type, not a Set of Int and a List of Int"]
    );
    assert_eq!(
        errors("let s = {a in [1], b in [2]}"),
        ["with several generators, a comprehension starts with its result"]
    );
}

#[test]
fn core_standard_functions() {
    assert_eq!(body("show(isqrt(10))"), "(show (isqrt 10))");
    // An Int is already whole: `floor` gives it back.
    assert_eq!(body("show(floor(3))"), "(show 3)");
    assert_eq!(body("show(round(2.5))"), "(show (round 2.5))");
    assert_eq!(body("show(reverse(\"ab\"))"), "(show (reverse \"ab\"))");
    assert_eq!(errors("show(isqrt(2.5))"), ["`isqrt` takes an Int, not a Float"]);
    // Nothing runs after `exit`: `x` has a value wherever it is read.
    assert!(check_text("let x in Int\nif true:\n    x = 1\nelse:\n    exit(1)\n;\nshow(x)").is_ok());
}

#[test]
fn parallel_parts_change_nothing_outside() {
    assert!(check_text("let r = parallel [x * 2, x in 1..3]").is_ok());
    assert_eq!(
        errors("var n = 0\nparallel for x in 1..3:\n    n += x\n;"),
        ["a parallel part cannot change `n`, which is declared outside it"]
    );
    assert_eq!(
        errors("var n = 0\nfun f() modifies n:\n    n += 1\n;\nparallel for x in 1..3:\n    f()\n;"),
        ["`f` modifies `n`, so it cannot run in parallel"]
    );
}
