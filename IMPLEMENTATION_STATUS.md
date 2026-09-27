# État de l'implémentation de Lion

Mis à jour le 2026-09-27, avec le compilateur natif (étape 5) et les parties parallèles sur plusieurs cœurs (étape 6). Ce fichier suffit pour reprendre le travail dans une nouvelle session. Il complète trois autres documents :

- [`docs/spec/lion-0.1.md`](docs/spec/lion-0.1.md) : la spécification, **source de vérité** ;
- [`docs/implementation-notes.md`](docs/implementation-notes.md) : chaque décision de l'implémentation (R1–R13, I1–I15, C1–C84) ;
- [`README.md`](README.md) : la présentation et l'usage.

## 1. Vérification faite pour ce bilan

| Contrôle | Résultat |
| --- | --- |
| `cargo build` | OK |
| `cargo clippy --all-targets` | 0 avertissement |
| `cargo fmt --check` | OK |
| `cargo test` (tout le workspace) | OK : 141 tests unitaires, 174 programmes golden, et les 80 programmes de `tests/runtime`, `tests/integration` et `tests/programs` compilés en natif avec les mêmes sorties |
| Programmes du §27 de la spec | 27.1 (CSV, structures) et 27.2 (hasard, parallèle, ensembles) tournent sans modification, dans les deux modes ; Sur 12 cœurs, 27.2 prend 0,67 s interprété (`--release`) et 0,19 s compilé ; avec `LION_THREADS=1`, 3,2 s et 0,65 s. 27.3 dépend du module `ui`, imaginaire |

L'arbre de travail est propre, sans fichier non commité.

## 2. Où en est la feuille de route (§28 de la spec)

| Étape | État |
| --- | --- |
| 1–2. Frontend complet et mode interprété | **Atteinte** |
| 3. Bibliothèque standard | **Atteinte** pour `files`, `text`, `math`, `random`, `csv` et le noyau. Manquent `sets`, `json`, `dates`, `time`, `net` |
| 4. Outillage | **Atteinte** : `lion test`, `lion fmt`, mode interactif |
| 5. Compilateur natif `lion build` | **Atteinte** : les deux modes donnent les mêmes résultats sur tous les programmes de test (C81, C82) |
| 6. Parallélisme et tâches | **Atteinte** pour le critère du §28 : les parties parallèles utilisent tous les cœurs, dans les deux modes, avec le résultat du calcul séquentiel (C83, C84). Les tâches sont encore calculées à leur création (C71) : voir §9 |
| 7. Bibliothèque `ui` | Pas commencée |

## 3. Architecture

Workspace Cargo en Rust (édition 2024, Rust ≥ 1.88), **sans aucune dépendance externe**.

