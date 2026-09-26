# Lion — Spécification du langage (v0.1)

Sep 26, 2026 · @Roquefort

## 1. Statut et conventions

Ce document décrit Lion 0.1. En phase 0.x, toute règle peut encore changer (§25).

Chaque règle porte un statut. Une règle sans marque est **validée**.

| Marque | Sens |
| --- | --- |
| (aucune) | Validé : décidé explicitement par l'auteur du langage |
| \[Dn\] | Choix délégué : tranché par Claude selon la boussole (§2), annulable. Liste complète en annexe A |
| (provisoire) | Idée retenue mais pas encore confirmée |

Vocabulaire utilisé partout :

- **Erreur de compilation** : le programme est refusé avant de s'exécuter, dans les deux modes.
- **Bug** : arrêt immédiat à l'exécution, avec un message clair (§18). Il ne peut pas être intercepté.
- **Échec attendu** : une valeur de type `Error`, que le programme doit traiter (§18).
- **Alerte** : message non bloquant, émis seulement en mode interprété (§22).

Consigne aux implémenteurs, humains ou IA : une zone floue se signale, elle ne se comble jamais au hasard.

## 2. Vision, principes et boussole

Lion est un langage polyvalent, interprété ou compilé, dont l'écriture et la logique sont proches des mathématiques.

**Cas d'usage cibles**

- Des programmes d'analyse et de logique, écrits vite et clairement.
- Des applications solides, avec interface graphique, rapides et efficaces.

**Principes, par ordre de priorité en cas de conflit**

1. **L'utilité avant l'exploration.** Quand pureté mathématique et praticité s'opposent, la praticité gagne.
2. **Polyvalent.** Lion s'écrit comme des maths, mais ce n'est pas un langage *pour* faire des maths.
3. **Rapide.** Nettement plus rapide que Python ; cible : Java/C# ou mieux.
4. **Fiable.** Ce qui peut échouer est vérifié à la compilation chaque fois que c'est possible.
5. **Idée centrale : les ensembles.** Les types sont des ensembles, `x in Float` se lit « x ∈ Float », et les ensembles définis par compréhension sont des valeurs de premier plan.
6. **Deux modes, une sémantique.** Un programme se comporte exactement pareil interprété ou compilé ; l'interprété ajoute seulement des alertes.

**Boussole pour les détails**

- Il se lit comme des maths.
- Il est strict, mais pas bavard.
- Il est explicite sur ce qui change : `var`, `var self`, `modifies`, `shared`.
- Il est rapide par construction.
- Il préfère les mots aux symboles, sans aller jusqu'aux phrases.

**Ce que Lion garde de la programmation orientée objet** : données et comportements regroupés, la notation `objet.méthode()`, le polymorphisme, et « tout est objet » au sens où toute valeur a des méthodes (`x.abs()`). Un nombre reste une valeur simple en mémoire. Il n'y a pas d'héritage.

## 3. Aperçu rapide

Ce court programme montre l'essentiel de Lion : un type avec invariant, une méthode, une compréhension, un `match` et une erreur traitée.

```lion
struct Student:
    name in Text
    grade in Float, 0 <= grade <= 20        // invariant
;

fun Student.passes() in Bool:
    return self.grade >= 10
;

let class = [Student("Léa", 14), Student("Tom", 8)]
let admitted = {s.name, s in class, s.passes()}
show("Admis : {admitted}")

match ask("Une note ?") as Float:
    in Error e: show("Pas un nombre : {e.message()}") ;
    in Float g, 0 <= g <= 20: show("Note valide") ;
    otherwise: show("Hors barème") ;
;
```

Le fichier s'exécute directement avec `lion run notes.lion`, sans fonction `main`.

## 4. Lexique

La syntaxe de Lion est entièrement ASCII ; l'Unicode n'apparaît que dans les textes et les commentaires.

### 4.1 Fichiers et caractères

- Un fichier source est en UTF-8 et porte l'extension `.lion` \[D11\].
- Identifiants, mots-clés et opérateurs sont en ASCII. Un identifiant commence par une lettre, suivie de lettres, chiffres ou `_` \[D57\].
- Les mots-clés sont en anglais.

### 4.2 La casse a un sens

| Première lettre | Désigne | Exemples |
| --- | --- | --- |
| Majuscule | un type : struct, énumération, union, trait, variable de type | `Student`, `Color`, `T` |
| Minuscule | une valeur : variable, constante, fonction, champ, module, valeur d'énumération | `grade`, `red`, `load` |

Pour les acronymes, seule la première lettre compte : on écrit `HttpClient` \[D4\]. Les constantes n'ont pas de casse spéciale (provisoire).

### 4.3 Commentaires

- `//` jusqu'à la fin de la ligne.
- `/* ... */` sur plusieurs lignes, non imbriqués \[D5, D58\].
- `///` pour la documentation d'une déclaration \[D5\].

### 4.4 Mots-clés réservés

`let` `var` `fun` `return` `if` `elif` `else` `then` `match` `otherwise` `for` `while` `break` `continue` `in` `as` `and` `or` `not` `same` `union` `inter` `minus` `subset` `div` `mod` `over` `true` `false` `none` `maybe` `of` `struct` `trait` `use` `private` `shared` `synced` `task` `wait` `parallel` `try` `compile` `modifies` `infix` `foreign` `pure` `unsafe` `test` `expect` `self`

`show`, `ask`, `error`, `exit`, `reverse` et `sum` sont des fonctions de la bibliothèque standard, pas des mots-clés.

### 4.5 Littéraux

| Sorte | Exemples | Notes |
| --- | --- | --- |
| Entier | `42`, `1_000_000`, `0xFF`, `0b1010` | `_` sépare les chiffres \[D43\] ; hexadécimal et binaire \[D59\] |
| Flottant | `3.14`, `2.5e-3` | un chiffre de chaque côté du point \[D59\] |
| Texte | `"Bonjour {name}"` | `{expr}` insère une valeur \[D24\] ; échappements `\n` `\t` `\\` `\"` `\{` `\}` \[D60\] |
| Booléen | `true`, `false` | `Bool = {true, false}` \[D45\] |
| Vide | `none` | unique valeur du type `None` |
| Liste | `[1, 2, 3]` | ordonnée (§16) |
| Ensemble | `{1, 2, 3}` | sans ordre, sans doublon (§16) |
| Intervalle | `1..9` | entiers, bornes incluses (§16) |
| Couple | `("Léa", 12)`, `(x,)` | un seul élément : virgule finale \[D56\] |

## 5. Structure du code : lignes et blocs

Une ligne contient une seule instruction ; `:` ouvre un bloc et `;` le ferme, comme des parenthèses.

### 5.1 Instructions

- La fin de ligne termine l'instruction. Il n'y a pas de séparateur pour en mettre deux sur une ligne.
- Une expression peut continuer sur la ligne suivante tant qu'une `(`, un `[` ou une `{` reste ouvert \[D52\].
- Hors de ces cas, une ligne qui commence par un opérateur est une erreur de compilation :

```lion
total = a + b
      + c              // erreur : « + c » n'est pas une instruction

total = (a + b
         + c)          // correct
```

### 5.2 Blocs

- `:` ouvre un bloc, `;` le ferme.
- `elif` et `else` ferment la branche précédente ; seul le `if` entier se ferme par `;`.
- Un bloc d'une seule instruction peut tenir sur une ligne : `if x > 0: show(x) ;`.

```lion
if x > 0:
    show("positif")
elif x == 0:
    show("nul")
else:
    show("négatif")
;
```

### 5.3 Indentation

- L'indentation est décorative : elle ne change jamais le sens du programme.
- Le compilateur s'en sert pour localiser les erreurs, par exemple « `;` probablement oublié ligne 3 ».
- Le formateur officiel impose une indentation de 4 espaces \[D20\].

### 5.4 Guide de style

Les `;` s'empilent quand on imbrique des fonctions anonymes longues. C'est accepté ; le guide de style recommande de nommer une fonction plutôt que d'imbriquer un long bloc anonyme.

## 6. Variables, constantes et portée

`let` crée une constante immuable en profondeur, `var` une variable modifiable ; l'un des deux est toujours obligatoire.

### 6.1 Déclarer

```lion
let a = 5                 // type déduit : Int
let b = 5 in Float        // annotation : b vaut 5.0
let c = 5 as Float        // synonyme de la ligne précédente
var d in Float            // pas encore de valeur
let e in Text             // constante affectée plus tard, une seule fois
```

- Lire une variable avant qu'elle ait reçu une valeur est une erreur de compilation, sur tous les chemins possibles.
- Un `let` sans valeur reçoit exactement une affectation ; une seconde est une erreur de compilation.
- Aucun avertissement n'est émis pour une variable jamais lue.

### 6.2 `in` dans une déclaration

Après le `=`, un `in` suivi d'un nom **en majuscule** annote le type. Suivi d'un nom en minuscule, c'est un test d'appartenance.

