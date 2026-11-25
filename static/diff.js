// What changed between two versions of a package, from their APIs, the `api.json` the build writes
// for each version: their APIs' breaking changes, cautions and other changes, the packages their
// SBOMs list, and their advisories. Shared by a package's compare page and a repository's, which
// sums up each of its packages.
//
// What can break a component upgrading from one version to the other comes first: WIT types are
// structural, with no subtyping, so a removed or changed type or function breaks whoever uses
// it, importing or implementing it. An addition is listed apart: it breaks only a component that
// implements, exports, the interface, which then has to provide it too. Dependencies' versions are
// compared on their own; a type is referred to by its name alone, so a dependency's new version
// doesn't change every item that uses it.
(() => {
  const keys = (a, b) => [...new Set([...Object.keys(a ?? {}), ...Object.keys(b ?? {})])].sort();

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
        // a package depended on, by its icon apart from an interface
        kind: "package",
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

  // a size in bytes as a version's page says it, e.g. `12.3 KiB`
  const bytes = (n) => {
    const abs = Math.abs(n);
    if (abs >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
    if (abs >= 1024) return `${(n / 1024).toFixed(1)} KiB`;
    return `${n} B`;
  };
  // a change in size, always in KiB so changes compare at a glance, e.g. `0.5 KiB` for 463 bytes
  // and `27.5 KiB`; one too small to show at a tenth of a KiB is said as less than that
  const kib = (n) => {
    const k = Math.abs(n) / 1024;
    return k > 0 && k < 0.05 ? "<0.1 KiB" : `${k.toLocaleString("en", { minimumFractionDigits: 1, maximumFractionDigits: 1 })} KiB`;
  };

  // orders versions by their `major.minor.patch`, a pre-release before its release, else as text
  const cmpVersion = (x, y) => {
    const parse = (v) => {
      const [core, pre] = v.split("+")[0].split(/-(.*)/s);
      return { nums: core.split(".").map((n) => parseInt(n, 10) || 0), pre: pre ?? null };
    };
    const [p, q] = [parse(x), parse(y)];
    for (let i = 0; i < Math.max(p.nums.length, q.nums.length); i++) {
      const d = (p.nums[i] ?? 0) - (q.nums[i] ?? 0);
      if (d) return d;
    }
    if (p.pre === q.pre) return 0;
    if (p.pre === null) return 1;
    if (q.pre === null) return -1;
    return p.pre.localeCompare(q.pre, undefined, { numeric: true });
  };

  // the packages two versions' SBOMs list that were added, removed, upgraded or downgraded, by
  // their ecosystem and name, the removed first; null when either has no verified SBOM
  const sbomChanges = (a, b) => {
    if (!a.sbom || !b.sbom) return null;
    const byKey = (sbom) => new Map(sbom.packages.map((p) => [`${p.kind}/${p.name}`, p]));
    const [was, is] = [byKey(a.sbom), byKey(b.sbom)];
    const at = (p) => p.versions.join(", ");
    const changes = [];
    for (const key of [...new Set([...was.keys(), ...is.keys()])].sort()) {
      const old = was.get(key);
      const now = is.get(key);
      const p = now ?? old;
      const label = !old ? `${p.name}@${at(now)}` : !now ? `${p.name}@${at(old)}` : `${p.name} ${at(old)} → ${at(now)}`;
      let change = !old ? "added" : !now ? "removed" : null;
      if (!change && at(old) !== at(now)) {
        change = cmpVersion(now.versions.at(-1) ?? "", old.versions.at(-1) ?? "") >= 0 ? "upgraded" : "downgraded";
      }
      if (change) changes.push({ change, label, kind: p.kind, build_only: p.build_only });
    }
    const order = ["removed", "downgraded", "upgraded", "added"];
    return changes.sort((x, y) => order.indexOf(x.change) - order.indexOf(y.change));
  };

  // the advisories of what two versions' SBOMs list: those the later one is fixed in, those new
  // to it, and those affecting both; null when either has no verified SBOM
  const advisoryChanges = (a, b) => {
    if (!a.sbom || !b.sbom) return null;
    const was = new Map(a.sbom.advisories.map((x) => [x.id, x]));
    const is = new Map(b.sbom.advisories.map((x) => [x.id, x]));
    return {
      fixed: [...was.values()].filter((x) => !is.has(x.id)),
      added: [...is.values()].filter((x) => !was.has(x.id)),
      both: [...is.values()].filter((x) => was.has(x.id)),
    };
  };

  (window.directory ??= {}).diff = { diff, bytes, kib, cmpVersion, sbomChanges, advisoryChanges };
})();
