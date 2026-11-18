// Host half of the wave-insight DSH bundle.
//
// The implementation is split under ./host: asset resolution, the JSON-lines
// bridge to the native helper, the loopback viewer backend, the `waveInsight`
// session projection, the per-session view cache, and the tool definitions.
// This entry only re-exports the three things the Loader reads.
export { apply, Config, inject } from './host/plugin.js'
