// Lifecycle for the loopback viewer backend.
//
// The backend is one child process per Host process: it hosts the built viewer
// and its `/invoke` route, reading dumps through the native memory-mapped
// reader. Every plugin instance that mounts shares it through a refcount, so a
// second row (or a preset reload) cannot kill the panel of the first.
//
// Failure is recorded, never thrown at load: the model-facing tools keep
// working when the viewer is unavailable, and the panel shows the reason.
import { spawn } from 'node:child_process'
import { chmodSync, existsSync } from 'node:fs'
import path from 'node:path'
import { waveError } from './errors.js'

const READY_TIMEOUT_MS = 10_000
const SHUTDOWN_GRACE_MS = 2_000
const LOG_TAIL = 4 * 1024

export class WebBackend {
  /** @type {import('node:child_process').ChildProcess | null} */
  #child = null
  /** @type {string | null} */
  #origin = null
  /** @type {string | null} */
  #token = null
  /** @type {string | null} */
  #error = null
  /** @type {Promise<string | null> | null} */
  #starting = null
  #stdoutTail = ''
  #stderrTail = ''
  #closing = false

  /**
   * @param {{ asset: { path?: string, candidates?: string[] }, uiDir: string, platform?: NodeJS.Platform }} options
   */
  constructor(options) {
    this.asset = options.asset
    this.uiDir = options.uiDir
    this.platform = options.platform ?? process.platform
  }

