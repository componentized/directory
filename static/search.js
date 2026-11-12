// Search: a dropdown of matches in the header, and the filters of the all packages page.
(() => {
  const params = new URLSearchParams(location.search);
  const input = document.querySelector("[data-search]");
  let index;
  const load = () => (index ??= fetch("/search.json").then((r) => r.json()));

  const score = (pkg, terms) => {
    const id = pkg.package.toLowerCase();
    const text = `${id} ${pkg.description.toLowerCase()}`;
    if (!terms.every((t) => text.includes(t))) return 0;
    return terms.reduce((s, t) => s + (id === t ? 8 : id.includes(t) ? 4 : 1), 0);
  };

  if (input) {
    const list = document.createElement("ul");
    list.className = "suggest";
    list.hidden = true;
    input.after(list);
    input.value = params.get("q") ?? "";

    const render = async () => {
      const terms = input.value.toLowerCase().split(/\s+/).filter(Boolean);
      if (!terms.length) return (list.hidden = true);
      const matches = (await load())
        .map((pkg) => [score(pkg, terms), pkg])
        .filter(([s]) => s > 0)
        .sort((a, b) => b[0] - a[0] || a[1].package.localeCompare(b[1].package))
        .slice(0, 8);
      list.replaceChildren(
        ...matches.map(([, pkg]) => {
          const li = document.createElement("li");
          const a = document.createElement("a");
          a.href = pkg.url;
          const code = document.createElement("code");
          code.textContent = pkg.package;
          const kind = document.createElement("span");
          kind.className = `badge ${pkg.kind}`;
          kind.textContent = pkg.kind;
          const desc = document.createElement("span");
          desc.className = "dim";
          desc.textContent = pkg.description;
          a.append(code, " ", kind, desc);
          li.append(a);
          return li;
        }),
      );
      list.hidden = matches.length === 0;
    };

    input.addEventListener("focus", load, { once: true });
    input.addEventListener("input", render);
    input.addEventListener("blur", () => setTimeout(() => (list.hidden = true), 150));
    input.addEventListener("keydown", (e) => {
      const links = [...list.querySelectorAll("a")];
      const i = links.indexOf(document.activeElement);
      if (e.key === "ArrowDown" && links.length) {
        e.preventDefault();
        links[0].focus();
      } else if (e.key === "Escape") {
        list.hidden = true;
        input.blur();
      }
    });
    list.addEventListener("keydown", (e) => {
      const links = [...list.querySelectorAll("a")];
      const i = links.indexOf(document.activeElement);
      if (e.key === "ArrowDown") (e.preventDefault(), links[Math.min(i + 1, links.length - 1)]?.focus());
      if (e.key === "ArrowUp") (e.preventDefault(), (i <= 0 ? input : links[i - 1]).focus());
      if (e.key === "Escape") (list.hidden = true, input.focus());
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "/" && !/^(INPUT|SELECT|TEXTAREA)$/.test(document.activeElement?.tagName)) {
        e.preventDefault();
        input.focus();
      }
    });
  }

  const filters = document.querySelector("[data-filters]");
  if (filters) {
    const text = filters.querySelector("[data-filter-text]");
    const kind = filters.querySelector("[data-filter-kind]");
    const namespace = filters.querySelector("[data-filter-namespace]");
    const count = filters.querySelector("[data-filter-count]");
    const empty = document.querySelector("[data-filter-empty]");
    const cards = [...document.querySelectorAll("[data-filtered] .card")];
    text.value = params.get("q") ?? "";
    kind.value = params.get("kind") ?? "";
    namespace.value = params.get("namespace") ?? "";

    const apply = () => {
      const terms = text.value.toLowerCase().split(/\s+/).filter(Boolean);
      let shown = 0;
      for (const card of cards) {
        const visible =
          (!kind.value || card.dataset.kind === kind.value) &&
          (!namespace.value || card.dataset.namespace === namespace.value) &&
          terms.every((t) => card.dataset.text.includes(t));
        card.hidden = !visible;
        shown += visible;
      }
      count.textContent = `${shown} package${shown === 1 ? "" : "s"}`;
      empty.hidden = shown > 0;
      const next = new URLSearchParams();
      if (text.value) next.set("q", text.value);
      if (kind.value) next.set("kind", kind.value);
      if (namespace.value) next.set("namespace", namespace.value);
      history.replaceState(null, "", next.size ? `?${next}` : location.pathname);
    };
    for (const el of [text, kind, namespace]) el.addEventListener("input", apply);
    apply();
  }
})();
