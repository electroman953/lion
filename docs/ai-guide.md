# Writing Lion — a guide for AI assistants

Lion is a statically typed language with value semantics, run by `lion run` (a virtual
machine) or compiled by `lion build` (native code, 3 to 13 times faster, same results).
This guide is what you need to write correct Lion on the first try. The full
specification is `docs/spec/lion-0.1.md` (French) in the Lion repository; the exact API
of each standard module is given by the tool `lion_module` (its source, with comments).

**Workflow.** Write a file, call `lion_check` after every change (it takes
milliseconds), fix every diagnostic, then `lion_run` or `lion_test`. Diagnostics are
precise and usually say the fix (`help:`). When Lion lacks something you need — a
function, a module, a construct — work around it, and call `lion_report_gap` so that
the language gets it.

## 1. Blocks, lines, comments

```lion
// A comment. `:` opens a block, `;` closes it. Indentation is 4 spaces.
if x > 0:
    show("positive")
elif x == 0:
    show("zero")
else:
    show("negative")
;                                   // one `;` closes the whole if/elif/else
for s in students: show(s.name) ;   // a one-line block
while n > 1:
    n = n div 2
;
```

A statement ends at the end of the line; a long expression continues only inside
parentheses, brackets or braces:

```lion
let app = (net.router()
    .get("/", home)
    .post("/save", save))
```

## 2. Variables and values

```lion
let a = 5               // constant, deeply immutable
var b = 5 in Float      // variable with a type: 5.0
var names = [] in List of Text    // an empty collection needs its type
b += 1                  // also -=, *=
```

- Value semantics: assigning or passing a List/structure copies it (lazily). A function
  changes its caller's variable only through a `var` parameter.
- Names are ASCII (`a-z A-Z 0-9 _`); types start with an uppercase letter.
- Reserved words cannot name anything: `test`, `of`, `in`, `as`, `div`, `mod`, `over`,
  `task`, `wait`, `parallel`, `shared`, `synced`, `same`, `match`, `then`, `otherwise`,
  `maybe`, `use`, `private`, `compile`, `modifies`, `infix`, `foreign`, `pure`, `unsafe`,
  `expect`, `self`, `union`, `inter`, `minus`, `subset`…
- `_` throws a value away: `let _ = f()`, `for _ in 1..3`, `let (a, _) = t`; it is never read.

## 3. Types

`Int` (64 bits, overflow is a bug), `Float`, `Rational` (`1 over 3`), `Bool`, `Text`,
`None` (value `none`), `List of T`, `Set of T`, `Map of (K, V)`, tuples `(a, b)`, ranges
`1..9` (both ends included), `A or B`, `maybe T` (= `T or None`), `Task of T`,
`fun(Int, Text) in Bool`.

- `7 / 2` is the Float `3.5`; `div` and `mod` give integers (euclidean); `7 over 2` is
  the exact Rational.
- `^` is power. `and`, `or`, `not`. Chained comparisons: `0 <= x <= 20`.
- Conversions: `3.7 as Int` (3), `x as Text`, `"12" as Int` gives `Int or Error`.
- Lists and Texts are indexed **from 1**: `l[1]`, `l[l.size]`, slices `l[2..4]`
  (both ends included; `l[2..1]` is empty). `t[i]` is a one-character Text.
- `l.size`, `l.first`, `l.last`, `l.add(x)` (on its own line), `x in l`, `sum(l)`,
  `reverse(l)`. Maps: `let m = {"a" -> 1, "b" -> 2}`, `var e = {} in Map of (Text, Int)`,
  `m["a"] = 1`, `"a" in m`, `m.get("a")` (`maybe`), `for k in m`.
- Tuples: `let t = (1, "a")`, `t[1]` (a position written in digits), `let (n, s) = t`,
  `for (name, grade) in pairs: … ;`.
- Sorting: `sets.sorted(l)` (numbers, enumerations), `text.sorted(names)` (dictionary
  order), `sets.sorted_by(l, fun(p in Person) = p.age)`,
  `sets.sorted_with(l, fun(a in P, b in P) = text.before(a.name, b.name))`.

## 4. Texts

