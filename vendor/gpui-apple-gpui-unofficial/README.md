> **Note:** This is an unofficial release of Zed's [gpui_apple](https://github.com/zed-industries/zed/tree/main/crates/gpui_apple) crate, published to crates.io by [gpui-unofficial](https://github.com/iamnbutler/gpui-unofficial). It is not maintained by the Zed team. For issues with the crate itself, see the [Zed repository](https://github.com/zed-industries/zed).


## Workspace integration

This is the crates.io `gpui-apple-gpui-unofficial` 1.17.0-pre source, with its original Apache license and provenance retained. The workspace pairs it with the vendored GPUI core. An opt-in benchmark observer reports Metal command-buffer completion; a reused native offscreen target isolates renderer work from drawable-supply pacing. Ordinary rendering installs no observer.

The host may prepare first-window device and pipeline resources while AppKit initializes. The window adopts those resources on its own thread; layer and drawable ownership stay on the window thread. Production builds compile the Metal library ahead of launch, using the toolchain described in the workspace Rust build guide.

The renderer implements the shared retained-content contract with bounded color textures, separately updated path masks and premultiplied composition. Unsupported or over-budget frames expand the original scene operations before drawing. Capability reporting travels with the renderer's atlas; content identity, viewport changes and device ownership govern reuse. Tests read back Metal pixels for nested alpha, changing masks and resource invalidation.
