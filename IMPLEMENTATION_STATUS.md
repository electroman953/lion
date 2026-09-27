# État de l'implémentation de Lion

Mis à jour le 2026-09-27, au commit `285ff6d` (33 commits sur `main`). Ce fichier suffit pour reprendre le travail dans une nouvelle session. Il complète trois autres documents :

- [`docs/spec/lion-0.1.md`](docs/spec/lion-0.1.md) : la spécification, **source de vérité** ;
- [`docs/implementation-notes.md`](docs/implementation-notes.md) : chaque décision de l'implémentation (R1–R13, I1–I15, C1–C80) ;
- [`README.md`](README.md) : la présentation et l'usage.

## 1. Vérification faite pour ce bilan

| Contrôle | Résultat |
| --- | --- |
| `cargo build` | OK |
| `cargo clippy --all-targets` | 0 avertissement |
| `cargo fmt --check` | OK |
| `cargo test` (tout le workspace) | OK : 122 tests unitaires et 171 programmes golden |
| Programmes du §27 de la spec | 27.1 (CSV, structures) et 27.2 (hasard, parallèle, ensembles) tournent sans modification ; 27.2 prend environ 3 s en `--release`. 27.3 dépend du module `ui`, imaginaire |

L'arbre de travail est propre, sans fichier non commité.

## 2. Où en est la feuille de route (§28 de la spec)

| Étape | État |
| --- | --- |
| 1–2. Frontend complet et mode interprété | **Atteinte** |
| 3. Bibliothèque standard | **Atteinte** pour `files`, `text`, `math`, `random`, `csv` et le noyau. Manquent `sets`, `json`, `dates`, `time`, `net` |
| 4. Outillage | **Atteinte** : `lion test`, `lion fmt`, mode interactif |
| 5. Compilateur natif `lion build` | **Pas commencée** : la commande répond « not implemented yet ». Plan au §9 |
| 6. Parallélisme réel sur plusieurs cœurs | **Pas commencée** : les parties parallèles et les tâches s'exécutent l'une après l'autre, avec les mêmes résultats (C60, C71) |
| 7. Bibliothèque `ui` | Pas commencée |

## 3. Architecture

Workspace Cargo en Rust (édition 2024, Rust ≥ 1.88), **sans aucune dépendance externe**.

```
source .lion
  → lion_syntax   lexer → tokens → parser → AST ; formateur (lion fmt)
  → lion_sema     noms, types, flux, règles de sûreté → IR typé
  → lion_ir       IR typé : le contrat commun aux backends (+ visiteur d'expressions)
      └→ lion_vm      bytecode à registres + machine virtuelle ; évalue aussi `compile`
  lion_runtime       opérations primitives (arithmétique vérifiée, conversions, affichage, bugs)
  lion_std           modules de la bibliothèque standard, écrits en Lion (include_str!)
  lion_diagnostics   sources, positions, diagnostics et leur rendu façon rustc
  lion_cli           la commande `lion` et les tests golden
```

Principes :

- L'IR n'existe que pour un programme valide. Les noms y sont résolus, les conversions explicites, les opérateurs spécialisés par type (`AddInt`, `AddFloat`…). Un backend ne refait aucune analyse.
- `lion_runtime` est la seule implémentation des opérations primitives, pour que les deux modes donnent les mêmes résultats et les mêmes bugs (§22.2).
- Pipeline du driver (`crates/lion_cli/src/driver.rs`, `check_files`) :
  1. lire le script et, récursivement, les modules qu'il utilise (`use`) ;
  2. lexer, puis parser ;
  3. `lion_sema::check_program` ;
  4. `lion_vm::evaluate_compile`, qui remplace chaque `compile expr` par sa valeur ;
  5. `lion_vm::compile` vers le bytecode, puis `lion_vm::run`.
- La VM peut rappeler une fonction Lion depuis une instruction (`Machine::invoke`). Elle s'en sert pour `equals` dans les comparaisons, les Sets et les Maps.

## 4. Composants implémentés, fichier par fichier

**`lion_syntax`**
- `lexer.rs`, `token.rs` : lexer ; indentation mémorisée pour localiser un `;` oublié.
- `parser.rs`, `ast.rs` : grammaire du §26, avec récupération après erreur.
- `format.rs` : `lion fmt`.
- `print.rs` : la sortie de `lion debug ast`.

**`lion_sema`** (le cœur ; `lib.rs` contient `Checker` et `check_program`)
- Fonctions et méthodes :
  - `functions.rs` : enregistrement, signatures, instances génériques, appels, corps ;
  - `methods.rs` : méthodes et leur répartition ;
  - `closures.rs` : closures, valeurs de fonctions, méthodes détachées ;
  - `generic_structs.rs` : structures génériques.
