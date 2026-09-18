const std = @import("std");

const Config = struct {
    name: []const u8,
    enabled: bool = true,
    retries: u8 = 3,
};

fn greeting(config: Config) []const u8 {
    if (!config.enabled) return "disabled";
    return config.name;
}

pub fn main() !void {
    const config = Config{ .name = "Zig" };
    std.debug.print("{s} ({d})\n", .{ greeting(config), config.retries });
}
