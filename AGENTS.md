# AGENTS.md

Guidance for coding agents working in this repository. The human-facing docs are
[`readme.md`](readme.md) (build, test, CI) and [`dsh-plugin/README.md`](dsh-plugin/README.md)
(the DSH bundle). This file only records what a **release** touches, because the
version numbers are spread over several files and the tag name decides what CI
publishes.

## Two versions, shipped by two different tags

| Version | Lives in | Tag | What the tag does |
|---|---|---|---|
| **App** `0.24.x` | `Cargo.toml`, `Cargo.lock`, `src-tauri/tauri.conf.json` | `v0.24.x` | Runs [`.github/workflows/tauri.yml`](.github/workflows/tauri.yml) and **creates the GitHub release**: desktop bundles for macOS/Windows/Ubuntu, the plain binaries, and the packed `dsh-plugin` tarball (plus the npm publish once that is turned on) |
| **DSH plugin** `0.2.x` | [`dsh-plugin/package.json`](dsh-plugin/package.json) | `dsh-plugin-v0.2.x` | Nothing. The workflow filter is `tags: ['v*']`, so a `dsh-plugin-*` tag is only a marker in history |

The plugin version is independent of the app version: a plugin-only fix moves it
to `0.2.x+1` and leaves the app version alone. The tarball itself is only built
by the `v*` job, so a plugin fix that should reach users still takes an app
patch bump and a `v*` tag — a `dsh-plugin-v*` tag on its own ships nothing.

## Cutting an app release (`v*`)

### 1. Bump

- [`Cargo.toml`](Cargo.toml) — `[workspace.package] version`. All seven members
  inherit it, so this is the only place the app version is written in Rust.
- `Cargo.lock` — the same seven member blocks: `app`, `back-end`, `dsh-bridge`,
  `front-end`, `wave-insight-companion`, `wave-insight-lib`,
  `wave-insight-web-backend`.
- [`src-tauri/tauri.conf.json`](src-tauri/tauri.conf.json) — `version`. The
  Tauri bundle name, installer metadata and `releaseName: 'v__VERSION__'` all
  come from here, so a tag without this bump republishes the previous version.
- [`CHANGELOG.md`](CHANGELOG.md) — new `## [x.y.z] - YYYY-MM-DD` section below
  `## [Unreleased]`; Keep a Changelog headings (`Added`/`Changed`/`Fixed`).
- [`docs/roadmap.md`](docs/roadmap.md) — the "Where we are" paragraph: new
  release first, the previous one demoted to a sentence after it.

Plugin side, in the same commit when the plugin ships with the release:

- [`dsh-plugin/package.json`](dsh-plugin/package.json) `version`, and a
  `## [0.2.x]` entry in [`dsh-plugin/CHANGELOG.md`](dsh-plugin/CHANGELOG.md).
- [`dsh-plugin/README.md`](dsh-plugin/README.md) if a platform row or the build
  story changed.

### 2. Verify

```bash
cargo metadata --locked --no-deps --format-version 1   # the lockfile still resolves

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings   # needs front-end/dist
cargo test --workspace --exclude front-end --exclude app --locked

node qa/viewer-build-stamp.mjs        # the bundled viewer matches front-end/
node qa/dsh-plugin-smoke.mjs          # host tools, projection, restore
node qa/dsh-plugin-client.test.mjs
node qa/dsh-invoke-shim.test.mjs
node qa/web-backend-smoke.mjs         # needs the release backend build
node dsh-plugin/scripts/binaries.mjs verify --set <platform/arch>
```

`viewer-build-stamp.mjs` hashes the app version out of `Cargo.toml`, so a plain
version bump never makes the bundled viewer stale. What does is a change under
`front-end/`, `wave-insight-lib/src`, the release profile or
`rust-toolchain.toml` — rebuild the wasm and run
`node qa/viewer-build-stamp.mjs --write` in that case.

### 3. Tag and push

```bash
git commit -am 'release: 0.24.x - <one line>'
git tag -a v0.24.x -m 'v0.24.x - <one line>'
git push origin refs/heads/main:refs/heads/main    # explicit refspec, see traps
git push origin refs/tags/v0.24.x
```

