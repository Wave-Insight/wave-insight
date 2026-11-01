// Generate the design/dump mismatch fixture used by the task-13 browser checks.
//
// The sources declare `module top` (with `u_cpu` / `u_cpu16` instances), but the
// dump's root scope is `tb_top`: the two hierarchies describe the same signals
// with a differently spelled root, which is what breaks naive path lookups.
// A second scope `x` holds its own `clk`, so the dump path `tb_top.x.clk` has no
// direct design counterpart and deliberately resolves to two candidates.
const fs = require('node:fs');
const path = require('node:path');

const src = fs.readFileSync(path.join(__dirname, 'wave.vcd'), 'ascii');
const lines = src.split(/\r?\n/);
const out = [];
let renamed = false;
let patched = false;
for (const line of lines) {
  if (!renamed && line.trim() === '$scope module top $end') {
    out.push('$scope module tb_top $end');
    renamed = true;
    continue;
  }
  // Add a sibling scope `x` with a 1-bit clk inside tb_top: its dump path
  // (`tb_top.x.clk`) only resolves against the design through the local-name
  // fallback, where two `clk` signals exist -> ambiguous on purpose.
  // The first unindented $upscope closes tb_top: add the sibling scope just
  // before it.
  if (!patched && line === '$upscope $end') {
    out.push('  $scope module x $end');
    out.push('    $var wire 1 ~ clk $end');
    out.push('  $upscope $end');
    patched = true;
  }
  out.push(line);
}
// give the added signal a value change so it is a real waveform
const firstHash = out.findIndex((l) => l === '#0');
if (firstHash >= 0) out.splice(firstHash + 1, 0, '0~');
out.push('#1605');
out.push('1~');

fs.writeFileSync(path.join(__dirname, 'mismatch.vcd'), out.join('\n') + '\n', 'ascii');
console.log('mismatch.vcd: renamed=' + renamed + ' patched=' + patched + ' lines=' + out.length);
