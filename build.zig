const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const exe = b.addExecutable(.{
        .name = "study-platform",
        .root_source_file = b.path("main.zig"),
        .target = target,
        .optimize = optimize,
        .version = .{ .major = 0, .minor = 2, .patch = 0 },
    });
    exe.linkLibC();
    linkSqlite(exe);
    b.installArtifact(exe);

    const run_cmd = b.addRunArtifact(exe);
    if (b.args) |args| run_cmd.addArgs(args);
    const run_step = b.step("run", "Ejecuta Study Platform");
    run_step.dependOn(&run_cmd.step);

    const unit_tests = b.addTest(.{
        .root_source_file = b.path("test_root.zig"),
        .target = target,
        .optimize = optimize,
    });
    unit_tests.linkLibC();
    linkSqlite(unit_tests);
    const run_tests = b.addRunArtifact(unit_tests);
    const test_step = b.step("test", "Ejecuta pruebas unitarias Zig");
    test_step.dependOn(&run_tests.step);
}

fn linkSqlite(artifact: *std.Build.Step.Compile) void {
    // En servidores suele estar disponible el nombre de desarrollo sqlite3;
    // Arena puede tener solo el SONAME versionado. Se enlaza el artefacto
    // dinámico existente, sin copiar ni compilar código C de terceros.
    const candidates = [_][]const u8{
        "/usr/lib/x86_64-linux-gnu/libsqlite3.so",
        "/lib/x86_64-linux-gnu/libsqlite3.so",
        "/usr/lib/x86_64-linux-gnu/libsqlite3.so.0",
        "/lib/x86_64-linux-gnu/libsqlite3.so.0",
        "/usr/lib/aarch64-linux-gnu/libsqlite3.so.0",
        "/lib/aarch64-linux-gnu/libsqlite3.so.0",
    };
    for (candidates) |path| {
        if (std.fs.accessAbsolute(path, .{})) |_| {
            artifact.addObjectFile(.{ .cwd_relative = path });
            return;
        } else |_| {}
    }
    artifact.linkSystemLibrary("sqlite3");
}
