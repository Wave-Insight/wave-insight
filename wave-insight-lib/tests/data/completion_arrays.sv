// Hand written fixture for arrayed-instance completion (Wave 11 test, no external
// source): arrayed and plain instances plus probe comments holding the dotted
// text completion requests are placed in.
module array_leaf(input logic clk, input logic [3:0] d, output logic [3:0] q);
    assign q = d;
endmodule

module array_top(input logic clk, output logic [3:0] out);
    array_leaf u[1:0] (.clk(clk), .d(4'h0), .q());
    array_leaf single(.clk(clk), .d(4'h0), .q());
    assign out = u[1].q;
    // completion probes: u[0]. u[1]. u. q[3:0]. single. u[i].
endmodule