```
source .lion
  → lion_syntax   lexer → tokens → parser → AST ; formateur (lion fmt)
  → lion_sema     noms, types, flux, règles de sûreté → IR typé
  → lion_ir       IR typé : le contrat commun aux backends (+ visiteur, analyse des
                  boucles parallèles)
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
- `parallel.rs` : `reach`, ce que les tours d'une boucle parallèle peuvent atteindre, soit les modules à initialiser et s'ils doivent rester dans l'ordre (C84).
- `print.rs` : la sortie de `lion debug ir`.

**`lion_vm`**
- `compile.rs` : IR vers bytecode.
- `bytecode.rs` : les instructions typées.
- `machine.rs` : l'exécution, les pièges (`Trap`), `run`, `run_test`, `run_from` (le mode interactif) et `invoke`. On y trouve aussi les boucles parallèles : `parallel`, `turns`, les machines des fils (`Job`), et la sortie enregistrée puis rejouée dans l'ordre (`Recorder`).
- `parallel.rs` : l'ordonnanceur commun aux deux modes (`threads`, `run_chunks`, `turn_count`, `turn_value`).
- Valeurs :
  - `value.rs` : `Value`, qui est `Arc` et copie à l'écriture (`make_mut`) ;
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
- Une boucle parallèle devient `rt.parallel(tours, …, &|rt, first, last, gathered| …)`. La closure redéclare les variables que les tours écrivent, et reçoit les pointeurs `var` enveloppés dans `Shared`.
- Chaque fonction devient `fN(rt, paramètres…, captures…) -> R<T>`. Un paramètre avec valeur par défaut est un `Option` ; `dN` est l'enveloppe pour les appels par valeur et `equals`.
- Tests unitaires dans `tests.rs` (programme Lion en entrée, forme du Rust en sortie).

**`lion_native`** (`lib.rs`) : le runtime des programmes compilés.
- `Program` : la description produite par le code, avec les sources, les structures, les textes et les fonctions dynamiques.
- `Rt` : sortie tamponnée, entrée, nombre d'appels en cours, modules initialisés, layouts, cache des fonctions C.
- `start` : lance le programme sur un fil à grosse pile (4 Gio, sinon 1 Gio, sinon 256 Mio), rend un bug comme `lion run` et sort avec les mêmes codes.
- `Rt::parallel` : exécute les tranches de tours sur des `Rt` de travail, puis écrit leurs sorties et joint leurs valeurs dans l'ordre (`TurnExit`).
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

  Les tours des parties parallèles s'exécutent sur plusieurs fils (C83). Ceux qui touchent un `synced` ou lisent le clavier restent dans l'ordre (C84). Les tâches sont calculées à leur création (C71).
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
| Tâches concurrentes : `task` calcule sa valeur à sa création (C71) | §19.1, étape 6 |
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
| `tests/runtime` | 65 | `lion run` (sémantique, bugs, alertes) |
| `tests/integration` | 13 | `lion run` (programmes complets) |
| `tests/programs` | 2 | `lion run` depuis leur dossier (programmes 27.1 et 27.2 de la spec) |
| `tests/testing` | 3 | `lion test` |
| `tests/interactive` | 1 | `lion` seul, le fichier en entrée |

Le mode compilé a ses propres tests (`cargo test --test native`, `crates/lion_cli/tests/native.rs`) :
- `compiled_programs_behave_as_interpreted` : les programmes de `tests/runtime`, `tests/integration` et `tests/programs` sont traduits par `lion debug rust` et compilés ensemble, comme les modules d'un seul exécutable, dans `target/tmp/native-suite`. Chacun est ensuite comparé à son `.expected`, alertes retirées. Les avertissements affichés par `lion debug rust` précèdent la sortie, comme avec `lion run`.
- `lion_build_makes_an_executable` : lance `lion build` avec un cache dans `target/tmp/lion-build`.

Ces tests demandent cargo, qu'ils trouvent dans la variable `CARGO` posée par `cargo test`. Un programme ajouté à `tests/runtime` est donc testé dans les deux modes.

Des tests unitaires existent aussi dans les crates suivantes : `lion_syntax` (49), `lion_sema` (42), `lion_runtime` (18), `lion_vm` (11), `lion_codegen` (10), `lion_native` (5), `lion_diagnostics` (4) et `lion_ir` (2).

Les tests golden tournent avec autant de fils que de cœurs ; `LION_THREADS=1` les fait tourner sur un seul, avec la même sortie.

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

- Parallélisme :
  - les tâches sont calculées à leur création (C71) ;
  - les tours qui touchent un `shared synced` ou lisent le clavier s'exécutent dans l'ordre, sans gain de vitesse (C84) ;
  - les fils sont créés pour chaque boucle parallèle, sans réserve de fils : une boucle parallèle dans une boucle très répétée paie ce coût à chaque tour ;
  - une tranche commencée va jusqu'au bout, même si un tour d'une tranche précédente a quitté la boucle (C83).
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

## 9. Prochaine étape recommandée : les tâches concurrentes (fin de l'étape 6)

Le critère de l'étape 6 est atteint : 27.2 utilise tous les cœurs. Il reste `task`, qui calcule encore sa valeur dès sa création (C71). Plan retenu, à décider en C85 :

1. **Quand une tâche peut tourner à part.** Une tâche ne s'exécute sur son propre fil que si elle ne dépend de rien que le reste du programme peut changer pendant qu'elle tourne. Les variables locales qu'elle lit sont copiées à sa création : elles ne gênent pas. Il faut aussi qu'elle ne puisse, même à travers les fonctions qu'elle appelle :
   - ni lire une globale `var` ou une cellule partagée avec une closure ;
   - ni utiliser un `synced`, lire le clavier ou noter un `expect` ;
   - ni contenir un `try` qui quitterait la fonction qui la crée.

   Sinon, elle reste calculée à sa création, comme aujourd'hui. Cette condition s'ajoute à l'analyse `lion_ir::parallel::reach`.
2. **Sortie et bugs.** Ce qu'écrit une tâche qui tourne à part est gardé, puis écrit au premier `wait` qui l'attend. Un bug, un `exit` ou l'échec d'un `try` au niveau du script apparaît aussi à ce `wait`, avec la trace des appels en cours à la création. À la fin du script, les tâches jamais attendues sont attendues dans l'ordre de leur création. Après un `exit` ou un bug du programme, elles sont abandonnées.
3. **Nombre de fils.** Au plus `threads()` tâches tournent à la fois. Au-delà, une nouvelle tâche est calculée à sa création, mais sa sortie est gardée de la même façon.
4. **Représentation.**
   - `Value::Task` devient un état partagé : le résultat, quand il est connu, et sinon le moyen de l'attendre.
   - Machine virtuelle : il faut des fils qui vivent plus longtemps qu'une instruction, par exemple avec un `std::thread::scope` autour de toute l'exécution. Les poignées des tâches sont rangées dans la machine, et la `Value` n'en porte que le numéro.
   - Code compilé : une closure `move` qui copie les variables lues, sur un `std::thread::spawn` avec un `Rt` de travail.
5. **Tests.** Il faut des golden qui fixent la place de la sortie des tâches et de leurs bugs, identiques dans les deux modes et avec `LION_THREADS=1`.

Ensuite viendra l'étape 7, la bibliothèque `ui`, dont la conception reste à faire (§B.2). Des sujets plus petits peuvent aussi être repris au besoin :
- traits génériques, méthodes de structures génériques ;
- types et fonctions standard comme valeurs ;
- modules `json` et `dates` ;
- une réserve de fils pour les boucles parallèles ;
- un nettoyage du cache de `lion build`.

## 10. Conventions de travail

- La spec est la source de vérité. Une ambiguïté se tranche selon les règles de `docs/implementation-notes.md`, puis s'y consigne (Cn suivant : **C85**). Une construction non définie est refusée avec un diagnostic, jamais inventée en silence. L'auteur a délégué toutes les décisions (2026-09-26).
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
