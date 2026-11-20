// Small package-root shim: Zig restricts @embedFile to the package tree.
// Keeping the actual backend in src/ while making this module the root allows
// static/ and migrations/ to be embedded without copying or rewriting them.
pub fn main() !void {
    try @import("src/main.zig").main();
}
