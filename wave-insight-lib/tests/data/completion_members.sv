// Hand written fixture for member/prefix completion (Wave 10 test, no external
// source): three nested modules plus probe comments that hold the dotted text a
// completion request is placed in.
module member_leaf(input logic clk, input logic [7:0] d, output logic [7:0] q);
    assign q = d;
endmodule

module member_mid(input logic [7:0] a, output logic [7:0] b);
    member_leaf u_leaf(.clk(1'b0), .d(a), .q(b));
    // completion probes: u_leaf. u_leaf.d U_LEAF nope.
endmodule

module member_top(input logic clk, output logic [7:0] out);
    member_mid u_mid(.a(8'h00), .b(out));
    assign out = u_mid.u_leaf.q;
    // completion probes: u_mid. u_mid.u_leaf. u_mid.u_l u_mid.U_L member_leaf.
endmodule
