# Crafting On A Stick for Horizon

A port of [Crafting On A Stick](https://github.com/OfekN-mods/crafting-on-a-stick) by OfekN to [Horizon](https://github.com/horizon-rust/horizon), the Rust rewrite of Minecraft Java 26.3. It is a WebAssembly mod: the same Rust builds natively too.

Nine items carry a workbench: crafting table, loom, grindstone, cartography table, stonecutter, smithing table, and the anvil in its three states. Using one opens the vanilla menu where you stand, and it stays open while you carry the item. The crafting table keeps its grid in the item between uses. The anvil wears the item instead of a block: anvil, chipped, damaged, gone.

The sticks are in the Tools & Utilities tab, after vanilla's items, as in the original.

Not ported yet: the selection wheel and its key, the Curios slot (Horizon has no accessory slots), and the JEI catalysts.

## Building

The mod needs `PlayerRef::open_station` and `station_closing` (horizon-rust/horizon#18364), and its item names need horizon-rust/horizon#18460. Until a Nightly has both, build it inside a Horizon checkout that does, such as the `atm` branch of [DrDth/horizon](https://github.com/DrDth/horizon/tree/atm): `cargo horizon-mod` builds against a checkout only when the mod lies inside it.

```sh
git clone https://github.com/DrDth/crafting-on-a-stick-horizon <checkout>/.local/dev-mods/crafting_on_a_stick
cd <checkout>/.local/dev-mods/crafting_on_a_stick
cargo run --manifest-path ../../../Cargo.toml -p cargo-horizon-mod -- horizon-mod build --wasm
```

The package is `target/horizon-mod/crafting_on_a_stick.hmod`.

## Credits and licence

The original mod, its textures, models, recipes and translations are OfekN's and its translators', under the GNU General Public License v3.0. The assets here come unchanged from its `26.2` branch; the Rust code is a port of its 1.21.0 sources. This port is under the same licence (`LICENSE`).
