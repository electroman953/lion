# Notes d'implémentation

Ce document accompagne la [spécification](spec/lion-0.1.md), qui reste la source de vérité. Il consigne tout ce que l'implémentation a dû décider, pour que l'auteur du langage puisse relire, confirmer ou annuler chaque point.

Politique convenue : **ce que la spec ne définit pas est refusé** avec un diagnostic explicite, et consigné ici. Refuser n'invente aucune sémantique, et un refus pourra être assoupli plus tard sans casser de programme.

## 1. Décisions prises avec l'auteur (2026-09-26)

| Sujet | Décision |
| --- | --- |
| Langage d'implémentation | Rust (workspace Cargo, sans dépendance externe pour l'instant) |
| Langue des messages | Anglais, mise en page inspirée de rustc : titre, `-->` fichier:ligne:colonne, extrait, soulignement, notes |
| Affichage d'un Float | Représentation la plus courte qui redonne exactement la même valeur, toujours avec une partie décimale : `3.0`, `0.1`, `0.30000000000000004`. Valeurs spéciales : `NaN`, `Infinity`, `-Infinity` |
| Constructions non définies par la spec | Refusées et consignées (section 2) |

Détail technique choisi dans le cadre de l'affichage des Float : la notation exponentielle est utilisée en dessous de 1e-4 et à partir de 1e16, comme en Python. Elle est écrite comme un littéral Lion valide (`1.0e16`, `1.0e-5`), si bien que tout Float affiché peut être recopié dans un programme.

## 2. Constructions refusées car non définies par la spec 0.1

| # | Construction | Comportement actuel | Référence |
| --- | --- | --- | --- |
| R1 | `+`, `-`, `*`… entre Text (`"a" + "b"`) | Erreur de compilation, avec la suggestion d'utiliser l'interpolation `"{a}{b}"` | §9.5 ne dit pas si Text a `plus` |
| R2 | Comparer des valeurs de types différents (`1 == "1"`, `1 < true`) | Erreur de compilation. Seule exception : Int et Float se comparent, l'Int étant converti (« calcul mixte », §8.5) | §9.4 |
| R3 | Ordre sur Text (`"a" < "b"`) | Erreur de compilation | §9 ne définit pas d'ordre sur Text. Pour Bool, le refus découle de la spec : `Bool = {true, false}` est une énumération sans ordre (§13.1) |
| R4 | `div` et `mod` sur des Float (`7.5 div 2`) | Erreur de compilation | §8.1 les définit pour Int |
| R5 | `as` hors du tableau de conversions (`true as Int`, `true as Text`) | Erreur de compilation. `show` et l'interpolation acceptent toutes les valeurs | §8.5 |
| ~~R6~~ | ~~Déclarer un nom de la bibliothèque standard~~ | **Levé** par C2 : c'est désormais permis | — |
| R7 | `show` avec 0 ou plusieurs arguments, ou un argument nommé | Erreur de compilation | Les noms des paramètres de la bibliothèque standard ne sont pas spécifiés (annexe B.2) |
| R8 | Un texte littéral qui continue sur la ligne suivante | Erreur « unterminated text » ; il faut écrire `\n` | §4.5 donne l'échappement `\n` sans parler des retours à la ligne bruts |
| R9 | Un `}` non échappé dans un texte (`"a}b"`) | Erreur, avec la suggestion `\}` | §4.5 prévoit `\}` sans dire si le `}` seul est permis |
| R10 | Littéral entier au-delà de 2^63−1, y compris en hexadécimal (`0x8000000000000000`) | Erreur de compilation | §8.1. Conséquence : Int minimal ne s'écrit pas en littéral, car `-9223372036854775808` applique le moins unaire à un littéral trop grand. On écrit `-9223372036854775807 - 1` |
| R11 | Littéral flottant infini (`1.0e999`) | Erreur de compilation. Un littéral trop petit est arrondi (à 0.0 pour `1.0e-999`), comme tout littéral | §8.2 |
| R12 | Nombre collé à des lettres (`1e5`, `12abc`), préfixes en majuscules (`0XFF`) | Erreur, avec la suggestion `1.0e5` | Grammaire §26 : `float_lit` exige un point, `int_lit` écrit `0x` et `0b` |
| R13 | `return valeur` au niveau d'un script | Erreur de compilation ; seul `return` sans valeur termine le script | §20.1 ne donne pas de sens à une valeur renvoyée par un script |

## 3. Interprétations de la spec, à confirmer

La spec me paraît claire sur ces points, mais ils méritent un coup d'œil.

