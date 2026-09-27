# État de l'implémentation de Lion

Mis à jour le 2026-09-27, avec le compilateur natif (étape 5). Ce fichier suffit pour reprendre le travail dans une nouvelle session. Il complète trois autres documents :

- [`docs/spec/lion-0.1.md`](docs/spec/lion-0.1.md) : la spécification, **source de vérité** ;
- [`docs/implementation-notes.md`](docs/implementation-notes.md) : chaque décision de l'implémentation (R1–R13, I1–I15, C1–C82) ;
- [`README.md`](README.md) : la présentation et l'usage.

## 1. Vérification faite pour ce bilan

| Contrôle | Résultat |
| --- | --- |
| `cargo build` | OK |
| `cargo clippy --all-targets` | 0 avertissement |
| `cargo fmt --check` | OK |
| `cargo test` (tout le workspace) | OK : 134 tests unitaires, 172 programmes golden, et les 78 programmes de `tests/runtime`, `tests/integration` et `tests/programs` compilés en natif avec les mêmes sorties |
| Programmes du §27 de la spec | 27.1 (CSV, structures) et 27.2 (hasard, parallèle, ensembles) tournent sans modification, dans les deux modes ; 27.2 prend environ 3 s interprété (`--release`) et 0,6 s compilé. 27.3 dépend du module `ui`, imaginaire |

L'arbre de travail est propre, sans fichier non commité.

## 2. Où en est la feuille de route (§28 de la spec)

| Étape | État |
| --- | --- |
| 1–2. Frontend complet et mode interprété | **Atteinte** |
| 3. Bibliothèque standard | **Atteinte** pour `files`, `text`, `math`, `random`, `csv` et le noyau. Manquent `sets`, `json`, `dates`, `time`, `net` |
| 4. Outillage | **Atteinte** : `lion test`, `lion fmt`, mode interactif |
| 5. Compilateur natif `lion build` | **Atteinte** : les deux modes donnent les mêmes résultats sur tous les programmes de test (C81, C82) |
| 6. Parallélisme réel sur plusieurs cœurs | **Pas commencée** : les parties parallèles et les tâches s'exécutent l'une après l'autre, avec les mêmes résultats (C60, C71) |
| 7. Bibliothèque `ui` | Pas commencée |

## 3. Architecture

Workspace Cargo en Rust (édition 2024, Rust ≥ 1.88), **sans aucune dépendance externe**.

```
source .lion
  → lion_syntax   lexer → tokens → parser → AST ; formateur (lion fmt)
  → lion_sema     noms, types, flux, règles de sûreté → IR typé
  → lion_ir       IR typé : le contrat commun aux backends (+ visiteur d'expressions)
      ├→ lion_vm       bytecode à registres + machine virtuelle ; évalue aussi `compile`
      └→ lion_codegen  IR → code Rust, compilé par cargo/rustc (LLVM) : le mode compilé
  lion_native        runtime des programmes compilés (réutilise les valeurs de lion_vm)
  lion_runtime       opérations primitives (arithmétique vérifiée, conversions, affichage, bugs)
  lion_std           modules de la bibliothèque standard, écrits en Lion (include_str!)
  lion_diagnostics   sources, positions, diagnostics et leur rendu façon rustc
  lion_cli           la commande `lion`, les tests golden et les tests du mode compilé
```

Principes :

- L'IR n'existe que pour un programme valide. Les noms y sont résolus, les conversions explicites, les opérateurs spécialisés par type (`AddInt`, `AddFloat`…). Un backend ne refait aucune analyse.
- `lion_runtime` est la seule implémentation des opérations primitives, et `lion_vm::shared` celle des opérations sur les valeurs (égalité avec `equals`, Sets, Maps, modifications en place…). Les deux modes les appellent, pour donner les mêmes résultats et les mêmes bugs (§22.2).
- Pipeline du driver (`crates/lion_cli/src/driver.rs`, `check_files`) :
  1. lire le script et, récursivement, les modules qu'il utilise (`use`) ;
  2. lexer, puis parser ;
  3. `lion_sema::check_program` ;
  4. `lion_vm::evaluate_compile`, qui remplace chaque `compile expr` par sa valeur ;
  5. `lion_vm::compile` vers le bytecode, puis `lion_vm::run` ; ou, pour `lion build`, `lion_codegen::generate` vers du Rust, compilé par `native::build` (voir plus bas).
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
- Opérations partagées avec le mode compilé : `shared.rs`. Ces fonctions comparent les valeurs à travers le trait `Comparer` ; la machine l'implémente en appelant `equals` elle-même (`invoke`).

