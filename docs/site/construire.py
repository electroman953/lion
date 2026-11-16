#!/usr/bin/env python3
"""Construit le site de documentation de Lion (lion.mbny.fr) dans docs/site/_site/.

Pages : accueil (accueil.md), guide (guide.md), référence (docs/spec/lion-0.1.md),
bibliothèque standard (tirée des commentaires de crates/lion_std/std/*.lion).
Usage : python3 docs/site/construire.py   (il faut le paquet Python `markdown`)
"""
import html
import json
import re
import shutil
import unicodedata
from pathlib import Path

import markdown

ICI = Path(__file__).resolve().parent
DEPOT = ICI.parent.parent
SORTIE = ICI / "_site"
STD = DEPOT / "crates/lion_std/std"

# Résumé en français de chaque module (les commentaires du code sont en anglais).
RESUMES = {
    "text": "Chercher, découper, remplacer, changer la casse, mettre en forme les nombres.",
    "math": "Racines, puissances, trigonométrie, arrondis, constantes.",
    "random": "Nombres au hasard, tirages, mélanges ; générateurs reproductibles.",
    "files": "Lire, écrire, ajouter, lister, copier, déplacer et supprimer des fichiers.",
    "csv": "Lire et écrire des tableaux CSV.",
    "json": "Lire et écrire du JSON de toute forme, ou des listes d'objets.",
    "sets": "Trier, minimum, maximum, regrouper, compter, ensemble des parties.",
    "dates": "Dates vérifiées, calculs sur les jours, jour de la semaine, format ISO.",
    "time": "Horloge, mesure des durées, attente.",
    "net": "Requêtes web (HTTPS), sessions avec cookies, petits serveurs HTTP et routeur.",
    "html": "Lire une page HTML avec des sélecteurs CSS.",
    "regex": "Expressions régulières : chercher, extraire, remplacer.",
    "url": "Découper, construire et combiner des adresses.",
    "base64": "Encoder et décoder en Base64.",
    "hash": "Empreintes (SHA-256…) de textes et de fichiers.",
    "system": "Arguments du programme, variables d'environnement, lancer d'autres programmes.",
    "term": "Couleurs et styles dans le terminal.",
    "ui": "Fenêtres graphiques : textes, boutons, champs, cases à cocher, mises en page.",
    "bytes": "Données binaires (Bytes) : octets, conversions, hexadécimal.",
}
ORDRE = ["text", "math", "random", "sets", "files", "csv", "json", "dates", "time", "net",
         "html", "url", "regex", "base64", "hash", "bytes", "system", "term", "ui"]

NOYAU = [
    ("show(x)", "Affiche une valeur, suivie d'un retour à la ligne."),
    ("ask(question in Text) in Text", "Affiche la question et renvoie la ligne tapée."),
    ("exit(code in Int)", "Termine le programme avec ce code de sortie."),
    ("error(message in Text) in Error", "Construit une erreur, que l'on renvoie avec `return`."),
    ("sum(l)", "La somme des éléments d'une liste ou d'un ensemble."),
    ("reverse(l)", "La liste ou le texte à l'envers."),
    ("floor(x in Float) in Int", "Arrondi vers le bas."),
    ("ceil(x in Float) in Int", "Arrondi vers le haut."),
    ("round(x in Float) in Int", "Arrondi au plus proche."),
    ("isqrt(n in Int) in Int", "Racine carrée entière."),
]

PAGES = [
    ("index.html", "Accueil"),
    ("guide.html", "Guide"),
    ("reference.html", "Référence"),
    ("modules.html", "Bibliothèque"),
]


def slug(texte, sep="-"):
    t = unicodedata.normalize("NFKD", re.sub(r"<[^>]+>", "", texte).lower())
    t = "".join(c for c in t if not unicodedata.combining(c))
    t = re.sub(r"[^\w\s-]", "", t, flags=re.UNICODE)
    return re.sub(r"[\s_]+", sep, t).strip(sep)


def rendre_md(source):
    md = markdown.Markdown(extensions=["tables", "fenced_code", "toc"],
                           extension_configs={"toc": {"slugify": slug, "toc_depth": "2-3"}})
    corps = md.convert(source)
    return corps, md.toc_tokens


def table_des_matieres(jetons):
    """La liste des titres de niveau 2 (et 3) pour la colonne de gauche."""
    lignes = ["<ul>"]
    for t in jetons:
        for h in ([t] if t["level"] == 2 else t["children"]):
            lignes.append(f'<li><a href="#{h["id"]}">{h["name"]}</a>')
            enfants = [c for c in h["children"] if c["level"] == 3]
            if enfants:
                lignes.append("<ul>")
                lignes += [f'<li><a href="#{c["id"]}">{c["name"]}</a></li>' for c in enfants]
                lignes.append("</ul>")
            lignes.append("</li>")
    lignes.append("</ul>")
    return "\n".join(lignes)


def titres(jetons, page):
    """Les titres d'une page pour la recherche."""
    out = []

    def visiter(t):
        if t["level"] in (2, 3):
            out.append({"t": html.unescape(re.sub(r"<[^>]+>", "", t["name"])), "u": f"{page}#{t['id']}",
                        "p": dict(PAGES)[page]})
        for c in t["children"]:
            visiter(c)
    for t in jetons:
        visiter(t)
    return out