Under PowerShell, `git push` prints its progress on stderr and the shell wraps
that as a `NativeCommandError` even on success, so a red block is not a failed
push: check `$LASTEXITCODE` and confirm with `git ls-remote origin`.

The tag run is the release: `tauri.yml` builds the three desktop targets,
`dsh-plugin-binaries.yml` builds the four native pairs (Windows x64, Linux x64
**musl**, macOS ARM64, macOS x64), and the `dsh-plugin` job stages them, runs the
QA suites against the release binaries, `npm pack`s the bundle, attaches it next
to the desktop assets and — while the repository variable `NPM_PUBLISH` is
`true` — publishes the version to npm. It is the one place `bin/` is populated —
no binary is committed.

### The first npm publish (once)

`dsh-plugin/package.json` is `dsh-wave-insight`, and npm cannot attach a Trusted
Publisher to a package that does not exist yet, so the first version goes out by
hand after a release has built it.

1. Cut a `v*` release; the `dsh-plugin` job attaches
   `dsh-wave-insight-<version>.tgz` to it. The publish step is still off, so the
   job succeeds without one.
2. From a machine whose npm account owns the name, publish exactly that tarball:
   `npm publish dsh-wave-insight-<version>.tgz` (an interactive 2FA prompt is
   expected). It was packed from a staged `bin/`, so it is the same artifact the
   job would have published.
3. On npmjs.com, the package's **Settings > Trusted Publisher > GitHub Actions**:
   organization `Wave-Insight`, repository `wave-insight`, workflow file
   `tauri.yml`. Allow direct publishing as well as staging: a connection created
   since 2026-09-03 permits only `npm stage publish` unless **npm publish** is
   ticked, and this job runs `npm publish`. A new connection also expires if its
   first publish does not succeed within two days, so create it when the next
   release is at hand and recreate it if it lapsed.
4. Set the repository variable `NPM_PUBLISH` to `true`.

Every later tag then publishes the version `dsh-plugin/package.json` carries and
skips a version that is already there.

## Traps

- **Never rewrite every `version = "0.24.x"` line in `Cargo.lock`.** Third-party
  crates can share the number: `darling`, `darling_core` and `darling_macro`
  were at `0.24.1` and a blind replace made the lockfile unresolvable
  (`failed to select a version for the requirement darling = ^0.24`). Edit only
  the seven workspace members and re-check with `cargo metadata --locked`.
- **A `dsh-plugin-v*` tag does not create a release.** If a plugin fix should
  ship, it needs the app bump and a `v*` tag; `dsh-plugin-v*` is history only.
- **`git push origin main` is ambiguous here.** A stray `refs/tags/main` exists,
  so git refuses with `src refspec main matches more than one`; always push
  `refs/heads/main:refs/heads/main`.
- **Re-pointing a released tag needs `--force`**, and any run the old tag already
  started keeps its own commit. Prefer fixing forward with a new patch version.
- **Linux binaries are `x86_64-unknown-linux-musl`** (static, so one file serves
  Alpine and glibc distributions). Building that target on Windows fails against
  the system MinGW `ld`; link and smoke-test `linux/x64` in CI, not locally.
- **`dsh-plugin/bin/` is gitignored.** A checkout has no helper or backend until
  `cargo build -p dsh-bridge -p wave-insight-web-backend --release` and
  `node dsh-plugin/scripts/binaries.mjs stage`.
- **The npm step skips a version that is already published.** The bundle version
  only moves when the plugin changes, so most `v*` releases publish nothing;
  without that guard the second release of an unchanged plugin fails on a
  duplicate version.
- **The package name is load-bearing.** The loader resolves a bundle's row by the
  name in [`dsh-plugin/cordis.patch.yml`](dsh-plugin/cordis.patch.yml), and the
  browser half registers under the package name in
  [`dsh-plugin/client.js`](dsh-plugin/client.js). Renaming
  [`dsh-plugin/package.json`](dsh-plugin/package.json) without both — and the
  assertions in [`qa/dsh-plugin-client.test.mjs`](qa/dsh-plugin-client.test.mjs) —
  installs a bundle whose row never resolves.