```lion
let x = 3 in Int                      // annotation
let ok = 3 in primes                  // Bool : 3 ∈ primes ?
let s = ("Léa", 12) in Student        // construction (§12)
let valid = (("Léa", 12) in Student)  // parenthèses : c'est un test, donc un Bool
```

### 6.3 Affecter

- `=` n'accepte qu'une valeur du même type. Seule exception : un Int est converti automatiquement en Float (§8).
- `+=`, `-=` et `*=` existent \[D44\].
- Pour changer de type, on redéclare dans le **même bloc** : la nouvelle liaison masque l'ancienne.

```lion
var count = 0
count = "zéro"             // erreur : Text n'est pas Int
let count = "zéro"         // correct : nouvelle liaison, l'ancienne est masquée
```

### 6.4 Immuabilité profonde

Une valeur tenue par un `let` ne change jamais, ni elle ni son contenu.

```lion
let notes = [12, 8, 15]
notes.add(10)              // erreur de compilation : add modifie la liste
```

### 6.5 Portée et masquage

- Masquer une variable dans un bloc **intérieur** est une erreur de compilation.
- Exception : les paramètres et variables locales d'une fonction définie au niveau global peuvent reprendre le nom d'une globale.

```lion
let n = 10
fun double(n) = n * 2      // correct : fonction globale

fun f():
    let i = 0
    for i in 1..3: show(i) ;   // erreur : i masque la locale i
;
```

- Toute fonction peut lire les globales, y compris celles déclarées plus bas dans le fichier.
- Modifier une globale exige `modifies` (§11).

## 7. Types et typage

Lion est typé statiquement avec inférence : chaque valeur a un type connu à la compilation, sans qu'on ait besoin de l'écrire.

### 7.1 Les types sont des ensembles

`x in T` teste si la valeur `x` appartient au type `T` et renvoie un `Bool`. Le mot-clé `is` n'existe pas.

```lion
if v in Float: show("un flottant") ;
if ("Léa", 25) in Student: ... ;       // faux : l'invariant de Student est violé (§12)
```

### 7.2 Types de base

| Type | Contenu | Section |
| --- | --- | --- |
| `Int` | entier sur 64 bits | §8 |
| `Float` | flottant IEEE 754 sur 64 bits | §8 |
| `Rational` | fraction exacte, optionnelle | §8 |
| `Bool` | `{true, false}` | §13 |
| `Text` | texte Unicode | §16 |
| `None` | la seule valeur `none` | §7.4 |
| `List of T`, `Set of T`, `Domain of T`, `Range`, `Map of (K, V)` | collections | §16 |
| `(Text, Int)` | couple ou n-uplet | §16 |
| `fun(Int, Text) in Bool` | fonction \[D63\] | §11 |
| `Task of T` | tâche en cours \[D68\] | §19 |
| `Type` | l'ensemble de tous les types | §15 |

### 7.3 Unions

- `A or B` désigne une valeur de type A ou de type B. Exemples : `Int or Error`, `Circle or Rect`.
- `maybe T` est un raccourci de `T or None` \[D13\]. `maybe maybe T` s'aplatit en `maybe T`.
- `of` lie plus fort que `or` \[D34\] : `List of Student or Error` signifie `(List of Student) or Error`.
- `maybe List of Int` est une liste peut-être absente ; `List of (maybe Int)` est une liste d'entiers peut-être absents.
- Un n-uplet en argument de `of` prend des doubles parenthèses : `List of ((Text, Int))` \[D67\].

### 7.4 Utiliser une union : l'affinage

Une valeur de type union ne peut pas être utilisée comme l'un de ses membres avant d'avoir été testée. Après le test, le compilateur connaît le type exact.

```lion
let s = find_student("Léa")       // type : maybe Student
show(s.grade)                     // erreur de compilation : s peut valoir none
if s in None: return ;            // ou : if s == none: return ;
show(s.grade)                     // correct : ici, s est un Student
```

L'affinage fonctionne avec `if`, `match` (§10) et après une sortie anticipée (`return`, `break`, `continue`).

## 8. Nombres et conversions

Les entiers sont fixes et rapides, mais un débordement arrête toujours le programme ; `/` donne toujours un Float.

### 8.1 Int

- Entier signé sur 64 bits. `Int` n'est donc pas ℤ.
- Un débordement est un bug, dans les deux modes. Le compilateur supprime la vérification quand il prouve qu'elle est inutile.
- `div` et `mod` suivent la division euclidienne : le reste est toujours positif ou nul \[D62\]. Diviser par zéro est un bug.

```lion
7 / 2        // 3.5 (Float)
7 div 2      // 3
-7 div 3     // -3
-7 mod 3     // 2
```

### 8.2 Float

- Flottant IEEE 754 sur 64 bits. `Float` n'est donc pas ℝ : `0.1 + 0.2 == 0.3` vaut `false`.
- `1.0 / 0.0` donne l'infini et `0.0 / 0.0` donne `NaN`, comme le standard. Le mode interprété émet une alerte quand une telle valeur apparaît.
- `NaN == NaN` vaut `false`. Insérer `NaN` dans un `Set`, même caché dans une structure, est un bug.

### 8.3 Rational

- Fraction exacte, créée avec `over` : `1 over 3`. Elle est toujours simplifiée.
- `over` n'accepte que des Int (sinon : erreur de compilation). `x over 0` est un bug.
- `over` a la même priorité que `*` : `1 over 3 + 1` vaut 4/3.
- `Rational + Int` donne un Rational ; `Rational + Float` donne un Float, sans alerte.
- Numérateur et dénominateur sont des Int : leur débordement est un bug.

### 8.4 Puissance

`x ^ n` \[D73\]. Il est associatif à droite et lie plus fort que le moins unaire : `-2 ^ 2` vaut -4. `Int ^ Int` donne un Int ; un exposant négatif est un bug. Dès qu'un Float intervient, le résultat est un Float.

### 8.5 Conversions

| Conversion | Comment | Résultat |
| --- | --- | --- |
| Int → Float | automatique partout : affectation, argument, calcul mixte, collection | alerte en mode interprété si la valeur dépasse 2^53 et perd de la précision |
| Float → Int | `3.7 as Int` | `3` : troncature vers zéro. NaN, infini ou valeur hors limites : bug \[D74\] |
| Rational → Float | `r as Float`, ou automatique dans un calcul mixte | Float |
| Texte → nombre | `"12" as Int` | `Int or Error` : l'erreur dit pourquoi \[D14\] |
| Nombre → texte | `12 as Text` | `"12"` |

`floor`, `ceil` et `round` sont dans la bibliothèque standard pour les autres arrondis.

## 9. Opérateurs et expressions

Les opérateurs logiques et ensemblistes s'écrivent en mots ; tout s'évalue de gauche à droite.

### 9.1 Table des priorités \[D61\]

Du plus fort au plus faible.

| Niveau | Opérateurs | Associativité |
| --- | --- | --- |
| 1 | `f(x)`, `l[i]`, `a.b`, `a.m()` | gauche |
| 2 | `^` | droite |
| 3 | `-x` (moins unaire) | préfixe |
| 4 | `*`, `/`, `div`, `mod`, `over`, opérateurs `infix` de l'utilisateur | gauche |
| 5 | `+`, `-` | gauche |
| 6 | `..` | aucune |
| 7 | `inter` | gauche |
| 8 | `union`, `minus` | gauche |
| 9 | `as` | gauche |
| 10 | `==`, `!=`, `<`, `>`, `<=`, `>=` (enchaînables) ; `in`, `subset`, `same` (non enchaînables) | voir 9.3 |
| 11 | `not` | préfixe |
| 12 | `and` | gauche |
| 13 | `or` | gauche |
| 14 | `try`, `task`, `wait`, `compile`, `shared`, `synced`, `parallel`, `if…then…else`, `match…then` | couvrent toute l'expression à leur droite \[D49\] |

Exemples de lecture :

```lion
1..n - 1                  // 1..(n - 1)
p + 2 in primes           // (p + 2) in primes
x + 1 as Text             // (x + 1) as Text
not x in S                // not (x in S)
try r.get("grade") as Float   // try (r.get("grade") as Float)
```

### 9.2 Ordre d'évaluation \[D27\]

- Les opérandes et les arguments s'évaluent de gauche à droite.
- `and` et `or` s'arrêtent dès que le résultat est connu.

### 9.3 Comparaisons enchaînées

`0 <= grade <= 20` équivaut à `0 <= grade and grade <= 20`, avec `grade` évalué une seule fois \[D69\]. Seuls `==`, `!=`, `<`, `>`, `<=` et `>=` s'enchaînent ; `in`, `subset` et `same` demandent des parenthèses pour être combinés. `!=` est provisoire.

### 9.4 Égalité et identité

- `==` compare le **contenu**. Pour une structure, c'est champ par champ par défaut, redéfinissable (§12).
- `same` compare l'**identité** et n'existe que pour les valeurs `shared` (§17). Ailleurs, c'est une erreur de compilation.

### 9.5 Surcharge des opérateurs

