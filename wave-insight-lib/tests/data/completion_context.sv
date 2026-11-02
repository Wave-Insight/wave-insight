// Hand written fixture for completion context classification (Wave 13 follow-up,
// no external source): non-arrayed and arrayed instances whose port connections
// contain bit-select expressions, so every caret column on the line has to fall
// back to the module scope instead of an empty completion list.
module context_leaf(input logic [7:0] d, output logic [7:0] q);
    assign q = d;
endmodule

module context_top(
    input  logic [7:0] bus,
    input  logic [7:0] data,
    output logic [7:0] out
);
    context_leaf plain(.d(bus[7:4]), .q(out));
    context_leaf arr[1:0] (.d(data[3:0]), .q());
    context_leaf single_bit(.d(bus[0]), .q());
endmodule
