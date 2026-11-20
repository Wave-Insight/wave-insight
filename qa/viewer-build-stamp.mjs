// Guard against a stale bundled viewer.
//
// `dsh-plugin/ui` is a build artifact of `front-end/` (web-remote wasm) plus the
// web shim, but nothing in the repository ties the two together: a front-end fix
// can land while the shipped UI still runs an older build, which is exactly how
// the DSH panel kept showing a viewer that predated the native-dump fix.
//
// This stamp records a hash of the inputs that produce the UI. `--write` after a
// rebuild; the default mode fails when the sources moved on without a rebuild.
//
//   node qa/viewer-build-stamp.mjs           # check
//   node qa/viewer-build-stamp.mjs --write   # after: trunk build ... + copy
import { createHash } from 'node:crypto'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
const stampFile = path.join(root, 'dsh-plugin', 'ui', 'build.json')

/** Files whose content decides the bundled viewer. */
const INPUTS = [
  'front-end/Cargo.toml',
  'front-end/index.html',
  'front-end/styles.css',
  'front-end/web/dsh-invoke.js',
  'Cargo.toml',
  'rust-toolchain.toml',
]

/** Every `.rs` file under a directory, relative to the repository root. */
function rustSources(dir) {
  const out = []
  const walk = (current) => {
    for (const entry of fs.readdirSync(path.join(root, current), { withFileTypes: true })) {
      if (entry.name === 'target' || entry.name === 'node_modules' || entry.name.startsWith('.')) continue
      const relative = path.join(current, entry.name)
      if (entry.isDirectory()) walk(relative)
      else if (entry.name.endsWith('.rs')) out.push(relative.replace(/\\/g, '/'))
    }
  }
  walk(dir)
  return out
}

const files = [...INPUTS, ...rustSources('front-end/src'), ...rustSources('wave-insight-lib/src')].sort()

/**
 * The root `Cargo.toml` is an input for one reason: its `[profile.*]` sections
 * decide how the wasm is compiled (`opt-level`, `lto`, `codegen-units`,
 * `panic`). Everything else in that file — the `[workspace.package]` version, a
 * new member, a dependency — changes nothing about the bundle, so hashing the
 * whole file made a release bump or an unrelated crate report the viewer as
 * stale. Only the profiles are hashed.
 */
function normalized(file, text) {
  if (file !== 'Cargo.toml') return text
  const lines = text.split('\n')
  const out = []
  let inProfile = false
  for (const line of lines) {
    if (/^\[/.test(line)) inProfile = line.startsWith('[profile')
    if (inProfile) out.push(line)
  }
  return out.length > 0 ? out.join('\n') : ''
}

const hash = createHash('sha256')
for (const file of files) {
  // Line endings are normalized: a CRLF checkout must not change the stamp.
  const text = fs.readFileSync(path.join(root, file), 'utf8').replace(/\r\n/g, '\n')
  hash.update(file)
  hash.update('\0')
  hash.update(normalized(file, text))
  hash.update('\0')
}
const sources = hash.digest('hex')

const write = process.argv.includes('--write')
const current = fs.existsSync(stampFile) ? JSON.parse(fs.readFileSync(stampFile, 'utf8')) : null

if (write) {
  fs.writeFileSync(
    stampFile,
    `${JSON.stringify({ sources, builtAt: new Date().toISOString(), inputs: files.length }, null, 2)}\n`,
  )
  console.log(`viewer stamp written: ${sources.slice(0, 12)}… over ${files.length} files`)
  process.exit(0)
}

if (!current) {
  console.error('no dsh-plugin/ui/build.json: rebuild the viewer and run: node qa/viewer-build-stamp.mjs --write')
  process.exit(1)
}
if (current.sources !== sources) {
  console.error(
    [
      'the bundled viewer is stale: front-end sources changed since it was built.',
      `  stamped: ${current.sources}`,
      `  current: ${sources}`,
      '  rebuild it and refresh the stamp:',
      '    cd front-end && trunk build --no-default-features --features web-remote --release',
      '    copy dist/*.js dist/*.wasm dist/*.css index.html into dsh-plugin/ui, keep dsh-invoke.js',
      '    node qa/viewer-build-stamp.mjs --write',
    ].join('\n'),
  )
  process.exit(1)
}
console.log(`viewer stamp matches (${sources.slice(0, 12)}…, built ${current.builtAt})`)