Un type rend un opérateur disponible en définissant la méthode au nom réservé correspondant \[D2\]. Les mots-opérateurs (`div`, `mod`, `over`, `union`, `inter`, `minus`, `subset`, `in`, `as`, `same`, `and`, `or`, `not`) ne se redéfinissent pas.

| Opérateur | Méthode |
| --- | --- |
| `+` | `plus(other)` |
| `-` binaire | `subtract(other)` |
| `-` unaire | `negate()` \[D75\] |
| `*` | `times(other)` |
| `/` | `divide(other)` |
| `^` | `power(other)` \[D75\] |
| `==`, `!=` | `equals(other)` |
| `<`, `>`, `<=`, `>=` | `less(other)` : les trois autres en sont déduits |

Un utilisateur peut aussi créer ses propres opérateurs en mots, en les déclarant `infix` \[D3\] :

```lion
infix fun Vector.dot(other in Vector) in Float:
    return self.x * other.x + self.y * other.y
;
let p = u dot v
```

## 10. Contrôle de flux

Lion a `if`, deux boucles et `match`, une définition par cas vérifiée par le compilateur.

### 10.1 `if`

La forme en bloc est décrite au §5.2. La forme courte, sur une ligne, est une **expression** :

```lion
let sign = if x > 0 then 1 elif x == 0 then 0 else -1
let bonus = if late then 0          // pas de else : type maybe Int, vaut none si late est faux
```

### 10.2 Boucles \[D28\]

```lion
for s in students: show(s.name) ;
while n > 1:
    n = n div 2
;
```

- `break` quitte la boucle, `continue` passe au tour suivant.
- Une liste se parcourt dans l'ordre, un `Range` en ordre croissant, un `Set` dans un ordre non garanti (§16).
- La version parallèle, `parallel for`, est décrite au §19.

### 10.3 `match`

Forme instruction : chaque cas a son bloc `:` … `;`, et le `match` se ferme par `;`.

```lion
match load_grade("notes.txt"):
    in Error e: show("Échec : {e.message()}") ;
    in Int g, g >= 10: show("Admis") ;
    in Int g: show("Recalé") ;
;
```

Forme expression : chaque cas donne une valeur avec `then` \[D51\].

```lion
let label = match status:
    idle then "Aucune donnée"
    loading then "Chargement…"
    otherwise then "Prêt"
;
```

| Motif | Sens |
| --- | --- |
| `42`, `"oui"`, `red` | égal à cette valeur |
| `in Int g` | de type Int ; `g` désigne la valeur affinée |
| `in 1..9`, `in primes` | appartient à cet ensemble \[D76\] |
| `in Int g, g >= 10` | un motif suivi de conditions ; la virgule vaut `and` |
| `otherwise` | tous les cas restants \[D16\] |

- Les cas sont essayés dans l'ordre ; le premier qui correspond gagne.
- Un cas possible non couvert, sans `otherwise`, est une erreur de compilation.
- Un cas qui ne peut jamais être atteint provoque un avertissement \[D77\].

## 11. Fonctions

Une fonction se déclare avec `fun`, ses paramètres sont des constantes, et toute modification de l'extérieur est annoncée dans sa signature.

### 11.1 Déclarer

```lion
fun area(width in Float, height in Float) in Float:
    return width * height
;
fun square(x) = x * x                     // forme courte [D7]
let twice = fun(x) = x * 2                // anonyme, forme courte
let log = fun(msg): show("> {msg}") ;     // anonyme, forme bloc
```

Ordre des clauses d'une signature \[D64\] : paramètres, puis `in` et le type de retour, puis les variables de type (§15), puis `modifies`.

```lion
fun biggest(a in T, b in T) in T, T in Comparable:
fun reset() modifies total:
```

### 11.2 Paramètres

- Un paramètre est une constante, immuable en profondeur, comme un `let`.
- Un paramètre `var` peut être modifié, et la modification atteint la variable de l'appelant.

```lion
fun add_grade(var notes, n):
    notes.add(n)
;
var mine = [12, 8]
let theirs = [10]
add_grade(mine, 15)          // correct : mine vaut [12, 8, 15]
add_grade(var mine, 15)      // identique ; le var à l'appel est facultatif et purement visuel
add_grade(theirs, 15)        // erreur : un let ne peut pas aller dans un paramètre var
add_grade([1, 2], 15)        // correct : valeur temporaire, modification perdue
```

- Écrire `var` à l'appel devant un paramètre qui n'est pas `var` est une erreur de compilation.
- Les valeurs par défaut vont en fin de liste : `fun f(a, c = 1, d = 2)`. On ne peut pas en sauter une : pour changer `d`, on écrit `f(1, 1, 5)`.
- On peut nommer les arguments pour la lisibilité, `area(width: 3, height: 4)`, mais l'ordre reste obligatoire.

### 11.3 Curryfication

Appeler une fonction avec une partie de ses paramètres obligatoires renvoie une fonction qui attend le reste.

```lion
fun f(a, b, c = 1): ... ;
let g = f(1)          // fonction qui attend b
g(2)                  // exécute f(1, 2, 1)
f(1)(2, 5)            // exécute f(1, 2, 5)
```

- La curryfication s'arrête dès que tous les paramètres obligatoires sont fournis : la fonction s'exécute avec les valeurs par défaut.
- Pour changer une valeur par défaut, on la fournit dans l'appel qui complète les paramètres obligatoires.
- `g()` appelle la fonction ; `g` seul désigne la fonction comme valeur.

### 11.4 Valeur de retour

- Si un `return` de la fonction renvoie une valeur, **tous** les chemins doivent en renvoyer une ; sinon, erreur de compilation.
- Si aucun `return` ne renvoie de valeur, la fonction renvoie `none`, de type `None`.
- `return if x > 0 then 1` est autorisé : la fonction renvoie alors un `maybe Int`.

### 11.5 Variables extérieures et closures

- Une fonction peut **lire** librement les variables extérieures. Une closure lit une copie des variables locales prise à sa création ; une fonction globale lit la valeur courante des globales \[D71\].
- Pour **modifier** une variable extérieure, globale ou capturée, la fonction l'annonce avec `modifies`. La variable est alors liée par référence et maintenue en vie tant que la fonction existe.
- Modifier un objet à travers une globale `shared` compte aussi comme une modification \[D50\].

```lion
fun make_counter():
    var count = 0
    fun next() modifies count:
        count += 1
        return count
    ;
    return next
;
let c = make_counter()
c()     // 1
c()     // 2
```

- On n'écrit `modifies` que pour une modification **directe**. Si `f` appelle `g`, qui modifie `total`, le compilateur le calcule seul ; `f` n'a rien à écrire.
- Ces effets calculés servent à refuser les exécutions parallèles dangereuses (§19). Les outils peuvent les afficher.

## 12. Structures et invariants

Un `struct` décrit des données ; ses méthodes se définissent à part, et ses invariants font de lui un vrai ensemble de valeurs valides.

### 12.1 Déclarer

```lion
struct Exam:
    student in Text
    score in Float = 0, 0 <= score       // valeur par défaut [D6], puis une contrainte
    max in Float = 20
    score <= max                         // invariant de structure
    private note in Text = ""             // visible seulement dans ce fichier [D10]
;
```

- Chaque ligne déclare un champ (`nom in Type`, puis éventuellement `= défaut` et des contraintes séparées par des virgules) ou un invariant portant sur plusieurs champs.
- Un `struct` est `Student = {(name, grade) | name in Text, grade in Float, 0 <= grade <= 20}`, au sens mathématique.

### 12.2 Construire

```lion
let a = Student("Léa", 12)                   // forme appel
let b = Student(name: "Léa", grade: 12)      // noms pour la lisibilité, ordre obligatoire
let c = ("Léa", 12) as Student               // forme ensemble
let d = ("Léa", 12) in Student               // dans une déclaration
```

- Les quatre formes sont équivalentes et suivent les règles d'appel des fonctions \[D55\].
- On peut omettre les champs finaux qui ont une valeur par défaut \[D70\]. `AppState()` crée une valeur dont tous les champs prennent leur défaut.
- `let t = ("Léa", 12)`, sans type, est un simple couple.

### 12.3 Invariants

| Situation | Vérification | En cas de violation |
| --- | --- | --- |
| Construction d'un type qui a des invariants | à la construction | le résultat est de type `T or Error`, à traiter |
| Construction avec des valeurs écrites en dur et valides | à la compilation | aucune : le résultat est un simple `T` \[D39\] |
| `s.grade = 22` hors de toute méthode | immédiate | bug \[D40\] |
| Dans une méthode `var self` | quand l'appel `var self` le plus extérieur sur cet objet se termine | bug |
| Test `("Léa", 25) in Student` | immédiate | le test vaut `false`, sans bug |

Pendant une méthode `var self`, l'objet peut être temporairement invalide, y compris pour les fonctions qu'elle appelle \[D9\].

```lion
fun Student.add_bonus(var self, n):
    self.grade += n                     // 18 + 5 = 23 : pas encore vérifié
    if self.grade > 20: self.grade = 20 ;
;                                       // vérifié ici : 20, correct
```

