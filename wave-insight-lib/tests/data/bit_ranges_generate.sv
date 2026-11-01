// Hand written fixture for a generate-loop select (Wave 8 test; no external
// source): the index is not constant, so the range stays textual.
module bit_ranges_generate #(
    parameter int N = 4
) (
    input  logic [N-1:0] d,
    output logic [N-1:0] q
);
    genvar i;
    generate
        for (i = 0; i < N; i = i + 1) begin : g
            assign q[i] = d[i];
        end
    endgenerate
endmodule
