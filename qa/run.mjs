#!/usr/bin/env node
// One cross-platform runner for the browser QA matrix.
//
//   node qa/run.mjs                          # full matrix against front-end/dist
//   node qa/run.mjs --scenarios empty,vcd    # a subset
//   node qa/run.mjs --layout                 # R2 layout rules are required
//   node qa/run.mjs --offline                # self-hosted monaco check only
//
// What it does, in order: stages front-end/dist into qa/.stage, starts the
// static server and a headless Chrome over CDP, runs the matrix, tears both down
// and exits with the matrix exit code (non-zero on failures).
//
// Chrome is found via the CHROME environment variable, otherwise the usual
// per-OS locations. Ports are picked free at run time, so two runs (or the
// offline check sharing the server) cannot collide.
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const QA = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.dirname(QA);
const STAGE = path.join(QA, '.stage');
const SHOTS = path.join(QA, 'artifacts');
const FIXTURES = path.join(QA, 'fixtures');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function usage() {
  console.log(`usage: node qa/run.mjs [--scenarios a,b] [--layout] [--offline] [--dist <dir>] [--keep] [--list]

  --scenarios   comma/space separated scenario names (default: the full matrix)
  --layout      QA_LAYOUT=1: the R2 layout rules are required, not pending
  --offline     run monaco-offline.js (CDN-free editor check) instead of the matrix
  --dist        build to stage (default: front-end/dist)
  --keep        keep qa/.stage after the run
  --list        print the scenario names and exit
  CHROME        path to a Chrome/Chromium binary`);
}

function parseArgs(argv) {
  const out = { scenarios: [], layout: false, offline: false, dist: path.join(REPO, 'front-end', 'dist'), keep: false, list: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--help' || arg === '-h') { out.help = true; }
    else if (arg === '--layout') { out.layout = true; }
    else if (arg === '--offline') { out.offline = true; }
    else if (arg === '--keep') { out.keep = true; }
    else if (arg === '--list') { out.list = true; }
    else if (arg === '--scenarios' || arg === '-Scenarios') { out.scenarios.push(...String(argv[++i] || '').split(/[,\s]+/).filter(Boolean)); }
    else if (arg.startsWith('--scenarios=')) { out.scenarios.push(...arg.slice(12).split(/[,\s]+/).filter(Boolean)); }
    else if (arg === '--dist' || arg === '-Dist') { out.dist = path.resolve(argv[++i] || ''); }
    else if (arg.startsWith('--dist=')) { out.dist = path.resolve(arg.slice(7)); }
    else if (arg.startsWith('-')) { throw new Error('unknown option: ' + arg); }
    else { out.scenarios.push(arg); }
  }
  return out;
}

function fail(message) {
  console.error('qa: ' + message);
  process.exit(2);
}

function whichSync(command) {
  const paths = (process.env.PATH || '').split(path.delimiter).filter(Boolean);
  const exts = process.platform === 'win32' ? (process.env.PATHEXT || '.EXE;.CMD;.BAT').split(';') : [''];
  for (const dir of paths) {
    for (const ext of exts) {
      const candidate = path.join(dir, command + ext);
      try { if (fs.statSync(candidate).isFile()) return candidate; } catch {}
    }
  }
  return null;
}

function findChrome() {
  if (process.env.CHROME) {
    if (!fs.existsSync(process.env.CHROME)) fail('CHROME points at a missing file: ' + process.env.CHROME);
    return process.env.CHROME;
  }
  const candidates = [];
  if (process.platform === 'win32') {
    candidates.push(
      path.join(process.env['ProgramFiles'] || 'C:\\Program Files', 'Google', 'Chrome', 'Application', 'chrome.exe'),
      path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Google', 'Chrome', 'Application', 'chrome.exe'),
      path.join(process.env.LOCALAPPDATA || '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
      path.join(process.env['ProgramFiles'] || 'C:\\Program Files', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
    );
  } else if (process.platform === 'darwin') {
    candidates.push(
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
      '/Applications/Chromium.app/Contents/MacOS/Chromium',
      '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
    );
  } else {
    for (const name of ['google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser', 'microsoft-edge']) {
      const found = whichSync(name);
      if (found) candidates.push(found);
    }
  }
  const found = candidates.find((candidate) => candidate && fs.existsSync(candidate));
  if (!found) {
    fail('no Chrome/Chromium found. Install one or set CHROME=<path>.\n  looked at:\n    ' + candidates.filter(Boolean).join('\n    '));
  }
  return found;
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

async function waitHttp(url, what, timeoutMs = 30000) {
  const end = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < end) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(2000) });
      if (res.ok) return;
      last = 'HTTP ' + res.status;
    } catch (error) { last = String(error && error.message); }
    await sleep(250);
  }
  fail(what + ' did not come up at ' + url + ' (' + last + ')');
}