### 12.4 Méthodes

```lion
fun Student.passes() in Bool:
    return self.grade >= 10
;
fun Student.add_bonus(var self, n): ... ;
```

- `self` n'apparaît dans la liste des paramètres que sous la forme `var self`, pour une méthode qui modifie l'objet.
- La mutabilité dépend de la variable qui tient l'objet : sur un `let`, appeler une méthode `var self` est une erreur de compilation.
- On peut ajouter des méthodes à n'importe quel type, depuis n'importe quel fichier : `fun Text.shout() = ...`. Elles ne sont visibles que là où ce fichier est importé.

### 12.5 Égalité

`==` compare champ par champ par défaut. Un type peut la redéfinir avec `fun Student.equals(other in Student) in Bool`. Un `Set` utilise cette égalité pour éliminer les doublons.

### 12.6 Méthodes détachées

| Expression | Résultat |
| --- | --- |
| `s.passes` | une fonction sans argument, qui lit une copie de `s` |
| `s.add_bonus`, `s` non `shared` | erreur de compilation |
| `score.increment`, `score` un `var shared` | une fonction qui modifie le même compteur ; le lien reste vivant, même rangé dans un `let` |

## 13. Énumérations et unions

Une énumération est un ensemble fini de valeurs nommées, et une union nommée est une liste fermée de types ; les deux s'écrivent avec `=`.

### 13.1 Énumérations

```lion
Color = {red, green, blue}            // sans ordre
Days = [mon, tue, wed, thu, fri]      // ordonnée [D33] : mon < tue, parcours dans l'ordre
Bool = {true, false}                  // définie ainsi dans la bibliothèque standard [D45]
```

- Le nom du type prend une majuscule, les valeurs une minuscule.
- La forme complète d'une valeur est `Color.red`. On peut écrire `red` seul quand le type attendu est connu : dans un `match` sur une `Color`, après `in Color`, ou comme argument d'un paramètre de type `Color` \[D32\].
- Parcourir une énumération `{...}` se fait dans un ordre non garanti ; une énumération `[...]` se parcourt dans l'ordre et se compare avec `<`.

### 13.2 Unions nommées

```lion
Shape = Circle or Rect

fun area(s in Shape) in Float:
    return match s:
        in Circle c then 3.14159 * c.r ^ 2
        in Rect r then r.w * r.h
    ;
;
```

- Une union nommée est **fermée** : sa liste de types est fixe, et `match` vérifie que tous les cas sont traités.
- Un trait (§14) est **ouvert** : n'importe quel type peut le satisfaire. On choisit l'union quand la liste est connue d'avance.
- `maybe T`, `Int or Error` et `Shape` reposent sur le même mécanisme d'union (§7.3).

## 14. Traits et polymorphisme

Un trait nomme une exigence, et tout type qui la remplit la satisfait automatiquement, sans le déclarer ; il n'y a pas d'héritage.

```lion
trait Shape:
    fun area() in Float
    fun describe() in Text:                  // méthode par défaut
        return "forme d'aire {self.area()}"
    ;
;

struct Circle:
    r in Float, r >= 0
;
fun Circle.area() in Float = 3.14159 * self.r ^ 2    // Circle est maintenant une Shape

fun total_area(shapes in List of Shape) in Float = sum([s.area(), s in shapes])
```

### 14.1 Règles

- Un type satisfait un trait dès qu'il possède toutes les méthodes exigées, avec les bons types.
- La conformité accidentelle est acceptée : un `Farm` avec une méthode `area()` est une `Shape`.
- Un trait peut exiger des champs, par exemple `name in Text` (provisoire).
- `x in Shape` teste à l'exécution si une valeur satisfait le trait.
- Une méthode ajoutée depuis l'extérieur ne fait satisfaire le trait que là où elle est importée.

### 14.2 Méthodes par défaut

- Un type qui satisfait le trait reçoit ses méthodes par défaut, y compris par conformité accidentelle.
- Si le type définit déjà une méthode du même nom, la sienne l'emporte.
- Si deux traits fournissent au même type deux méthodes par défaut du même nom, c'est une erreur de compilation.

### 14.3 Pas de substitution entre structures

Une fonction qui attend un `Rectangle` refuse un `Square`, même si les deux sont des `Shape`. Le problème classique Carré/Rectangle ne peut donc pas se poser.

### 14.4 Coût

Une liste mélangée comme `[circle, square]`, de type `List of Shape`, choisit la méthode à appeler à l'exécution. C'est le seul cas où Lion paie ce coût \[D1\] ; partout ailleurs, l'appel est résolu à la compilation.

## 15. Génériques

Les types génériques s'écrivent en mots avec `of`, et une variable de type se déclare toujours avec la contrainte qu'elle respecte.

### 15.1 Types génériques

```lion
List of Int
Set of Text
Map of (Text, Int)            // plusieurs paramètres : entre parenthèses
List of List of Int           // s'imbrique vers la droite
```

Un `struct` ou un trait peut lui aussi prendre des paramètres de type \[D78\] :

```lion
struct Pair of (A, B), A in Type, B in Type:
    first in A
    second in B
;
```

### 15.2 Fonctions génériques

```lion
fun biggest(a in T, b in T) in T, T in Comparable:
    return if b > a then b else a
;
fun first(l in List of T) in maybe T, T in Type:
    return if l.size > 0 then l[1]
;
```

- Une variable de type est introduite par une clause `T in …` : un trait, ou `Type`, l'ensemble de tous les types.
- Un nom en majuscule qui n'est ni un type connu ni une variable déclarée est une erreur de compilation. Une faute de frappe comme `Strng` ne devient donc jamais générique par accident.

### 15.3 Compilation

Le compilateur produit une copie spécialisée du code pour chaque type utilisé. C'est le plus rapide à l'exécution, au prix d'un programme un peu plus gros.

## 16. Collections, ensembles et compréhensions

Les crochets gardent l'ordre, les accolades non ; un ensemble peut être fini (`Set`) ou défini par une propriété (`Domain`).

### 16.1 Les cinq collections

| Type | Écriture | Ordre | Doublons | Parcourable |
| --- | --- | --- | --- | --- |
| `List of T` | `[3, 1, 3]` | oui | oui | oui |
| `Set of T` | `{3, 1}` | non garanti | non | oui |
| `Range` | `1..9` | croissant | non | oui |
| `Domain of T` | `{x in Int, x > 0}` | — | — | non : tests d'appartenance seulement |
| `Map of (K, V)` | via la bibliothèque standard | — | clés uniques | oui |

### 16.2 Listes et textes

- Les indices commencent à **1** : `l[1]` est le premier élément, `l[l.size]` le dernier.
- Un indice hors limites, nul ou négatif est un bug. On écrit `l.first` et `l.last` \[D38\].
- Un extrait inclut ses bornes : `l[2..4]` \[D41\].
- `x in l` utilise `==`. Donc `nan in [nan]` vaut `false`, avec une alerte en mode interprété \[D8\].
- Un `Text` est une suite de caractères Unicode : `"é".size` vaut 1 et `"Lion"[1]` vaut `"L"` \[D23, D42\].

### 16.3 Intervalles

- `a..b` : entiers seulement, bornes incluses, toujours croissant. `9..1` est vide.
- Pour décroître : `reverse(1..9)`. `reverse` n'accepte qu'un type ordonné : liste, intervalle, énumération `[...]`.
- `1..` sans borne et `0.0..1.0` sont des erreurs de compilation.
- Un `Range` est un `Set` ordonné. Il n'occupe jamais de mémoire : il est parcouru sans être construit \[D46\].
- `1..9 union 20..30` donne un `Set` ordinaire, sans ordre garanti.
- Il n'y a pas de syntaxe de pas : on filtre, et on utilise des crochets pour garder l'ordre : `[x in 1..9, x mod 2 == 1]`.

### 16.4 Compréhensions

```lion
{f(s), s in students, s.grade >= 10}     // sortie, générateur, conditions
{s in students, s.grade >= 10}           // sortie implicite : s lui-même
{x in A, x in B}                         // intersection de A et B
{(a, b), a in A, b in B}                 // produit cartésien : sortie explicite obligatoire
[s.name, s in students]                  // version liste : ordre et doublons conservés
```

1. Le premier élément est l'expression de sortie, sauf s'il a la forme `v in collection` avec `v` nouvelle : c'est alors un générateur, et la sortie est `v`.
2. `v in X` avec une variable **nouvelle** est un générateur ; avec une variable **déjà définie**, c'est une condition.
3. La sortie implicite n'est permise qu'avec un seul générateur : `{a in A, b in B}` est une erreur de compilation.
4. La virgule entre conditions vaut `and`.

### 16.5 Set ou Domain ?

Le compilateur décide, à partir de la forme de l'expression :

- Un générateur sur une collection finie donne un `Set`.
- Un générateur sur un type, comme `x in Int`, donne un `Domain`, sauf si les conditions bornent `x` des deux côtés par des comparaisons simples : `x` comparé avec `<`, `<=`, `>` ou `>=` à une expression qui ne dépend pas de `x`. Les autres conditions sont alors de simples filtres.

