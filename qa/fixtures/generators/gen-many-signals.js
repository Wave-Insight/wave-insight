// Generate a dump too big for the sidebar to draw row by row: the top scope
// declares 2100 signals and 120 child scopes, so both the signal list and the
// module tree have to virtualize (see the QAs "virtualization" scenario).
//
// The point is the row count, not the waveform: the body only initialises every
// signal and changes a handful of them, so the file stays small enough to load
// in a browser test while the middleware (open + first parse) is fast.
const fs = require('node:fs');
const path = require('node:path');

const TOP_SIGNALS = 2100;
const CHILD_SCOPES = 120;
const CHILD_SIGNALS = 2;

// VCD identifiers are printable ascii; two characters from this alphabet cover
// every variable the dump declares (63 * 63 = 3969 > 2340).
const ALPHABET = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_';
const idFor = (index) =>
  ALPHABET[Math.floor(index / ALPHABET.length)] + ALPHABET[index % ALPHABET.length];

const lines = [];
const push = (line) => lines.push(line);

push('$date qa many-signals fixture $end');
push('$version wave-insight qa $end');
push('$timescale 1ns $end');

// one id per variable, shared by the header and the body
const vars = [];

push('$scope module top $end');
for (let index = 0; index < TOP_SIGNALS; index++) {
  const id = idFor(vars.length);
  const name = 'sig_' + String(index).padStart(4, '0');
  vars.push({ id, name });
  push('  $var wire 1 ' + id + ' ' + name + ' $end');
}
for (let scope = 0; scope < CHILD_SCOPES; scope++) {
  push('  $scope module u' + String(scope).padStart(3, '0') + ' $end');
  for (let bit = 0; bit < CHILD_SIGNALS; bit++) {
    const id = idFor(vars.length);
    const name = 'sig_' + String.fromCharCode(97 + bit);
    vars.push({ id, name });
    push('    $var wire 1 ' + id + ' ' + name + ' $end');
  }
  push('  $upscope $end');
}
push('$upscope $end');
push('$enddefinitions $end');

push('$dumpvars');
for (let index = 0; index < vars.length; index++) {
  push((index % 3 === 0 ? '1' : '0') + vars[index].id);
}
push('$end');

// a few real transitions, so the dump is not a single static sample
for (let step = 1; step <= 3; step++) {
  push('#' + step * 10);
  for (let index = 0; index < 40; index++) {
    const variable = vars[(step * 40 + index) % vars.length];
    push((step % 2 === 0 ? '0' : '1') + variable.id);
  }
}

// the fixture lives next to the other dumps, not next to its generator
const target = path.join(__dirname, '..', 'many_signals.vcd');
fs.writeFileSync(target, lines.join('\n') + '\n', 'ascii');
console.log(
  'many_signals.vcd: ' + vars.length + ' signals, ' + lines.length + ' lines, ' +
    (fs.statSync(target).size / 1024).toFixed(0) + ' KB'
);
