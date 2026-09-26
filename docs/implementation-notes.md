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
| R6 | Déclarer un nom de la bibliothèque standard (`let show = 1`, `var sum = 0`) | Erreur de compilation | §6.5 ne dit pas si la bibliothèque standard forme une portée extérieure. **À trancher bientôt** : `sum` est un nom de variable courant |
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

## 4. Points de la spec à trancher plus tard (non bloquants aujourd'hui)

- **O1. Globales lues par une fonction avant leur initialisation.** §6.1 exige de détecter à la compilation toute lecture avant affectation. Mais §6.5 permet à une fonction de lire une globale déclarée plus bas, et rien n'empêche d'appeler cette fonction avant la déclaration. Il faudra soit une analyse entre fonctions, soit une règle. Le point deviendra concret avec les fonctions.
- **O2. `x in List of Int` hors d'un `match`.** La règle `comparison` de §26 n'accepte qu'une `as_expr` à droite de `in`, donc pas un type avec `of`, alors que les motifs de `match` l'acceptent. Le point deviendra concret avec les unions.
- **O3. Mode interactif.** La spec l'ouvre avec `lion` seul (§24) ; la demande d'implémentation mentionnait `lion repl`. L'implémentation suivra la spec.

## 5. Choix techniques (sans effet sur la sémantique)

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
