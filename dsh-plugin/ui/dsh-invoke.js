/*
 * DSH web-remote shim for the Wave Insight wasm app.
 *
 * Classic script (no modules), loaded before the wasm module. It gives the
 * remote Rust build the two JS entry points it expects:
 *
 *   window.__WAVE_INVOKE__(cmd, args) -> Promise<result JSON>
 *   window.__TAURI__.core.invoke(...) -> no-op (agent replies only)
 *
 * and forwards the host's `message` events into the DOM CustomEvent the Rust
 * web listener subscribes to.
 *
 * The DSH plugin starts this backend and embeds a per-process token in the
 * iframe URL (`?t=...`); every /invoke carries it so a page on another origin
 * cannot drive the dump registry.
 */
(function () {
  'use strict';

  if (typeof window === 'undefined') {
    return;
  }

  var token = '';
  try {
    token = new URLSearchParams(window.location.search).get('t') || '';
  } catch (error) {
    token = '';
  }

  // The page that framed us; '' when the referrer is hidden.
  var parentOrigin = '';
  try {
    parentOrigin = document.referrer ? new URL(document.referrer).origin : '';
  } catch (error) {
    parentOrigin = '';
  }

  function invoke(cmd, args, keepalive) {
    var headers = { 'content-type': 'application/json' };
    if (token) {
      headers['x-wave-token'] = token;
    }
    return fetch('/invoke', {
      method: 'POST',
      headers: headers,
      body: JSON.stringify({ cmd: cmd, args: args }),
      keepalive: keepalive === true
    }).then(function (response) {
      if (!response.ok) {
        throw new Error('wave invoke ' + cmd + ' failed: HTTP ' + response.status);
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
  }

  // Track the dumps this page opened, so a reload frees the native mappings
  // instead of leaking one per reload until the 32-dump cap is reached.
  var openIds = [];
  function track(cmd, args, result) {
    if (cmd === 'open_dump' && result && typeof result.id === 'number') {
      openIds.push(result.id);
    } else if (cmd === 'close_dump' && args && typeof args.id === 'number') {
      openIds = openIds.filter(function (id) { return id !== args.id; });
    } else if (cmd === 'close_all_dumps') {
      openIds = [];
    }
  }

  window.__WAVE_INVOKE__ = function (cmd, args) {
    return invoke(cmd, args, false).then(function (result) {
      track(cmd, args, result);
      return result;
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

  // One-way commands from the DSH host. Only the framing window may drive the
  // app, and only from the origin that framed it.
  window.addEventListener('message', function (event) {
    if (!event || event.source !== window.parent) {
      return;
    }
    if (parentOrigin && event.origin !== parentOrigin) {
      return;
    }
    var data = event.data;
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
        window.parent.postMessage({ type: 'wave-dsh-ready' }, parentOrigin || '*');
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

  // A page going away for good frees its native mappings; a bfcache page keeps
  // them, because it will resume with the same dump ids.
  window.addEventListener('pagehide', function (event) {
    if (event && event.persisted) {
      return;
    }
    var ids = openIds;
    openIds = [];
    for (var index = 0; index < ids.length; index += 1) {
      try {
        invoke('close_dump', { id: ids[index] }, true).catch(function () {});
      } catch (error) {
        // Best effort on unload.
      }
    }
  });
})();
