// Hand written fixture for driver/load bit ranges (Wave 8 test; no external
// source): two disjoint slices drive one signal, one slice/single bit is read,
// and an instance port is connected through a slice.
module slice_leaf(input logic [3:0] d, output logic [3:0] q);
    assign q = d;
endmodule

module bit_ranges(
    input  logic [7:0] a,
    input  logic [7:0] b,
    output logic [7:0] q,
    output logic       y
);
    assign q[3:0] = a[7:4];
    assign q[7:4] = b[3:0];
    assign y = a[0];
endmodule

module bit_ranges_top(input logic [7:0] bus, output logic [7:0] out);
    slice_leaf u(.d(bus[3:0]), .q(out[3:0]));
endmodule
