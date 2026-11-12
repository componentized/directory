// The page contents column: collapsible on wide screens, a folding panel above the content on
// narrow ones, and highlighting the section being read. Collapsing is remembered in this browser;
// an inline script in the page's head applies it before the page paints.
(() => {
  const toc = document.getElementById("toc");
  if (!toc) return;
  const root = document.documentElement;
  const toggle = toc.querySelector("[data-toc-toggle]");
  const label = toc.querySelector("[data-toc-label]");
  const narrow = matchMedia("(max-width: 1000px)");

  const expanded = () =>
    narrow.matches ? root.dataset.tocOpen === "true" : root.dataset.toc !== "collapsed";
  const sync = () => {
    const open = expanded();
    toggle.setAttribute("aria-expanded", String(open));
    label.textContent = open ? "Hide the page contents" : "Show the page contents";
    toggle.title = label.textContent;
  };

  toggle.addEventListener("click", () => {
    if (narrow.matches) {
      // a narrow screen folds the panel for this visit only
      if (expanded()) delete root.dataset.tocOpen;
      else root.dataset.tocOpen = "true";
    } else if (expanded()) {
      root.dataset.toc = "collapsed";
      try {
        localStorage.setItem("toc", "collapsed");
      } catch {}
    } else {
      delete root.dataset.toc;
      try {
        localStorage.removeItem("toc");
      } catch {}
    }
    sync();
  });
  narrow.addEventListener("change", sync);
  sync();

  // on a narrow screen, following a link folds the panel out of the way
  toc.addEventListener("click", (e) => {
    if (narrow.matches && e.target.closest("a")) {
      delete root.dataset.tocOpen;
      sync();
    }
  });

  // highlight the link of the last section whose top has scrolled past the header
  const links = [...toc.querySelectorAll('a[href^="#"]')]
    .map((link) => [link, document.getElementById(decodeURIComponent(link.hash.slice(1)))])
    .filter(([, target]) => target);
  let current = null;
  const spy = () => {
    let active = null;
    for (const [link, target] of links) {
      if (target.getBoundingClientRect().top <= 96) active = link;
    }
    // at the bottom of the page, the last sections can't scroll past the header
    if (innerHeight + scrollY >= document.body.scrollHeight - 2 && links.length) {
      const visible = links.filter(([, t]) => t.getBoundingClientRect().top < innerHeight);
      if (visible.length) active = visible[visible.length - 1][0];
    }
    if (active === current) return;
    current?.removeAttribute("aria-current");
    active?.setAttribute("aria-current", "location");
    current = active;
    if (active && !narrow.matches && toc.scrollHeight > toc.clientHeight) {
      active.scrollIntoView({ block: "nearest" });
    }
  };
  let queued = false;
  addEventListener(
    "scroll",
    () => {
      if (queued) return;
      queued = true;
      requestAnimationFrame(() => {
        queued = false;
        spy();
      });
    },
    { passive: true },
  );
  spy();
})();
