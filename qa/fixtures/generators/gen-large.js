// Deterministic large VCD generator for the large-dump QA scenarios.
//
// Writes a realistic dump (thousands of signals, a mix of 1-bit wires and
// 8/16/32/64-bit buses, many time steps, some x/z values) of a requested byte
// size. The content is deterministic (fixed LCG), so a failing size is
// reproducible run to run.
//
//   node qa/fixtures/generators/gen-large.js <out.vcd> <megabytes>
//
// The `largeDump` scenario also requires() this module and generates its
// smallest size into qa/artifacts/large/ (gitignored) at run time.
const fs = require('node:fs');
const path = require('node:path');

const MB = 1024 * 1024;
// VCD identifiers are printable ASCII (33..126); encode the index as a base-94
// counter so every signal gets a unique id at any signal count (the old
// two-character scheme over a 63-char alphabet collided above 3969 signals and
// produced malformed dumps: the same id declared with conflicting widths).
const ID_CHARS = Array.from({ length: 94 }, (_, offset) => String.fromCharCode(33 + offset));
const idFor = (index) => {
  let value = index;
  let id = '';
  do {
    id += ID_CHARS[value % ID_CHARS.length];
    value = Math.floor(value / ID_CHARS.length);
  } while (value > 0);
  return id;
};

// Deterministic pseudo random values (xorshift), no Math.random anywhere.
function makeRandom(seed) {
  let state = seed >>> 0;
  return () => {
    state ^= state << 13; state >>>= 0;
    state ^= state >>> 17;
    state ^= state << 5; state >>>= 0;
    return state;
  };
}

function header(signals, options = {}) {
  const lines = [];
  lines.push('$date qa large-dump fixture $end');
  lines.push('$version wave-insight qa $end');
  lines.push('$timescale 1ns $end');
  lines.push('$scope module top $end');
  for (let index = 0; index < signals; index++) {
    const width = options.forceWidth || (options.widths ? options.widths[index % options.widths.length] : (index % 7 === 0 ? 64 : index % 5 === 0 ? 32 : index % 3 === 0 ? 16 : index % 2 === 0 ? 8 : 1));
    const id = idFor(index);
    const name = 'sig_' + String(index).padStart(5, '0');
    if (width === 1) lines.push('  $var wire 1 ' + id + ' ' + name + ' $end');
    else lines.push('  $var wire ' + width + ' ' + id + ' ' + name + ' [' + (width - 1) + ':0] $end');
  }
  lines.push('$upscope $end');
  lines.push('$enddefinitions $end');
  return lines.join('\n') + '\n';
}

function generate(filePath, targetBytes, options = {}) {
  const signals = options.signals || 1400;
  const changedPerStep = options.changedPerStep || Math.max(64, Math.floor(signals / 5));
  const random = makeRandom(0x5eed);
  const out = fs.createWriteStream(filePath);
  let written = 0;
  let buffer = [];
  const flush = () => { const text = buffer.join(''); buffer = []; out.write(text); written += Buffer.byteLength(text); };
  buffer.push(header(signals, options));
  // initial values
  buffer.push('$dumpvars\n');
  for (let index = 0; index < signals; index++) {
    const width = options.forceWidth || (options.widths ? options.widths[index % options.widths.length] : (index % 7 === 0 ? 64 : index % 5 === 0 ? 32 : index % 3 === 0 ? 16 : index % 2 === 0 ? 8 : 1));
    const id = idFor(index);
    if (width === 1) buffer.push('0' + id + '\n');
    else buffer.push('b' + '0'.repeat(width) + ' ' + id + '\n');
  }
  buffer.push('$end\n');
  let time = 0;
  let step = 0;
  while (written < targetBytes) {
    buffer.push('#' + time + '\n');
    for (let k = 0; k < changedPerStep; k++) {
      const index = (step * changedPerStep + k * 7) % signals;
      const width = options.forceWidth || (options.widths ? options.widths[index % options.widths.length] : (index % 7 === 0 ? 64 : index % 5 === 0 ? 32 : index % 3 === 0 ? 16 : index % 2 === 0 ? 8 : 1));
      const id = idFor(index);
      if (width === 1) {
        const value = random() % 5;
        buffer.push(value === 0 ? 'x' + id + '\n' : value === 1 ? 'z' + id + '\n' : String(value % 2) + id + '\n');
      } else {
        let bits = '';
        for (let bit = 0; bit < width; bit++) bits += String((random() >>> (bit % 31)) & 1);
        buffer.push('b' + bits + ' ' + id + '\n');
      }
    }
    step += 1;
    time += 10;
    if (buffer.length >= 4096) flush();
  }
  flush();
  return new Promise((resolve, reject) => {
    out.end((error) => (error ? reject(error) : resolve({ path: filePath, bytes: written, signals: signals, steps: step })));
  });
}

module.exports = { generate, MB };

if (require.main === module) {
  const target = process.argv[2];
  const megabytes = Number(process.argv[3] || 30);
  if (!target) { console.error('usage: node gen-large.js <out.vcd> <megabytes>'); process.exit(2); }
  const started = Date.now();
  generate(path.resolve(target), megabytes * MB)
    .then((result) => console.log(JSON.stringify(Object.assign({ ms: Date.now() - started }, result))))
    .catch((error) => { console.error('generate failed: ' + (error && error.stack)); process.exit(1); });
}
