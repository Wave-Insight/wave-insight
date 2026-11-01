// Builds the corrupt-dump fixtures the `corruption` scenario uploads.
//
// They are derived from the committed valid fixtures (so they stay tiny and
// reproducible) and exercise the three failure paths the Wave 7 hardening
// added to wave-insight-lib:
//
//   truncated.fst  first half of io_mode_wave.fst: the structural walk in
//                  waveform/source.rs must report a truncated/corrupt FST
//                  instead of letting the reader allocate from a bogus length
//   flipped.fst    io_mode_wave.fst with the first data section's meta
//                  uncompressed-length u64 set to u64::MAX: the plausibility
//                  check (plausible_decoded_length) must reject it instead of
//                  the old multi-terabyte allocation abort
//   truncated.vcd  wave.vcd cut in the middle of a value-change line: the VCD
//                  parser must report a readable error
//
//   node qa/fixtures/generators/gen-corrupt.js
const fs = require('node:fs');
const path = require('node:path');

const FIXTURES = path.join(__dirname, '..');
const read = (name) => fs.readFileSync(path.join(FIXTURES, name));
const write = (name, bytes) => {
  fs.writeFileSync(path.join(FIXTURES, name), bytes);
  return bytes.length;
};

// --- truncated FST: the first half of the valid dump -----------------------
const fst = read('io_mode_wave.fst');
const truncatedFst = fst.subarray(0, Math.floor(fst.length / 2));
const truncatedBytes = write('truncated.fst', truncatedFst);

// --- byte-flipped FST: a data section claims an absurd uncompressed length -
// Top-level block layout: [type u8][section_length u64 big endian][payload].
const flipped = Buffer.from(fst);
let cursor = 0;
let flippedAt = -1;
while (cursor + 9 <= flipped.length) {
  const type = flipped[cursor];
  const blockLen = Number(flipped.readBigUInt64BE(cursor + 1));
  if (blockLen < 8 || cursor + 1 + blockLen > flipped.length) break;
  // VcData / dynamic alias sections carry their time-table lengths in the
  // last 24 bytes: [uncompressed u64][compressed u64][items u64].
  if ((type === 1 || type === 5 || type === 8) && blockLen >= 32) {
    const meta = cursor + 1 + blockLen - 24;
    flipped.writeBigUInt64BE(0xffffffffffffffffn, meta);
    flippedAt = meta;
    break;
  }
  cursor = cursor + 1 + blockLen;
}
if (flippedAt < 0) throw new Error('no FST data section found to corrupt');
const flippedBytes = write('flipped.fst', flipped);

// --- truncated VCD: cut inside the header, before $enddefinitions ----------
// A VCD has no trailer and no length field, so cutting only value changes off
// the end leaves a file that is still a valid (shorter) dump: wellen accepts
// EOF there and the app opens it. The corruption that must be rejected is a
// truncated *header* (the $var list is cut, $enddefinitions is missing), so the
// fixture cuts in the middle of the declaration section.
const vcd = read('wave.vcd').toString('latin1');
const endDefinitions = vcd.indexOf('$enddefinitions');
if (endDefinitions < 0) throw new Error('wave.vcd has no $enddefinitions');
const cut = endDefinitions - 30;
const truncatedVcd = Buffer.from(vcd.slice(0, cut), 'latin1');
const truncatedVcdBytes = write('truncated.vcd', truncatedVcd);

console.log('corrupt fixtures written:');
console.log('  truncated.fst ' + truncatedBytes + ' bytes (half of io_mode_wave.fst)');
console.log('  flipped.fst   ' + flippedBytes + ' bytes (data-section length at byte ' + flippedAt + ' set to u64::MAX)');
console.log('  truncated.vcd ' + truncatedVcdBytes + ' bytes (wave.vcd header cut at byte ' + cut + ', mid $var line)');
