//! Crafting On A Stick for Horizon: workbenches carried as items. Using one opens its vanilla
//! menu where the player stands (`PlayerRef::open_station`); it stays open while the player
//! carries it. The crafting table keeps its grid in the item between uses, and the anvil wears
//! the item in place of a block.
//!
//! A port of OfekN's Crafting On A Stick (GPL-3.0), from its 1.21.0 sources.
use mod_api::{
    AnvilCraftPostEvent, BuildCreativeModeTabContentsEvent, ClientHost, Component, Compound,
    Hand, Hooks, Host, HostSide, HostTransaction, InteractionResult, ItemKey, ItemPart, ItemPlace,
    ItemResource, ItemStack, Registrar, RegistrationContext, ServerHost, StationClosingEvent, Tag,
    UseItemEvent,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

const ID: &str = "crafting_on_a_stick";

/// Each item, the vanilla menu it opens, and whether it keeps its input slots between uses.
const STATIONS: [(&str, &str); 9] = [
    ("crafting_table", "minecraft:crafting"),
    ("loom", "minecraft:loom"),
    ("grindstone", "minecraft:grindstone"),
    ("cartography_table", "minecraft:cartography_table"),
    ("stonecutter", "minecraft:stonecutter"),
    ("smithing_table", "minecraft:smithing"),
    ("anvil", "minecraft:anvil"),
    ("chipped_anvil", "minecraft:anvil"),
    ("damaged_anvil", "minecraft:anvil"),
];

/// The anvils, from whole to worn out: each wear takes one step.
const ANVILS: [&str; 3] = ["anvil", "chipped_anvil", "damaged_anvil"];

/// `CommonHooks.onAnvilRepair`'s default break chance.
const BREAK_CHANCE: f32 = 0.12;

/// The value of `key` in `compound`.
fn field<'a>(compound: &'a Compound, key: &str) -> Option<&'a Tag> {
    compound
        .iter()
        .find(|(k, _)| k.to_string_lossy() == key)
        .map(|(_, v)| v)
}

/// The stacks `stack` keeps in its `minecraft:container` component, by slot; empty when none.
fn stored(host: &dyn Host, stack: &ItemStack) -> Vec<Option<ItemStack>> {
    let mut slots = Vec::new();
    let Some(nbt) = stack.to_nbt(host) else {
        return slots;
    };
    let Some(Tag::Compound(components)) = field(&nbt, "components") else {
        return slots;
    };
    let Some(Tag::List(entries)) = field(components, "minecraft:container") else {
        return slots;
    };
    for entry in entries {
        let Tag::Compound(entry) = entry else {
            continue;
        };
        let (Some(Tag::Int(slot)), Some(Tag::Compound(item))) =
            (field(entry, "slot"), field(entry, "item"))
        else {
            continue;
        };
        let Ok(slot) = usize::try_from(*slot) else {
            continue;
        };
        if slots.len() <= slot {
            slots.resize(slot + 1, None);
        }
        slots[slot] = ItemStack::from_nbt(host, item);
    }
    slots
}

/// The `minecraft:container` component, as raw NBT.
struct Container;

impl Component for Container {
    type Value = Tag;
    fn id(&self) -> &str {
        "minecraft:container"
    }
    fn decode(&self, value: &Tag) -> Option<Tag> {
        Some(value.clone())
    }
    fn encode(&self, value: Tag) -> Tag {
        value
    }
}

/// `stack` keeping `slots` in its `minecraft:container` component, or without the component
/// when all are empty (`ItemStack.remove(DataComponents.CONTAINER)`).
fn with_stored(
    host: &dyn Host,
    stack: &ItemStack,
    slots: &[Option<ItemStack>],
) -> Option<ItemStack> {
    let mut entries = Vec::new();
    for (slot, item) in slots.iter().enumerate() {
        let Some(item) = item else { continue };
        let mut entry = Compound::new();
        entry.insert("slot".into(), Tag::Int(slot as i32));
        entry.insert("item".into(), Tag::Compound(item.to_nbt(host)?));
        entries.push(Tag::Compound(entry));
    }
    let mut changed = stack.clone();
    if entries.is_empty() {
        changed.remove(&Container);
    } else {
        changed.set(&Container, Tag::List(entries)).ok()?;
    }
    Some(changed)
}

/// `stack` as another item, its components kept (`ItemStack.transmuteCopy`).
fn transmute(host: &dyn Host, stack: &ItemStack, item: ItemKey) -> Option<ItemStack> {
    let mut nbt = stack.to_nbt(host)?;
    nbt.insert("id".into(), Tag::String(item.id(host).as_str().into()));
    ItemStack::from_nbt(host, &nbt)
}

