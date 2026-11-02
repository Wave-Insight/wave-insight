# fst-gen - reproducible synthetic FST fixture

The QA suite cannot read a real >100 MB FST from the repository (the files are
far too big to commit), so this directory keeps the generator recipe instead.
The generated files are deliberately **not** committed: they live in
`%TEMP%\wi-fst` (or any gitignored path) and are handed to the tests with the
`QA_LARGE_FST=<path>` environment variable that `denseDump` reads.

## Sources

* https://github.com/gtkwave/libfst - GTKWave's FST read/write library (MIT).
  The recipe was validated with commit
  `17f6a5d27b2f576d65b550114fbde3c035cc74a8` (master, 2026-10-17).
  Relevant files: `src/fstapi.c`, `src/fstapi.h`, `src/fastlz.c`, `src/fastlz.h`,
  `src/lz4.c`, `src/lz4.h`.
* `gen.c` - the deterministic generator in this directory (fixed LCG seed, fixed
  `$date`, so regenerating is byte-identical).

## Build the generator (MSYS2 mingw64, zlib from the same tree)

```powershell
cd $env:TEMP\wi-fst
git clone --depth 1 https://github.com/gtkwave/libfst.git libfst

gcc -O2 -D__USE_MINGW_ANSI_STDIO=1 -o gen.exe <repo>\qa\fixtures\generators\fst-gen\gen.c `
    libfst\src\fstapi.c libfst\src\fastlz.c libfst\src\lz4.c `
    -Ilibfst\src -lz
```

Notes: `-lz` is required (zlib from MSYS2 mingw64). Current libfst has no
`NO_ZLIB`/`NO_LZ4` flags - do not pass them. `-D__USE_MINGW_ANSI_STDIO=1` only
fixes `%llu` in the progress output of this old mingw printf, the FST file is
unaffected.

## Generate

```powershell
cd $env:TEMP\wi-fst
.\gen.exe large.fst 1000000 2000 512 512 45 6   # ~6.5 s -> 102,273,753 bytes
.\gen.exe dense.fst   30000 2000 512 512 45 6   # ~0.3 s ->   3,203,089 bytes
```

`gen.exe <out.fst> <nsteps> <nbits> <nbus32> <nbus64> <active_bits_per_step> <buses_per_step>`

| arg | meaning |
|---|---|
| `nsteps` | time steps, 10 ns apart (1,000,000 steps = 10 ms) |
| `nbits` | 1-bit signals, spread over 8 sub-scopes `top.u0..u7` |
| `nbus32` / `nbus64` | 32/64-bit buses in `top.buses32` / `top.buses64` |
| `active_bits_per_step` | how many 1-bit signals toggle per step (rolling window) |
| `buses_per_step` | how many 32-bit *and* how many 64-bit buses change per step |

`top.clk` toggles at **every** time step - that is the dense signal whose
full-history decode the `denseDump` scenario guards against.

## Validate and measure

Open the file through `wave-insight-lib` (wellen 0.25.7) - a small cargo
program that prints signals, time range and (optionally) the total change count:

```powershell
cargo +1.98.1 run --offline -- "$env:TEMP\wi-fst\dense.fst"
cargo +1.98.1 run --offline -- "$env:TEMP\wi-fst\large.fst" --total
```

## Known sizes

| file | bytes | MiB | steps | signals | time range | `top.clk` changes | total changes |
|---|---|---|---|---|---|---|---|
| large.fst | 102,273,753 | 97.5 | 1,000,000 | 3,025 | 0 - 9,999,990 | 1,000,000 | 58,002,967 |
| dense.fst | 3,203,089 | 3.05 | 30,000 | 3,025 | 0 - 299,990 | 30,000 | 1,742,967 |

Signal breakdown: 1 clock + 2,000x 1-bit (in 8 scopes) + 512x 32-bit + 512x
64-bit = 3,025 variables. Opening `large.fst` costs 9.4 ms / 8.5 MB in the
library (hierarchy + time table); a full streaming pass over all 58 M changes
takes 14 s with flat memory - so any multi-GB blow-up comes from materialising
histories (the full-decode renderer this scenario guards).

## Tuning

Size is linear in `nsteps`; measured bytes/step: 72 for 40 bit changes + 4+4 bus
changes, 236 for 40 bit + 16+16 bus changes, 115 for 0 bit + 8+8 bus changes.
1-bit changes are cheap (~0.35 B after the LZ4 repack) and buses dominate
(~4.5 B per 32-bit, ~9 B per 64-bit change).
