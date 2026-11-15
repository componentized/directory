// An IDE style tooltip for a link to a type or function, on every page: its definition,
// highlighted, the first paragraph of its docs, and where it's from. A link points into a version's
// interface page, e.g. `/wasi/http/0.3.0/types/#type-fields`, or `#type-fields` within one, so its
// details are in that version's API, `api.json`, fetched once when first needed. A link to an
// interface's page, e.g. `/wasi/http/0.3.0/types/`, has one too: the interface, how many types and
// functions it has, and the first paragraph of its docs. So does a link to a version's page, e.g.
// `/wasi/http/0.3.0/`, in a list of packages such as a version's dependencies: the package, what it
// has, when it was published, and its description. Links in the contents and in menus, which name
// what they lead to anyway, have none, nor do cards, which show as much.
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
  // whether a link is to an interface's page, `/{namespace}/{name}/{version}/{interface}/`, rather
  // than into it
  const toInterface = (link) => {
    const url = new URL(link.getAttribute("href"), location.href);
    const parts = url.pathname.split("/");
    return url.origin === location.origin && !url.hash && parts.length === 6 && parts[5] === "" && parts.slice(1, 5).every(Boolean);
  };
  // whether a link is to a version's page, `/{namespace}/{name}/{version}/`, in a list of packages,
  // e.g. a version's dependencies, rather than of its versions, or one that asks for a tooltip,
  // `data-tip`, e.g. an interface's package
  const toVersion = (link) => {
    const url = new URL(link.getAttribute("href"), location.href);
    const parts = url.pathname.split("/");
    return url.origin === location.origin && !url.hash && parts.length === 5 && parts[4] === "" && parts.slice(1, 4).every(Boolean) &&
      parts[3] !== "compare" && (link.hasAttribute("data-tip") || (!!link.closest("ul.links") && !link.closest(".versions-list")));
  };
  // the item a link points to, with where it's from, or the interface or the version; or null
  const target = async (link) => {
    const url = new URL(link.getAttribute("href"), location.href);
    const anchor = decodeURIComponent(url.hash.slice(1));
    const [, namespace, name, version, iface] = url.pathname.split("/");
    if (toVersion(link)) {
      const data = await apiAt(`/${namespace}/${name}/${version}/api.json`);
      return data ? { version: data } : null;
    }
    if (!iface || url.origin !== location.origin) return null;
    if (!anchor && !toInterface(link)) return null;
    const data = await apiAt(`/${namespace}/${name}/${version}/api.json`);
    const interfaceName = decodeURIComponent(iface);
    const found = data?.interfaces?.[interfaceName];
    const id = `${data?.package}/${interfaceName}@${data?.version}`;
    if (!anchor) return found ? { iface: found, id } : null;
    const [itemName, item] = Object.entries(found?.items ?? {}).find(([, i]) => i.anchor === anchor) ?? [];
    return item ? { item, name: itemName, from: id } : null;
  };
  // a record's fields, a variant's or enum's cases, or a flags' flags, one per line, the first few
  // of a long list and how many more; null for a type without such a list, e.g. a resource or an
  // alias, whose definition is one line
  const LISTED = 8;
  const listed = (kind, name, members) => {
    if (!["record", "variant", "enum", "flags"].includes(kind) || !members?.length) return null;
    const shown = members.length > LISTED ? members.slice(0, LISTED - 1) : members;
    const more = members.length - shown.length;
    return `<span class="k">${esc(kind)}</span> <span class="t">${esc(name)}</span> {\n${shown
      .map((m) => `  ${m},\n`)
      .join("")}${more ? `  <span class="wit-tip-more">… ${more} more</span>\n` : ""}}`;
  };
  // a version's tooltip: its package and version, what it has, when it was published, and its
  // description
  const versionTip = ({ version: v }) => {
    const count = (n, what) => `${n} ${what}${n === 1 ? "" : "s"}`;
    const has = v.kind === "component"
      ? [count(v.imports?.length ?? 0, "import"), count(v.exports?.length ?? 0, "export")]
      : [count(Object.keys(v.interfaces ?? {}).length, "interface"), count(Object.keys(v.worlds ?? {}).length, "world")];
    return `<div class="wit-tip-head">${icon(v.kind)}<div class="wit-tip-body"><code class="wit-tip-def">${esc(`${v.package}@${v.version}`)}</code><span class="wit-tip-from">${[...has, v.date].filter(Boolean).map(esc).join(" · ")}</span></div></div>
      ${v.summary ? `<p class="wit-tip-docs">${esc(v.summary)}</p>` : ""}`;
  };
  // an interface's tooltip: its id, how many types and functions it has, its gates, and its docs
  const interfaceTip = ({ iface, id }) => {
    const items = Object.values(iface.items ?? {});
    const funcs = items.filter((i) => i.kind === "func").length;
    const types = items.length - funcs;
    const count = (n, what) => `${n} ${what}${n === 1 ? "" : "s"}`;
    const gates = [iface.unstable && `unstable: ${iface.unstable}`, iface.deprecated && "deprecated"].filter(Boolean);
    return `<div class="wit-tip-head">${icon("interface")}<div class="wit-tip-body"><code class="wit-tip-def">${esc(id)}</code><span class="wit-tip-from">${[count(types, "type"), count(funcs, "function"), ...gates].map(esc).join(" · ")}</span></div></div>
      ${iface.summary ? `<p class="wit-tip-docs">${esc(iface.summary)}</p>` : ""}`;
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
    if (found.iface) return place(link, interfaceTip(found));
    if (found.version) return place(link, versionTip(found));
    const { item, name, from } = found;
    // each definition with the interface it's in beneath it: an item's, and for a used type, the
    // type it uses, as it's defined where it comes from
    const definition = (iconHtml, def, where, extra = "") =>
      `<div class="wit-tip-head${extra}">${iconHtml}<div class="wit-tip-body"><code class="wit-tip-def">${def}</code>${where ? `<span class="wit-tip-from">${esc(where)}</span>` : ""}</div></div>`;
    // a use or an alias also shows the type it refers to, with its docs, when they aren't the
    // item's own, e.g. those of an alias with docs of its own
    const origin = item.origin;
    const originDocs = origin?.summary && origin.summary !== item.summary ? origin.summary : "";
    const own = item.kind === "use" ? item.def_html : listed(item.kind, name, item.members_html);
    place(link, `${definition(itemIcon(item.kind, item), own ?? item.def_html ?? esc(item.def), from)}
      ${item.summary ? `<p class="wit-tip-docs">${esc(item.summary)}</p>` : ""}
      ${origin ? definition(icon(origin.kind ?? item.used_kind), listed(origin.kind, origin.name, origin.members_html) ?? origin.def_html, origin.from, " wit-tip-origin") : ""}
      ${originDocs ? `<p class="wit-tip-docs">${esc(originDocs)}</p>` : ""}`);
  };
  // shows a tooltip for a link, beside it
  const place = (link, html) => {
    tip.innerHTML = html;
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
    const link = e.target.closest?.("a[href]");
    if (!link || link.closest(".toc, .pivot-menu, [data-no-tip]")) return null;
    return link.matches('[href*="#type-"], [href*="#func-"]') || toInterface(link) || toVersion(link) ? link : null;
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
