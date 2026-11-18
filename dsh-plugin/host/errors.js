// Error identity for the wave-insight Host half.
//
// DSH only persists a structured `{ name, code }` for its own `HarnessError`
// subclasses, and a plugin cannot import that class from the DSH installation,
// so the code is also written into the message the model sees. Cancellation is
// the one case that must stay untouched: the executor classifies a tool result
// as ABORTED when the thrown value is exactly `signal.reason` or an Error named
// `AbortError`, and wrapping it here would turn a cancel into a failure.

export class WaveInsightError extends Error {
  /**
   * @param {string} code stable, upper-snake reason token
   * @param {string} message model-facing explanation
   * @param {ErrorOptions} [options]
   */
  constructor(code, message, options) {
    super(`${message} (${code})`, options)
    this.name = 'WaveInsightError'
    this.code = code
    /** The message without the trailing code, for protocol-level matching. */
    this.detail = message
  }
}

/**
 * @param {string} code stable, upper-snake reason token
 * @param {string} message model-facing explanation
 * @param {ErrorOptions} [options]
 * @returns {WaveInsightError}
 */
export function waveError(code, message, options) {
  return new WaveInsightError(code, message, options)
}

/** @param {unknown} error @returns {boolean} */
export function isAbortError(error) {
  return error instanceof Error && error.name === 'AbortError'
}

/**
 * Reject an in-flight helper call with the caller's own cancellation reason so
 * DSH can still tell a cancelled turn from a failed one.
 * @param {AbortSignal | undefined} signal
 * @returns {unknown}
 */
export function cancellationOf(signal) {
  if (signal?.reason !== undefined) return signal.reason
  const error = new Error('wave-insight call aborted')
  error.name = 'AbortError'
  return error
}
