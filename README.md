# Plugin_Materials

Materials: the material row and texture table every kind draws with, the PBR evaluation and the Radiant material templates.

`pulsar.materials` ("Materials") is a plugin of
[Pulsar-Native](https://github.com/Far-Beyond-Pulsar/Pulsar-Native), the engine and editor that renders with
[Helio](https://github.com/Far-Beyond-Pulsar/Helio). Pulsar-Native vendors this repository as a git submodule
at `plugins/vendor/materials`.

## What it owns

Every object kind is a plugin. A plugin owns its SceneDB components (their
data and scriptable methods; the editor calls the same methods scripts do),
the contracts other kinds read (its `api` crate: graph keys, row ABIs, WGSL
snippets), the Helio feature that renders it (passes, scene derivations,
settings), its file formats and its editor UI. Helio knows no object kind,
and other kinds meet this one only through frame contracts, extension points,
draw phases and api crates.

| Crate | Path | What |
|---|---|---|
| `helio-mats` | `api/` | The materials' public contract (pulsar.materials): the GPU material row and table, the material texture table and its binding modes, texture uploads, PBR evaluation and specular AA WGSL, and the Radiant material templates. The one crate another kind reads a material concept from. |
| `materials_component` | `component/` | Materials (pulsar.materials): the registrations of the plugin's Helio feature (the material texture table) and of its settings (the material sampler, from the project's texture streaming settings) |
| `materials_plugin` | `editor/` | Materials (pulsar.materials): the editor plugin -- its metadata, the material graph editor's registration, over the plugin's component crate |
| `helio-pass-materials` | `feature/` | The materials' Helio feature (pulsar.materials): the material texture table, built every frame from the scene's texture store and published for every kind's drawers |
| `materials_graph_editor` | `graph_editor/` | Materials (pulsar.materials): the material graph editor -- the node canvas, PSGC compilation, live previews and the .material asset's save/load (formerly Plugin_Shaders' shader_editor) |

- **Runtime module** (what a game built for a project links): `materials_component`.
- **Editor module** (what the editor links in addition): `materials_plugin`.
- **Requires**: nothing. A project enables or disables the plugin in its
  `Pulsar.toml` (`[plugins]`); disabled, the kind is absent and the frame
  draws without it.
- **Reads other kinds through**: none.
- **Its standalone hosts also compose**: none (demos and GPU tests
  draw a whole scene; the plugin itself does not depend on these).

## Building and testing

Like [Plugin_Physics](https://github.com/Far-Beyond-Pulsar/Plugin_Physics), this plugin's crates are members
of Pulsar-Native's workspace: their dependencies are `workspace = true`, and
Helio comes from Pulsar-Native's `crates/renderer/helio` submodule. It builds
only inside a Pulsar-Native checkout:

```bash
git clone --recursive https://github.com/Far-Beyond-Pulsar/Pulsar-Native
cd Pulsar-Native
cargo check -p helio-mats -p materials_component -p materials_plugin -p helio-pass-materials -p materials_graph_editor --tests
cargo test -p helio-mats -p materials_component -p helio-pass-materials
```

Suites that render real frames need a GPU; CI leaves them out by name
(Pulsar-Native's `.config/nextest.toml`, `ci` profile). This repository's CI
checks the plugin inside a Pulsar-Native checkout: rustfmt, clippy and the
`ci` profile's tests.

A change that spans this plugin and Pulsar-Native (or Helio) merges
dependency-first: merge here, re-pin `plugins/vendor/materials` in Pulsar-Native to
this repository's `main`, then merge the Pulsar-Native PR (Pulsar-Native's
`.agents/CRATES.md`, "Submodule pins").

## Architecture

- [Every object kind is a plugin](https://github.com/Far-Beyond-Pulsar/Helio/blob/main/docs/architecture/plugin-migration.md): the
  rule, what Helio is, the plugin table and the "As built" notes of each
  phase.
- [The layered rewrite](https://github.com/Far-Beyond-Pulsar/Helio/blob/main/docs/architecture/layered-rewrite.md): layers, buffer
  contracts, extension points and why the renderer has this shape.
- [The `helio-architecture` skill](https://github.com/Far-Beyond-Pulsar/Helio/blob/main/.agents/skills/helio-architecture/SKILL.md): required reading for any change
  here (see `AGENTS.md`).
- Pulsar-Native's `.agents/PLUGIN_SYSTEM.md` and `.agents/CRATES.md`: plugin
  descriptors, the installed set and the submodule pins.

History before 2026-11-02 (when the plugin moved into Pulsar-Native) is in
Helio's repository; this repository's history starts with that move
(`git subtree split` of `plugins/vendor/materials`).

## License

MIT, as the rest of Pulsar (`LICENSE.md`).
