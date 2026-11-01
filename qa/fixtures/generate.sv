// Hand written fixture for generate elaboration (Wave 3 test; no external
// source). Covers a generate-for over an array of submodules, a generate-if
// with both branches, a generate construct outside a generate region and a
// generate-case with a default.
module gen_leaf(input logic a, output logic y);
    assign y = a;
endmodule

module gen_top #(
    parameter int N = 4,
    parameter bit WIDE = 1
) (
    input  logic [N-1:0] a,
    output logic [N-1:0] y
);
    genvar i;
    generate
        for (i = 0; i < N; i = i + 1) begin : g
            gen_leaf u_leaf (.a(a[i]), .y(y[i]));
        end
    endgenerate

    generate
        if (WIDE) begin : wide
            logic done;
            assign done = a[0];
        end else begin : narrow
            logic done;
            assign done = y[0];
        end
    endgenerate

    if (N > 2) begin : big
        logic busy;
        assign busy = a[1];
    end

    generate
        case (N)
            2: begin : two
                logic flag;
                assign flag = a[0];
            end
            default: begin : many
                logic flag;
                assign flag = a[0];
            end
        endcase
    endgenerate
endmodule
