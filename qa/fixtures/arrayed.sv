// Hand written fixture for arrayed-instance expansion (Wave 3 test; no external
// source).
module arr_leaf(input logic [3:0] d, output logic [3:0] q);
    assign q = d;
endmodule

module arrayed_top(
    input  logic [7:0] a,
    input  logic [7:0] b,
    output logic [7:0] q
);
    // Two element instance: paths arrayed_top.u[1] and arrayed_top.u[0].
    arr_leaf u[1:0] (.d(a[7:4]), .q(q[7:4]));
    // Four element instance with an unconnected output port.
    arr_leaf v[3:0] (.d(b[3:0]), .q());
endmodule
