# Lion

Lion est un langage polyvalent, interprété ou compilé, dont l'écriture et la logique sont proches des mathématiques. Sa [spécification](docs/spec/lion-0.1.md) est la source de vérité de ce dépôt ; les choix d'implémentation sont consignés dans [docs/implementation-notes.md](docs/implementation-notes.md).

## État

L'étape 2 de la feuille de route (§28) est atteinte : les programmes 27.1 et 27.2 de la spec tournent tels quels (`tests/programs`). L'implémentation construit le langage par tranches verticales qui fonctionnent réellement de bout en bout. Ce qui n'est pas encore implémenté est refusé avec le message `not implemented yet`, suivi de la section de la spec concernée.

**Ce qui fonctionne aujourd'hui**

- **Programmes-scripts** (§20.1) : `let`, `var`, déclaration sans valeur (`let e in Text`), annotation (`in Float`), redéclaration dans le même bloc, `=`, `+=`, `-=`, `*=`.
- **Contrôle de flux** : blocs `if`/`elif`/`else`, `while` et `for` (sur plusieurs lignes ou sur une seule), intervalles `a..b` et `x in a..b`, `break`, `continue`, `return` pour terminer le script, expression `if … then … else`. Les portées suivent les blocs ; un nom ne peut pas être redéclaré dans un bloc intérieur (§6.5).
- **Fonctions comme valeurs** (§11) : type `fun(Int) in Int`, fonctions passées en argument, rangées dans des listes ou des champs, fonctions anonymes `fun(x in Int) = x * 2`, closures (copie des variables à la création, partage avec `modifies`), curryfication `f(1)(2)`.
- **Fonctions** (§11) : déclarations `fun` en bloc ou en forme courte (`fun square(x) = x * x`), paramètres sans type qui rendent la fonction générique (une version par types d'arguments, §15.3), types de retour écrits ou inférés, `return` vérifié sur tous les chemins, récursion, paramètres `var` qui modifient la variable de l'appelant, valeurs par défaut, arguments nommés, globales lues par les fonctions et modifiées avec `modifies`.
- **Listes** (§16) : `[1, 2, 3]`, `List of T`, indices à partir de 1 et extraits `l[2..4]` (bug hors limites), `size`, `first`, `last`, `add`, `l[i] = v`, `x in l`, égalité par contenu, `for x in l`, compréhensions `[f(x), x in l, condition]`, `sum`, avec la sémantique de valeur (§17.1). Les Text s'indexent aussi par caractère.
- **Ensembles et n-uplets** (§16) : `{1, 2}`, `Set of T`, compréhensions `{f(x), x in l, condition}`, `x in s`, `union`, `inter`, `minus`, `subset`, `size`, `for`, `sum`, `add` ; n-uplets `(a, b)` comparables et utilisables dans les collections.
- **Unions et erreurs** (§7.3, §7.4, §18) : `maybe T`, `A or B`, tests de type `x in T`, affinage après un test ou une sortie anticipée, `if … then` sans `else`, `Error`, `error("…")`, `e.message()`, `"12" as Int` qui donne `Int or Error`, `try` dans une fonction ou au niveau du script, `match` en instruction et en expression (motifs de valeur, de type `in Int g`, d'appartenance `in 1..9`, conditions, `otherwise`, exhaustivité vérifiée).
- **Structures** (§12) : `struct` avec champs, valeurs par défaut, conditions et invariants, construction `T(...)`, avec noms, `(...) as T` et `let x = (...) in T`, champs finaux omis. Une construction avec des constantes est vérifiée à la compilation, sinon elle donne `T or Error`. Modifier un champ qui viole un invariant est un bug. Égalité champ par champ, méthodes (`fun Student.passes()`, `var self`, y compris sur `Int` ou `Text`), structures récursives (`maybe Node`), structures dans les unions et les `match`.
- **Énumérations et unions nommées** (§13) : `Color = {red, green}`, `Days = [mon, tue]` ordonnée (`<`, parcours avec `for`), `Color.red` ou `red` seul quand le type est connu, `match` exhaustif sur les valeurs, `Shape = Circle or Rect`.
- **Types** : `Int` (64 bits, débordement = bug), `Float` (IEEE 754), `Bool`, `Text`, `None`, avec conversion automatique Int → Float.
- **Traits et variables de type** (§14, §15.2) : conformité structurelle, méthodes par défaut, `List of Shape` avec appel choisi à l'exécution, `x in Shape` ; `fun biggest(a in T, b in T) in T, T in Comparable`.
- **Opérateurs définis par les types** (§9.5) : méthodes `plus`, `subtract`, `times`, `divide`, `power`, `negate`, `less`, et fonctions `infix` (`u dot v`).
- **Opérateurs** :
  - `+ - * /` ;
  - `div` et `mod` euclidiens ;
  - `^` ;
  - comparaisons enchaînées (`0 <= x <= 20`) ;
  - `and`, `or` (court-circuit), `not` ;
  - conversions `as` entre nombres et vers Text.
- **Textes** : échappements et interpolation `"x = {x}"`.
- **Modules** (§20) : `use geometry`, `use shapes.circle`, noms qualifiés (`geometry.area(...)`, `geometry.Point`), `private`, globales initialisées au premier usage, modules qui s'utilisent mutuellement.
- **Bibliothèque standard** (§23), écrite en Lion : `files`, `text`, `math`, `random` (générateurs reproductibles) et `csv`.
- **Tâches** (§19.1) : `task f(x)`, `wait t`, avec les règles de sûreté du §19.3 ; méthodes détachées `s.passes` (§12.6).
- **Parallélisme de données** (§19.2) : `parallel [...]`, `parallel {...}`, `parallel for`, avec les règles de sûreté du §19.3 vérifiées à la compilation. L'exécution reste séquentielle pour l'instant (mêmes résultats).
- **Tests intégrés** (§24.1) : `test "nom": ... ;`, `expect a == b` qui montre « expected 6, got 5 », commande `lion test`.
- **Bibliothèque standard, noyau** (§23) : `show`, `ask`, `exit`, `error`, `sum`, `reverse`, `floor`, `ceil`, `round`, `isqrt`.
- **Vérifications à la compilation** : types, noms inconnus (avec suggestions), constantes réaffectées, lecture d'une variable qui peut ne pas avoir de valeur sur un des chemins (§6.1), `;` oublié localisé grâce à l'indentation (§5.3).
- **Bugs à l'exécution** (§18) : débordement, division entière par zéro, exposant négatif, conversion Float → Int impossible, récursion sans fin (plus de 100 000 appels imbriqués). Chacun est signalé avec l'emplacement, les appels en cours, les valeurs en cause et une suggestion.
- **Alertes du mode interprété** (§22.3) : infini, NaN, perte de précision.

**Pas encore implémenté** : structures et traits génériques (`Pair of (A, B)`), fonctions anonymes génériques, égalité définie par `equals`, lecture des éléments d'un n-uplet, `Domain` et `Map`, valeurs `shared`, appel de code C, exécution concurrente des tâches et sur plusieurs cœurs, compilateur natif, débogueur.

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
| `lion test [f.lion \| dossier]` | lance les blocs `test "nom": ... ;` et leurs `expect` |
| `lion fmt [--check] [f.lion \| dossier]` | met en page selon le style officiel (4 espaces par bloc) |
| `lion` | mode interactif : on tape du Lion ligne par ligne |
| `lion debug tokens\|ast\|ir\|bytecode f.lion` | montre une étape du compilateur |

Codes de sortie : 0 succès, 1 programme refusé, 2 bug à l'exécution, 64 ligne de commande incorrecte, 70 erreur interne.

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

## Architecture

```
source .lion
  → lion_syntax   lexer → tokens → parser → AST
  → lion_sema     résolution des noms, vérification des types → IR typé
  → lion_ir       IR typé : le contrat commun aux backends
      ├→ lion_vm      bytecode à registres + machine virtuelle (mode interprété)
      └→ (à venir)    compilateur natif
  lion_runtime       sémantique des opérations primitives, partagée par les backends
  lion_std           les modules de la bibliothèque standard, écrits en Lion
  lion_diagnostics   positions, erreurs, bugs, alertes et leur rendu
  lion_cli           la commande `lion` ; charge le script et les modules qu'il utilise
```
