<div class="hero">
<img src="lion.svg" alt="" width="96" height="96">
<h1>Lion</h1>
<p class="accroche">Un langage polyvalent, interprété ou compilé, qui s'écrit comme des mathématiques.</p>
<p class="actions"><a class="bouton" href="guide.html">Commencer le guide</a> <a class="bouton second" href="modules.html">Bibliothèque standard</a></p>
</div>

```lion
struct Eleve:
    nom in Text
    note in Float, 0 <= note <= 20        // invariant vérifié
;

fun Eleve.admis() in Bool = self.note >= 10

let classe = [Eleve("Léa", 14), Eleve("Tom", 8)]
let admis = {e.nom, e in classe, e.admis()}
show("Admis : {admis}")

match ask("Une note ? ") as Float:
    in Error e: show("Pas un nombre : {e.message()}") ;
    in Float n, 0 <= n <= 20: show("Note valide") ;
    otherwise: show("Hors barème") ;
;
```

<div class="cartes">
<div class="carte"><h3>Clair</h3><p>Ensembles en compréhension, comparaisons enchaînées <code>0 &lt;= x &lt;= 20</code>, fractions exactes, indices à partir de 1. Le code se lit comme une copie de maths.</p></div>
<div class="carte"><h3>Fiable</h3><p>Types vérifiés à la compilation, invariants sur les structures, erreurs visibles dans les types (<code>Int or Error</code>) et messages qui proposent la correction.</p></div>
<div class="carte"><h3>Rapide</h3><p><code>lion run</code> pour essayer tout de suite, <code>lion build</code> pour un exécutable natif 3 à 13 fois plus rapide, et <code>parallel</code> pour utiliser tous les cœurs.</p></div>
<div class="carte"><h3>Complet</h3><p>Fichiers, JSON, CSV, requêtes web, serveurs HTTP, expressions régulières, dates, fenêtres graphiques : la bibliothèque standard est écrite en Lion.</p></div>
</div>

## Installer

```sh
cargo install --git https://github.com/electroman953/lion lion_cli
lion new mon-projet && cd mon-projet && lion run
```

Il faut Rust 1.88 ou plus récent (<https://rustup.rs>). La suite est dans le [guide](guide.html#installer-lion).

## Par où commencer

- **Vous débutez :** le [guide](guide.html) présente le langage dans l'ordre, avec un exemple complet par notion.
- **Vous cherchez une fonction :** la [bibliothèque standard](modules.html), ou la recherche en haut de la page.
- **Vous voulez la règle exacte :** la [référence](reference.html), la spécification complète de Lion 0.1.
