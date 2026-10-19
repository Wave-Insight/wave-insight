// Non-ANSI ports with explicit net/variable types and several names per
// declaration.
module legacy_ports(a, b, y, z);
    input wire [3:0] a, b;
    output reg [3:0] y;
    output z;
    wire [7:0] bus;
    reg  [3:0] r;

    assign y = a + b;
    assign bus = {4'b0, r};

    always @(*) begin
        r = a;
        z = r[0];
    end
endmodule
