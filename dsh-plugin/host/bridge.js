// JSON-lines protocol to the `dsh-bridge` helper.
//
// One process per call: the helper re-opens the dump, answers one request, and
// exits, so a wedged dump cannot outlive a tool call. This module owns the wire
// format (snake_case fields from Rust) and turns it into the camelCase shape the
// tools use, so a protocol change stays in one place.
import { spawn } from 'node:child_process'
import { chmodSync } from 'node:fs'
import { cancellationOf, waveError } from './errors.js'

const DEFAULT_TIMEOUT_MS = 60_000
/** A decoded window is bounded by the tool; anything larger is a protocol bug. */
const MAX_RESPONSE_BYTES = 32 * 1024 * 1024
/** Kept only for the failure message; a full pipe would block the child. */
const MAX_STDERR_BYTES = 8 * 1024

/** @param {unknown} value @returns {{start:number,end:number}|null} */
export function normalizeRange(value) {
  if (!value || typeof value !== 'object') return null
  const start = Number(value.start)
  const end = Number(value.end)
  if (!Number.isFinite(start) || !Number.isFinite(end)) return null
  return { start: Math.trunc(start), end: Math.trunc(end) }
}

export class Bridge {
  /** @type {string | null} */
  #executable = null

  /**
   * @param {{ path?: string, candidates?: string[], supported?: boolean }} asset
   * @param {{ timeoutMs?: number, platform?: NodeJS.Platform }} [options]
   */
  constructor(asset, options = {}) {
    this.asset = asset
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS
    this.platform = options.platform ?? process.platform
  }

  /** @returns {boolean} */
  get available() {
    return Boolean(this.asset?.path)
  }

  /** @returns {string} */
  executable() {
    if (this.#executable) return this.#executable
    const file = this.asset?.path
    if (!file) {
      const expected = this.asset?.candidates?.join(', ') || 'no candidate path'
      const platform = `${process.platform}/${process.arch}`
      throw waveError(
        'HELPER_UNAVAILABLE',
        `no dsh-bridge helper is available for ${platform}. Build one with "cargo build -p dsh-bridge --release" and set the plugin config field "helperPath" (or WAVE_DSH_BRIDGE). Looked for: ${expected}`,
      )
    }
    if (this.platform !== 'win32') {
      try {
        chmodSync(file, 0o755)
      } catch {
        // A read-only install still works when the archive kept the mode.
      }
    }
    this.#executable = file
    return file
  }

  /**
   * Run one request and return the decoded response body.
   * @param {Record<string, unknown>} request
   * @param {{ signal?: AbortSignal, timeoutMs?: number }} [options]
   * @returns {Promise<Record<string, any>>}
   */
  async call(request, options = {}) {
    const { signal } = options
    if (signal?.aborted) throw cancellationOf(signal)
    const executable = this.executable()
    const timeoutMs = options.timeoutMs ?? this.timeoutMs
    const payload = `${JSON.stringify(request)}\n`

    const child = spawn(executable, [], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true })

    return await new Promise((resolve, reject) => {
      /** @type {Buffer[]} */
      const chunks = []
      let received = 0
      let stderr = ''
      let settled = false
      let timedOut = false

      const timer = setTimeout(() => {
        timedOut = true
        kill(child)
        finish(() => reject(waveError('HELPER_TIMEOUT', `the wave-insight helper did not answer within ${Math.round(timeoutMs / 1000)}s`)))
      }, timeoutMs)

      const onAbort = () => {
        kill(child)
        finish(() => reject(cancellationOf(signal)))
      }

      const cleanup = () => {
        clearTimeout(timer)
        signal?.removeEventListener?.('abort', onAbort)
      }

      /** @param {() => void} settle */
      function finish(settle) {
        if (settled) return
        settled = true
        cleanup()
        settle()
      }

      signal?.addEventListener?.('abort', onAbort, { once: true })

      child.stdout.on('data', (chunk) => {
        if (settled) return
        received += chunk.length
        if (received > MAX_RESPONSE_BYTES) {
          kill(child)
          finish(() => reject(waveError('HELPER_BUFFER', 'the wave-insight helper response exceeded the 32 MB safety limit')))
          return
        }
        chunks.push(chunk)
      })
      child.stderr.on('data', (chunk) => {
        stderr = `${stderr}${chunk.toString('utf8')}`.slice(-MAX_STDERR_BYTES)
      })
      child.stdin.on('error', () => {
        // EPIPE when the helper exits before reading the request; 'close'
        // reports the real failure.
      })
      child.on('error', (error) => {
        finish(() => reject(waveError('HELPER_SPAWN', `could not start the wave-insight helper: ${error.message}`, { cause: error })))
      })
      child.on('close', (code) => {
        if (settled || timedOut) return
        if (code !== 0) {
          const tail = stderr.trim().replace(/\s+/g, ' ').slice(0, 400)
          finish(() => reject(waveError('HELPER_EXIT', `the wave-insight helper exited with code ${code}${tail ? `: ${tail}` : ''}`)))
          return
        }
        finish(() => {
          const text = Buffer.concat(chunks).toString('utf8')
          const line = text.split(/\r?\n/).find((part) => part.trim().length > 0)
          if (!line) {
            reject(waveError('HELPER_NO_RESPONSE', 'the wave-insight helper returned no response'))
            return
          }
          /** @type {Record<string, any>} */
          let response
          try {
            response = JSON.parse(line)
          } catch (error) {
            reject(waveError('HELPER_BAD_RESPONSE', 'the wave-insight helper returned malformed JSON', { cause: error }))
            return
          }
          if (response?.ok !== true) {
            reject(waveError('HELPER_ERROR', String(response?.error ?? 'the wave-insight helper reported an unknown error')))
            return
          }
          resolve(response)
        })
      })

      child.stdin.end(payload)
    })
  }

