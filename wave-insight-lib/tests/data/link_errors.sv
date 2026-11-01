// Hand written fixture for unresolved names, wildcard connections and a port
// width mismatch (Wave 3 test; no external source).
module narrow_leaf(input logic [3:0] d, output logic [3:0] q);
    assign q = d;
endmodule

module link_leaf(input logic a, output logic y);
    assign y = a & missing_load;
endmodule

module link_top(input logic clk, output logic y);
    link_leaf u_link(.a(clk), .y(y));
    assign y = missing_driver;
endmodule

module wild_leaf(input logic clk, input logic rst, output logic y);
    assign y = clk & rst;
endmodule

module wild_top(input logic clk, output logic y);
    // rst has no same-named signal in wild_top: this used to be dropped.
    wild_leaf u_wild(.*);
endmodule

module width_top(input logic [7:0] data);
    // 8 bit signal into a 4 bit port: a width mismatch warning.
    narrow_leaf u_short(.d(data), .q());
endmodule
