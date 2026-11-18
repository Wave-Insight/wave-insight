// Minimal structural schemas for the session projection.
//
// The projection registry only calls `schema.parse(value)` (its own internal
// type is `{ parse(value: unknown): unknown }`), so a schema does not have to
// be a `zod` instance. Writing it by hand keeps this package dependency-free:
// a DSH plugin installed by directory link gets no `node_modules`, and an
// `import 'zod'` would then resolve to whatever copy happens to sit on a parent
// path — which is exactly how a 4.x-declared plugin ends up loading 3.x.

/**
 * @param {string} label
 * @param {(value: unknown) => string | null} check returns an error message, or null when valid
 */
export function createSchema(label, check) {
  return {
    /**
     * @param {unknown} value
     * @returns {unknown}
     */
    parse(value) {
      const issue = check(value)
      if (issue) {
        const error = new Error(`wave-insight: invalid ${label}: ${issue}`)
        error.name = 'WaveInsightSchemaError'
        throw error
      }
      return value
    },
    /**
     * @param {unknown} value
     */
    safeParse(value) {
      try {
        return { success: true, data: this.parse(value) }
      } catch (error) {
        return { success: false, error }
      }
    },
  }
}

/** @param {unknown} value @returns {value is Record<string, any>} */
export function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

/** @param {unknown} value @returns {boolean} */
export function isInteger(value) {
  return typeof value === 'number' && Number.isInteger(value)
}

/** @param {unknown} value @returns {boolean} */
export function isStringOrNull(value) {
  return value === null || typeof value === 'string'
}

/**
 * @param {unknown} value
 * @returns {string | null}
 */
export function checkRange(value) {
  if (value === null) return null
  if (!isRecord(value)) return 'range must be an object or null'
  if (!isInteger(value.start) || !isInteger(value.end)) return 'range bounds must be integers'
  return null
}