```lion
{x in Int, x > 0}                        // Domain
{x in Int, x > 0, x < n}                 // Set : bornes simples, n peut être une variable
{x in Int, 1 <= x, x <= 9, x mod 2 == 0} // Set : bornes + filtre
{x in Int, x * x < 50}                   // Domain : fini en maths, mais bornes non reconnues
```

Sur un `Domain`, parcourir, tester l'égalité, tester le vide ou demander la taille sont des erreurs de compilation.

### 16.6 Opérations ensemblistes

`union`, `inter`, `minus` et `subset` s'appliquent aux `Set` et aux `Domain`. Le complémentaire s'écrit `Int minus pos`.

| Opération | Résultat |
| --- | --- |
| `Set inter Domain` | `Set` (c'est un filtre) |
| `Set union Domain` | `Domain` |
| `Set minus Domain` | `Set` \[D79\] |
| `Domain minus Set` | `Domain` \[D79\] |

Insérer `NaN` dans un `Set` est un bug (§8.2).

## 17. Mémoire, partage et mutabilité

En Lion, affecter copie ; partager un objet modifiable demande le mot `shared`, et rien d'autre ne se partage.

### 17.1 Sémantique de valeur

```lion
var a = [1, 2]
let b = a            // b est une copie
a.add(3)
show(b)              // [1, 2]
```

- Toute affectation et tout passage de paramètre non `var` se comportent comme une copie.
- L'implémentation ne copie réellement qu'au moment d'une modification. Copier une grosse liste pour la lire ne coûte donc rien.
- Seul un paramètre `var` travaille sur la donnée de l'appelant (§11.2).

### 17.2 Partage explicite : `shared`

```lion
var score = shared Counter()
score.increment()
```

| Situation | Règle |
| --- | --- |
| Modifier un objet `shared` | il faut le tenir par un `var` |
| `let t = s`, `s` un `let shared` | autorisé : même objet, jamais modifiable par personne |
| `var t = s`, `s` un `let shared` | erreur de compilation |
| `let t = s`, `s` un `var shared` | `t` reçoit une copie figée, qui n'est plus partagée |
| Passer un `var shared` à un paramètre non `var` | le paramètre reçoit une copie figée |
| Passer un `var shared` à un paramètre `var` | le paramètre désigne le même objet |
| Garder un lien vivant dans un champ | le champ doit être `var` |

Conséquence : un `let` ne change jamais, même quand il contient un objet partagé.

- `a == b` compare le contenu ; `a same b` teste s'il s'agit du même objet partagé.

### 17.3 Gestion automatique

- Les valeurs ordinaires sont libérées automatiquement à la fin de leur bloc, sans ramasse-miettes.
- Les objets `shared` utilisent un comptage de références, avec un détecteur de cycles pour les graphes.
- Il n'y a donc jamais de pause imprévisible, ce qui garde une interface graphique fluide.

## 18. Erreurs

Lion sépare les bugs, qui arrêtent le programme, des échecs attendus, qui sont des valeurs que le compilateur oblige à traiter ; il n'y a pas d'exceptions.

### 18.1 Les deux familles

| Famille | Exemples | Traitement |
| --- | --- | --- |
| Bug : le programme est faux | débordement d'Int, invariant violé par une modification, indice hors limites, division entière par zéro, `x over 0`, `NaN` dans un `Set` | arrêt immédiat, impossible à intercepter |
| Échec attendu : le monde ne coopère pas | fichier absent, texte qui n'est pas un nombre, donnée qui viole un invariant à la construction | une valeur `Error`, dans un type `T or Error` |

### 18.2 Le type `Error`

- `Error` est un trait : tout type qui possède une méthode `message() in Text` est une erreur \[D17, D66\]. On peut donc créer ses propres erreurs.
- `error("note invalide")` crée une erreur simple de la bibliothèque standard \[D65\].

### 18.3 Traiter ou faire remonter

```lion
fun load_grade(path in Text) in Int or Error:
    let text = try read_file(path)       // en cas d'erreur, load_grade la renvoie telle quelle
    return try text as Int
;

let g = load_grade("notes.txt")
if g in Error:
    show("Impossible : {g.message()}")
    return
;
show(g + 1)                              // ici, g est un Int
```

- Utiliser une valeur `T or Error` comme un `T` sans l'avoir testée est une erreur de compilation.
- `try expr` : si `expr` produit une `Error`, la fonction englobante la renvoie immédiatement. Sinon, `try` donne la valeur.
- `try` couvre toute l'expression à sa droite et intercepte n'importe quelle étape qui échoue \[D35\] : `try r.get("grade") as Float`.
- Dans une compréhension, `try` abandonne toute la compréhension et fait remonter l'erreur.
- `try` n'est permis que dans une fonction dont le type de retour contient `Error`, ou au niveau d'un script, où l'erreur arrête le script avec son message \[D80\].

### 18.4 Messages de bug

Un bug affiche le fichier, la ligne, les valeurs en cause et, si possible, une suggestion \[D15, D18\] :

```text
Bug : débordement d'entier (notes.lion, ligne 12)
  9223372036854775807 + 1 dépasse la capacité d'un Int.
  Suggestion : utiliser un Float, ou vérifier la valeur avant l'addition.
```

## 19. Concurrence et parallélisme

Le parallélisme est explicite et vérifié à la compilation : aucune corruption de données entre tâches n'est possible.

### 19.1 Tâches légères

```lion
let t = task load_grade("notes.txt")    // type : Task of (Int or Error)
// ... le programme continue ...
let g = wait t                           // récupère le résultat
```

- Aucun mot-clé `async` ne se propage de fonction en fonction : n'importe quelle fonction peut être lancée comme tâche.
- `wait` ne bloque que la tâche en cours. Une interface graphique utilise des fonctions de rappel de sa bibliothèque, pour ne jamais geler \[D54\].

### 19.2 Parallélisme de données

```lion
let results = parallel [estimate_pi(1_000_000, seed), seed in 1..8]
let primes = parallel {p in 2..1_000_000, is_prime(p)}
parallel for f in files: analyse(f) ;
```

Le parallélisme n'est jamais automatique : sur de petites boucles, il ralentirait plus qu'il n'accélère.

### 19.3 Règles de sûreté

| Donnée | Entre deux tâches |
| --- | --- |
| Valeur ordinaire (copiée ou immuable) | passe librement |
| `shared` | interdit : erreur de compilation |
| `shared synced` | autorisé : un verrou automatique protège chaque accès |

- Une fonction exécutée en parallèle ne peut pas modifier d'état extérieur, même indirectement. Le compilateur le vérifie grâce aux effets calculés (§11.5).
- Une fonction étrangère (§21) est supposée modifier un état global, sauf si elle est déclarée `pure` \[D48\].
- Un générateur aléatoire global modifie un état caché : en parallèle, chaque tâche crée le sien, `var rng = random.Generator(seed)`.
- `show` dans des tâches parallèles : l'ordre des lignes n'est pas garanti, mais deux lignes ne se mélangent jamais \[D47\].
- Un `synced` que rien ne partage entre tâches provoque un avertissement \[D53\].

Exemple de message :

```text
Erreur : f modifie `total` (via g) et ne peut pas s'exécuter en parallèle (calcul.lion, ligne 18).
  Suggestion : utiliser une variable locale, ou déclarer total en shared synced.
```

## 20. Modules et programmes

Un fichier est un module, et seul le fichier lancé exécute des instructions ; il n'y a pas de fonction `main`.

### 20.1 Programmes

- Le code écrit au niveau du fichier s'exécute directement : `show("Bonjour")` est un programme complet.
- Au niveau d'un script, `return` termine le script. `exit(code)` termine avec un code de sortie \[D36\].
- Un programme Lion a tous les droits du système : fichiers, réseau, etc. Il n'y a pas de modèle de permissions.

### 20.2 Modules

```lion
use geometry            // geometry.lion
use shapes.circle       // shapes/circle.lion [D12]

let a = geometry.area(3, 4)     // les noms importés restent qualifiés [D84]
```

- Un module importé ne contient que des déclarations : `fun`, `struct`, `trait`, types, `let` et `var` globaux. Des instructions exécutables à son niveau sont une erreur de compilation \[D31\].
- Les globales d'un module s'initialisent dans l'ordre du fichier, à sa première utilisation \[D81\].
- Deux modules qui s'importent mutuellement sont autorisés, tant que leurs globales ne dépendent pas l'une de l'autre en cycle \[D81\].

### 20.3 Visibilité

- Tout est public par défaut.
- `private` devant un champ, une fonction ou une globale limite son accès au fichier qui le déclare \[D10\].
- Une méthode ajoutée à un type existant n'est visible que dans les fichiers qui importent son module. Deux bibliothèques peuvent donc ajouter chacune une méthode `area` à `Text` sans conflit, tant qu'on ne les importe pas au même endroit.

### 20.4 Paquets

Un gestionnaire de paquets officiel est prévu, en dernière étape de la feuille de route (§28). Sa conception reste à faire.

## 21. Calcul à la compilation et FFI

Lion peut exécuter du code pendant la compilation, mais n'a pas de macros ; le code C s'appelle seulement dans des blocs `unsafe`.

### 21.1 Calcul à la compilation

```lion
let primes = compile {p in 2..1_000_000, is_prime(p)}
```

- `compile expr` évalue l'expression pendant la compilation ; le programme contient directement le résultat \[D21\].
- C'est l'appel qui est marqué, pas la fonction : n'importe quelle fonction sans effet extérieur peut servir.
- Une expression qui lit un fichier, le clavier ou le réseau, ou qui modifie un état, est refusée par `compile` \[D82\].
- Il n'y a pas de macros : ce qu'on lit est exactement ce qui s'exécute.

### 21.2 FFI : appeler du C

```lion
foreign "libm" pure fun cos(x in Float) in Float
foreign "libc" fun getenv(name in Text) in maybe Text

fun cosine(x in Float) in Float:
    unsafe:
        return cos(x)
    ;
;
```

- Une fonction `foreign` déclare une fonction C et sa bibliothèque.
- Elle ne s'appelle que dans un bloc `unsafe`, car le compilateur de Lion ne peut rien y vérifier.
- Les bibliothèques, à commencer par la bibliothèque standard, enveloppent ces appels dans des fonctions sûres. Un utilisateur normal n'écrit jamais `unsafe`.
- Sans `pure`, une fonction étrangère est supposée modifier un état global et ne peut pas s'exécuter en parallèle \[D48\].
- Les indices commencent à 1 en Lion et à 0 en C : les enveloppes se chargent de la traduction.

## 22. Exécution : deux modes, une sémantique

Une partie commune lit et vérifie le code ; ensuite, une machine virtuelle l'interprète ou un compilateur produit du code natif.

### 22.1 Architecture

| Étape | Rôle |
| --- | --- |
| Partie commune | lecture du code, vérification des types, des effets et des règles de sûreté. Toutes les erreurs de compilation viennent d'ici, quel que soit le mode |
| Mode interprété | machine virtuelle à bytecode : démarrage instantané, idéal pour les scripts et le développement ; toutes les alertes sont actives |
| Mode compilé | code machine natif, via une infrastructure existante de type LLVM ; cible : Java/C# ou mieux |

### 22.2 Garantie

Un programme donne exactement les mêmes résultats et les mêmes bugs dans les deux modes. La seule différence permise : les alertes, émises uniquement en mode interprété.

### 22.3 Alertes du mode interprété

| Alerte | Déclencheur |
| --- | --- |
| Perte de précision | conversion Int → Float d'une valeur au-delà de 2^53 |
| Valeur flottante spéciale | apparition de l'infini ou de `NaN` |
| Test toujours faux | `x in liste` avec `x` valant `NaN` |

### 22.4 Performance

- Les vérifications à l'exécution (débordement, indices, invariants) sont supprimées quand le compilateur prouve qu'elles sont inutiles.
- Pas de ramasse-miettes pour les valeurs ordinaires, donc pas de pauses (§17.3).
- Code spécialisé par type pour les génériques (§15.3).
- Les intervalles ne sont jamais construits en mémoire (§16.3).

## 23. Bibliothèque standard

La bibliothèque standard fournit d'office tout ce qu'un script d'analyse ou une application demande ; son API détaillée reste à concevoir.

| Module | Contenu | Étape (§28) |
| --- | --- | --- |
| noyau (sans `use`) | `show`, `ask`, `error`, `exit`, `sum`, `reverse`, `floor`, `ceil`, `round`, `isqrt`, types de base | 3 |
| `text` | recherche, découpage, remplacement, majuscules | 3 |
| `math` | constantes, trigonométrie, logarithmes, statistiques de base | 3 |
| `sets` | opérations avancées sur `Set` et `Domain` | 3 |
| `files` | lire, écrire, lister ; tout renvoie `T or Error` | 3 |
| `csv`, `json` | lecture et écriture ; `r.get("col")` renvoie `Text or Error` \[D37\] | 3 |
| `dates`, `time` | dates, durées, mesure du temps | 3 |
| `random` | `random.Generator(seed)` et des raccourcis globaux | 3 |
| `test` | `expect` et outils de test (§24) | 4 |
| `net` | réseau | après 3 |
| `ui` | bibliothèque graphique propre à Lion | 7 |

- `show(x)` affiche une valeur ; `ask("Ton nom ?")` lit une ligne au clavier et renvoie un `Text` \[D29\].
- Toute opération d'entrée-sortie qui peut échouer renvoie `T or Error`.
- La bibliothèque graphique sera écrite pour Lion ; elle utilisera la FFI pour parler au système (fenêtres, clavier, carte graphique).

## 24. Outillage

Un seul outil, `lion`, regroupe l'exécution, la compilation, les tests et le formatage \[D30\].

| Commande | Rôle |
| --- | --- |
| `lion run f.lion` | exécute en mode interprété, avec toutes les alertes |
| `lion build f.lion` | compile en code natif |
| `lion check` | vérifie sans exécuter ; les avertissements de style y sont intégrés |
| `lion test` | lance tous les tests du projet |
| `lion fmt` | formate selon le style officiel unique, non configurable \[D20\] |
| `lion` seul | ouvre le mode interactif : on tape du Lion ligne par ligne \[D26\] |

### 24.1 Tests intégrés \[D19\]

```lion
test "addition":
    expect add(2, 3) == 5
    expect add(-1, 1) == 0
;
```

- Un bloc `test` peut apparaître dans n'importe quel fichier ; il n'est exécuté que par `lion test`.
- `expect` échoue avec les valeurs comparées : « attendu 5, obtenu 6 » \[D72\].

### 24.2 Débogueur

Débogueur pas à pas en mode interprété, où toute l'information est disponible \[D25\].

### 24.3 Messages d'erreur

Tous les messages, du compilateur comme des bugs, sont pédagogiques : ce qui ne va pas, où, et une suggestion de correction \[D18\]. Le compilateur utilise l'indentation pour mieux localiser un `;` oublié (§5.3).

## 25. Versions et compatibilité

Tout peut changer jusqu'à la 1.0 ; ensuite, un code qui compile continue de compiler.

| Période | Règle |
| --- | --- |
| 0.x | toute règle peut changer, pour corriger les erreurs de conception |
| 1.0 et après | un programme qui compile continue de compiler et de se comporter pareil |
| Changement qui casse | uniquement via une nouvelle « édition » : chaque projet déclare son édition et migre quand il veut |

Des projets d'éditions différentes doivent pouvoir s'utiliser mutuellement \[D83\].

## 26. Grammaire formelle

Cette grammaire EBNF décrit la syntaxe de Lion 0.1 ; les règles dépendantes du contexte sont listées juste après.

```ebnf
(* NL = fin de ligne. { x } = répétition, [ x ] = option. *)

program        = { line } ;
line           = [ statement ] NL ;

statement      = use_decl | type_def | struct_decl | trait_decl | fun_decl
               | foreign_decl | test_decl | var_decl | assignment
               | if_stmt | match_stmt | for_stmt | while_stmt | unsafe_block
               | return_stmt | "break" | "continue" | expect_stmt | expr ;

body           = NL { line } | statement ;        (* plusieurs lignes, ou une instruction sur la même ligne *)

(* Déclarations *)
use_decl       = "use" lower_id { "." lower_id } ;
var_decl       = ( "let" | "var" ) lower_id [ "=" expr ] [ "in" type ] ;
assignment     = place ( "=" | "+=" | "-=" | "*=" ) expr ;
place          = ( lower_id | "self" ) { "." lower_id | "[" expr "]" } ;

fun_decl       = [ "infix" ] "fun" [ upper_id "." ] lower_id "(" [ params ] ")" signature
                 ( ":" body ";" | "=" expr ) ;
signature      = [ "in" type ] { "," upper_id "in" type } [ "modifies" lower_id { "," lower_id } ] ;
params         = param { "," param } ;
param          = [ "var" ] ( lower_id | "self" ) [ "in" type ] [ "=" expr ] ;
foreign_decl   = "foreign" text_lit [ "pure" ] "fun" lower_id "(" [ params ] ")" [ "in" type ] ;

struct_decl    = "struct" upper_id [ type_params ] ":" NL { struct_line NL } ";" ;
struct_line    = [ "private" ] lower_id "in" type [ "=" expr ] { "," expr }
               | expr ;                                        (* invariant de structure *)
trait_decl     = "trait" upper_id [ type_params ] ":" NL { trait_line NL } ";" ;
trait_line     = "fun" lower_id "(" [ params ] ")" signature [ ":" body ";" ]
               | lower_id "in" type ;
type_params    = "of" ( upper_id | "(" upper_id { "," upper_id } ")" ) { "," upper_id "in" type } ;
type_def       = upper_id "=" ( "{" lower_id { "," lower_id } "}"     (* énumération *)
                              | "[" lower_id { "," lower_id } "]"     (* énumération ordonnée *)
                              | type ) ;                              (* union nommée *)
test_decl      = "test" text_lit ":" body ";" ;
expect_stmt    = "expect" expr ;

(* Contrôle *)
if_stmt        = "if" expr ":" body { "elif" expr ":" body } [ "else" ":" body ] ";" ;
for_stmt       = [ "parallel" ] "for" lower_id "in" expr ":" body ";" ;
while_stmt     = "while" expr ":" body ";" ;
return_stmt    = "return" [ expr ] ;
unsafe_block   = "unsafe" ":" body ";" ;
match_stmt     = "match" expr ":" NL { pattern ":" body ";" NL } ";" ;
match_expr     = "match" expr ":" NL { pattern "then" expr NL } ";" ;
pattern        = ( "otherwise" | literal | postfix | "in" ( type | expr ) [ lower_id ] ) { "," expr } ;

(* Types *)
type           = maybe_type { "or" maybe_type } ;
maybe_type     = [ "maybe" ] app_type ;
app_type       = qual_type [ "of" ( app_type | "(" type { "," type } ")" ) ]
               | "(" type "," type { "," type } ")"             (* n-uplet *)
               | "(" type ")"
               | "fun" "(" [ type { "," type } ] ")" [ "in" type ] ;

qual_type      = { lower_id "." } upper_id ;                   (* ex. notes_data.Student *)

(* Expressions, de la priorité la plus faible à la plus forte *)
expr           = prefix_kw expr | if_expr | match_expr | fun_expr | or_expr ;
prefix_kw      = "try" | "task" | "wait" | "compile" | "shared" | "synced" | "parallel" ;
if_expr        = "if" expr "then" expr { "elif" expr "then" expr } [ "else" expr ] ;
fun_expr       = "fun" "(" [ params ] ")" signature ( ":" body ";" | "=" expr ) ;
or_expr        = and_expr { "or" and_expr } ;
and_expr       = not_expr { "and" not_expr } ;
not_expr       = "not" not_expr | comparison ;
comparison     = as_expr [ ( "in" | "subset" | "same" ) as_expr | cmp_op as_expr { cmp_op as_expr } ] ;
cmp_op         = "==" | "!=" | "<" | ">" | "<=" | ">=" ;
as_expr        = union_expr { "as" type } ;
union_expr     = inter_expr { ( "union" | "minus" ) inter_expr } ;
inter_expr     = range_expr { "inter" range_expr } ;
range_expr     = additive [ ".." additive ] ;
additive       = multiplicative { ( "+" | "-" ) multiplicative } ;
multiplicative = unary { ( "*" | "/" | "div" | "mod" | "over" | infix_id ) unary } ;
unary          = "-" unary | power ;
power          = postfix [ "^" unary ] ;
postfix        = primary { "(" [ args ] ")" | "[" expr "]" | "." ( lower_id | upper_id ) } ;
args           = arg { "," arg } ;
arg            = [ "var" ] [ lower_id ":" ] expr ;
primary        = literal | lower_id | upper_id | "self" | "(" expr ")" | tuple
               | "[" [ elements ] "]" | "{" [ elements ] "}" ;
tuple          = "(" ")" | "(" element "," [ element { "," element } ] ")" ;
elements       = element { "," element } ;
element        = [ lower_id ":" ] expr ;

(* Lexique *)
lower_id       = lower { letter | digit | "_" } ;
upper_id       = upper { letter | digit | "_" } ;
infix_id       = lower_id ;                                     (* seulement s'il est déclaré infix *)
literal        = int_lit | float_lit | text_lit | "true" | "false" | "none" ;
int_lit        = digit { digit | "_" } | "0x" hex { hex | "_" } | "0b" bin { bin | "_" } ;
float_lit      = digit { digit | "_" } "." digit { digit | "_" } [ ( "e" | "E" ) [ "+" | "-" ] digit { digit } ] ;
text_lit       = '"' { char | escape | "{" expr "}" } '"' ;
```

**Règles dépendantes du contexte**

1. **Annotation ou test.** Dans `var_decl`, si l'expression se termine par `in T` avec `T` un type, c'est une annotation. Pour un test, on met des parenthèses : `let ok = (("Léa", 12) in Student)`.
2. **Deux sens de `:`.** Dans les parenthèses d'un appel ou d'un n-uplet, `nom :` introduit un argument ou un champ nommé. Partout ailleurs, `:` ouvre un bloc.
3. **Continuation.** Les fins de ligne sont ignorées tant qu'une `(`, un `[` ou une `{` reste ouvert.
4. **Compréhensions.** Une liste d'éléments est une compréhension dès qu'un élément a la forme `v in X` avec `v` nouvelle (§16.4).
5. **Enchaînement.** Seuls les `cmp_op` s'enchaînent ; `a in B in C` est une erreur.
6. **Mots-clés préfixes.** `try`, `task`, `wait`, `compile`, `shared`, `synced` et `parallel` s'appliquent à toute l'expression à leur droite.
7. **Casse.** `upper_id` et `lower_id` ne sont pas interchangeables : un type en minuscule, ou une variable en majuscule, est une erreur de compilation.

## 27. Programmes d'exemple

Ces trois programmes, écrits pendant la conception, servent de tests de réalité : une implémentation conforme doit les accepter tels quels, sauf la bibliothèque `ui`, qui reste imaginaire.

### 27.1 Analyse de notes depuis un CSV

```lion
use csv

struct Student:
    name in Text
    grade in Float, 0 <= grade <= 20
;

fun Student.passes() in Bool = self.grade >= 10

fun load(path in Text) in List of Student or Error:
    let rows = try csv.read(path)
    var students = [] in List of Student
    for r in rows:
        let name = try r.get("name")
        let grade = try r.get("grade") as Float
        students.add(try Student(name, grade))     // une note de 25 devient une Error
    ;
    return students
;

let result = load("notes.csv")
if result in Error:
    show("Erreur : {result.message()}")
    return
;
let admis = {s.name, s in result, s.passes()}
let moyenne = sum([s.grade, s in result]) / result.size     // NaN si le fichier est vide, avec alerte
show("Admis : {admis}")
show("Moyenne : {moyenne}")
```

### 27.2 Calcul intensif et parallèle

```lion
use random

fun estimate_pi(samples in Int, seed in Int) in Float:
    var rng = random.Generator(seed)        // un générateur par tâche
    var hits = 0
    for i in 1..samples:
        let x = rng.float()
        let y = rng.float()
        if x * x + y * y <= 1.0: hits += 1 ;
    ;
    return 4.0 * hits / samples
;

let results = parallel [estimate_pi(1_000_000, seed), seed in 1..8]
show("π ≈ {sum(results) / 8}")

fun is_prime(n in Int) in Bool:
    if n < 2: return false ;
    for d in 2..isqrt(n):                   // pour n = 2 ou 3, 2..1 est vide
        if n mod d == 0: return false ;
    ;
    return true
;

let primes = parallel {p in 2..1_000_000, is_prime(p)}
let twins = {(p, p + 2), p in primes, p + 2 in primes}
show("Couples de premiers jumeaux : {twins.size}")
```

### 27.3 Application avec état partagé

`notes_data.lion` contient `Student` et `load` du programme 27.1, sans ses instructions. `ui` est imaginaire.

```lion
use ui
use notes_data

Status = {idle, loading, ready, failed}

struct AppState:
    students in List of notes_data.Student = []
    status in Status = idle
    message in Text = ""
;

var state = shared AppState()

fun average(l in List of notes_data.Student) in Float = sum([s.grade, s in l]) / l.size

fun on_loaded(result in List of notes_data.Student or Error) modifies state:
    match result:
        in Error e:
            state.status = failed
            state.message = e.message()
        ;
        in List of notes_data.Student l:
            state.students = l
            state.status = ready
        ;
    ;
;

fun start_loading() modifies state:
    state.status = loading
    let t = task notes_data.load("notes.csv")    // load ne touche pas à state : pas besoin de synced
    ui.when_done(t, on_loaded)
;

fun view() in ui.Element:
    let body = match state.status:
        idle then "Aucune donnée"
        loading then "Chargement…"
        ready then "Moyenne : {average(state.students)}"
        failed then "Erreur : {state.message}"
    ;
    return ui.column([
        ui.Title("Carnet de notes", 24),
        ui.Label(body),
        ui.Button("Charger", on_click: start_loading)
    ])
;

ui.run(view)
```

## 28. Feuille de route

L'interpréteur vient avant le compilateur, pour pouvoir tester le langage au plus vite ; l'interface graphique et les paquets viennent en dernier.

| Étape | Contenu | Terminée quand |
| --- | --- | --- |
| 1 | Spécification du noyau | ce document ne contient plus de point ouvert bloquant |
| 2 | Interpréteur : partie commune et machine virtuelle | les programmes 27.1 et 27.2 tournent |
| 3 | Bibliothèque standard de base : texte, fichiers, maths, ensembles, CSV | le programme 27.1 lit un vrai fichier |
| 4 | Outillage : formateur, messages d'erreur, tests | `lion fmt` et `lion test` fonctionnent |
| 5 | Compilateur natif | les deux modes donnent les mêmes résultats sur tous les tests |
| 6 | Parallélisme et tâches | le programme 27.2 utilise tous les cœurs |
| 7 | Bibliothèque graphique `ui` | le programme 27.3 tourne |
| 8 | Gestionnaire de paquets | on installe une bibliothèque en une commande |

## Annexe A. Registre des choix délégués

Ces 84 choix ont été tranchés par Claude selon la boussole (§2) ; chacun peut être annulé d'un mot.

| # | Choix | § |
| --- | --- | --- |
| D1 | Seul cas de résolution de méthode à l'exécution : les listes mélangées | 14.4 |
| D2 | Noms de surcharge : `plus`, `subtract`, `times`, `divide`, `equals`, `less` | 9.5 |
| D3 | Opérateurs utilisateur déclarés avec `infix` | 9.5 |
| D4 | Acronymes : `HttpClient` | 4.2 |
| D5 | Commentaires `/* */` et `///` | 4.3 |
| D6 | Valeurs par défaut des champs | 12.1 |
| D7 | Formes courtes et anonymes des fonctions | 11.1 |
| D8 | `nan in [nan]` vaut `false`, avec alerte | 16.2 |
| D9 | Objet temporairement invalide pendant une méthode `var self` | 12.3 |
| D10 | Public par défaut ; `private` limite au fichier | 20.3 |
| D11 | Extension `.lion` | 4.1 |
| D12 | `use shapes.circle` charge `shapes/circle.lion` | 20.2 |
| D13 | `Int or Error` ; `maybe T` = `T or None` | 7.3 |
| D14 | Conversion depuis un texte : `T or Error` | 8.5 |
| D15 | Message de bug : fichier, ligne, valeurs | 18.4 |
| D16 | `otherwise` ; `match` exhaustif | 10.3 |
| D17 | `Error` est un trait | 18.2 |
| D18 | Messages pédagogiques avec suggestion | 24.3 |
| D19 | Tests intégrés au langage | 24.1 |
| D20 | Formateur officiel unique, indentation de 4 espaces | 5.3, 24 |
| D21 | `compile expr` | 21.1 |
| D22 | Contenu de la bibliothèque standard | 23 |
| D23 | Texte Unicode, taille en caractères | 16.2 |
| D24 | Interpolation `{expr}` | 4.5 |
| D25 | Débogueur pas à pas en mode interprété | 24.2 |
| D26 | Mode interactif | 24 |
| D27 | Évaluation de gauche à droite ; `and`/`or` court-circuitent | 9.2 |
| D28 | `for`, `while`, `break`, `continue` | 10.2 |
| D29 | `show` et `ask` | 23 |
| D30 | Outil unique `lion` | 24 |
| D31 | Un module importé ne contient que des déclarations | 20.2 |
| D32 | `Color.red`, ou `red` seul si le type est connu | 13.1 |
| D33 | Énumération ordonnée avec `[...]` | 13.1 |
| D34 | `of` lie plus fort que `or` | 7.3 |
| D35 | `try` couvre toute l'expression à sa droite | 18.3 |
| D36 | `return` au niveau d'un script ; `exit(code)` | 20.1 |
| D37 | `r.get("col")` renvoie `Text or Error` | 23 |
| D38 | Pas d'indices négatifs ; `first` et `last` | 16.2 |
| D39 | Construction infaillible avec des valeurs en dur valides | 12.3 |
| D40 | Modification qui viole un invariant : bug | 12.3 |
| D41 | Extraits `l[2..4]`, bornes incluses | 16.2 |
| D42 | `"Lion"[1]` vaut `"L"` | 16.2 |
| D43 | Séparateur `1_000_000` | 4.5 |
| D44 | `+=`, `-=`, `*=` | 6.3 |
| D45 | `Bool = {true, false}` | 13.1 |
| D46 | Un `Range` n'est jamais construit en mémoire | 16.3 |
| D47 | `show` en parallèle : lignes jamais mélangées | 19.3 |
| D48 | Fonction étrangère impure par défaut ; `pure` | 19.3, 21.2 |
| D49 | Les mots-clés préfixes couvrent toute l'expression à droite | 9.1 |
| D50 | Modifier via une globale `shared` exige `modifies` | 11.5 |
| D51 | `match` expression avec `then` | 10.3 |
| D52 | Continuation de ligne dans `(`, `[` et `{` | 5.1 |
| D53 | Avertissement « synced inutile » | 19.3 |
| D54 | Rappels fournis par la bibliothèque ; `wait` ne bloque que sa tâche | 19.1 |
| D55 | `Type(...)` suit les règles d'appel des fonctions | 12.2 |
| D56 | Couple à un élément : `(x,)` | 4.5 |
| D57 | Syntaxe ASCII ; forme des identifiants | 4.1 |
| D58 | `/* */` non imbriqués | 4.3 |
| D59 | Littéraux : exposant, `0x`, `0b` | 4.5 |
| D60 | Échappements dans les textes | 4.5 |
| D61 | Table des priorités | 9.1 |
| D62 | `div` et `mod` euclidiens ; division entière par zéro : bug | 8.1 |
| D63 | Types de fonctions : `fun(A) in B` | 7.2 |
| D64 | Ordre des clauses d'une signature | 11.1 |
| D65 | `error(message)` | 18.2 |
| D66 | `message()` est une méthode : les exemples de l'interview écrivaient `.message` | 18.2 |
| D67 | N-uplet dans `of` : doubles parenthèses | 7.3 |
| D68 | `Task of T` | 7.2 |
| D69 | Comparaison enchaînée : chaque opérande évalué une fois | 9.3 |
| D70 | Champs finaux avec défaut omissibles à la construction | 12.2 |
| D71 | Une closure lit une copie des locales ; une fonction globale lit la valeur courante des globales | 11.5 |
| D72 | Message d'`expect` : attendu et obtenu | 24.1 |
| D73 | Puissance `^` | 8.4 |
| D74 | Float → Int par troncature ; NaN, infini ou hors limites : bug | 8.5 |
| D75 | Noms de surcharge `negate` et `power` | 9.5 |
| D76 | Motif `in ensemble` dans un `match` | 10.3 |
| D77 | Avertissement pour un cas de `match` inatteignable | 10.3 |
| D78 | Structures et traits génériques | 15.1 |
| D79 | `Set minus Domain` donne un Set ; `Domain minus Set` un Domain | 16.6 |
| D80 | `try` seulement là où une Error peut remonter, ou au niveau d'un script | 18.3 |
| D81 | Initialisation des globales de module ; imports mutuels | 20.2 |
| D82 | `compile` refuse les expressions à effets | 21.1 |
| D83 | Interopérabilité entre éditions | 25 |
| D84 | Noms importés toujours qualifiés : `geometry.area`, `notes_data.Student` | 20.2 |

## Annexe B. Idées rejetées et points ouverts

Les idées rejetées sont gardées ici pour ne pas être réintroduites par erreur ; les points ouverts sont à trancher avant ou pendant l'étape concernée.

### B.1 Idées rejetées

| Idée | Remplacée par |
| --- | --- |
| Syntaxe en phrases, verbeuse | mots-clés courts et symboles de base |
| Flux de gauche à droite, affectation inversée (`10 → x`) | `=` classique |
| Marqueurs de profondeur (`:`, `::`, `:::`) | blocs `:` … `;` |
| Fonction-fiche avec sections et contrats | invariants de structure |
| Colonnes de gardes | `match` |
| Sigils de rôle (`$`, `#`, `!`) | `var`, `let`, `modifies` |
| Symboles Unicode dans la syntaxe | ASCII et mots |
| Annotations de taille (`small`, `medium`) | Int fixe sur 64 bits |
| `give` et les fonctions à valeurs multiples | `return` |
| Mot-clé `is` | `in` |
| `step`, `1..`, `9..1` décroissant | filtre, `reverse` |
| Indices à partir de 0 | indices à partir de 1 |
| Héritage | traits structurels |
| Exceptions | erreurs-valeurs et `try` |
| Macros | `compile` |
| `async` et `await` | `task` et `wait` |
| Parallélisme automatique | `parallel` explicite |
| Modèle de permissions | tous les droits |

### B.2 Points ouverts

- [ ] API détaillée de la bibliothèque standard : fonctions de `text`, `files`, `dates`, et l'écriture littérale d'une `Map`.
- [ ] Conception de la bibliothèque graphique `ui`.
- [ ] Gestionnaire de paquets et fichier de projet : édition, dépendances.
- [ ] Décomposer une structure dans un motif de `match`, par exemple `in Circle(r)` ?
- [ ] Types entiers plus petits (octets) pour les fichiers binaires et la FFI ?
- [ ] Égalité de `0.0` et `-0.0` comme clés d'un `Set`.
- [ ] Provisoires à confirmer : `!=` ; pas de casse spéciale pour les constantes ; un trait peut exiger des champs.
