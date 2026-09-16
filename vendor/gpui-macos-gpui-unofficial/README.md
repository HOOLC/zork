> **Note:** This is an unofficial release of Zed's [gpui_macos](https://github.com/zed-industries/zed/tree/main/crates/gpui_macos) crate, published to crates.io by [gpui-unofficial](https://github.com/iamnbutler/gpui-unofficial). It is not maintained by the Zed team. For issues with the crate itself, see the [Zed repository](https://github.com/zed-industries/zed).



## Workspace integration

This is the crates.io `gpui-macos-gpui-unofficial` 1.17.0-pre source, with its original Apache license and crate provenance retained. It uses the same vendored GPUI core and Metal renderer as the other workspace platforms. Optional startup observations separate native application, window and renderer initialization from client business startup.

Native windows convert the requested top-left position to AppKit's bottom-left content origin and finish frame/chrome placement before showing. This avoids moving an already visible first frame into its intended position.

Keyboard layout and shortcut mapping share one lazily initialized main-thread snapshot. The system layout-change notification invalidates them together before notifying the application.
