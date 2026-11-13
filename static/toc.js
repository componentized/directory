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
  // where a link to a section scrolls it to, below what's pinned to the top of the window
  const offset = () => parseFloat(getComputedStyle(root).scrollPaddingTop) || 72;
  const spy = () => {
    let active = null;
    // the right column's sections are left out while it's beside the content
    const listed = links.filter(
      ([link]) => getComputedStyle(link.closest(".toc-aside") ?? link).display !== "none",
    );
    for (const [link, target] of listed) {
      if (target.getBoundingClientRect().top <= offset() + 24) active = link;
    }
    // at the bottom of the page, the last sections can't scroll past the header
    if (innerHeight + scrollY >= document.body.scrollHeight - 2 && listed.length) {
      const visible = listed.filter(([, t]) => t.getBoundingClientRect().top < innerHeight);
      if (visible.length) active = visible[visible.length - 1][0];
    }
    if (active === current) return;
    current?.removeAttribute("aria-current");
    active?.setAttribute("aria-current", "location");
    current = active;
    if (active && !narrow.matches && toc.scrollHeight > toc.clientHeight) {
      // scroll the contents alone: scrollIntoView would scroll the page as well, which in Safari
      // fights the reader's own scrolling
      const box = toc.getBoundingClientRect();
      const item = active.getBoundingClientRect();
      if (item.top < box.top + 8) toc.scrollTop -= box.top + 8 - item.top;
      else if (item.bottom > box.bottom - 8) toc.scrollTop += item.bottom - (box.bottom - 8);
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
  // the right column moves beside or under the content as the window resizes
  addEventListener("resize", spy, { passive: true });
  spy();
})();
