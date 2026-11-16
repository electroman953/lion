// Documentation de Lion : coloration du code, recherche, sommaire, filtre des modules.
(function () {
  "use strict";

  // ---------------------------------------------------------------- coloration
  const MOTS = new Set(("if elif else then match otherwise for while break continue return try task wait parallel expect " +
    "let var fun struct trait use private shared synced modifies infix foreign pure unsafe compile test " +
    "and or not in as same union inter minus subset div mod over of maybe true false none self").split(" "));
  const APPELS = /^(show|ask|error|exit|reverse|sum|floor|ceil|round|isqrt)$/;
  const JETON = /(\/\/[^\n]*)|("(?:\\.|[^"\\\n])*")|(\b0x[0-9A-Fa-f_]+\b|\b\d[\d_]*(?:\.\d[\d_]*)?(?:e[+-]?\d+)?\b)|(\b[A-Za-z_]\w*\b)(\s*\()?/g;
  const echapper = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

  function colorer(code) {
    const src = code.textContent;
    let out = "", dernier = 0, m;
    // Les textes sur plusieurs lignes (""") : colorés en bloc.
    const triple = /"""[\s\S]*?"""/g;
    const zones = [];
    while ((m = triple.exec(src))) zones.push([m.index, m.index + m[0].length]);
    const dansZone = (i) => zones.find((z) => i >= z[0] && i < z[1]);
    JETON.lastIndex = 0;
    while ((m = JETON.exec(src))) {
      const z = dansZone(m.index);
      if (z) {
        out += echapper(src.slice(dernier, z[0])) + '<span class="t-texte">' + echapper(src.slice(z[0], z[1])) + "</span>";
        dernier = z[1];
        JETON.lastIndex = z[1];
        continue;
      }
      out += echapper(src.slice(dernier, m.index));
      dernier = m.index + m[0].length;
      if (m[1]) out += '<span class="t-com">' + echapper(m[1]) + "</span>";
      else if (m[2]) out += '<span class="t-texte">' + echapper(m[2]) + "</span>";
      else if (m[3]) out += '<span class="t-nombre">' + m[3] + "</span>";
      else {
        const mot = m[4], suite = m[5] || "";
        if (MOTS.has(mot)) out += '<span class="t-mot">' + mot + "</span>";
        else if (/^[A-Z]/.test(mot)) out += '<span class="t-type">' + mot + "</span>";
        else if (suite) out += '<span class="t-appel">' + mot + "</span>";
        else out += mot;
        out += suite;
      }
    }
    out += echapper(src.slice(dernier));
    code.innerHTML = out;
  }

  document.querySelectorAll("pre code").forEach((code) => {
    const pre = code.parentElement;
    if (code.classList.contains("language-lion")) colorer(code);
    if (pre.closest(".decl")) return;
    const b = document.createElement("button");
    b.className = "copier";
    b.type = "button";
    b.textContent = "Copier";
    b.addEventListener("click", () => {
      navigator.clipboard.writeText(code.textContent).then(() => {
        b.textContent = "Copié";
        setTimeout(() => (b.textContent = "Copier"), 1200);
      }, () => {});
    });
    pre.appendChild(b);
  });

  // ---------------------------------------------------------------- sommaire
  const toc = document.querySelector(".toc");
  const menu = document.querySelector(".menu");
  if (toc && menu) {
    menu.addEventListener("click", () => {
      const ouvert = toc.classList.toggle("ouvert");
      menu.setAttribute("aria-expanded", String(ouvert));
    });
    toc.addEventListener("click", (e) => {
      if (e.target.closest("a")) { toc.classList.remove("ouvert"); menu.setAttribute("aria-expanded", "false"); }
    });
    const liens = new Map();
    toc.querySelectorAll("a[href^='#']").forEach((a) => liens.set(decodeURIComponent(a.hash.slice(1)), a));
    const cibles = [...liens.keys()].map((id) => document.getElementById(id)).filter(Boolean);
    let actif = null;
    const suivre = () => {
      let courant = cibles[0];
      for (const c of cibles) if (c.getBoundingClientRect().top < 120) courant = c;
      const a = courant && liens.get(courant.id);
      if (a && a !== actif) {
        if (actif) actif.classList.remove("ici");
        a.classList.add("ici");
        actif = a;
        const r = a.getBoundingClientRect(), t = toc.getBoundingClientRect();
        if (r.top < t.top || r.bottom > t.bottom) a.scrollIntoView({ block: "nearest" });
      }
    };
    addEventListener("scroll", suivre, { passive: true });
    suivre();
  } else if (menu) menu.hidden = true;

  // ---------------------------------------------------------------- recherche
  const q = document.getElementById("q");
  const res = document.getElementById("resultats");
  let index = null, choix = -1;
  const normal = (s) => s.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "");
  async function charger() {
    if (!index) {
      try { index = await (await fetch("recherche.json")).json(); } catch (e) { index = []; }
      index.forEach((e) => (e.n = normal(e.t)));
    }
  }
  function afficher() {
    const mots = normal(q.value.trim()).split(/\s+/).filter(Boolean);
    if (!mots.length) { res.hidden = true; return; }
    const trouves = index.filter((e) => mots.every((m) => e.n.includes(m)))
      .sort((a, b) => (a.n.startsWith(mots[0]) ? 0 : 1) - (b.n.startsWith(mots[0]) ? 0 : 1) || a.t.length - b.t.length)
      .slice(0, 30);
    res.innerHTML = trouves.length
      ? trouves.map((e) => `<li><a href="${e.u}"><span>${echapper(e.t)}</span><small>${echapper(e.p)}</small></a></li>`).join("")
      : "<li><a>Aucun résultat</a></li>";
    choix = -1;
    res.hidden = false;
  }
  q.addEventListener("focus", charger);
  q.addEventListener("input", async () => { await charger(); afficher(); });
  q.addEventListener("keydown", (e) => {
    const liens = [...res.querySelectorAll("a[href]")];
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      choix = Math.max(0, Math.min(liens.length - 1, choix + (e.key === "ArrowDown" ? 1 : -1)));
      liens.forEach((a, i) => a.classList.toggle("actif", i === choix));
    } else if (e.key === "Enter" && liens.length) {
      location.href = liens[Math.max(choix, 0)].href;
      res.hidden = true;
    } else if (e.key === "Escape") { res.hidden = true; q.blur(); }
  });
  document.addEventListener("click", (e) => { if (!e.target.closest(".recherche")) res.hidden = true; });
  res.addEventListener("click", () => (res.hidden = true));
  document.addEventListener("keydown", (e) => {
    if (e.key === "/" && document.activeElement !== q && !e.target.closest("input")) { e.preventDefault(); q.focus(); }
  });

  // ---------------------------------------------------------------- filtre des modules
  const filtre = document.querySelector(".filtre");
  if (filtre) {
    filtre.addEventListener("input", () => {
      const m = normal(filtre.value.trim());
      document.querySelectorAll(".module").forEach((mod) => {
        let visibles = 0;
        mod.querySelectorAll(".decl").forEach((d) => {
          const ok = !m || normal(mod.id + "." + d.textContent).includes(m);
          d.classList.toggle("cache", !ok);
          if (ok) visibles++;
        });
        mod.querySelectorAll(".decls").forEach((g) => {
          const vide = !g.querySelector(".decl:not(.cache)");
          g.classList.toggle("cache", vide);
          const h = g.previousElementSibling;
          if (h && h.tagName === "H3") h.classList.toggle("cache", vide);
        });
        mod.classList.toggle("cache", visibles === 0);
      });
    });
  }
})();