function stage(dist) {
  if (!fs.existsSync(path.join(dist, 'index.html'))) {
    fail('no build at ' + dist + ' (expected index.html). Build the front end first:\n  cd front-end && trunk build --no-default-features --features wasm --release');
  }
  const wasm = fs.readdirSync(dist).filter((name) => name.endsWith('_bg.wasm'));
  if (!wasm.length) fail('no *_bg.wasm in ' + dist + ' — that is not a trunk build');
  fs.rmSync(STAGE, { recursive: true, force: true });
  fs.mkdirSync(STAGE, { recursive: true });
  fs.cpSync(dist, STAGE, { recursive: true });
  fs.mkdirSync(SHOTS, { recursive: true });
  console.log('qa: staged ' + dist + ' -> ' + path.relative(REPO, STAGE) + ' (' + wasm[0] + ')');
}

function listScenarios() {
  const source = fs.readFileSync(path.join(QA, 'qa.js'), 'utf8');
  const names = [...source.matchAll(/^scenarios\.(\w+) =/gm)].map((match) => match[1]);
  console.log(names.join('\n'));
  process.exit(0);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) { usage(); return 0; }
  if (args.list) { listScenarios(); return 0; }

  const chrome = findChrome();
  console.log('qa: chrome ' + chrome);
  stage(args.dist);

  const serverPort = await freePort();
  const cdpPort = await freePort();
  const appUrl = 'http://127.0.0.1:' + serverPort + '/';
  const profile = path.join(STAGE, 'chrome-profile');
  const env = Object.assign({}, process.env, {
    QA_PORT: String(serverPort),
    CDP_PORT: String(cdpPort),
    QA_SHOTS: SHOTS,
  });
  if (args.layout) env.QA_LAYOUT = '1'; else delete env.QA_LAYOUT;

  let server = null;
  let chromeProcess = null;
  // teardown kills both processes on purpose, so their exit codes must not be
  // reported as failures
  let stopping = false;
  const stop = () => {
    stopping = true;
    if (chromeProcess && chromeProcess.pid) {
      if (process.platform === 'win32') spawnSync('taskkill', ['/PID', String(chromeProcess.pid), '/T', '/F'], { stdio: 'ignore' });
      else { try { process.kill(-chromeProcess.pid, 'SIGKILL'); } catch { try { chromeProcess.kill('SIGKILL'); } catch {} } }
      chromeProcess = null;
    }
    if (server && server.pid) {
      if (process.platform === 'win32') spawnSync('taskkill', ['/PID', String(server.pid), '/T', '/F'], { stdio: 'ignore' });
      else { try { server.kill('SIGKILL'); } catch {} }
      server = null;
    }
  };

  try {
    server = spawn(process.execPath, [path.join(QA, 'serve.js'), STAGE, String(serverPort), FIXTURES], { stdio: 'ignore' });
    server.on('exit', (code) => { if (code && !stopping) console.error('qa: static server exited with ' + code); });
    await waitHttp(appUrl, 'static server');

    chromeProcess = spawn(chrome, [
      '--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
      '--disable-extensions', '--disable-background-networking', '--mute-audio',
      '--remote-debugging-port=' + cdpPort, '--remote-allow-origins=*',
      '--window-size=1440,900', '--user-data-dir=' + profile, 'about:blank',
    ], { stdio: 'ignore', detached: process.platform !== 'win32' });
    await waitHttp('http://127.0.0.1:' + cdpPort + '/json/version', 'headless chrome');

    let target;
    let childArgs;
    if (args.offline) {
      target = path.join(QA, 'monaco-offline.js');
      childArgs = [];
      env.QA_URL = appUrl;
      env.QA_DIST = STAGE;
    } else {
      target = path.join(QA, 'qa.js');
      childArgs = args.scenarios;
    }
    const label = args.offline ? 'monaco-offline' : (args.scenarios.length ? args.scenarios.join(',') : 'full matrix');
    console.log('qa: running ' + label + ' on ' + appUrl + ' (shots: ' + path.relative(REPO, SHOTS) + ')');

    const child = spawn(process.execPath, [target, ...childArgs], { env, stdio: 'inherit' });
    const code = await new Promise((resolve) => child.on('exit', (childCode, signal) => resolve(childCode === null ? 2 : childCode)));
    console.log('qa: ' + label + ' -> exit ' + code);
    return code;
  } finally {
    stop();
    if (!args.keep) { try { fs.rmSync(path.join(STAGE, 'chrome-profile'), { recursive: true, force: true }); } catch {} }
    await sleep(200);
  }
}

main().then((code) => { process.exit(code); }).catch((error) => { fail(String(error && error.stack || error)); });