  /**
   * Open a dump and return its summary in Host shape.
   * @param {string} path
   * @param {{ limit?: number, signal?: AbortSignal, timeoutMs?: number }} [options]
   */
  async open(path, options = {}) {
    const response = await this.call(
      { cmd: 'open', path, limit: options.limit ?? 20 },
      { signal: options.signal, timeoutMs: options.timeoutMs },
    )
    return {
      name: String(response.name ?? path),
      path: String(response.path ?? path),
      format: String(response.format ?? 'unknown'),
      timescale: response.timescale == null ? null : String(response.timescale),
      timeRange: normalizeRange(response.time_range),
      signalCount: Number.isFinite(Number(response.signal_count)) ? Number(response.signal_count) : 0,
      signalsTruncated: response.signals_truncated === true,
      signals: Array.isArray(response.signals)
        ? response.signals.map((signal) => String(signal?.path ?? signal)).filter((value) => value.length > 0)
        : [],
    }
  }

  /**
   * Search signal paths.
   * @param {string} path
   * @param {string} query
   * @param {{ limit?: number, signal?: AbortSignal }} [options]
   */
  async find(path, query, options = {}) {
    const response = await this.call(
      { cmd: 'find', path, query, limit: options.limit ?? 50 },
      { signal: options.signal },
    )
    const matches = Array.isArray(response.matches) ? response.matches : []
    return matches
      .map((match) => ({
        path: String(match?.path ?? ''),
        width: Number.isFinite(Number(match?.width)) ? Number(match.width) : null,
      }))
      .filter((match) => match.path.length > 0)
  }

