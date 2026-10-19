// Deliberately malformed: the port list is never closed and the module ends
// in the middle of a statement.  Loading this must produce diagnostics, not a
// panic.
module broken(input logic a;
    assign a = 1'b0;
endmodule
