# Voxide

A voxel sandbox engine in Rust, built for speed and for mods. The game itself (`mods/base`) is a mod
like any other: it only uses the public SDK.

- Native mods behind a stable C ABI (`crates/abi`), written with the safe `crates/sdk` wrapper
- Cubic 16³ chunks, unbounded in every direction; regions simulate in parallel
- Own render API (`crates/render`), Vulkan backend in `backends/vulkan`
- Client/server split from the start, peer-to-peer multiplayer

## Running

```
cargo build --workspace
cargo run
```

Mods are loaded from `mods/<id>/`; in development builds the library is picked up from
`target/<profile>/`.

## License

MPL-2.0