- Expressions et instructions :
  - `expr.rs` : expressions, opérateurs, comparaisons, conversions, `same`, rationnels ;
  - `stmt.rs` : instructions, `let` et `var`, boucles, `unsafe` ;
  - `collections.rs` : listes, compréhensions, index, propriétés ;
  - `places.rs` : les modifications en place, comme `l[i] = v` ou `m[k] = v`.
- Types :
  - `types.rs` : résolution des types écrits ;
  - `structs.rs` : structures, invariants, validateur et constructeur synthétisés ;
  - `enums.rs` : énumérations et unions nommées ;
  - `traits.rs` : traits, `Comparable`, et `Error` comme trait ;
  - `equality.rs` : `equals`.
- Flux et `match` :
  - `flow.rs`, `narrowing.rs` : affectation définie et affinage ;
  - `matching.rs` : `match`.
- Règles de sûreté et partage :
  - `parallel.rs` : parties parallèles, §19.3 ;
  - `sharing.rs` : `shared`, `synced` et `same`.
- Calcul à la compilation :
  - `consteval.rs` : constantes évaluées à la compilation (D39, C48) ;
  - `compile_time.rs` : la vérification de `compile`.
- Le reste :
  - `modules.rs` : modules, globales, initialisation paresseuse ;
  - `domains.rs` : `Domain` ;
  - `ffi.rs` : fonctions C ;
  - `testing.rs` : `test` et `expect` ;
  - `standard.rs` : `show`, `ask`, `exit`, etc. ;
  - `names.rs` : résolution des noms.

**`lion_ir`**
- `lib.rs` : `Program`, `Function`, `Stmt`, `ExprKind`, `Builtin`, `Native`, `ForeignFunction`.
- `types.rs` : `Type`, qui est `Copy`, et l'interner global.
- `visit.rs` : parcours de toutes les expressions.
- `print.rs` : la sortie de `lion debug ir`.

**`lion_vm`**
- `compile.rs` : IR vers bytecode.
- `bytecode.rs` : les instructions typées.
- `machine.rs` : l'exécution, les pièges (`Trap`), `run`, `run_test`, `run_from` (le mode interactif) et `invoke`.
- Valeurs :
  - `value.rs` : `Value`, qui est `Rc` et copie à l'écriture ;
  - `set.rs`, `map.rs` : index par hachage ;
  - `natives.rs` : fonctions de la bibliothèque standard fournies par l'implémentation.
- Appels C et `compile` :
  - `ffi.rs` : appels C par `dlopen` ;
  - `constants.rs` : l'évaluation de `compile`.

**`lion_runtime`** : `ops.rs` (arithmétique vérifiée, rationnels, conversions), `bug.rs` (les `BugKind` et leurs messages), `format.rs` (affichage des Float et des rationnels), `stdlib.rs`.

**`lion_std/std/*.lion`** : `csv`, `files`, `math`, `random`, `text`.

**`lion_cli`** : `main.rs` (les commandes), `driver.rs` (le pipeline), `tests/golden.rs` (le lanceur des tests golden).

## 5. Fonctionnalités de Lion supportées (mode interprété)

Toutes sont testées par des programmes golden.

- **Bases** :
  - scripts, `let` et `var`, déclarations sans valeur, annotations ;
  - `if`, `elif`, `else`, `while`, `for`, `break`, `continue`, `return` ;
  - `match` en instruction et en expression, avec exhaustivité ;
  - comparaisons enchaînées, textes avec interpolation.
- **Nombres** :
  - Int 64 bits, dont le débordement est un bug ;
  - Float IEEE ;
  - Rational exact (`1 over 3`, C72) ;
  - puissance `^`, `div` et `mod` euclidiens ;
  - conversions `as`.
- **Fonctions** :
  - paramètres typés ou non, les seconds rendant la fonction générique ;
  - paramètres `var`, valeurs par défaut, arguments nommés ;
  - récursion, `modifies` ;
  - closures, fonctions comme valeurs, curryfication ;
  - fonctions anonymes, y compris génériques : `let twice = fun(x) = x * 2` (C78).
- **Collections** :
  - List, Set, Range, n-uplets ;
  - Map (`Map of (K, V)`, API minimale C79) ;
  - Domain (`{x in Int, x > 0}`, C74) ;
  - compréhensions, avec les bornes reconnues pour un générateur sur `Int` ;
  - `union`, `inter`, `minus`, `subset`.
- **Types** :
  - unions, `maybe`, affinage ;
  - structures, avec invariants, méthodes, `var self` et vérification des constantes à la compilation ;
  - structures génériques (`struct Pair of (A, B)`, C75) ;
  - énumérations ordonnées ou non ;
  - traits avec méthodes par défaut ;
  - variables de type (`T in Comparable`) ;
  - `equals` (C76).
