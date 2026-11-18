// Where the Host half finds its native pieces.
//
// The package ships one helper per platform and (currently) the Windows UI
// backend. A row may override either path in its `config`, and the environment
// variables exist for ad-hoc development; the bundled file is the fallback so a
// plain install needs no configuration. Nothing here throws: a missing binary
// must not stop the plugin from loading, because the model-facing tools and the
// panel degrade independently.
import { existsSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

/** Package root, derived from this file's location (`host/assets.js`). */
export const PACKAGE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

/** Bundled JSON-lines helper per platform/arch. */
const HELPERS = {
  win32: { x64: 'dsh-bridge-windows-x64.exe' },
  linux: { x64: 'dsh-bridge-linux-x64' },
  darwin: { arm64: 'dsh-bridge-macos-arm64' },
}

/** Bundled loopback UI backend per platform/arch. */
const BACKENDS = {
  win32: { x64: 'wave-insight-web-backend-windows-x64.exe' },
  linux: { x64: 'wave-insight-web-backend-linux-x64' },
  darwin: { arm64: 'wave-insight-web-backend-macos-arm64' },
}

/** Render `platform/arch` the way the diagnostics do. */
export function platformLabel(platform = process.platform, arch = process.arch) {
  return `${platform}/${arch}`
}

/**
 * Candidate paths for one bundled binary, most specific first, without
 * duplicates. Missing candidates stay in the list so the caller can report the
 * bundled path it expected.
 */
function candidates(explicit, fromEnv, bundledName) {
  const list = []
  if (typeof explicit === 'string' && explicit.length > 0) list.push(path.resolve(explicit))
  if (typeof fromEnv === 'string' && fromEnv.length > 0) list.push(path.resolve(fromEnv))
  if (bundledName) list.push(path.join(PACKAGE_ROOT, 'bin', bundledName))
  return [...new Set(list)]
}

/** The first existing path, or `undefined`. */
function firstExisting(list) {
  return list.find((candidate) => existsSync(candidate))
}

/**
 * Resolve the native paths for one plugin instance.
 * @param {{ helperPath?: string, backendPath?: string }} [config]
 * @param {{ platform?: NodeJS.Platform, arch?: string, env?: NodeJS.ProcessEnv }} [runtime]
 */
export function resolveAssets(config = {}, runtime = {}) {
  const platform = runtime.platform ?? process.platform
  const arch = runtime.arch ?? process.arch
  const env = runtime.env ?? process.env
  const helperName = HELPERS[platform]?.[arch]
  const backendName = BACKENDS[platform]?.[arch]

  const helperCandidates = candidates(config.helperPath, env.WAVE_DSH_BRIDGE, helperName)
  const backendCandidates = candidates(config.backendPath, env.WAVE_WEB_BACKEND, backendName)

  return {
    platform,
    arch,
    uiDir: path.join(PACKAGE_ROOT, 'ui'),
    helper: {
      path: firstExisting(helperCandidates),
      candidates: helperCandidates,
      bundled: Boolean(helperName),
      supported: Boolean(helperName),
    },
    backend: {
      path: firstExisting(backendCandidates),
      candidates: backendCandidates,
      bundled: Boolean(backendName),
      supported: Boolean(backendName),
    },
  }
}
