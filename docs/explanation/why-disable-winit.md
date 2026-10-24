# Why disable winit

Winit owns an OS event loop, which conflicts with test runners driving
ticks programmatically. Disabling only `WinitPlugin` keeps rendering,
assets, and audio alive — so systems touching those still run and can
still break loudly if buggy.
