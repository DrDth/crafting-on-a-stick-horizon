# Crafting On A Stick for Horizon

A port of [Crafting On A Stick](https://github.com/OfekN-mods/crafting-on-a-stick) by OfekN to [Horizon](https://github.com/horizon-rust/horizon), the Rust rewrite of Minecraft Java 26.3. It is a WebAssembly mod: the same Rust builds natively too.

Nine items carry a workbench: crafting table, loom, grindstone, cartography table, stonecutter, smithing table, and the anvil in its three states. Using one opens the vanilla menu where you stand, and it stays open while you carry the item. The crafting table keeps its grid in the item between uses. The anvil wears the item instead of a block: anvil, chipped, damaged, gone.

Not ported yet: the selection wheel and its key, the Curios slot (Horizon has no accessory slots), and the JEI catalysts.

## Building

The mod needs `PlayerRef::open_station` and `station_closing` (horizon-rust/horizon#18347). Until a Nightly has them, `Cargo.toml` points at a Horizon checkout of that branch.

## Credits and licence

The original mod, its textures, models, recipes and translations are OfekN's and its translators', under the GNU General Public License v3.0. The assets here come unchanged from its `26.2` branch; the Rust code is a port of its 1.21.0 sources. This port is under the same licence (`LICENSE`).
