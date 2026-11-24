# home-feeling

Extensible desktop tools for Windows and Linux, built on [bones](https://github.com/an-dr/bones). Each tool is a **Thing**: a Lua script you edit in any text editor and reload with one button. The first Thing is a tiling window manager for Windows.

## Status

Early development: the specification is written, the code is not. See [docs/index.md](docs/index.md) for use cases, architecture, and decisions.

## Build from source

```sh
git clone --recurse-submodules https://github.com/an-dr/home-feeling
cd home-feeling
cargo run -p home-feeling
```

Needs a Rust toolchain, a C compiler, CMake, and Ninja, because bones builds SDL3 from source. On Linux, run `cargo xtask linux-packages` inside `vendor/bones` to list the packages it also needs.
