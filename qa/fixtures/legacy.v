// Classic non-ANSI module: `input a, b;` is the case the old character
// scanner mis-parsed.  No waveform file is involved anywhere in the design
// tests, which is the case that used to panic.
module legacy(a, b, y);
    input a, b;
    output [7:0] y;
    wire [7:0] wide;

    assign wide = {7'b0, a & b};
    assign y = wide;
endmodule
