# AI tools

Generated from `tool_specs()` in `src/bin/ui/commands.rs`; `cargo test --bin ui ai_tools_doc` checks it. Regenerate with `cargo test --bin ui -- --ignored regenerate_ai_tools_doc`.

101 tools.

| Tool | What it does |
|------|--------------|
| `switch_mode` | Switch workspace mode. |
| `set_theme` | Dark or light appearance. |
| `set_ui_scale` | Make the whole interface bigger or smaller (saved for next time). |
| `toggle_panel` | Show or hide a panel. |
| `select_tool` | Pick a Paint tool. |
| `set_brush` | Brush parameters. Any subset. |
| `brush_stroke` | Stroke the current paint tool (brush, eraser, dodge, burn, clone, healing) through points on the selected layer. |
| `gradient` | Linear gradient from the brush colour to transparent on the selected layer, inside the selection. |
| `set_clone_source` | Clone stamp source point. |
| `add_layer` | New empty raster layer above the selected one. |
| `add_group` | New group (folder) containing the selected layer. |
| `duplicate_layer` | Duplicate a layer. |
| `remove_layer` | Delete a layer. |
| `move_layer` | Reorder a layer. |
| `select_layer` | Make a layer the active layer. |
| `rename_layer` | Rename a layer. |
| `set_layer_opacity` | Layer opacity in percent. |
| `set_layer_fill` | Layer fill in percent. |
| `set_layer_blend` | Layer blend mode. |
| `set_layer_visibility` | Show or hide a layer. |
| `set_layer_locked` | Lock or unlock a layer's pixels. |
| `offset_layer` | Move a layer's pixels by dx, dy. |
| `set_group_visibility` | Show or hide a layer group. |
| `add_mask` | Add a layer mask from the current selection (or reveal-all). |
| `set_mask` | Edit the layer mask. |
| `remove_mask` | Delete the layer mask. |
| `select_rect` | Rectangular marquee selection in image pixels. |
| `select_polygon` | Lasso/polygon selection. |
| `magic_wand` | Quick-select similar colours from a seed pixel. |
| `select_all` | Select the whole canvas. |
| `deselect` | Clear the selection. |
| `invert_selection` | Invert the selection. |
| `apply_filter` | Apply an adjustment to the selected layer (inside the selection). |
| `crop` | Crop the document. |
| `add_text` | Create a text layer. |
| `pen_add_point` | Append a point to the pen path. |
| `pen_stroke` | Stroke the pen path with the brush on the selected layer. |
| `pen_to_selection` | Turn the pen path into a selection. |
| `pen_clear` | Clear the pen path. |
| `undo` | Undo the last Paint edit. |
| `redo` | Redo. |
| `zoom` | Set Paint zoom. |
| `pan` | Pan the Paint view in screen points. |
| `open_file` | Open an image as a new layer, or read a PSD header (user approval required). |
| `save_psd` | Save the document as a layered PSD with pixels (user approval required). Without a path the user picks one in a Save dialog (suggesting <name>_edit.psd in Documents/Seraphina); relative paths go under Documents/Seraphina. |
| `save_project` | Save every mode (Paint, Animate, Composite, 3D, Materials, Stream settings, optionally this chat) as a .seraphina project (user approval required). Omit path to save to the current project or ask the user. |
| `open_project` | Open a .seraphina project, replacing the workspace; the user is asked about unsaved changes (user approval required). |
| `new_project` | Start a new project; the user is asked about unsaved changes (user approval required). |
| `export_png` | Export a flattened PNG (user approval required). Without a path the user picks one in a Save dialog; relative paths go under Documents/Seraphina. |
| `scene_add` | Add a 3D object. |
| `scene_select` | Select a 3D object by name. |
| `scene_gizmo` | 3D viewport drag mode. |
| `scene_transform` | Translate (relative), rotate Y (absolute degrees) or scale (absolute) a 3D object. |
| `scene_extrude` | Extrude the selected mesh's top face. |
| `scene_bevel` | Bevel (chamfer) the selected mesh's top face. |
| `scene_delete` | Delete a 3D object. |
| `scene_edge_loop` | Insert an edge loop through the quad ring crossing an edge (default: the most vertical quad edge). |
| `scene_multi_cut` | Split one face with a cut between two of its edges (default: top face, edges 0 and 2 at their midpoints). |
| `scene_bridge` | Merge another mesh into this one and bridge their two facing faces with quads. |
| `scene_boolean` | Boolean of two meshes (the result replaces `object`; `with` is removed). |
| `scene_quad_draw` | Add a quad (4 world-space points) to the quad-draw mesh, welding shared corners. |
| `scene_shading` | Viewport display mode. |
| `materials_select` | Show an element's detail card in the Materials Lab. |
| `materials_search` | Filter the periodic table. |
| `materials_add` | Add atoms of an element to the composition builder. |
| `materials_remove` | Remove one atom of an element from the composition builder. |
| `materials_clear` | Clear the composition builder. |
| `materials_build` | Build a material from a chemical formula (e.g. H2O, Fe2O3, Ca(OH)2, CuZn) or the builder's composition: checks nuclear stability and charge balance, derives PBR and physical properties. |
| `materials_build_atom` | Build an element from protons, neutrons and electrons (rejects ions and unstable nuclei). |
| `materials_preset` | Load a measured material preset. |
| `materials_save` | Save the built material to the library (MaterialRegistry). |
| `material_assign` | Assign a material to a 3D mesh object (e.g. 'make the cube copper'). The material may be a library name, a preset (gold, copper, iron, aluminum, silicon, glass, water, graphite, diamond) or a formula; it is saved to the library if new. |
| `arena_play` | Play or pause the physics arena: every visible 3D mesh becomes a rigid body (mass = material density × volume) under the arena environment (gravity, air drag, buoyancy, wind, heat exchange, phase changes). |
| `arena_step` | Advance the physics arena by fixed 1/240 s steps (paused). Runs over 240 steps go to a background thread; the next arena/scene command waits for them. |
| `arena_reset` | Stop the physics arena and restore the scene to how it was before the simulation. |
| `arena_env` | Set the arena environment: a preset and/or individual values. |
| `arena_body` | Physics settings of a 3D mesh: starting temperature, heater power (e.g. melt it), pinned, initial velocity. |
| `lab_tab` | Show a Materials Lab tab: elements (periodic table), reactions, circuits, computer, relativity. |
| `chem_react` | Run a chemical reaction: the equation is balanced exactly, thermochemistry (ΔH, ΔS, ΔG, K) comes from CODATA/NIST-JANAF/CRC data, and an adiabatic constant-pressure reactor integrates the published rate law. Reports conversion, final and adiabatic flame temperature. Omitted fields keep their current values. |
| `circuit_run` | Simulate a circuit by modified nodal analysis: divider (DC), rc / rlc (step transients), diode (I–V vs temperature), inverter (CMOS transfer curve), fuse (a real wire: R = ρL/A from the element, Joule heating, melts open), silicon (doped-Si resistivity). |
| `cpu_run` | Run a program on the simulated 4-bit computer built from transistor-characterised CMOS gates. The die temperature and clock change gate delays; too hot or too fast and it computes wrong answers. die_object binds the die temperature to a 3D arena body (heat it with arena_body heater_w); empty string unbinds. |
| `cpu_heat_sweep` | Heat the die from 27 °C until the program fails, and report why (timing, logic levels or intrinsic silicon). |
| `rel_speed` | Special-relativity readouts (γ, time dilation, length contraction, energy, Doppler) at a speed. |
| `nbody_preset` | Load a gravity scenario (Newtonian N-body; optional 1PN Schwarzschild GR correction). gr_exaggeration > 1 shrinks c as a visual aid so the precession is visible. |
| `nbody_play` | Play or pause the N-body simulation. |
| `nbody_measure_precession` | Simulate Mercury for 5 orbits with and without GR at real strength and report the perihelion advance in arcseconds per century. |
| `timeline_play` | Play or pause the current mode's timeline. |
| `timeline_scrub` | Move the playhead (seconds). |
| `set_keyframe` | Key the selected 3D object's Y / composite layer opacity / animate frame at the playhead. |
| `delete_keyframe` | Delete the key at the playhead. |
| `run_reader` | Run the bitstream reader over the loaded stream. |
| `reset_reader` | Rewind the bitstream reader. |
| `set_streaming` | Go live to the RTMP target from Settings via ffmpeg, or end the stream. Starting needs the user. |
| `set_recording` | Start/stop recording the canvas (or screen) to MP4 via ffmpeg. Starting needs the user. |
| `retry_stream` | Reconnect the live stream now (after a dropped connection). |
| `set_output_source` | What recording/streaming captures: the canvas, the screen, or a camera (webcam). |
| `set_stream_target` | RTMP server URL (the stream key is set by the user in Settings). |
| `generate_image` | Generate an image with Grok Imagine and add it to Paint as a new layer (user approval required; billed per image). |
| `video_generate` | Queue a Grok Imagine video generation (user approval required). |
| `video_extend` | Extend a generated clip (user approval required). |
| `video_stitch` | Write a concat list of clips (user approval required). |
