# Weft

A voxel sandbox engine in Rust, built for speed and mods. The game itself is a mod
like any other only using public SDK.

- Native mods behind a stable C ABI (`crates/abi`), written with the safe `crates/sdk` wrapper
- Cubic 16³ chunks, unbounded in every direction; regions simulate in parallel
- Own render API, Vulkan backend
- Client/server split, peer-to-peer multiplayer

## Running

```
cargo build --workspace
cargo run
```

Mods are loaded from `mods/<id>/`; in development builds the library is picked up from
`target/<profile>/`.

## Development
Before starting working on a mod, check docs folder, there are plenty of examples on different systems and basic mod setup

## License - MPL-2.0
