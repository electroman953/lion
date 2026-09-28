# Lion pour VS Code

L'extension du langage Lion : coloration, indentation des blocs, et, par `lion lsp`, le serveur de langage de la commande `lion`, les diagnostics, le formatage, le plan des fichiers et la navigation dans les noms.

## Ce qu'elle fait

- **Coloration** des mots-clés, types, textes avec interpolation `{...}` et échappements, nombres, commentaires `//`, `///` et `/* */` (`syntaxes/lion.tmLanguage.json`).
- **Indentation** : une ligne qui finit par `:` ouvre un bloc ; `;`, `elif` et `else` en début de ligne le ferment. Quatre espaces par bloc, comme `lion fmt`.
- **Diagnostics** pendant la frappe, les mêmes que `lion check`, avec leurs notes et leurs suggestions. Un module est vérifié avec le programme qui l'utilise : le `main.lion` du projet, ou le fichier ouvert qui l'atteint. Une erreur dans un module est aussi signalée au `use` qui l'atteint.
- **Formatage** (`Format Document`, ou `editor.formatOnSave`) : celui de `lion fmt`. Un fichier avec une erreur de syntaxe n'est pas formaté.
- **Plan** du fichier (vue « Outline », `Ctrl+Shift+O`) : fonctions, méthodes, structures et leurs champs, énumérations, unions, traits, constantes, variables et tests.
- **Survol** (`Ctrl+K Ctrl+I`, ou la souris) : la déclaration du nom sous le curseur, avec ses types : `let total in Int`, `fun add(a in Int, b in Int) in Int`, `Student.grade in Float`.
- **Aller à la définition** (`F12`) et **références** (`Shift+F12`), dans le fichier et dans les autres fichiers du programme. Le même nom est surligné là où il est écrit.
- **Commandes** « Lion: Run the File » et « Lion: Run the Tests of the File » (bouton ▶ en haut de l'éditeur) : `lion run` ou `lion test` dans un terminal, depuis le dossier du fichier.

Le serveur ne lance aucun code du programme : il ne calcule pas les `compile` (§21.1), dont les bugs n'apparaissent qu'avec `lion check` (C102).

Pendant la frappe, une erreur de syntaxe arrête le vérificateur. Le survol, la définition et les références répondent alors avec les noms du dernier texte qui s'analysait (C103).

## Installer

Il faut la commande `lion` (Rust ≥ 1.88), depuis la racine du dépôt :

```sh
cargo install --path crates/lion_cli      # installe ~/.cargo/bin/lion
```

L'extension cherche `lion` dans le `PATH`, puis dans `~/.cargo/bin`. Le réglage `lion.path` peut donner un autre chemin, par exemple `/chemin/vers/Lion/target/release/lion`.

Puis l'extension elle-même, depuis ce dossier :

```sh
npm install
npm run package                             # produit lion-0.1.0.vsix
code --install-extension lion-0.1.0.vsix
```

## Tests

```sh
npm test         # la coloration, par le moteur TextMate de VS Code
npm run e2e      # l'extension dans un vrai VS Code, avec target/debug/lion
```

`npm run e2e` ouvre une fenêtre de VS Code avec un profil temporaire, puis la ferme. `LION_VSCODE` donne le programme de VS Code (par défaut celui du snap, `/snap/code/current/usr/share/code/code`) et `LION` la commande `lion`.

## Réglages

| Réglage | Rôle |
| --- | --- |
| `lion.path` | la commande `lion` |
| `lion.trace.server` | `messages` ou `verbose` : les messages échangés avec `lion lsp`, dans la sortie « Lion » |
