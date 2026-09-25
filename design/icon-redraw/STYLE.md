# Zork soft-line icons

All masters use `viewBox="0 0 24 24"`, root `fill="none"`,
`stroke="currentColor"`, `stroke-width="1.75"`, and round caps and joins.
At 14, 16 and 20 px this gives strokes of 1.02, 1.17 and 1.46 px.
Keep native antialiasing; do not snap or change weight per size.

Functional silhouettes occupy approximately 3–21 on each axis. Circular
outlines use radius 9; rectangular bodies normally span 3.5–20.5 with
2.5–3 unit corners. Rounded bodies use cubic shoulders with tangent joins.
Narrow symbols are centred optically rather than stretched to fill the box.
Small interior corners use 1–1.5 units; direction tips retain their semantic
angle with rounded joins. File folds are structural, not extra decoration.
Keep at least 2 units of clear interior space where possible; omit incidental
detail before reducing stroke weight.

All paint is `currentColor`. Solid shapes are reserved for stop, microphone
capsules, small information dots and inherently filled logos. Filled paths
explicitly set `stroke="none"`. No opacity layering or multicolour shapes.
Files contain only the SVG root and paths: no groups, transforms, IDs,
metadata, masks or editor attributes. Duplicate meanings use identical bytes;
filled microphones are the deliberate fill variant.

Provider and maker badges preserve the companies' recognisable silhouettes
and negative spaces. Fit their longest painted dimension to 18 units and
centre the actual bounds on (12,12), without adding a surrounding container.
Do not apply the functional stroke to filled trademarks or round away their
identifying geometry. Shared provider/maker identities use identical paths.
`compatible` depicts a plug and `generic` a neutral model cube; neither claims
to be a company logo.

The functional set is now original Zork artwork. Company logos remain
trademarks of their respective owners; their silhouettes are adapted from
the existing provider assets and the monochrome exports in
[@lobehub/lobe-icons](https://github.com/lobehub/lobe-icons/tree/master/packages/static-svg/icons).
Original third-party license files must remain. The new maker logo sources
are DeepSeek, Qwen, Zhipu, Doubao, Moonshot, MiniMax, Mistral, Meta and Google
exports from that collection; OpenAI, Anthropic and xAI reuse the provider
masters. Logo adaptation is limited to monochrome paint, path cleanup and
optical scaling; these are not newly invented logos.

Review the labelled light and dark sheets at native resolution: every master
appears at 14, 20 and 32 px. The before/after sheet preserves pre-redraw input
for representative comparisons. Runtime integration and native performance
are outside this asset-only review; no Rust application build is required.
