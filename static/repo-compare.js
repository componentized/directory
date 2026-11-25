// Compares two versions of a repository: each package it released at either version, summed up
// from their APIs, the `api.json` the build writes for each version, as a package's own compare
// page diffs them, with diff.js. A package with differences links to its own compare page, at the
// same two versions, for the details. Before tagging a release, it compares the latest release
// with the dev build that's to become the next one.
(() => {
  const form = document.querySelector("[data-compare]");
  const result = document.querySelector("[data-compare-result]");
  const counts = document.querySelector("[data-compare-counts]");
  if (!form || !result) return;
  const { esc, icon } = window.directory;
  const { diff, kib, sbomChanges, advisoryChanges } = window.directory.diff;
  const versions = JSON.parse(document.getElementById("compare-versions").textContent);
  const byVersion = new Map(versions.map((v) => [v.version, v]));

  // the versions compared: the newest, e.g. the dev build of the next release, and the latest
  // release before it, unless the url says otherwise
  const to = { value: versions[0].version };
  const from = { value: (versions.slice(1).find((v) => v.stable) ?? versions[1]).version };
  const params = new URLSearchParams(location.search);
  for (const [state, key] of [[from, "from"], [to, "to"]]) {
    const version = params.get(key);
    if (version && byVersion.has(version)) state.value = version;
  }

  const picks = {
    from: form.querySelector('[data-compare-side="from"]'),
    to: form.querySelector('[data-compare-side="to"]'),
  };
  // a menu shows its version, marks it current, and unfolds it when it's an older one
  const show = () => {
    for (const [side, state] of [["from", from], ["to", to]]) {
      const pick = picks[side];
      pick.querySelector("[data-pivot-label]").textContent = state.value;
      for (const a of pick.querySelectorAll("a[data-version]")) {
        const current = a.dataset.version === state.value;
        a.toggleAttribute("aria-current", current);
        if (current) a.closest("li").hidden = false;
      }
    }
  };
  // choosing a version compares it in place, rather than following its link
  form.addEventListener("click", (e) => {
    const link = e.target.closest("a[data-version]");
    if (!link || e.metaKey || e.ctrlKey || e.shiftKey || e.button > 0) return;
    e.preventDefault();
    const pick = link.closest("[data-compare-side]");
    (pick.dataset.compareSide === "from" ? from : to).value = link.dataset.version;
    pick.open = false;
    pick.querySelector("summary").focus({ preventScroll: true });
    compare();
  });
  form.addEventListener("submit", (e) => e.preventDefault());
  form.querySelector("[data-compare-swap]").addEventListener("click", () => {
    [from.value, to.value] = [to.value, from.value];
    compare();
  });

  const apis = new Map();
  const api = (url) => {
    if (!apis.has(url)) {
      apis.set(
        url,
        fetch(url).then((r) => {
          if (!r.ok) throw new Error(`${r.status} fetching ${url}`);
          return r.json();
        }),
      );
    }
    return apis.get(url);
  };

  // what changed in one package between the two versions: its API's changes, its size, the
  // packages its SBOM lists and its advisories, or that one of the versions hasn't it
  const summarize = async (was, is) => {
    if (!was || !is) return { was, is };
    if (was.version === is.version) return { was, is, same: true };
    const [a, b] = await Promise.all([api(was.api), api(is.api)]);
    const { breaking, cautions, notes } = diff(a, b);
    const size = a.size && b.size ? b.size - a.size : null;
    const sbom = sbomChanges(a, b);
    const advisories = advisoryChanges(a, b);
    // WIT that differs but for its dependencies' versions, e.g. its docs, which the API's changes
    // don't count
    const unversioned = (html) => (html ?? "").replace(/<[^>]*>/g, "").replace(/@\d[0-9A-Za-z.+-]*/g, "");
    const wit = unversioned(a.wit) !== unversioned(b.wit);
    const changed = breaking.length || cautions.length || notes.length || size || sbom?.length || advisories?.fixed.length || advisories?.added.length || wit;
    const interfaceChanged = !!(breaking.length || cautions.length || notes.length || wit);
    return { was, is, breaking, cautions, notes, size, sbom, advisories, wit, interfaceChanged, changed: !!changed, unchecked: [a, b].some((v) => v.sbom && !v.sbom.checked) };
  };

  // a count linking to its section of the package's own comparison, or a dash for none
  const counted = (n, href, kind, label) =>
    n ? `<a class="repo-count repo-count-${kind}" href="${esc(href)}" title="${esc(`${n} ${label}`)}">${n}</a>` : `<span class="dim" aria-label="none">–</span>`;

  const row = (s, a, b) => {
    const p = s.is ?? s.was;
    const name = `${icon(p.icon)}<a href="${esc(p.url)}" data-tip><code>${esc(p.package)}</code></a>`;
    if (!s.was) return `<tr class="repo-added"><td class="dep-name">${name}</td><td><span class="change-badge change-added">added</span> <code>${esc(s.is.version)}</code></td><td colspan="8" class="dim">Not released at ${esc(a)}</td></tr>`;
    if (!s.is) return `<tr class="repo-removed"><td class="dep-name">${name}</td><td><span class="change-badge change-removed">removed</span> <code>${esc(s.was.version)}</code></td><td colspan="8" class="dim">Not released at ${esc(b)}</td></tr>`;
    const at = `<code>${esc(s.was.version)}</code> → <code>${esc(s.is.version)}</code>`;
    if (s.same) return `<tr class="repo-same"><td class="dep-name">${name}</td><td><code>${esc(s.is.version)}</code></td><td colspan="8" class="dim">The same version</td></tr>`;
    const compare = `${s.is.compare}?from=${encodeURIComponent(s.was.version)}&to=${encodeURIComponent(s.is.version)}`;
    if (!s.changed) return `<tr class="repo-same"><td class="dep-name">${name}</td><td>${at}</td><td colspan="8" class="dim">No differences</td></tr>`;
    const size = s.size == null
      ? `<span class="dim">–</span>`
      : s.size === 0
        ? `<span class="dim">same</span>`
        : `<a class="size-delta ${s.size > 0 ? "size-up" : "size-down"}" href="${esc(`${compare}#compare-size`)}">${s.size > 0 ? "+" : "−"}${esc(kib(s.size))}</a>`;
    const sbom = s.sbom == null ? `<span class="dim" title="Not both versions have a verified SBOM">–</span>` : counted(s.sbom.length, `${compare}#compare-sbom`, "sbom", "changed packages");
    const adv = s.advisories == null
      ? `<span class="dim" title="Not both versions have a verified SBOM">–</span>`
      : s.advisories.added.length || s.advisories.fixed.length
        ? `<a class="repo-advisories" href="${esc(`${compare}#compare-advisories`)}">${[
            s.advisories.added.length && `<span class="change-badge change-new" title="${esc(s.advisories.added.map((x) => x.id).join(", "))}">${s.advisories.added.length} new</span>`,
            s.advisories.fixed.length && `<span class="change-badge change-fixed" title="${esc(s.advisories.fixed.map((x) => x.id).join(", "))}">${s.advisories.fixed.length} fixed</span>`,
          ].filter(Boolean).join(" ")}</a>`
        : s.advisories.both.length
          ? `<span class="dim" title="${esc(s.advisories.both.map((x) => x.id).join(", "))}">${s.advisories.both.length} remaining</span>`
          : `<span class="dim" aria-label="none">–</span>`;
    return `<tr><td class="dep-name">${name}</td><td>${at}</td>
      <td class="num">${counted(s.breaking.length, `${compare}#compare-breaking`, "stop", "breaking changes")}</td>
      <td class="num">${counted(s.cautions.length, `${compare}#compare-cautions`, "caution", "cautions")}</td>
      <td class="num">${counted(s.notes.length, `${compare}#compare-other`, "info", "other changes")}</td>
      <td class="num">${s.wit ? `<a href="${esc(`${compare}#compare-wit`)}" title="Its WIT differs">yes</a>` : `<span class="dim">–</span>`}</td>
      <td class="num">${size}</td>
      <td class="num">${sbom}</td>
      <td>${adv}</td>
      <td><a class="repo-details" href="${esc(compare)}" title="${esc(`Compare ${s.is.package} ${s.was.version} with ${s.is.version}`)}">compare</a></td></tr>`;
  };

  let shown = 0;
  const compare = async () => {
    const run = ++shown;
    show();
    const [a, b] = [from.value, to.value];
    const url = new URL(location.href);
    url.searchParams.set("from", a);
    url.searchParams.set("to", b);
    history.replaceState(null, "", url);
    counts.hidden = true;
    if (a === b) {
      result.innerHTML = `<p class="dim">Choose two different versions to compare.</p>`;
      return;
    }
    result.setAttribute("aria-busy", "true");
    try {
      // the packages of either version, the later version's first, in its order
      const was = new Map(byVersion.get(a).packages.map((p) => [p.package, p]));
      const is = new Map(byVersion.get(b).packages.map((p) => [p.package, p]));
      const names = [...new Set([...is.keys(), ...was.keys()])];
      const summaries = await Promise.all(names.map((n) => summarize(was.get(n), is.get(n))));
      if (run !== shown) return;
      const sum = (f) => summaries.reduce((n, s) => n + (f(s) ?? 0), 0);
      const totals = {
        changed: summaries.filter((s) => s.changed || !s.was || !s.is).length,
        api: summaries.filter((s) => s.interfaceChanged).length,
        sbom: summaries.filter((s) => s.sbom?.length).length,
        sized: summaries.filter((s) => s.size).length,
        breaking: sum((s) => s.breaking?.length),
        cautions: sum((s) => s.cautions?.length),
        notes: sum((s) => s.notes?.length),
        added: sum((s) => s.advisories?.added.length),
        fixed: sum((s) => s.advisories?.fixed.length),
      };
      const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;
      const count = (kind, n, label) =>
        `<span><span class="compare-count compare-count-${kind}" title="${esc(label)}" aria-label="${esc(label)}"><svg class="meta-icon" aria-hidden="true" focusable="false"><use href="#meta-${kind}"/></svg>${n}</span></span>`;
      counts.innerHTML = [
        count("stop", totals.breaking, plural(totals.breaking, "breaking change", "breaking changes")),
        count("caution", totals.cautions, plural(totals.cautions, "caution", "cautions")),
        count("info", totals.notes, plural(totals.notes, "other change", "other changes")),
      ].join("");
      counts.hidden = false;
      const advisories = totals.added || totals.fixed
        ? `, advisories: ${[totals.added && `<span class="still-affected">${totals.added} new</span>`, totals.fixed && `${totals.fixed} fixed`].filter(Boolean).join(", ")}`
        : "";
      const unchecked = summaries.some((s) => s.unchecked)
        ? `<p class="dim">Not every package some of these SBOMs list is checked for advisories yet, so there may be more.</p>`
        : "";
      result.innerHTML = `
        <section class="compare-section" id="repo-packages">
          <h2>Packages <span class="count-chip" title="${esc(plural(totals.changed, "package differs", "packages differ"))}">${totals.changed}</span></h2>
          <p class="dim">${totals.changed} of ${plural(names.length, "package differs", "packages differ")}: ${totals.api} in ${totals.api === 1 ? "its" : "their"} API or WIT, ${totals.sbom} in ${totals.sbom === 1 ? "its" : "their"} SBOM, ${totals.sized} in size${advisories}.</p>
          ${unchecked}
          <label class="wit-options"><input type="checkbox" data-hide-same> Hide packages without differences</label>
          <div class="sbom-scroll"><table class="sbom-table repo-compare">
            <thead><tr><th scope="col">Package</th><th scope="col">Versions</th>
              <th scope="col" class="num" title="Breaking changes"><svg class="meta-icon" aria-hidden="true" focusable="false"><use href="#meta-stop"/></svg><span class="visually-hidden">Breaking changes</span></th>
              <th scope="col" class="num" title="Cautions"><svg class="meta-icon" aria-hidden="true" focusable="false"><use href="#meta-caution"/></svg><span class="visually-hidden">Cautions</span></th>
              <th scope="col" class="num" title="Other changes"><svg class="meta-icon" aria-hidden="true" focusable="false"><use href="#meta-info"/></svg><span class="visually-hidden">Other changes</span></th>
              <th scope="col" class="num">WIT</th><th scope="col" class="num">Size</th><th scope="col" class="num" title="Packages its SBOM lists that were added, removed, upgraded or downgraded">SBOM</th><th scope="col">Advisories</th><th scope="col"><span class="visually-hidden">Details</span></th></tr></thead>
            <tbody>${summaries.map((s) => row(s, a, b)).join("")}</tbody>
          </table></div>
        </section>`;
      result.querySelector("[data-hide-same]").addEventListener("change", (e) => {
        for (const tr of result.querySelectorAll("tr.repo-same")) tr.hidden = e.target.checked;
      });
    } catch (err) {
      if (run !== shown) return;
      result.innerHTML = `<p class="notice">Couldn't compare the versions: ${esc(err.message)}</p>`;
    } finally {
      if (run === shown) result.removeAttribute("aria-busy");
    }
  };
  compare();
})();