- **Erreurs** : `Error` est un trait. Aussi : `error(...)`, `try`, `e.message()`, les erreurs propres au programme, et des bugs avec trace d'appels.
- **Modules** : `use`, `private`, globales initialisées au premier usage, cycles entre modules.
- **Concurrence** :
  - `task` et `wait` ;
  - `parallel [...]`, `parallel {...}`, `parallel for` ;
  - `shared`, `shared synced`, `same` (C73) ;
  - règles de sûreté du §19.3 et D48.
  L'exécution reste séquentielle.
- **Divers** :
  - `compile expr` (C77) ;
  - appels C : `foreign "libm" pure fun ...` dans des blocs `unsafe` (C80 ; Unix x86-64 et AArch64) ;
  - `test` et `expect` ;
  - les alertes du mode interprété (§22.3).

### Pas encore supporté

Chacun de ces cas donne une erreur « not implemented yet » ou un refus explicite :

| Fonctionnalité | Réf. spec |
| --- | --- |
| `lion build`, le compilateur natif | §22, étape 5 |
| Parallélisme réel sur plusieurs cœurs | §19, étape 6 |
| Traits génériques (`trait Container of T`) | §15.1 |
| Méthodes d'une structure générique ou de List/Set/Map (`fun Pair.swap()`) | §12.4, §15 |
| Une structure générique écrite avec une variable de type (`p in Pair of (T, T)`) | §15.1 |
| Types génériques d'un autre module (`geometry.Pair of (...)`) | §15.1 |
| Types comme valeurs (`let t = Int`) | §7.1 |
| Fonctions standard comme valeurs (`let f = show`) | §11, §23 |
| Lire un élément de n-uplet : la spec ne dit pas comment (C53) | §16 |
| Modules `sets`, `json`, `dates`, `time`, `net`, `ui` de la bibliothèque standard | §23 |
| Écriture littérale d'une Map, que la spec laisse ouverte (§29) | C79 |

## 6. Tests

```sh
cargo test --workspace                     # tout : tests unitaires + golden
cargo test --test golden                   # seulement les programmes golden
LION_BLESS=1 cargo test --test golden      # régénère les .expected après un changement voulu, puis relire le diff
```

Chaque test golden est un fichier `tests/<suite>/*.lion` accompagné de son `.expected`, qui contient le code de sortie, stdout et stderr. Le lanceur est `crates/lion_cli/tests/golden.rs`.

| Dossier | Fichiers | Commande exercée |
| --- | --- | --- |
| `tests/lexer` | 4 | `lion debug tokens` |
| `tests/parser` | 23 | `lion debug ast` |
| `tests/typechecker` | 15 | `lion debug ir` |
| `tests/errors` | 48 | `lion check` (erreurs de compilation) |
| `tests/runtime` | 62 | `lion run` (sémantique, bugs, alertes) |
| `tests/integration` | 13 | `lion run` (programmes complets) |
| `tests/programs` | 2 | `lion run` depuis leur dossier (programmes 27.1 et 27.2 de la spec) |
| `tests/testing` | 3 | `lion test` |
| `tests/interactive` | 1 | `lion` seul, le fichier en entrée |

Des tests unitaires existent aussi dans les crates suivantes : `lion_syntax` (49), `lion_sema` (42), `lion_runtime` (18), `lion_vm` (7), `lion_diagnostics` (4) et `lion_ir` (2).

## 7. Commandes

```sh
cargo build --release
./target/release/lion run exemple.lion
./target/release/lion check exemple.lion
./target/release/lion test [fichier.lion | dossier]
./target/release/lion fmt [--check] [fichier.lion | dossier]
./target/release/lion debug tokens|ast|ir|bytecode exemple.lion
./target/release/lion                       # mode interactif
```

Codes de sortie : 0 succès, 1 programme refusé, 2 bug à l'exécution, 64 ligne de commande incorrecte, 70 erreur interne.

## 8. Problèmes connus et limites

- Parallélisme et tâches séquentiels (C60, C71) : mêmes résultats, mais pas de gain de vitesse ; seul l'ordre des `show` pourrait différer d'une exécution concurrente.
- FFI :
  - seulement sur Unix x86-64 et AArch64, sans fonction variadique ;
  - Int vaut `int64_t` et Bool vaut `int` ;
  - au plus 6 arguments entiers et 8 flottants (C80).
- `same` se décide à la compilation (C73). Un champ ne peut pas garder un lien vivant vers un objet partagé, puisque la grammaire n'a pas de champ `var`.
- Un argument `var` qui est une variable partagée avec une closure passe par une copie, rangée au retour (C50) ; la closure appelée pendant l'appel voit l'ancienne valeur.
- La visibilité des méthodes par module n'est pas prise en compte pour la conformité aux traits (C67).
- `lion fmt` ne change que l'indentation (C69).
- La profondeur d'appel est limitée à 100 000 : au-delà, c'est le bug « stack overflow ».
- Les alertes : une seule par emplacement du code (I5).

