/*
 * gen.c -- generate a synthetic FST waveform using gtkwave/libfst's fstapi.
 *
 * usage: gen.exe <out.fst> <nsteps> <nbits> <nbus32> <nbus64> <active_per_step> <bus_period>
 *
 *   out.fst         output file path
 *   nsteps          number of time steps
 *   nbits           number of 1-bit signals (scattered in 8 sub-scopes)
 *   nbus32          number of 32-bit buses
 *   nbus64          number of 64-bit buses
 *   active_per_step number of 1-bit signals that toggle each time step
 *   bus_per_step    number of 32-bit buses and 64-bit buses that change each step
 *                   (randomly chosen, random values)
 *
 * The clock signal toggles at every time step (the dense signal).
 */

#define __USE_MINGW_ANSI_STDIO 1 /* mingw: make %llu work with the msvcrt-agnostic printf */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <time.h>

#include "fstapi.h"

static uint64_t lcg_state = 0x123456789abcdefULL;

static uint32_t lcg32(void)
{
    lcg_state = lcg_state * 6364136223846793005ULL + 1442695040888963407ULL;
    return (uint32_t)(lcg_state >> 33);
}

static void die(const char *msg)
{
    fprintf(stderr, "gen: %s\n", msg);
    exit(1);
}

int main(int argc, char **argv)
{
    const char *out;
    uint64_t nsteps;
    int nbits, nbus32, nbus64, active, bus_per_step;

    fstWriterContext *ctx;
    fstHandle clk;
    fstHandle *bits = NULL, *b32 = NULL, *b64 = NULL;
    char name[128];
    char valbuf[2];
    uint64_t changes = 0;
    clock_t t_start;
    int i, k, g;
    const int ngroups = 8;

    if (argc != 8) {
        fprintf(stderr,
                "usage: %s <out.fst> <nsteps> <nbits> <nbus32> <nbus64> <active_per_step> <bus_per_step>\n",
                argv[0]);
        return 2;
    }

    out = argv[1];
    nsteps = strtoull(argv[2], NULL, 10);
    nbits = atoi(argv[3]);
    nbus32 = atoi(argv[4]);
    nbus64 = atoi(argv[5]);
    active = atoi(argv[6]);
    bus_per_step = atoi(argv[7]);

    if (nsteps < 1 || nbits < 1 || active < 0 || bus_per_step < 0) die("bad arguments");
    if (active > nbits) active = nbits;

    bits = (fstHandle *)calloc((size_t)nbits, sizeof(fstHandle));
    b32 = (fstHandle *)calloc((size_t)(nbus32 ? nbus32 : 1), sizeof(fstHandle));
    b64 = (fstHandle *)calloc((size_t)(nbus64 ? nbus64 : 1), sizeof(fstHandle));
    if (!bits || !b32 || !b64) die("out of memory");

    t_start = clock();

    ctx = fstWriterCreate(out, 1 /* use_compressed_hier */);
    if (!ctx) die("fstWriterCreate failed");

    fstWriterSetVersion(ctx, "wi-fst-gen 1.0");
    fstWriterSetDate(ctx, "Mon Nov  2 19:30:00 2026");
    fstWriterSetTimescale(ctx, -9); /* 1 ns */
    fstWriterSetComment(ctx, "synthetic dense waveform generated for wave-insight FST memory repro");

    fstWriterSetScope(ctx, FST_ST_VCD_MODULE, "top", NULL);

    clk = fstWriterCreateVar(ctx, FST_VT_VCD_WIRE, FST_VD_IMPLICIT, 1, "clk", 0);
    if (!clk) die("clk var failed");

    /* 1-bit signals, spread across ngroups sub-scopes */
    for (g = 0; g < ngroups; g++) {
        snprintf(name, sizeof(name), "u%d", g);
        fstWriterSetScope(ctx, FST_ST_VCD_MODULE, name, NULL);
        for (i = g; i < nbits; i += ngroups) {
            snprintf(name, sizeof(name), "sig_%05d", i);
            bits[i] = fstWriterCreateVar(ctx, FST_VT_VCD_WIRE, FST_VD_IMPLICIT, 1, name, 0);
            if (!bits[i]) die("bit var failed");
        }
        fstWriterSetUpscope(ctx);
    }

    /* 32-bit buses */
    if (nbus32 > 0) {
        fstWriterSetScope(ctx, FST_ST_VCD_MODULE, "buses32", NULL);
        for (i = 0; i < nbus32; i++) {
            snprintf(name, sizeof(name), "bus32_%04d", i);
            b32[i] = fstWriterCreateVar(ctx, FST_VT_VCD_WIRE, FST_VD_IMPLICIT, 32, name, 0);
            if (!b32[i]) die("bus32 var failed");
        }
        fstWriterSetUpscope(ctx);
    }

    /* 64-bit buses */
    if (nbus64 > 0) {
        fstWriterSetScope(ctx, FST_ST_VCD_MODULE, "buses64", NULL);
        for (i = 0; i < nbus64; i++) {
            snprintf(name, sizeof(name), "bus64_%04d", i);
            b64[i] = fstWriterCreateVar(ctx, FST_VT_VCD_WIRE, FST_VD_IMPLICIT, 64, name, 0);
            if (!b64[i]) die("bus64 var failed");
        }
        fstWriterSetUpscope(ctx);
    }

    fstWriterSetUpscope(ctx); /* top */

    /* ---- time step 0: define every signal so the whole design is alive ---- */
    fstWriterEmitTimeChange(ctx, 0);
    valbuf[0] = '0';
    valbuf[1] = 0;
    fstWriterEmitValueChange(ctx, clk, valbuf);
    changes++;
    for (i = 0; i < nbits; i++) {
        valbuf[0] = (lcg32() & 1) ? '1' : '0';
        fstWriterEmitValueChange(ctx, bits[i], valbuf);
        changes++;
    }
    for (i = 0; i < nbus32; i++) {
        fstWriterEmitValueChange32(ctx, b32[i], 32, lcg32());
        changes++;
    }
    for (i = 0; i < nbus64; i++) {
        uint64_t v = ((uint64_t)lcg32() << 32) | lcg32();
        fstWriterEmitValueChange64(ctx, b64[i], 64, v);
        changes++;
    }

    /* ---- remaining time steps ---- */
    for (uint64_t t = 1; t < nsteps; t++) {
        fstWriterEmitTimeChange(ctx, t * 10ULL);

        valbuf[0] = (t & 1) ? '1' : '0';
        fstWriterEmitValueChange(ctx, clk, valbuf);
        changes++;

        /* rolling window of 1-bit signals, each toggling every nbits/active steps */
        if (active > 0) {
            uint64_t base = (uint64_t)t * (uint64_t)active;
            for (k = 0; k < active; k++) {
                uint64_t idx = (base + (uint64_t)k) % (uint64_t)nbits;
                valbuf[0] = (lcg32() & 1) ? '1' : '0';
                fstWriterEmitValueChange(ctx, bits[idx], valbuf);
                changes++;
            }
        }

        if (bus_per_step > 0) {
            int nb32 = (bus_per_step < nbus32) ? bus_per_step : nbus32;
            int nb64 = (bus_per_step < nbus64) ? bus_per_step : nbus64;
            for (k = 0; k < nb32; k++) {
                uint64_t j = (uint64_t)(lcg32() % (uint32_t)nbus32);
                fstWriterEmitValueChange32(ctx, b32[j], 32, lcg32());
                changes++;
            }
            for (k = 0; k < nb64; k++) {
                uint64_t j = (uint64_t)(lcg32() % (uint32_t)nbus64);
                uint64_t v = ((uint64_t)lcg32() << 32) | lcg32();
                fstWriterEmitValueChange64(ctx, b64[j], 64, v);
                changes++;
            }
        }

        if ((t % 100000ULL) == 0) {
            fprintf(stderr, "gen: %llu / %llu steps (%llu changes)\n",
                    (unsigned long long)t, (unsigned long long)nsteps,
                    (unsigned long long)changes);
        }
    }

    fstWriterClose(ctx);
    free(bits);
    free(b32);
    free(b64);

    fprintf(stderr, "gen: done: steps=%llu changes=%llu elapsed=%.1fs\n",
            (unsigned long long)nsteps, (unsigned long long)changes,
            (double)(clock() - t_start) / (double)CLOCKS_PER_SEC);
    return 0;
}
