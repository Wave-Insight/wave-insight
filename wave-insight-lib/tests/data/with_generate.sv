// A module with constructs outside the supported subset: every one of them
// must degrade to a diagnostic while the module itself is still usable.
module with_generate(input logic clk, output logic [7:0] q);
    typedef logic [7:0] byte_t;
    genvar i;

    generate
        for (i = 0; i < 2; i = i + 1) begin : g
            assign q = 8'b0;
        end
    endgenerate
endmodule
