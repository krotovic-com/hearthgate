// Hearthgate docs theme: code highlighting and copy buttons, page outline, breadcrumbs,
// page titles in the pager, the phone menu and the light/dark switch.
(function () {
  "use strict";
  const main = document.querySelector("main");

  // Code blocks: highlight with mdBook's bundled highlight.js and add a copy button.
  main.querySelectorAll("pre > code").forEach(function (code) {
    const lang = (code.className.match(/language-(\w+)/) || [])[1];
    if (lang === "sh" || lang === "shell") code.classList.add("language-bash");
    if (window.hljs && lang && lang !== "text") {
      try { window.hljs.highlightBlock(code); } catch (e) { /* leave it plain */ }
    }
    const pre = code.parentElement;
    const wrap = document.createElement("div");
    wrap.className = "code";
    pre.parentNode.insertBefore(wrap, pre);
    wrap.appendChild(pre);
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "copy";
    btn.textContent = "Copy";
    btn.addEventListener("click", function () {
      const done = function (label) { btn.textContent = label; setTimeout(function () { btn.textContent = "Copy"; }, 1500); };
      try {
        navigator.clipboard.writeText(code.innerText.replace(/\n$/, "")).then(function () { done("Copied"); }, function () { done("Couldn't copy"); });
      } catch (e) { done("Couldn't copy"); }
    });
    wrap.appendChild(btn);
  });

  // Tables scroll inside their own box on narrow screens.
  main.querySelectorAll("table").forEach(function (t) {
    const wrap = document.createElement("div");
    wrap.className = "table-wrap";
    t.parentNode.insertBefore(wrap, t);
    wrap.appendChild(t);
  });

  // Breadcrumbs and pager titles from the sidebar.
  const sidebar = document.getElementById("sidebar");
  const active = sidebar && sidebar.querySelector("a.active");
  const h1 = main.querySelector("h1");
  if (active && h1) {
    let li = active.closest("li");
    let part = null;
    for (let p = li; p; p = p.previousElementSibling) {
      if (p.classList.contains("part-title")) { part = p.textContent.trim(); break; }
    }
    if (part) {
      const crumbs = document.createElement("div");
      crumbs.className = "crumbs";
      crumbs.innerHTML = '<a href="' + path_to_root + 'index.html">Docs</a><span>/</span><span></span>';
      crumbs.lastChild.textContent = part;
      h1.parentNode.insertBefore(crumbs, h1);
    }
  }
  document.querySelectorAll("[data-title-for]").forEach(function (b) {
    const target = b.getAttribute("data-title-for");
    const link = sidebar && Array.prototype.find.call(sidebar.querySelectorAll("a"), function (a) {
      return a.getAttribute("href") && a.getAttribute("href").replace(path_to_root, "") === target;
    });
    if (link) b.textContent = link.textContent.trim();
  });

  // On this page.
  const toc = document.getElementById("page-toc");
  const heads = Array.prototype.filter.call(main.querySelectorAll("h2[id], h3[id]"), function (h) { return h.id; });
  if (toc && heads.length >= 2) {
    const ul = toc.querySelector("ul");
    heads.forEach(function (h) {
      const li = document.createElement("li");
      const a = document.createElement("a");
      a.href = "#" + h.id;
      a.textContent = h.textContent.trim();
      if (h.tagName === "H3") a.className = "sub";
      li.appendChild(a);
      ul.appendChild(li);
    });
    toc.hidden = false;
    const links = ul.querySelectorAll("a");
    const spy = function () {
      let cur = 0;
      heads.forEach(function (h, i) { if (h.getBoundingClientRect().top < 120) cur = i; });
      links.forEach(function (a, i) { a.classList.toggle("on", i === cur); });
    };
    window.addEventListener("scroll", spy, { passive: true });
    spy();
  }

  // Phone menu.
  const menu = document.getElementById("menu-toggle");
  if (menu && sidebar) {
    menu.addEventListener("click", function () {
      const open = sidebar.classList.toggle("open");
      menu.setAttribute("aria-expanded", String(open));
    });
  }

  // Light/dark switch. Follows the system until you choose; the choice is shared with the website.
  const themeBtn = document.getElementById("theme-toggle");
  if (themeBtn) {
    themeBtn.addEventListener("click", function () {
      const root = document.documentElement;
      const dark = root.dataset.theme ? root.dataset.theme === "dark" : window.matchMedia("(prefers-color-scheme: dark)").matches;
      root.dataset.theme = dark ? "light" : "dark";
      try { localStorage.setItem("hearthgate-theme", root.dataset.theme); } catch (e) { /* private window */ }
    });
  }

  // Left and right arrow keys move between pages.
  document.addEventListener("keydown", function (e) {
    if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
    const tag = document.activeElement && document.activeElement.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA") return;
    const rel = e.key === "ArrowLeft" ? "prev" : e.key === "ArrowRight" ? "next" : null;
    const a = rel && document.querySelector('.pager a[rel~="' + rel + '"]');
    if (a) window.location.href = a.href;
  });
})();
