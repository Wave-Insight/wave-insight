// Packed vectors, multi dimensional packed ranges and a few scalar types.
module vectors #(
    parameter W = 8,
    parameter D = 4
) (
    input  logic             [W-1:0]       data,
    input  logic [D*W-1:0]                 packed4,
    output logic             [W-1:0]       out,
    output logic [3:0][7:0]                matrix
);
    logic [W-1:0] tmp;
    logic [1:0]   pair;
    int           count;
    logic         flag;

    assign tmp    = data;
    assign out    = tmp;
    assign pair   = tmp[1:0];
    assign matrix = {4{data}};
    assign flag   = ^data;
endmodule
