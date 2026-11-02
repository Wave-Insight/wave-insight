// Hand written fixture for generate-scope completion (Wave 11 test, no external
// source): one generated scope per loop iteration with an instance and a signal,
// plus probe comments holding the dotted text completion requests are placed in.
module scope_leaf(input logic a, output logic y);
    assign y = a;
endmodule

module scope_top #(
    parameter int N = 2
) (
    input  logic [N-1:0] d,
    output logic [N-1:0] q
);
    genvar i;
    generate
        for (i = 0; i < N; i = i + 1) begin : g
            scope_leaf u_leaf(.a(d[i]), .y(q[i]));
            logic marker;
            assign marker = d[i];
            // completion probes: g[0]. g[i]. g[0].u_leaf. g[i].u_leaf. g[9].
        end
    endgenerate
endmodule
