# AGENTS.md

Guidance for coding agents working in this repository. The human-facing docs are
[`readme.md`](readme.md) (build, test, CI) and [`dsh-plugin/README.md`](dsh-plugin/README.md)
(the DSH bundle). This file only records what a **release** touches, because the
version numbers are spread over several files and the tag name decides what CI
publishes.

## Two versions, shipped by two different tags

| Version | Lives in | Tag | What the tag does |
|---|---|---|---|
| **App** `0.24.x` | `Cargo.toml`, `Cargo.lock`, `src-tauri/tauri.conf.json` | `v0.24.x` | Runs [`.github/workflows/tauri.yml`](.github/workflows/tauri.yml) and **creates the GitHub release**: desktop bundles for macOS/Windows/Ubuntu, the plain binaries, and the packed `dsh-plugin` tarball |
| **DSH plugin** `0.2.x` | [`dsh-plugin/package.json`](dsh-plugin/package.json) | `dsh-plugin-v0.2.x` | Nothing. The workflow filter is `tags: ['v*']`, so a `dsh-plugin-*` tag is only a marker in history |

The plugin version is independent of the app version: a plugin-only fix moves it
to `0.2.x+1` and leaves the app version alone. The tarball itself is only built
by the `v*` job, so a plugin fix that should reach users still takes an app
patch bump and a `v*` tag — a `dsh-plugin-v*` tag on its own ships nothing.

## Cutting an app release (`v*`)

### 1. Bump

- [`Cargo.toml`](Cargo.toml) — `[workspace.package] version`. All six members
  inherit it, so this is the only place the app version is written in Rust.
- `Cargo.lock` — the same six member blocks: `app`, `back-end`, `dsh-bridge`,
  `front-end`, `wave-insight-lib`, `wave-insight-web-backend`.
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
QA suites against the release binaries, `npm pack`s the bundle and attaches it
next to the desktop assets. It is the one place `bin/` is populated — no binary
is committed.

## Traps

- **Never rewrite every `version = "0.24.x"` line in `Cargo.lock`.** Third-party
  crates can share the number: `darling`, `darling_core` and `darling_macro`
  were at `0.24.1` and a blind replace made the lockfile unresolvable
  (`failed to select a version for the requirement darling = ^0.24`). Edit only
  the six workspace members and re-check with `cargo metadata --locked`.
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
