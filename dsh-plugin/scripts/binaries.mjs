#!/usr/bin/env node
// Stage or check the native binaries the bundle ships.
//
//   node dsh-plugin/scripts/binaries.mjs stage  [--set <platform/arch,...>] [--from <dir>]
//   node dsh-plugin/scripts/binaries.mjs verify [--set <platform/arch,...>] [--tarball <file>]
//
// `bin/` is not in git. The release workflow unpacks the
// `.github/workflows/dsh-plugin-binaries.yml` artifacts into it, so a checkout has
// to build the row for its own machine first:
//
//   cargo build -p dsh-bridge -p wave-insight-web-backend --release
//   node dsh-plugin/scripts/binaries.mjs stage
//
// `verify` is the packaging gate: `prepublishOnly` runs it, so a tarball can
// never ship without a platform the matrix in `host/assets.js` promises.
import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { PLATFORMS, bundledNames } from '../host/assets.js'

const PACKAGE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const REPO_ROOT = path.resolve(PACKAGE_ROOT, '..')
const BIN_DIR = path.join(PACKAGE_ROOT, 'bin')
const MANIFEST = path.join(PACKAGE_ROOT, 'package.json')

/** Stems `cargo` uses, so a build output can be mapped to its release name. */
const STEMS = ['dsh-bridge', 'wave-insight-web-backend']

/** @param {string} message */
function die(message) {
  console.error(`binaries: ${message}`)
  process.exit(1)
}

function usage() {
  console.error([
    'usage:',
    '  node dsh-plugin/scripts/binaries.mjs stage  [--set <platform/arch,...>] [--from <dir>]',
    '  node dsh-plugin/scripts/binaries.mjs verify [--set <platform/arch,...>] [--tarball <file>]',
    '',
    `platforms: ${Object.keys(PLATFORMS).join(', ')}`,
  ].join('\n'))
}

/** @param {string[]} argv */
function parseArgs(argv) {
  /** @type {{ command: string | undefined, targets: string | undefined, from: string | undefined, tarball: string | undefined, help: boolean }} */
  const parsed = { command: argv[0], targets: undefined, from: undefined, tarball: undefined, help: false }
  for (let index = 1; index < argv.length; index += 1) {
    const flag = argv[index]
    const value = () => {
      const next = argv[index + 1]
      if (next === undefined) die(`${flag} needs a value`)
      index += 1
      return next
    }
    if (flag === '--set') parsed.targets = value()
    else if (flag === '--from') parsed.from = value()
    else if (flag === '--tarball') parsed.tarball = value()
    else if (flag === '--help' || flag === '-h') parsed.help = true
    else die(`unknown argument ${flag}`)
  }
  return parsed
}

/**
 * The rows to act on: the explicit `--set`, or whatever the fallback decides.
 * @param {string | undefined} setting
 * @param {() => string[]} [fallback]
 */
function select(setting, fallback = () => Object.keys(PLATFORMS)) {
  if (!setting) return fallback()
  const keys = setting.split(',').map((entry) => entry.trim()).filter(Boolean)
  for (const key of keys) {
    if (!PLATFORMS[key]) die(`unknown platform "${key}"; known: ${Object.keys(PLATFORMS).join(', ')}`)
  }
  return keys
}

/** The matrix row for the machine running this script. */
function hostKey() {
  const key = `${process.platform}/${process.arch}`
  if (!PLATFORMS[key]) die(`no bundled binaries for ${key}; the matrix has: ${Object.keys(PLATFORMS).join(', ')}`)
  return key
}

/**
 * Map a release name back to what `cargo` writes:
 * `dsh-bridge-macos-x64` -> `dsh-bridge`, `...windows-x64.exe` -> `dsh-bridge.exe`.
 * @param {string} release
 */
function builtName(release) {
  const stem = STEMS.find((candidate) => release.startsWith(`${candidate}-`))
  if (!stem) die(`bin/${release} does not look like a bundled binary`)
  return `${stem}${release.endsWith('.exe') ? '.exe' : ''}`
}

/**
 * A directory may already hold the release name (an unpacked artifact) or the
 * plain build output (a `cargo` target directory); take whichever is there.
 * @param {string} dir @param {string} release
 */
function locate(dir, release) {
  for (const name of [release, builtName(release)]) {
    const file = path.join(dir, name)
    if (fs.existsSync(file)) return file
  }
  return null
}