/// `Item.canFitInsideContainerItems`: anything but a shulker box.
fn fits_inside_containers(host: &dyn Host, stack: &ItemStack) -> bool {
    !stack.item().id(host).ends_with("shulker_box")
}

/// Inventory slots by `ItemPlace::PlayerSlot`'s numbers: the main 36, the armour, the off hand
/// (`Player.getInventory`, as the mod searches it).
fn inventory_slots() -> impl Iterator<Item = i32> {
    (0..36).chain(100..=103).chain([-106])
}

/// What a player has open from this mod.
#[derive(Clone, Copy)]
struct Open {
    window: i32,
    /// The item it was opened with.
    item: ItemKey,
}

/// What the mod and its item parts share.
#[derive(Default)]
struct State {
    /// Item key by item path, from `register`.
    items: OnceLock<HashMap<&'static str, ItemKey>>,
    /// The station each player has open, by UUID.
    open: Mutex<HashMap<u128, Open>>,
    /// A small xorshift state for the anvil's wear roll.
    random: Mutex<u64>,
}

#[derive(Default)]
pub struct Mod {
    state: Arc<State>,
}

impl State {
    fn key(&self, path: &str) -> Option<ItemKey> {
        self.items.get()?.get(path).copied()
    }

    /// The item path of `key`, when it is one of this mod's.
    fn path(&self, key: ItemKey) -> Option<&'static str> {
        self.items
            .get()?
            .iter()
            .find(|(_, k)| **k == key)
            .map(|(p, _)| *p)
    }

    /// The items that keep a station of `path` open: the item itself, every anvil for an anvil.
    fn keep(&self, path: &str) -> Vec<ItemKey> {
        let family: &[&str] = if ANVILS.contains(&path) {
            &ANVILS
        } else {
            &[path]
        };
        family.iter().filter_map(|p| self.key(p)).collect()
    }

    fn roll(&self) -> f32 {
        let mut state = self.random.lock().unwrap();
        if *state == 0 {
            *state = 0x9E37_79B9_7F4A_7C15;
        }
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        (*state >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Opens the station of the item `stack` for `player`, the one in `place`: its stored grid
    /// goes into the menu and leaves the item.
    fn open(&self, host: &mut dyn ServerHost, player: u128, place: ItemPlace, stack: &ItemStack) {
        let Some(path) = self.path(stack.item()) else {
            return;
        };
        let Some((_, menu)) = STATIONS.iter().find(|(p, _)| *p == path) else {
            return;
        };
        let Some(menu_key) = host
            .menu_keys()
            .into_iter()
            .find(|key| key.id(host) == *menu)
        else {
            return;
        };
        let Some(handle) = host.player(player) else {
            return;
        };
        // What the crafting table kept from its last use.
        let stored = stored(host, stack);
        let inputs = if path == "crafting_table" {
            stored.clone()
        } else {
            Vec::new()
        };
        let Ok(Some(window)) = handle.open_station(host, menu_key, inputs, self.keep(path)) else {
            return;
        };
        if stored.iter().any(Option::is_some)
            && let Some(emptied) = with_stored(host, stack, &[])
        {
            let resource = ItemResource::of(&emptied);
            let _ = host.transaction(|host, tx| {
                if place.exchange(host, tx, &resource, 1) == 1 {
                    Ok(())
                } else {
                    Err(())
                }
            });
        }
        self.open.lock().unwrap().insert(
            player,
            Open {
                window,
                item: stack.item(),
            },
        );
    }

    /// The first stack of `item` in the player's inventory that `accept` takes, and its place.
    fn find(
        &self,
        host: &mut dyn ServerHost,
        player: u128,
        item: ItemKey,
        accept: impl Fn(&dyn Host, &ItemStack) -> bool,
    ) -> Option<(ItemPlace, ItemStack)> {
        inventory_slots().find_map(|slot| {
            let place = ItemPlace::PlayerSlot { player, slot };
            let stack = place.stack(host)?;
            (stack.item() == item && accept(host, &stack)).then_some((place, stack))
        })
    }
}

/// Using a station on a stick opens it.
struct Stick(Arc<State>);

#[mod_api::horizon_part]
impl ItemPart for Stick {
    fn use_item(
        &self,
        host: HostSide<'_>,
        event: &mut UseItemEvent<'_>,
    ) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return Some(InteractionResult::Consume);
        };
        // The slot itself, not the hand: a creative hand keeps what an exchange takes.
        let slot = match event.hand {
            Hand::Main => host.player(event.player)?.selected_slot(host)? as i32,
            Hand::Off => -106,
        };
        let place = ItemPlace::PlayerSlot {
            player: event.player,
            slot,
        };
        self.0.open(host, event.player, place, event.stack);
        Some(InteractionResult::SuccessServer)
    }
}

