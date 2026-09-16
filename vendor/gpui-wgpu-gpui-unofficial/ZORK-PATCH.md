# Local renderer patch

Upstream: `gpui-wgpu-gpui-unofficial` 1.17.0-pre from crates.io. Original manifest, provenance and Apache license are retained. The standalone test lock pins the same GPUI 1.17 / WGPU and Naga 29.0.4 versions as the application.

## Resolved MSAA attachments

The path rasterization pass resolves its MSAA attachment into `path_intermediate`. Later passes sample only the resolved texture, and every subsequent MSAA use clears it. Discard the MSAA attachment after resolve instead of storing unused full-viewport samples. Single-sample attachments still use Store.

## WebGPU path batches

For the WebGPU Web backend, rasterize a frame's path batches into separate padded atlas cells in one antialiased pass, then composite their sprites in the original scene order within one main pass. Integer translations preserve sample coverage and gradient coordinates. Each original batch keeps its blending semantics; two clear pixels around cells prevent filtering bleed.

Packing tries a bounded set of row widths so tall Retina batches do not waste power-of-two columns and unnecessarily switch to the fallback. The atlas is bounded to 16 Mi texels, the device's texture dimensions and 256 batches. A failed plan uses the original per-batch path. Existing texture storage is preferred when the new layout fits, avoiding per-frame reallocations as geometry changes. Composite sprites are clipped to their own cell bounds so hidden paths cannot sample a neighboring cell. Native and WebGL keep the per-batch path.

When an atlas outgrows its attachments, reserve geometric capacity (1.5× the prior dimension, rounded to 256 pixels) within the same dimension and texel bounds. Single-axis or exact-size allocation is used when combined slack would exceed the budget. This absorbs the initial send animation's observed sequence of seven small height increases in one allocation. The shaders use actual attachment dimensions, so spare capacity does not change sample coordinates.

Geometry, sample count, resolved colors, content masks, scene order and text rendering are preserved. Shared path-sprite CPU/shader layouts include an offset for atlas sampling; the fallback uses zero offset.

## Verification

The development `msaa=compare` probe runs Store / Discard / Store on one page and device; it is diagnostic tooling, not the delivered renderer implementation. `client_probe.js` replays fixed gallery inputs and reports render-pass counts and GPU timestamps. Unit tests cover shader validity, CPU/shader layouts, overlapping batches, fractional screen origins, culling, Retina packing and bounded fallback. Full gallery input/visual checks cover WebGPU and WebGL.