```lion
let who = "Léa"
show("Hello {who}, you have {n * 2} points")   // `{…}` inserts any expression
```

- **There is no `+` on Texts**: build them with interpolation, `"{a}{b}"`, or
  `text.join(parts, ", ")`.
- Escapes: `\n \t \\ \" \{ \}`. A literal brace is `\{`.
- So a regular expression `\d{4}` is written `"\\d\{4\}"`.
- Several lines: `"""` ends its line, the text follows, and `"""` closes on a line of its
  own; its indentation is removed from every line. Quotes need no escape inside:

  ```lion
  let page = """
      <p class="note">Hello {name}</p>
      """
  ```

- Numbers in texts: `text.fixed(12.5, 2)` gives `12.50`; `math.round_to(x, 2)`;
  `text.pad_left("7", 3, "0")` gives `007`; `text.repeat("-", 20)`.
- A type shows itself with a method `fun T.text() in Text`: `show(x)` and `"{x}"` use it.

## 5. Functions

```lion
fun square(x in Int) in Int = x * x               // one-line form
fun greet(name in Text, polite = true) in Text:   // defaults come last
    if polite: return "Hello {name}" ;
    return "Hi {name}"
;
fun grow(var l in List of Int):                    // changes the caller's variable
    l.add(1)
;
greet("Léa", polite: false)   // named arguments, in any order, after the others; they may skip defaults
let f = fun(x in Int) = x + 1 // a function value; `f(2)`
```

- A parameter without a type makes the function generic (one version per argument
  types). Type variables: `fun biggest(a in T, b in T) in T, T in Comparable`.
- Reading a global from a function is free; **changing one needs `modifies`**:
  `fun count() modifies total: total += 1 ;`
- Methods: `fun Student.passes() in Bool = self.grade >= 10`; a method that changes
  the object takes `var self` first: `fun Counter.bump(var self): self.n += 1 ;`.
  Methods can be added to any type, even `Text` or `Int`.

## 6. Structures, enumerations, unions

```lion
struct Exam:
    student in Text
    score in Float = 0, 0 <= score      // default, then conditions
    max in Float = 20
    score <= max                        // invariant
    private note in Text = ""
;
let e = Exam("Léa", 12.5)               // fields in order; last ones with defaults may be omitted
let f = Exam(student: "Max", score: 9)  // names, same order
// With values not known at compile time and conditions, `Exam(...)` gives `Exam or Error`.

Color = {red, green, blue}              // enumeration; `Color.red`, or `red` where a Color is expected
Days = [mon, tue, wed]                  // ordered enumeration: `mon < tue`
Shape = Circle or Rect                  // closed union of structures
```

## 7. Optional values, errors, match

```lion
let p = text.find(t, "x")        // maybe Int
if p != none: show(p + 1) ;      // after the test, p is an Int (narrowing)

fun load(path in Text) in Int or Error:
    let content = try files.read(path)      // an Error returns from `load` at once
    return try text.trim(content) as Int
;

match load("n.txt"):
    in Error e: show("failed: {e.message()}") ;
    in Int n, n >= 10: show("big") ;
    in Int n: show("small") ;
;
let label = match n:
    0 then "none"
    in 1..9 then "few"
    otherwise then "many"
;
```

- **`try` covers the whole expression on its right**: `try a.get("x").int()` applies
  `try` to `.int()`. Write `(try a.get("x")).int()` or `try (try a.get("x")).int()`.
  Same for comparisons: `(try json.load(p)) == v`.
- `try` is allowed only in a function whose return type has `Error`, or at the top level
  of the script (it stops the script with the message).
- A `maybe T` or `T or Error` value must be tested before use as a `T`
  (`if x == none: return ;`, `if x in Error: … ;`, `match`), or given a default:
  `let page = r.query("page") otherwise "1"`.
- `error("message")` makes an Error; `e.message()` reads it. A *bug* (overflow, index
  out of range, invalid pattern…) stops the program: it is not an Error.

## 8. Comprehensions, ranges, sets

```lion
let evens = [x, x in 1..20, x mod 2 == 0]
let names = [s.name, s in students, s.grade >= 10]
let pairs = [(a, b), a in 1..3, b in 1..3]
let unique = {w, w in words}             // a Set
```

