# Proposition : le gestionnaire de paquets (étape 8)

Statut : **implémentée** (C101 dans les [notes](../implementation-notes.md)). L'auteur a choisi les quatre options recommandées le 2026-09-27. Écarts avec le texte ci-dessous : `lion new` n'écrit pas de `.gitignore`, `--git` fait prendre un dépôt local pour un dépôt git, et un paquet de git ne peut pas dépendre d'un dossier local.

La spec prévoit un gestionnaire officiel, dont la conception reste à faire (§20.4, §B.2). Elle fixe deux contraintes :
- chaque projet déclare son édition, et des projets d'éditions différentes s'utilisent mutuellement (§25, D83) ;
- l'étape 8 est terminée quand « on installe une bibliothèque en une commande » (§28).

## 1. Ce que verrait l'utilisateur

```sh
lion new carnet                                   # crée le projet
cd carnet
lion add https://github.com/leo/geometrie         # installe une bibliothèque : une commande
lion run                                          # exécute main.lion, avec ses dépendances
```

```lion
use geometrie                  // le module principal du paquet
use geometrie.cercle           // un autre module du paquet

show(geometrie.aire(3, 4))
```

## 2. Le projet

Un projet est un dossier qui contient un fichier de projet et des modules :

```
carnet/
    lion.toml         le fichier de projet
    lion.lock         les versions exactes des dépendances, écrit par lion
    main.lion         le script du projet
    notes.lion        un module du projet : use notes
```

Proposition de fichier de projet (sous-ensemble de TOML, lu par `lion` sans dépendance) :

```toml
[project]
name = "carnet"
version = "0.1.0"
edition = "0.1"

[dependencies]
geometrie = { git = "https://github.com/leo/geometrie", version = "1.2" }
outils = { path = "../outils" }
```

Une bibliothèque est un projet comme un autre. `use geometrie` charge son module `geometrie.lion`, et `use geometrie.cercle` son module `cercle.lion`.

## 3. Les commandes

| Commande | Rôle |
| --- | --- |
| `lion new nom` | crée le dossier, `lion.toml`, `main.lion` et `.gitignore` |
| `lion add source [nom]` | ajoute une dépendance, la télécharge, met `lion.lock` à jour |
| `lion remove nom` | retire une dépendance |
| `lion update [nom]` | prend les versions les plus récentes permises |
| `lion run`, `build`, `test`, `check` sans fichier | agissent sur le projet du dossier courant ; les dépendances manquantes sont téléchargées d'après `lion.lock` |

## 4. Sources, versions, stockage

- **Sources** : un dépôt git (par la commande `git`) ou un dossier local. Un registre central, avec son index, viendrait plus tard, quand il y aura des paquets à y mettre.
- **Versions** : `majeur.mineur.correctif`. Une version d'un dépôt git est une étiquette `v1.2.0`. `version = "1.2"` accepte de 1.2.0 inclus à 2.0.0 exclu, et `lion add` choisit la plus récente. Deux dépendances qui demandent des versions incompatibles d'un même paquet sont une erreur qui nomme les deux.
- **Stockage** : les paquets téléchargés vont dans le cache de Lion (`~/.cache/lion/packages/nom-commit`), partagé entre projets et jamais modifié. `lion.lock` garde le commit exact et une empreinte du contenu : la même commande donne partout le même programme.
- **Noms** : un paquet ne peut pas porter le nom d'un module de la bibliothèque standard (`csv`, `ui`…), ni celui d'un module du projet.

## 5. Éditions (§25, D83)

- `edition = "0.1"` : les règles de la spec que le paquet suit. Chaque module est vérifié selon l'édition de son paquet. Tous les modules donnent le même IR : des paquets d'éditions différentes s'utilisent donc sans difficulté (D83).
- Une seule édition existe aujourd'hui. Le champ est lu, et une édition inconnue est une erreur qui le dit.

## 6. Questions à l'auteur

1. **Format du fichier de projet** : `lion.toml`, ou un fichier écrit en Lion (des `let` évalués à la compilation, `let edition = "0.1"`) ? Recommandation : `lion.toml`, lisible par tous les outils, dont les éditeurs.
2. **Sources** : git et dossiers locaux d'abord, registre central plus tard ? Recommandation : oui.
3. **Nom des éditions** : le numéro de la spec (`"0.1"`, puis `"1.0"`), ou une année (`"2027"`, comme Rust) ? Recommandation : le numéro de la spec, que le programmeur connaît déjà.
4. **Accès au code** : `use geometrie`, le premier nom d'un `use` étant celui du paquet ? Recommandation : oui, comme pour les modules de la bibliothèque standard.
