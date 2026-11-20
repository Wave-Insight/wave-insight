# Companion RTL fixture

This directory is the SpinalHDL layout the companion scan looks for, and the
only fixture that has one:

```
simWorkspace/run/wave.vcd      the dump (a copy of ../wave.vcd)
simWorkspace/run/rtl/top.v     generated RTL (a copy of ../top.sv)
simWorkspace/run/rtl/cpu.sv
```

`qa/dsh-plugin-smoke.mjs` loads the dump and asserts that the two sources come
back, which is the rule the desktop open path has applied since Wave 19.