#[mod_api::horizon_mod]
impl Hooks for Mod {
    fn register(&self, registrar: &mut dyn Registrar, _: &RegistrationContext) {
        let mut items = HashMap::new();
        for (path, _) in STATIONS {
            let key = registrar.item_key(&format!("{ID}:{path}"));
            registrar.item_part(key, Stick(self.state.clone()));
            items.insert(path, key);
        }
        let _ = self.state.items.set(items);
    }

    /// `ClientModEvents.event`: every stick joins the tools tab, in the original's order.
    fn build_creative_mode_tab_contents(
        &self,
        _: &mut dyn ClientHost,
        event: &mut BuildCreativeModeTabContentsEvent<'_>,
    ) {
        if event.tab != "minecraft:tools_and_utilities" {
            return;
        }
        let Some(items) = self.state.items.get() else {
            return;
        };
        for (path, _) in STATIONS {
            if let Some(stack) = items.get(path).and_then(|&key| ItemStack::new(key, 1)) {
                event.items.push(stack);
            }
        }
    }

    /// `CraftingMenu.removed` of the mod: the grid goes into the first crafting table on a
    /// stick that holds none, unless nothing in it can go into a container item.
    fn station_closing(&self, host: &mut dyn ServerHost, event: &mut StationClosingEvent<'_>) {
        let Some(open) = self.state.open.lock().unwrap().remove(&event.player) else {
            return;
        };
        if open.window != event.window || self.state.path(open.item) != Some("crafting_table") {
            return;
        }
        let kept: Vec<Option<ItemStack>> = event
            .inputs
            .iter()
            .map(|slot| {
                slot.clone()
                    .filter(|stack| fits_inside_containers(host, stack))
            })
            .collect();
        if kept.iter().all(Option::is_none) {
            return;
        }
        let Some((place, stack)) = self
            .state
            .find(host, event.player, open.item, |host, stack| {
                stored(host, stack).iter().all(Option::is_none)
            })
        else {
            return;
        };
        let Some(filled) = with_stored(host, &stack, &kept) else {
            return;
        };
        let resource = ItemResource::of(&filled);
        let stored = host
            .transaction(|host, tx| {
                if place.exchange(host, tx, &resource, 1) == 1 {
                    Ok(())
                } else {
                    Err(())
                }
            })
            .is_ok();
        if stored {
            for (slot, kept) in event.inputs.iter_mut().zip(&kept) {
                if kept.is_some() {
                    *slot = None;
                }
            }
        }
    }

    /// `AnvilMenu.onTake` of the mod: the anvil on a stick wears in place of a block, one step
    /// in `BREAK_CHANCE`, and its sound plays where the player stands.
    fn anvil_craft_post(&self, host: &mut dyn ServerHost, event: &mut AnvilCraftPostEvent<'_>) {
        let Some(open) = self.state.open.lock().unwrap().get(&event.player).copied() else {
            return;
        };
        if !self
            .state
            .path(open.item)
            .is_some_and(|p| ANVILS.contains(&p))
        {
            return;
        }
        let creative = host
            .player(event.player)
            .and_then(|p| p.is_creative(host))
            .unwrap_or(false);
        let mut broke = false;
        if !creative && self.state.roll() < BREAK_CHANCE {
            // The most worn anvil goes first, as the mod searches.
            for (k, path) in ANVILS.iter().enumerate().rev() {
                let Some(key) = self.state.key(path) else {
                    continue;
                };
                let Some((place, stack)) = self.state.find(host, event.player, key, |_, _| true)
                else {
                    continue;
                };
                let next = ANVILS.get(k + 1).and_then(|p| self.state.key(p));
                let worn = next.and_then(|next| transmute(host, &stack, next));
                let _ = host.transaction(|host, tx| match &worn {
                    Some(worn) => {
                        let worn = ItemResource::of(worn);
                        if place.exchange(host, tx, &worn, 1) == 1 {
                            Ok(())
                        } else {
                            Err(())
                        }
                    }
                    None => {
                        let gone = place.extract(host, tx, &ItemResource::of(&stack), 1);
                        if gone == 1 { Ok(()) } else { Err(()) }
                    }
                });
                broke = next.is_none();
                break;
            }
        }
        if let Some(player) = host.player(event.player)
            && let Some(level) = player.level(host)
        {
            let _ = level.level_event(host, if broke { 1029 } else { 1030 }, event.pos, 0);
        }
    }
}
