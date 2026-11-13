// Compares two versions of a package from their APIs, the `api.json` the build writes for each
// version, so a package has one compare page rather than a page for each pair of its versions.
//
// What can break a component upgrading from one version to the other comes first: WIT types are
// structural, with no subtyping, so a removed or changed type or function breaks whoever uses
// it, importing or implementing it. An addition is listed apart: it breaks only a component that
// implements, exports, the interface, which then has to provide it too. Dependencies' versions are
// compared on their own; a type is referred to by its name alone, so a dependency's new version
// doesn't change every item that uses it.
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
  const keys = (a, b) => [...new Set([...Object.keys(a ?? {}), ...Object.keys(b ?? {})])].sort();

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

  // the differences between two APIs, `a` the version upgraded from and `b` the one upgraded to
  const diff = (a, b) => {
    const breaking = [];
    const cautions = [];
    const notes = [];
    for (const name of keys(a.interfaces, b.interfaces)) {
      const was = a.interfaces[name];
      const is = b.interfaces[name];
      const where = { interface: name };
      if (!is) {
        const unstable = was.unstable;
        (unstable ? cautions : breaking).push({ ...where, change: "removed", kind: "interface", label: name, version: a.version, unstable, caveat: gated(unstable, null) });
        continue;
      }
      if (!was) {
        cautions.push({ ...where, change: "added", kind: "interface", label: name, version: b.version, unstable: is.unstable });
        continue;
      }
      if (was.unstable && !is.unstable) {
        notes.push({ ...where, change: "stabilized", kind: "interface", label: name, version: b.version, caveat: STABILIZED(was.unstable) });
      }
      if (!was.deprecated && is.deprecated) {
        notes.push({ ...where, change: "deprecated", kind: "interface", label: name, version: b.version });
      }
      for (const key of keys(was.items, is.items)) {
        const old = was.items[key];
        const now = is.items[key];
        const item = { ...where, key, label: key };
        // an item of an unstable interface is as unstable as it is
        const wasUnstable = old && (old.unstable ?? was.unstable);
        const isUnstable = now && (now.unstable ?? is.unstable);
        if (!now) {
          (wasUnstable ? cautions : breaking).push({ ...item, change: "removed", kind: old.kind, anchor: old.anchor, version: a.version, old, unstable: wasUnstable, caveat: gated(wasUnstable, null) });
        } else if (!old) {
          cautions.push({
            ...item,
            change: "added",
            kind: now.kind,
            anchor: now.anchor,
            version: b.version,
            now,
            unstable: isUnstable,
            caveat: now.kind === "func" ? gated(isUnstable, "a component exporting the interface must implement it") : null,
          });
        } else if (old.def !== now.def) {
          // a change to an item that was unstable breaks only a component using its feature
          (wasUnstable ? cautions : breaking).push({ ...item, change: "changed", kind: now.kind, anchor: now.anchor, version: b.version, old, now, unstable: wasUnstable, caveat: gated(wasUnstable, null) });
        } else if (wasUnstable && !isUnstable) {
          notes.push({ ...item, change: "stabilized", kind: now.kind, anchor: now.anchor, version: b.version, now, caveat: STABILIZED(wasUnstable) });
        } else if (!old.deprecated && now.deprecated) {
          notes.push({ ...item, change: "deprecated", kind: now.kind, anchor: now.anchor, version: b.version, now, unstable: isUnstable });
        }
      }
    }

    // a world's imports and exports: a removed one, or a new export a component targeting the
    // world must provide, can break it; a new import is optional for it, but a host must provide it.
    // One behind a feature gate, `@unstable(feature = ..)`, is only there when the feature is, so
    // it asks something only of a host or component with the feature, and breaks nothing else
    for (const name of keys(a.worlds, b.worlds)) {
      const was = a.worlds[name];
      const is = b.worlds[name];
      const where = { world: name };
      if (!is) {
        breaking.push({ ...where, change: "removed", kind: "world", label: name, version: a.version });
        continue;
      }
      if (!was) {
        cautions.push({ ...where, change: "added", kind: "world", label: name, version: b.version });
        continue;
      }
      for (const side of ["imports", "exports"]) {
        refs(was[side], is[side], (ref, change, old) => {
          const entry = { ...where, change, kind: ref.label.endsWith(": func") ? "func" : "interface", label: `${side === "imports" ? "import" : "export"} ${ref.label}`, unstable: ref.unstable };
          if (change === "stabilized") notes.push({ ...entry, unstable: null, caveat: STABILIZED(old.unstable) });
          else if (ref.types_only) notes.push({ ...entry, caveat: TYPES_ONLY });
          else if (change === "removed" || side === "exports") {
            const caveat = side === "exports" && change === "added" ? "a component targeting the world must export it" : null;
            (ref.unstable ? cautions : breaking).push({ ...entry, caveat: gated(ref.unstable, caveat) });
          } else cautions.push({ ...entry, caveat: gated(ref.unstable, "a host must provide it") });
        });
      }
    }

    // a component's imports and exports: a new import needs a host to provide it, and a removed
    // export breaks whoever uses it
    if (b.kind === "component") {
      const where = { component: b.package };
      refs(a.imports, b.imports, (ref, change) => {
        const entry = { ...where, change, kind: "interface", label: `import ${ref.label}` };
        if (ref.types_only) notes.push({ ...entry, caveat: TYPES_ONLY });
        else if (change === "added") breaking.push({ ...entry, caveat: "a host must provide it" });
        else cautions.push({ ...entry, caveat: "a host no longer needs to provide it" });
      });
      refs(a.exports, b.exports, (ref, change) => {
        const entry = { ...where, change, kind: "interface", label: `export ${ref.label}` };
        if (ref.types_only) notes.push({ ...entry, caveat: TYPES_ONLY });
        else if (change === "removed") breaking.push(entry);
        else cautions.push(entry);
      });
    }

    // the dependencies' versions: a new major or minor version is a caution, as it may change
    // what the dependency provides, a new patch is informational
    const deps = (list) => new Map(list.map((d) => [d.slice(0, d.lastIndexOf("@")), d]));
    const versionOf = (dep) => dep.slice(dep.lastIndexOf("@") + 1);
    const wasDeps = deps(a.dependencies);
    const isDeps = deps(b.dependencies);
    for (const pkg of [...new Set([...wasDeps.keys(), ...isDeps.keys()])].sort()) {
      const was = wasDeps.get(pkg);
      const is = isDeps.get(pkg);
      if (was === is) continue;
      const entry = {
        dependency: true,
        change: !was ? "added" : !is ? "removed" : "upgraded",
        kind: "interface",
        label: !was ? is : !is ? was : `${was} → ${versionOf(is)}`,
      };
      const step = was && is ? release(versionOf(was), versionOf(is)) : null;
      if (step === "major" || step === "minor") {
        cautions.push({ ...entry, caveat: `a new ${step} version, which may change what it provides` });
      } else {
        notes.push(entry);
      }
    }
    return { breaking, cautions, notes };
  };

  // an interface of types alone, with no functions or resources of its own, asks nothing of a
  // host: there's nothing to call, so importing or exporting it is informational
  const TYPES_ONLY = "types only: there's nothing to provide or call, so nothing has to supply it";

  // what a change asks, when it's behind a feature gate: only with the feature enabled. A change
  // that would otherwise break, e.g. a removal, breaks only a component using the feature
  const gated = (feature, caveat) => {
    if (!feature) return caveat;
    if (!caveat) return `unstable, so it breaks only a component using the ${feature} feature`;
    return `${caveat} when the ${feature} feature is enabled`;
  };
  const STABILIZED = (feature) => `no longer behind the ${feature} feature`;

  // how far apart two versions are, `major`, `minor` or `patch`, by their `major.minor.patch`,
  // or `null` when they share it, e.g. two pre-releases of one version
  function release(a, b) {
    const parts = (v) => v.split(/[-+]/)[0].split(".").map((n) => parseInt(n, 10) || 0);
    const [x, y] = [parts(a), parts(b)];
    if (x[0] !== y[0]) return "major";
    if (x[1] !== y[1]) return "minor";
    if (x[2] !== y[2]) return "patch";
    return null;
  }

  // calls back with each ref only one side has, by its key, the ref without its version, and each
  // both have that's no longer behind a feature gate, with the ref it was
  function refs(was = [], is = [], each) {
    const wasKeys = new Map(was.map((r) => [r.key, r]));
    const isKeys = new Map(is.map((r) => [r.key, r]));
    for (const [key, ref] of wasKeys) if (!isKeys.has(key)) each(ref, "removed");
    for (const [key, ref] of isKeys) {
      const old = wasKeys.get(key);
      if (!old) each(ref, "added");
      else if (old.unstable && !ref.unstable) each(ref, "stabilized", old);
    }
  }

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
      const count = (id, kind, n, label) =>
        `<a class="compare-count compare-count-${kind}" href="#${id}" title="${esc(label)}" aria-label="${esc(label)}"><svg class="meta-icon" aria-hidden="true" focusable="false"><use href="#meta-${kind}"/></svg>${n}</a>`;
      counts.innerHTML = [
        count("compare-breaking", "stop", breaking.length, plural(breaking.length, "breaking change", "breaking changes")),
        count("compare-cautions", "caution", cautions.length, plural(cautions.length, "caution", "cautions")),
        count("compare-other", "info", notes.length, plural(notes.length, "other change", "other changes")),
      ].join('<span class="dim" aria-hidden="true">·</span>');
      counts.hidden = false;
      result.innerHTML = `
        ${section("compare-breaking", "Breaking changes", breaking, "Nothing removed or changed: a component using the earlier version works with this one.")}
        ${section("compare-cautions", "Cautions", cautions, "Nothing added, and no dependency's new major or minor version.")}
        ${section("compare-other", "Other changes", notes, "No deprecations, and the same dependencies.")}
        <section class="compare-section" id="compare-wit">
          <h2>WIT</h2>
          <label class="wit-options"><input type="checkbox" data-ignore-versions checked> Ignore dependencies' versions</label>
          <div data-wit-diff>${witDiff(old.wit ?? "", now.wit ?? "", true)}</div>
        </section>`;
      result.querySelector("[data-ignore-versions]").addEventListener("change", (e) => {
        result.querySelector("[data-wit-diff]").innerHTML = witDiff(old.wit ?? "", now.wit ?? "", e.target.checked);
      });
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
