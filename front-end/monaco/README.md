# Pinned monaco-editor

`vs/` is the `min/vs` directory of **monaco-editor 0.44.0**, unmodified, copied
out of the npm tarball. The app loads it from `./monaco/vs` at runtime (the AMD
loader is kept, see `src/code_reader/monaco_glue.rs`); nothing is fetched from a
CDN, so the editor also works offline or behind a firewall.

Trunk copies this directory into `dist/monaco/vs` through the
`<link data-trunk rel="copy-dir" href="monaco/vs" />` tag in `index.html`, i.e.
it is served next to `index.html`, not bundled into the wasm.

## Updating the pinned version

1. Pick one version of `monaco-editor` and use it everywhere (the numbers in
   `monaco_glue.rs` and here must match):

   ```sh
   npm pack monaco-editor@0.44.0      # or: curl -LO https://registry.npmjs.org/monaco-editor/-/monaco-editor-0.44.0.tgz
   tar -xzf monaco-editor-0.44.0.tgz
   ```

2. Replace this directory's `vs/` with `package/min/vs` of that release:

   ```sh
   rm -rf vs && cp -r package/min/vs vs
   ```

3. Bump the version in the header of this file and in `VS_VERSION` in
   `src/code_reader/monaco_glue.rs`.

4. Rebuild (`trunk build`) and check that `dist/monaco/vs/loader.js`,
   `dist/monaco/vs/editor/editor.main.js` and
   `dist/monaco/vs/basic-languages/systemverilog/systemverilog.js` exist, then
   open the app with **all external hosts blocked** (e.g. DevTools "block request
   domain" for `cdnjs.cloudflare.com`, `unpkg.com`, `jsdelivr.net`) and confirm
   the editor still mounts and the language providers still answer.

## Why the whole `min/vs` and not a subset

The 0.44.0 `min/vs` tree is ~12 MB (113 files). Only `loader.js`,
`editor/editor.main.*`, `base/**` and `basic-languages/systemverilog/**` are
strictly needed for this app, but pruning the rest (`language/typescript` alone
is 4.4 MB) would silently break any future use of those languages and would make
step 2 "copy a subset" instead of "copy the release". Keep the tree complete
unless the size becomes a real deployment problem.
