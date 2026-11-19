# Componentized Directory <!-- omit in toc -->

A directory of Wasm components and WIT interfaces published by the Componentized project to [`ghcr.io/componentized`](https://github.com/orgs/componentized/packages). Every package, version, world, interface, type and function is browsable, along with what imports, exports and depends on it. It is served at [componentized.dev](https://componentized.dev).

- [How it works](#how-it-works)
- [The index](#the-index)
- [Commands](#commands)
- [Icons](#icons)
- [Workflows](#workflows)
- [Setup](#setup)
- [License](#license)

## How it works

1. `directory scan` finds the org's packages from its public repositories: a package for each repository, and one for each of its `components/*` directories, on its default branch and at each of its release tags, e.g. `v0.2.1`, so a component renamed or removed since keeps its releases, e.g. `filesystem/readonly`. A package published from anywhere else can be added with `--package`.
2. It lists each package's tags on ghcr.io and indexes every stable release (`x.y.z`) and dev build (`x.y.z-dev`) that isn't in the index yet. It pulls the wasm layer, verifies its digest, and decodes it to WIT. A `_debug` tag is recorded as the debug build of its version. Other tags are skipped: other pre-releases, build metadata, `v`-prefixed tags, `dev`, and cosign signatures.
   - A release is immutable, so it is indexed once.
   - A dev tag moves with each build. It is re-indexed whenever its digest changes, until its release is published.
3. It indexes each release of the packages [`.config/directory/tracked.toml`](.config/directory/tracked.toml) lists, e.g. every WASI release, and their pre-releases of the versions they never released, e.g. `wasi:config@0.2.0-draft`. The registry can't list a namespace's packages, so each is named.
4. It then indexes the package versions those releases depend on, transitively, even when they are pre-releases, e.g. `wasi:logging@0.1.0-draft`. Each dependency's registry host comes from the wkg config, [`.config/wasm-pkg/config.toml`](.config/wasm-pkg/config.toml). The host's `/.well-known/wasm-pkg/registry.json` then gives the OCI repository, e.g. `wasi` → `wasi.dev` → `ghcr.io/webassembly/wasi/*`. As in wkg, `package_registry_overrides` takes precedence over `namespace_registries`, which takes precedence over `default_registry`. A mapping may also carry the registry metadata inline. To index the dependencies of a new namespace, add its registry to the config.
5. `directory build` renders the committed index as a static site. Versions are grouped by the repository they're built from, which comes from their `org.opencontainers.image.source` annotation. An image that names the wrong repository is corrected in [`.config/directory/sources.toml`](.config/directory/sources.toml), which `build` and `scan` apply as they read the index; the index keeps what the registry says. A repository releases its interface package and components together, with the same version, and a dev version is its newest build: the packages built from the same commit as the newest, or on the same day when one doesn't record its commit. A package left over from an earlier build, e.g. one the repository has since dropped, is listed with the packages released at other versions, so a namespace's page lists each repository's packages at one version, switching between its versions in place, and each package's page links the others its repository released with it.
6. `directory scan` also links each stable version to the GitHub release of its source repository tagged with the version, `v0.3.1` or else `0.3.1`. When the version records the commit it was built from, the tag must be that commit. A release published after its packages is linked by a later scan.
7. Each version's API, its interfaces' types and functions and its worlds' or component's imports and exports, is written as `api.json` beside its page. A package's compare page, `/{namespace}/{name}/compare/?from=…&to=…`, diffs two versions' APIs in the browser, so the site holds a file per version rather than a page per pair. It leads with what can break a component upgrading: what was removed or changed. An item behind a feature gate, `@unstable(feature = …)`, is labeled with its feature: it only asks something of a host or component with the feature enabled, so a change to it is a caution rather than a breaking change, and one that loses its gate is noted as stabilized.
8. `directory scan` records the digest of each component nested whole in a component, and whether it has code of its own: a composed component nests the components it was composed from, while a component's bindings nest a wrapper of types alone. A full scan reads them for a version indexed before scans did, fetching its wasm layer again. `directory build` links a composed component to the versions whose wasm layer has those bytes, under "Composition", preferring a release to a dev build with the same bytes, and each of those versions back to it, under "Composed by". A nested component that matches none is counted, e.g. one generated as it was composed. A composition's diagram shows, under the component an instance of a part is of, the name the composition gives the instance in its `component-name` section, when it gives one, e.g. `stderr`, as a tool like wac names the instances it makes.
9. `directory scan` checks each image's cosign signature the first time it sees the image: a new version, or a dev build's new build. A version's debug build is an image of its own, checked on its own, and its page shows each image's signature with the image, under Pull and Debug. It finds the signature in the registry, a Sigstore bundle by the referrers tag `sha256-{digest}`, or else an older `sha256-{digest}.sig` signature, and `cosign verify` verifies it: its certificate is from Sigstore's certificate authority, and the signature is in Sigstore's transparency log, Rekor. Any signer is accepted, as an image can be signed by another repository than its source, and who signed it, e.g. a GitHub Actions workflow, is recorded and shown on the version's page with its log entry. An image is recorded as verified, unsigned, or as having a signature that didn't verify. Without `cosign` installed, a scan checks none, and a later full scan checks those it left.
8. The site is built only from the committed index, never from the registry.

The id of a WIT package is the package it declares, e.g. `ghcr.io/componentized/logging` holds `componentized:logging`. A component's id comes from its repository path, e.g. `ghcr.io/componentized/logging/levels` is `componentized:logging-levels`.

## The index

The index is committed in [`index/`](index), one directory per version:

```
index/{namespace}/{name}/{version}/
  metadata.json   # kind, origin, OCI location and digests, annotations, imports, exports, dependencies, GitHub release, nested components, signature
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

A component's imports and exports have no order that means anything, and the tools that make components don't let it be chosen, so they're sorted, in the metadata, on the component's page and in a composition's diagram: first the interfaces with ids, by namespace, package and interface name, semantic version, and then the name of a named import, e.g. `latch0`; then the rest, inline interfaces, functions and types, by name. A world of a WIT package keeps the order it declares. A named item counts as importing or exporting the interface it aliases. Each scan re-derives these lists from the stored WIT, so metadata written by an older scan is brought up to date without fetching anything.

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

[`samples/`](samples) holds synthetic packages, laid out like the index, that exercise every WIT feature and keyword so the site's rendering of each can be checked: the `kitchen-sink` namespace, at [`/kitchen-sink/`](https://componentized.dev/kitchen-sink/). They aren't published to any registry.

- Their `metadata.json` has `"origin": "synthetic"`. A component's imports and exports are derived from its WIT at build time, so its metadata needn't list them.
- `directory build` renders their pages apart from the index's: no page of the index lists, counts, searches or links them, while their own pages link each other. Each page notes it's a synthetic sample, is kept out of search engines and has no pull commands.
- A sample must be synthetic and keep to a namespace the index doesn't hold, and the index can't hold a synthetic version; the build fails otherwise.
- `scan` never reads them. `--samples` builds from another directory.

## Icons

`directory build` also publishes the icons the site shows for each kind of package and item as SVG files, for a page elsewhere that links to the directory, at `/_icons/{variant}/{name}.svg`, e.g. [`/_icons/neutral/component.svg`](https://componentized.dev/_icons/neutral/component.svg). [`/_icons/`](https://componentized.dev/_icons/) lists them all. The directory's `_` can't begin a WIT namespace, whose identifiers are kebab-case, so it never collides with a namespace's page, `/{namespace}/`.

```html
<a href="https://componentized.dev/componentized/logging/"><img src="https://componentized.dev/_icons/neutral/package.svg" width="26" height="26" style="margin: -5px" alt=""> componentized:logging</a>
```

- `light` and `dark` are in the colors of the site's light and dark themes.
- `neutral` is for a page whose theme isn't known. Each color is its light and dark colors mixed to the one with the most contrast against both white and black, 4.58:1.
- Every file is 26×26: the icon's 16×16 in the middle, and a 5px margin all round for the badges that hang past its corners, e.g. `use-record`'s or `func-static-get`'s, as they do on the site. An icon is in the same place and at the same size with or without badges, so a list of them lines up. A margin of `-5px` lays one out in the space of a 16×16 icon, its badges overlapping what's around it. A badge's halo is cut out of the icon under it rather than painted in a background color, so every variant sits on any background.

They're drawn from the sprite every page inlines, `kind_sprite` in [`templates/macros.html`](templates/macros.html), by [`src/icons.rs`](src/icons.rs), so a change to an icon changes its files too. A test checks the `light` and `dark` colors against [`static/style.css`](static/style.css).

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

- Settings → Pages: set the source to **GitHub Actions**, and the custom domain to `componentized.dev`.
- DNS: `A` and `AAAA` records for `componentized.dev` to GitHub Pages' addresses, `185.199.108.153` to `185.199.111.153` and `2606:50c0:8000::153` to `2606:50c0:8003::153`, and a `CNAME` record from `www.componentized.dev` to `componentized.github.io`.
- Settings → Actions → General: allow workflows to have read and write permissions, so the scan can push to `main`. If `main` is protected, let `github-actions[bot]` bypass the protection.

## License

Apache License v2.0: see [LICENSE](./LICENSE) for details.
