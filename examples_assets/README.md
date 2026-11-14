# Material examples

Valid material assets. Open a `.material` in the Material Graph Editor, or
copy one into your project (its folder, not just the JSON) to start from it.
This folder is read-only.

## Shader-graph materials (`.material` folders)

A `.material` is a folder holding `shader_graph_save.json`: the shader graph
(`main_graph`, nodes and connections) plus `shader_metadata`. Lines starting
with `//` before the JSON are a header the loader skips; each graph also has
comment boxes explaining its nodes (visible in the editor), and a `description`.

| Material | Teaches |
|---|---|
| `Checker.material` | The smallest procedural graph: UV into a checkerboard, two colour constants, a fragment output |
| `Marble.material` | Noise (`fbm_2d`), math nodes (multiply, sin, smoothstep), `color_lerp`, driving roughness and base colour from one mask |
| `Lava.material` | Animation with `time` and `panner`, `voronoi_2d`, vec3 component extraction, emissive output |
| `TexturedTint.material` | Sampling a texture by project-relative path (`textures/albedo.png`, not shipped: point it at your own image) |
| `TintedGlass.material` | A graph with a single constant: opacity below 1 makes the material translucent |

## Surface materials (`.mat` files)

A `.mat` is a scalar surface document (JSON, `version` 1): `base_color` (RGBA),
`metallic`, `roughness`, `emissive_color`, `emissive_intensity`, `alpha`. It is
the material asset that is not a graph; mesh slots use it.

| Material | Teaches |
|---|---|
| `Gold.mat` | A metal: `metallic` 1, tinted base colour |
| `Rubber.mat` | A rough dielectric |
| `NeonSign.mat` | Emission: dark base, bright `emissive_color` and `emissive_intensity` |
| `FrostedGlass.mat` | `alpha` below 1 selects the blended, transparent path |