## 9. Prochaine étape recommandée : le compilateur natif (étape 5)

Rien n'est commencé dans le dépôt. Voici le plan retenu, à reprendre tel quel :

1. **Choix.** `lion build f.lion [-o sortie]` traduit l'IR typé en Rust, compilé par `cargo build --release` (rustc, donc LLVM, comme le demande le §22.1). Le code produit est lié à une bibliothèque d'exécution qui réutilise `lion_vm` : `Value`, Set, Map, natives, FFI, `Trap`. Les deux modes partagent ainsi le même code, donc les mêmes résultats et les mêmes bugs (§22.2). Il faut une chaîne Rust installée, ce que la commande devra vérifier en le disant clairement.
2. **Refactorisation préalable, sans changer de comportement.** Extraire de `machine.rs` vers un module `lion_vm::shared` :
   - l'égalité (`equal`) ;
   - les opérations de Set et de Map (`set_contains`, `set_insert`, `map_position`…) ;
   - les chemins de modification (`key_positions`, `element_mut`) ;
   - `get_index`, `get_slice`, `position`, `range_size`, `range_sum`.

   Ces fonctions prennent un trait `Comparer` (qui appelle `equals`) et une erreur `Stop { Bug(BugKind), Fault(Box<Trap>) }`. La machine implémente `Comparer` avec `invoke`. Tous les tests golden doivent rester verts.
3. **Nouvelles crates.**
   - `lion_native` : le runtime des programmes compilés. Il contient `Rt` (sortie tamponnée, entrée, profondeur d'appel, modules initialisés, structures et énumérations, table des fonctions « dynamiques » pour les valeurs de fonction et `equals`, cache des symboles C), une fonction `start()` (fil d'exécution à grosse pile, rendu des bugs avec les sources embarquées, mêmes codes de sortie que `lion run`, sans alertes) et des aides.
   - `lion_codegen` : IR vers texte Rust.
4. **Représentation dans le code produit.**
   - Types :
     - Int, Float et Bool deviennent `i64`, `f64` et `bool` ;
     - tout le reste devient `Value` ;
     - on convertit aux frontières selon le type de stockage et le type de l'expression. Une variable affinée garde son type de stockage.
   - Globales : `static mut` typées.
   - Paramètres :
     - un paramètre `var` devient `*mut T` ;
     - un paramètre avec valeur par défaut devient `Option<T>`, et le défaut est évalué dans l'appelé.
   - Chaque fonction devient `fn fN(rt, params…, captures…) -> Result<T, Box<Trap>>`, avec une enveloppe dynamique pour les valeurs de fonction.
   - Appels :
     - les arguments sont évalués d'abord ;
     - puis `rt.enter()` vérifie la profondeur ;
     - puis `.map_err(called_from(span))` reconstruit la trace d'appels.
   - Boucles `for` : sur un instantané de la collection.
5. **Commandes et tests.**
   - `lion debug rust f.lion` montre le code produit.
   - `lion build` embarque les sources des crates d'exécution (via `build.rs`) dans un cache.
   - Une suite golden « native » compile tous les `tests/runtime` et `tests/programs` en un seul binaire, puis compare aux mêmes `.expected`, sans les blocs `alert:`.

Ensuite viendra l'étape 6, le parallélisme réel. Des sujets plus petits peuvent aussi être repris au besoin : traits génériques, méthodes de structures génériques, types et fonctions standard comme valeurs, modules `json` et `dates`.

## 10. Conventions de travail

- La spec est la source de vérité. Une ambiguïté se tranche selon les règles de `docs/implementation-notes.md`, puis s'y consigne (Cn suivant : **C81**). Une construction non définie est refusée avec un diagnostic, jamais inventée en silence. L'auteur a délégué toutes les décisions (2026-09-26).
- Travail par tranches verticales. Chaque tranche passe par : implémentation, tests golden et unitaires, `cargo build`, `clippy`, `fmt`, `test`, mise à jour du README et des notes, puis un commit Conventional Commits. Chaque message de commit se termine par :
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_016or6ey4wHpi3riTFJbw1TL
  ```
- Langues :
  - code, commentaires et diagnostics en anglais ;
  - documentation (README, notes, ce fichier) en français ;
  - résumés à l'auteur en français.
- Diagnostics façon rustc : titre, `-->`, extrait, notes et `help`. Un bug d'exécution affiche la trace des appels. Une erreur dans la bibliothèque standard est montrée à l'appel du programme.
- Une commande de la CLI n'apparaît qu'une fois réellement fonctionnelle. Sinon, elle répond explicitement « not implemented yet ».
