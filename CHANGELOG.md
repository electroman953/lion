# Historique des modifications

Les décisions détaillées sont dans [docs/implementation-notes.md](docs/implementation-notes.md) (Rn, In, Cn). Ce qui reste à faire est dans [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) (§5, §8 et §9). Chaque ligne correspond à un commit du dépôt.

## 2026-09-28, cinquième session : les noms dans l'éditeur

**Ajouté**
- L'index des noms du vérificateur (C103) : ce que chaque nom désigne, où il est déclaré et où il
  est écrit. Il est construit pendant la vérification, donc il existe aussi pour un programme avec
  des erreurs, là où l'IR n'existe que pour un programme valide.
- `lion lsp` répond au survol (la déclaration, avec les types), à l'aller à la définition, aux
  références et au surlignage du même nom. Pendant la frappe, une erreur de syntaxe arrête le
  vérificateur : le serveur répond alors avec les noms du dernier texte qui s'analysait.
- L'extension VS Code vérifie ces trois réponses dans un vrai VS Code (`npm run e2e`).

## 2026-09-27, quatrième session : VS Code et le serveur de langage

**Ajouté**
- `lion lsp`, le serveur de langage des éditeurs (C102) : diagnostics pendant la frappe, formatage par `lion fmt`, plan des fichiers. Un module est vérifié avec le programme qui l'utilise, et le serveur ne lance jamais le code du programme.
- L'extension VS Code (`editors/vscode`) : coloration, indentation des blocs, client de `lion lsp`, commandes « Run » et « Test ». Testée par le moteur de coloration de VS Code (`npm test`) et dans un vrai VS Code (`npm run e2e`).
- `lion_runtime::json` écrit aussi le JSON, et donne les positions du protocole (UTF-16) dans `lion_diagnostics`.

**Modifié**
- Le driver lit les modules par une fonction qu'on lui donne (le disque, ou les textes de l'éditeur), et dit quel `use` a atteint chaque fichier (`driver::analyze`).

## 2026-09-27, troisième session : étapes 7 et 8

**Ajouté**
- La bibliothèque graphique `ui` : le programme 27.3 tourne (étape 7, C98).
  - Les fenêtres sont dessinées par Lion et montrées par le protocole X11, sans dépendance : crate `lion_ui`, police « misc-fixed » du domaine public intégrée.
  - Les éléments, la mise en page et la boucle d'événements sont écrits en Lion (`std/ui.lion`).
  - Un mode sans écran (`LION_UI=headless`) sert aux tests.
  - Vérifié à l'écran sous KDE Plasma (Wayland, par XWayland).
- Les projets et les paquets (étape 8, C101) : `lion new`, `lion add`, `lion remove`, `lion update`, `lion.toml`, `lion.lock`, sources git et dossiers locaux, versions, éditions. `lion run`, `check`, `build` et `test` s'utilisent sans fichier dans un projet.
- Bibliothèque standard :
  - `sets` (C86) ;
  - `time` (C88) ;
  - `dates` (C89) ;
  - `json` en lignes, comme `csv` (C96) ;
  - l'écriture des CSV (C100).
- Génériques :
  - méthodes des structures génériques (C90) ;
  - `Pair of (T, U)` dans les signatures et types génériques d'un autre module (C91) ;
  - traits génériques (C93) ;
  - méthodes de List, Set et Map (C97).
- Les fonctions du noyau et des modules comme valeurs : `each(l, show)`, `apply(text.upper, t)` (C94).
- `s.remove(x)` sur un Set (C95).
- `lion build` nettoie son cache : ce qui n'a servi à aucune compilation depuis 30 jours est supprimé (C99).
- Une réserve de fils pour les boucles parallèles : 2 000 petites boucles passent de 0,41 s à 0,06 s.
- Les propositions de conception [`ui`](docs/design/ui.md) et [paquets](docs/design/packages.md), validées par l'auteur.

**Modifié**
- **À confirmer par l'auteur** : une méthode déclarée dans le fichier de sa structure ou de son énumération est vue partout où vont ses valeurs, y compris dans les fonctions génériques d'un autre module. La visibilité par import du §20.3 ne vaut plus que pour les méthodes ajoutées aux types du langage (C87, qui précise C61).
- Une erreur dans une fonction générique de la bibliothèque standard est montrée à l'appel du programme (C86).
- Une instance de structure générique faite tard, qui rejoint un trait, provoque une nouvelle vérification du programme (C92).
- Le driver trouve lui-même le fichier de chaque `use` et identifie les modules par leur fichier (C101).
- Les lanceurs de tests exécutent chaque programme avec `LION_UI=headless`. Un programme de `tests/programs` peut avoir son propre dossier (`notes_app/notes_app.lion`).

## 2026-09-27, deuxième session : étapes 5 et 6

- Le mode compilé, `lion build` : l'IR est traduit en Rust et compilé par LLVM, avec la même sémantique que `lion run` (C81, C82).
- Les parties parallèles tournent sur tous les cœurs (C83, C84), et les tâches sur leur propre fil (C85).
- Les valeurs sont partagées entre fils (`Arc`), et les opérations sur les valeurs sont communes aux deux modes.

## 2026-09-26 et 2026-09-27, première session : étapes 1 à 4

- La spécification 0.1 et le frontend complet : lexer, parser, vérificateur, IR typé.
- La machine virtuelle et le mode interprété, avec les alertes du §22.3.
- Le langage :
  - structures, énumérations, unions, `match`, erreurs et `try` ;
  - fonctions génériques, closures, traits ;
  - rationnels, Domains, Maps, `shared` ;
  - `compile`, appels C.
- La bibliothèque standard en Lion : `files`, `text`, `math`, `random`, `csv`. Les programmes 27.1 et 27.2 tournent.
- Les outils : `lion test`, `lion fmt`, le mode interactif.