# ------------------------------------------------------------------ modules standard

def lire_module(nom):
    """Lit std/<nom>.lion : la description du module et ses déclarations publiques,
    chacune avec son commentaire, rangées par sections (`// ---- titre`)."""
    lignes = (STD / f"{nom}.lion").read_text(encoding="utf-8").splitlines()
    i = 0
    entete = []
    while i < len(lignes) and lignes[i].startswith("//"):
        entete.append(lignes[i][2:].strip())
        i += 1
    sections = [("", [])]
    commentaire = []
    while i < len(lignes):
        l = lignes[i]
        m = re.match(r"^// -{5,}\s*(.*)$", l)
        if m:
            titre = m.group(1).strip()
            if titre == "inside":
                break
            sections.append((titre, []))
            commentaire = []
            i += 1
            continue
        if l.startswith("//"):
            commentaire.append(l[2:].strip())
            i += 1
            continue
        if not l.strip():
            commentaire = []
            i += 1
            continue
        public = not l.startswith("private ") and not l.startswith("use ")
        if re.match(r"^(struct|trait) ", l):
            bloc = [l]
            i += 1
            garde_com = []
            while i < len(lignes) and lignes[i].strip() != ";":
                s = lignes[i].strip()
                if s.startswith("//"):
                    garde_com.append(lignes[i])
                elif s.startswith("private ") or not s:
                    garde_com = []
                else:
                    bloc += garde_com + [lignes[i]]
                    garde_com = []
                i += 1
            bloc.append(";")
            if public:
                nom_decl = l.split()[1].rstrip(":")
                sections[-1][1].append((nom_decl, "\n".join(bloc), commentaire))
            commentaire = []
            i += 1
            continue
        m = re.match(r'^(?:foreign "[^"]+" )?fun (.*)$', l)
        if m and public:
            sig = signature("fun " + m.group(1))
            nom_decl = re.match(r"fun ([\w.]+)", sig).group(1)
            sections[-1][1].append((nom_decl, sig, commentaire))
        elif re.match(r"^[A-Z]\w* = ", l) and public:
            sections[-1][1].append((l.split()[0], l, commentaire))
        commentaire = []
        # sauter le corps d'une fonction en bloc
        if l.rstrip().endswith(":") and not re.match(r"^[A-Z]", l):
            prof = 1
            i += 1
            while i < len(lignes) and prof > 0:
                if lignes[i].strip() == ";" and not lignes[i].startswith(" "):
                    prof = 0
                i += 1
            continue
        i += 1
    return " ".join(entete), [s for s in sections if s[1]]


def signature(ligne):
    """`fun f(a in Int, b = 1) in Int = a + b` donne `fun f(a in Int, b = 1) in Int`."""
    if ligne.rstrip().endswith(":"):
        return ligne.rstrip()[:-1].rstrip()
    prof = 0
    for j, c in enumerate(ligne):
        if c == "(":
            prof += 1
        elif c == ")":
            prof -= 1
        elif prof == 0 and ligne[j:j + 3] == " = ":
            return ligne[:j]
    return ligne


def code_inline(texte):
    """Un commentaire du code source en HTML : les `x` deviennent du code."""
    t = html.escape(texte)
    return re.sub(r"`([^`]+)`", r"<code>\1</code>", t)


def page_modules():
    recherche = []
    blocs = []
    toc = ["<ul>", '<li><a href="#noyau">Noyau (sans <code>use</code>)</a></li>']
    blocs.append('<section class="module" id="noyau"><h2>Noyau <small>sans <code>use</code></small></h2>')
    blocs.append("<p>Ces fonctions sont toujours disponibles.</p><div class=\"decls\">")
    for sig, desc in NOYAU:
        nom = sig.split("(")[0]
        ident = f"noyau-{nom}"
        blocs.append(f'<div class="decl" id="{ident}"><pre><code class="language-lion">fun {html.escape(sig)}</code></pre>'
                     f"<p>{code_inline(desc)}</p></div>")
        recherche.append({"t": nom, "u": f"modules.html#{ident}", "p": "Noyau"})
    blocs.append("</div></section>")
    for nom in ORDRE:
        entete, sections = lire_module(nom)
        toc.append(f'<li><a href="#{nom}"><code>{nom}</code></a></li>')
        recherche.append({"t": f"module {nom}", "u": f"modules.html#{nom}", "p": "Bibliothèque"})
        blocs.append(f'<section class="module" id="{nom}"><h2><code>{nom}</code></h2>')
        blocs.append(f'<p class="resume">{html.escape(RESUMES.get(nom, ""))}</p>')
        blocs.append(f'<pre><code class="language-lion">use {nom}</code></pre>')
        if entete:
            blocs.append(f'<p class="source">{code_inline(entete)}</p>')
        for titre, decls in sections:
            if titre:
                blocs.append(f"<h3>{html.escape(titre.capitalize())}</h3>")
            blocs.append('<div class="decls">')
            for nom_decl, code, com in decls:
                ident = f"{nom}-{slug(nom_decl, '-')}"
                blocs.append(f'<div class="decl" id="{ident}"><pre><code class="language-lion">{html.escape(code)}</code></pre>')
                if com:
                    blocs.append(f"<p>{code_inline(' '.join(com))}</p>")
                blocs.append("</div>")
                recherche.append({"t": f"{nom}.{nom_decl}", "u": f"modules.html#{ident}", "p": "Bibliothèque"})
            blocs.append("</div>")
        blocs.append("</section>")
    toc.append("</ul>")
    intro = ("<h1>Bibliothèque standard</h1>"
             "<p>Chaque module s'importe avec <code>use nom</code>, puis ses fonctions s'appellent avec leur préfixe : "
             "<code>text.upper(t)</code>. Les signatures et les descriptions viennent directement du code source "
             "des modules (<code>crates/lion_std/std</code>), écrit en Lion ; les descriptions détaillées sont en anglais, "
             "comme dans le code.</p>"
             '<input class="filtre" type="search" placeholder="Filtrer les fonctions (ex. split, json, get)…" aria-label="Filtrer les fonctions">')
    return intro + "\n".join(blocs), "\n".join(toc), recherche