  /** Origin the iframe loads from, including the access token, or null. */
  get clientUrl() {
    if (!this.#origin) return null
    return this.#token ? `${this.#origin}?t=${encodeURIComponent(this.#token)}` : this.#origin
  }

  /** Why the viewer is unavailable, or null. */
  get error() {
    return this.#error
  }

  /** Whether a backend process is currently serving. */
  get running() {
    return this.#origin !== null
  }

  /**
   * Start the backend once. Resolves with the client URL when it is serving,
   * `null` when it could not start; the reason is available from `error`.
   * @returns {Promise<string | null>}
   */
  start() {
    if (this.#origin) return Promise.resolve(this.clientUrl)
    if (this.#starting) return this.#starting
    this.#error = null
    this.#starting = this.#spawn()
      .catch((error) => {
        this.#error = error instanceof Error ? error.message : String(error)
        return null
      })
      .finally(() => {
        this.#starting = null
      })
    return this.#starting
  }

  /**
   * Wait at most `timeoutMs` for the backend, then report whatever is known.
   * @param {number} [timeoutMs]
   * @returns {Promise<string | null>}
   */
  async ready(timeoutMs = 5_000) {
    const pending = this.start()
    let timer
    const timeout = new Promise((resolve) => {
      timer = setTimeout(() => resolve(null), timeoutMs)
    })
    try {
      await Promise.race([pending, timeout])
    } finally {
      clearTimeout(timer)
    }
    return this.clientUrl
  }

  /** Kill the backend and wait for it to exit. Safe to call more than once. */
  async close() {
    this.#closing = true
    const child = this.#child
    this.#child = null
    this.#origin = null
    this.#token = null
    if (child && child.exitCode === null && !child.killed) {
      await new Promise((resolve) => {
        const timer = setTimeout(resolve, SHUTDOWN_GRACE_MS)
        child.once('close', () => {
          clearTimeout(timer)
          resolve()
        })
        kill(child)
      })
    }
    this.#closing = false
  }

  /** @returns {Promise<string | null>} */
  async #spawn() {
    const executable = this.asset?.path
    if (!executable) {
      const expected = this.asset?.candidates?.join(', ') || 'no candidate path'
      throw waveError(
        'UI_UNAVAILABLE',
        `no bundled wave-insight viewer backend for ${process.platform}/${process.arch}. Build one with "cargo build -p wave-insight-web-backend --release" and set the plugin config field "backendPath" (or WAVE_WEB_BACKEND). Looked for: ${expected}`,
      )
    }
    if (!existsSync(executable)) throw waveError('UI_UNAVAILABLE', `the wave-insight viewer backend is missing: ${executable}`)
    const index = path.join(this.uiDir, 'index.html')
    if (!existsSync(index)) {
      throw waveError('UI_UNAVAILABLE', `the built wave-insight viewer is missing: ${index}. Build the web-remote front end and copy it into dsh-plugin/ui.`)
    }
    if (this.platform !== 'win32') {
      try {
        chmodSync(executable, 0o755)
      } catch {
        // A read-only install still works when the archive kept the mode.
      }
    }

    const child = spawn(executable, ['--ui-dir', this.uiDir, '--port', '0'], {
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    })
    this.#child = child
    this.#stdoutTail = ''
    this.#stderrTail = ''

    child.stdout?.setEncoding('utf8')
    child.stderr?.setEncoding('utf8')
    // Keep draining both pipes for the child's whole life: an unread pipe fills
    // and blocks the backend inside a request handler.
    child.stdout?.on('data', (chunk) => {
      this.#stdoutTail = `${this.#stdoutTail}${chunk}`.slice(-LOG_TAIL)
      announce(chunk)
    })
    child.stderr?.on('data', (chunk) => {
      this.#stderrTail = `${this.#stderrTail}${chunk}`.slice(-LOG_TAIL)
    })

    /** @type {(port: number, token: string | null) => void} */
    let onReady = () => {}
    /** @type {(error: Error) => void} */
    let onFail = () => {}
    const handshake = new Promise((resolve, reject) => {
      onReady = (port, token) => resolve({ port, token })
      onFail = (error) => reject(error)
    })
    let settled = false
    const timer = setTimeout(() => {
      if (settled) return
      settled = true
      kill(child)
      onFail(waveError('UI_START_TIMEOUT', `the wave-insight viewer did not report a port within ${READY_TIMEOUT_MS / 1000}s`))
    }, READY_TIMEOUT_MS)

    /** @param {string} chunk */
    function announce(chunk) {
      if (settled) return
      const match = /LISTENING\s+(\d+)(?:\s+(\S+))?/.exec(chunk)
      if (!match) return
      settled = true
      clearTimeout(timer)
      onReady(Number(match[1]), match[2] ?? null)
    }

    child.on('error', (error) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      onFail(waveError('UI_UNAVAILABLE', `could not start the wave-insight viewer backend: ${error.message}`, { cause: error }))
    })
    child.on('exit', (code) => {
      clearTimeout(timer)
      const tail = this.#stderrTail.trim().replace(/\s+/g, ' ').slice(0, 300)
      if (!settled) {
        settled = true
        onFail(waveError('UI_UNAVAILABLE', `the wave-insight viewer backend exited with code ${code}${tail ? `: ${tail}` : ''}`))
      }
      if (this.#child === child) this.#child = null
      if (this.#origin) {
        this.#origin = null
        this.#token = null
        if (!this.#closing) {
          this.#error = `the wave-insight viewer backend stopped (exit ${code})${tail ? `: ${tail}` : ''}`
        }
      }
    })

    try {
      const { port, token } = /** @type {{ port: number, token: string | null }} */ (await handshake)
      this.#origin = `http://127.0.0.1:${port}/`
      this.#token = token
      this.#error = null
      return this.clientUrl
    } catch (error) {
      this.#error = error instanceof Error ? error.message : String(error)
      if (this.#child === child) this.#child = null
      await waitForExit(child)
      return null
    }
  }
}

/** @param {import('node:child_process').ChildProcess} child */
function kill(child) {
  try {
    child.kill()
  } catch {
    // Already gone.
  }
}

/** @param {import('node:child_process').ChildProcess} child */
async function waitForExit(child) {
  if (child.exitCode !== null) return
  await new Promise((resolve) => {
    const timer = setTimeout(resolve, SHUTDOWN_GRACE_MS)
    child.once('close', () => {
      clearTimeout(timer)
      resolve()
    })
    kill(child)
  })
}

/**
 * @typedef {{ daemon: WebBackend, refs: number }} Shared
 * @type {Shared | null}
 */
let shared = null

/**
 * Acquire the process-wide backend for one plugin instance.
 *
 * The first caller's binary path wins; later callers share the same child. The
 * last release stops it.
 * @param {{ asset: { path?: string, candidates?: string[] }, uiDir: string, platform?: NodeJS.Platform }} options
 * @returns {{ daemon: WebBackend, release: () => Promise<void> }}
 */
export function acquireWebBackend(options) {
  if (!shared) shared = { daemon: new WebBackend(options), refs: 0 }
  const entry = shared
  entry.refs += 1
  void entry.daemon.start()
  let released = false
  return {
    daemon: entry.daemon,
    async release() {
      if (released) return
      released = true
      entry.refs -= 1
      if (entry.refs > 0) return
      if (shared === entry) shared = null
      await entry.daemon.close()
    },
  }
}
