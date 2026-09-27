# Proposition : la bibliothèque `ui` (étape 7)

Statut : **proposition, à valider par l'auteur**. Rien n'est implémenté. La conception de `ui` est un point ouvert de la spec (§B.2), et l'étape 7 est terminée « quand le programme 27.3 tourne » (§28).

## 1. Ce que l'essai de 27.3 a montré

Le programme 27.3 a été vérifié et exécuté tel quel par l'implémentation actuelle (2026-09-27), avec un module `ui` factice écrit en Lion à côté du script :

```lion
trait Element:
    fun render() in Text
;
struct Button:
    text in Text
    on_click in fun()
;
fun column(children in List of Element) in Column = Column(children)
fun when_done(t in Task of T, f in fun(T)), T in Type:
    f(wait t)
;
fun run(view in fun() in Element):
    show(view().render())
;
```

Le langage fournit donc déjà tout ce que l'API demande :
- un type commun à tous les éléments ;
- des structures dont un champ est une fonction (`on_click in fun()`) ;
- une fonction générique sur les tâches (`Task of T`) ;
- l'état dans un `var shared` global, modifié par des fonctions qui l'annoncent avec `modifies`.

**L'étape 7 ne demande aucune nouvelle syntaxe.** Il reste à choisir trois choses :
1. le modèle d'exécution, c'est-à-dire la boucle d'événements ;
2. le backend d'affichage ;
3. la liste des éléments.

Seul le backend est vraiment bloquant.

## 2. Modèle d'exécution proposé : la vue est une fonction de l'état

C'est la lecture la plus directe de 27.3, et le modèle d'Elm ou de React.

- `ui.run(view)` ouvre une fenêtre, appelle `view()` et affiche l'arbre d'éléments obtenu.
- Un événement, par exemple un clic sur un `Button`, appelle la fonction de l'élément (`on_click`). Après chaque fonction de rappel, `ui` rappelle `view()` et redessine. L'état vit dans les variables du programme (`var state = shared AppState()`) ; `ui` ne garde rien.
- `ui.when_done(t, f)` rend la main tout de suite. Quand la tâche `t` est finie, la boucle appelle `f` avec son résultat, puis redessine. On n'écrit jamais `wait` dans un rappel : l'interface ne gèle pas (D54).
- Tous les rappels s'exécutent **sur le fil de l'interface, l'un après l'autre**. C'est ce qui rend juste le commentaire de 27.3 : « load ne touche pas à state : pas besoin de synced ».
- `ui.run` rend la main quand la fenêtre est fermée, ou après `ui.quit()`.
- Un bug dans `view()` ou dans un rappel arrête le programme comme partout ailleurs, avec la trace des appels.

## 3. API minimale proposée (v1)

`Element` est une **union nommée fermée** (§13.2) des éléments de `ui`. Un élément « maison » est une fonction qui compose les éléments existants : `fun card(t in Text) in ui.Element = ui.column([...])`. Un trait ouvert demanderait de spécifier une API de dessin ; on pourra l'ouvrir plus tard sans casser de programme.

| Élément | Construction | Rôle |
| --- | --- | --- |
| `Title` | `ui.Title(text, size = 20)` | texte en gros |
| `Label` | `ui.Label(text)` | texte |
| `Button` | `ui.Button(text, on_click: f)` | appelle `f()` au clic |
| `Input` | `ui.Input(value, on_change: f)` | champ de saisie ; appelle `f(nouveau texte)`, l'état garde le texte |
| `Checkbox` | `ui.Checkbox(label, checked, on_toggle: f)` | appelle `f(nouvelle valeur)` |
| `Column`, `Row` | `ui.column([...])`, `ui.row([...])` | empilent leurs enfants |

Les fonctions du module :
- `ui.run(view in fun() in Element)` ;
- `ui.when_done(t in Task of T, f in fun(T)), T in Type` ;
- `ui.quit()`.

Ce que l'API laisse de côté pour la v1 : les styles, les couleurs, les images, le défilement et les listes longues. On les ajoutera quand un programme en aura besoin.

## 4. Backend : le choix bloquant

Contraintes à respecter :
- aucune dépendance Rust externe jusqu'ici ;
- la spec : « elle utilisera la FFI pour parler au système (fenêtres, clavier, carte graphique) » (§23) ;
- « deux modes, une sémantique » : le mode interprété et le mode compilé doivent partager le même code d'interface. Il vivrait dans `lion_vm`, comme `natives.rs`, ou dans une crate `lion_ui` partagée.

| Option | Principe | Pour | Contre |
| --- | --- | --- | --- |
| **A. Fenêtre native X11** (recommandée) | Le runtime parle directement le protocole X11 sur son socket, sans bibliothèque C. Le dessin est logiciel (rectangles, texte avec la police `fixed` intégrée au serveur X), puis envoyé à la fenêtre | Vraie fenêtre, aucune dépendance, marche en mode compilé. Sous Wayland, passe par XWayland | Linux et BSD d'abord ; Windows et macOS à écrire à part. Texte sans anticrénelage, en Latin-1 d'abord (`…` remplacé par `...`). Environ 2 à 3 tranches |
| B. Terminal | L'interface est dessinée dans le terminal, au clavier et à la souris | Simple, portable sous Unix, facile à tester | Ce n'est pas une interface graphique au sens de la spec |
| C. Navigateur | `ui.run` lance un petit serveur HTTP local et ouvre la page | Vraies polices, tout Unicode, portable | Pas une fenêtre native ; dépend d'un navigateur |
| D. FFI généralisée | `ui` écrit en Lion au-dessus de libX11 ou Wayland, via `foreign` | La vision du §23 | Demande des décisions de langage : pointeurs, tampons d'octets (§B.2 : « types entiers plus petits »), rappels C |

Dans tous les cas, s'y ajoute un **backend sans écran**, pour les tests. Il est choisi par une variable d'environnement et lit une liste d'événements (`click "Charger"`, `type "abc"`, `wait`). Après chaque événement, il écrit l'arbre d'éléments en texte. Les programmes `ui` deviennent ainsi des tests golden, vérifiés dans les deux modes.

## 5. Questions à l'auteur

1. **Backend** : A, B, C ou D ? Recommandation : A, avec le backend sans écran pour les tests. D reste la direction de long terme, quand la FFI aura les octets et les pointeurs.
2. **Modèle** : vue redessinée après chaque rappel, et rappels sur le fil de l'interface (§2) ?
3. **`Element`** : une union fermée, plutôt qu'un trait ouvert (§3) ?
4. **Éléments de la v1** : la liste du §3 suffit-elle ?
