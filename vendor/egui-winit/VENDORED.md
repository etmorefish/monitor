# Vendored: egui-winit 0.37.0 (patched)

Upstream: <https://github.com/emilk/egui/tree/main/crates/egui-winit>, version
0.37.0, MIT OR Apache-2.0. Copied verbatim from the crates.io registry, then
minimal patches were applied to stop mouse motion from driving full repaints
(see `docs/高帧率重绘问题.md` for the full analysis):

1. `WindowEvent::CursorMoved` now returns `EventResponse { repaint: false }`
   instead of `repaint: true`, and schedules the next pass with
   `Context::request_repaint_after(MOVE_REPAINT_DELAY_MS)`. On Windows,
   upstream's `repaint: true` makes eframe run a synchronous full
   paint *per mouse event* (`RepaintNow` path).

2. `WindowEvent::CursorLeft` and `WindowEvent::CursorEntered` get the same
   treatment. They only fire once per leave/enter for a real mouse, but
   synthetic input to an unfocused window makes winit ping-pong
   enter/leave per move, which upstream would again turn into a 60 fps chain.

3. `State::on_mouse_motion` now always returns `false`, so eframe no longer
   schedules a repaint per raw `DeviceEvent::MouseMotion`.

The crate is wired in via `[patch.crates-io]` in `Cargo.toml`; the package
version stays `0.37.0` so it satisfies eframe's requirement. Search the file
for `monitor patch` to find the edit sites.

Keep in sync: if egui-winit is upgraded, re-copy the crate and re-apply the
patches, and keep `MOVE_REPAINT_DELAY_MS` aligned with the throttle in
`src/gui/throttle.rs`.
