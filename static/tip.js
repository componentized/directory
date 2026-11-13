// An IDE style tooltip for a link to a type or function, on every page: its definition,
// highlighted, the first paragraph of its docs, and where it's from. A link points into a version's
// interface page, e.g. `/wasi/http/0.3.0/types/#type-fields`, or `#type-fields` within one, so its
// details are in that version's API, `api.json`, fetched once when first needed. Links in the
// contents and in menus, which name what they lead to anyway, have none.
//
// It also draws the icons of items as their pages do, for the compare page to share.
(() => {
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
  const icon = (kind) =>
    `<svg class="kind-icon kind-${esc(kind)}" aria-hidden="true" focusable="false"><use href="#kind-${esc(kind)}"/></svg>`;
  // an item's icon: a used type by the kind it uses, badged as used, and a function badged with
  // its modifiers, `static`, `async`, `get`, `set` or `constructor`
  const itemIcon = (kind, item) => {
    if (kind === "use" && item?.used_kind) {
      return `<span class="kind-icon-used" aria-hidden="true">${icon(item.used_kind)}<svg class="kind-badge" viewBox="0 0 16 16" focusable="false"><use href="#kind-used"/></svg></span>`;
    }
    if (kind === "func" && item) {
      const mods = item.modifiers ?? [];
      const label = `${mods.join(" ")} function`.trim();
      return `<span class="kind-icon-badged" title="${esc(label)}">${icon("func")}${mods
        .map((m) => `<svg class="fbadge fbadge-${esc(m)}" viewBox="0 0 16 16" aria-hidden="true" focusable="false"><use href="#mod-${esc(m)}"/></svg>`)
        .join("")}<span class="visually-hidden">${esc(label)}</span></span>`;
    }
    return icon(kind);
  };
  window.directory = { esc, icon, itemIcon };

  const tip = document.createElement("div");
  tip.className = "wit-tip wit";
  tip.id = "wit-tip";
  tip.setAttribute("role", "tooltip");
  tip.hidden = true;
  document.body.append(tip);

  const apis = new Map();
  const apiAt = (url) => {
    if (!apis.has(url)) {
      apis.set(url, fetch(url).then((r) => (r.ok ? r.json() : null)).catch(() => null));
    }
    return apis.get(url);
  };
  // the item a link points to, with where it's from, or null
  const target = async (link) => {
    const url = new URL(link.getAttribute("href"), location.href);
    const anchor = decodeURIComponent(url.hash.slice(1));
    const [, namespace, name, version, iface] = url.pathname.split("/");
    if (!anchor || !iface || url.origin !== location.origin) return null;
    const data = await apiAt(`/${namespace}/${name}/${version}/api.json`);
    const items = data?.interfaces?.[decodeURIComponent(iface)]?.items ?? {};
    const item = Object.values(items).find((i) => i.anchor === anchor);
    return item ? { item, from: `${data.package}/${decodeURIComponent(iface)}@${data.version}` } : null;
  };

  let shownFor = null;
  let pending = 0;
  const hide = () => {
    clearTimeout(pending);
    tip.hidden = true;
    shownFor?.removeAttribute("aria-describedby");
    shownFor = null;
  };
  const showFor = async (link) => {
    const found = await target(link);
    if (!found || !link.isConnected) return;
    const { item, from } = found;
    // a used type also shows the type it uses, as it's defined where it comes from
    // each definition with the interface it's in beneath it: an item's, and for a used type, the
    // type it uses, as it's defined where it comes from
    const definition = (iconHtml, def, where, extra = "") =>
      `<div class="wit-tip-head${extra}">${iconHtml}<div class="wit-tip-body"><code class="wit-tip-def">${def}</code>${where ? `<span class="wit-tip-from">${esc(where)}</span>` : ""}</div></div>`;
    tip.innerHTML = `${definition(itemIcon(item.kind, item), item.def_html ?? esc(item.def), from)}
      ${item.origin ? definition(icon(item.used_kind), item.origin.def_html, item.origin.from, " wit-tip-origin") : ""}
      ${item.summary ? `<p class="wit-tip-docs">${esc(item.summary)}</p>` : ""}`;
    // the pointer can't reach into a tooltip, so its types are text rather than links, colored
    // as they were
    for (const a of tip.querySelectorAll("a")) {
      const span = document.createElement("span");
      span.className = a.className;
      span.append(...a.childNodes);
      a.replaceWith(span);
    }
    tip.hidden = false;
    shownFor?.removeAttribute("aria-describedby");
    shownFor = link;
    link.setAttribute("aria-describedby", tip.id);
    // below the link, or above it when there's no room below, and within the window
    const r = link.getBoundingClientRect();
    const t = tip.getBoundingClientRect();
    const below = r.bottom + 6;
    const top = below + t.height > innerHeight - 8 ? Math.max(8, r.top - t.height - 6) : below;
    const left = Math.min(Math.max(8, r.left), innerWidth - t.width - 8);
    tip.style.top = `${top}px`;
    tip.style.left = `${Math.max(8, left)}px`;
  };
  const linkIn = (e) => {
    const link = e.target.closest?.('a[href*="#type-"], a[href*="#func-"]');
    return link && !link.closest(".toc, .pivot-menu, [data-no-tip]") ? link : null;
  };
  document.addEventListener("mouseover", (e) => {
    const link = linkIn(e);
    if (!link || link === shownFor) return;
    clearTimeout(pending);
    pending = setTimeout(() => showFor(link), 250);
  });
  document.addEventListener("mouseout", (e) => {
    const link = linkIn(e);
    if (link && !link.contains(e.relatedTarget)) hide();
  });
  document.addEventListener("focusin", (e) => {
    const link = linkIn(e);
    if (link) showFor(link);
  });
  document.addEventListener("focusout", (e) => {
    if (linkIn(e)) hide();
  });
  addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !tip.hidden) hide();
  });
  addEventListener("scroll", hide, { passive: true });
})();