| # | Point | Lecture retenue |
| --- | --- | --- |
| I1 | `7 / 0` avec deux Int | Les Int sont convertis en Float, le résultat est `Infinity` avec une alerte, et ce n'est pas un bug. Raison : `/` « donne toujours un Float » (§8), et le bug « division entière par zéro » (§8.1, §18.1) vise `div` et `mod` |
| I2 | « Un `let` sans valeur reçoit exactement une affectation » (§6.1) | La règle est vérifiée chemin par chemin. Une affectation qui peut suivre une autre est une erreur, y compris dans une boucle. À la fin de son bloc, la constante doit avoir une valeur sur tous les chemins qui y arrivent. Une constante jamais affectée est une erreur |
| I3 | Ligne commençant par `-x` ou `not x` | Refusée : « une ligne qui commence par un opérateur est une erreur » (§5.1), même si `-x` serait une expression valide |
| I4 | Commentaire `/* */` contenant un retour à la ligne | Il termine l'instruction, comme la fin de ligne qu'il contient (même règle qu'en Go) |
| I5 | Alertes du mode interprété (§22.3) | Chaque emplacement du code n'alerte qu'une fois, sinon une boucle produirait des millions de messages. « Valeur flottante spéciale » : une opération sur des valeurs finies produit l'infini ou NaN. Propager une valeur déjà spéciale (`inf + 1`) n'alerte pas |
| I6 | Alerte de perte de précision Int → Float | Seulement si la conversion est inexacte : 2^60 se convertit exactement et n'alerte pas (« dépasse 2^53 **et** perd de la précision », §8.5) |
| I7 | `x as T` quand `x` est déjà de type T | Accepté, sans effet : `5 as Int` équivaut à l'annotation `5 in Int` (§6.1) |
| I8 | `let n = a and b in Bool` | `in Bool` annote toute l'expression : « si l'expression se termine par `in T` » (§26, règle 1) |
| I9 | `x as Int or ok` | `(x as Int) or ok` : dans un type, `or` ne continue que si un type suit. Avec les règles de casse, la lecture n'est pas ambiguë |
| I10 | `maybe maybe T` | Accepté par le parser, car §7.3 l'évoque, alors que la grammaire §26 n'autorise qu'un seul `maybe` |
| I11 | Placement du `;` qui ferme un bloc | La grammaire §26 est appliquée à la lettre (`line = [statement] NL`, `body = NL {line} \| statement`). Un bloc de plusieurs lignes se ferme par `;`, `elif` ou `else` en début de ligne ; `show(x) ;` à la fin de sa dernière ligne est refusé, avec une explication. Un bloc d'une ligne, `if x > 0: show(x) ;`, se ferme sur la même ligne |
| I12 | `if c then 1 else 2.5` | De type Float : la conversion Int → Float est « automatique partout » (§8.5), comme dans une collection `[1, 2.5]`. Des branches de types sans lien (Int et Text) donneraient une union : elles sont « not implemented » en attendant les unions, de même que `if c then 1` sans `else`, dont le type est `maybe Int` (§10.1) |
| I13 | Conditions constantes | Pour l'affectation définie, seul `while true` est reconnu : cette boucle ne se termine que par `break`. Aucune autre condition n'est évaluée à la compilation, pas même `if true` |
| I14 | Code inatteignable (après `return`, `break`, `continue`) | Accepté sans avertissement : la spec n'en parle pas. Il est vérifié (noms, types), mais sans erreur d'affectation définie, car aucun chemin n'y mène |
| I15 | Affectations dans une boucle | Au début de chaque tour, une variable affectée n'importe où dans le corps est considérée comme « peut-être affectée ». Conséquence : un `let` sans valeur ne peut pas être affecté dans une boucle, même suivi de `break`. Java applique la même règle ; elle est prudente et pourra être affinée |

## 4. Choix délégués pendant l'implémentation (C*)

Le 2026-09-26, l'auteur a délégué toutes les décisions « jusqu'à la fin du programme ». Chaque choix est pris selon la boussole du §2 et reste annulable d'un mot, comme les [Dn] de la spec.

| # | Choix | § |
| --- | --- | --- |
| C1 | Un paramètre sans type rend la fonction générique : chaque combinaison de types d'arguments crée une instance, vérifiée et compilée séparément, comme le prévoit le §15.3. Un paramètre omis prend le type de sa valeur par défaut, et un paramètre `var` celui de la variable donnée. Une erreur trouvée dans une instance désigne aussi l'appel qui l'a créée ; la même erreur trouvée dans plusieurs instances n'est signalée qu'une fois | 11.1, 15 |
| C2 | Une déclaration peut masquer une fonction standard (`var sum = 0`, `fun show(...)`). La bibliothèque standard n'est pas un bloc du programme, donc le §6.5 ne s'applique pas. Appeler une variable qui masque une fonction standard donne un message qui le signale | 6.5, 23 |
| C3 | Un appel fait depuis le script exige que toutes les globales que la fonction peut lire, directement ou via les fonctions qu'elle appelle, aient une valeur à ce point. C'est le même calcul transitif que les effets du §11.5 | 6.1, 11.5 |
| C4 | Les fonctions de premier niveau sont hissées : on peut les appeler avant leur déclaration. Leurs noms sont uniques (pas de surcharge) et distincts des noms de variables du script | 11 |
| C5 | Une fonction ne peut pas désigner une globale déclarée plusieurs fois au premier niveau (§6.3), car elle ne saurait pas laquelle | 6.3, 11.5 |
| C6 | Les paramètres appartiennent au bloc le plus extérieur du corps : `let x = ...` pour un paramètre `x` le redéclare (§6.3) et ce n'est pas un masquage | 6.3, 6.5 |
| C7 | Donner une globale à un paramètre `var` compte comme une modification directe : la fonction doit l'annoncer avec `modifies` | 11.2, 11.5 |
| C8 | L'argument d'un paramètre `var` doit avoir une valeur | 6.1, 11.2 |
| C9 | Un `let` global sans valeur reçoit sa valeur dans le script, pas dans une fonction | 6.1 |
| C10 | Une valeur par défaut est vérifiée dans une portée qui contient les globales et les paramètres précédents ; elle est évaluée à chaque appel qui omet l'argument. Un paramètre `var` n'a pas de valeur par défaut | 11.2 |
| C11 | Une fonction récursive dont le type de retour n'est pas écrit est refusée, avec l'aide « write the return type » | 11.4 |
| C12 | Un type de retour non écrit est inféré des `return` (ou de l'expression de la forme courte). Int et Float donnent Float ; aucune valeur donne None ; d'autres mélanges attendent les unions | 11.4 |
| C13 | Au plus 100 000 appels en cours ; au-delà, c'est un bug « too many nested calls ». La même limite vaut dans les deux modes | 18, 22.2 |
| C14 | Un argument nommé doit porter le nom du paramètre à sa position | 11.2 |
| C15 | Assigner une globale depuis une fonction sans `modifies` est une erreur. `modifies` ne peut nommer qu'une variable `var` du script. Nommer une variable jamais modifiée est accepté sans remarque | 11.5 |
| C16 | Pour l'affectation définie du script, une globale affectée par une fonction appelée ne compte pas comme affectée. C'est prudent ; on déclare la globale avec sa valeur | 6.1 |
| C17 | Quand un appel oblige à vérifier un corps de fonction avant que le script ait atteint la déclaration d'une globale que ce corps utilise, c'est une erreur. Elle désigne l'appel, qui serait de toute façon refusé par C3 | 6.1, 6.5 |
| C18 | `f()` sans argument alors que `f` a des paramètres obligatoires est une erreur. Un appel avec une partie des arguments est une curryfication (§11.3), « not implemented » jusqu'au sous-slice 3c | 11.3 |
| C19 | Les diagnostics sont présentés dans l'ordre du fichier, même si les corps de fonction sont vérifiés dans un autre ordre | 24.3 |
| C20 | Une fonction générique jamais appelée ne peut pas être vérifiée, faute de types : un avertissement le dit | 15.3, 24 |
| C21 | `show(1..5)` affiche `1..5`, la forme du littéral | 16.3 |
| C22 | La variable d'une boucle `for` est une constante du corps, redéclarée à chaque tour ; lui affecter une valeur est une erreur | 10.2 |
| C23 | `x in a..b` demande un Int ; un Float est refusé, comme les autres comparaisons entre types différents (R2) | 16.3 |
| C24 | `show` d'une collection l'écrit comme un littéral : `[1, 2.5]`, `["Léa", "Tom"]`. Les Text y sont entre guillemets, échappés | 16, 23 |
| C25 | `l.add(v)` s'écrit comme une instruction, sur une variable `var` ou l'un de ses éléments ; elle ne donne pas de valeur. Changer une liste temporaire serait perdu : c'est refusé | 6.4, 16 |
| C26 | `l.first` et `l.last` d'une liste vide sont un bug, comme un indice hors limites | 16.2, D38 |
| C27 | Un extrait avec un intervalle vide (`l[3..2]`) donne une liste vide ; un intervalle non vide doit être dans `1..l.size`, sinon c'est un bug | 16.2, D41 |
| C28 | Une liste vide prend le type attendu là où elle est écrite (annotation, variable affectée, paramètre, retour, `[] as List of Int`) ; ailleurs, son type est inconnu et c'est une erreur | 16 |
| C29 | Les éléments de types différents dans une liste attendent les unions ; Int et Float donnent une liste de Float (§8.5) | 7.3, 8.5 |
| C30 | Un Text ne se modifie pas caractère par caractère (`t[1] = "x"` est refusé) ; `t[i]`, `t[a..b]` et `t.size` comptent en caractères (D23, D42) | 16.2 |
| C31 | `sum` accepte une liste d'Int ou de Float, et un intervalle ; la somme vide vaut 0 ou 0.0 | 23 |
| C32 | `l[i] += v` évalue ses indices une seule fois | 9.2 |
| C33 | Une valeur d'union n'est pas étiquetée : son type se lit sur la sorte de la valeur à l'exécution. Tester un type de liste dans une union qui en contient plusieurs (`List of Int or List of Text`) est « not implemented » | 7.3 |
| C34 | `x in T` suivi d'un type est un test de type, même avec `of` (`r in List of Int`). Cela tranche O2 : la règle `comparison` du §26 s'étend aux types. Dans une déclaration, un `in T` final reste l'annotation (règle 1) | 7.1, 26 |
| C35 | Un test toujours faux (`5 in Text`) est une erreur de compilation | 7.1 |
| C36 | L'affinage (§7.4) vient de `x in T`, `x == none`, `x != none`, `not`, `and` (à droite de `and`, et dans la branche vraie) et `or` (à droite de `or`, et dans la branche fausse). Il s'applique aux variables locales et aux paramètres, pas aux globales lues par une fonction. Une affectation affine vers le type de la valeur ; une boucle oublie l'affinage des variables modifiées dans son corps ; un appel oublie celui des arguments `var` et, dans le script, des globales | 7.4 |
| C37 | Des valeurs de types différents (branches d'un `if`, éléments d'une liste, `return`) ont pour type leur union ; un Int y rejoint Float si les deux apparaissent | 7.3, 8.5 |
| C38 | `"12" as Int` et `as Float` ignorent les espaces autour ; ils acceptent un signe et, pour Float, la notation décimale avec exposant ; ils refusent `inf` et `nan`. Le message de l'Error dit pourquoi (D14) | 8.5 |
| C39 | `try` au niveau du script arrête le script sur une Error : le message est affiché avec l'endroit du `try`, et le code de sortie est 1 | 18.3 |
| C40 | Dans une fonction sans type de retour écrit, `try` ajoute `Error` au type de retour inféré | 11.4, 18.3 |
| C41 | Une Error s'affiche comme un littéral : `error("message")` | 18.2 |
| C43 | Un `match` ne s'écrit pas entre parenthèses ni entre crochets : chaque cas est sur sa ligne, et les fins de ligne y sont ignorées (§5.1). Le message propose de le nommer d'abord : `let v = match ...` | 10.3, 5.1 |
| C44 | Exhaustivité d'un `match` : chaque cas sans condition retire le type qu'il couvre (`in T`), ou la valeur `none`, `true` ou `false`. Sans `otherwise`, ce qui reste doit être vide ; sinon c'est une erreur qui nomme ce qui manque. Un cas placé après une couverture complète déclenche l'avertissement de D77 | 10.3 |
| C45 | Dans un cas `in T`, la variable examinée par le `match`, si c'en est une, est aussi affinée, même sans nom lié : `in Int: show(x + 1)` | 7.4, 10.3 |
| C46 | `private` s'écrit devant un champ, une fonction ou une globale. Hors du fichier qui les déclare, un champ privé ne se lit ni ne se change, une fonction ou une globale privées ne s'atteignent pas. Une structure à champs privés se construit quand même depuis un autre fichier, par position (`random.Generator(seed)`) : le constructeur est public (D10) | 12.1, 20.3 |
| C47 | Dans une méthode, `var self` est le premier paramètre ; `self` ne s'écrit pas autrement dans les paramètres, ni avec un type, ni avec une valeur par défaut. Une méthode s'écrit sur une structure ou sur `Int`, `Float`, `Bool`, `Text`, `None`, `Error` ; les méthodes des types génériques (`List`…) viendront avec le §15. Une méthode ne porte pas le nom d'un champ de sa structure, ni celui d'une propriété standard (`Text.size`, `Error.message`). Les méthodes d'opérateurs (`plus`, `equals`, `less`…, §9.5) sont refusées pour l'instant | 12.4, 9.5 |
| C48 | La valeur par défaut d'un champ est une constante : littéraux, listes, calculs sur des constantes, structures construites avec des constantes. Les valeurs par défaut et les conditions d'une structure ne lisent pas les variables du script ; les conditions peuvent appeler des fonctions, et `try` n'y a pas de sens | 12.1 |
| C49 | Une construction dont toutes les valeurs sont des constantes est vérifiée à la compilation (D39) : valide, elle donne un simple `T` ; invalide, c'est une erreur de compilation qui cite la condition. Le test `(...) in T` avec des constantes vaut simplement `true` ou `false`. Les formes `T(...)`, `(...) as T` et `let x = (...) in T` sont équivalentes (§12.2) : quand la vérification attend l'exécution, toutes donnent un `T or Error` | 12.2, 12.3 |
| C50 | Une partie d'une variable (`l[i]`, `s.field`) donnée à un paramètre `var`, ou objet d'une méthode `var self`, est copiée dans un temporaire, passée à la fonction, puis rangée à sa place au retour. Le résultat est celui d'une référence, sauf si la fonction lit la variable d'origine pendant l'appel. Une partie d'une constante est refusée, comme la constante elle-même | 11.2, 12.4 |
| C51 | Une structure s'affiche comme on la construit, avec les noms des champs : `Student(name: "Léa", grade: 14.0)` | 12, 23 |
| C52 | Invariants (§12.3, D9, D40). Une modification d'un champ, d'un élément ou d'une liste à l'intérieur d'une structure vérifie ensuite toutes les structures qui contiennent la partie modifiée, de la plus intérieure à la plus extérieure. Après un appel de méthode `var self`, l'objet est aussi vérifié. Seule exception : dans une méthode `var self`, les modifications de `self` et les appels `var self` sur `self` ne sont pas vérifiés ; c'est l'appel le plus extérieur qui vérifie. Une fonction qui reçoit l'objet par un paramètre `var` ordinaire est vérifiée à chaque modification. Le message d'Error et le bug citent la condition telle qu'écrite et les valeurs des champs qu'elle lit : `grade = 25.0 does not satisfy 0 <= grade <= 20` | 12.3 |
| C53 | Un n-uplet est une valeur de type `(A, B)` : il s'écrit, se compare, s'affiche et va dans les collections. La spec ne dit pas comment lire ses éléments : c'est refusé pour l'instant. Des noms dans un n-uplet (`(name: "Léa", ...)`) ne servent qu'à construire une structure | 4.5, 12.2, 16 |
| C54 | Une valeur d'énumération s'écrit seule (`red`) partout où le type attendu est connu : annotation `in Color`, argument d'un paramètre `Color`, valeur de retour déclarée `Color`, affectation d'une variable ou d'un champ de type `Color`, élément d'une liste attendue `List of Color`, cas d'un `match` sur une `Color`, opérande de `==`, `<`… à côté d'une `Color`, valeur testée par `in` dans une `List of Color`. Une variable du même nom reste prioritaire. Sinon, l'erreur propose `Color.red` | 13.1 |
| C55 | `for d in Days` parcourt les valeurs d'une énumération dans l'ordre de sa déclaration, qu'elle soit ordonnée ou non ; l'ordre d'une énumération `{...}` n'est pas garanti par la spec, celui-ci en est un cas permis. Seule une énumération `[...]` se compare avec `<`, `<=`, `>`, `>=` | 13.1 |
| C56 | Une union nommée est un autre nom de l'union qu'elle définit : les messages montrent ses membres (`Circle or Rect`). Une énumération ou une union se déclare au niveau du fichier, comme une structure, et son nom est distinct de ceux des autres types | 13.2 |
| C57 | Un `Set` garde ses éléments dans l'ordre de leur première insertion : c'est l'ordre de parcours et d'affichage (`{3, 1, 2}`). La spec ne garantit aucun ordre ; celui-ci donne la même sortie à chaque exécution. Deux Sets sont égaux s'ils ont les mêmes éléments, dans n'importe quel ordre. `s.add(x)` ajoute à un Set tenu par un `var`, comme pour une liste. `union`, `inter` et `minus` gardent l'ordre de l'opérande de gauche, puis celui de droite | 16.1, 16.6 |
| C58 | Une compréhension dont un générateur parcourt un type (`x in Int`) donne un `Domain` (§16.5), pas encore implémenté ; un générateur sur une énumération (`d in Days`) parcourt ses valeurs | 16.4, 16.5 |
| C59 | Noyau de la bibliothèque standard, dont la spec ne donne pas l'API (§23). Les paramètres n'ont pas de nom (R7).<br>• `isqrt(n)` : le plus grand `r` tel que `r * r <= n` ; `n` négatif est un bug.<br>• `floor(x)`, `ceil(x)`, `round(x)` : un Int ; `round` arrondit la moitié en s'éloignant de zéro (`round(2.5)` vaut 3) ; un Int est rendu tel quel ; NaN, un infini ou une valeur hors des Int sont un bug, comme `as Int`.<br>• `reverse(l)` : une List ou un Text à l'envers.<br>• `ask(prompt)` : écrit le texte suivi d'une espace, puis lit une ligne sans sa fin ; à la fin de l'entrée, la ligne est vide. Le texte est facultatif.<br>• `exit(code)` : termine le programme avec ce code, entre 0 et 255 (sinon bug), 0 par défaut ; rien ne s'exécute après un `exit` | 20.1, 23 |
| C60 | `parallel` s'applique à une compréhension (`parallel [...]`, `parallel {...}`) ou à une boucle (`parallel for`). Les règles de sûreté du §19.3 sont vérifiées : la partie parallèle n'affecte aucune variable déclarée hors d'elle, ne la donne pas à un paramètre `var`, et n'appelle aucune fonction qui modifie une globale, directement ou par les fonctions qu'elle appelle. Cette version calcule les parties parallèles l'une après l'autre : les résultats sont ceux du calcul séquentiel, dans l'ordre. L'usage de plusieurs cœurs est l'étape 6 de la feuille de route. `show` y reste permis (D47) | 19.2, 19.3 |
| C61 | Modules. `use a.b` charge d'abord le module `a.b` de la bibliothèque standard, sinon le fichier `a/b.lion` du dossier du script. Ses noms s'atteignent par la dernière partie du chemin : `b.f(...)`, `b.x`, `b.Type`, `b.Enum.value` (D84). Une variable ou une fonction du même nom masque le module. Les modules peuvent s'utiliser mutuellement. Une méthode est visible dans son fichier et dans ceux qui utilisent son module (§20.3). Un chemin de fichier donné aux fonctions de `files` et `csv` part du dossier courant | 20.2, 20.3 |
| C62 | Une globale d'un module reçoit sa valeur à sa déclaration, puisqu'un module n'exécute pas d'instructions (D31). Les valeurs des globales d'un module sont calculées dans l'ordre du fichier, juste avant le premier appel d'une de ses fonctions ou la première lecture d'une de ses globales depuis un autre fichier (D81). Un autre fichier lit les globales d'un module sans les changer : c'est le rôle des fonctions du module | 20.2 |
| C63 | La bibliothèque standard est écrite en Lion (`crates/lion_std/std`). Ce que seul le système peut faire y est déclaré `foreign "lion"`, forme réservée à la bibliothèque standard. Modules et fonctions :<br>• `files` : `read`, `write`, `exists`, `lines` ;<br>• `text` : `split`, `join`, `upper`, `lower`, `trim`, `contains`, `starts_with`, `ends_with`, `replace`, `find` (position à partir de 1, ou `none`), `lines` ;<br>• `math` : `pi`, `e`, `sqrt`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `exp`, `log`, `log10`, `abs`, `min`, `max`, `mean` ;<br>• `random` : `Generator(seed)` et ses méthodes `float()` et `int(low, high)` (SplitMix64, mêmes nombres partout), `random.float()` et `random.int(low, high)` sur un générateur du module initialisé à partir de l'heure ;<br>• `csv` : `read(path)` donne une `List of csv.Row or Error`, la première ligne nomme les colonnes, `r.get(col)` donne un `Text or Error` (D37) | 23 |
| C64 | Un bug ou une alerte qui survient dans le code de la bibliothèque standard est montré à l'appel du programme qui y a mené | 18.4, 22.3 |
| C65 | Fonctions comme valeurs (§11).<br>• Une fonction du fichier s'utilise comme valeur de type `fun(A, B) in R` ; une fonction générique prend les types attendus là où elle est écrite (`let f = id in fun(Int) in Int`), sinon c'est une erreur. Une fonction à paramètres `var` ne devient pas une valeur.<br>• Une fonction anonyme ou déclarée dans un corps est une closure : elle lit une copie des variables autour d'elle, prise à sa création ; celles qu'elle nomme après `modifies` sont partagées avec elle (§11.5). Elle n'a ni valeur par défaut, ni paramètre `var`, et ses paramètres ont un type, écrit ou donné par le type de fonction attendu (`apply(fun(n) = n + 1, 41)`). `let twice = fun(x) = x * 2`, sans type attendu, n'est donc pas encore accepté.<br>• Une fonction déclarée dans un corps voit son propre nom, pour s'appeler.<br>• Un appel avec une partie des arguments obligatoires donne une fonction qui attend les autres (§11.3) ; les arguments d'une valeur de fonction ne se nomment pas.<br>• Deux fonctions ne se comparent pas. Une valeur de fonction ne s'appelle pas dans une partie parallèle, car ses effets ne sont pas connus à cet endroit (§19.3).<br>• Une fonction s'affiche `<fun nom>` | 11, 7.2 |
| C42 | `Error` est pour l'instant un type prédéfini : les erreurs créées par `error(...)` et par la bibliothèque standard. Les types d'erreur définis par le programme (le trait du §18.2) viendront avec les structures et les traits | 18.2 |

## 5. Points de la spec à trancher plus tard (non bloquants aujourd'hui)

- ~~**O1. Globales lues par une fonction avant leur initialisation.**~~ Tranché par C3 et C17.
- ~~**O2. `x in List of Int` hors d'un `match`.**~~ Tranché par C34.
- **O3. Mode interactif.** La spec l'ouvre avec `lion` seul (§24) ; la demande d'implémentation mentionnait `lion repl`. L'implémentation suivra la spec.

## 6. Choix techniques (sans effet sur la sémantique)

- **Architecture.** Une chaîne de crates, chacune avec un rôle unique :
  - `lion_diagnostics` : sources, positions, diagnostics et leur rendu ;
  - `lion_syntax` : lexer, AST, parser ;
  - `lion_sema` : noms et types ;
  - `lion_ir` : l'IR typé ;
  - `lion_runtime` : la sémantique des opérations primitives ;
  - `lion_vm` : le bytecode et la machine virtuelle ;
  - `lion_cli` : la commande `lion`.
- **L'IR typé est le contrat entre le frontend et les backends.** Il n'existe que pour un programme valide : noms résolus, conversions explicites, opérateurs spécialisés par type. La machine virtuelle et le futur compilateur natif ne refont aucune analyse.
- **`lion_runtime` est la seule implémentation des opérations primitives** : arithmétique vérifiée, division euclidienne, puissance, conversions, affichage des Float. La VM l'appelle, et le compilateur natif y sera lié. C'est ce qui garantit les mêmes résultats et les mêmes bugs dans les deux modes (§22.2).
- **Machine virtuelle à registres.** Chaque variable a son registre. Les instructions sont typées (`AddInt`, `AddFloat`…), sans vérification de type à l'exécution. La sortie est tamponnée, et vidée avant chaque alerte et chaque bug pour garder l'ordre des messages.
- **Comparaisons enchaînées.** Elles sont traduites dès le frontend en `let` temporaires et en `and` : chaque opérande est évalué une fois, de gauche à droite, et l'évaluation s'arrête dès que le résultat est connu (§9.3).
- **Codes de sortie de `lion`** : 0 succès, 1 programme refusé ou illisible, 2 bug à l'exécution, 64 ligne de commande incorrecte, 70 erreur interne.
- **Erreurs internes.** Une panique de l'implémentation est présentée comme « internal compiler error », jamais comme une erreur du programme.
- **Récupération d'erreurs du parser.** Après une erreur, il reprend à l'instruction suivante, en sautant les blocs `:` … `;` de l'instruction fautive. Une indentation incohérente ne sert pas encore à localiser un `;` oublié (§5.3).
- **Détails du lexer.** Un BOM UTF-8 en tête de fichier est ignoré, et les fins de ligne CRLF sont acceptées.
- **Indentation et `;` oubliés (§5.3).** Chaque token porte sa ligne et l'indentation de sa ligne ; une tabulation compte jusqu'au multiple de 4 suivant. Le parser n'en tient jamais compte pour le sens du programme. Il s'en sert seulement quand un bloc reste ouvert à la fin du fichier :
  - s'il a vu un bloc fermé par un `;`, `elif` ou `else` moins indenté que le `if` ou le `while` qui l'a ouvert, c'est là que le `;` manque ;
  - sinon, il désigne la première ligne moins indentée que l'ouverture du bloc.
- **Affectation définie.** Elle est calculée pendant la vérification des types, dans un état de flux fusionné aux points de rencontre (`flow.rs`). Ce même mécanisme servira à l'affinage des unions (§7.4), qui dépend de `return`, `break` et `continue`.
- **Contrôle de flux dans l'IR.** Les chaînes `elif` sont imbriquées dans la branche `else`, et `break` ou `continue` visent la boucle la plus intérieure. `return` au niveau du script devient l'arrêt de la machine virtuelle.
- **Fonctions dans l'IR et la machine virtuelle.** Le programme est une liste de fonctions, dont le script. Les globales sont les registres du cadre du script, au bas de la pile ; les autres fonctions les lisent avec `LoadGlobal` et les écrivent avec `StoreGlobal`. Un paramètre `var` reçoit une référence vers un registre, celui d'un appelant ou d'une globale : elle pointe toujours vers le bas de la pile, donc elle reste valide. Les valeurs par défaut forment un prologue qui teste le nombre d'arguments reçus.
- **Ordre d'évaluation.** Dans `total + bump()`, si `bump` modifie `total`, l'opérande de gauche est d'abord copié, pour garder l'ordre de gauche à droite (§9.2).
- **Traces de bug.** Un bug dans une fonction affiche les appels en cours (au plus trois lieux distincts), pour qu'on voie d'où vient l'appel fautif.
- **Erreurs internes de la VM.** Une valeur du mauvais type dans un registre est un défaut de l'implémentation : la VM panique et la commande `lion` l'annonce comme « internal compiler error ».
- **Performance de la VM** (mesurée le 2026-09-26 face à CPython 3.13, meilleur de trois essais) :

  | Programme | Lion | Python |
  | --- | --- | --- |
  | boucle `while` de 10 millions de tours | 0,14 s | 0,70 s |
  | boucle `for` sur 10 millions d'entiers | 0,11 s | 0,52 s |
  | `fib(32)` récursif, environ 7 millions d'appels | 0,10 s | 0,15 s |
  | 2 millions d'éléments de liste, compréhension et somme | 0,08 s | 0,22 s |

  Ce qui a compté :
  - des opérandes immédiats pour les opérations sur Int (`n - 1`) ;
  - une comparaison et un saut fusionnés (`if n < 2` en une instruction) ;
  - `return if … then … else` compilé en deux `return` ;
  - un rapport de bug en boîte, pour des résultats petits sur le chemin sans erreur.

  Pour les appels :
  - le cadre de la fonction appelée commence sur ses arguments, sans copie, comme dans Lua. C'est sûr parce que le compilateur réserve les arguments dans ses derniers registres ;
  - les registres ne sont pas remis à zéro à l'appel, puisque le compilateur écrit chaque registre avant de le lire ;
  - au retour, seuls les registres qui tiennent de la mémoire (Text, List…) sont libérés (§17.3).

  Le mode compilé (étape 5) reste le levier principal de vitesse.
- **Types composés.** `Type` reste une petite valeur copiable : les types qui en contiennent d'autres, comme `List of T`, désignent ces derniers par une référence vers une table globale, où chaque type n'est stocké qu'une fois. L'égalité des types est ainsi une simple comparaison.
- **Intervalles.** Un `Range` ne stocke que ses deux bornes (D46). La boucle `for` sur un intervalle compare le compteur à la borne avant de l'augmenter, si bien que `for i in 1..9223372036854775807` se termine sans débordement.
- **Listes.** Une liste est partagée tant que personne ne la modifie ; la première modification d'une liste partagée la copie. Cette copie à l'écriture donne la sémantique de valeur du §17.1 sans copier les grandes listes qu'on ne fait que lire. Les modifications en place (`l[i] = v`, `l.add(v)`) descendent dans les listes imbriquées depuis une variable locale, une globale ou un paramètre `var`.
- **Structures dans l'IR et la machine virtuelle.** Une valeur de structure est un vecteur de champs partagé avec copie à l'écriture, comme une liste, qui porte sa description (nom, noms des champs) pour l'affichage. Les champs sont désignés par leur position. Une structure avec des conditions reçoit deux fonctions produites par le vérificateur : `T.check`, qui renvoie `none` ou le texte de la condition fausse, et le constructeur `T`, qui renvoie la valeur ou une `Error`. Le frontend évalue les conditions sur des constantes avec les opérations de `lion_runtime`, comme à l'exécution. Un test de type distingue deux structures par leur description quand l'union en contient plusieurs.
- **Énumérations.** Une valeur d'énumération est sa position, avec une référence vers la description de l'énumération pour l'affichage. Les comparaisons d'ordre comparent les positions comme des Int. Les structures et les énumérations d'un programme ont chacune un numéro de type, qui sert aux tests de type quand une union en contient plusieurs.
- **Closures.** Une valeur de fonction est la fonction et les valeurs qu'elle a capturées, rangées après ses paramètres à chaque appel ; une application partielle y ajoute les premiers arguments. Une variable qu'une fonction imbriquée modifie vit dans une cellule partagée, créée à chaque exécution de sa déclaration : une closure créée à chaque tour de boucle a donc sa propre variable.
- **Ensembles.** Un `Set` est une liste dans l'ordre d'insertion plus une table de hachage de ses éléments. Le hachage suit `==` : deux valeurs égales ont le même hachage (0.0 et -0.0 compris), et l'ordre des éléments d'un Set imbriqué ne compte pas. NaN n'y entre jamais, ce qui fait de `==` une vraie relation d'équivalence sur les clés.
- **Compréhensions.** Une compréhension devient, dans l'IR, un bloc qui remplit une liste avec des boucles `for` imbriquées et des `if`, dans l'ordre des générateurs et des conditions (§16.4).

