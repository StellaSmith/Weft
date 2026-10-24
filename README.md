# Voxide

A voxel sandbox engine in Rust, built for speed and for mods. The game itself is a mod
like any other only using public SDK.

- Native mods behind a stable C ABI, written with the safe SDK wrapper
- Cubic 16³ chunks, unbounded in every direction; regions simulate in parallel
- 
- Own render API, Vulkan backend
- Client/server split, peer-to-peer multiplayer

## Running

```
cargo build --workspace
cargo run
```

Mods are loaded from `mods/<id>/`; in development builds the library is picked up from
`target/<profile>/`.

## License

MPL-2.0
