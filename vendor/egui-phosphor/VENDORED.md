# egui-phosphor 0.14.0 (vendored)

Source: https://github.com/amPerl/egui-phosphor (crates.io `egui-phosphor` 0.14.0), MIT OR Apache-2.0.

The crate has no release for egui 0.37, so the workspace patches `egui-phosphor` to this copy, which
depends on egui 0.37. Only the dependency version in `Cargo.toml` differs; examples, tests and the
unused font variants (thin, light, bold) are left out. The `regular` and `fill` fonts are the Phosphor
Icons font (MIT, https://phosphoricons.com). Drop this directory and the patch once the crate releases
for egui 0.37.
