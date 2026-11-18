/*
 * DSH web-remote shim for the Wave Insight wasm app.
 *
 * Classic script (no modules), safe to load before the wasm module. It gives
 * the remote Rust build the two JS entry points it expects:
 *
 *   window.__WAVE_INVOKE__(cmd, args) -> Promise<result JSON>
 *   window.__TAURI__.core.invoke(...) -> no-op (agent replies only)
 *
 * and forwards the host's `message` events into the DOM CustomEvent the Rust
 * web listener subscribes to.
 */
(function () {
  'use strict';

  if (typeof window === 'undefined') {
    return;
  }

  // Native dump commands: one POST per call to the loopback HTTP backend.
  window.__WAVE_INVOKE__ = function (cmd, args) {
    return fetch('/invoke', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ cmd: cmd, args: args })
    }).then(function (response) {
      if (!response.ok) {
        throw new Error(
          'wave invoke ' + cmd + ' failed: HTTP ' + response.status
        );
      }
      return response.json();
    }).then(function (payload) {
      // The Rust side decodes the raw result directly (same as Tauri IPC),
      // so unwrap our HTTP envelope and surface backend errors as rejections.
      if (!payload || payload.ok !== true) {
        throw new Error(
          'wave invoke ' + cmd + ' failed: ' +
          ((payload && payload.error) || 'unknown backend error')
        );
      }
      return payload.result;
    });
  };

  // The agent reply path uses __TAURI__.core.invoke; in the browser the reply
  // is fire-and-forget, so a no-op is enough. `event.listen` is defined too so
  // a stray tauri-gated listener would not crash the page.
  if (!window.__TAURI__) {
    window.__TAURI__ = {
      core: {
        invoke: async function () {
          return null;
        }
      },
      event: {
        listen: async function () {
          return function () {};
        }
      }
    };
  } else {
    if (!window.__TAURI__.core) {
      window.__TAURI__.core = {
        invoke: async function () {
          return null;
        }
      };
    }
    if (!window.__TAURI__.event) {
      window.__TAURI__.event = {
        listen: async function () {
          return function () {};
        }
      };
    }
  }

  // One-way commands from the DSH host.
  window.addEventListener('message', function (event) {
    var data = event && event.data;
    if (!data || data.type !== 'wave-dsh') {
      return;
    }
    window.dispatchEvent(
      new CustomEvent('wave-web-command', { detail: data.command })
    );
  });

  // Announce page readiness once. The Rust listener posts the same message
  // again after a dump is adopted; both are harmless (the client replays its
  // current view idempotently).
  var announced = false;
  function announceReady() {
    if (announced) {
      return;
    }
    announced = true;
    try {
      if (window.parent && window.parent !== window) {
        window.parent.postMessage({ type: 'wave-dsh-ready' }, '*');
      }
    } catch (error) {
      // A cross-origin parent cannot be signalled; ignore it.
    }
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', announceReady, { once: true });
  } else {
    announceReady();
  }
})();
