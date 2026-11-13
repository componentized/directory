// The breadcrumbs that switch to another namespace, package, version or interface. Each is a
// <details> menu of links, so it works without this script, which closes the others when one
// opens, closes on Escape or a click elsewhere, moves through the links with the arrow keys,
// keeps the menu on screen, and adds a filter to a long menu.
(() => {
  const pivots = [...document.querySelectorAll("[data-pivot]")];
  if (!pivots.length) return;
  // a menu with more choices than this gets a filter
  const FILTER_AT = 12;

  // a menu's choices, and buttons such as one that shows a line's older versions
  const links = (pivot) =>
    [...pivot.querySelectorAll(".pivot-menu a, .pivot-menu li > button")].filter(
      (a) => !a.closest("li").hidden,
    );

  const close = (pivot, refocus) => {
    if (!pivot.open) return;
    pivot.open = false;
    if (refocus) pivot.querySelector("summary").focus();
  };

  for (const pivot of pivots) {
    const menu = pivot.querySelector(".pivot-menu");
    const items = [...menu.querySelectorAll("li")];
    let filter = null;

    // a menu that folds its choices, e.g. a compare page's versions, has no filter, which would
    // unfold them
    if (items.length > FILTER_AT && !pivot.hasAttribute("data-pivot-no-filter")) {
      const what =
        pivot.dataset.pivotWhat ?? pivot.querySelector("summary").title.replace(/^Switch /, "");
      const wrap = document.createElement("li");
      wrap.className = "pivot-filter";
      filter = document.createElement("input");
      filter.type = "search";
      filter.placeholder = `Filter ${items.length} ${what}s`;
      filter.setAttribute("aria-label", `Filter the ${what}s`);
      filter.autocomplete = "off";
      wrap.append(filter);
      const empty = document.createElement("li");
      empty.className = "pivot-empty";
      empty.textContent = "No matches";
      empty.hidden = true;
      menu.prepend(wrap);
      menu.append(empty);
      filter.addEventListener("input", () => {
        const terms = filter.value.toLowerCase().split(/\s+/).filter(Boolean);
        let shown = 0;
        for (const item of items) {
          const text = item.textContent.toLowerCase();
          item.hidden = !terms.every((t) => text.includes(t));
          if (!item.hidden) shown++;
        }
        empty.hidden = shown > 0;
      });
      filter.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          // follow the first match
          const first = links(pivot)[0];
          if (first) (e.preventDefault(), first.click());
        }
      });
    }

    pivot.addEventListener("toggle", () => {
      if (!pivot.open) return;
      for (const other of pivots) if (other !== pivot) close(other, false);
      // keep the menu within the window
      menu.style.left = "";
      const right = menu.getBoundingClientRect().right;
      const overflow = right - (document.documentElement.clientWidth - 16);
      if (overflow > 0) menu.style.left = `${-4 - overflow}px`;
      // scroll the menu alone to its current choice: scrollIntoView, or focusing without
      // preventScroll, would scroll the page as well
      const current = menu.querySelector("[aria-current]");
      if (current) {
        const item = current.getBoundingClientRect();
        const box = menu.getBoundingClientRect();
        if (item.bottom > box.bottom) menu.scrollTop += item.bottom - box.bottom + box.height / 2;
      }
      (filter ?? current ?? links(pivot)[0])?.focus({ preventScroll: true });
    });

    // a version switch shows the section's panels for the chosen version, in place of following
    // the link to that version's page
    if (pivot.hasAttribute("data-pivot-switch")) {
      const section = pivot.closest("section") ?? document;
      const label = pivot.querySelector("[data-pivot-label]");
      menu.addEventListener("click", (e) => {
        const link = e.target.closest("a[data-version]");
        if (!link || e.metaKey || e.ctrlKey || e.shiftKey || e.button > 0) return;
        e.preventDefault();
        const version = link.dataset.version;
        for (const panel of section.querySelectorAll("[data-version-panel]")) {
          panel.hidden = panel.dataset.versionPanel !== version;
        }
        for (const a of menu.querySelectorAll("a[data-version]")) a.removeAttribute("aria-current");
        link.setAttribute("aria-current", "true");
        label.textContent = version;
        close(pivot, true);
      });
    }

    pivot.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && pivot.open) {
        e.preventDefault();
        close(pivot, true);
        return;
      }
      if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
      if (!pivot.open) {
        if (e.target === pivot.querySelector("summary")) (e.preventDefault(), (pivot.open = true));
        return;
      }
      const all = links(pivot);
      if (!all.length) return;
      e.preventDefault();
      const at = all.indexOf(document.activeElement);
      const next =
        e.key === "ArrowDown"
          ? all[at < 0 ? 0 : Math.min(at + 1, all.length - 1)]
          : at <= 0
            ? (filter ?? all[0])
            : all[at - 1];
      next.focus({ preventScroll: true });
      // keep the focused choice in view within the menu
      const item = next.getBoundingClientRect();
      const box = menu.getBoundingClientRect();
      if (item.top < box.top) menu.scrollTop -= box.top - item.top + 4;
      else if (item.bottom > box.bottom) menu.scrollTop += item.bottom - box.bottom + 4;
    });

    // leaving the menu with the keyboard closes it
    pivot.addEventListener("focusout", (e) => {
      if (pivot.open && e.relatedTarget && !pivot.contains(e.relatedTarget)) close(pivot, false);
    });
  }

  document.addEventListener("click", (e) => {
    for (const pivot of pivots) if (!pivot.contains(e.target)) close(pivot, false);
  });
})();
