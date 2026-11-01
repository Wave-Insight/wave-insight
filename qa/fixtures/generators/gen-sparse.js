// Sparse QA waveform for the cursor-snap checks: one 1-bit signal that changes
// every 2000 time steps, so value edges are ~90 px apart at fit zoom and a
// snapping tolerance of a few pixels is observable.
const fs = require('node:fs');
const path = require('node:path');

const lines = ['$date qa sparse fixture $end', '$version wave-insight qa $end', '$timescale 1ns $end'];
lines.push('$scope module top $end');
lines.push('  $var wire 1 ! a $end');
lines.push('  $var wire 4 " b [3:0] $end');
lines.push('$upscope $end');
lines.push('$enddefinitions $end');
lines.push('$dumpvars');
lines.push('0!');
lines.push('b0000 "');
lines.push('$end');
const END = 20000;
const STEP = 2000;
for (let t = 0; t <= END; t += STEP) {
  lines.push('#' + t);
  if (t > 0) lines.push((t / STEP) % 2 === 0 ? '0!' : '1!');
  lines.push('b' + (t / STEP).toString(2).padStart(4, '0') + ' "');
}
fs.writeFileSync(path.join(__dirname, 'sparse.vcd'), lines.join('\n') + '\n', 'ascii');
console.log('sparse.vcd: ' + lines.length + ' lines, edges every ' + STEP + ' up to ' + END);
