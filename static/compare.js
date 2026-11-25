// Compares two versions of a package from their APIs, the `api.json` the build writes for each
// version, so a package has one compare page rather than a page for each pair of its versions.
// What changed is worked out by diff.js, which a repository's compare page shares; this page lists
// each change, breaking changes first, and diffs the WIT line by line.
(() => {
  const form = document.querySelector("[data-compare]");
  const result = document.querySelector("[data-compare-result]");
  const counts = document.querySelector("[data-compare-counts]");
  if (!form || !result) return;
  const versions = JSON.parse(document.getElementById("compare-versions").textContent);
  const byVersion = new Map(versions.map((v) => [v.version, v]));
  // the versions compared, which the menus show and set
  const from = { value: versions[Math.min(1, versions.length - 1)].version };
  // the latest, the newest stable version, else the newest
  const to = { value: (versions.find((v) => v.stable) ?? versions[0]).version };
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

  // the icons of items, as their pages draw them, from tip.js
  const { esc, icon, itemIcon } = window.directory;
  // what changed between two versions' APIs, their SBOMs and their advisories, from diff.js
  const { diff, bytes, kib, sbomChanges, advisoryChanges } = window.directory.diff;

  // the versions in the url, or the latest and the one before it
  const params = new URLSearchParams(location.search);
  const choose = (select, version) => {
    if (version && byVersion.has(version)) select.value = version;
  };
  choose(to, params.get("to"));
  // the version before another to compare it with: the latest release in the line of the next
  // older version, rather than a pre-release such as a dev build, unless the line has no release
  const older = (version) => {
    const i = versions.findIndex((v) => v.version === version);
    const next = versions[Math.min(i + 1, versions.length - 1)];
    const released = versions.slice(i + 1).find((v) => v.line === next.line && v.stable);
    return (released ?? next).version;
  };
  choose(from, params.get("from") ?? older(to.value));
  // the oldest version has none older, so compare it with the one after it instead
  if (from.value === to.value) {
    const i = versions.findIndex((v) => v.version === to.value);
    if (i > 0) choose(to, versions[i - 1].version);
  }

  const apis = new Map();
  const api = (version) => {
    if (!apis.has(version)) {
      apis.set(
        version,
        fetch(byVersion.get(version).api).then((r) => {
          if (!r.ok) throw new Error(`${r.status} fetching the API of ${version}`);
          return r.json();
        }),
      );
    }
    return apis.get(version);
  };

  // a changed item's members, a record's fields or a variant's cases, as removed and added lines;
  // or, when its members didn't change, its definition before and after. The text is compared,
  // its highlighted HTML shown, its types linked into the version each line is from.
  const changes = (old, now) => {
    const was = new Set(old.members);
    const is = new Set(now.members);
    const html = (item, i) => item.members_html?.[i] ?? esc(item.members[i]);
    const removed = old.members.flatMap((m, i) => (is.has(m) ? [] : [["del", html(old, i)]]));
    const added = now.members.flatMap((m, i) => (was.has(m) ? [] : [["add", html(now, i)]]));
    const lines = removed.length || added.length
      ? [...removed, ...added]
      : [["del", old.def_html ?? esc(old.def)], ["add", now.def_html ?? esc(now.def)]];
    return `<div class="diff wit">${lines
      .map(([k, line]) => `<code class="diff-${k}"><span class="diff-mark" aria-hidden="true">${k === "del" ? "−" : "+"}</span><span class="visually-hidden">${k === "del" ? "removed: " : "added: "}</span>${line}</code>`)
      .join("")}</div>`;
  };

  const link = (c) => {
    const version = byVersion.get(c.version);
    if (!version || !c.interface) return null;
    return `${version.url}${encodeURIComponent(c.interface)}/${c.anchor ? `#${encodeURIComponent(c.anchor)}` : ""}`;
  };

  const entry = (c) => {
    const href = link(c);
    const label = `<code>${esc(c.label)}</code>`;
    // a type or function links to its definition, in its syntax color, with a tooltip; an
    // interface to its page
    const syntax = c.anchor ? (c.kind === "func" ? ' class="f"' : ' class="t"') : "";
    return `<li class="change">
      <span class="change-badge change-${c.change}">${esc(c.change)}</span>
      ${itemIcon(c.kind, c.now ?? c.old)}${href ? `<a${syntax} href="${esc(href)}">${label}</a>` : label}
      ${c.unstable ? `<span class="gate unstable" title="@unstable(feature = ${esc(c.unstable)})">unstable: ${esc(c.unstable)}</span>` : ""}
      ${c.caveat ? `<span class="change-caveat">${esc(c.caveat)}</span>` : ""}
      ${c.old && c.now ? changes(c.old, c.now) : ""}
    </li>`;
  };

  // changes grouped by where they are: an interface, a world, the component, or the dependencies
  const section = (id, title, list, empty) => {
    const groups = new Map();
    for (const c of list) {
      const group = c.interface
        ? `${icon("interface")}<code>${esc(c.interface)}</code>`
        : c.world
          ? `${icon("world")}<code>${esc(c.world)}</code>`
          : c.component
            ? `${icon("component")}<code>${esc(c.component)}</code>`
            : "Dependencies";
      if (!groups.has(group)) groups.set(group, []);
      groups.get(group).push(c);
    }
    const body = list.length
      ? [...groups]
          .map(([group, changes]) => `<h3 class="change-group">${group}</h3><ul class="changes">${changes.map(entry).join("")}</ul>`)
          .join("")
      : `<p class="dim">${esc(empty)}</p>`;
    return `<section class="compare-section" id="${id}"><h2>${esc(title)} <span class="count-chip">${list.length}</span></h2>${body}</section>`;
  };

  // the edits that turn the lines `a` into `b`, by Myers' algorithm, as `["=", line]`, `["-", line]`
  // and `["+", line]`, comparing the lines `key` gives
  const lineDiff = (a, b, key) => {
    const ka = a.map(key);
    const kb = b.map(key);
    const n = a.length;
    const m = b.length;
    const max = n + m;
    const off = max + 1;
    const v = new Int32Array(2 * max + 3);
    const trace = [];
    let found = -1;
    for (let d = 0; d <= max && found < 0; d++) {
      trace.push(v.slice());
      for (let k = -d; k <= d; k += 2) {
        let x = k === -d || (k !== d && v[off + k - 1] < v[off + k + 1]) ? v[off + k + 1] : v[off + k - 1] + 1;
        let y = x - k;
        while (x < n && y < m && ka[x] === kb[y]) x++, y++;
        v[off + k] = x;
        if (x >= n && y >= m) {
          found = d;
          break;
        }
      }
    }
    const edits = [];
    let x = n;
    let y = m;
    for (let d = found; d >= 0; d--) {
      const t = trace[d];
      const k = x - y;
      const prevK = k === -d || (k !== d && t[off + k - 1] < t[off + k + 1]) ? k + 1 : k - 1;
      const prevX = t[off + prevK];
      const prevY = prevX - prevK;
      while (x > prevX && y > prevY) edits.push(["=", b[--y], a[--x]]);
      if (d > 0) {
        if (x === prevX) edits.push(["+", b[--y]]);
        else edits.push(["-", a[--x]]);
      }
      x = prevX;
      y = prevY;
    }
    return edits.reverse();
  };

  // a version left out of a line, to compare lines whose dependencies' versions alone changed,
  // e.g. `wasi:io/streams@0.2.6.{error}`, but not a gate, e.g. `@since(version = 0.2.0)`
  const unversioned = (line) => line.replace(/@\d[0-9A-Za-z.+-]*/g, (v) => (v.endsWith(".") ? "." : ""));

  // a version's WIT, highlighted HTML, as lines of their HTML and their text, which the diff
  // compares; a line's tags never run over to the next, so the two split alike
  const witLines = (html) => {
    const parsed = document.createElement("div");
    parsed.innerHTML = html;
    const text = parsed.textContent.split("\n");
    return html.split("\n").map((line, i) => ({ html: line, text: text[i] ?? "" }));
  };

  // the WIT's changes with three lines around each, the lines between them folded. A removed
  // line is the earlier version's, linking into it; the others are the later version's.
  const CONTEXT = 3;
  const witDiff = (old, now, ignoreVersions) => {
    const edits = lineDiff(witLines(old), witLines(now), ignoreVersions ? (l) => unversioned(l.text) : (l) => l.text);
    if (!edits.some(([op]) => op !== "=")) return `<p class="dim">The WIT is the same${ignoreVersions ? ", but for its dependencies' versions" : ""}.</p>`;
    const near = edits.map(() => false);
    edits.forEach(([op], i) => {
      if (op === "=") return;
      for (let j = Math.max(0, i - CONTEXT); j <= Math.min(edits.length - 1, i + CONTEXT); j++) near[j] = true;
    });
    let html = "";
    let folded = 0;
    const fold = () => {
      if (folded) html += `<span class="wit-fold">⋯ ${folded} unchanged line${folded === 1 ? "" : "s"}</span>`;
      folded = 0;
    };
    edits.forEach(([op, line], i) => {
      if (!near[i]) return folded++;
      fold();
      const kind = op === "-" ? "del" : op === "+" ? "add" : "ctx";
      const mark = op === "=" ? " " : op === "-" ? "−" : "+";
      const said = op === "-" ? "removed: " : op === "+" ? "added: " : "";
      html += `<span class="wit-line wit-${kind}"><span class="diff-mark" aria-hidden="true">${mark}</span>${said ? `<span class="visually-hidden">${said}</span>` : ""}${line.html || " "}</span>`;
    });
    fold();
    return `<pre class="wit wit-diff"><code>${html}</code></pre>`;
  };

  // the sizes of the two versions' wasm layers, and of their debug builds', with how much each
  // grew or shrank
  const sizeSection = (a, b) => {
    // a wasm layer is never empty, so a size of 0 isn't known
    const row = (label, was, is) => {
      was ||= null;
      is ||= null;
      if (was == null && is == null) return "";
      if (was == null || is == null) {
        return `<dt>${esc(label)}</dt><dd>${was == null ? `<span class="dim">none</span> → <code>${esc(bytes(is))}</code>` : `<code>${esc(bytes(was))}</code> → <span class="dim">none</span>`}</dd>`;
      }
      const delta = is - was;
      const share = Math.abs((delta / was) * 100);
      const pct = was ? ` (${delta > 0 ? "+" : "−"}${share < 0.1 ? "<0.1" : share.toFixed(1)}%)` : "";
      const said = delta === 0 ? `<span class="dim">the same</span>` : `<span class="size-delta ${delta > 0 ? "size-up" : "size-down"}">${delta > 0 ? "+" : "−"}${esc(kib(delta))}${esc(pct)}</span>`;
      return `<dt>${esc(label)}</dt><dd><code>${esc(bytes(was))}</code> → <code>${esc(bytes(is))}</code> ${said}</dd>`;
    };
    const rows = [row("Wasm", a.size, b.size), row("Debug build", a.debug_size, b.debug_size)].join("");
    const dl = rows && `<dl class="facts compare-sizes">${rows}</dl>`;
    return `<section class="compare-section" id="compare-size"><h2>Size</h2>
      ${dl || `<p class="dim">Neither version's size is known.</p>`}</section>`;
  };

  // a version without a verified SBOM, said so, for the SBOM's and advisories' sections
  const noSbom = (a, b) => [a, b].filter((v) => !v.sbom).map((v) => `<code>${esc(v.version)}</code>`).join(" and ");

  // the packages the two versions' SBOMs list: those added, removed, upgraded and downgraded, by
  // their ecosystem and name
  const sbomSection = (a, b) => {
    const title = "SBOM packages";
    if (!a.sbom || !b.sbom) {
      return `<section class="compare-section" id="compare-sbom"><h2>${title}</h2><p class="dim">${noSbom(a, b)} ${!a.sbom && !b.sbom ? "have" : "has"} no verified SBOM to compare.</p></section>`;
    }
    const changes = sbomChanges(a, b);
    const body = changes.length
      ? `<ul class="changes">${changes
          .map((c) => `<li class="change"><span class="change-badge change-${c.change}">${esc(c.change)}</span> <code>${esc(c.label)}</code>${c.kind ? ` <span class="dim">${esc(c.kind)}</span>` : ""}${c.build_only ? ` <span class="badge sbom-build" title="Used to build it, e.g. a macro or a build script, rather than built into it">build</span>` : ""}</li>`)
          .join("")}</ul>`
      : `<p class="dim">The same packages, at the same versions.</p>`;
    return `<section class="compare-section" id="compare-sbom"><h2>${title} <span class="count-chip">${changes.length}</span></h2>
      <p class="dim">${a.sbom.packages.length} → ${b.sbom.packages.length} packages</p>${body}</section>`;
  };

  // the advisories of what the two versions' SBOMs list: those the later version is no longer
  // affected by, those new to it, and those that affect both, each linked to its page
  const advisoriesSection = (a, b) => {
    const title = "Advisories";
    if (!a.sbom || !b.sbom) {
      return `<section class="compare-section" id="compare-advisories"><h2>${title}</h2><p class="dim">${noSbom(a, b)} ${!a.sbom && !b.sbom ? "have" : "has"} no verified SBOM to compare.</p></section>`;
    }
    const { fixed, added, both } = advisoryChanges(a, b);
    const item = (x, change) => {
      const informational = x.kind !== "vulnerability";
      const bullet = informational
        ? `<span class="badge advisory-kind">${esc(x.kind)}</span>`
        : `<span class="sev-${esc(x.severity)}" title="${esc(x.severity)}"><svg class="sev-icon" aria-hidden="true" focusable="false"><use href="#sev-${esc(x.severity)}"/></svg><span class="visually-hidden">${esc(x.severity)}</span></span>`;
      return `<li class="change"><span class="change-badge change-${change}">${esc(change)}</span> ${bullet} <a href="/advisories/${encodeURIComponent(x.id)}/"><code>${esc(x.id)}</code></a> <span class="dim"><code>${esc(x.packages.join(", "))}</code></span>${x.build_only ? ` <span class="badge sbom-build" title="Used to build it, e.g. a macro or a build script, rather than built into it">build</span>` : ""}
        <span class="change-caveat">${esc(x.summary)}</span></li>`;
    };
    const group = (heading, list, change) =>
      list.length ? `<h3 class="change-group">${esc(heading)} <span class="count-chip">${list.length}</span></h3><ul class="changes">${list.map((x) => item(x, change)).join("")}</ul>` : "";
    // a version not fully checked may have advisories not known yet
    const unchecked = [a, b].filter((v) => !v.sbom.checked).map((v) => `<code>${esc(v.version)}</code>`);
    const caveat = unchecked.length ? `<p class="dim">Not every package ${unchecked.join(" and ")} lists is checked yet, so there may be more.</p>` : "";
    const body = fixed.length || added.length || both.length
      ? `${group(`New in ${b.version}`, added, "new")}${group(`Fixed in ${b.version}`, fixed, "fixed")}${group("Affecting both", both, "remaining")}`
      : `<p class="dim">None known of either version.</p>`;
    return `<section class="compare-section" id="compare-advisories"><h2>${title} <span class="count-chip" title="${b.sbom.advisories.length} affecting ${esc(b.version)}">${b.sbom.advisories.length}</span></h2>${caveat}${body}</section>`;
  };

  // the section a link into the page is to, e.g. `#compare-size` from a repository's comparison,
  // which is only there once the comparison is drawn: it's scrolled to then, the first time only,
  // so choosing other versions keeps the page where it is
  let anchor = decodeURIComponent(location.hash.slice(1));
  const scrollToAnchor = () => {
    if (!anchor) return;
    const target = document.getElementById(anchor);
    anchor = "";
    // below the pinned head, by the page's `scroll-padding-top`
    target?.scrollIntoView();
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
      const [old, now] = await Promise.all([api(a), api(b)]);
      if (run !== shown) return;
      const { breaking, cautions, notes } = diff(old, now);
      // how many of each, in the pinned head, each linking to its section
      const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;
      // each in a span of its own, so the dot between them, `.dots`, is outside its link
      const count = (id, kind, n, label) =>
        `<span><a class="compare-count compare-count-${kind}" href="#${id}" title="${esc(label)}" aria-label="${esc(label)}"><svg class="meta-icon" aria-hidden="true" focusable="false"><use href="#meta-${kind}"/></svg>${n}</a></span>`;
      counts.innerHTML = [
        count("compare-breaking", "stop", breaking.length, plural(breaking.length, "breaking change", "breaking changes")),
        count("compare-cautions", "caution", cautions.length, plural(cautions.length, "caution", "cautions")),
        count("compare-other", "info", notes.length, plural(notes.length, "other change", "other changes")),
      ].join("");
      counts.hidden = false;
      result.innerHTML = `
        ${section("compare-breaking", "Breaking changes", breaking, "Nothing removed or changed: a component using the earlier version works with this one.")}
        ${section("compare-cautions", "Cautions", cautions, "Nothing added, and no dependency's new major or minor version.")}
        ${section("compare-other", "Other changes", notes, "No deprecations, and the same dependencies.")}
        ${sizeSection(old, now)}
        ${old.sbom || now.sbom ? sbomSection(old, now) + advisoriesSection(old, now) : ""}
        <section class="compare-section" id="compare-wit">
          <h2>WIT</h2>
          <label class="wit-options"><input type="checkbox" data-ignore-versions checked> Ignore dependencies' versions</label>
          <div data-wit-diff>${witDiff(old.wit ?? "", now.wit ?? "", true)}</div>
        </section>`;
      result.querySelector("[data-ignore-versions]").addEventListener("change", (e) => {
        result.querySelector("[data-wit-diff]").innerHTML = witDiff(old.wit ?? "", now.wit ?? "", e.target.checked);
      });
      scrollToAnchor();
    } catch (err) {
      if (run !== shown) return;
      result.innerHTML = `<p class="notice">Couldn't compare the versions: ${esc(err.message)}</p>`;
    } finally {
      if (run === shown) result.removeAttribute("aria-busy");
    }
  };

  form.addEventListener("submit", (e) => e.preventDefault());
  form.querySelector("[data-compare-swap]").addEventListener("click", () => {
    [from.value, to.value] = [to.value, from.value];
    compare();
  });
  compare();
})();
