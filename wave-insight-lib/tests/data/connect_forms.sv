// Implicit (`.a`) and wildcard (`.*`) port connections.
module leaf(input logic a, input logic b, output logic y);
    assign y = a & b;
endmodule

module implicit_conn(input logic a, input logic b, output logic y);
    leaf u_leaf (.a, .b, .y);
endmodule

module wildcard_conn(input logic a, input logic b, output logic y);
    leaf u_leaf (.*);
endmodule