# ------------------------------------------------------------------ gabarit

def gabarit(fichier, titre, corps, toc):
    courant = ' aria-current="page"'
    nav = "".join(f'<a href="{f}"{courant if f == fichier else ""}>{t}</a>' for f, t in PAGES)
    cote = f'<nav class="toc" aria-label="Sommaire">{toc}</nav>' if toc else ""
    return f"""<!doctype html>
<html lang="fr">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{titre} · Lion</title>
<meta name="description" content="Documentation du langage Lion : guide, référence et bibliothèque standard.">
<link rel="icon" href="lion.svg" type="image/svg+xml">
<link rel="stylesheet" href="style.css?v={VERSION}">
</head>
<body class="{'accueil' if fichier == 'index.html' else ''}">
<header class="haut">
  <a class="marque" href="index.html"><img src="lion.svg" alt="" width="28" height="28"> Lion</a>
  <nav class="pages">{nav}</nav>
  <div class="recherche">
    <input id="q" type="search" placeholder="Rechercher…" aria-label="Rechercher dans la documentation" autocomplete="off">
    <ul id="resultats" hidden></ul>
  </div>
  <button class="menu" aria-label="Sommaire" aria-expanded="false">☰</button>
</header>
<div class="cadre{' sans-toc' if not toc else ''}">
{cote}
<main>
{corps}
<footer>Lion 0.1 · <a href="https://github.com/electroman953/lion">code source</a></footer>
</main>
</div>
<script src="lion.js?v={VERSION}"></script>
</body>
</html>
"""


VERSION = "1"


def construire():
    global VERSION
    if SORTIE.exists():
        shutil.rmtree(SORTIE)
    SORTIE.mkdir()
    recherche = []

    corps, jetons = rendre_md((ICI / "accueil.md").read_text(encoding="utf-8"))
    pages = {"index.html": ("Accueil", corps, "")}

    corps, jetons = rendre_md((ICI / "guide.md").read_text(encoding="utf-8"))
    pages["guide.html"] = ("Guide", corps, table_des_matieres(jetons))
    recherche += titres(jetons, "guide.html")

    spec = (DEPOT / "docs/spec/lion-0.1.md").read_text(encoding="utf-8")
    spec = re.sub(r"^# .*\n", "# Référence du langage\n", spec, count=1)
    spec = re.sub(r"\]\((?!https?:|#)[^)]*\)", "](https://github.com/electroman953/lion)", spec)
    corps, jetons = rendre_md(spec)
    corps = corps.replace("<h1", '<p class="note">Cette page est la spécification complète de Lion 0.1 : '
                          'chaque règle du langage y est décrite. Pour apprendre, commencez par le '
                          '<a href="guide.html">guide</a>.</p><h1', 1)
    # le paragraphe de note doit venir après le titre
    corps = re.sub(r'(<p class="note">.*?</p>)(<h1[^>]*>.*?</h1>)', r"\2\1", corps, count=1, flags=re.S)
    pages["reference.html"] = ("Référence", corps, table_des_matieres(jetons))
    recherche += titres(jetons, "reference.html")

    corps, toc, rech = page_modules()
    pages["modules.html"] = ("Bibliothèque standard", corps, toc)
    recherche += rech

    import hashlib
    VERSION = hashlib.sha1(((ICI / "style.css").read_bytes() + (ICI / "lion.js").read_bytes())).hexdigest()[:8]
    for fichier, (titre, corps, toc) in pages.items():
        (SORTIE / fichier).write_text(gabarit(fichier, titre, corps, toc), encoding="utf-8")
    (SORTIE / "recherche.json").write_text(json.dumps(recherche, ensure_ascii=False), encoding="utf-8")
    for f in ("style.css", "lion.js", "lion.svg"):
        shutil.copy(ICI / f, SORTIE / f)
    print(f"{len(pages)} pages, {len(recherche)} entrées de recherche dans {SORTIE}")


if __name__ == "__main__":
    construire()
