// Small CPU-ish leaf module used by the hierarchy / parameter tests.
module cpu #(
    parameter WIDTH = 8,
    parameter DEPTH = 16
) (
    input  logic             clk,
    input  logic             reset,
    input  logic [WIDTH-1:0] instr,
    output logic [WIDTH-1:0] pc,
    output logic [WIDTH-1:0] data_out
);
    logic [WIDTH-1:0] next_pc;
    reg   [WIDTH-1:0] pc_reg;
    wire              active;

    localparam STEP = 1;

    assign active  = reset ? 1'b0 : 1'b1;
    assign next_pc = pc_reg + 1'b1;
    assign pc      = pc_reg;
    assign data_out = instr;

    always @(posedge clk) begin
        if (reset)
            pc_reg <= {WIDTH{1'b0}};
        else
            pc_reg <= next_pc;
    end
endmodule
