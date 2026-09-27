# Lion

Lion est un langage polyvalent, interprété ou compilé, dont l'écriture et la logique sont proches des mathématiques. Sa [spécification](docs/spec/lion-0.1.md) est la source de vérité de ce dépôt ; les choix d'implémentation sont consignés dans [docs/implementation-notes.md](docs/implementation-notes.md).

## État

Les étapes 2 à 7 de la feuille de route (§28) sont atteintes :
- les trois programmes du §27 de la spec tournent tels quels (`tests/programs`), dont 27.3, l'application graphique ;
- le compilateur natif `lion build` donne les mêmes résultats que le mode interprété sur tous les programmes de test ;
- les parties parallèles utilisent tous les cœurs ;
- la bibliothèque graphique `ui` ouvre de vraies fenêtres (X11, et Wayland par XWayland), dessinées par Lion lui-même, sans dépendance.

Le bilan détaillé, avec les limites connues et les prochaines étapes, est dans [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md). L'implémentation construit le langage par tranches verticales qui fonctionnent réellement de bout en bout. Ce qui n'est pas encore implémenté est refusé avec le message `not implemented yet`, suivi de la section de la spec concernée.

**Ce qui fonctionne aujourd'hui**

- **Programmes-scripts** (§20.1) : `let`, `var`, déclaration sans valeur (`let e in Text`), annotation (`in Float`), redéclaration dans le même bloc, `=`, `+=`, `-=`, `*=`.
- **Contrôle de flux** : blocs `if`/`elif`/`else`, `while` et `for` (sur plusieurs lignes ou sur une seule), intervalles `a..b` et `x in a..b`, `break`, `continue`, `return` pour terminer le script, expression `if … then … else`. Les portées suivent les blocs ; un nom ne peut pas être redéclaré dans un bloc intérieur (§6.5).
- **Fonctions comme valeurs** (§11) : type `fun(Int) in Int`, fonctions passées en argument, rangées dans des listes ou des champs, fonctions anonymes `fun(x in Int) = x * 2`, closures (copie des variables à la création, partage avec `modifies`), curryfication `f(1)(2)`, fonctions standard et fonctions des modules comme valeurs (`each(l, show)`, `apply(text.upper, t)`).
- **Fonctions** (§11) : déclarations `fun` en bloc ou en forme courte (`fun square(x) = x * x`), paramètres sans type qui rendent la fonction générique (une version par types d'arguments, §15.3), types de retour écrits ou inférés, `return` vérifié sur tous les chemins, récursion, paramètres `var` qui modifient la variable de l'appelant, valeurs par défaut, arguments nommés, globales lues par les fonctions et modifiées avec `modifies`.
- **Listes** (§16) : `[1, 2, 3]`, `List of T`, indices à partir de 1 et extraits `l[2..4]` (bug hors limites), `size`, `first`, `last`, `add`, `l[i] = v`, `x in l`, égalité par contenu, `for x in l`, compréhensions `[f(x), x in l, condition]`, `sum`, avec la sémantique de valeur (§17.1). Les Text s'indexent aussi par caractère.
- **Ensembles et n-uplets** (§16) : `{1, 2}`, `Set of T`, compréhensions `{f(x), x in l, condition}`, `x in s`, `union`, `inter`, `minus`, `subset`, `size`, `for`, `sum`, `add`, `remove` ; n-uplets `(a, b)` comparables et utilisables dans les collections.
- **Méthodes des collections** (§12.4) : `fun List.second() in maybe T, T in Type`, sur List, Set et Map (C97).
- **Maps** (§16.1) : `var counts = {} in Map of (Text, Int)`, `counts[w] = 1`, `counts[w] += 1`, `w in counts`, `counts.get(w)`, `counts.remove(w)`, `for w in counts` (API minimale choisie en C79).
- **Domaines** (§16.5) : `{x in Int, x > 0}` est un `Domain of Int`, qu'on teste avec `in` et combine avec `union`, `inter`, `minus` (`Int minus positives`) ; `{x in Int, 1 <= x, x <= n}` est un Set, car ses bornes sont reconnues.
- **Unions et erreurs** (§7.3, §7.4, §18) : `maybe T`, `A or B`, tests de type `x in T`, affinage après un test ou une sortie anticipée, `if … then` sans `else`, `Error` (tout type qui a `message()` est une erreur), `error("…")`, `e.message()`, `"12" as Int` qui donne `Int or Error`, `try` dans une fonction ou au niveau du script, `match` en instruction et en expression (motifs de valeur, de type `in Int g`, d'appartenance `in 1..9`, conditions, `otherwise`, exhaustivité vérifiée).
- **Structures** (§12) : `struct` avec champs, valeurs par défaut, conditions et invariants, construction `T(...)`, avec noms, `(...) as T` et `let x = (...) in T`, champs finaux omis. Une construction avec des constantes est vérifiée à la compilation, sinon elle donne `T or Error`. Modifier un champ qui viole un invariant est un bug. Égalité champ par champ, méthodes (`fun Student.passes()`, `var self`, y compris sur `Int` ou `Text`) qui suivent les valeurs de leur type dans les autres modules, structures récursives (`maybe Node`), structures dans les unions et les `match`.
- **Énumérations et unions nommées** (§13) : `Color = {red, green}`, `Days = [mon, tue]` ordonnée (`<`, parcours avec `for`), `Color.red` ou `red` seul quand le type est connu, `match` exhaustif sur les valeurs, `Shape = Circle or Rect`.
- **Types** : `Int` (64 bits, débordement = bug), `Float` (IEEE 754), `Rational` (fractions exactes `1 over 3`, toujours simplifiées), `Bool`, `Text`, `None`, avec conversion automatique Int → Float.
- **Structures génériques** (§15.1) : `struct Pair of (A, B), A in Type, B in Type`, `Pair(1, "one")` qui déduit les types, `Pair of (Int, Text)`, structures récursives `Node of T`, méthodes (`fun Pair.swap() in Pair of (B, A)`), variables de type dans les fonctions génériques (`p in Pair of (T, U)`), structures génériques d'un autre module (`geometry.Pair of (Int, Text)`).
- **Traits et variables de type** (§14, §15.2) : conformité structurelle, méthodes par défaut, `List of Shape` avec appel choisi à l'exécution, `x in Shape` ; `fun biggest(a in T, b in T) in T, T in Comparable` ; traits génériques (`trait Container of T`), avec `T` trouvé dans les méthodes d'un type.
- **Opérateurs définis par les types** (§9.5) : méthodes `plus`, `subtract`, `times`, `divide`, `power`, `negate`, `less`, et fonctions `infix` (`u dot v`) ; `equals` (§12.5), qui sert aussi à `x in l` et aux Sets.
- **Opérateurs** :
  - `+ - * /` ;
  - `div` et `mod` euclidiens ;
  - `^` ;
  - comparaisons enchaînées (`0 <= x <= 20`) ;
  - `and`, `or` (court-circuit), `not` ;
  - conversions `as` entre nombres et vers Text.
- **Textes** : échappements et interpolation `"x = {x}"`.
- **Modules** (§20) : `use geometry`, `use shapes.circle`, noms qualifiés (`geometry.area(...)`, `geometry.Point`), `private`, globales initialisées au premier usage, modules qui s'utilisent mutuellement.
- **Bibliothèque standard** (§23), écrite en Lion : `files`, `text`, `math`, `random` (générateurs reproductibles), `csv`, `sets` (tri, `min`, `max`, ensemble des parties, `any`, `all`, `count`, `group`), `json` (liste d'objets lue en lignes, `r.get("nom")`), `time` (horloge, mesure des durées, attente) `dates` (dates vérifiées, `d + 30`, `b - a`, jour de la semaine, format ISO) et `ui` (fenêtres : titres, textes, boutons, champs, cases à cocher, colonnes et lignes).
- **Tâches** (§19.1) : `task f(x)`, `wait t`, avec les règles de sûreté du §19.3 ; méthodes détachées `s.passes` (§12.6). Une tâche qui ne lit rien que le programme peut changer tourne sur son propre fil ; ce qu'elle écrit apparaît à son `wait` (C85).
- **Partage explicite** (§17.2) : `var score = shared Counter()`, `shared synced` pour les tâches, `a same b`, méthodes détachées d'un objet partagé (`score.increment`), avec les règles de la spec vérifiées à la compilation.
- **Parallélisme de données** (§19.2) : `parallel [...]`, `parallel {...}`, `parallel for`, avec les règles de sûreté du §19.3 vérifiées à la compilation. Les tours s'exécutent sur tous les cœurs (ou `LION_THREADS` fils), dans les deux modes, avec le résultat du calcul séquentiel : la sortie arrive dans l'ordre des tours, et le premier bug dans cet ordre arrête le programme (C83).
- **Appels C** (§21.2) : `foreign "libm" pure fun cos(x in Float) in Float`, appelée dans un bloc `unsafe:` (Unix, x86-64 et AArch64).
- **Calcul à la compilation** (§21.1) : `let primes = compile {p in 2..1_000_000, is_prime(p)}` ; le programme contient directement la valeur, et une expression à effets est refusée.
- **Tests intégrés** (§24.1) : `test "nom": ... ;`, `expect a == b` qui montre « expected 6, got 5 », commande `lion test`.
- **Bibliothèque standard, noyau** (§23) : `show`, `ask`, `exit`, `error`, `sum`, `reverse`, `floor`, `ceil`, `round`, `isqrt`.
- **Vérifications à la compilation** : types, noms inconnus (avec suggestions), constantes réaffectées, lecture d'une variable qui peut ne pas avoir de valeur sur un des chemins (§6.1), `;` oublié localisé grâce à l'indentation (§5.3).
- **Bugs à l'exécution** (§18) : débordement, division entière par zéro, fraction de dénominateur nul, exposant négatif, conversion Float → Int impossible, récursion sans fin (plus de 100 000 appels imbriqués). Chacun est signalé avec l'emplacement, les appels en cours, les valeurs en cause et une suggestion.
- **Alertes du mode interprété** (§22.3) : infini, NaN, perte de précision.
- **Mode compilé** (§22) : `lion build f.lion` produit un exécutable natif, par Rust et LLVM, qui donne exactement la même sortie, les mêmes bugs et le même code de sortie que `lion run`, sans les alertes. Il va de 3 à 13 fois plus vite que la machine virtuelle sur nos mesures.

**Pas encore implémenté** : types comme valeurs, lecture des éléments d'un n-uplet, module `net`, débogueur. La liste complète est dans [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md).

## Interfaces graphiques

```lion
use ui

var count = 0
fun increment() modifies count:
    count += 1
;
fun view() in ui.Element = ui.column([ui.Title("Compteur"), ui.Label("Valeur : {count}"), ui.Button("Plus un", on_click: increment)])
ui.run(view)
```

`ui.run(view)` ouvre une fenêtre et la redessine après chaque événement (voir C98 et la [conception](docs/design/ui.md)). Il faut un écran X11 : sous Wayland, XWayland suffit. Pour les tests, `LION_UI=headless` remplace l'écran par un fichier d'événements (`LION_UI_EVENTS`) et écrit chaque frame en texte ; `LION_UI_SNAPSHOT=image.ppm` enregistre l'image.

## Construire et utiliser

Il faut Rust 1.88 ou plus récent (<https://rustup.rs>).

```sh
cargo build --release
./target/release/lion run exemple.lion
```

```lion
let x = 10
let y = 20
let z = x + y

show(z)          // 30
```

| Commande | Rôle |
| --- | --- |
| `lion run f.lion` | vérifie puis exécute en mode interprété, avec les alertes |
| `lion check f.lion` | vérifie sans exécuter |
| `lion build f.lion [-o exécutable]` | compile en code natif (il faut Rust, voir plus bas) |
| `lion test [f.lion \| dossier]` | lance les blocs `test "nom": ... ;` et leurs `expect` |
| `lion fmt [--check] [f.lion \| dossier]` | met en page selon le style officiel (4 espaces par bloc) |
| `lion` | mode interactif : on tape du Lion ligne par ligne |
| `lion debug tokens\|ast\|ir\|bytecode\|rust f.lion` | montre une étape du compilateur |

Codes de sortie : 0 succès, 1 programme refusé, 2 bug à l'exécution, 64 ligne de commande incorrecte, 70 erreur interne. Un programme compilé sort avec les mêmes codes que `lion run`.

`lion build` traduit le programme en Rust, puis le compile avec cargo : il faut donc Rust sur la machine qui compile, pas sur celle qui exécute. La première compilation prépare le runtime des programmes compilés dans un cache (`~/.cache/lion`), une fois pour toutes ; les suivantes prennent moins d'une seconde pour un petit programme.

```sh
lion build notes.lion        # crée l'exécutable ./notes
./notes
```

## Tests

```sh
cargo test --workspace
```

- **Tests unitaires** : dans chaque crate.
- **Tests golden** : dans `tests/`, chaque fichier `.lion` passe par le vrai binaire `lion`. Le code de sortie, stdout et stderr sont comparés au fichier `.expected` voisin.

| Dossier | Commande testée |
| --- | --- |
| `tests/lexer` | `lion debug tokens` |
| `tests/parser` | `lion debug ast` |
| `tests/typechecker` | `lion debug ir` |
| `tests/runtime` | `lion run` (sémantique, bugs, alertes) |
| `tests/errors` | `lion check` (erreurs de compilation) |
| `tests/integration` | `lion run` (programmes complets) |
| `tests/programs` | `lion run`, depuis leur dossier (les programmes du §27 de la spec) |
| `tests/testing` | `lion test` (tests intégrés et `expect`) |
| `tests/interactive` | `lion` seul, avec le fichier en entrée (mode interactif) |

Après un changement voulu de sortie, régénérer avec `LION_BLESS=1 cargo test --test golden`, puis relire le diff.

- **Tests du mode compilé** (`cargo test --test native`) : tous les programmes de `tests/runtime`, `tests/integration` et `tests/programs` sont compilés en natif, puis comparés aux **mêmes** fichiers `.expected`, sans les alertes. C'est la garantie « deux modes, une sémantique » (§22.2). Un second test lance `lion build` lui-même.

## Architecture

```
source .lion
  → lion_syntax   lexer → tokens → parser → AST
  → lion_sema     résolution des noms, vérification des types → IR typé
  → lion_ir       IR typé : le contrat commun aux backends
      ├→ lion_vm      bytecode à registres + machine virtuelle (mode interprété)
      └→ lion_codegen traduction en Rust, compilée par rustc et LLVM (mode compilé)
  lion_native        runtime des programmes compilés, qui partage les valeurs et les
                     opérations de lion_vm
  lion_runtime       sémantique des opérations primitives, partagée par les backends
  lion_std           les modules de la bibliothèque standard, écrits en Lion
  lion_ui            fenêtres de la bibliothèque `ui` : protocole X11, dessin, police intégrée
  lion_diagnostics   positions, erreurs, bugs, alertes et leur rendu
  lion_cli           la commande `lion` ; charge le script et les modules qu'il utilise
```
