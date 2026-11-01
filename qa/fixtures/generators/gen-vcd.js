// Generate the QA waveform: two scope levels, 1-bit and multi-bit signals,
// x/z values, a few hundred changes.
const fs = require('node:fs');
const path = require('node:path');

const lines = [];
const push = (line) => lines.push(line);

push('$date qa fixture $end');
push('$version wave-insight qa $end');
push('$timescale 1ns $end');

// ids are printable VCD identifiers; one per variable
const vars = {
  topClk: '!', topReset: '"', topInstr: '#', topPc: '$', topData: '%', topResult: '&', topPc16: "'",
  cpuClk: '(', cpuReset: ')', cpuInstr: '*', cpuPc: '+', cpuData: ',', cpuNext: '-', cpuReg: '.', cpuActive: '/',
  cpu16Clk: 'a', cpu16Reset: 'b', cpu16Instr: 'c', cpu16Next: 'd', cpu16Reg: 'e', cpu16Active: 'f',
};

push('$scope module top $end');
push(`  $var wire 1 ${vars.topClk} clk $end`);
push(`  $var wire 1 ${vars.topReset} reset $end`);
push(`  $var wire 8 ${vars.topInstr} instr [7:0] $end`);
push(`  $var wire 8 ${vars.topPc} pc [7:0] $end`);
push(`  $var wire 8 ${vars.topData} data_out [7:0] $end`);
push(`  $var wire 16 ${vars.topResult} result [15:0] $end`);
push(`  $var wire 16 ${vars.topPc16} pc16 [15:0] $end`);
push('  $scope module u_cpu $end');
push(`    $var wire 1 ${vars.cpuClk} clk $end`);
push(`    $var wire 1 ${vars.cpuReset} reset $end`);
push(`    $var wire 8 ${vars.cpuInstr} instr [7:0] $end`);
push(`    $var wire 8 ${vars.cpuPc} pc [7:0] $end`);
push(`    $var wire 8 ${vars.cpuData} data_out [7:0] $end`);
push(`    $var wire 8 ${vars.cpuNext} next_pc [7:0] $end`);
push(`    $var wire 8 ${vars.cpuReg} pc_reg [7:0] $end`);
push(`    $var wire 1 ${vars.cpuActive} active $end`);
push('  $upscope $end');
push('  $scope module u_cpu16 $end');
push(`    $var wire 1 ${vars.cpu16Clk} clk $end`);
push(`    $var wire 1 ${vars.cpu16Reset} reset $end`);
push(`    $var wire 8 ${vars.cpu16Instr} instr [7:0] $end`);
push(`    $var wire 8 ${vars.cpu16Next} next_pc [7:0] $end`);
push(`    $var wire 8 ${vars.cpu16Reg} pc_reg [7:0] $end`);
push(`    $var wire 1 ${vars.cpu16Active} active $end`);
push('  $upscope $end');
push('$upscope $end');
push('$enddefinitions $end');

const bin = (value, width) => 'b' + value.toString(2).padStart(width, '0');

push('$dumpvars');
push(`0${vars.topClk}`);
push(`0${vars.topReset}`);
push(`b0 ${vars.topInstr}`);      // x/z mixed: all unknown
push(`bxxxxxxxx ${vars.topPc}`);
push(`bzzzzzzzz ${vars.topData}`);
push(`b0 ${vars.topResult}`);
push(`b0 ${vars.topPc16}`);
push(`0${vars.cpuClk}`);
push(`0${vars.cpuReset}`);
push(`b0 ${vars.cpuInstr}`);
push(`b0 ${vars.cpuPc}`);
push(`b0 ${vars.cpuData}`);
push(`b0 ${vars.cpuNext}`);
push(`b0 ${vars.cpuReg}`);
push(`1${vars.cpuActive}`);
push(`0${vars.cpu16Clk}`);
push(`0${vars.cpu16Reset}`);
push(`b0 ${vars.cpu16Instr}`);
push(`b0 ${vars.cpu16Next}`);
push(`b0 ${vars.cpu16Reg}`);
push(`1${vars.cpu16Active}`);
push('$end');

// 160 clock cycles -> ~320 time stamps, ~1000 changes
for (let t = 1; t <= 320; t++) {
  push('#' + (t * 5));
  const high = t % 2 === 0 ? 1 : 0;
  push(`${high}${vars.topClk}`);
  push(`${high}${vars.cpuClk}`);
  push(`${high}${vars.cpu16Clk}`);
  if (high) {
    const cycle = t / 2;
    push(bin(cycle & 0xff, 8) + ' ' + vars.topPc);
    push(bin(cycle & 0xff, 8) + ' ' + vars.cpuNext);
    push(bin(cycle & 0xff, 8) + ' ' + vars.cpuReg);
    push(bin(cycle & 0xff, 8) + ' ' + vars.cpu16Next);
    push(bin(cycle & 0xff, 8) + ' ' + vars.cpu16Reg);
    push(bin(cycle & 0xffff, 16) + ' ' + vars.topResult);
    push(bin(cycle & 0xffff, 16) + ' ' + vars.topPc16);
    push(bin((cycle * 3) & 0xff, 8) + ' ' + vars.topInstr);
    push(bin((cycle * 3) & 0xff, 8) + ' ' + vars.cpuInstr);
    push(bin((cycle * 3) & 0xff, 8) + ' ' + vars.cpu16Instr);
    push(bin((cycle * 7) & 0xff, 8) + ' ' + vars.topData);
    push(bin((cycle * 7) & 0xff, 8) + ' ' + vars.cpuData);
    // sprinkle x and z on a few cycles
    if (cycle % 17 === 0) {
      push('b0000xxxx ' + vars.cpuReg);
      push('bzzzzzzzz ' + vars.topData);
    }
    if (cycle % 23 === 0) {
      push('0' + vars.cpuActive);
      push('0' + vars.cpu16Active);
    } else {
      push('1' + vars.cpuActive);
      push('1' + vars.cpu16Active);
    }
  }
}

fs.writeFileSync(path.join(__dirname, 'wave.vcd'), lines.join('\n') + '\n', 'ascii');
console.log('wave.vcd: ' + lines.length + ' lines');
