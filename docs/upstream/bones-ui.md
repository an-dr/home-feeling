# Upstream plan: bones UI

What home-feeling's Things window needs from `bones` (`vendor/bones`) and works around today. Nothing here changes the submodule; each item is a change to propose to `an-dr/bones`, after which the workaround is deleted.

## 1. Rounded corners of buttons look broken

**Symptom.** On the Things window, buttons and other rounded fills show a ragged, speckled outline at their corners, as if the corner arc were drawn with isolated dots. It shows at the display scale of a typical 1080p laptop; a screenshot is attached to the issue when it is filed.

**What is ruled out.**

- Vertex colours: `bones-module-ui/src/ui.rs` converts egui's premultiplied colours to straight alpha before they reach the renderer, and so does the texture upload (`output_translation.rs`), so a double-multiplied edge is not the cause.
- The look: `look/style.rs` gives every widget state the same `CornerRadius::same(look.corner)` and no stroke, so nothing in the look draws a dotted outline.

**Candidate causes, not yet confirmed.**

1. egui anti-aliases a rounded edge by feathering, a one-pixel fade of the vertex alpha. The renderer (`bones-module-renderer/src/inner.rs`, `draw_ui_mesh`) draws it with SDL's `render_geometry` on a texture with linear filtering, so a vertex sampling the font atlas's white pixel can pick up its neighbours at a fractional scale.
2. The mesh is tessellated at one pixels-per-point and drawn at another, so feathered edges land between device pixels.
3. SDL's software or GPU path rounds vertex positions in a way that breaks the thin feathered triangles at the arcs.

**Plan.**

1. Reproduce in bones' own `examples/ui_smoke.rs` at scale 1.0, 1.25, 1.5, and 2.0, and record which scales show it.
2. Capture the tessellated mesh at a corner and look at the vertices and the texture coordinates they sample.
3. Try, one at a time: nearest-neighbour filtering for the atlas texture, snapping the white-pixel coordinates to the pixel centre, and turning egui's `TessellationOptions::feathering` off.
4. Add a headless screenshot test that renders a button and compares its corner pixels with a stored image, so the fix cannot regress.
5. Open the pull request with the cause stated from step 2, not guessed.

## 2. A colour picker widget

**Gap.** The standard kinds have no colour input. The Things window builds one from a swatch (an uploaded 2 by 2 image per colour), a hex text field, twelve preset swatches, and hue, saturation, and brightness sliders. That costs sixteen widget ids per colour setting and an image upload per distinct colour.

**Proposal.** A `color-picker` kind with props `{ rgba: u32, alpha: bool }`, drawn with egui's colour button and popup, reporting a new event `ColorChanged { id, rgba }` (or `ValueChanged` with the packed value). It joins the wire format in `wit/wire-format.json` and the `kinds` tests.

**Then remove** `color.rs`, `swatch`, and `sync_swatches` from the Things window and the sixteen-id slots.

## 3. Tabs

**Gap.** There is no tab container. The Things window fakes one with a row of `Selectable` widgets and decides itself which content to build, and does the same for the sub-tabs of the settings.

**Proposal.** A `Layout::Tabs { selected }` container whose children are `Tab` nodes, each with a title and a body, and a `Selected { id, index }` event when the user picks one. The ui module keeps the selection per node id, as it does for `Scroll`. Nested tabs need no extra support.

**Then remove** the `Tab` enum, the section selection state, and the tab-row builders from the Things window.

## 4. Smaller gaps seen while building the page

- A slider with a unit or a value label of its own; today the label is a separate `Strong` above it.
- A way to give a `TextEdit` a validation state, such as a red outline, so a refused value is marked on the field and not by a line under it.
- A labelled row, a caption on the left and a control on the right, for settings lists.
