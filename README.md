# Componentized Directory <!-- omit in toc -->

A directory of the Wasm components and WIT interfaces published by the Componentized project to [`ghcr.io/componentized`](https://github.com/orgs/componentized/packages). Every package, version, world, interface, type and function is browsable, along with what imports, exports and depends on it. It is served at [directory.componentized.dev](https://directory.componentized.dev).

- [How it works](#how-it-works)
- [The index](#the-index)
- [Commands](#commands)
- [Workflows](#workflows)
- [Setup](#setup)
- [License](#license)

## How it works

1. `directory scan` finds the org's packages. It reads the GitHub Packages API when the token may (`packages: read`). It also reads the org's public repositories and their `components/*` directories.
2. It lists each package's tags on ghcr.io and indexes every stable release (`x.y.z`) and dev build (`x.y.z-dev`) that isn't in the index yet. It pulls the wasm layer, verifies its digest, and decodes it to WIT. A `_debug` tag is recorded as the debug build of its version. Other tags are skipped: other pre-releases, build metadata, `v`-prefixed tags, `dev`, and cosign signatures.
   - A release is immutable, so it is indexed once.
   - A dev tag moves with each build. It is re-indexed whenever its digest changes, until its release is published.
3. It then indexes the package versions those releases depend on, transitively, even when they are pre-releases, e.g. `wasi:logging@0.1.0-draft`. Each dependency's registry host comes from the wkg config, [`.config/wasm-pkg/config.toml`](.config/wasm-pkg/config.toml). The host's `/.well-known/wasm-pkg/registry.json` then gives the OCI repository, e.g. `wasi` → `wasi.dev` → `ghcr.io/webassembly/wasi/*`. As in wkg, `package_registry_overrides` takes precedence over `namespace_registries`, which takes precedence over `default_registry`. A mapping may also carry the registry metadata inline. To index the dependencies of a new namespace, add its registry to the config.
4. `directory build` renders the committed index as a static site. Versions are grouped by the repository they're built from, which comes from their `org.opencontainers.image.source` annotation. A repository releases its interface package and components together, with the same version, so `/repos/{owner}/{repo}/{version}/` lists everything the repository released at that version.
5. The site is built only from the committed index, never from the registry.

The id of a WIT package is the package it declares, e.g. `ghcr.io/componentized/logging` holds `componentized:logging`. A component's id comes from its repository path, e.g. `ghcr.io/componentized/logging/levels` is `componentized:logging-levels`.

## The index

The index is committed in [`index/`](index), one directory per version:

```
index/{namespace}/{name}/{version}/
  metadata.json   # kind, origin, OCI location and digests, annotations, imports, exports, dependencies
  package.wit     # a WIT package, its main package only
  world.wit       # a component's world, its main package only
```

The WIT holds only the version's main package. The foreign packages it uses resolve from their own indexed versions, which hold the full packages with their docs, not just the parts one component uses. Worlds are kept as written: resolving against a full package would import every interface that package's interfaces use, not just the ones the component imports.

A decoded version nests the parts of the foreign packages it uses. Each scan reduces it to its main package once everything it depends on is indexed. A version keeps its nested packages when it can't resolve from the index:

- a dependency isn't published, e.g. `wasi:keyvalue@0.2.0-draft2`
- the indexed dependency differs from the one the version was built against, e.g. a dev tag that has since moved

A component's `imports` and `exports` in `metadata.json` name each item the way its world does:

- an interface by its id, `wasi:logging/logging@0.1.0-draft`
- a named import or export by its name and the interface it aliases, `latch0: componentized:http/latch@0.1.0`
- an inline interface, `scope: interface`
- a function, `run: func`

A named item counts as importing or exporting the interface it aliases. Each scan re-derives these lists from the stored WIT, so metadata written by an older scan is brought up to date without fetching anything.

A release that is already indexed is never fetched again. To re-index a version, delete its directory.

Once `x.y.z` is released, the site hides `x.y.z-dev` from version lists, search, the home page, and the "imported by", "exported by" and "dependents" lists. Its pages are still rendered, marked `noindex`, so existing links keep working. A dev build with no release yet, e.g. `0.3.0-dev` when the latest release is `0.2.1`, is listed. The package page shows the newest release, or the newest dev build when a package has no release.

## Commands

```sh
# index new releases; a token lets discovery use the Packages API and raises rate limits
GITHUB_TOKEN=$(gh auth token) cargo run -- scan

# only the packages of a repository, e.g. while testing
cargo run -- scan --only logging

# render the site to _site and preview it
cargo run -- build
python3 -m http.server -d _site
```

`cargo run -- scan --help` lists the other options, e.g. `--config` for a different wkg config.

## Workflows

- [`scan.yaml`](.github/workflows/scan.yaml) runs every hour, on demand, and on a `release` repository dispatch. It commits the new versions to `main`, then deploys the site. A push made with the `GITHUB_TOKEN` doesn't start other workflows, so the scan calls `pages.yaml` itself.
- [`pages.yaml`](.github/workflows/pages.yaml) renders the site and deploys it to GitHub Pages on every push to `main`.
- [`ci.yaml`](.github/workflows/ci.yaml) checks formatting, lints, tests, and renders the index.

A component repository's release workflow can request a scan as soon as it publishes, using a token that may dispatch to this repository:

```sh
gh api repos/componentized/directory/dispatches -f event_type=release
```

## Setup

- Settings → Pages: set the source to **GitHub Actions**, and the custom domain to `directory.componentized.dev`.
- DNS: a `CNAME` record from `directory.componentized.dev` to `componentized.github.io`.
- Settings → Actions → General: allow workflows to have read and write permissions, so the scan can push to `main`. If `main` is protected, let `github-actions[bot]` bypass the protection.

## License

Apache License v2.0: see [LICENSE](./LICENSE) for details.
