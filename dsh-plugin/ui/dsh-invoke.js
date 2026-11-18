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

  // serde_wasm_bindgen::to_value produces a JS Map for a serde_json object, and
  // JSON.stringify renders a Map as "{}" — which is how every open_dump arrived
  // at the backend without its path. Convert argument values to plain JSON
  // shapes before they cross the fetch boundary.
  function toPlain(value) {
    if (value === null || typeof value !== 'object') {
      return value;
    }
    if (value instanceof Map) {
      var mapped = {};
      value.forEach(function (item, key) {
        mapped[String(key)] = toPlain(item);
      });
      return mapped;
    }
    if (value instanceof Set) {
      var items = [];
      value.forEach(function (item) {
        items.push(toPlain(item));
      });
      return items;
    }
    if (Array.isArray(value)) {
      return value.map(toPlain);
    }
    if (ArrayBuffer.isView(value)) {
      return Array.prototype.slice.call(value);
    }
    var copy = {};
    for (var key in value) {
      if (Object.prototype.hasOwnProperty.call(value, key)) {
        copy[key] = toPlain(value[key]);
      }
    }
    return copy;
  }

  function invoke(cmd, args, keepalive) {
    var headers = { 'content-type': 'application/json' };
    if (token) {
      headers['x-wave-token'] = token;
    }
    return fetch('/invoke', {
      method: 'POST',
      headers: headers,
      body: JSON.stringify({ cmd: cmd, args: toPlain(args) }),
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

  // Announce readiness once. The Rust app replaces the loader markup with its
  // own DOM when it mounts, and attaches the `wave-web-command` listener in the
  // same render — announcing at DOMContentLoaded (as this shim used to) tells
  // the host to send commands before anything is listening, so they are lost.
  // Wait for the mounted app instead, with a bounded fallback.
  var announced = false;
  function announceReady() {
    if (announced) {
      return;
    }
    announced = true;
    try {
      if (window.parent && window.parent !== window) {
        // `boot` distinguishes this post-mount announce from the app's own
        // announce after it adopts a dump: only the former means "the listener
        // exists but nothing is loaded yet".
        window.parent.postMessage({ type: 'wave-dsh-ready', boot: true }, parentOrigin || '*');
      }
    } catch (error) {
      // A cross-origin parent cannot be signalled; ignore it.
    }
  }

  var appMounted = function () {
    var body = document.body;
    return Boolean(body && body.querySelector('main, div, canvas'));
  };
  var waited = 0;
  function watchForApp() {
    if (announced) {
      return;
    }
    if (appMounted()) {
      // The listener is attached during the same render; hand the parent the
      // message on the next task so the frame is fully consistent.
      setTimeout(announceReady, 0);
      return;
    }
    waited += 50;
    if (waited >= 5000) {
      announceReady();
      return;
    }
    setTimeout(watchForApp, 50);
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', watchForApp, { once: true });
  } else {
    watchForApp();
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
