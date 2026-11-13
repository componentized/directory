# Componentized Directory <!-- omit in toc -->

A directory of the Wasm components and WIT interfaces published by the Componentized project to [`ghcr.io/componentized`](https://github.com/orgs/componentized/packages). Every package, version, world, interface, type and function is browsable, along with what imports, exports and depends on it. It is served at [directory.componentized.dev](https://directory.componentized.dev).

- [How it works](#how-it-works)
- [The index](#the-index)
- [Commands](#commands)
- [Workflows](#workflows)
- [Setup](#setup)
- [License](#license)

## How it works

1. `directory scan` finds the org's packages from its public repositories: a package for each repository, and one for each of its `components/*` directories. A package published from anywhere else can be added with `--package`.
2. It lists each package's tags on ghcr.io and indexes every stable release (`x.y.z`) and dev build (`x.y.z-dev`) that isn't in the index yet. It pulls the wasm layer, verifies its digest, and decodes it to WIT. A `_debug` tag is recorded as the debug build of its version. Other tags are skipped: other pre-releases, build metadata, `v`-prefixed tags, `dev`, and cosign signatures.
   - A release is immutable, so it is indexed once.
   - A dev tag moves with each build. It is re-indexed whenever its digest changes, until its release is published.
3. It indexes each release of the packages [`.config/directory/tracked.toml`](.config/directory/tracked.toml) lists, e.g. every WASI release, and their pre-releases of the versions they never released, e.g. `wasi:config@0.2.0-draft`. The registry can't list a namespace's packages, so each is named.
4. It then indexes the package versions those releases depend on, transitively, even when they are pre-releases, e.g. `wasi:logging@0.1.0-draft`. Each dependency's registry host comes from the wkg config, [`.config/wasm-pkg/config.toml`](.config/wasm-pkg/config.toml). The host's `/.well-known/wasm-pkg/registry.json` then gives the OCI repository, e.g. `wasi` → `wasi.dev` → `ghcr.io/webassembly/wasi/*`. As in wkg, `package_registry_overrides` takes precedence over `namespace_registries`, which takes precedence over `default_registry`. A mapping may also carry the registry metadata inline. To index the dependencies of a new namespace, add its registry to the config.
5. `directory build` renders the committed index as a static site. Versions are grouped by the repository they're built from, which comes from their `org.opencontainers.image.source` annotation. An image that names the wrong repository is corrected in [`.config/directory/sources.toml`](.config/directory/sources.toml), which `build` and `scan` apply as they read the index; the index keeps what the registry says. A repository releases its interface package and components together, with the same version, so a namespace's page lists each repository's packages at one version, switching between its versions in place, and each package's page links the others its repository released with it.
6. `directory scan` also links each stable version to the GitHub release of its source repository tagged with the version, `v0.3.1` or else `0.3.1`. When the version records the commit it was built from, the tag must be that commit. A release published after its packages is linked by a later scan.
7. Each version's API, its interfaces' types and functions and its worlds' or component's imports and exports, is written as `api.json` beside its page. A package's compare page, `/{namespace}/{name}/compare/?from=…&to=…`, diffs two versions' APIs in the browser, so the site holds a file per version rather than a page per pair. It leads with what can break a component upgrading: what was removed or changed. An item behind a feature gate, `@unstable(feature = …)`, is labeled with its feature: it only asks something of a host or component with the feature enabled, so a change to it is a caution rather than a breaking change, and one that loses its gate is noted as stabilized.
8. The site is built only from the committed index, never from the registry.

The id of a WIT package is the package it declares, e.g. `ghcr.io/componentized/logging` holds `componentized:logging`. A component's id comes from its repository path, e.g. `ghcr.io/componentized/logging/levels` is `componentized:logging-levels`.

## The index

The index is committed in [`index/`](index), one directory per version:

```
index/{namespace}/{name}/{version}/
  metadata.json   # kind, origin, OCI location and digests, annotations, imports, exports, dependencies, GitHub release
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
# index new releases; a token raises the GitHub API's rate limits
GITHUB_TOKEN=$(gh auth token) cargo run -- scan

# only the packages of a repository, e.g. while testing
cargo run -- scan --only logging

# only one version of them, e.g. the release a repository just published
cargo run -- scan --only logging --version 0.3.1

# render the site to _site and preview it
cargo run -- build
python3 -m http.server -d _site
```

`cargo run -- scan --help` lists the other options, e.g. `--config` for a different wkg config.

## Samples

[`samples/`](samples) holds synthetic packages, laid out like the index, that exercise every WIT feature and keyword so the site's rendering of each can be checked: the `kitchen-sink` namespace, at [`/kitchen-sink/`](https://directory.componentized.dev/kitchen-sink/). They aren't published to any registry.

- Their `metadata.json` has `"origin": "synthetic"`. A component's imports and exports are derived from its WIT at build time, so its metadata needn't list them.
- `directory build` renders their pages apart from the index's: no page of the index lists, counts, searches or links them, while their own pages link each other. Each page notes it's a synthetic sample, is kept out of search engines and has no pull commands.
- A sample must be synthetic and keep to a namespace the index doesn't hold, and the index can't hold a synthetic version; the build fails otherwise.
- `scan` never reads them. `--samples` builds from another directory.

## Workflows

- [`scan.yaml`](.github/workflows/scan.yaml) runs every hour, on demand, and on a `release` repository dispatch. It commits the new versions to `main`, then deploys the site. A push made with the `GITHUB_TOKEN` doesn't start other workflows, so the scan calls `pages.yaml` itself.
- [`pages.yaml`](.github/workflows/pages.yaml) renders the site and deploys it to GitHub Pages on every push to `main`.
- [`ci.yaml`](.github/workflows/ci.yaml) checks formatting, lints, tests, and renders the index.

A component repository's release workflow can request a scan of the version it just published, as soon as it publishes. `only` names the repository, e.g. `componentized/valkey`, the publishing workflow's `GITHUB_REPOSITORY`, so just its packages are scanned, e.g. `componentized:valkey` and `componentized:valkey-*` for the `valkey` repository, and `version` the version, e.g. `0.3.1` or `0.3.2-dev`. The versions they depend on are still indexed, and the GitHub releases still linked, while the tracked packages wait for the next full scan. Without `version`, all the versions of the repository's packages are scanned, and without `only`, it's a full scan.

```sh
gh api repos/componentized/directory/dispatches -f event_type=release -f 'client_payload[only]=componentized/valkey' -f 'client_payload[version]=0.3.1'
```

The token must be allowed to dispatch to this repository, e.g. a GitHub App token with contents write permission on `componentized/directory`. A workflow's own `GITHUB_TOKEN` can't. As a step of the publish job, after the components are pushed to ghcr.io:

```yaml
- name: Create directory token
  id: directory-token
  uses: actions/create-github-app-token@v3
  with:
    client-id: ${{ secrets.CUSTODIAN_CLIENT_ID }}
    private-key: ${{ secrets.CUSTODIAN_PRIVATE_KEY }}
    repositories: directory
    permission-contents: write
- name: Request a directory scan
  env:
    GH_TOKEN: ${{ steps.directory-token.outputs.token }}
    VERSION: 0.3.1 # the version just published
  run: gh api repos/componentized/directory/dispatches -f event_type=release -f "client_payload[only]=${GITHUB_REPOSITORY}" -f "client_payload[version]=${VERSION}"
```

Scans queue, one at a time, but only one waits: a newer request replaces it. A request replaced before it runs is caught by the next full scan.

## Setup

- Settings → Pages: set the source to **GitHub Actions**, and the custom domain to `directory.componentized.dev`.
- DNS: a `CNAME` record from `directory.componentized.dev` to `componentized.github.io`.
- Settings → Actions → General: allow workflows to have read and write permissions, so the scan can push to `main`. If `main` is protected, let `github-actions[bot]` bypass the protection.

## License

Apache License v2.0: see [LICENSE](./LICENSE) for details.
