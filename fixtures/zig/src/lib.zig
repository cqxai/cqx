const std=@import("std");
pub fn stop(input:[]const u8) void {std.process.exit(1);var p=std.process.Child.init(&.{input},allocator);}
