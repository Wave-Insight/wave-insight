// Top level: one instance bound by name, one bound by position, and a
// parameter override on each.
module top (
    input  logic        clk,
    input  logic        reset,
    input  logic [7:0]  instr,
    output logic [15:0] result
);
    logic [7:0]  pc;
    logic [7:0]  data_out;
    logic [15:0] pc16;

    cpu #(.WIDTH(8), .DEPTH(16)) u_cpu (
        .clk(clk),
        .reset(reset),
        .instr(instr),
        .pc(pc),
        .data_out(data_out)
    );

    cpu #(16, 32) u_cpu16 (
        clk,
        reset,
        {8'b0, instr},
        pc16,
        result
    );

    assign result[7:0] = data_out;
endmodule
