// Deterministic DENSE VCD generator for the denseDump QA scenario and the
// large-dump reproduction.
//
// Dense means: changes per byte, not bytes per se. The clock toggles on every
// time step, so a 10M-step dump is 10M+ changes in ~120 MB - the shape that
// forces a full-history decode in the renderer (the FST blow-up proxy).
//
//   node qa/fixtures/generators/gen-dense.js <out.vcd> <steps> [signals]
//
// The `denseDump` scenario requires() this module and generates its small
// default into qa/artifacts/dense/ (gitignored) at run time.
const fs = require('node:fs');

function generateDense(filePath, options = {}) {
  const steps = options.steps || 200000;
  const signals = options.signals || 1;
  const out = fs.createWriteStream(filePath);
  let written = 0;
  let buffer = [];
  const flush = () => { const text = buffer.join(''); buffer = []; out.write(text); written += Buffer.byteLength(text); };
  const lines = [];
  lines.push('$date qa dense fixture $end');
  lines.push('$version wave-insight qa $end');
  lines.push('$timescale 1ns $end');
  lines.push('$scope module dense $end');
  lines.push('  $var wire 1 !! clk $end');
  for (let index = 0; index < signals; index++) {
    lines.push('  $var wire 8 !' + String.fromCharCode(35 + (index % 60)) + ' data_' + String(index).padStart(3, '0') + ' [7:0] $end');
  }
  lines.push('$upscope $end');
  lines.push('$enddefinitions $end');
  lines.push('$dumpvars');
  lines.push('0!!');
  for (let index = 0; index < signals; index++) lines.push('b0 !' + String.fromCharCode(35 + (index % 60)));
  lines.push('$end');
  buffer.push(lines.join('\n') + '\n');
  let time = 0;
  for (let step = 0; step < steps; step++) {
    time += 1;
    buffer.push('#' + time + '\n');
    buffer.push((step % 2 === 0 ? '1' : '0') + '!!\n');
    for (let index = 0; index < signals; index++) {
      const value = (step + index * 17) & 0xff;
      buffer.push('b' + value.toString(2).padStart(8, '0') + ' !' + String.fromCharCode(35 + (index % 60)) + '\n');
    }
    if (buffer.length >= 8192) flush();
  }
  flush();
  return new Promise((resolve, reject) => {
    out.end((error) => (error ? reject(error) : resolve({ path: filePath, bytes: written, steps: steps, signals: signals })));
  });
}

module.exports = { generateDense };

if (require.main === module) {
  const target = process.argv[2];
  const steps = Number(process.argv[3] || 200000);
  const signals = Number(process.argv[4] || 1);
  if (!target) { console.error('usage: node gen-dense.js <out.vcd> <steps> [signals]'); process.exit(2); }
  const started = Date.now();
  generateDense(require('node:path').resolve(target), { steps: steps, signals: signals })
    .then((result) => console.log(JSON.stringify(Object.assign({ ms: Date.now() - started }, result))))
    .catch((error) => { console.error('generate failed: ' + (error && error.stack)); process.exit(1); });
}
