# Run playtests on a GPU-less CI host

Headless takeover drops the windowing, not the renderer — and the
renderer still wants a GPU adapter. This page explains why a
camera-bearing game panics on a GPU-less CI runner, and how to fix the
runner (not your game).

## Why a GPU is still required headless

Under a playtest scenario, [`PlaytestDefaultPlugins`] drops
`WinitPlugin` and swaps in `ScheduleRunnerPlugin::run_loop` so the game
ticks without a window or display. But the plugin group keeps
`RenderPlugin` — and `RenderPlugin` initializes a real `wgpu` device:
[`initialize_renderer`] asks the Vulkan/OpenGL stack for an adapter and
expects one:

```rust,ignore
let adapter = selected_adapter.expect(GPU_NOT_FOUND_ERROR_MESSAGE);
```

Stock CI runners (`ubuntu-latest` etc.) ship no GPU and often no GPU
*drivers*. A camera-bearing game (anything that inserts a camera and so
initializes the render pipeline) then dies at startup with:

```text
thread 'main' panicked at …/bevy_render-0.20.1/src/renderer/mod.rs:
Unable to find a GPU! Make sure you have installed required drivers!
For extra information, see: https://github.com/bevyengine/bevy/blob/latest/docs/linux_dependencies.md
```

## The fix: install Mesa's software Vulkan driver

`mesa-vulkan-drivers` provides `lavapipe`, a CPU-based Vulkan adapter
that satisfies the adapter request. It is what bevy_playtest's own
fixture battery installs in `.github/workflows/ci.yml`:

```yaml
- name: Install GPU-less rendering prerequisites
  run: |
    sudo apt-get update
    sudo apt-get install -y --no-install-recommends \
      libasound2-dev libudev-dev libxkbcommon-dev libwayland-dev \
      mesa-vulkan-drivers
```

(`libasound2-dev`, `libudev-dev`, `libxkbcommon-dev` and
`libwayland-dev` are bevy's other Linux link dependencies; see bevy's
[linux_dependencies.md]. If you already install bevy on CI, you have
most of these.)

With `mesa-vulkan-drivers` present, the takeover run starts, the run
completes, and bevy may log a benign warning that the selected adapter
is software-rendering (`DeviceType::Cpu`) — headless playtests don't
draw frames, so the slowness warning doesn't matter.

`lavapipe` is a CPU-based *rasterizer*, not an emulator: no KVM, no
nested virtualization, no privileged runner needed. If your runner can
compile bevy, it can run `lavapipe`.

## macOS note

macOS runners (and developer machines) have Metal and are unaffected —
no extra packages are needed. The panic is a Linux/GPU-less-host issue;
the `mesa-vulkan-drivers` stanza above is Linux-only.

[`PlaytestDefaultPlugins`]: ../reference/README.md#plugins
[`initialize_renderer`]: https://github.com/bevyengine/bevy/blob/v0.20.1/crates/bevy_render/src/renderer/mod.rs
[linux_dependencies.md]: https://github.com/bevyengine/bevy/blob/latest/docs/linux_dependencies.md
