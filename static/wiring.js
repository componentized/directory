// Draws a composition's wiring as a block diagram, from left to right: the composition's imports,
// its parts, each in a column after the parts it takes an export of, and its exports. Each part has
// a port on its left for each of its imports, and one on its right for each of its exports that
// something plugs into; an edge joins each export to the imports it satisfies. An import the host
// provides is drawn faintly, as most parts take a few, until its part or port is pointed at.
//
// A part can be focused on: the diagram is drawn again with only it and the parts upstream of it,
// downstream of it, or both.
//
// The diagram pans by dragging and zooms with its buttons, the keyboard, or Ctrl and the wheel,
// and expands to fill the window. Its parts and ports are links, to their pages, with the
// tooltips any link to a version or an interface has, whatever the zoom.
(() => {
  const figure = document.querySelector("[data-wiring]");
  const source = document.getElementById("wiring-data");
  if (!figure || !source) return;
  const data = JSON.parse(source.textContent);
  const esc = window.directory?.esc ?? ((s) => String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`));
  const icon = window.directory?.icon ?? (() => "");
  const hideTip = () => window.directory?.hideTip?.();
  // what a part is called: its component's name, and the name the composition gives its instance
  const partName = (p) => `${p.name ?? "a component not in the directory"}${p.instance ? ` (${p.instance})` : ""}`;

  const viewport = figure.querySelector("[data-wiring-viewport]");
  const stage = figure.querySelector("[data-wiring-stage]");
  const expand = figure.querySelector("[data-wiring-expand]");
  const hint = figure.querySelector("[data-wiring-hint]");

  // what's shown: every part, or one part in focus with those upstream of it, that it takes from,
  // downstream of it, that take from it, or both, each followed as far as it goes
  const view = { focus: null, direction: "both" };
  // the parts each takes an export of, and those that take one of its
  const takesFrom = data.parts.map((p) => [...new Set(p.inputs.map((i) => i.from.part).filter((f) => f !== undefined))]);
  const takenBy = data.parts.map((_, n) => data.parts.flatMap((p, t) => (takesFrom[t].includes(n) ? [t] : [])));
  const reach = (start, next) => {
    const seen = new Set([start]);
    const queue = [start];
    while (queue.length) for (const n of next[queue.shift()]) if (!seen.has(n)) seen.add(n), queue.push(n);
    return seen;
  };
  const shownParts = () => {
    if (view.focus === null) return new Set(data.parts.keys());
    const up = view.direction === "down" ? new Set() : reach(view.focus, takesFrom);
    const down = view.direction === "up" ? new Set() : reach(view.focus, takenBy);
    return new Set([view.focus, ...up, ...down]);
  };

  // the wiring that's shown: the parts, their imports from the parts shown or from the host, their
  // exports that something shown takes, and the composition's own imports and exports that
  // something shown plugs into
  const shown = () => {
    const parts = shownParts();
    const plugged = (f) => (f.part === undefined || parts.has(f.part));
    const inputs = new Map([...parts].map((n) => [n, data.parts[n].inputs.filter((i) => plugged(i.from))]));
    const exports = data.exports.filter((e) => plugged(e.from));
    const taken = [...inputs.values(), exports].flat().map((i) => i.from);
    const outputs = new Map([...parts].map((n) => [n, data.parts[n].outputs.filter((o) => taken.some((f) => f.part === n && f.port === o.name))]));
    const imports = data.imports.filter((i) => taken.some((f) => f.import === i.name));
    return { parts: [...parts].sort((a, b) => a - b), inputs, outputs, imports, exports };
  };

  // each edge, from an export to the import it satisfies, and the nodes it joins
  let edges = [];
  let svg = null;
  // over the parts, each edge's band that the pointer finds it by, wider than it's drawn
  let hits = null;
  // lays out what's shown, from scratch
  const build = () => {
    const w = shown();
    const has = (f) => f.part !== undefined && w.inputs.has(f.part);
    // each part's column: after the parts it takes an export of, the first for one that takes only
    // the composition's imports. The parts come in the order they're made, so a part's sources come
    // before it.
    const column = new Map();
    for (const n of w.parts) {
      const after = w.inputs.get(n).map((i) => (has(i.from) ? column.get(i.from.part) + 1 : 0));
      column.set(n, Math.max(0, ...after));
    }
    // then each part that something takes an export of moves up to just before the first of
    // those, so a part only the last takes from, e.g. one of a few latches combined, sits beside it
    // rather than a long edge away. The later parts are placed first, as they're what the earlier
    // ones move up to.
    for (const n of [...w.parts].reverse()) {
      const takers = w.parts.filter((t) => w.inputs.get(t).some((i) => i.from.part === n)).map((t) => column.get(t));
      if (takers.length) column.set(n, Math.max(column.get(n), Math.min(...takers) - 1));
    }
    // a column a part moved out of, left empty, is closed up
    const used = [...new Set(column.values())].sort((a, b) => a - b);
    const columns = used.map(() => []);
    for (const n of w.parts) columns[used.indexOf(column.get(n))].push(n);
    // each edge, from an export to the import it satisfies, and the nodes it joins; the
    // composition's imports are a column before its parts, and its exports one after
    const last = columns.length;
    const columnOf = (n) => used.indexOf(column.get(n));
    edges = [];
    const from = (f) =>
      f.part === undefined
        ? { from: `imp:${f.import}`, node: "imports", item: `imp:${f.import}`, column: -1, host: true }
        : { from: `out:${f.part}:${f.port}`, node: `part:${f.part}`, item: `part:${f.part}`, column: columnOf(f.part), host: false };
    const edge = (f, to, node, item, at) => {
      const source = from(f);
      // an edge that passes columns has a lane in each, which the parts there are placed around and
      // it's drawn along, rather than behind them; the edges from one port share their lanes
      const via = [];
      for (let c = source.column + 1; c < at; c++) via.push(`lane:${source.from}@${c}`);
      edges.push({ from: source.from, to, nodes: [source.node, node], host: source.host, chain: [source.item, ...via, item], via });
    };
    for (const n of w.parts) for (const i of w.inputs.get(n)) edge(i.from, `in:${n}:${i.name}`, `part:${n}`, `part:${n}`, columnOf(n));
    for (const e of w.exports) edge(e.from, `exp:${e.name}`, "exports", `exp:${e.name}`, last);

    // the items of each column, its parts and the lanes through it, and their neighbors either side
    const items = columns.map((parts) => parts.map((n) => `part:${n}`));
    const before = new Map();
    const after = new Map();
    const link = (map, k, v) => (map.get(k) ?? map.set(k, []).get(k)).push(v);
    for (const e of edges) {
      e.via.forEach((lane) => {
        const c = Number(lane.slice(lane.lastIndexOf("@") + 1));
        if (!items[c].includes(lane)) items[c].push(lane);
      });
      for (let k = 1; k < e.chain.length; k++) {
        link(before, e.chain[k], e.chain[k - 1]);
        link(after, e.chain[k - 1], e.chain[k]);
      }
    }
    // within a column, the items in the order of the rows of their neighbors, so edges cross less:
    // sweeping forward by what each takes from, back by what takes from it, and forward again
    const row = new Map([...w.imports.map((i, r) => [`imp:${i.name}`, r]), ...w.exports.map((e, r) => [`exp:${e.name}`, r])]);
    items.forEach((list) => list.forEach((k, r) => row.set(k, r)));
    const order = (list, neighbors) => {
      const center = new Map(
        list.map((k, r) => {
          const rows = (neighbors.get(k) ?? []).map((n) => row.get(n)).filter((v) => v !== undefined);
          return [k, rows.length ? rows.reduce((a, b) => a + b, 0) / rows.length : r];
        }),
      );
      list.sort((a, b) => center.get(a) - center.get(b));
      list.forEach((k, r) => row.set(k, r));
    };
    for (let sweep = 0; sweep < 3; sweep++) {
      items.forEach((list) => order(list, before));
      [...items].reverse().forEach((list) => order(list, after));
    }
    items.forEach((list) => order(list, before));

    // a port: a link to its interface's page, when it has one, with a dot on the edge it's joined
    const port = (key, p, side) => {
      const label = `<code>${esc(p.label)}</code>`;
      const attrs = `class="wiring-port wiring-port-${side}" data-key="${esc(key)}"`;
      return p.url
        ? `<a ${attrs} href="${esc(p.url)}" draggable="false">${label}</a>`
        : `<span ${attrs} tabindex="0">${label}</span>`;
    };
    const ports = (list, side, key) =>
      list.length ? `<div class="wiring-ports wiring-ports-${side}">${list.map((p) => port(key(p), p, side)).join("")}</div>` : "";
    const part = (n) => {
      const p = data.parts[n];
      const [, version] = p.label?.split("@") ?? [];
      const component = p.url
        ? `<a href="${esc(p.url)}" data-tip draggable="false"><code>${esc(p.name)}</code></a>${version ? `<span class="wiring-version">@${esc(version)}</span>` : ""}`
        : `<span class="dim">Not in the directory</span>`;
      // the component it's an instance of, and under it the name the composition gives the
      // instance, e.g. `stderr`, when it gives one
      const title = p.instance ? `${component}<code class="wiring-instance">${esc(p.instance)}</code>` : component;
      const focused = view.focus === n;
      const name = partName(p);
      return `<div class="wiring-node${focused ? " is-focus" : ""}" data-node="part:${n}">
        <div class="wiring-node-head">${icon(p.kind)}<span class="wiring-title">${title}</span>
          <button type="button" class="wiring-focus-button" data-wiring-focus-part="${n}" aria-pressed="${focused}" aria-label="${esc(focused ? `Stop focusing on ${name}` : `Focus on ${name}`)}"><svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="5"/><circle cx="8" cy="8" r="1.5"/><path d="M8 1v2.5M8 12.5V15M1 8h2.5M12.5 8H15"/></svg></button>
        </div>
        <div class="wiring-node-ports">${ports(w.inputs.get(n), "in", (i) => `in:${n}:${i.name}`)}${ports(w.outputs.get(n), "out", (o) => `out:${n}:${o.name}`)}</div>
      </div>`;
    };
    const boundary = (what, list, side, key) =>
      list.length
        ? `<div class="wiring-boundary wiring-boundary-${side}" data-node="${what}"><div class="wiring-boundary-head dim">${what === "imports" ? "Imports" : "Exports"}</div>${ports(list, side, key)}</div>`
        : "";
    stage.innerHTML = `<svg class="wiring-edges" aria-hidden="true"></svg>
      ${boundary("imports", w.imports, "out", (i) => `imp:${i.name}`)}
      <div class="wiring-frame"><div class="wiring-frame-head"><code>${esc(data.name)}</code></div>
        <div class="wiring-columns">${items
          .map((list) => `<div class="wiring-column">${list.map((k) => (k.startsWith("lane:") ? `<div class="wiring-lane" data-lane="${esc(k)}"></div>` : part(Number(k.slice(5))))).join("")}</div>`)
          .join("")}</div>
      </div>
      ${boundary("exports", w.exports, "in", (e) => `exp:${e.name}`)}
      <svg class="wiring-edges wiring-hits" aria-hidden="true"></svg>`;
    svg = stage.querySelector(".wiring-edges");
    hits = stage.querySelector(".wiring-hits");

  };

  const at = (key) => stage.querySelector(`[data-key="${CSS.escape(key)}"]`);
  let scale = 1;
  // the edges, between the dots of their ports, measured in the stage's own units, unscaled
  const draw = () => {
    const box = stage.getBoundingClientRect();
    const point = (el, side) => {
      const r = el.getBoundingClientRect();
      const x = side === "out" ? r.right : r.left;
      return [(x - box.left) / scale, (r.top + r.height / 2 - box.top) / scale];
    };
    for (const layer of [svg, hits]) {
      layer.setAttribute("width", stage.scrollWidth);
      layer.setAttribute("height", stage.scrollHeight);
    }
    const paths = edges
      .map((e, i) => {
        const a = at(e.from);
        const b = at(e.to);
        if (!a || !b) return "";
        // from port to port through each lane on the way: curving between columns, straight along
        // a lane
        const points = [point(a, "out")];
        for (const key of e.via) {
          const lane = stage.querySelector(`[data-lane="${CSS.escape(key)}"]`);
          if (lane) points.push(point(lane, "in"), point(lane, "out"));
        }
        points.push(point(b, "in"));
        let d = `M${points[0].join(" ")}`;
        for (let k = 0; k + 1 < points.length; k += 2) {
          const [[x1, y1], [x2, y2]] = [points[k], points[k + 1]];
          const dx = Math.max(24, Math.abs(x2 - x1) / 2);
          d += `C${x1 + dx} ${y1} ${x2 - dx} ${y2} ${x2} ${y2}`;
          if (points[k + 2]) d += `L${points[k + 2].join(" ")}`;
        }
        return d;
      })
      .map((d, i) => [d, i]);
    svg.innerHTML = paths
      .map(([d, i]) => d && `<path data-edge="${i}" class="wiring-edge${edges[i].host ? " wiring-edge-host" : ""}" d="${d}"/>`)
      .join("");
    hits.innerHTML = paths.map(([d, i]) => d && `<path data-hit="${i}" class="wiring-hit" d="${d}"/>`).join("");
  };

  // pointing at a part, or a port, picks out its edges, the ports they join, and their parts
  const highlight = (target) => {
    const el = target?.closest?.("[data-key], [data-node]");
    for (const on of stage.querySelectorAll(".is-on")) on.classList.remove("is-on");
    figure.classList.toggle("wiring-picking", !!el);
    if (!el) return;
    const key = el.dataset.key;
    const node = el.closest("[data-node]")?.dataset.node;
    edges.forEach((e, i) => {
      if (key ? e.from !== key && e.to !== key : !e.nodes.includes(node)) return;
      svg.querySelector(`[data-edge="${i}"]`)?.classList.add("is-on");
      for (const k of [e.from, e.to]) at(k)?.classList.add("is-on");
    });
    el.classList.add("is-on");
  };
  // pointing at an edge is pointing at the port it comes from: its edges, all of them, stand out
  stage.addEventListener("pointerover", (e) => {
    const hit = e.target.closest?.("[data-hit]");
    highlight(hit ? at(edges[hit.dataset.hit].from) : e.target);
  });
  stage.addEventListener("pointerleave", () => highlight(null));
  stage.addEventListener("focusin", (e) => highlight(e.target));
  stage.addEventListener("focusout", (e) => stage.contains(e.relatedTarget) || highlight(null));

  // the view: the stage moved by x and y, and scaled
  const MIN = 0.2;
  const MAX = 3;
  let x = 0;
  let y = 0;
  let moved = false;
  const apply = () => {
    stage.style.transform = `translate(${x}px, ${y}px) scale(${scale})`;
    hideTip();
  };
  const size = () => [stage.scrollWidth, stage.scrollHeight];
  // the whole diagram in view, centered, no larger than its own size; inline, no smaller than it
  // reads at either, the rest of it a pan away
  const fit = (readable) => {
    const [w, h] = size();
    const v = viewport.getBoundingClientRect();
    const whole = Math.min(1, (v.width - 24) / w, (v.height - 24) / h);
    scale = Math.min(MAX, Math.max(MIN, readable ? Math.max(whole, 0.65) : whole));
    x = Math.max(12, (v.width - w * scale) / 2);
    y = Math.max(12, (v.height - h * scale) / 2);
    if (readable && w * scale > v.width - 24) x = 12;
    moved = false;
    apply();
  };
  const zoom = (factor, cx, cy) => {
    const v = viewport.getBoundingClientRect();
    const px = cx ?? v.width / 2;
    const py = cy ?? v.height / 2;
    const next = Math.min(MAX, Math.max(MIN, scale * factor));
    x = px - (px - x) * (next / scale);
    y = py - (py - y) * (next / scale);
    scale = next;
    moved = true;
    apply();
  };
  // inline, the view is as tall as the diagram at its width, up to most of the window
  const height = () => {
    if (figure.classList.contains("wiring-expanded")) {
      viewport.style.height = "";
      return;
    }
    const [w, h] = size();
    const k = Math.min(1, Math.max(0.65, (viewport.clientWidth - 24) / w));
    viewport.style.height = `${Math.min(h * k + 24, innerHeight * 0.7)}px`;
  };

  for (const button of figure.querySelectorAll("[data-wiring-zoom]")) {
    button.addEventListener("click", () => {
      const how = button.dataset.wiringZoom;
      if (how === "fit") fit(false);
      else zoom(how === "in" ? 1.25 : 0.8);
    });
  }
  // the wheel scrolls the page, unless it's held with Ctrl or ⌘, as a trackpad's pinch is, or the
  // diagram fills the window
  viewport.addEventListener(
    "wheel",
    (e) => {
      if (!(e.ctrlKey || e.metaKey || figure.classList.contains("wiring-expanded"))) return;
      e.preventDefault();
      const v = viewport.getBoundingClientRect();
      zoom(Math.exp(-e.deltaY * (e.deltaMode === 1 ? 0.05 : 0.002)), e.clientX - v.left, e.clientY - v.top);
    },
    { passive: false },
  );
  viewport.addEventListener("keydown", (e) => {
    if (e.target !== viewport) return;
    const step = 40;
    const keys = {
      "+": () => zoom(1.25),
      "=": () => zoom(1.25),
      "-": () => zoom(0.8),
      0: () => fit(false),
      ArrowLeft: () => (x += step),
      ArrowRight: () => (x -= step),
      ArrowUp: () => (y += step),
      ArrowDown: () => (y -= step),
    };
    if (!keys[e.key]) return;
    e.preventDefault();
    keys[e.key]();
    moved = true;
    apply();
  });

  // dragging pans, with one pointer, and pinching zooms, with two; a drag that starts on a link
  // doesn't follow it
  const pointers = new Map();
  let dragged = false;
  let pinch = null;
  viewport.addEventListener("dragstart", (e) => e.preventDefault());
  viewport.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    pointers.set(e.pointerId, [e.clientX, e.clientY]);
    dragged = false;
    if (pointers.size === 2) {
      const [[ax, ay], [bx, by]] = [...pointers.values()];
      pinch = Math.hypot(ax - bx, ay - by);
    }
  });
  viewport.addEventListener("pointermove", (e) => {
    const last = pointers.get(e.pointerId);
    if (!last) return;
    const dx = e.clientX - last[0];
    const dy = e.clientY - last[1];
    if (!dragged && Math.hypot(dx, dy) < 4) return;
    if (!dragged) {
      dragged = true;
      viewport.setPointerCapture(e.pointerId);
      viewport.classList.add("wiring-dragging");
    }
    pointers.set(e.pointerId, [e.clientX, e.clientY]);
    if (pointers.size === 2 && pinch) {
      const [[ax, ay], [bx, by]] = [...pointers.values()];
      const distance = Math.hypot(ax - bx, ay - by);
      const v = viewport.getBoundingClientRect();
      zoom(distance / pinch, (ax + bx) / 2 - v.left, (ay + by) / 2 - v.top);
      pinch = distance;
      return;
    }
    x += dx;
    y += dy;
    moved = true;
    apply();
  });
  const release = (e) => {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinch = null;
    if (!pointers.size) viewport.classList.remove("wiring-dragging");
  };
  viewport.addEventListener("pointerup", release);
  viewport.addEventListener("pointercancel", release);
  viewport.addEventListener("click", (e) => {
    if (!dragged) return;
    e.preventDefault();
    e.stopPropagation();
    dragged = false;
  }, true);

  // the focus, and what's shown with it, said in the toolbar, where it's changed or cleared
  const bar = figure.querySelector("[data-wiring-focus]");
  const rebuild = (refocus) => {
    highlight(null);
    build();
    draw();
    fit(!figure.classList.contains("wiring-expanded"));
    if (view.focus !== null) {
      const p = data.parts[view.focus];
      bar.querySelector("[data-wiring-focus-name]").textContent = partName(p);
    }
    bar.hidden = view.focus === null;
    for (const b of bar.querySelectorAll("[data-wiring-direction]")) b.setAttribute("aria-pressed", String(b.dataset.wiringDirection === view.direction));
    figure.classList.toggle("wiring-focused", view.focus !== null);
    // the keyboard stays where it was, on the part's button, which was drawn again
    if (refocus) stage.querySelector(refocus)?.focus({ preventScroll: true });
  };
  const focusOn = (n) => {
    view.focus = view.focus === n ? null : n;
    rebuild(`[data-wiring-focus-part="${n}"]`);
  };
  stage.addEventListener("click", (e) => {
    const button = e.target.closest("[data-wiring-focus-part]");
    if (button) focusOn(Number(button.dataset.wiringFocusPart));
  });
  for (const b of bar.querySelectorAll("[data-wiring-direction]")) {
    b.addEventListener("click", () => {
      view.direction = b.dataset.wiringDirection;
      rebuild();
    });
  }
  bar.querySelector("[data-wiring-clear]").addEventListener("click", () => {
    const n = view.focus;
    view.focus = null;
    rebuild(`[data-wiring-focus-part="${n}"]`);
  });

  // expanded, the diagram fills the window, over the page, until it's collapsed or Escape
  const setExpanded = (on) => {
    figure.classList.toggle("wiring-expanded", on);
    document.documentElement.classList.toggle("wiring-locked", on);
    expand.setAttribute("aria-pressed", String(on));
    expand.title = on ? "Collapse" : "Expand";
    expand.setAttribute("aria-label", expand.title);
    hint.textContent = on ? "Drag to pan · Scroll to zoom · Esc to close" : "Drag to pan · Ctrl + scroll to zoom";
    height();
    fit(!on);
    if (on) viewport.focus({ preventScroll: true });
  };
  expand.addEventListener("click", () => setExpanded(!figure.classList.contains("wiring-expanded")));
  // Escape clears the focus first, then collapses the diagram
  addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    const expanded = figure.classList.contains("wiring-expanded");
    if (view.focus !== null && (expanded || figure.contains(document.activeElement))) {
      const n = view.focus;
      view.focus = null;
      rebuild(`[data-wiring-focus-part="${n}"]`);
    } else if (expanded) setExpanded(false);
  });

  figure.hidden = false;
  build();
  draw();
  height();
  fit(true);
  // a resize, or the fonts arriving, moves the ports: the edges follow, and the view refits
  // unless it has been moved
  new ResizeObserver(() => {
    draw();
    height();
    if (!moved) fit(!figure.classList.contains("wiring-expanded"));
  }).observe(viewport);
  document.fonts?.ready.then(() => {
    draw();
    if (!moved) fit(!figure.classList.contains("wiring-expanded"));
  });
})();
