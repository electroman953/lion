# Guide de Lion

Ce guide apprend Lion pas à pas, sans supposer que vous connaissez déjà un autre langage. Chaque exemple est un programme complet : copiez-le dans un fichier `essai.lion` et lancez `lion run essai.lion`.

Pour le détail exact d'une règle, la [référence du langage](reference.html) fait foi. Pour les fonctions des modules (`text`, `net`, `json`…), voyez la [bibliothèque standard](modules.html).

## Installer Lion

Lion s'installe avec Rust (version 1.88 ou plus récente). Si vous n'avez pas Rust, installez-le d'abord depuis <https://rustup.rs>.

```sh
cargo install --git https://github.com/electroman953/lion lion_cli
lion --version
```

La commande `lion` arrive dans `~/.cargo/bin`. Si le terminal ne la trouve pas, ajoutez ce dossier à votre `PATH`.

Les commandes à connaître tout de suite :

| Commande | Ce qu'elle fait |
| --- | --- |
| `lion run f.lion` | vérifie le programme, puis l'exécute |
| `lion check f.lion` | vérifie seulement (types, noms, blocs) ; c'est instantané |
| `lion` | le mode interactif : on tape du Lion ligne par ligne |
| `lion fmt f.lion` | remet le fichier en forme (4 espaces par bloc) |
| `lion test f.lion` | lance les tests du fichier |
| `lion build f.lion -o f` | compile en un exécutable natif, bien plus rapide |
| `lion new nom` | crée un projet (`lion.toml` et `main.lion`) |

**Dans VS Code**, l'extension Lion du dépôt (`editors/vscode`) colore le code, signale les erreurs pendant la frappe, formate le fichier et permet d'aller à la définition d'un nom (`F12`).

## Premier programme

```lion
show("Bonjour !")
let nom = ask("Comment t'appelles-tu ? ")
show("Enchanté, {nom}.")
```

Trois choses à remarquer :

- **Pas de fonction `main`** : le code écrit au niveau du fichier s'exécute de haut en bas.
- `show` affiche une valeur, `ask` pose une question et renvoie la réponse sous forme de texte.
- `{nom}` dans un texte y insère la valeur de `nom`. On peut y mettre n'importe quelle expression : `"{2 * 21}"` donne `42`.

Les **commentaires** commencent par `//` et vont jusqu'au bout de la ligne.

## Lignes et blocs

Une instruction tient sur une ligne. Un **bloc** s'ouvre par `:` en fin de ligne et se ferme par `;`. L'indentation (4 espaces) est obligatoire pour la lisibilité, mais c'est le `;` qui ferme le bloc.

```lion
let x = 7
if x > 0:
    show("positif")
elif x == 0:
    show("nul")
else:
    show("négatif")
;
```

Un seul `;` ferme tout le `if … elif … else`. Un bloc court peut tenir sur une ligne :

```lion
for i in 1..3: show(i) ;
```

Une longue expression peut continuer sur la ligne suivante seulement entre parenthèses, crochets ou accolades :

```lion
let total = (1 + 2 + 3
    + 4 + 5)
show(total)
```

## Variables et constantes

```lion
let pi = 3.14159        // constante : on ne peut plus la changer
var score = 0           // variable
score = 10
score += 5              // aussi -= et *=
show(score)             // 15
```

- `let` déclare une **constante**. Une constante est immuable « en profondeur » : on ne peut changer ni sa valeur, ni un élément d'une liste qu'elle contient.
- `var` déclare une **variable**.
- On peut préciser le type avec `in` : `var moyenne = 0 in Float` donne `0.0`.

Les noms s'écrivent en ASCII (`a-z`, `A-Z`, `0-9`, `_`). Les **types commencent par une majuscule** (`Int`, `Student`), les variables et fonctions par une minuscule.

Certains mots sont réservés et ne peuvent pas servir de nom : `test`, `of`, `in`, `as`, `div`, `mod`, `over`, `task`, `wait`, `match`, `then`, `otherwise`, `maybe`, `use`, `private`, `self`, `union`, `inter`, `minus`, `subset`… Un nom comme `test` pour une fonction est donc refusé.

