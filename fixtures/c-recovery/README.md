Small, altered parser fixtures extracted from:

- Lua `lapi.c:109–123`, commit `0b29f408433e92953cc72b1d3e06c7ac8139e439` (MIT; see `lua.LICENSE`). Added an include.
- cJSON `cJSON.c:123–130`, commit `6d9f2443ab071f86e5d9b43025a40929ec41c46c` (MIT; see `cJSON.LICENSE`). Added `<stdio.h>`.
- tinyxml2 `tinyxml2.h:130–191`, commit `8224e427b655b83dae5e2298f1e6919523a78737` (zlib; see `tinyxml2.LICENSE`). Converted CRLF to LF and renamed the standalone excerpt to `.hpp` to select C++ without a companion translation unit.

These are excerpts for analysis, not compilable replacements for upstream files.