  /**
   * Read bounded value changes for several signals.
   * @param {string} path
   * @param {string[]} signals
   * @param {{ start: number, end?: number, max?: number, signal?: AbortSignal }} options
   */
  async waves(path, signals, options) {
    const request = {
      cmd: 'wave',
      path,
      signals,
      start: options.start,
      max: options.max ?? 32,
    }
    if (typeof options.end === 'number') request.end = options.end
    const response = await this.call(request, { signal: options.signal })
    const waves = Array.isArray(response.waves) ? response.waves : []
    return signals.map((_name, index) => {
      const wave = waves[index] ?? {}
      const changes = Array.isArray(wave.changes) ? wave.changes : []
      return {
        capped: wave.capped === true,
        changes: changes.map((change) => ({
          time: Number(change?.t),
          value: String(change?.v ?? ''),
        })),
      }
    })
  }

  /**
   * Check which paths exist without decoding any values.
   *
   * New helpers answer `validate` in one pass. Older bundled helpers only know
   * `wave`, so this falls back to peeling the first unknown name per call — the
   * behaviour the plugin had before, kept so a Linux/macOS install stays usable
   * with a helper built from an older revision.
   * @param {string} path
   * @param {string[]} names
   * @param {{ signal?: AbortSignal }} [options]
   * @returns {Promise<{ known: string[], unknown: string[] }>}
   */
  async validate(path, names, options = {}) {
    const wanted = [...new Set(names)]
    if (wanted.length === 0) return { known: [], unknown: [] }
    try {
      const response = await this.call({ cmd: 'validate', path, signals: wanted }, { signal: options.signal })
      const unknown = Array.isArray(response.unknown)
        ? response.unknown.map(String).filter((name) => wanted.includes(name))
        : []
      return { known: wanted.filter((name) => !unknown.includes(name)), unknown }
    } catch (error) {
      if (error?.code === 'HELPER_ERROR' && /unknown command:?\s*validate/i.test(error.message)) {
        return await this.#validateByPeeling(path, wanted, options)
      }
      throw error
    }
  }

  /**
   * The module definitions of the given sources, in Host shape.
   * @param {{ paths?: string[], dump?: string, limit?: number }} request
   * @param {{ signal?: AbortSignal }} [options]
   */
  async modules(request, options = {}) {
    const response = await this.call({ cmd: 'modules', ...request }, { signal: options.signal })
    const modules = Array.isArray(response.modules) ? response.modules : []
    return {
      files: Array.isArray(response.files) ? response.files.map(String) : [],
      moduleCount: Number.isFinite(Number(response.module_count)) ? Number(response.module_count) : modules.length,
      truncated: response.truncated === true,
      modules: modules.map((module) => ({
        name: String(module?.name ?? ''),
        file: String(module?.file ?? ''),
        line: Number.isFinite(Number(module?.line)) ? Number(module.line) : 0,
        ports: Array.isArray(module?.module_ports)
          ? module.module_ports.map((port) => ({
              name: String(port?.name ?? ''),
              direction: String(port?.direction ?? 'unknown'),
              kind: String(port?.kind ?? 'other'),
              width: Number.isFinite(Number(port?.width)) ? Number(port.width) : null,
            }))
          : [],
        instances: Array.isArray(module?.instances)
          ? module.instances.map((instance) => ({
              name: String(instance?.name ?? ''),
              module: String(instance?.module ?? ''),
            }))
          : [],
      })),
    }
  }

  /**
   * Where one hierarchical signal is declared, plus the spans that drive and
   * read it. `declared: false` is a real answer: the design does not know the
   * path, and the caller reports that instead of inventing a location.
   * @param {{ paths?: string[], dump?: string, top?: string, signal: string }} request
   * @param {{ signal?: AbortSignal }} [options]
   */
  async declare(request, options = {}) {
    const response = await this.call({ cmd: 'declare', ...request }, { signal: options.signal })
    const declaration = response.declaration
    return {
      declared: response.declared === true,
      signal: String(response.signal ?? request.signal),
      top: String(response.top ?? ''),
      module: String(response.module ?? ''),
      instance: String(response.instance ?? ''),
      kind: String(response.kind ?? 'other'),
      direction: response.direction == null ? null : String(response.direction),
      width: Number.isFinite(Number(response.width)) ? Number(response.width) : null,
      location: response.location == null ? null : String(response.location),
      declaration:
        declaration && typeof declaration === 'object'
          ? { file: String(declaration.file ?? ''), line: Number(declaration.line ?? 0), col: Number(declaration.col ?? 0) }
          : null,
      drivers: referenceList(response.drivers),
      loads: referenceList(response.loads),
      diagnostics: diagnosticList(response.diagnostics),
    }
  }

