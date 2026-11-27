const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const exe = b.addExecutable(.{
        .name = "study-platform",
        .root_source_file = b.path("main.zig"),
        .target = target,
        .optimize = optimize,
        .version = .{ .major = 1, .minor = 1, .patch = 2 },
    });
    exe.linkLibC();
    linkSqlite(exe);
    // La version vive en un solo sitio: build.zig. `--version` la lee de aqui
    // para que no vuelva a quedarse fija en el codigo.
    const options = b.addOptions();
    options.addOption([]const u8, "version", "1.1.2");
    exe.root_module.addOptions("build_options", options);
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
    unit_tests.root_module.addOptions("build_options", options);
    const run_tests = b.addRunArtifact(unit_tests);
    const test_step = b.step("test", "Ejecuta pruebas unitarias Zig");
    test_step.dependOn(&run_tests.step);

    // App de escritorio (GTK3 + WebKit2GTK, HTML5 exacto). No entra en el build
    // por defecto para que un servidor sin librerías gráficas siga compilando:
    //   zig build desktop
    const desktop_exe = b.addExecutable(.{
        .name = "nova-desktop",
        .root_source_file = b.path("desktop/main.zig"),
        .target = target,
        .optimize = optimize,
    });
    desktop_exe.linkLibC();
    desktop_exe.linkSystemLibrary("gtk+-3.0");
    desktop_exe.linkSystemLibrary("webkit2gtk-4.1");
    const desktop_install = b.addInstallArtifact(desktop_exe, .{});
    const desktop_step = b.step("desktop", "Compila la app de escritorio (GTK3 + WebKit2GTK, HTML5 exacto)");
    desktop_step.dependOn(&desktop_install.step);
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