**`lion_codegen`** (`lib.rs`) : l'IR en Rust. Il suit pas à pas `lion_vm::compile` : même ordre d'évaluation, mêmes emplacements pour les bugs.
- Représentation :
  - Int, Float et Bool deviennent `i64`, `f64` et `bool` ; le reste devient `Value` ;
  - une variable partagée avec une closure tient une cellule, et un paramètre `var` est un pointeur (`*mut T`) ;
  - les globales atteintes par des fonctions deviennent des `static mut`.
- Une opération devient une instruction `let` (forme « ANF »).
- Chaque fonction devient `fN(rt, paramètres…, captures…) -> R<T>`. Un paramètre avec valeur par défaut est un `Option` ; `dN` est l'enveloppe pour les appels par valeur et `equals`.
- Tests unitaires dans `tests.rs` (programme Lion en entrée, forme du Rust en sortie).

**`lion_native`** (`lib.rs`) : le runtime des programmes compilés.
- `Program` : la description produite par le code, avec les sources, les structures, les textes et les fonctions dynamiques.
- `Rt` : sortie tamponnée, entrée, nombre d'appels en cours, modules initialisés, layouts, cache des fonctions C.
- `start` : lance le programme sur un fil à grosse pile (4 Gio, sinon 1 Gio, sinon 256 Mio), rend un bug comme `lion run` et sort avec les mêmes codes.
- Aides : `enter` et `leave` autour de chaque appel, `called_from` pour la trace, et des conversions (`int`, `text`, `field`…).

**`lion_runtime`** : `ops.rs` (arithmétique vérifiée, rationnels, conversions), `bug.rs` (les `BugKind` et leurs messages), `format.rs` (affichage des Float et des rationnels), `stdlib.rs`.

**`lion_std/std/*.lion`** : `csv`, `files`, `math`, `random`, `text`.

**`lion_cli`** :
- `main.rs` : les commandes ;
- `driver.rs` : le pipeline ;
- `native.rs` : `lion build`. Il écrit le runtime dans un cache, prépare un paquet cargo par exécutable, lance `cargo build --release --offline` et copie le binaire ;
- `build.rs` : embarque dans `lion` les sources et les manifestes des crates du runtime (`FILES`, `HASH`) ;
- `tests/golden.rs` : le lanceur des tests golden ;
- `tests/native.rs` : le lanceur des tests du mode compilé.

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
- **Mode compilé** : `lion build f.lion [-o exécutable]` (C81), avec la même sémantique que `lion run`, sans les alertes (C82). Il va 3 à 13 fois plus vite que la VM sur nos mesures (notes, §6).

### Pas encore supporté

Chacun de ces cas donne une erreur « not implemented yet » ou un refus explicite :

| Fonctionnalité | Réf. spec |
| --- | --- |
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
| `tests/runtime` | 63 | `lion run` (sémantique, bugs, alertes) |
| `tests/integration` | 13 | `lion run` (programmes complets) |
| `tests/programs` | 2 | `lion run` depuis leur dossier (programmes 27.1 et 27.2 de la spec) |
| `tests/testing` | 3 | `lion test` |
| `tests/interactive` | 1 | `lion` seul, le fichier en entrée |

Le mode compilé a ses propres tests (`cargo test --test native`, `crates/lion_cli/tests/native.rs`) :
- `compiled_programs_behave_as_interpreted` : les programmes de `tests/runtime`, `tests/integration` et `tests/programs` sont traduits par `lion debug rust` et compilés ensemble, comme les modules d'un seul exécutable, dans `target/tmp/native-suite`. Chacun est ensuite comparé à son `.expected`, alertes retirées. Les avertissements affichés par `lion debug rust` précèdent la sortie, comme avec `lion run`.
- `lion_build_makes_an_executable` : lance `lion build` avec un cache dans `target/tmp/lion-build`.

Ces tests demandent cargo, qu'ils trouvent dans la variable `CARGO` posée par `cargo test`. Un programme ajouté à `tests/runtime` est donc testé dans les deux modes.

Des tests unitaires existent aussi dans les crates suivantes : `lion_syntax` (49), `lion_sema` (42), `lion_runtime` (18), `lion_vm` (7), `lion_codegen` (7), `lion_native` (5), `lion_diagnostics` (4) et `lion_ir` (2).

## 7. Commandes