## Les types de base

| Type | Exemples | Remarques |
| --- | --- | --- |
| `Int` | `42`, `-7`, `1_000_000` | entier sur 64 bits ; un dépassement arrête le programme |
| `Float` | `3.5`, `1e-3` | nombre à virgule |
| `Rational` | `1 over 3` | fraction exacte, toujours simplifiée |
| `Bool` | `true`, `false` | |
| `Text` | `"bonjour"` | texte en UTF-8 |
| `None` | `none` | « pas de valeur » |

Les opérations sur les nombres :

```lion
show(7 / 2)        // 3.5 : la division / donne toujours un Float
show(7 div 2)      // 3   : quotient entier
show(7 mod 2)      // 1   : reste
show(2 ^ 10)       // 1024
show(1 over 3 + 1 over 6)   // 1/2, en valeur exacte
```

`div` et `mod` sont **euclidiens** : le reste est toujours positif, `-7 mod 3` vaut `2`.

Les comparaisons se **chaînent** comme en maths :

```lion
let note = 14
if 0 <= note <= 20: show("note valide") ;
```

Les opérateurs logiques s'écrivent en toutes lettres : `and`, `or`, `not`.

**Conversions** avec `as` :

```lion
show(3.7 as Int)          // 3
show(12 as Text)          // "12"
let lu = "12" as Int      // Int or Error : le texte peut ne pas être un nombre
```

Le dernier cas renvoie `Int or Error`, car `"abc" as Int` ne peut pas réussir. On verra plus bas comment traiter ce genre de résultat.

## Les textes

```lion
let prenom = "Léa"
let n = 3
show("{prenom} a {n} messages")
show("{prenom}{prenom}")          // pour coller deux textes
```

**Il n'y a pas de `+` entre deux textes** : on colle des textes par interpolation (`"{a}{b}"`) ou avec `text.join`.

Échappements : `\n` (retour à la ligne), `\t`, `\\`, `\"`, et `\{` `\}` pour une accolade littérale.

Un texte sur plusieurs lignes s'écrit entre `"""` : l'indentation commune est retirée.

```lion
let nom = "Léa"
let page = """
    <h1>Bonjour</h1>
    <p class="note">{nom}</p>
    """
show(page)
```

Les textes s'indexent **à partir de 1** : `t[1]` est le premier caractère, `t.size` le nombre de caractères. Le module `text` contient le reste :

```lion
use text

let phrase = "  le lion dort  "
show(text.trim(phrase))                  // "le lion dort"
show(text.upper("lion"))                 // "LION"
show(text.split("a,b,c", ","))           // ["a", "b", "c"]
show(text.join(["a", "b", "c"], " - "))  // "a - b - c"
show(text.contains(phrase, "lion"))      // true
show(text.fixed(12.5, 2))                // "12.50"
show(text.pad_left("7", 3, "0"))         // "007"
```

## Conditions et boucles

```lion
// if comme expression : il donne une valeur
let age = 17
let statut = if age >= 18 then "majeur" else "mineur"
show(statut)

// for sur un intervalle (les deux bornes sont incluses)
for i in 1..5:
    show(i * i)
;

// while
var n = 27
var etapes = 0
while n != 1:
    n = if n mod 2 == 0 then n div 2 else 3 * n + 1
    etapes += 1
;
show("{etapes} étapes")
```

`break` sort d'une boucle, `continue` passe au tour suivant.

**`match`** compare une valeur à plusieurs cas, dans l'ordre :

```lion
let n = 4
let mot = match n:
    0 then "aucun"
    in 1..9 then "quelques-uns"
    otherwise then "beaucoup"
;
show(mot)
```

## Fonctions

```lion
fun carre(x in Int) in Int = x * x          // forme courte

fun saluer(nom in Text, poli = true) in Text:  // forme en bloc
    if poli: return "Bonjour {nom}" ;
    return "Salut {nom}"
;

show(carre(5))
show(saluer("Léa"))
show(saluer("Tom", poli: false))            // argument nommé
```