  /**
   * What the parser or the elaborator could not resolve.
   * @param {{ paths?: string[], dump?: string, top?: string }} request
   * @param {{ signal?: AbortSignal }} [options]
   */
  async diagnostics(request, options = {}) {
    const response = await this.call({ cmd: 'diagnostics', ...request }, { signal: options.signal })
    const entries = diagnosticList(response.diagnostics)
    return {
      modules: Number.isFinite(Number(response.modules)) ? Number(response.modules) : 0,
      top: response.top == null ? null : String(response.top),
      count: Number.isFinite(Number(response.count)) ? Number(response.count) : entries.length,
      truncated: response.truncated === true,
      diagnostics: entries,
    }
  }

  /**
   * The companion RTL next to a dump (the SpinalHDL layout), by absolute path.
   * @param {string} path the waveform file
   * @param {{ signal?: AbortSignal }} [options]
   */
  async sources(path, options = {}) {
    const response = await this.call({ cmd: 'sources', dump: path }, { signal: options.signal })
    return Array.isArray(response.paths) ? response.paths.map(String) : []
  }

  /**
   * Fallback for helpers without a `validate` command.
   * @param {string} path
   * @param {string[]} wanted
   * @param {{ signal?: AbortSignal }} options
   */
  async #validateByPeeling(path, wanted, options) {
    /** @type {string[]} */
    const unknown = []
    let rest = [...wanted]
    while (rest.length > 0) {
      try {
        await this.call(
          { cmd: 'wave', path, signals: rest, start: 0, end: 0, max: 0 },
          { signal: options.signal },
        )
        break
      } catch (error) {
        const text = String(error?.detail ?? error?.message ?? '')
        const match = /unknown signal:\s*(.+)$/.exec(text)
        const name = match?.[1]?.trim()
        if (!name || !rest.includes(name)) throw error
        unknown.push(name)
        rest = rest.filter((item) => item !== name)
      }
    }
    return { known: rest, unknown }
  }
}

/**
 * Driver/load references as `{ location, bits }`, dropping anything the helper
 * did not fill in.
 * @param {unknown} value
 * @returns {Array<{ location: string, bits: string | null }>}
 */
function referenceList(value) {
  if (!Array.isArray(value)) return []
  return value
    .map((entry) => ({
      location: String(entry?.location ?? ''),
      bits: entry?.bits == null ? null : String(entry.bits),
    }))
    .filter((entry) => entry.location.length > 0)
}

/**
 * Diagnostics as `{ severity, kind, message, location }`; the span becomes one
 * `file:line:col` string because that is what a model quotes back.
 * @param {unknown} value
 * @returns {Array<{ severity: string, kind: string, message: string, location: string | null }>}
 */
function diagnosticList(value) {
  if (!Array.isArray(value)) return []
  return value.map((entry) => {
    const span = entry?.span
    const location =
      span && typeof span === 'object'
        ? `${String(span.file ?? '')}:${Number(span.line ?? 0)}:${Number(span.col ?? 0)}`
        : null
    return {
      severity: String(entry?.severity ?? 'error'),
      kind: String(entry?.kind ?? 'unknown'),
      message: String(entry?.message ?? ''),
      location,
    }
  })
}
/** @param {import('node:child_process').ChildProcess} child */
function kill(child) {
  try {
    child.kill()
  } catch {
    // Already gone.
  }
}
