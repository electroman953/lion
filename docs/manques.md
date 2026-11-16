# Ce qui manque à Lion

Les assistants qui écrivent du Lion notent ici, par l'outil `lion_report_gap` de `lion mcp` (C108), ce que le langage ou sa bibliothèque ne leur a pas permis de faire simplement. Chaque entrée est à trier : l'ajouter à Lion (et la rayer ici avec le numéro de la décision), ou expliquer pourquoi non.

Les premières entrées ont été constatées par Claude en écrivant les modules web, le 2026-11-16 ; celles qui touchent au langage demandent une décision de l'auteur.

## 2026-11-16 — Trier des Text, et trier selon une clé

- **Besoin** : `sets.sorted(["b", "a"])` est refusé, car Text n'a pas `<` (R3) ; et rien ne trie une liste de structures selon un champ.
- **Contournement** : aucun simple : écrire un tri à la main avec une fonction de comparaison.
- **Exemple souhaité** :

```lion
let noms = sets.sorted(["Max", "Léa"])
let par_age = sets.sorted_by(personnes, fun(p) = p.age)
```

## 2026-11-16 — Arrondir ou écrire un Float avec n décimales

- **Besoin** : afficher un prix `12.50` ou une moyenne à deux décimales ; `round` ne prend qu'un argument et `x as Text` écrit tous les chiffres.
- **Contournement** : `round(x * 100) / 100`, qui n'écrit pas les zéros finaux.
- **Exemple souhaité** :

```lion
show(text.fixed(prix, 2))     // "12.50"
show(round(moyenne, 2))
```

## 2026-11-16 — Compléter ou répéter un texte

- **Besoin** : aligner des colonnes, écrire `007` ; il n'y a ni `pad` ni `repeat`.
- **Contournement** : une boucle qui ajoute des espaces.
- **Exemple souhaité** :

```lion
show(text.pad_left("7", 3, "0"))    // "007"
show(text.repeat("-", 20))
```

## 2026-11-16 — Lire un élément d'un n-uplet

- **Besoin** : `let t = (1, "a")` se construit et se compare, mais `t[1]` est refusé (C53 : la spec ne dit pas comment).
- **Contournement** : une structure à la place du n-uplet.

## 2026-11-16 — Texte sur plusieurs lignes

- **Besoin** : écrire une page HTML, une requête SQL ou un JSON dans le code ; un texte tient sur une ligne.
- **Contournement** : `\n` dans une seule ligne, ou un fichier lu par `files.read`.

## 2026-11-16 — Valeur par défaut d'un `maybe`

- **Besoin** : `r.query("page")` rend `maybe Text` ; il faut un `if` ou un `match` pour donner une valeur par défaut.
- **Exemple souhaité** : `let page = r.query("page") otherwise "1"` (à concevoir : `??` n'existe pas).

## 2026-11-16 — Des octets

- **Besoin** : lire une image en mémoire, calculer sur des données binaires, recevoir un corps qui n'est pas du texte. Un `Text` est toujours de l'UTF-8.
- **Contournement** : passer par des fichiers (`net.download`, `net.file`, `r.upload(...).path`, `hash.file_sha256`).

## 2026-11-16 — Ignorer une valeur

- **Besoin** : `let _ = r.respond(a)` est refusé : `_` n'est pas un nom.
- **Contournement** : `let ignored = …`.

## 2026-11-16 — Map écrite avec ses valeurs

- **Besoin** : `{"a": 1}` n'existe pas (C79) ; une Map se remplit clé par clé.

## 2026-11-16 — Sauter un argument par défaut

- **Besoin** : `f(a, workers: 4)` est refusé quand un autre paramètre par défaut précède `workers` (§11.2) ; il faut écrire tous ceux d'avant.

## 2026-11-16 — `show` d'une `json.Value`

- **Besoin** : `show(v)` affiche `Value(source: "…")`, la structure interne, au lieu du JSON. Il faudrait une façon pour un type de dire comment il s'affiche.
- **Contournement** : `show(v.encode())` ou `v.pretty()`.

## 2026-11-16 — Effets d'une fonction passée en paramètre à une tâche

- **Besoin** : `net.serve_parallel(port, f)` avec un `f` qui modifie une globale n'est pas refusé : le vérificateur ne suit pas une fonction passée en paramètre jusqu'à la tâche. Les réponses restent justes (elles passent l'une après l'autre), mais le programmeur ne le sait pas.