- Un paramètre s'écrit `nom in Type`. Le type de retour vient après `in`.
- Un paramètre avec une **valeur par défaut** (`poli = true`) peut être omis. Les paramètres par défaut viennent en dernier.
- Les **arguments nommés** (`poli: false`) se placent après les autres.
- Un paramètre sans type rend la fonction **générique** : elle s'adapte au type des arguments.

### Les valeurs sont copiées

En Lion, passer une liste à une fonction la **copie** (la copie n'est faite que si c'est nécessaire, donc c'est rapide). Une fonction ne peut donc pas modifier par accident la liste de l'appelant. Pour la modifier volontairement, le paramètre est déclaré `var` :

```lion
fun ajouter_un(var l in List of Int):
    l.add(1)
;

var nombres = [5, 6]
ajouter_un(nombres)
show(nombres)        // [5, 6, 1]
```

### Les variables globales

Une fonction peut **lire** une globale librement. Pour la **modifier**, elle doit l'annoncer avec `modifies` :

```lion
var compteur = 0

fun compter() modifies compteur:
    compteur += 1
;

compter()
compter()
show(compteur)      // 2
```

### Les fonctions sont des valeurs

```lion
let double = fun(x in Int) = x * 2
show(double(21))

fun appliquer(f in fun(Int) in Int, x in Int) in Int = f(x)
show(appliquer(double, 5))
```

## Listes, ensembles et dictionnaires

### Listes

```lion
var notes = [12, 15, 9]
notes.add(18)                       // sur sa propre ligne
show(notes[1])                      // 12 : on compte à partir de 1
show(notes.size)                    // 4
show(notes.last)                    // 18
show(notes[2..3])                   // [15, 9] : extrait, bornes incluses
show(sum(notes))                    // 54
show(15 in notes)                   // true

for n in notes: show(n) ;
```

Une liste vide a besoin de son type : `var noms = [] in List of Text`.

Lire `notes[10]` dans une liste de 4 éléments est un **bug** : le programme s'arrête avec un message qui indique la ligne et les valeurs en cause.

### Compréhensions

Une compréhension construit une liste comme on écrit un ensemble en maths :

```lion
let pairs = [x, x in 1..20, x mod 2 == 0]
let carres = [x * x, x in 1..5]
let couples = [(a, b), a in 1..3, b in 1..3, a < b]
show(pairs)
show(carres)
show(couples)
```

### Ensembles

Un `Set` ne garde chaque valeur qu'une fois :

```lion
let a = {1, 2, 3}
let b = {3, 4}
show(a union b)       // {1, 2, 3, 4}
show(a inter b)       // {3}
show(a minus b)       // {1, 2}
show({1, 2} subset a) // true
let lettres = {c, c in ["a", "b", "a"]}
show(lettres.size)    // 2
```

### Dictionnaires (Map)

```lion
var age = {"Léa" -> 17, "Tom" -> 18}
age["Max"] = 16
age["Tom"] += 1
show("Léa" in age)            // true
show(age.get("Zoé"))          // none : get donne une valeur « peut-être absente »
for nom in age: show("{nom} : {age[nom]}") ;
```

Un dictionnaire vide : `var comptes = {} in Map of (Text, Int)`.

### N-uplets

```lion
let point = (3, "A")
let (x, nom) = point
show("{nom} en {x}")
```

### Trier

```lion
use sets
use text

show(sets.sorted([3, 1, 2]))                 // [1, 2, 3]
show(text.sorted(["poire", "abricot"]))      // ordre du dictionnaire
```

Pour trier selon un critère : `sets.sorted_by(l, fun(p in Personne) = p.age)`.

## Structures et méthodes

Une **structure** regroupe des valeurs nommées. Elle peut porter des **conditions** (des invariants) qui sont vérifiées à chaque construction et à chaque modification.

```lion
struct Eleve:
    nom in Text
    note in Float = 0, 0 <= note <= 20     // valeur par défaut, puis condition
;

fun Eleve.admis() in Bool = self.note >= 10

fun Eleve.bonus(var self, points in Float):
    self.note += points
;

var lea = Eleve("Léa", 14)
lea.bonus(2)
show(lea.note)        // 16.0
show(lea.admis())     // true

let tom = Eleve(nom: "Tom", note: 8)
show(tom.admis())     // false
```

- Les champs se donnent dans l'ordre, ou par leur nom. Les derniers champs qui ont une valeur par défaut peuvent être omis.
- Une **méthode** s'écrit `fun Type.nom(...)`. Dans son corps, `self` est l'objet.
- Une méthode qui **modifie** l'objet prend `var self` en premier paramètre.
- Si les valeurs données sont connues à la compilation (comme `Eleve("Léa", 14)`), la condition est vérifiée avant même l'exécution. Sinon, la construction renvoie `Eleve or Error`.
- Mettre une note de 25 par une modification qui viole la condition est un **bug** : le programme s'arrête.

Pour qu'un type s'affiche joliment avec `show` ou `"{x}"`, donnez-lui une méthode `text` :

```lion
struct Point:
    x in Int
    y in Int
;
fun Point.text() in Text = "({self.x}, {self.y})"
show(Point(1, 2))     // (1, 2)
```

## Énumérations et unions

```lion
Couleur = {rouge, vert, bleu}            // énumération
Jour = [lundi, mardi, mercredi]          // énumération ordonnée : lundi < mardi

fun nom_fr(c in Couleur) in Text = match c:
    rouge then "rouge"
    vert then "vert"
    bleu then "bleu"
;

show(nom_fr(Couleur.vert))
show(Jour.lundi < Jour.mercredi)         // true
```

Seule, une valeur s'écrit avec son type (`Jour.lundi`) ; là où Lion attend déjà une `Couleur`, `rouge` suffit.

Le `match` sur une énumération doit couvrir **tous** les cas : un oubli est une erreur de compilation.

Une **union** dit qu'une valeur est d'un type ou d'un autre :

```lion
struct Cercle:
    r in Float
;
struct Rectangle:
    l in Float
    h in Float
;
Forme = Cercle or Rectangle

fun aire(f in Forme) in Float = match f:
    in Cercle c then 3.14159 * c.r ^ 2
    in Rectangle r then r.l * r.h
;

show(aire(Rectangle(2, 3)))
```

## Valeurs absentes et erreurs

C'est le point le plus important de Lion. Ce qui peut échouer **se voit dans le type**, et le compilateur oblige à le traiter.

### `maybe` : une valeur qui peut manquer

`maybe Int` veut dire `Int or None`. Avant d'utiliser la valeur, on teste qu'elle est là :

```lion
use text

let position = text.find("bonjour", "j")    // maybe Int
if position != none:
    show(position + 1)                      // ici, position est un Int
;
let age = {"Léa" -> 17}.get("Tom") otherwise 0    // valeur par défaut
show(age)
```

Après le test `position != none`, Lion sait que `position` est un `Int` : c'est l'**affinage**.

### `Error` : un échec prévu

Une fonction qui peut échouer renvoie `T or Error` :

```lion
fun inverse(x in Float) in Float or Error:
    if x == 0: return error("pas d'inverse pour 0") ;
    return 1 / x
;

match inverse(0):
    in Error e: show("échec : {e.message()}") ;
    in Float v: show(v) ;
;
```

### `try` : faire remonter l'erreur

Dans une fonction qui renvoie elle-même `… or Error`, `try` simplifie tout : si l'expression est une erreur, la fonction s'arrête et renvoie cette erreur ; sinon `try` donne la valeur.

```lion
use files
use text

fun lire_nombre(chemin in Text) in Int or Error:
    let contenu = try files.read(chemin)       // une erreur ressort aussitôt
    return try text.trim(contenu) as Int
;

match lire_nombre("n.txt"):
    in Error e: show("impossible : {e.message()}") ;
    in Int n: show("lu : {n}") ;
;
```

Au niveau du script (hors fonction), `try` arrête le programme avec le message de l'erreur. C'est pratique pour un petit script.

**Attention à la portée de `try`** : il s'applique à **toute** l'expression à sa droite. `try a.get("x").int()` applique `try` au `.int()` final. Pour enchaîner, écrivez `(try a.get("x")).int()`, ou découpez en plusieurs `let`.

### Bug ou erreur ?

| | Bug | Erreur (`Error`) |
| --- | --- | --- |
| Exemples | indice hors limites, dépassement d'`Int`, division entière par 0, invariant violé | fichier absent, texte qui n'est pas un nombre, site injoignable |
| Ce qui se passe | le programme s'arrête, avec la ligne, les appels en cours et les valeurs | une valeur `Error` que le programme doit traiter |
| Peut-on l'intercepter ? | non : c'est une faute du programme, il faut la corriger | oui, avec `match`, `try` ou `otherwise` |

## Modules et projets

Un fichier peut en utiliser un autre avec `use`. Les noms restent préfixés par le module :

```lion
use text
use math
show(text.upper("lion"))
show(math.sqrt(2.0))
```

Vos propres modules marchent pareil : `use geometrie` charge `geometrie.lion` à côté du programme, `use formes.cercle` charge `formes/cercle.lion`. Une déclaration marquée `private` n'est pas visible depuis les autres fichiers.

### Un projet

```sh
lion new mon-projet
cd mon-projet
lion run            # lance main.lion
```

`lion new` crée `lion.toml` (le nom, la version et les dépendances du projet), `main.lion` et un `.gitignore`. Dans un projet, `lion run`, `lion check`, `lion build` et `lion test` sans nom de fichier visent `main.lion`.

Pour utiliser une bibliothèque écrite en Lion :

```sh
lion add https://github.com/quelquun/une-lib    # ou un dossier local
lion update
lion remove une-lib
```

## Tests

Les tests s'écrivent dans le fichier, à côté du code :

```lion
fun carre(x in Int) in Int = x * x

test "carré":
    expect carre(3) == 9
    expect carre(-2) == 4
;
```

`lion test f.lion` lance les tests d'un fichier, `lion test dossier` ceux de tous les fichiers. En cas d'échec, Lion affiche par exemple `expected 9, got 6`. Le code au niveau du fichier ne s'exécute pas pendant les tests (sauf les déclarations de globales).

## Fichiers, web et scripts

### Fichiers

```lion
use files

try files.write("note.txt", "bonjour\n")
try files.append("note.txt", "encore\n")
let contenu = try files.read("note.txt")
show(contenu)
for f in try files.find(".", "*.lion"): show(f) ;
```

### JSON

```lion
use json

let v = try json.decode("\{\"nom\": \"Léa\", \"notes\": [12, 15]\}")
let nom = try (try v.get("nom")) as Text
let premiere = try (try v.path("notes[1]")).int()
show("{nom} : {premiere}")

// Construire du JSON
let corps = json.object().with("x", json.from(3)).with("y", json.from(7))
show(corps.encode())        // {"x":3,"y":7}
```

Les accolades `{` `}` du JSON s'échappent en `\{` `\}` dans un texte Lion, car elles servent à l'interpolation. Le plus simple reste de lire le JSON depuis un fichier (`json.load(chemin)`) ou de le construire avec `json.object()`.

Une valeur JSON peut aussi se convertir directement en structure Lion : `try v as Eleve`, et `json.from(eleve)` fait l'inverse.

### Requêtes web

Le module `net` passe par le programme `curl`, qui doit être installé.

```lion
use net

let page = try net.get("https://example.com")
show(page.status)                       // 200
show(page.header("content-type"))

let r = try net.post(
    "https://httpbin.org/post",
    "\{\"a\": 1\}",
    [net.Header("Content-Type", "application/json")])
show(r.ok())
```

`net.session()` garde les cookies d'une requête à l'autre, comme un navigateur, pour rester connecté à un site.

### Un petit serveur web

```lion
use net

fun bonjour(r in net.Request) in net.Response = net.plain("Bonjour {r.param("nom")}\n")

let app = net.router().get("/bonjour/:nom", bonjour).files("/", "public")
try net.serve(8080, app.handle)
```

`http://localhost:8080/bonjour/Léa` répond `Bonjour Léa`, et les autres adresses servent les fichiers du dossier `public`.

### Arguments et autres programmes

```lion
use system

for mot in system.args(): show(mot) ;     // lion run f.lion a b c
let git = try system.run("git", ["status", "--short"])
show(git.output)
```

## Tâches et parallélisme

Une **tâche** calcule en arrière-plan ; `wait` attend son résultat :

```lion
fun lent(n in Int) in Int = sum([i * i, i in 1..n])

let t = task lent(1_000_000)
show("pendant ce temps…")
show(wait t)
```

Pour répartir un gros calcul sur tous les cœurs :

```lion
fun est_premier(n in Int) in Bool:
    if n < 2: return false ;
    for d in 2..isqrt(n):
        if n mod d == 0: return false ;
    ;
    return true
;

let premiers = parallel [n, n in 1..200_000, est_premier(n)]
show(premiers.size)
```

Ce qui tourne en parallèle ne peut rien modifier à l'extérieur : le compilateur le vérifie. Le résultat est toujours le même qu'en séquentiel, dans le même ordre. Réservez `parallel` aux calculs lourds, car le partage du travail a un coût.

## Aller plus vite

- **`lion build f.lion -o f`** produit un exécutable natif, 3 à 13 fois plus rapide que `lion run`, avec exactement les mêmes résultats. Il faut Rust installé.
- Pour construire un long texte, remplissez une liste puis appelez `text.join(liste, "")`, plutôt que de recoller le texte à chaque tour de boucle.
- `compile expr` calcule une valeur pendant la compilation : `let premiers = compile {p in 2..10_000, est_premier(p)}`.

## Les pièges fréquents

| Vous écrivez | Il faut écrire | Pourquoi |
| --- | --- | --- |
| `"a" + "b"` | `"{a}{b}"` | pas de `+` sur les textes |
| `l[0]` | `l[1]` | on compte à partir de 1 |
| `try v.get("a").int()` | `(try v.get("a")).int()` | `try` couvre toute l'expression à sa droite |
| `var l = []` | `var l = [] in List of Int` | une collection vide a besoin de son type |
| `fun test()` | un autre nom | `test`, `of`, `in`… sont réservés |
| `x ?? defaut` | `x otherwise defaut` | |
| modifier une globale dans une fonction | ajouter `modifies nom` | |
| `\d{4}` dans une regex | `"\\d\{4\}"` | `{` sert à l'interpolation |
| oublier un `;` | fermer chaque bloc | Lion s'aide de l'indentation pour montrer où il manque |

Quand `lion check` refuse un programme, lisez la ligne `help:` : elle propose presque toujours la correction.

## Si vous venez de Python

| Python | Lion |
| --- | --- |
| `x = 5` | `let x = 5` (constante) ou `var x = 5` |
| `def f(x):` + indentation | `fun f(x in Int) in Int:` … `;` |
| `f"{x}"` | `"{x}"` |
| `l[0]`, `l[-1]` | `l[1]`, `l.last` |
| `range(1, 11)` | `1..10` |
| `[x*x for x in l if x > 0]` | `[x * x, x in l, x > 0]` |
| `{"a": 1}` | `{"a" -> 1}` |
| `None` | `none` |
| `try: … except:` | `T or Error`, `try`, `match` |
| `class A:` | `struct A:` + méthodes `fun A.m()` |
| `import json` | `use json` |
| `if __name__ == "__main__":` | rien : le code du fichier s'exécute |
| `assert` dans pytest | `test "…": expect … ;` |