## 9. Modules and programs

- The top-level code of a file runs: `show("Hello")` is a program.
  `exit(code)` ends with an exit code.
- `use text`, `use shapes.circle` (the file `shapes/circle.lion`); names are qualified:
  `text.upper(t)`, `geometry.Point(1, 2)`. `private` hides a declaration in its file.
- A project: `lion new name` (lion.toml + main.lion); `lion add <git url | folder>`.
- Standard modules (call `lion_module` for each API): `text`, `math`, `random`,
  `files`, `csv`, `json`, `sets`, `dates`, `time`, `net`, `html`, `regex`, `url`,
  `base64`, `hash`, `system`, `term`, `ui`. Core, without `use`: `show`, `ask`, `exit`,
  `error`, `sum`, `reverse`, `floor`, `ceil`, `round`, `isqrt`.

## 10. Tests

```lion
test "square":
    expect square(3) == 9
;
```

`lion test file.lion` (or a folder) runs them; the other top-level statements do not run,
except the declarations of globals with a value.

## 11. Concurrency and performance

- `let t = task f(x)` then `let r = wait t`. A task runs on its own thread when it reads
  nothing that can change (no `var` global); otherwise it runs at once.
- `parallel for f in files: analyse(f) ;`, `parallel [g(x), x in l]`: on all cores.
  What runs in parallel cannot change anything outside it (checked at compile time):
  share with `var c = shared synced Counter()`; a function that writes files or calls
  another program cannot run there.
- **Speed**: `lion build f.lion -o f` gives a native executable, 3–13× faster than
  `lion run`, with identical output. Use `parallel` only on heavy work (it costs).
  Build a long Text with `text.join` of a List rather than many interpolations in a loop.
  `compile expr` computes a constant while compiling.

## 12. Scripts, web, servers (the common recipes)

```lion
use files
use json
use net
use system

let args = system.parse_args(["out"])            // --out file, -v, rest in args.rest
let page = try net.get("https://example.com")    // needs the program `curl`
let data = try page.json()                       // json.Value
struct Repo:
    name in Text
    stars in Int = 0
;
let repo = try data as Repo                      // json.Value as any Lion type, or Error
show(json.from(repo).pretty())                   // any Lion value to JSON
for f in try files.find(".", "*.\{jpg,png\}"): show(f) ;
let out = try system.run("git", ["status", "--short"])
```

```lion
use net
fun user(r in net.Request) in net.Response = net.plain("user {r.param("id")}")
let app = net.router().get("/users/:id", user).files("/", "public")
try net.serve(8080, app.handle)       // net.serve_parallel(8080, app.handle) for pure handlers
```

HTML scraping: `html.parse(t).select("li.item > a")`, `e.text`, `e.attr("href")`;
`url.join(base, href)`; `regex.all("\\d+", t)`. Binary data is `Bytes`, never a Text:
`net.get_bytes(url)`, `files.read_bytes(p)`, `files.write_bytes(p, b)`,
`net.bytes(b, "image/png")`, `r.bytes()`, `b.size`, `b[1]` (an Int), `bytes.hex(b)`,
`t as Bytes`, `b as Text` (`Text or Error`).

## 13. What Lion does not have yet

Plan around these; `lion_gaps` gives the full, current list.

- Texts have no `<`: sort them with `text.sorted` or `sets.sorted_with`.
- `T.text()` applies to the value itself; inside a List the fields are shown.
- A generic function leaving out a default before a named argument fixes the type of
  the next parameters to those of their defaults.

## 14. Mistakes that the checker will report (avoid them)

| You write | Write instead |
| --- | --- |
| `"a" + "b"` | `"{a}{b}"` |
| `try x.get("a").int()` | `(try x.get("a")).int()` |
| `fun test()`, `fun of()` | another name: they are keywords |
| `x ?? default` | `x otherwise default` |
| `[] ` without a type | `[] in List of Int`, or a parameter `l in List of T = []` |
| a multi-line `"…"` | `"""` on its own lines |
| `{4}` in a pattern | `\{4\}` |
| changing a global in a function | add `modifies name` |