/** @param {string[]} targets @param {string | undefined} from */
function stage(targets, from) {
  fs.mkdirSync(BIN_DIR, { recursive: true })
  /** A file nobody built. */
  const missing = []
  /** A file that exists but could not be copied (a running DSH holds it open). */
  const failed = []
  let staged = 0
  for (const key of targets) {
    const row = PLATFORMS[key]
    const dirs = from
      ? [path.resolve(from)]
      : [path.join(REPO_ROOT, 'target', row.triple, 'release'), path.join(REPO_ROOT, 'target', 'release')]
    for (const column of /** @type {const} */ (['helper', 'backend'])) {
      const release = row[column]
      const source = dirs.map((dir) => locate(dir, release)).find(Boolean)
      if (!source) {
        missing.push(`no ${release} in ${dirs.join(' or ')}`)
        continue
      }
      const destination = path.join(BIN_DIR, release)
      try {
        fs.copyFileSync(source, destination)
      } catch (error) {
        failed.push(`could not write bin/${release}: ${error instanceof Error ? error.message : String(error)}`)
        continue
      }
      if (!release.endsWith('.exe')) fs.chmodSync(destination, 0o755)
      staged += 1
    }
  }
  if (missing.length || failed.length) {
    const lines = [`could not stage ${targets.join(', ')}:`]
    for (const problem of [...missing, ...failed]) lines.push(`  - ${problem}`)
    if (missing.length) {
      lines.push('', 'build the missing ones with:', '  cargo build -p dsh-bridge -p wave-insight-web-backend --release')
    }
    if (failed.length) {
      lines.push('', 'a binary that is in use cannot be replaced on Windows: stop DSH (or disable the plugin row) and retry.')
    }
    die(lines.join('\n'))
  }
  console.log(`binaries: staged ${staged} file(s) into dsh-plugin/bin for ${targets.join(', ')}`)
}

/** @param {string[]} targets @param {string | undefined} tarball */
function verify(targets, tarball) {
  const expected = bundledNames()
  const manifest = JSON.parse(fs.readFileSync(MANIFEST, 'utf8'))
  const declared = /** @type {string[]} */ (manifest.files ?? [])
    .filter((entry) => entry.startsWith('bin/'))
    .map((entry) => entry.slice('bin/'.length))
  /** @type {string[]} */
  const problems = []

  // The manifest, the matrix and this script have to agree, or a released
  // tarball silently loses a platform.
  for (const name of expected) {
    if (!declared.includes(name)) problems.push(`package.json "files" does not list bin/${name}`)
  }
  for (const name of declared) {
    if (!expected.includes(name)) problems.push(`package.json "files" lists bin/${name}, which is not a row of PLATFORMS`)
  }

  for (const key of targets) {
    for (const name of [PLATFORMS[key].helper, PLATFORMS[key].backend]) {
      const file = path.join(BIN_DIR, name)
      if (!fs.existsSync(file)) {
        problems.push(`bin/${name} (${key}) is missing; run: node dsh-plugin/scripts/binaries.mjs stage`)
        continue
      }
      if (fs.statSync(file).size === 0) problems.push(`bin/${name} (${key}) is empty`)
    }
  }

  // Leftovers do not ship (`files` filters them) but they hide a rename.
  if (fs.existsSync(BIN_DIR)) {
    for (const name of fs.readdirSync(BIN_DIR)) {
      if (!expected.includes(name)) console.warn(`binaries: bin/${name} is not in the matrix and will not ship`)
    }
  }

  if (tarball) {
    if (!fs.existsSync(tarball)) die(`${tarball} does not exist`)
    /** @type {string} */
    let listing
    try {
      listing = execFileSync('tar', ['-tzf', tarball], { encoding: 'utf8' })
    } catch (error) {
      die(`could not read ${tarball} with tar: ${error instanceof Error ? error.message : String(error)}`)
    }
    const entries = listing.split('\n').map((entry) => entry.trim())
    for (const name of expected) {
      if (!entries.includes(`package/bin/${name}`)) problems.push(`${tarball} does not carry bin/${name}`)
    }
  }

  if (problems.length) die(`the package is incomplete:\n  - ${[...new Set(problems)].join('\n  - ')}`)
  console.log(`binaries: ${targets.join(', ')} staged and declared${tarball ? ', tarball complete' : ''}`)
}

function main() {
  const args = parseArgs(process.argv.slice(2))
  if (args.help) {
    usage()
    process.exit(0)
  }
  if (args.command === 'stage') {
    // A `--from` directory is a release staging area, so it has to hold every
    // platform; without one, stage just this machine's pair.
    const targets = select(args.targets, () => (args.from ? Object.keys(PLATFORMS) : [hostKey()]))
    stage(targets, args.from)
    return
  }
  if (args.command === 'verify') {
    verify(select(args.targets), args.tarball)
    return
  }
  usage()
  process.exit(1)
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main()
