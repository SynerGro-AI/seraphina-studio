# Tool plan: Photoshop + After Effects + Maya → pure Rust

Goal: every capability of Photoshop, Adobe After Effects, and Maya that can be expressed as pure data/frame/math operations becomes a pure Rust module. Platform or GPU pieces come only when no pure path exists.

`[x]` means the feature is in the library (see the module list in
README.md) or in the app (`src/bin/ui`) and has a test. `[ ]` is
planned and not built yet (targets: M7+, one mode per milestone; see
README "Planned (not built)").

## Photoshop

### Adjustments
- [x] Levels
- [x] Curves
- [ ] Color balance
- [ ] Hue/saturation
- [ ] Brightness/contrast
- [ ] Exposure
- [x] Invert
- [ ] Posterize
- [ ] Threshold
- [ ] Vibrance
- [x] Black & white (grayscale filter)
- [ ] Photo filter
- [ ] Channel mixer
- [ ] Selective color
- [ ] Gradient map

### Filters
- [ ] Box blur
- [x] Gaussian blur
- [ ] Sharpen
- [ ] Add noise
- [ ] Motion blur
- [ ] Radial blur
- [ ] Median
- [ ] Emboss
- [ ] Find edges
- [ ] High pass

### Layer styles
- [ ] Drop shadow
- [ ] Inner glow
- [ ] Outer glow
- [ ] Bevel and emboss
- [ ] Stroke
- [ ] Gradient overlay
- [ ] Pattern overlay
- [ ] Satin

### Selections
- [x] Rectangular marquee
- [x] Lasso / polygon
- [x] Magic wand (tolerance)
- [x] Invert selection
- [ ] Expand / contract
- [ ] Save / load selection

### Layers
- [x] Layer stack
- [x] Hide / show
- [x] Opacity and fill
- [x] Blend modes
- [x] Layer mask
- [x] Group
- [ ] Clipping mask

### Tools
- [x] Brush / eraser
- [x] Clone stamp
- [x] Healing (spot heal)
- [x] Dodge / burn
- [x] Gradient
- [x] Text
- [x] Pen (to stroke or selection)
- [x] Crop
- [x] Zoom in / out
- [x] Hand / pan
- [ ] Sponge
- [ ] Shape
- [ ] Free transform (scale, rotate, skew, warp)

## After Effects

### Core
- [x] Timeline / clips
- [x] Compositor
- [x] Keyframes (opacity over time)
- [ ] Parenting
- [ ] Nulls
- [ ] Expressions

### Effects
- [ ] Color correction (Paint has levels, curves, grayscale, invert; Composite has none yet)
- [ ] Blur & sharpen
- [ ] Distort (wave, bulge, twirl)
- [ ] Generate (gradient, grid, lens flare)
- [ ] Keying (chroma, luma)
- [ ] Matte (track matte, set matte)
- [ ] Noise & grain
- [ ] Stylize (glow, find edges)
- [ ] Time (echo, posterize time)
- [ ] Transitions (fade, wipe)
- [ ] 3D channel (depth, fog)

### 3D
- [ ] 3D layers
- [ ] Camera
- [ ] Lights

## Maya

### Modeling
- [x] Polygon mesh (cube, plane, cylinder)
- [x] Extrude, bevel, bridge, edge loop, multi-cut, quad draw
- [x] Booleans (union, difference, intersection)
- [ ] NURBS curve / surface
- [ ] Subdivision

### Rigging
- [x] Joint hierarchy
- [ ] IK / FK
- [ ] Constraints
- [ ] Skinning weights

### Animation
- [x] Set / delete key, play
- [ ] Graph editor
- [ ] Motion paths
- [ ] Blend shapes

### Dynamics
- [x] Rigid bodies (the Physics Arena)
- [ ] Particles
- [ ] Soft body / cloth
- [ ] Fluids

### Look / render
- [x] Materials (Materials mode)
- [x] Shading: flat Lambert and Blinn-Phong
- [x] Lights and cameras as scene objects
- [x] Perspective view
- [ ] UV mapping
- [ ] Orthographic view

## Live weights

`live_weights` is a store of named float weights that can change
between frames; the terminal demo pipeline reads it each frame.
Planned: letting the AI set a weight once instead of on every frame.
