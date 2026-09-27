# État de l'implémentation de Lion

Mis à jour le 2026-09-27, avec le compilateur natif (étape 5), le parallélisme sur plusieurs cœurs (étape 6), la bibliothèque graphique `ui` (étape 7), les modules `sets`, `json`, `time` et `dates`, et les génériques complets (§15.1). Ce fichier suffit pour reprendre le travail dans une nouvelle session. Il complète trois autres documents :

- [`docs/spec/lion-0.1.md`](docs/spec/lion-0.1.md) : la spécification, **source de vérité** ;
- [`docs/implementation-notes.md`](docs/implementation-notes.md) : chaque décision de l'implémentation (R1–R13, I1–I15, C1–C100) ;
- [`docs/design/ui.md`](docs/design/ui.md) : la conception de la bibliothèque `ui`, validée par l'auteur et implémentée (C98) ;
- [`README.md`](README.md) : la présentation et l'usage.

## 1. Vérification faite pour ce bilan

| Contrôle | Résultat |
| --- | --- |
| `cargo build` | OK |
| `cargo clippy --all-targets` | 0 avertissement |
| `cargo fmt --check` | OK |
| `cargo test` (tout le workspace) | OK : 156 tests unitaires, 200 programmes golden, et les 97 programmes de `tests/runtime`, `tests/integration` et `tests/programs` compilés en natif avec les mêmes sorties |
| Programmes du §27 de la spec | Les trois tournent sans modification, dans les deux modes : 27.1 (CSV, structures), 27.2 (hasard, parallèle, ensembles) et 27.3 (application graphique, `tests/programs/notes_app`, sans écran avec un fichier d'événements) ; Sur 12 cœurs, 27.2 prend 0,67 s interprété (`--release`) et 0,19 s compilé ; avec `LION_THREADS=1`, 3,2 s et 0,65 s. |

L'arbre de travail est propre, sans fichier non commité. Le dépôt est publié en privé sur GitHub : <https://github.com/electroman953/lion> (remote `origin`).

## 2. Où en est la feuille de route (§28 de la spec)

| Étape | État |
| --- | --- |
| 1–2. Frontend complet et mode interprété | **Atteinte** |
| 3. Bibliothèque standard | **Atteinte** pour `files`, `text`, `math`, `random`, `csv`, `json`, `sets`, `time`, `dates` et le noyau. Manque `net` |
| 4. Outillage | **Atteinte** : `lion test`, `lion fmt`, mode interactif |
| 5. Compilateur natif `lion build` | **Atteinte** : les deux modes donnent les mêmes résultats sur tous les programmes de test (C81, C82) |
| 6. Parallélisme et tâches | **Atteinte** : les parties parallèles utilisent tous les cœurs, dans les deux modes, avec le résultat du calcul séquentiel (C83, C84) ; les tâches tournent sur leur propre fil quand rien de ce qu'elles lisent ne peut changer (C85) |
| 7. Bibliothèque `ui` | **Atteinte** : 27.3 tourne. Fenêtres X11 dessinées par Lion (crate `lion_ui`), éléments, mise en page et boucle d'événements écrits en Lion (`std/ui.lion`), backend sans écran pour les tests (C98) |

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
  lion_ui            fenêtres de `ui` : client X11, dessin logiciel, police intégrée, mode sans écran
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
  - `generic_structs.rs` : structures génériques, leurs instances, `Pair of (T, U)` dans les signatures (C91), et la nouvelle vérification quand une instance tardive rejoint un trait (C92).
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
  - `generic_traits.rs` : traits génériques, et `T` trouvé dans les méthodes d'un type (C93) ;
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
- `types.rs` : `Type`, qui est `Copy`, et l'interner global ; `Type::Applied` pour `Pair of (T, U)` dans une signature, et l'origine des instances de structures génériques (C91).
- `visit.rs` : parcours de toutes les expressions.
- `parallel.rs` : `reach`, ce que les tours d'une boucle parallèle peuvent atteindre, soit les modules à initialiser et s'ils doivent rester dans l'ordre (C84) ; `task_reach`, si une tâche peut tourner à part (C85).
- `print.rs` : la sortie de `lion debug ir`.

**`lion_vm`**
- `compile.rs` : IR vers bytecode.
- `bytecode.rs` : les instructions typées.
- `machine.rs` : l'exécution, les pièges (`Trap`), `run`, `run_test`, `run_from` (le mode interactif) et `invoke`. On y trouve aussi les boucles parallèles (`parallel`, `turns`, les machines des fils `Job`), les tâches (`task`, `wait`, `finish_tasks`, `TaskJob`), et la sortie enregistrée puis rejouée dans l'ordre (`Recorder`, `replay`).
- `parallel.rs` : l'ordonnanceur commun aux deux modes (`threads`, `run_chunks`, `turn_count`, `turn_value`, `spawn_task`), avec la réserve de fils des boucles parallèles (`pool`).
- Valeurs :
  - `value.rs` : `Value`, qui est `Arc` et copie à l'écriture (`make_mut`) ; `TaskCell`, l'état d'une tâche ;
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
- Une tâche qui tourne à part devient `rt.task(move |rt| …)`, qui reçoit des copies des variables qu'elle lit.
- Chaque fonction devient `fN(rt, paramètres…, captures…) -> R<T>`. Un paramètre avec valeur par défaut est un `Option` ; `dN` est l'enveloppe pour les appels par valeur et `equals`.
- Tests unitaires dans `tests.rs` (programme Lion en entrée, forme du Rust en sortie).

**`lion_native`** (`lib.rs`) : le runtime des programmes compilés.
- `Program` : la description produite par le code, avec les sources, les structures, les textes et les fonctions dynamiques.
- `Rt` : sortie tamponnée, entrée, nombre d'appels en cours, modules initialisés, layouts, cache des fonctions C.
- `start` : lance le programme sur un fil à grosse pile (4 Gio, sinon 1 Gio, sinon 256 Mio), rend un bug comme `lion run` et sort avec les mêmes codes.
- `Rt::parallel` : exécute les tranches de tours sur des `Rt` de travail, puis écrit leurs sorties et joint leurs valeurs dans l'ordre (`TurnExit`).
- `Rt::task`, `Rt::wait` : les tâches à part (C85).
- Aides : `enter` et `leave` autour de chaque appel, `called_from` pour la trace, et des conversions (`int`, `text`, `field`…).

**`lion_runtime`** : `ops.rs` (arithmétique vérifiée, rationnels, conversions), `bug.rs` (les `BugKind` et leurs messages), `format.rs` (affichage des Float et des rationnels), `json.rs` (lecture stricte du JSON, réutilisable pour un LSP), `stdlib.rs`.

**`lion_std/std/*.lion`** : `csv`, `dates`, `files`, `json`, `math`, `random`, `sets`, `text`, `time`, `ui`.

**`lion_ui`** (C98) :
- `x11.rs` : le protocole X11 parlé directement (connexion, `Xauthority`, fenêtre, `PutImage` par bandes, clavier, événements) ;
- `canvas.rs` : l'image dessinée pixel par pixel ;
- `font.rs`, `font_data.rs` : les polices « misc-fixed » du domaine public, extraites par `tools/extract_font.py` ;
- `headless.rs` : les événements lus dans un fichier, et l'image en PPM ;
- `lib.rs` : le registre des fenêtres, appelé par les natifs de `lion_vm` (donc par les deux modes).

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
  - fonctions anonymes, y compris génériques : `let twice = fun(x) = x * 2` (C78) ;
  - fonctions standard et fonctions des modules comme valeurs : `each(l, show)`, `apply(text.upper, t)` (C94).
- **Collections** :
  - List, Set, Range, n-uplets ;
  - Map (`Map of (K, V)`, API minimale C79) ;
  - Domain (`{x in Int, x > 0}`, C74) ;
  - compréhensions, avec les bornes reconnues pour un générateur sur `Int` ;
  - `union`, `inter`, `minus`, `subset`.
- **Types** :
  - unions, `maybe`, affinage ;
  - structures, avec invariants, méthodes, `var self` et vérification des constantes à la compilation ;
  - structures génériques (`struct Pair of (A, B)`, C75), leurs méthodes (C90), écrites avec des variables de type ou venues d'un autre module (C91) ;
  - énumérations ordonnées ou non ;
  - traits avec méthodes par défaut ;
  - variables de type (`T in Comparable`) ;
  - traits génériques (`trait Container of T`, C93) ;
  - `equals` (C76).
- **Erreurs** : `Error` est un trait. Aussi : `error(...)`, `try`, `e.message()`, les erreurs propres au programme, et des bugs avec trace d'appels.
- **Modules** : `use`, `private`, globales initialisées au premier usage, cycles entre modules.
- **Concurrence** :
  - `task` et `wait` ;
  - `parallel [...]`, `parallel {...}`, `parallel for` ;
  - `shared`, `shared synced`, `same` (C73) ;
  - règles de sûreté du §19.3 et D48.

  Les tours des parties parallèles s'exécutent sur plusieurs fils (C83). Ceux qui touchent un `synced` ou lisent le clavier restent dans l'ordre (C84). Une tâche tourne sur son propre fil quand rien de ce qu'elle lit ne peut changer, et sa sortie apparaît à son `wait` (C85).
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
| Types comme valeurs (`let t = Int`) | §7.1 |
| Lire un élément de n-uplet : la spec ne dit pas comment (C53) | §16 |
| Module `net` de la bibliothèque standard | §23 |
| `ui` sur Windows et macOS, touches mortes, défilement, styles, images | C98 |
| Heures d'une journée, fuseaux horaires, ajout de mois dans `dates` | C89 |
| Écriture littérale d'une Map, que la spec laisse ouverte (§29) | C79 |

## 6. Tests

```sh
cargo test --workspace                     # tout : tests unitaires + golden
cargo test --test golden                   # seulement les programmes golden
LION_BLESS=1 cargo test --test golden      # régénère les .expected après un changement voulu, puis relire le diff
```

Chaque test golden est un fichier `tests/<suite>/*.lion` accompagné de son `.expected`, qui contient le code de sortie, stdout et stderr. Le lanceur est `crates/lion_cli/tests/golden.rs`. Tous les programmes tournent avec `LION_UI=headless` ; un fichier `.events` à côté d'un programme lui donne les événements de son interface (C98).

| Dossier | Fichiers | Commande exercée |
| --- | --- | --- |
| `tests/lexer` | 4 | `lion debug tokens` |
| `tests/parser` | 23 | `lion debug ast` |
| `tests/typechecker` | 15 | `lion debug ir` |
| `tests/errors` | 57 | `lion check` (erreurs de compilation) |
| `tests/runtime` | 81 | `lion run` (sémantique, bugs, alertes) |
| `tests/integration` | 13 | `lion run` (programmes complets) |
| `tests/programs` | 3 | `lion run` depuis leur dossier (programmes 27.1, 27.2 et 27.3 de la spec ; un programme peut avoir son dossier, `notes_app/notes_app.lion`) |
| `tests/testing` | 3 | `lion test` |
| `tests/interactive` | 1 | `lion` seul, le fichier en entrée |

Le mode compilé a ses propres tests (`cargo test --test native`, `crates/lion_cli/tests/native.rs`) :
- `compiled_programs_behave_as_interpreted` : les programmes de `tests/runtime`, `tests/integration` et `tests/programs` sont traduits par `lion debug rust` et compilés ensemble, comme les modules d'un seul exécutable, dans `target/tmp/native-suite`. Chacun est ensuite comparé à son `.expected`, alertes retirées. Les avertissements affichés par `lion debug rust` précèdent la sortie, comme avec `lion run`.
- `lion_build_makes_an_executable` : lance `lion build` avec un cache dans `target/tmp/lion-build`.

Ces tests demandent cargo, qu'ils trouvent dans la variable `CARGO` posée par `cargo test`. Un programme ajouté à `tests/runtime` est donc testé dans les deux modes.

Des tests unitaires existent aussi dans les crates suivantes : `lion_syntax` (49), `lion_sema` (42), `lion_runtime` (20), `lion_vm` (15), `lion_codegen` (12), `lion_native` (5), `lion_diagnostics` (4), `lion_ir` (3), `lion_ui` (5) et `lion_cli` (1).

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
  - une tâche qui lit une globale `var`, une cellule ou un `synced`, ou qui est créée dans une partie parallèle ou une autre tâche, est calculée à sa création (C71, C85) ;
  - un bug dans une tâche à part n'arrête le programme qu'à son `wait`, ou à la fin du script (C85) ;
  - les tours qui touchent un `shared synced` ou lisent le clavier s'exécutent dans l'ordre, sans gain de vitesse (C84) ;
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
- Interface `ui` (C98) :
  - X11 seulement (Linux, BSD, Wayland par XWayland), en couleurs vraies sur 24 bits ;
  - pas de touches mortes, de défilement, de styles ni d'images ;
  - une fenêtre X11 n'a pas pu être vérifiée à l'œil pendant la session du 2026-09-27 : KWin masquait toutes les fenêtres X (même `xlogo`). Le protocole est accepté par le serveur, et le rendu est vérifié par les images du mode sans écran.
- Mode compilé :
  - `lion build` demande une chaîne Rust sur la machine qui compile ;
  - la première compilation prépare le runtime dans le cache (`LION_CACHE`, `$XDG_CACHE_HOME/lion` ou `~/.cache/lion`), ce qui prend quelques secondes ;
  - le cache garde un paquet par exécutable produit, supprimé après 30 jours sans compilation (C99) ;
  - deux `lion build` simultanés vers le même exécutable se gênent ;
  - `lion test` et le mode interactif restent interprétés.

## 9. Prochaine étape recommandée

Les étapes 2 à 7 de la feuille de route sont atteintes. Reste :
- **l'étape 8**, le gestionnaire de paquets, avec le fichier de projet et les éditions (§25). Sa conception est à proposer à l'auteur avant de coder, comme pour `ui` ;
- **vérifier `ui` à l'écran** avec l'auteur (`lion run tests/programs/notes_app/notes_app.lion` depuis ce dossier), puis l'étendre : Windows et macOS, touches mortes, défilement, styles.

Questions encore ouvertes pour l'auteur : ce qu'on peut faire d'un type comme valeur (`let t = Int`, §7.1), la lecture des éléments d'un n-uplet (C53), l'écriture de `json`.

Ce qui peut se faire sans nouvelle règle de langage :
1. **Bibliothèque standard (§23, étape 3)** : `net` vient après l'étape 3 ; l'écriture de `json` reste à faire (C100). `dates` pourra recevoir les heures et les fuseaux horaires (C89).
2. **Génériques (§15.1)** : complets, avec les méthodes de List, Set et Map (C90–C93, C97).
3. **Parallélisme** :
   - un verrou plus fin pour `shared synced` (C84).
4. **Outils** : le débogueur pas à pas du §24.2 (D25).
5. **VS Code et LSP**, à préparer dans l'architecture :
   - sortir le pipeline de `lion_cli/src/driver.rs` dans une bibliothèque qui rend les diagnostics au lieu de les afficher, avec un fournisseur de fichiers pour les tampons non sauvegardés ;
   - convertir les `Span` (octets) en positions LSP (ligne, colonne UTF-16) dans `lion_diagnostics` ;
   - faire produire par `lion_sema` un index (définitions, références, type de chaque nom) même pour un programme avec des erreurs, puisque l'IR n'existe que pour un programme valide ; tenir compte des versions des fonctions génériques (C1) ;
   - dans un processus long : `catch_unwind` autour de chaque analyse, un cache des modules standard vérifiés, et l'interner de types global, qui ne se vide jamais ;
   - JSON-RPC : l'analyseur JSON de `lion_runtime/src/json.rs` existe déjà (C96), il reste l'écriture ;
   - une sous-commande `lion lsp` (outil unique, D30), et une extension dans `editors/vscode` : grammaire TextMate, indentation (`:` ouvre, `;`, `elif` et `else` ferment), formatage par `lion fmt` ; plus tard, un adaptateur de débogage (DAP).

## 10. Conventions de travail

- La spec est la source de vérité. Une ambiguïté se tranche selon les règles de `docs/implementation-notes.md`, puis s'y consigne (Cn suivant : **C101**). Une construction non définie est refusée avec un diagnostic, jamais inventée en silence. L'auteur a délégué toutes les décisions (2026-09-26). Le 2026-09-27, il a précisé qu'on ne modifie pas la sémantique de Lion sans lui demander : les choix d'API et d'implémentation restent délégués et consignés, mais une règle nouvelle ou changée du langage se propose d'abord.
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
