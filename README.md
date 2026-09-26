# Lion

Lion est un langage polyvalent, interprété ou compilé, dont l'écriture et la logique sont proches des mathématiques. Sa [spécification](docs/spec/lion-0.1.md) est la source de vérité de ce dépôt ; les choix d'implémentation sont consignés dans [docs/implementation-notes.md](docs/implementation-notes.md).

## État

L'implémentation est au début de l'étape 2 de la feuille de route (§28). Elle construit le langage par tranches verticales qui fonctionnent réellement de bout en bout. Ce qui n'est pas encore implémenté est refusé avec le message `not implemented yet`, suivi de la section de la spec concernée.

**Ce qui fonctionne aujourd'hui**

- **Programmes-scripts** (§20.1) : `let`, `var`, déclaration sans valeur (`let e in Text`), annotation (`in Float`), redéclaration dans le même bloc, `=`, `+=`, `-=`, `*=`.
- **Types** : `Int` (64 bits, débordement = bug), `Float` (IEEE 754), `Bool`, `Text`, `None`, avec conversion automatique Int → Float.
- **Opérateurs** :
  - `+ - * /` ;
  - `div` et `mod` euclidiens ;
  - `^` ;
  - comparaisons enchaînées (`0 <= x <= 20`) ;
  - `and`, `or` (court-circuit), `not` ;
  - conversions `as` entre nombres et vers Text.
- **Textes** : échappements et interpolation `"x = {x}"`. La fonction `show`.
- **Vérifications à la compilation** : types, noms inconnus (avec suggestions), constantes réaffectées, lecture d'une variable sans valeur.
- **Bugs à l'exécution** (§18) : débordement, division entière par zéro, exposant négatif, conversion Float → Int impossible. Chacun est signalé avec l'emplacement, les valeurs en cause et une suggestion.
- **Alertes du mode interprété** (§22.3) : infini, NaN, perte de précision.

**Pas encore implémenté** : blocs `if`, boucles, fonctions, collections, structures, énumérations, unions, `match`, erreurs-valeurs et `try`, modules, concurrence, compilateur natif, formateur, tests intégrés, mode interactif.

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
  lion_diagnostics   positions, erreurs, bugs, alertes et leur rendu
  lion_cli           la commande `lion`
```