```sh
cargo build --release
./target/release/lion run exemple.lion
./target/release/lion check exemple.lion
./target/release/lion build exemple.lion [-o exécutable]   # il faut cargo (Rust ≥ 1.88)
./target/release/lion test [fichier.lion | dossier]
./target/release/lion fmt [--check] [fichier.lion | dossier]
./target/release/lion debug tokens|ast|ir|bytecode|rust exemple.lion
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
- Mode compilé :
  - `lion build` demande une chaîne Rust sur la machine qui compile ;
  - la première compilation prépare le runtime dans le cache (`LION_CACHE`, `$XDG_CACHE_HOME/lion` ou `~/.cache/lion`), ce qui prend quelques secondes ;
  - le cache garde un paquet par exécutable produit, et rien ne le nettoie ;
  - deux `lion build` simultanés vers le même exécutable se gênent ;
  - `lion test` et le mode interactif restent interprétés.

## 9. Prochaine étape recommandée : le parallélisme réel (étape 6)

L'étape 6 est « terminée quand le programme 27.2 utilise tous les cœurs » (§28). Rien n'est commencé. Aujourd'hui, les parties parallèles et les tâches s'exécutent l'une après l'autre (C60, C71), et le checker vérifie déjà les règles de sûreté du §19.3 (C60, C73, D48). Plan recommandé, à confirmer au début de l'étape :

1. **L'IR doit marquer ce qui est parallèle.** Aujourd'hui `parallel [...]` donne le même bloc qu'une compréhension ordinaire (`lion_sema/src/parallel.rs` vérifie, puis oublie le mot). Il faut un nœud, par exemple `ExprKind::Parallel { source, body: FunctionId, … }`. Son corps devient une fonction qui reçoit un élément et donne sa contribution (valeur ou rien), et `parallel for` suit la même forme. `task e` devient aussi une fonction sans argument.
2. **Des valeurs partageables entre fils.** `Value` repose sur `Rc` : il faut passer à `Arc`, ainsi que les Sets, les Maps, `Layout` et `Closure`, et les cellules à `Arc<Mutex<…>>`, ce qui donne aussi à `shared synced` son verrou. Il faut d'abord mesurer ce que coûte le comptage atomique sur les bancs d'essai des notes (§6). Si le coût est trop fort, l'alternative est de copier les valeurs aux frontières des parties parallèles. Elle se complique en mode compilé, où les globales `static` sont lues directement.
3. **Un ordonnanceur commun aux deux modes**, dans `lion_vm` et avec `std::thread::scope`. Il découpe les éléments en tranches, un fil par cœur (`available_parallelism`), et chaque fil a sa propre machine ou son propre `Rt`.
   - Les résultats sont rangés dans l'ordre des éléments. L'ordre d'un Set (C57) reste donc celui du calcul séquentiel.
   - La sortie de chaque élément est tamponnée, puis écrite dans l'ordre. Le §19.3 ne garantit pas l'ordre des lignes (D47) ; ce choix donne en plus une sortie déterministe, identique dans les deux modes et stable pour les tests golden.
   - Si plusieurs éléments rencontrent un bug, seul celui du premier élément compte : c'est le bug du calcul séquentiel, précédé des sorties des éléments d'avant.
   - Les modules que la partie peut utiliser sont initialisés avant de lancer les fils (D81), sur le fil principal.
   - Une partie qui appelle `ask`, même indirectement, reste séquentielle.
4. **Tâches** (§19.1) : `task` lance le calcul sur un fil, et `wait` attend son résultat. Il faut choisir quand écrire la sortie d'une tâche : au plus tôt, ou tamponnée jusqu'au `wait` pour rester déterministe.
5. **Tests.**
   - Les golden actuels doivent rester identiques dans les deux modes.
   - Un test de performance doit vérifier que 27.2 va plus vite avec plusieurs cœurs, par exemple en comparant avec `LION_THREADS=1`, une variable d'environnement à créer.
   - Il faut des tests de déterminisme (sorties et bugs dans des parties parallèles), à lancer plusieurs fois.

Des sujets plus petits peuvent aussi être repris au besoin : traits génériques, méthodes de structures génériques, types et fonctions standard comme valeurs, modules `json` et `dates`, et un nettoyage du cache de `lion build`.

## 10. Conventions de travail

- La spec est la source de vérité. Une ambiguïté se tranche selon les règles de `docs/implementation-notes.md`, puis s'y consigne (Cn suivant : **C83**). Une construction non définie est refusée avec un diagnostic, jamais inventée en silence. L'auteur a délégué toutes les décisions (2026-09-26).
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
